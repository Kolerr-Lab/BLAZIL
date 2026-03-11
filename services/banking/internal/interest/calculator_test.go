package interest_test

import (
	"errors"
	"testing"

	"github.com/blazil/banking/internal/interest"
)

// ─── Simple interest ──────────────────────────────────────────────────────────

func TestSimpleInterest_OneYear(t *testing.T) {
	calc := interest.NewSimpleInterestCalculator()
	// $1000.00 @ 5% for 1 year = $50.00 (5000 cents)
	got, err := calc.Calculate(100_000, 500, 1)
	if err != nil {
		t.Fatalf("Calculate: %v", err)
	}
	if got != 5_000 {
		t.Errorf("expected 5000, got %d", got)
	}
}

func TestSimpleInterest_TwoYears(t *testing.T) {
	calc := interest.NewSimpleInterestCalculator()
	// $1000.00 @ 5% for 2 years = $100.00 (10000 cents)
	got, err := calc.Calculate(100_000, 500, 2)
	if err != nil {
		t.Fatalf("Calculate: %v", err)
	}
	if got != 10_000 {
		t.Errorf("expected 10000, got %d", got)
	}
}

func TestSimpleInterest_ZeroRate(t *testing.T) {
	calc := interest.NewSimpleInterestCalculator()
	got, err := calc.Calculate(100_000, 0, 1)
	if err != nil {
		t.Fatalf("Calculate: %v", err)
	}
	if got != 0 {
		t.Errorf("expected 0, got %d", got)
	}
}

func TestSimpleInterest_NegativePrincipal(t *testing.T) {
	calc := interest.NewSimpleInterestCalculator()
	_, err := calc.Calculate(-1, 500, 1)
	if !errors.Is(err, interest.ErrNegativePrincipal) {
		t.Errorf("expected ErrNegativePrincipal, got %v", err)
	}
}

func TestSimpleInterest_InvalidPeriods(t *testing.T) {
	calc := interest.NewSimpleInterestCalculator()
	_, err := calc.Calculate(100_000, 500, 0)
	if !errors.Is(err, interest.ErrInvalidPeriods) {
		t.Errorf("expected ErrInvalidPeriods, got %v", err)
	}
}

// ─── Compound interest ────────────────────────────────────────────────────────

func TestCompoundInterest_OneYear(t *testing.T) {
	calc := interest.NewCompoundInterestCalculator()
	// $1000.00 @ 5% compounded for 1 year = $50.00 (same as simple for n=1)
	got, err := calc.Calculate(100_000, 500, 1)
	if err != nil {
		t.Fatalf("Calculate: %v", err)
	}
	if got != 5_000 {
		t.Errorf("expected 5000, got %d", got)
	}
}

func TestCompoundInterest_TwoYears_HigherThanSimple(t *testing.T) {
	cc := interest.NewCompoundInterestCalculator()
	sc := interest.NewSimpleInterestCalculator()
	compound, _ := cc.Calculate(100_000, 500, 2)
	simple, _ := sc.Calculate(100_000, 500, 2)
	// Compound must always be >= simple for positive principal & rate.
	if compound < simple {
		t.Errorf("compound(%d) should be >= simple(%d)", compound, simple)
	}
}

func TestCompoundInterest_ZeroRate(t *testing.T) {
	calc := interest.NewCompoundInterestCalculator()
	got, err := calc.Calculate(100_000, 0, 5)
	if err != nil {
		t.Fatalf("Calculate: %v", err)
	}
	if got != 0 {
		t.Errorf("expected 0 for zero rate, got %d", got)
	}
}

func TestCompoundInterest_NegativeRate(t *testing.T) {
	calc := interest.NewCompoundInterestCalculator()
	_, err := calc.Calculate(100_000, -100, 1)
	if !errors.Is(err, interest.ErrNegativeRate) {
		t.Errorf("expected ErrNegativeRate, got %v", err)
	}
}
