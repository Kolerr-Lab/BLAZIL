package policy_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"github.com/blazil/policy"
)

func TestMockPolicy_Allow(t *testing.T) {
	ev := policy.NewMockPolicyEvaluator(true)
	input := policy.PolicyInput{
		Subject:  "user-1",
		Action:   "payment:write",
		Roles:    []string{"payment:write"},
		Amount:   100,
		Currency: "USD",
	}
	ok, err := ev.Allow(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !ok {
		t.Fatal("expected Allow=true, got false")
	}
}

func TestMockPolicy_Deny(t *testing.T) {
	ev := policy.NewMockPolicyEvaluator(false)
	input := policy.PolicyInput{
		Subject: "user-2",
		Action:  "order:place",
	}
	ok, err := ev.Allow(context.Background(), input)
	if ok {
		t.Fatal("expected Allow=false, got true")
	}
	if err != policy.ErrPolicyDenied {
		t.Fatalf("expected ErrPolicyDenied, got %v", err)
	}
}

func TestOPAEvaluator_FallbackToMock(t *testing.T) {
	// When OPA_URL is unset, NewOPAEvaluator returns a mock (allow all)
	os.Unsetenv("OPA_URL")
	ev := policy.NewOPAEvaluator()
	input := policy.PolicyInput{Subject: "user-3", Action: "balance:read"}
	ok, err := ev.Allow(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error falling back to mock: %v", err)
	}
	if !ok {
		t.Fatal("expected mock allow=true")
	}
}

func TestOPAEvaluator_HTTPCall(t *testing.T) {
	// Verify OPAEvaluator correctly POSTs to /v1/data/blazil/allow
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "want POST", http.StatusMethodNotAllowed)
			return
		}
		if r.URL.Path != "/v1/data/blazil/allow" {
			http.Error(w, "wrong path", http.StatusNotFound)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]bool{"result": true})
	}))
	defer ts.Close()

	ev := policy.NewOPAEvaluatorWithURL(ts.URL)
	input := policy.PolicyInput{Subject: "user-4", Action: "payment:write"}
	ok, err := ev.Allow(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !ok {
		t.Fatal("expected allow=true from mock OPA server")
	}
}

func TestPolicyInput_Validation(t *testing.T) {
	tests := []struct {
		name    string
		input   policy.PolicyInput
		wantErr bool
	}{
		{
			name:    "missing Subject",
			input:   policy.PolicyInput{Action: "payment:write"},
			wantErr: true,
		},
		{
			name:    "missing Action",
			input:   policy.PolicyInput{Subject: "user-1"},
			wantErr: true,
		},
		{
			name:    "valid",
			input:   policy.PolicyInput{Subject: "user-1", Action: "payment:write"},
			wantErr: false,
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			err := tc.input.Validate()
			if (err != nil) != tc.wantErr {
				t.Errorf("Validate() error = %v, wantErr %v", err, tc.wantErr)
			}
		})
	}
}
