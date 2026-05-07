//! Risk engine trait and implementation.
//!
//! The risk engine validates orders before execution and tracks positions
//! to enforce risk limits.

use crate::limit::RiskLimit;
use crate::position::Position;
use async_trait::async_trait;
use dashmap::DashMap;
use rust_decimal::Decimal;
use std::sync::Arc;
use thiserror::Error;

/// Risk check errors.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RiskError {
    /// Order size exceeds maximum allowed per order
    #[error("order size {order_size} exceeds limit {limit}")]
    OrderSizeLimitExceeded {
        order_size: Decimal,
        limit: Decimal,
    },

    /// Position size would exceed maximum after order
    #[error("position size {new_size} would exceed limit {limit}")]
    PositionSizeLimitExceeded {
        new_size: Decimal,
        limit: Decimal,
    },

    /// Notional exposure per instrument would exceed maximum
    #[error("notional {new_notional} for {instrument} would exceed limit {limit}")]
    InstrumentNotionalLimitExceeded {
        instrument: String,
        new_notional: Decimal,
        limit: Decimal,
    },

    /// Total notional exposure across all instruments would exceed maximum
    #[error("total notional {new_total} would exceed limit {limit}")]
    TotalNotionalLimitExceeded {
        new_total: Decimal,
        limit: Decimal,
    },

    /// Account not found in risk engine
    #[error("account {0} not registered in risk engine")]
    AccountNotFound(String),
}

/// Order information for risk checks.
#[derive(Debug, Clone)]
pub struct OrderRequest {
    /// Account placing the order
    pub account_id: String,
    /// Instrument being traded
    pub instrument: String,
    /// Order quantity (positive for buy, negative for sell)
    pub quantity: Decimal,
    /// Order price per unit
    pub price: Decimal,
}

impl OrderRequest {
    /// Creates a new order request.
    pub fn new(account_id: String, instrument: String, quantity: Decimal, price: Decimal) -> Self {
        Self {
            account_id,
            instrument,
            quantity,
            price,
        }
    }

    /// Returns the notional value of this order.
    pub fn notional(&self) -> Decimal {
        self.quantity.abs() * self.price
    }
}

/// Abstract risk engine interface.
///
/// Implementations validate orders against risk limits and track positions.
#[async_trait]
pub trait RiskEngine: Send + Sync {
    /// Registers an account with risk limits.
    ///
    /// If the account already exists, its limits are updated.
    async fn register_account(&self, account_id: String, limit: RiskLimit);

    /// Checks if an order violates risk limits.
    ///
    /// Returns `Ok(())` if the order passes all checks, or `Err(RiskError)` if it violates a limit.
    async fn check_order(&self, order: &OrderRequest) -> Result<(), RiskError>;

    /// Updates position after an order execution.
    ///
    /// Must be called after successful order execution to keep positions in sync.
    async fn update_position(&self, order: &OrderRequest);

    /// Gets the current position for an account and instrument.
    ///
    /// Returns `None` if no position exists (flat position).
    async fn get_position(&self, account_id: &str, instrument: &str) -> Option<Position>;

    /// Gets all positions for an account.
    async fn get_account_positions(&self, account_id: &str) -> Vec<Position>;

    /// Calculates total notional exposure across all instruments for an account.
    async fn get_total_notional(&self, account_id: &str) -> Decimal;
}

/// In-memory risk engine using DashMap for concurrent access.
///
/// Thread-safe implementation suitable for production use.
pub struct InMemoryRiskEngine {
    /// Maps account_id → RiskLimit
    limits: Arc<DashMap<String, RiskLimit>>,
    /// Maps (account_id, instrument) → Position
    positions: Arc<DashMap<(String, String), Position>>,
}

impl InMemoryRiskEngine {
    /// Creates a new in-memory risk engine.
    pub fn new() -> Self {
        Self {
            limits: Arc::new(DashMap::new()),
            positions: Arc::new(DashMap::new()),
        }
    }
}

