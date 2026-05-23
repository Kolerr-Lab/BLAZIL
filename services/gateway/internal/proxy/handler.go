package proxy

import (
	"errors"
	"io"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

// Handler returns the gRPC UnknownServiceHandler that proxies every incoming
// stream to the appropriate upstream via the Director.
//
// Register it with grpc.UnknownServiceHandler(proxy.Handler(director)).
//
// Stream lifecycle:
//  1. Extract the full method name from the incoming stream context.
//  2. Ask the Director for an upstream connection and an outgoing context.
//  3. Open a bidirectional client stream to the upstream.
//  4. Copy client→upstream and upstream→client concurrently.
//  5. Propagate trailer metadata back to the client on completion.
func Handler(director *Director) grpc.StreamHandler {
	return func(_ interface{}, serverStream grpc.ServerStream) error {
		fullMethod, ok := grpc.Method(serverStream.Context())
		if !ok {
			return status.Error(codes.Internal, "gateway: missing method in stream context")
		}

		outCtx, conn, err := director.Direct(serverStream.Context(), fullMethod)
		if err != nil {
			return err
		}

		// Open a fully bidirectional stream to the upstream.
		// We declare both ServerStreams and ClientStreams true to cover all RPC types
		// (unary is just a special case of bidi with one message each way).
		clientStream, err := conn.NewStream(outCtx, &grpc.StreamDesc{
			ServerStreams: true,
			ClientStreams: true,
		}, fullMethod)
		if err != nil {
			return status.Errorf(codes.Unavailable,
				"gateway: upstream stream open failed: %v", err)
		}

		// ── Forward: downstream client → upstream ────────────────────────────
		// Run in a separate goroutine so both directions proceed concurrently.
		clientToUpstreamErr := make(chan error, 1)
		go func() {
			clientToUpstreamErr <- copyClientToUpstream(serverStream, clientStream)
		}()

		// ── Forward: upstream → downstream client ────────────────────────────
		// This blocks until the upstream closes its send side.
		upstreamToClientErr := copyUpstreamToClient(clientStream, serverStream)

		// Wait for the client→upstream goroutine to finish.
		if err := <-clientToUpstreamErr; err != nil && !errors.Is(err, io.EOF) {
			return err
		}
		return upstreamToClientErr
	}
}

// copyClientToUpstream reads frames from the incoming client stream and sends
// them to the upstream client stream. Returns io.EOF on clean client close.
func copyClientToUpstream(src grpc.ServerStream, dst grpc.ClientStream) error {
	for {
		var frame []byte
		if err := src.RecvMsg(&frame); err != nil {
			// Signal the upstream that the client is done sending.
			_ = dst.CloseSend()
			return err // io.EOF on clean close
		}
		if err := dst.SendMsg(frame); err != nil {
			return err
		}
	}
}

// copyUpstreamToClient reads frames from the upstream client stream and sends
// them back to the downstream client. Forwards response headers before the
// first message and trailers after the last message.
func copyUpstreamToClient(src grpc.ClientStream, dst grpc.ServerStream) error {
	// Forward response headers from the upstream (e.g. grpc-status, trace IDs).
	if header, err := src.Header(); err == nil && len(header) > 0 {
		if setErr := dst.SetHeader(header); setErr != nil {
			return setErr
		}
	}
	for {
		var frame []byte
		if err := src.RecvMsg(&frame); err != nil {
			// Propagate trailer metadata (gRPC status code lives here).
			dst.SetTrailer(src.Trailer())
			if errors.Is(err, io.EOF) {
				return nil // clean upstream close
			}
			return err
		}
		if err := dst.SendMsg(frame); err != nil {
			return err
		}
	}
}
