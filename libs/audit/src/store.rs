//! Audit Log Store — Thread-Safe, Append-Only Storage

use crate::{AuditEntry, AuditEvent, HashChain};
use parking_lot::RwLock;
use std::sync::Arc;

/// Thread-safe audit log with tamper-evident storage
#[derive(Clone)]
pub struct AuditLog {
    inner: Arc<AuditLogInner>,
}

struct AuditLogInner {
    /// Hash chain for tamper-evident logging
    chain: RwLock<HashChain>,
    /// Append-only log storage
    entries: RwLock<Vec<AuditEntry>>,
}

impl AuditLog {
    /// Create a new audit log
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AuditLogInner {
                chain: RwLock::new(HashChain::new()),
                entries: RwLock::new(Vec::new()),
            }),
        }
    }

    /// Record an audit event (append-only)
    pub async fn record(&self, event: AuditEvent) {
        let entry = {
            let mut chain = self.inner.chain.write();
            chain.create_entry(event)
        };

        let mut entries = self.inner.entries.write();
        entries.push(entry);
    }

    /// Get total number of entries
    pub async fn len(&self) -> usize {
        let entries = self.inner.entries.read();
        entries.len()
    }

    /// Check if log is empty
    pub async fn is_empty(&self) -> bool {
        let entries = self.inner.entries.read();
        entries.is_empty()
    }

    /// Get entries by sequence range
    pub async fn get_range(&self, start: u64, end: u64) -> Vec<AuditEntry> {
        let entries = self.inner.entries.read();
        entries
            .iter()
            .filter(|e| e.sequence >= start && e.sequence < end)
            .cloned()
            .collect()
    }

    /// Get entries by transaction ID
    pub async fn get_by_transaction(&self, transaction_id: &str) -> Vec<AuditEntry> {
        let entries = self.inner.entries.read();
        entries
            .iter()
            .filter(|e| e.event.transaction_id == transaction_id)
            .cloned()
            .collect()
    }

    /// Get entries by actor
    pub async fn get_by_actor(&self, actor: &str) -> Vec<AuditEntry> {
        let entries = self.inner.entries.read();
        entries
            .iter()
            .filter(|e| e.event.actor == actor)
            .cloned()
            .collect()
    }

    /// Get all entries (for export)
    pub async fn get_all(&self) -> Vec<AuditEntry> {
        let entries = self.inner.entries.read();
        entries.clone()
    }

    /// Verify chain integrity
    pub async fn verify_integrity(&self) -> bool {
        let chain = self.inner.chain.read();
        let entries = self.inner.entries.read();
        chain.verify_chain_from_genesis(&entries)
    }

    /// Get current sequence number
    pub async fn current_sequence(&self) -> u64 {
        let chain = self.inner.chain.read();
        chain.current_sequence()
    }

    /// Export log to JSON
    pub async fn export_json(&self, start: Option<u64>, end: Option<u64>) -> String {
        let entries = self.inner.entries.read();

        let filtered: Vec<&AuditEntry> = entries
            .iter()
            .filter(|e| {
                let after_start = start.map_or(true, |s| e.sequence >= s);
                let before_end = end.map_or(true, |e_val| e.sequence < e_val);
                after_start && before_end
            })
            .collect();

        serde_json::to_string_pretty(&filtered).unwrap_or_else(|_| "[]".to_string())
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuditAction, AuditEvent};

    #[tokio::test]
    async fn test_audit_log_creation() {
        let log = AuditLog::new();
        assert!(log.is_empty().await);
        assert_eq!(log.len().await, 0);
    }

    #[tokio::test]
    async fn test_audit_log_record() {
        let log = AuditLog::new();

        let event = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );

        log.record(event).await;

        assert_eq!(log.len().await, 1);
    }

    #[tokio::test]
    async fn test_audit_log_multiple_records() {
        let log = AuditLog::new();

        for i in 0..10 {
            let event = AuditEvent::new(
                format!("tx_{}", i),
                format!("actor_{}", i),
                AuditAction::TransactionCreated,
            );
            log.record(event).await;
        }

        assert_eq!(log.len().await, 10);
        assert_eq!(log.current_sequence().await, 10);
    }

    #[tokio::test]
    async fn test_audit_log_get_by_transaction() {
        let log = AuditLog::new();

        let event1 = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );
        log.record(event1).await;

        let event2 = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCompleted,
        );
        log.record(event2).await;

        let event3 = AuditEvent::new(
            "tx_2".to_string(),
            "actor_2".to_string(),
            AuditAction::TransactionCreated,
        );
        log.record(event3).await;

        let tx1_entries = log.get_by_transaction("tx_1").await;
        assert_eq!(tx1_entries.len(), 2);
    }

    #[tokio::test]
    async fn test_audit_log_get_by_actor() {
        let log = AuditLog::new();

        let event1 = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );
        log.record(event1).await;

        let event2 = AuditEvent::new(
            "tx_2".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );
        log.record(event2).await;

        let event3 = AuditEvent::new(
            "tx_3".to_string(),
            "actor_2".to_string(),
            AuditAction::TransactionCreated,
        );
        log.record(event3).await;

        let actor1_entries = log.get_by_actor("actor_1").await;
        assert_eq!(actor1_entries.len(), 2);
    }

    #[tokio::test]
    async fn test_audit_log_integrity() {
        let log = AuditLog::new();

        for i in 0..10 {
            let event = AuditEvent::new(
                format!("tx_{}", i),
                format!("actor_{}", i),
                AuditAction::TransactionCreated,
            );
            log.record(event).await;
        }

        assert!(log.verify_integrity().await);
    }

    #[tokio::test]
    async fn test_audit_log_range_query() {
        let log = AuditLog::new();

        for i in 0..10 {
            let event = AuditEvent::new(
                format!("tx_{}", i),
                format!("actor_{}", i),
                AuditAction::TransactionCreated,
            );
            log.record(event).await;
        }

        let range = log.get_range(3, 7).await;
        assert_eq!(range.len(), 4); // sequences 3, 4, 5, 6
        assert_eq!(range[0].sequence, 3);
        assert_eq!(range[3].sequence, 6);
    }

    #[tokio::test]
    async fn test_audit_log_export_json() {
        let log = AuditLog::new();

        let event = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success");

        log.record(event).await;

        let json = log.export_json(None, None).await;
        assert!(json.contains("tx_1"));
        assert!(json.contains("actor_1"));
        assert!(json.contains("transaction_id"));
    }

    #[tokio::test]
    async fn test_audit_log_concurrent_writes() {
        let log = AuditLog::new();

        let mut handles = vec![];

        for i in 0..100 {
            let log_clone = log.clone();
            let handle = tokio::spawn(async move {
                let event = AuditEvent::new(
                    format!("tx_{}", i),
                    format!("actor_{}", i),
                    AuditAction::TransactionCreated,
                );
                log_clone.record(event).await;
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(log.len().await, 100);
        assert!(log.verify_integrity().await);
    }
}
