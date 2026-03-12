// Package policy provides OPA-backed authorization evaluation for Blazil.
//
// Dev mode: If OPA_URL is empty, MockPolicyEvaluator is used (allow all).
// Production mode: OPAEvaluator calls POST /v1/data/blazil/allow with 100ms timeout.
package policy

import (
	"context"
	"errors"
)

// ErrPolicyDenied is returned when OPA evaluates the input as deny.
var ErrPolicyDenied = errors.New("policy denied")

// ErrPolicyUnavailable is returned when OPA cannot be reached.
var ErrPolicyUnavailable = errors.New("policy evaluator unavailable")

// PolicyInput is the input document sent to OPA for evaluation.
type PolicyInput struct {
	Subject       string   `json:"subject"`        // user ID from JWT Claims
	Action        string   `json:"action"`         // "payment:write", "order:place", etc.
	Resource      string   `json:"resource"`       // "payment/123", "order/456"
	ResourceOwner string   `json:"resource_owner"` // owner ID for ownership checks
	Roles         []string `json:"roles"`          // JWT roles
	Amount        int64    `json:"amount"`         // for amount-based rules (minor units)
	Currency      string   `json:"currency"`
}

// Validate checks required fields on a PolicyInput.
func (p PolicyInput) Validate() error {
	if p.Subject == "" {
		return errors.New("PolicyInput: Subject is required")
	}
	if p.Action == "" {
		return errors.New("PolicyInput: Action is required")
	}
	return nil
}

// PolicyEvaluator is the interface for authorization policy evaluation.
type PolicyEvaluator interface {
	// Allow returns (true, nil) if the action is permitted.
	// Returns (false, nil) if denied. Returns (false, err) on evaluation failure.
	Allow(ctx context.Context, input PolicyInput) (bool, error)
}
