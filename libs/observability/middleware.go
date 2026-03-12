package observability

import (
	"context"
	"time"

	"go.opentelemetry.io/otel"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

// metadataCarrier adapts gRPC metadata to the OTel TextMapCarrier interface.
type metadataCarrier metadata.MD

func (c metadataCarrier) Get(key string) string {
	vals := metadata.MD(c).Get(key)
	if len(vals) == 0 {
		return ""
	}
	return vals[0]
}

func (c metadataCarrier) Set(key, val string) {
	metadata.MD(c).Set(key, val)
}

func (c metadataCarrier) Keys() []string {
	keys := make([]string, 0, len(c))
	for k := range c {
		keys = append(keys, k)
	}
	return keys
}

// UnaryServerInterceptor returns a gRPC server interceptor that:
//   - Extracts OTel trace context from incoming metadata
//   - Starts a span for the RPC
//   - Records GRPCRequestsTotal and GRPCRequestDuration
//   - Logs the request with zap, including trace_id
func UnaryServerInterceptor(serviceName string) grpc.UnaryServerInterceptor {
	logger := NewLogger(serviceName, "production")
	propagator := otel.GetTextMapPropagator()

	return func(
		ctx context.Context,
		req interface{},
		info *grpc.UnaryServerInfo,
		handler grpc.UnaryHandler,
	) (interface{}, error) {
		// Extract OTel context from incoming metadata.
		md, _ := metadata.FromIncomingContext(ctx)
		ctx = propagator.Extract(ctx, metadataCarrier(md))

		// Start span.
		ctx, span := otel.Tracer("blazil").Start(ctx, info.FullMethod)
		defer span.End()

		start := time.Now()
		resp, err := handler(ctx, req)
		elapsed := time.Since(start).Seconds()

		code := status.Code(err).String()
		GRPCRequestsTotal.WithLabelValues(serviceName, info.FullMethod, code).Inc()
		GRPCRequestDuration.WithLabelValues(serviceName, info.FullMethod).Observe(elapsed)

		log := WithTraceID(logger, ctx)
		if err != nil {
			log.Error("gRPC request failed",
				zap.String("method", info.FullMethod),
				zap.String("code", code),
				zap.Duration("duration", time.Since(start)),
				zap.Error(err),
			)
		} else {
			log.Info("gRPC request completed",
				zap.String("method", info.FullMethod),
				zap.String("code", code),
				zap.Duration("duration", time.Since(start)),
			)
		}

		return resp, err
	}
}

// UnaryClientInterceptor returns a gRPC client interceptor that injects the
// current OTel trace context into outgoing call metadata.
func UnaryClientInterceptor() grpc.UnaryClientInterceptor {
	propagator := otel.GetTextMapPropagator()

	return func(
		ctx context.Context,
		method string,
		req, reply interface{},
		cc *grpc.ClientConn,
		invoker grpc.UnaryInvoker,
		opts ...grpc.CallOption,
	) error {
		md, ok := metadata.FromOutgoingContext(ctx)
		if !ok {
			md = metadata.New(nil)
		}
		propagator.Inject(ctx, metadataCarrier(md))
		ctx = metadata.NewOutgoingContext(ctx, md)
		return invoker(ctx, method, req, reply, cc, opts...)
	}
}
