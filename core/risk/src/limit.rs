//! Risk limits for position management.
//!
//! Defines limits that constrain trading activity to prevent excessive risk.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Risk limits for an account or instrument.
///
/// These limits are checked before orders are submitted to prevent
/// excessive risk exposure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskLimit {
    /// Maximum absolute position size (in units)
    /// None = unlimited
    pub max_position_size: Option<Decimal>,

    /// Maximum notional exposure per instrument (in base currency)
    /// None = unlimited
    pub max_notional_per_instrument: Option<Decimal>,

    /// Maximum total notional exposure across all instruments
    /// None = unlimited
    pub max_total_notional: Option<Decimal>,

    /// Maximum order size (single order, in units)
    /// None = unlimited
    pub max_order_size: Option<Decimal>,
}

impl RiskLimit {
    /// Creates a new risk limit with the given constraints.
    ///
    /// # Examples
    ///
    /// ```
    /// use blazil_risk::limit::RiskLimit;
    /// use rust_decimal::Decimal;
    ///
    /// let limit = RiskLimit::new(
    ///     Some(Decimal::new(1000, 0)),  // max 1000 units position
    ///     Some(Decimal::new(100_000, 0)), // max $100k per instrument
    ///     Some(Decimal::new(500_000, 0)), // max $500k total
    ///     Some(Decimal::new(100, 0)),     // max 100 units per order
    /// );
    /// ```
    pub fn new(
        max_position_size: Option<Decimal>,
        max_notional_per_instrument: Option<Decimal>,
        max_total_notional: Option<Decimal>,
        max_order_size: Option<Decimal>,
    ) -> Self {
        Self {
            max_position_size,
            max_notional_per_instrument,
            max_total_notional,
            max_order_size,
        }
    }

    /// Creates an unlimited risk limit (no constraints).
    ///
    /// **WARNING**: Use only for testing or internal accounts.
    pub fn unlimited() -> Self {
        Self {
            max_position_size: None,
            max_notional_per_instrument: None,
            max_total_notional: None,
            max_order_size: None,
        }
    }

    /// Creates a conservative retail risk limit.
    ///
    /// Suitable for retail clients with limited capital.
    pub fn retail() -> Self {
        Self {
            max_position_size: Some(Decimal::new(1_000, 0)), // 1K units
            max_notional_per_instrument: Some(Decimal::new(50_000, 0)), // $50K
            max_total_notional: Some(Decimal::new(200_000, 0)), // $200K
            max_order_size: Some(Decimal::new(100, 0)),      // 100 units
        }
    }

    /// Creates an institutional risk limit.
    ///
    /// Suitable for professional traders and institutions.
    pub fn institutional() -> Self {
        Self {
            max_position_size: Some(Decimal::new(100_000, 0)), // 100K units
            max_notional_per_instrument: Some(Decimal::new(10_000_000, 0)), // $10M
            max_total_notional: Some(Decimal::new(50_000_000, 0)), // $50M
            max_order_size: Some(Decimal::new(10_000, 0)),     // 10K units
        }
    }
}

impl Default for RiskLimit {
    /// Default risk limit is retail-level (conservative).
    fn default() -> Self {
        Self::retail()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_has_no_constraints() {
        let limit = RiskLimit::unlimited();
        assert!(limit.max_position_size.is_none());
        assert!(limit.max_notional_per_instrument.is_none());
        assert!(limit.max_total_notional.is_none());
        assert!(limit.max_order_size.is_none());
    }

    #[test]
    fn retail_has_conservative_limits() {
        let limit = RiskLimit::retail();
        assert_eq!(limit.max_position_size, Some(Decimal::new(1_000, 0)));
        assert_eq!(
            limit.max_notional_per_instrument,
            Some(Decimal::new(50_000, 0))
        );
        assert_eq!(limit.max_total_notional, Some(Decimal::new(200_000, 0)));
        assert_eq!(limit.max_order_size, Some(Decimal::new(100, 0)));
    }

    #[test]
    fn institutional_has_higher_limits() {
        let limit = RiskLimit::institutional();
        assert_eq!(limit.max_position_size, Some(Decimal::new(100_000, 0)));
        assert_eq!(
            limit.max_notional_per_instrument,
            Some(Decimal::new(10_000_000, 0))
        );
        assert_eq!(limit.max_total_notional, Some(Decimal::new(50_000_000, 0)));
        assert_eq!(limit.max_order_size, Some(Decimal::new(10_000, 0)));
    }

    #[test]
    fn default_is_retail() {
        let default_limit = RiskLimit::default();
        let retail_limit = RiskLimit::retail();
        assert_eq!(default_limit, retail_limit);
    }

    #[test]
    fn new_creates_custom_limit() {
        let limit = RiskLimit::new(
            Some(Decimal::new(500, 0)),
            Some(Decimal::new(25_000, 0)),
            None,
            Some(Decimal::new(50, 0)),
        );
        assert_eq!(limit.max_position_size, Some(Decimal::new(500, 0)));
        assert_eq!(
            limit.max_notional_per_instrument,
            Some(Decimal::new(25_000, 0))
        );
        assert!(limit.max_total_notional.is_none());
        assert_eq!(limit.max_order_size, Some(Decimal::new(50, 0)));
    }
}
