// Package interest provides interest calculation algorithms for the Blazil banking service.
package interest

import (
	"fmt"
	"math"
)

// CompoundInterestCalculator computes interest using the compound-interest formula:
//
//	A = P × (1 + r)^n   →   I = A - P
//
// where r = annualRateBPS / 10000 and n = periods.
// Each period is treated as one compounding interval (e.g. monthly periods
// require annualRateBPS / 12 to be passed as rate per period — callers are
// responsible for adjusting the BPS before calling if sub-annual compounding
// is required; by default each period is one year).
type CompoundInterestCalculator struct{}

// NewCompoundInterestCalculator returns a CompoundInterestCalculator.
func NewCompoundInterestCalculator() *CompoundInterestCalculator {
	return &CompoundInterestCalculator{}
}

// Calculate implements InterestCalculator using compound interest.
// Returns the total interest earned over n periods, rounded to the nearest
// minor unit.
func (c *CompoundInterestCalculator) Calculate(principalMinorUnits, annualRateBPS int64, periods int) (int64, error) {
	if principalMinorUnits < 0 {
		return 0, fmt.Errorf("interest: %w", ErrNegativePrincipal)
	}
	if annualRateBPS < 0 {
		return 0, fmt.Errorf("interest: %w", ErrNegativeRate)
	}
	if periods <= 0 {
		return 0, fmt.Errorf("interest: %w", ErrInvalidPeriods)
	}

	r := float64(annualRateBPS) / 10000.0
	// A = P × (1 + r)^n
	finalAmount := float64(principalMinorUnits) * math.Pow(1+r, float64(periods))
	interest := finalAmount - float64(principalMinorUnits)
	return int64(math.Round(interest)), nil
}