impl Default for InMemoryRiskEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RiskEngine for InMemoryRiskEngine {
    async fn register_account(&self, account_id: String, limit: RiskLimit) {
        self.limits.insert(account_id, limit);
    }

    async fn check_order(&self, order: &OrderRequest) -> Result<(), RiskError> {
        // Get risk limit for this account
        let limit = self
            .limits
            .get(&order.account_id)
            .ok_or_else(|| RiskError::AccountNotFound(order.account_id.clone()))?;

        // Check 1: Order size limit
        if let Some(max_order_size) = limit.max_order_size {
            let order_size = order.quantity.abs();
            if order_size > max_order_size {
                return Err(RiskError::OrderSizeLimitExceeded {
                    order_size,
                    limit: max_order_size,
                });
            }
        }

        // Get current position (if any)
        let current_position = self
            .positions
            .get(&(order.account_id.clone(), order.instrument.clone()))
            .map(|p| p.clone());

        let current_quantity = current_position
            .as_ref()
            .map(|p| *p.quantity())
            .unwrap_or(Decimal::ZERO);
        let current_notional = current_position
            .as_ref()
            .map(|p| *p.notional())
            .unwrap_or(Decimal::ZERO);

        // Check 2: Position size limit (absolute value)
        if let Some(max_position_size) = limit.max_position_size {
            let new_quantity = current_quantity + order.quantity;
            let new_size = new_quantity.abs();
            if new_size > max_position_size {
                return Err(RiskError::PositionSizeLimitExceeded {
                    new_size,
                    limit: max_position_size,
                });
            }
        }

        // Check 3: Notional per instrument limit
        if let Some(max_notional) = limit.max_notional_per_instrument {
            // Calculate new notional (simplified: doesn't recalculate avg price)
            let order_notional = order.quantity * order.price;
            let new_notional = (current_notional + order_notional).abs();
            if new_notional > max_notional {
                return Err(RiskError::InstrumentNotionalLimitExceeded {
                    instrument: order.instrument.clone(),
                    new_notional,
                    limit: max_notional,
                });
            }
        }

        // Check 4: Total notional limit
        if let Some(max_total) = limit.max_total_notional {
            let current_total = self.get_total_notional(&order.account_id).await;
            let order_notional = order.quantity * order.price;
            let new_total = (current_total + order_notional).abs();
            if new_total > max_total {
                return Err(RiskError::TotalNotionalLimitExceeded {
                    new_total,
                    limit: max_total,
                });
            }
        }

        Ok(())
    }

    async fn update_position(&self, order: &OrderRequest) {
        let key = (order.account_id.clone(), order.instrument.clone());
        
        self.positions
            .entry(key.clone())
            .and_modify(|pos| pos.update(order.quantity, order.price))
            .or_insert_with(|| {
                let mut pos = Position::zero(order.instrument.clone());
                pos.update(order.quantity, order.price);
                pos
            });
    }

    async fn get_position(&self, account_id: &str, instrument: &str) -> Option<Position> {
        self.positions
            .get(&(account_id.to_string(), instrument.to_string()))
            .map(|p| p.clone())
    }

    async fn get_account_positions(&self, account_id: &str) -> Vec<Position> {
        self.positions
            .iter()
            .filter(|entry| entry.key().0 == account_id)
            .map(|entry| entry.value().clone())
            .collect()
    }

    async fn get_total_notional(&self, account_id: &str) -> Decimal {
        self.positions
            .iter()
            .filter(|entry| entry.key().0 == account_id)
            .map(|entry| entry.value().notional().abs())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> String {
        "test_account".to_string()
    }

    #[tokio::test]
    async fn register_account_stores_limit() {
        let engine = InMemoryRiskEngine::new();
        let limit = RiskLimit::retail();
        engine.register_account(account(), limit.clone()).await;

        // Verify by checking order that should pass
        let order = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(10, 0),
            Decimal::new(150, 0),
        );
        assert!(engine.check_order(&order).await.is_ok());
    }

