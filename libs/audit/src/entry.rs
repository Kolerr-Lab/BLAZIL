//! Tamper-Evident Audit Log Entry with Hash Chaining

use crate::AuditEvent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Tamper-evident audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Sequence number (monotonically increasing)
    pub sequence: u64,
    /// The audit event
    pub event: AuditEvent,
    /// Hash of previous entry (or genesis hash for first entry)
    pub previous_hash: String,
    /// Hash of this entry (SHA-256)
    pub entry_hash: String,
}

impl AuditEntry {
    /// Create a new audit entry
    pub fn new(sequence: u64, event: AuditEvent, previous_hash: String) -> Self {
        let mut entry = Self {
            sequence,
            event,
            previous_hash,
            entry_hash: String::new(), // Will be computed
        };
        entry.entry_hash = entry.compute_hash();
        entry
    }

    /// Compute the hash of this entry
    fn compute_hash(&self) -> String {
        let mut hasher = Sha256::new();

        // Hash: sequence + previous_hash + event_data
        hasher.update(self.sequence.to_le_bytes());
        hasher.update(self.previous_hash.as_bytes());
        hasher.update(
            serde_json::to_string(&self.event)
                .unwrap_or_default()
                .as_bytes(),
        );

        hex::encode(hasher.finalize())
    }

    /// Verify the integrity of this entry
    pub fn verify_integrity(&self) -> bool {
        self.entry_hash == self.compute_hash()
    }

    /// Verify the chain integrity (this entry links to previous)
    pub fn verify_chain(&self, expected_previous_hash: &str) -> bool {
        self.previous_hash == expected_previous_hash
    }
}

/// Hash chain manager
pub struct HashChain {
    genesis_hash: String,
    current_hash: String,
    sequence: u64,
}

impl HashChain {
    /// Create a new hash chain with genesis hash
    pub fn new() -> Self {
        // Genesis hash: SHA-256 of "BLAZIL_AUDIT_LOG_GENESIS"
        let mut hasher = Sha256::new();
        hasher.update(b"BLAZIL_AUDIT_LOG_GENESIS");
        let genesis_hash = hex::encode(hasher.finalize());

        Self {
            genesis_hash: genesis_hash.clone(),
            current_hash: genesis_hash,
            sequence: 0,
        }
    }

    /// Get current sequence number
    pub fn current_sequence(&self) -> u64 {
        self.sequence
    }

    /// Get current hash
    pub fn current_hash(&self) -> &str {
        &self.current_hash
    }

    /// Create next entry in the chain
    pub fn create_entry(&mut self, event: AuditEvent) -> AuditEntry {
        let entry = AuditEntry::new(self.sequence, event, self.current_hash.clone());

        // Update chain state
        self.current_hash = entry.entry_hash.clone();
        self.sequence += 1;

        entry
    }

    /// Verify chain integrity from genesis
    pub fn verify_chain_from_genesis(&self, entries: &[AuditEntry]) -> bool {
        if entries.is_empty() {
            return true;
        }

        // First entry must link to genesis
        if entries[0].previous_hash != self.genesis_hash {
            return false;
        }

        // Verify each entry
        for entry in entries {
            if !entry.verify_integrity() {
                return false;
            }
        }

        // Verify chain links
        for i in 1..entries.len() {
            if !entries[i].verify_chain(&entries[i - 1].entry_hash) {
                return false;
            }
        }

        true
    }
}

impl Default for HashChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuditAction;

    #[test]
    fn test_genesis_hash_deterministic() {
        let chain1 = HashChain::new();
        let chain2 = HashChain::new();

        assert_eq!(chain1.genesis_hash, chain2.genesis_hash);
        assert_eq!(chain1.current_hash, chain2.current_hash);
    }

    #[test]
    fn test_entry_hash_computation() {
        let event = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );

        let entry = AuditEntry::new(0, event, "previous_hash".to_string());

        assert!(!entry.entry_hash.is_empty());
        assert_eq!(entry.entry_hash.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_entry_integrity_verification() {
        let event = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );

        let entry = AuditEntry::new(0, event, "previous_hash".to_string());

        assert!(entry.verify_integrity());
    }

    #[test]
    fn test_entry_tampering_detection() {
        let event = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );

        let mut entry = AuditEntry::new(0, event, "previous_hash".to_string());

        // Tamper with the entry
        entry.event.actor = "attacker".to_string();

        // Verification should fail
        assert!(!entry.verify_integrity());
    }

    #[test]
    fn test_chain_creation() {
        let mut chain = HashChain::new();

        let event1 = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );
        let entry1 = chain.create_entry(event1);

        assert_eq!(entry1.sequence, 0);
        assert_eq!(entry1.previous_hash, chain.genesis_hash);

        let event2 = AuditEvent::new(
            "tx_2".to_string(),
            "actor_2".to_string(),
            AuditAction::TransactionCreated,
        );
        let entry2 = chain.create_entry(event2);

        assert_eq!(entry2.sequence, 1);
        assert_eq!(entry2.previous_hash, entry1.entry_hash);
    }

    #[test]
    fn test_chain_verification() {
        let mut chain = HashChain::new();

        let event1 = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );
        let entry1 = chain.create_entry(event1);

        let event2 = AuditEvent::new(
            "tx_2".to_string(),
            "actor_2".to_string(),
            AuditAction::TransactionCreated,
        );
        let entry2 = chain.create_entry(event2);

        let entries = vec![entry1, entry2];

        assert!(chain.verify_chain_from_genesis(&entries));
    }

    #[test]
    fn test_chain_tampering_detection() {
        let mut chain = HashChain::new();

        let event1 = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );
        let entry1 = chain.create_entry(event1);

        let event2 = AuditEvent::new(
            "tx_2".to_string(),
            "actor_2".to_string(),
            AuditAction::TransactionCreated,
        );
        let mut entry2 = chain.create_entry(event2);

        // Tamper with entry2
        entry2.event.actor = "attacker".to_string();

        let entries = vec![entry1, entry2];

        // Verification should fail
        assert!(!chain.verify_chain_from_genesis(&entries));
    }

    #[test]
    fn test_broken_chain_link_detection() {
        let mut chain = HashChain::new();

        let event1 = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        );
        let entry1 = chain.create_entry(event1);

        let event2 = AuditEvent::new(
            "tx_2".to_string(),
            "actor_2".to_string(),
            AuditAction::TransactionCreated,
        );
        let mut entry2 = chain.create_entry(event2);

        // Break the chain link
        entry2.previous_hash = "invalid_hash".to_string();

        let entries = vec![entry1, entry2];

        // Verification should fail
        assert!(!chain.verify_chain_from_genesis(&entries));
    }
}
