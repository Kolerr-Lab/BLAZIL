use chrono::{DateTime, Months, Utc};
use serde::{Deserialize, Serialize};

use crate::region::Region;

/// Categories of records subject to MAS/FinCEN retention obligations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// Customer transaction records — 5 years (MAS Notice 626 §6).
    TransactionRecord,
    /// KYC/CDD records — 5 years from end of customer relationship
    /// (MAS Notice 626 §16).
    KycRecord,
    /// System and user audit logs — 5 years (MAS TRM §9.3).
    AuditLog,
    /// Suspicious Activity Reports — 5 years from the **filing date**
    /// (FinCEN 31 CFR §1020.320(d); MAS Notice 626 aligns).
    ///
    /// See [`RetentionRecord::sar_filed_date`] for the dual-date tracking design.
    SarReport,
    /// Infrastructure and application system logs — minimum 1 year (MAS TRM §9.3).
    SystemLog,
    /// Customer consent records — duration of purpose + 1 year (PDPA §25).
    /// Modelled as 2 years to provide a conservative minimum floor.
    ConsentRecord,
}

impl RetentionClass {
    /// Returns the mandatory minimum retention period in whole years.
    pub fn retention_years(self) -> u32 {
        match self {
            RetentionClass::TransactionRecord => 5,
            RetentionClass::KycRecord => 5,
            RetentionClass::AuditLog => 5,
            RetentionClass::SarReport => 5,
            RetentionClass::SystemLog => 1,
            RetentionClass::ConsentRecord => 2,
        }
    }
}

/// A single data record subject to a retention obligation.
///
/// # SAR dual-date design
///
/// FinCEN 31 CFR §1020.320(d) requires that SAR records be retained for
/// **5 years from the date of filing**, not from the transaction date.
/// MAS Notice 626 aligns with this principle for Singapore filings.
///
/// To correctly model this, [`RetentionRecord`] carries two date fields:
///
/// - `transaction_date` — always populated; the date the underlying event occurred.
/// - `sar_filed_date`   — populated only after the SAR is filed with the regulator.
///
/// [`RetentionRecord::purge_after`] automatically selects the correct anchor
/// date based on the classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionRecord {
    /// Opaque record identifier (e.g. transaction ID, KYC case ID, SAR reference).
    pub id: String,
    /// Retention class governing the purge deadline.
    pub classification: RetentionClass,
    /// Region in which this record is stored.
    pub region: Region,
    /// Date the underlying event (transaction, filing, audit entry) occurred.
    ///
    /// Always populated. For most classes this is the retention anchor date.
    pub transaction_date: DateTime<Utc>,
    /// Date the SAR was filed with the regulator.
    ///
    /// **Only meaningful for [`RetentionClass::SarReport`].**
    ///
    /// Per FinCEN 31 CFR §1020.320(d), the 5-year retention window begins
    /// from the filing date. When `None`, the clock falls back to
    /// `transaction_date` (conservative — produces a longer retention window).
    pub sar_filed_date: Option<DateTime<Utc>>,
}

impl RetentionRecord {
    /// Creates a new retention record.
    ///
    /// Set `sar_filed_date` to `Some(date)` only for [`RetentionClass::SarReport`]
    /// records, and only once the SAR has been formally filed with the regulator.
    pub fn new(
        id: impl Into<String>,
        classification: RetentionClass,
        region: Region,
        transaction_date: DateTime<Utc>,
        sar_filed_date: Option<DateTime<Utc>>,
    ) -> Self {
        RetentionRecord {
            id: id.into(),
            classification,
            region,
            transaction_date,
            sar_filed_date,
        }
    }

    /// Returns the date from which the retention period is measured.
    ///
    /// | Classification | Anchor date                                            |
    /// |----------------|--------------------------------------------------------|
    /// | `SarReport`    | `sar_filed_date` if set; else `transaction_date`       |
    /// | All others     | `transaction_date`                                     |
    fn retention_anchor(&self) -> DateTime<Utc> {
        match self.classification {
            RetentionClass::SarReport => self.sar_filed_date.unwrap_or(self.transaction_date),
            _ => self.transaction_date,
        }
    }

    /// Returns the earliest date on which this record may be purged.
    pub fn purge_after(&self) -> DateTime<Utc> {
        self.retention_anchor() + Months::new(self.classification.retention_years() * 12)
    }

    /// Returns `true` if the mandatory retention period has expired.
    pub fn is_eligible_for_purge(&self) -> bool {
        Utc::now() >= self.purge_after()
    }

    /// Returns the number of days until this record may be purged.
    ///
    /// Returns a negative or zero value if the record is already eligible.
    pub fn days_until_purge(&self) -> i64 {
        (self.purge_after() - Utc::now()).num_days()
    }
}
