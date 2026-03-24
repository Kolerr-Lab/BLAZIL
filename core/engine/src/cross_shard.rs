//! Cross-shard transfer coordination.
//!
//! A transfer whose debit and credit accounts hash to **different shards** is
//! a cross-shard transfer.  Because TigerBeetle supports atomic linked-transfer
//! chains natively, no custom two-phase-commit logic is required:
//!
//! * **Local** (same shard): submit as a single transfer — no special handling.
//! * **Remote** (different shards): submit as a TB-linked pair:
//!   - Transfer 1 (debit leg) with `flags = LINKED`
//!   - Transfer 2 (credit leg) with `flags = 0` (chain terminator)
//!   - TB commits both atomically or rejects both.
//!
//! # Usage
//!
//! ```rust
//! use blazil_engine::cross_shard::{CrossShardTransfer, CrossShardRoute, route_cross_shard};
//!
//! let transfer = CrossShardTransfer::new(42, 99, 10_00, 1);
//! let route = route_cross_shard(&transfer, 4);
//! match route {
//!     CrossShardRoute::Local(shard) => println!("local → shard {}", shard),
//!     CrossShardRoute::Remote { src_shard, dst_shard } => {
//!         println!("cross-shard {} → {}", src_shard, dst_shard);
//!     }
//! }
//! ```

use crate::sharded_pipeline::route_to_shard;

// ── CrossShardTransfer ────────────────────────────────────────────────────────

/// Describes a funds transfer that may span two shards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossShardTransfer {
    /// Account ID of the debit side (funds leave here).
    pub src_account_id: u64,
    /// Account ID of the credit side (funds arrive here).
    pub dst_account_id: u64,
    /// Transfer amount in minor units (e.g. cents for USD).
    pub amount: u64,
    /// Caller-assigned transfer ID (must be globally unique).
    pub transfer_id: u64,
}

impl CrossShardTransfer {
    /// Creates a new `CrossShardTransfer`.
    pub fn new(src_account_id: u64, dst_account_id: u64, amount: u64, transfer_id: u64) -> Self {
        Self {
            src_account_id,
            dst_account_id,
            amount,
            transfer_id,
        }
    }
}

// ── CrossShardRoute ───────────────────────────────────────────────────────────

/// Routing decision for a [`CrossShardTransfer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossShardRoute {
    /// Both accounts hash to the same shard — submit as a single transfer.
    Local(usize),
    /// Accounts are on different shards — submit as a TB-linked pair.
    ///
    /// * Transfer 1 (`flags = LINKED`) is submitted to `src_shard`.
    /// * Transfer 2 (`flags = 0`) is submitted to `dst_shard`.
    ///
    /// TigerBeetle commits both atomically.
    Remote {
        /// Shard that owns the debit account.
        src_shard: usize,
        /// Shard that owns the credit account.
        dst_shard: usize,
    },
}

// ── route_cross_shard ─────────────────────────────────────────────────────────

/// Compute the routing decision for a [`CrossShardTransfer`].
///
/// Uses [`route_to_shard`] for both sides; if they are equal the transfer is
/// local, otherwise it is a remote (cross-shard) transfer.
///
/// # Arguments
///
/// * `transfer`    – The transfer to route.
/// * `shard_count` – Active shard count (must be a power of 2).
pub fn route_cross_shard(transfer: &CrossShardTransfer, shard_count: usize) -> CrossShardRoute {
    let src_shard = route_to_shard(transfer.src_account_id, shard_count);
    let dst_shard = route_to_shard(transfer.dst_account_id, shard_count);
    if src_shard == dst_shard {
        CrossShardRoute::Local(src_shard)
    } else {
        CrossShardRoute::Remote {
            src_shard,
            dst_shard,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_shard_is_local() {
        // With 4 shards (mask = 0b11): accounts 0 and 4 both → shard 0
        let t = CrossShardTransfer::new(0, 4, 100, 1);
        assert_eq!(route_cross_shard(&t, 4), CrossShardRoute::Local(0));
    }

    #[test]
    fn different_shard_is_remote() {
        // account 0 → shard 0, account 1 → shard 1
        let t = CrossShardTransfer::new(0, 1, 100, 2);
        assert_eq!(
            route_cross_shard(&t, 4),
            CrossShardRoute::Remote {
                src_shard: 0,
                dst_shard: 1,
            }
        );
    }

    #[test]
    fn single_shard_always_local() {
        // With 1 shard everything is local (mask = 0)
        let t = CrossShardTransfer::new(42, 99, 10_00, 3);
        assert_eq!(route_cross_shard(&t, 1), CrossShardRoute::Local(0));
    }

    #[test]
    fn remote_route_uses_correct_shards() {
        // 8 shards (mask = 0b111)
        // account 2 → shard 2, account 5 → shard 5
        let t = CrossShardTransfer::new(2, 5, 500, 4);
        assert_eq!(
            route_cross_shard(&t, 8),
            CrossShardRoute::Remote {
                src_shard: 2,
                dst_shard: 5,
            }
        );
    }
}
