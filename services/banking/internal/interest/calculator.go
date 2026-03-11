// Package interest provides interest calculation algorithms for the Blazil banking service.
package interest

// InterestCalculator computes interest amounts given a principal and rate.
// All implementations are stateless and safe for concurrent use.
type InterestCalculator interface {
	// Calculate returns the interest amount in minor units (e.g. cents).
	// principal is in minor units, annualRateBPS is the annual rate in basis points
	// (e.g. 500 = 5.00%), and periods is the number of compounding/accrual periods.
	Calculate(principalMinorUnits int64, annualRateBPS int64, periods int) (int64, error)
}
