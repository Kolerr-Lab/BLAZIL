// Package scenarios contains load-test scenario implementations for Blazil.
// Each scenario accepts a Config and returns a Result that describes whether
// the service met its SLOs under the given load profile.
package scenarios

import (
	"context"
	"fmt"
	"sync/atomic"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/keepalive"
	"google.golang.org/protobuf/encoding/protowire"

	"github.com/blazil/stresstest/metrics"
)

// Config holds the runtime parameters shared across all scenarios.
type Config struct {
	// Target is the host:port of the payments gRPC service.
	Target string
	// Duration is how long the main measurement window runs.
	Duration time.Duration
	// SampleInterval is how often metrics are snapshotted (default 5 s).
	SampleInterval time.Duration
}

// Result captures the outcome of a single scenario run.
type Result struct {
	Name       string
	Passed     bool
	TotalReqs  int64
	Successes  int64
	Failures   int64
	PeakTPS    float64
	SustainTPS float64 // mean TPS over the steady-state window
	P99Ms      float64
	ErrPct     float64
	Samples    []metrics.Sample
	Notes      string
}

// ── raw-proto gRPC codec ──────────────────────────────────────────────────────

// rawProtoCodec passes []byte directly to the gRPC transport without
// re-encoding.  This lets stress-test workers encode once and reuse.
type rawProtoCodec struct{}

func (rawProtoCodec) Marshal(v interface{}) ([]byte, error) {
	if b, ok := v.([]byte); ok {
		return b, nil
	}
	return nil, fmt.Errorf("rawProtoCodec: expected []byte, got %T", v)
}

func (rawProtoCodec) Unmarshal(_ []byte, _ interface{}) error {
	return nil // Response bytes are intentionally discarded.
}

func (rawProtoCodec) Name() string { return "proto" }

// dial opens a single gRPC connection to target using the raw-proto codec.
func dial(target string) (*grpc.ClientConn, error) {
	//nolint:staticcheck
	conn, err := grpc.Dial(target,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithDefaultCallOptions(grpc.ForceCodec(rawProtoCodec{})),
		grpc.WithKeepaliveParams(keepalive.ClientParameters{
			Time:    10 * time.Second,
			Timeout: 5 * time.Second,
		}),
	)
	return conn, err
}

// poolSize is the number of shared gRPC connections each scenario maintains.
// gRPC multiplexes thousands of concurrent streams per connection, so a small
// pool supports hundreds of goroutines without triggering server-side
// connection limits.
const poolSize = 10

// Pool holds a fixed set of reusable gRPC connections.
type Pool [poolSize]*grpc.ClientConn

// dialPool creates poolSize connections to target, all using the raw-proto
// codec and keepalive tuned for sustained load.
func dialPool(target string) (Pool, error) {
	var p Pool
	for i := range p {
		//nolint:staticcheck
		conn, err := grpc.Dial(target,
			grpc.WithTransportCredentials(insecure.NewCredentials()),
			grpc.WithDefaultCallOptions(grpc.ForceCodec(rawProtoCodec{})),
			grpc.WithKeepaliveParams(keepalive.ClientParameters{
				Time:    10 * time.Second,
				Timeout: 5 * time.Second,
			}),
		)
		if err != nil {
			for j := 0; j < i; j++ {
				_ = p[j].Close()
			}
			return p, err
		}
		p[i] = conn
	}
	return p, nil
}

// get returns the connection that goroutine id should use.
func (p Pool) get(id int) *grpc.ClientConn { return p[id%poolSize] }

// close closes every connection in the pool.
func (p Pool) close() {
	for _, c := range p {
		if c != nil {
			_ = c.Close()
		}
	}
}