    #[tokio::test]
    async fn check_order_fails_for_unregistered_account() {
        let engine = InMemoryRiskEngine::new();
        let order = OrderRequest::new(
            "unknown".to_string(),
            "AAPL".to_string(),
            Decimal::new(10, 0),
            Decimal::new(150, 0),
        );
        let result = engine.check_order(&order).await;
        assert!(matches!(result, Err(RiskError::AccountNotFound(_))));
    }

    #[tokio::test]
    async fn check_order_rejects_oversized_order() {
        let engine = InMemoryRiskEngine::new();
        engine.register_account(account(), RiskLimit::retail()).await;

        let order = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(200, 0), // > retail limit of 100
            Decimal::new(150, 0),
        );
        let result = engine.check_order(&order).await;
        assert!(matches!(result, Err(RiskError::OrderSizeLimitExceeded { .. })));
    }

    #[tokio::test]
    async fn check_order_allows_valid_order() {
        let engine = InMemoryRiskEngine::new();
        engine.register_account(account(), RiskLimit::retail()).await;

        let order = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(50, 0),
            Decimal::new(150, 0),
        );
        assert!(engine.check_order(&order).await.is_ok());
    }

    #[tokio::test]
    async fn update_position_creates_new_position() {
        let engine = InMemoryRiskEngine::new();
        let order = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(150, 0),
        );
        
        engine.update_position(&order).await;
        
        let pos = engine.get_position(&account(), "AAPL").await.unwrap();
        assert_eq!(pos.quantity(), &Decimal::new(100, 0));
        assert_eq!(pos.instrument(), "AAPL");
    }

    #[tokio::test]
    async fn update_position_modifies_existing_position() {
        let engine = InMemoryRiskEngine::new();
        
        let order1 = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(150, 0),
        );
        engine.update_position(&order1).await;

        let order2 = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(50, 0),
            Decimal::new(160, 0),
        );
        engine.update_position(&order2).await;
        
        let pos = engine.get_position(&account(), "AAPL").await.unwrap();
        assert_eq!(pos.quantity(), &Decimal::new(150, 0));
    }

    #[tokio::test]
    async fn get_account_positions_returns_all_positions() {
        let engine = InMemoryRiskEngine::new();
        
        let order1 = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(150, 0),
        );
        engine.update_position(&order1).await;

        let order2 = OrderRequest::new(
            account(),
            "TSLA".to_string(),
            Decimal::new(50, 0),
            Decimal::new(200, 0),
        );
        engine.update_position(&order2).await;
        
        let positions = engine.get_account_positions(&account()).await;
        assert_eq!(positions.len(), 2);
    }

    #[tokio::test]
    async fn get_total_notional_sums_all_positions() {
        let engine = InMemoryRiskEngine::new();
        
        let order1 = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(100, 0),
            Decimal::new(150, 0),
        );
        engine.update_position(&order1).await;

        let order2 = OrderRequest::new(
            account(),
            "TSLA".to_string(),
            Decimal::new(50, 0),
            Decimal::new(200, 0),
        );
        engine.update_position(&order2).await;
        
        let total = engine.get_total_notional(&account()).await;
        assert_eq!(total, Decimal::new(25_000, 0)); // 15000 + 10000
    }

    #[tokio::test]
    async fn check_order_enforces_position_size_limit() {
        let engine = InMemoryRiskEngine::new();
        let limit = RiskLimit::new(
            Some(Decimal::new(100, 0)), // max 100 units
            None,
            None,
            None,
        );
        engine.register_account(account(), limit).await;

        // First order: 80 units (OK)
        let order1 = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(80, 0),
            Decimal::new(150, 0),
        );
        assert!(engine.check_order(&order1).await.is_ok());
        engine.update_position(&order1).await;

        // Second order: +30 units = 110 total (EXCEEDS)
        let order2 = OrderRequest::new(
            account(),
            "AAPL".to_string(),
            Decimal::new(30, 0),
            Decimal::new(150, 0),
        );
        let result = engine.check_order(&order2).await;
        assert!(matches!(result, Err(RiskError::PositionSizeLimitExceeded { .. })));
    }
}
