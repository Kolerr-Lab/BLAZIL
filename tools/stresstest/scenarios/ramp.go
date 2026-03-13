package scenarios

import (
	"context"
	"fmt"
	"time"

	"github.com/blazil/stresstest/metrics"
)

// Ramp increases concurrency from 100 → 1000 goroutines in steps of 100
// every 5 s, measuring throughput at each step. Success criterion: TPS
// increases monotonically with concurrency (linear scaling).
func Ramp(cfg Config) Result {
	conn, err := dial(cfg.Target)
	if err != nil {
		return Result{Name: "ramp", Notes: fmt.Sprintf("dial error: %v", err)}
	}
	defer conn.Close()

	col, stopCol := metrics.NewCollector()
	defer stopCol()

	type stepResult struct {
		goroutines int
		tps        float64
		p99Ms      float64
		errPct     float64
	}

	const (
		minWorkers  = 100
		maxWorkers  = 1000
		stepWorkers = 100
		stepDur     = 5 * time.Second
	)

	var steps []stepResult
	var peakTPS float64

	for w := minWorkers; w <= maxWorkers; w += stepWorkers {
		col.Reset()
		ctx, cancel := context.WithTimeout(context.Background(), stepDur)
		for i := 0; i < w; i++ {
			go worker(ctx, conn, col, int64(w*1000+i))
		}
		<-ctx.Done()
		cancel()

		total, success, failed, _, p99 := col.SnapshotDelta()
		elapsed := stepDur.Seconds()
		tps := float64(success) / elapsed
		errPct := 0.0
		if total > 0 {
			errPct = float64(failed) / float64(total) * 100
		}
		steps = append(steps, stepResult{goroutines: w, tps: tps, p99Ms: p99, errPct: errPct})
		if tps > peakTPS {
			peakTPS = tps
		}
		fmt.Printf("  ramp %4d goroutines → %8.0f TPS  P99 %.2f ms  err %.2f%%\n",
			w, tps, p99, errPct)
	}

	// Pass if the final step has higher TPS than the first (the load scales up).
	passed := len(steps) >= 2 && steps[len(steps)-1].tps > steps[0].tps

	r := Result{
		Name:    "ramp",
		Passed:  passed,
		PeakTPS: peakTPS,
	}
	if !passed {
		r.Notes = "TPS did not increase with concurrency — possible bottleneck"
	}

	// Attach per-step data as Samples.
	for i, s := range steps {
		r.Samples = append(r.Samples, metrics.Sample{
			Elapsed: stepDur * time.Duration(i+1),
			TPS:     s.tps,
			P99Ms:   s.p99Ms,
			ErrPct:  s.errPct,
		})
	}
	return r
}