// encodePaymentRequest hand-encodes a ProcessPaymentRequest proto message
// using field numbers from the generated pb.go file:
//
//	1: idempotency_key (bytes)
//	2: debit_account_id (bytes)
//	3: credit_account_id (bytes)
//	4: amount_minor_units (varint)
//	5: currency_code (bytes)
//	6: ledger_id (varint)
//	7: metadata map<string,string> entry — key="reference", value="stress-test"
func encodePaymentRequest(idempotencyKey, debit, credit string, amount int64, currencyCode string, ledgerID uint32) []byte {
	b := make([]byte, 0, 256)

	b = protowire.AppendTag(b, 1, protowire.BytesType)
	b = protowire.AppendString(b, idempotencyKey)

	b = protowire.AppendTag(b, 2, protowire.BytesType)
	b = protowire.AppendString(b, debit)

	b = protowire.AppendTag(b, 3, protowire.BytesType)
	b = protowire.AppendString(b, credit)

	b = protowire.AppendTag(b, 4, protowire.VarintType)
	b = protowire.AppendVarint(b, uint64(amount))

	b = protowire.AppendTag(b, 5, protowire.BytesType)
	b = protowire.AppendString(b, currencyCode)

	b = protowire.AppendTag(b, 6, protowire.VarintType)
	b = protowire.AppendVarint(b, uint64(ledgerID))

	// Encode one map entry: field 7, message {key=1:"reference", value=2:"stress-test"}
	entry := make([]byte, 0, 40)
	entry = protowire.AppendTag(entry, 1, protowire.BytesType)
	entry = protowire.AppendString(entry, "reference")
	entry = protowire.AppendTag(entry, 2, protowire.BytesType)
	entry = protowire.AppendString(entry, "stress-test")
	b = protowire.AppendTag(b, 7, protowire.BytesType)
	b = protowire.AppendBytes(b, entry)

	return b
}

const paymentsMethodStream = "/payments.v1.PaymentsService/ProcessPaymentStream"

// WindowSize controls how many in-flight requests each worker maintains on its
// bidirectional stream. Higher values = better throughput but higher memory.
// Optimal range: 30-100 for maximum throughput (50,000+ TPS with 100 workers).
const WindowSize = 50

// worker uses gRPC bidirectional streaming for zero per-request RTT overhead.
// Each worker opens ONE persistent stream and maintains WindowSize in-flight
// requests. This architecture enables 50,000+ TPS with super batching:
//
//   - Stress test: 100 workers × 50 window = 5,000 concurrent requests
//   - Payments service: Processes stream continuously
//   - Engine: Batches transfers (100 per TigerBeetle commit)
//   - TigerBeetle: 1 VSR round for 100 transfers (~1-3ms)
//
// Total throughput: Limited by processing, not network RTT.
func worker(ctx context.Context, conn *grpc.ClientConn, col *metrics.Collector, workerID int64) {
	type inflight struct {
		start time.Time
		seq   int64
	}

	// Open bidirectional stream (ONE per worker, reused for all requests).
	stream, err := conn.NewStream(ctx, &grpc.StreamDesc{
		StreamName:    "ProcessPaymentStream",
		ServerStreams: true,
		ClientStreams: true,
	}, paymentsMethodStream)
	if err != nil {
		// Stream creation failed — record error and exit.
		col.Record(0, err)
		return
	}

	// Buffered channel acts as sliding window for flow control.
	// When full (WindowSize in-flight), sender blocks until response arrives.
	inflightCh := make(chan inflight, WindowSize)
	doneCh := make(chan struct{})
	var seq int64

	// Receiver goroutine: drains responses asynchronously.
	go func() {
		defer close(doneCh)
		for {
			var resp []byte
			err := stream.RecvMsg(&resp)
			if err != nil {
				// Stream closed or error — exit receiver.
				return
			}
			// Pop oldest in-flight request (FIFO order maintained by gRPC).
			select {
			case inf := <-inflightCh:
				ns := time.Since(inf.start).Nanoseconds()
				col.Record(ns, nil)
			case <-ctx.Done():
				return
			}
		}
	}()

	// Sender loop: fires requests continuously on stream without waiting.
	for {
		select {
		case <-ctx.Done():
			stream.CloseSend()
			<-doneCh // Wait for receiver to drain
			return
		default:
		}

		n := atomic.AddInt64(&seq, 1)
		key := fmt.Sprintf("st-%d-%d-%d", workerID, n, time.Now().UnixNano())
		payload := encodePaymentRequest(key,
			"ext-debit-acct-stress",
			"ext-credit-acct-stress",
			100,   // $1.00
			"USD",
			1,     // USD ledger
		)

		start := time.Now()

		// Send request on stream (non-blocking unless window is full).
		if err := stream.SendMsg(payload); err != nil {
			// Stream error — record and exit.
			col.Record(time.Since(start).Nanoseconds(), err)
			stream.CloseSend()
			<-doneCh
			return
		}

		// Push to in-flight queue (blocks if window is full = flow control).
		select {
		case inflightCh <- inflight{start: start, seq: n}:
			// Successfully queued.
		case <-ctx.Done():
			stream.CloseSend()
			<-doneCh
			return
		}
	}
}
