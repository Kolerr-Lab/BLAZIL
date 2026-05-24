// Package proxy implements a transparent gRPC reverse proxy.
//
// The gateway proxies gRPC calls without any knowledge of the upstream service's
// protobuf schema. This is achieved through rawBytesCodec, which passes message
// frames as raw []byte rather than decoding/re-encoding them through protobuf.
//
// Architecture:
//
//	Client → (gRPC :50050) → Gateway (auth+ratelimit+metering interceptors)
//	         → Director.Direct() → upstream *grpc.ClientConn
//	         → bidirectional stream proxy
//	         → upstream gRPC service (:50051…)
package proxy

import "fmt"

import "google.golang.org/grpc/encoding"

func init() {
	// Register the raw-bytes codec under the "proto" name so it overrides the
	// default protobuf codec. gRPC picks the codec by the Content-Type sub-type
	// ("proto" is the default for all gRPC calls). After this init() runs, all
	// messages on both server and client side pass through as raw []byte.
	encoding.RegisterCodec(rawBytesCodec{})
}

// rawBytesCodec passes gRPC message frames as raw []byte without any
// protobuf encoding or decoding. This lets the gateway proxy any gRPC
// service without importing its generated .pb.go stubs.
//
// Both the server and all upstream client connections must use this codec
// (set via grpc.ForceCodec on the server and grpc.WithDefaultCallOptions
// + grpc.ForceCodec on each client connection).
type rawBytesCodec struct{}

// Marshal returns b unchanged. v must be a []byte.
func (rawBytesCodec) Marshal(v interface{}) ([]byte, error) {
	b, ok := v.([]byte)
	if !ok {
		return nil, fmt.Errorf("proxy codec: marshal: expected []byte, got %T", v)
	}
	return b, nil
}

// Unmarshal writes data into *v. v must be a *[]byte.
func (rawBytesCodec) Unmarshal(data []byte, v interface{}) error {
	dest, ok := v.(*[]byte)
	if !ok {
		return fmt.Errorf("proxy codec: unmarshal: expected *[]byte, got %T", v)
	}
	*dest = data
	return nil
}

// Name returns the codec identifier used in gRPC Content-Type negotiation.
func (rawBytesCodec) Name() string { return "proto" } // must stay "proto" for gRPC wire compat

// Codec returns the singleton rawBytesCodec for use with grpc.ForceCodec.
func Codec() rawBytesCodec { return rawBytesCodec{} }
