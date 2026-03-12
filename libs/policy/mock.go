package policy

import "context"

// MockPolicyEvaluator is a no-op evaluator for dev/demo and unit tests.
// allowAll=true  → Allow returns (true, nil) for every valid input.
// allowAll=false → Allow returns (false, ErrPolicyDenied).
type MockPolicyEvaluator struct {
	allowAll bool
}

// NewMockPolicyEvaluator creates a mock evaluator.
// Pass true for "allow all" (dev/demo default) or false for "deny all" (test deny cases).
func NewMockPolicyEvaluator(allowAll bool) *MockPolicyEvaluator {
	return &MockPolicyEvaluator{allowAll: allowAll}
}

func (m *MockPolicyEvaluator) Allow(_ context.Context, input PolicyInput) (bool, error) {
	if err := input.Validate(); err != nil {
		return false, err
	}
	if m.allowAll {
		return true, nil
	}
	return false, ErrPolicyDenied
}
