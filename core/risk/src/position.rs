//! Position tracking for risk management.
//!
//! A position represents an account's exposure to a specific instrument
//! (security, currency pair, derivative, etc.).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A position in a specific instrument held by an account.
///
/// Tracks both quantity (number of units) and notional value (market value).
/// Supports long (positive quantity) and short (negative quantity) positions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Unique identifier for the instrument (e.g., stock symbol, currency pair)
    instrument: String,
    /// Quantity of units held (positive = long, negative = short, zero = flat)
    quantity: Decimal,
    /// Notional value in base currency (quantity × price)
    notional: Decimal,
    /// Average entry price per unit
    avg_price: Decimal,
}

impl Position {
    /// Creates a new position with the given parameters.
    ///
    /// # Arguments
    ///
    /// * `instrument` - Unique identifier for the instrument
    /// * `quantity` - Number of units (positive for long, negative for short)
    /// * `notional` - Market value in base currency
    /// * `avg_price` - Average entry price per unit
    ///
    /// # Examples
    ///
    /// ```
    /// use blazil_risk::position::Position;
    /// use rust_decimal::Decimal;
    ///
    /// let pos = Position::new(
    ///     "AAPL".to_string(),
    ///     Decimal::new(100, 0),  // 100 shares
    ///     Decimal::new(15000, 0), // $15,000 notional
    ///     Decimal::new(150, 0),   // $150/share avg price
    /// );
    /// assert_eq!(pos.quantity(), &Decimal::new(100, 0));
    /// ```
    pub fn new(
        instrument: String,
        quantity: Decimal,
        notional: Decimal,
        avg_price: Decimal,
    ) -> Self {
        Self {
            instrument,
            quantity,
            notional,
            avg_price,
        }
    }

    /// Creates a flat (zero) position for an instrument.
    pub fn zero(instrument: String) -> Self {
        Self {
            instrument,
            quantity: Decimal::ZERO,
            notional: Decimal::ZERO,
            avg_price: Decimal::ZERO,
        }
    }

    /// Returns the instrument identifier.
    pub fn instrument(&self) -> &str {
        &self.instrument
    }

    /// Returns the position quantity (positive = long, negative = short).
    pub fn quantity(&self) -> &Decimal {
        &self.quantity
    }

    /// Returns the notional value in base currency.
    pub fn notional(&self) -> &Decimal {
        &self.notional
    }

    /// Returns the average entry price per unit.
    pub fn avg_price(&self) -> &Decimal {
        &self.avg_price
    }

    /// Returns true if this is a long position (quantity > 0).
    pub fn is_long(&self) -> bool {
        self.quantity > Decimal::ZERO
    }

    /// Returns true if this is a short position (quantity < 0).
    pub fn is_short(&self) -> bool {
        self.quantity < Decimal::ZERO
    }

    /// Returns true if this is a flat position (quantity == 0).
    pub fn is_flat(&self) -> bool {
        self.quantity == Decimal::ZERO
    }

    /// Updates the position with a new trade.
    ///
    /// Adjusts quantity, notional, and recalculates average price.
    ///
    /// # Arguments
    ///
    /// * `quantity_delta` - Change in quantity (positive for buy, negative for sell)
    /// * `price` - Execution price for this trade
    pub fn update(&mut self, quantity_delta: Decimal, price: Decimal) {
        let new_quantity = self.quantity + quantity_delta;
        let trade_notional = quantity_delta * price;

        if new_quantity.is_zero() {
            // Position closed
            self.quantity = Decimal::ZERO;
            self.notional = Decimal::ZERO;
            self.avg_price = Decimal::ZERO;
        } else if self.quantity.is_zero()
            || (self.quantity > Decimal::ZERO) == (quantity_delta > Decimal::ZERO)
        {
            // New position or adding to existing position
            let total_notional = self.notional + trade_notional;
            self.quantity = new_quantity;
            self.notional = total_notional;
            self.avg_price = if !new_quantity.is_zero() {
                total_notional / new_quantity
            } else {
                Decimal::ZERO
            };
        } else {
            // Reducing position (partial or full close)
            self.quantity = new_quantity;
            self.notional = new_quantity * self.avg_price;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_position_has_correct_fields() {
        let pos = Position::new(
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(15000, 0),
            Decimal::new(150, 0),
        );
        assert_eq!(pos.instrument(), "AAPL");
        assert_eq!(pos.quantity(), &Decimal::new(100, 0));
        assert_eq!(pos.notional(), &Decimal::new(15000, 0));
        assert_eq!(pos.avg_price(), &Decimal::new(150, 0));
    }

    #[test]
    fn zero_position_is_flat() {
        let pos = Position::zero("TSLA".to_string());
        assert!(pos.is_flat());
        assert!(!pos.is_long());
        assert!(!pos.is_short());
        assert_eq!(pos.quantity(), &Decimal::ZERO);
    }

    #[test]
    fn positive_quantity_is_long() {
        let pos = Position::new(
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(15000, 0),
            Decimal::new(150, 0),
        );
        assert!(pos.is_long());
        assert!(!pos.is_short());
        assert!(!pos.is_flat());
    }

    #[test]
    fn negative_quantity_is_short() {
        let pos = Position::new(
            "AAPL".to_string(),
            Decimal::new(-100, 0),
            Decimal::new(-15000, 0),
            Decimal::new(150, 0),
        );
        assert!(pos.is_short());
        assert!(!pos.is_long());
        assert!(!pos.is_flat());
    }

    #[test]
    fn update_adds_to_long_position() {
        let mut pos = Position::new(
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(15000, 0),
            Decimal::new(150, 0),
        );
        pos.update(Decimal::new(50, 0), Decimal::new(160, 0));

        assert_eq!(pos.quantity(), &Decimal::new(150, 0));
        assert_eq!(pos.notional(), &Decimal::new(23000, 0)); // 15000 + (50*160)
                                                             // avg price = 23000 / 150 = 153.33...
        assert!(pos.avg_price() > &Decimal::new(153, 0) && pos.avg_price() < &Decimal::new(154, 0));
    }

    #[test]
    fn update_reduces_long_position() {
        let mut pos = Position::new(
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(15000, 0),
            Decimal::new(150, 0),
        );
        pos.update(Decimal::new(-30, 0), Decimal::new(160, 0));

        assert_eq!(pos.quantity(), &Decimal::new(70, 0));
        assert_eq!(pos.avg_price(), &Decimal::new(150, 0)); // avg price unchanged on reduce
    }

    #[test]
    fn update_closes_position() {
        let mut pos = Position::new(
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(15000, 0),
            Decimal::new(150, 0),
        );
        pos.update(Decimal::new(-100, 0), Decimal::new(155, 0));

        assert!(pos.is_flat());
        assert_eq!(pos.quantity(), &Decimal::ZERO);
        assert_eq!(pos.notional(), &Decimal::ZERO);
        assert_eq!(pos.avg_price(), &Decimal::ZERO);
    }

    #[test]
    fn update_opens_new_position_from_flat() {
        let mut pos = Position::zero("AAPL".to_string());
        pos.update(Decimal::new(100, 0), Decimal::new(150, 0));

        assert!(pos.is_long());
        assert_eq!(pos.quantity(), &Decimal::new(100, 0));
        assert_eq!(pos.notional(), &Decimal::new(15000, 0));
        assert_eq!(pos.avg_price(), &Decimal::new(150, 0));
    }
}
