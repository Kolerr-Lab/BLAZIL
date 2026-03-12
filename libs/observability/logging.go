package observability

import (
	"context"

	"go.opentelemetry.io/otel/trace"
	"go.uber.org/zap"
	"go.uber.org/zap/zapcore"
)

// NewLogger creates a structured zap logger for the given service.
// In "production" (or any non-"development" env) it emits JSON to stdout.
// In "development" it uses the human-readable console encoder.
// The logger always carries service and version fields.
func NewLogger(serviceName, env string) *zap.Logger {
	var cfg zap.Config
	if env == "development" {
		cfg = zap.NewDevelopmentConfig()
	} else {
		cfg = zap.NewProductionConfig()
		cfg.EncoderConfig.EncodeTime = zapcore.ISO8601TimeEncoder
	}

	logger, err := cfg.Build(
		zap.Fields(
			zap.String("service", serviceName),
			zap.String("version", "0.1.0"),
		),
	)
	if err != nil {
		// Fallback: a no-op logger that never panics.
		return zap.NewNop()
	}
	return logger
}

// WithTraceID returns a child logger that carries the OTel trace ID extracted
// from ctx. When no trace is active, it returns the original logger unchanged.
func WithTraceID(logger *zap.Logger, ctx context.Context) *zap.Logger {
	span := trace.SpanFromContext(ctx)
	if !span.SpanContext().IsValid() {
		return logger
	}
	return logger.With(zap.String("trace_id", span.SpanContext().TraceID().String()))
}
