//! Blazil Audit Logging — Tamper-Evident Transaction Audit Trail
//!
//! This module provides SOC 2 compliant audit logging with:
//! - Append-only, tamper-evident log format with hash chaining
//! - Structured audit events for transaction lifecycle
//! - Log export API (JSON, CEF format)
//! - Thread-safe, lock-free writes
//!
//! # Hash Chaining
//!
//! Each audit log entry includes a SHA-256 hash of:
//! - Previous entry hash
//! - Current entry data
//!
//! This creates a tamper-evident chain where any modification to historical
//! entries will invalidate all subsequent hashes.
//!
//! # Example
//!
//! ```
//! use blazil_audit::{AuditLog, AuditEvent, AuditAction};
//!
//! # #[tokio::main]
//! # async fn main() {
//! let log = AuditLog::new();
//!
//! log.record(AuditEvent::new(
//!     "tx_12345".to_string(),
//!     "user_alice".to_string(),
//!     AuditAction::TransactionCreated,
//! ).with_result("success")).await;
//!
//! // Export to JSON
//! let json_export = log.export_json(None, None).await;
//! println!("{}", json_export);
//! # }
//! ```

mod entry;
mod event;
mod export;
mod store;

pub use entry::{AuditEntry, HashChain};
pub use event::{AuditAction, AuditEvent, AuditResult};
pub use export::{ExportFormat, LogExporter};
pub use store::AuditLog;

#[cfg(test)]
mod tests;
