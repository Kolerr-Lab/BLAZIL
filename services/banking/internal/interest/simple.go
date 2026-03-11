// Package interest provides interest calculation algorithms for the Blazil banking service.
package interest

import (
	"fmt"
	"math"
)

// SimpleInterestCalculator computes interest using the simple-interest formula:
//
//	I = P × r × t
//
// where r = annualRateBPS / 10000 and t = periods / periodsPerYear (assumed = 1
// here, so each period is treated as one full year-fraction of size 1/periods).
// In practice, each period carries rate = annualRateBPS / 10000 / periods.
//
// Total interest = P × (annualRateBPS / 10000) × (periods / periods) = P × r.
// To honour the periods argument, interest is applied once per period at
// rate/period, and the result is the cumulative total.
type SimpleInterestCalculator struct{}

// NewSimpleInterestCalculator returns a SimpleInterestCalculator.
func NewSimpleInterestCalculator() *SimpleInterestCalculator { return &SimpleInterestCalculator{} }

// Calculate implements InterestCalculator using simple interest.
// Each call to Calculate returns PrincipalMinorUnits × annualRateBPS / 10000.
// The periods parameter represents the number of annual periods (e.g. 1 = one year).
// For intra-year periods pass annualRateBPS proportioned externally.
func (c *SimpleInterestCalculator) Calculate(principalMinorUnits, annualRateBPS int64, periods int) (int64, error) {
	if principalMinorUnits < 0 {
		return 0, fmt.Errorf("interest: %w", ErrNegativePrincipal)
	}
	if annualRateBPS < 0 {
		return 0, fmt.Errorf("interest: %w", ErrNegativeRate)
	}
	if periods <= 0 {
		return 0, fmt.Errorf("interest: %w", ErrInvalidPeriods)
	}

	// I = P × r × n  where r = annualRateBPS / 10000
	interestFloat := float64(principalMinorUnits) * float64(annualRateBPS) / 10000.0 * float64(periods)
	return int64(math.Round(interestFloat)), nil
}

// ErrNegativePrincipal is returned when the principal is negative.
var ErrNegativePrincipal = fmt.Errorf("principal must be non-negative")

// ErrNegativeRate is returned when the annual rate is negative.
var ErrNegativeRate = fmt.Errorf("annual rate must be non-negative")

// ErrInvalidPeriods is returned when periods is less than 1.
var ErrInvalidPeriods = fmt.Errorf("periods must be >= 1")
