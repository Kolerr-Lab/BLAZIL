package observability

import (
	"context"
	"fmt"
	"os"
	"strings"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracegrpc"
	"go.opentelemetry.io/otel/propagation"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
	"go.opentelemetry.io/otel/trace/noop"
	"google.golang.org/grpc/credentials"
)

// InitTracer initialises the global OpenTelemetry tracer provider.
//
// When endpoint is empty, a no-op provider is installed so the service starts
// without an OTel collector. The returned shutdown function must be deferred by
// the caller to flush remaining spans.
//
// TLS behaviour:
//   - Uses TLS by default (system root CAs).
//   - Set OTEL_EXPORTER_OTLP_INSECURE=true ONLY in development/CI environments
//     where the collector is on localhost without a certificate.
func InitTracer(serviceName, endpoint string) (shutdown func(), err error) {
	if endpoint == "" {
		otel.SetTracerProvider(noop.NewTracerProvider())
		otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(
			propagation.TraceContext{},
			propagation.Baggage{},
		))
		return func() {}, nil
	}

	insecure := strings.EqualFold(os.Getenv("OTEL_EXPORTER_OTLP_INSECURE"), "true")

	opts := []otlptracegrpc.Option{
		otlptracegrpc.WithEndpoint(endpoint),
	}
	if insecure {
		opts = append(opts, otlptracegrpc.WithInsecure())
	} else {
		opts = append(opts, otlptracegrpc.WithTLSCredentials(credentials.NewClientTLSFromCert(nil, "")))
	}

	exporter, err := otlptracegrpc.New(context.Background(), opts...)
	if err != nil {
		return nil, fmt.Errorf("create OTLP exporter: %w", err)
	}

	tp := sdktrace.NewTracerProvider(
		sdktrace.WithBatcher(exporter),
		sdktrace.WithSampler(sdktrace.AlwaysSample()),
	)

	otel.SetTracerProvider(tp)
	otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(
		propagation.TraceContext{},
		propagation.Baggage{},
	))

	return func() { _ = tp.Shutdown(context.Background()) }, nil
}

// TraceTransaction starts a new span for a Blazil transaction operation.
// The caller is responsible for calling span.End().
func TraceTransaction(ctx context.Context, operation string) (context.Context, trace.Span) {
	return otel.Tracer("blazil").Start(ctx, operation)
}
