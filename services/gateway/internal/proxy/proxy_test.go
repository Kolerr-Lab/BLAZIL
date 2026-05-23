package proxy_test

import (
	"testing"

	"github.com/blazil/services/gateway/internal/proxy"
)

// ── Codec ──────────────────────────────────────────────────────────────────

func TestCodec_RoundTrip(t *testing.T) {
	c := proxy.Codec()

	original := []byte("blazil-gateway-proxy-test-frame")
	encoded, err := c.Marshal(original)
	if err != nil {
		t.Fatalf("Marshal: %v", err)
	}
	var decoded []byte
	if err := c.Unmarshal(encoded, &decoded); err != nil {
		t.Fatalf("Unmarshal: %v", err)
	}
	if string(decoded) != string(original) {
		t.Errorf("round-trip mismatch: want %q, got %q", original, decoded)
	}
}

func TestCodec_MarshalRejectsNonBytes(t *testing.T) {
	c := proxy.Codec()
	if _, err := c.Marshal("not a byte slice"); err == nil {
		t.Error("expected error marshalling non-[]byte value")
	}
}

func TestCodec_UnmarshalRejectsNonPointer(t *testing.T) {
	c := proxy.Codec()
	var notPointer []byte
	if err := c.Unmarshal([]byte("data"), notPointer); err == nil {
		t.Error("expected error unmarshalling into non-*[]byte")
	}
}

func TestCodec_NameIsProto(t *testing.T) {
	// The codec name MUST be "proto" for gRPC Content-Type wire compatibility.
	if proxy.Codec().Name() != "proto" {
		t.Errorf("codec name: want %q, got %q", "proto", proxy.Codec().Name())
	}
}

// ── Director routing ──────────────────────────────────────────────────────

func TestDirector_RouteFor(t *testing.T) {
	tests := []struct {
		name       string
		routes     []proxy.Route
		fullMethod string
		wantAddr   string
	}{
		{
			name: "payments service",
			routes: []proxy.Route{
				{ServicePrefix: "payments.v1", UpstreamAddr: ":50051"},
				{ServicePrefix: "banking.v1", UpstreamAddr: ":50052"},
			},
			fullMethod: "/payments.v1.PaymentsService/SubmitTransaction",
			wantAddr:   ":50051",
		},
		{
			name: "banking service",
			routes: []proxy.Route{
				{ServicePrefix: "payments.v1", UpstreamAddr: ":50051"},
				{ServicePrefix: "banking.v1", UpstreamAddr: ":50052"},
			},
			fullMethod: "/banking.v1.BankingService/Transfer",
			wantAddr:   ":50052",
		},
		{
			name: "first matching prefix wins",
			routes: []proxy.Route{
				{ServicePrefix: "pay", UpstreamAddr: ":9000"},
				{ServicePrefix: "payments.v1", UpstreamAddr: ":50051"},
			},
			fullMethod: "/payments.v1.PaymentsService/Submit",
			wantAddr:   ":9000",
		},
		{
			name: "unknown service returns empty",
			routes: []proxy.Route{
				{ServicePrefix: "payments.v1", UpstreamAddr: ":50051"},
			},
			fullMethod: "/unknown.v1.Service/Method",
			wantAddr:   "",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			d := proxy.NewDirectorForTest(tc.routes)
			got := d.RouteForTest(tc.fullMethod)
			if got != tc.wantAddr {
				t.Errorf("routeFor(%q): want %q, got %q", tc.fullMethod, tc.wantAddr, got)
			}
		})
	}
}
