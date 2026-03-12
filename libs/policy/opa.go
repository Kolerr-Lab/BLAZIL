package policy

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"time"
)

// opaRequest wraps the input for the OPA REST API.
type opaRequest struct {
	Input PolicyInput `json:"input"`
}

// opaResponse is the OPA v1/data endpoint response.
type opaResponse struct {
	Result bool `json:"result"`
}

// OPAEvaluator calls the OPA REST API for policy decisions.
// Timeout: 100ms per spec (policy check must be fast).
type OPAEvaluator struct {
	opaURL     string
	httpClient *http.Client
}

// NewOPAEvaluator creates an OPAEvaluator reading OPA_URL from env.
// If OPA_URL is empty, returns MockPolicyEvaluator (allow all) for dev/demo.
func NewOPAEvaluator() PolicyEvaluator {
	url := os.Getenv("OPA_URL")
	if url == "" {
		return NewMockPolicyEvaluator(true)
	}
	return &OPAEvaluator{
		opaURL: url,
		httpClient: &http.Client{
			Timeout: 100 * time.Millisecond,
		},
	}
}

// NewOPAEvaluatorWithURL creates an OPAEvaluator with an explicit URL.
// Primarily for tests.
func NewOPAEvaluatorWithURL(url string) *OPAEvaluator {
	return &OPAEvaluator{
		opaURL: url,
		httpClient: &http.Client{
			Timeout: 100 * time.Millisecond,
		},
	}
}

func (o *OPAEvaluator) Allow(ctx context.Context, input PolicyInput) (bool, error) {
	body, err := json.Marshal(opaRequest{Input: input})
	if err != nil {
		return false, fmt.Errorf("marshal OPA request: %w", err)
	}

	url := o.opaURL + "/v1/data/blazil/allow"
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return false, fmt.Errorf("build OPA request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := o.httpClient.Do(req)
	if err != nil {
		return false, ErrPolicyUnavailable
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 256))
		return false, fmt.Errorf("OPA returned %d: %s", resp.StatusCode, b)
	}

	var result opaResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return false, fmt.Errorf("decode OPA response: %w", err)
	}
	return result.Result, nil
}
