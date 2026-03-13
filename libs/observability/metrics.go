// Package observability provides shared metrics, tracing, logging, and
// gRPC middleware for all Blazil Go services.
package observability

import (
	"errors"
	"net/http"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

// ── Metric definitions ────────────────────────────────────────────────────────

// TransactionsTotal counts transactions processed across all services.
var TransactionsTotal = prometheus.NewCounterVec(
	prometheus.CounterOpts{
		Name: "blazil_transactions_total",
		Help: "Total transactions processed",
	},
	[]string{"service", "status", "rails"},
)

// TransactionDuration records the duration of transaction processing.
var TransactionDuration = prometheus.NewHistogramVec(
	prometheus.HistogramOpts{
		Name:    "blazil_transaction_duration_seconds",
		Help:    "Transaction processing duration",
		Buckets: []float64{0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0},
	},
	[]string{"service", "operation"},
)

// RingBufferUtilization tracks ring buffer fill ratio (0.0–1.0).
var RingBufferUtilization = prometheus.NewGaugeVec(
	prometheus.GaugeOpts{
		Name: "blazil_ring_buffer_utilization_ratio",
		Help: "Ring buffer fill ratio 0.0-1.0",
	},
	[]string{"instance"},
)

// GRPCRequestsTotal counts gRPC requests by service, method, and status code.
var GRPCRequestsTotal = prometheus.NewCounterVec(
	prometheus.CounterOpts{
		Name: "blazil_grpc_requests_total",
		Help: "Total gRPC requests",
	},
	[]string{"service", "method", "code"},
)

// GRPCRequestDuration records the duration of gRPC requests.
var GRPCRequestDuration = prometheus.NewHistogramVec(
	prometheus.HistogramOpts{
		Name:    "blazil_grpc_request_duration_seconds",
		Help:    "gRPC request duration",
		Buckets: prometheus.DefBuckets,
	},
	[]string{"service", "method"},
)
// OrderBookDepth tracks the number of orders in each side of the order book.
var OrderBookDepth = prometheus.NewGaugeVec(
	prometheus.GaugeOpts{
		Name: "blazil_orderbook_depth",
		Help: "Number of orders in book",
	},
	[]string{"instrument", "side"},
)

// DepositsTotal counts crypto deposits by chain and status.
var DepositsTotal = prometheus.NewCounterVec(
	prometheus.CounterOpts{
		Name: "blazil_deposits_total",
		Help: "Total deposits processed",
	},
	[]string{"chain", "status"},
)

// WithdrawalsTotal counts crypto withdrawals by chain and status.
var WithdrawalsTotal = prometheus.NewCounterVec(
	prometheus.CounterOpts{
		Name: "blazil_withdrawals_total",
		Help: "Total withdrawals processed",
	},
	[]string{"chain", "status"},
)

// CrossShardTotal counts cross-shard transfer executions by outcome.
// Labels: status — "success", "failed", "voided".
var CrossShardTotal = prometheus.NewCounterVec(
	prometheus.CounterOpts{
		Name: "blazil_cross_shard_total",
		Help: "Total cross-shard transfers executed",
	},
	[]string{"status"},
)

// RegisterAll registers all Blazil metrics with the given registerer.
// Duplicate registrations (prometheus.AlreadyRegisteredError) are silently
// ignored so that tests can call RegisterAll multiple times without panicking.
func RegisterAll(reg prometheus.Registerer) error {
	collectors := []prometheus.Collector{
		TransactionsTotal,
		TransactionDuration,
		RingBufferUtilization,
		GRPCRequestsTotal,
		GRPCRequestDuration,
		OrderBookDepth,
		DepositsTotal,
		WithdrawalsTotal,
		CrossShardTotal,
	}
	for _, c := range collectors {
		if err := reg.Register(c); err != nil {
			var are prometheus.AlreadyRegisteredError
			if !errors.As(err, &are) {
				return err
			}
		}
	}
	return nil
}

// MetricsHandler returns an HTTP handler that exposes the default Prometheus
// metrics in text format. Wire this in a separate HTTP server; never on the
// gRPC port.
func MetricsHandler() http.Handler {
	return promhttp.Handler()
}
