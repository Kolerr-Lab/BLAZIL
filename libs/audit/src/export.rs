//! Audit Log Export — JSON and CEF Format Support

use crate::{AuditEntry, AuditLog};
use chrono::SecondsFormat;

/// Export format for audit logs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// JSON format (default)
    Json,
    /// Common Event Format (CEF) - Syslog compatible
    Cef,
}

/// Log exporter for compliance reporting
pub struct LogExporter;

impl LogExporter {
    /// Export audit log in specified format
    pub async fn export(
        log: &AuditLog,
        format: ExportFormat,
        start: Option<u64>,
        end: Option<u64>,
    ) -> String {
        match format {
            ExportFormat::Json => log.export_json(start, end).await,
            ExportFormat::Cef => Self::export_cef(log, start, end).await,
        }
    }

    /// Export to CEF (Common Event Format)
    ///
    /// CEF Format: CEF:Version|Device Vendor|Device Product|Device Version|Signature ID|Name|Severity|Extension
    ///
    /// Example:
    /// ```text
    /// CEF:0|Blazil|TransactionEngine|0.3.2|TXN_CREATED|Transaction Created|5|
    /// src=user_alice dst=ledger txId=tx_12345 outcome=success latency=1500000
    /// ```
    async fn export_cef(log: &AuditLog, start: Option<u64>, end: Option<u64>) -> String {
        let entries = if let (Some(s), Some(e)) = (start, end) {
            log.get_range(s, e).await
        } else {
            log.get_all().await
        };

        let mut output = String::new();

        for entry in entries {
            let cef_line = Self::entry_to_cef(&entry);
            output.push_str(&cef_line);
            output.push('\n');
        }

        output
    }

    /// Convert a single audit entry to CEF format
    fn entry_to_cef(entry: &AuditEntry) -> String {
        // CEF Header
        let version = "0";
        let vendor = "Blazil";
        let product = "TransactionEngine";
        let product_version = "0.3.2";
        let signature_id = format!("{:?}", entry.event.action);
        let name = Self::action_to_name(&entry.event.action);
        let severity = Self::result_to_severity(&entry.event.result);

        // CEF Extensions
        let timestamp = entry
            .event
            .timestamp
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let src = &entry.event.actor;
        let tx_id = &entry.event.transaction_id;
        let outcome = format!("{:?}", entry.event.result).to_lowercase();
        let seq = entry.sequence;
        let hash = &entry.entry_hash[..16]; // First 16 chars of hash

        let mut extensions = format!(
            "rt={} src={} txId={} outcome={} seq={} hash={}",
            timestamp, src, tx_id, outcome, seq, hash
        );

        if let Some(latency_ns) = entry.event.latency_ns {
            extensions.push_str(&format!(" latency={}", latency_ns));
        }

        if let Some(ref metadata) = entry.event.metadata {
            if let Ok(meta_str) = serde_json::to_string(metadata) {
                extensions.push_str(&format!(" metadata={}", meta_str));
            }
        }

        if let Some(ref error) = entry.event.error {
            extensions.push_str(&format!(" error={}", error.replace('|', "\\|")));
        }

        format!(
            "CEF:{}|{}|{}|{}|{}|{}|{}|{}",
            version, vendor, product, product_version, signature_id, name, severity, extensions
        )
    }

    fn action_to_name(action: &crate::AuditAction) -> String {
        match action {
            crate::AuditAction::TransactionCreated => "Transaction Created",
            crate::AuditAction::TransactionValidated => "Transaction Validated",
            crate::AuditAction::ComplianceScreeningStarted => "Compliance Screening Started",
            crate::AuditAction::ComplianceScreeningCompleted => "Compliance Screening Completed",
            crate::AuditAction::LedgerSubmitted => "Ledger Submitted",
            crate::AuditAction::LedgerCommitted => "Ledger Committed",
            crate::AuditAction::TransactionCompleted => "Transaction Completed",
            crate::AuditAction::TransactionRejected => "Transaction Rejected",
            crate::AuditAction::TransactionHeld => "Transaction Held",
            crate::AuditAction::TransactionReleased => "Transaction Released",
            crate::AuditAction::SarGenerated => "SAR Generated",
            crate::AuditAction::AccessControlCheck => "Access Control Check",
            crate::AuditAction::ApiAuthentication => "API Authentication",
            crate::AuditAction::ConfigurationChanged => "Configuration Changed",
        }
        .to_string()
    }

    fn result_to_severity(result: &crate::AuditResult) -> u8 {
        match result {
            crate::AuditResult::Success => 5, // Informational
            crate::AuditResult::Pending => 6, // Informational
            crate::AuditResult::Failure => 8, // High severity
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuditAction, AuditEvent};

    #[tokio::test]
    async fn test_export_json() {
        let log = AuditLog::new();

        let event = AuditEvent::new(
            "tx_1".to_string(),
            "actor_1".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success");

        log.record(event).await;

        let output = LogExporter::export(&log, ExportFormat::Json, None, None).await;

        assert!(output.contains("tx_1"));
        assert!(output.contains("actor_1"));
        assert!(output.contains("transaction_id"));
    }

    #[tokio::test]
    async fn test_export_cef() {
        let log = AuditLog::new();

        let event = AuditEvent::new(
            "tx_1".to_string(),
            "user_alice".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success");

        log.record(event).await;

        let output = LogExporter::export(&log, ExportFormat::Cef, None, None).await;

        assert!(output.contains("CEF:0"));
        assert!(output.contains("|Blazil|"));
        assert!(output.contains("|TransactionEngine|"));
        assert!(output.contains("src=user_alice"));
        assert!(output.contains("txId=tx_1"));
        assert!(output.contains("outcome=success"));
    }

    #[tokio::test]
    async fn test_cef_format_structure() {
        let log = AuditLog::new();

        let event = AuditEvent::new(
            "tx_12345".to_string(),
            "service_api".to_string(),
            AuditAction::LedgerCommitted,
        )
        .with_result("success");

        log.record(event).await;

        let output = LogExporter::export(&log, ExportFormat::Cef, None, None).await;

        // Verify CEF structure
        let parts: Vec<&str> = output.split('|').collect();
        assert_eq!(parts[0], "CEF:0"); // Version
        assert_eq!(parts[1], "Blazil"); // Vendor
        assert_eq!(parts[2], "TransactionEngine"); // Product
        assert_eq!(parts[3], "0.3.2"); // Version
        assert_eq!(parts[4], "LedgerCommitted"); // Signature ID (Debug format)
    }

    #[tokio::test]
    async fn test_export_range() {
        let log = AuditLog::new();

        for i in 0..10 {
            let event = AuditEvent::new(
                format!("tx_{}", i),
                "actor_1".to_string(),
                AuditAction::TransactionCreated,
            );
            log.record(event).await;
        }

        let output = LogExporter::export(&log, ExportFormat::Json, Some(3), Some(7)).await;

        // Should only contain 4 entries (sequences 3, 4, 5, 6)
        let count = output.matches("\"sequence\":").count();
        assert_eq!(count, 4);
    }

    #[test]
    fn test_severity_mapping() {
        use crate::AuditResult;

        assert_eq!(LogExporter::result_to_severity(&AuditResult::Success), 5);
        assert_eq!(LogExporter::result_to_severity(&AuditResult::Pending), 6);
        assert_eq!(LogExporter::result_to_severity(&AuditResult::Failure), 8);
    }
}
