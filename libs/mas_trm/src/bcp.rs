use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Criticality tier for a system or service, per MAS TRM Chapter 7.
///
/// Determines the maximum allowable RTO and RPO thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SystemCriticality {
    /// Loss causes minimal impact; extended recovery acceptable.
    Low,
    /// Loss causes moderate impact; recovery within one business day.
    Medium,
    /// Loss causes significant operational or financial impact.
    High,
    /// Loss causes immediate, severe impact (payment rails, core banking ledger).
    Critical,
}

impl SystemCriticality {
    /// Maximum Recovery Time Objective in hours for this criticality tier.
    ///
    /// | Tier     | Max RTO |
    /// |----------|---------|
    /// | Critical | 4 h     |
    /// | High     | 8 h     |
    /// | Medium   | 24 h    |
    /// | Low      | 72 h    |
    pub fn max_rto_hours(self) -> u32 {
        match self {
            SystemCriticality::Critical => 4,
            SystemCriticality::High => 8,
            SystemCriticality::Medium => 24,
            SystemCriticality::Low => 72,
        }
    }

    /// Maximum Recovery Point Objective in hours for this criticality tier.
    ///
    /// RPO thresholds are aligned with RTO thresholds per MAS TRM Chapter 7.
    pub fn max_rpo_hours(self) -> u32 {
        match self {
            SystemCriticality::Critical => 4,
            SystemCriticality::High => 8,
            SystemCriticality::Medium => 24,
            SystemCriticality::Low => 72,
        }
    }
}

/// A BCP target defining recovery objectives for a single system or service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BcpTarget {
    /// Name of the system or service (e.g. `"TigerBeetle Ledger"`).
    pub system_name: String,
    /// Criticality tier determining MAS TRM compliance thresholds.
    pub criticality: SystemCriticality,
    /// Committed Recovery Time Objective in hours.
    pub rto_hours: u32,
    /// Committed Recovery Point Objective in hours.
    pub rpo_hours: u32,
    /// Maximum Tolerable Period of Disruption in hours.
    ///
    /// Must be ≥ RTO. Informs the Business Impact Analysis but is not
    /// directly constrained by MAS TRM Chapter 7.
    pub mtpd_hours: u32,
}

impl BcpTarget {
    /// Returns `true` if both RTO and RPO meet MAS TRM Chapter 7 thresholds.
    pub fn is_mas_compliant(&self) -> bool {
        self.rto_hours <= self.criticality.max_rto_hours()
            && self.rpo_hours <= self.criticality.max_rpo_hours()
    }

    /// Returns a human-readable description of which targets are missed, or `None` if compliant.
    pub fn compliance_gap(&self) -> Option<String> {
        if self.is_mas_compliant() {
            return None;
        }
        let mut gaps: Vec<String> = Vec::new();
        if self.rto_hours > self.criticality.max_rto_hours() {
            gaps.push(format!(
                "RTO {}h exceeds MAS limit {}h",
                self.rto_hours,
                self.criticality.max_rto_hours()
            ));
        }
        if self.rpo_hours > self.criticality.max_rpo_hours() {
            gaps.push(format!(
                "RPO {}h exceeds MAS limit {}h",
                self.rpo_hours,
                self.criticality.max_rpo_hours()
            ));
        }
        Some(gaps.join("; "))
    }
}

/// An organisation-wide BCP assessment covering multiple systems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BcpAssessment {
    /// All systems and services included in this assessment.
    pub targets: Vec<BcpTarget>,
    /// Date of the most recent BCP test exercise.
    pub last_tested: DateTime<Utc>,
    /// Scheduled date for the next mandatory BCP test.
    ///
    /// MAS TRM §7.5 requires testing at least annually.
    pub next_test_due: DateTime<Utc>,
}

impl BcpAssessment {
    /// Returns `true` if every system meets its MAS TRM RTO/RPO obligations.
    pub fn all_compliant(&self) -> bool {
        self.targets.iter().all(BcpTarget::is_mas_compliant)
    }

    /// Returns all systems that do not meet their MAS TRM obligations.
    pub fn non_compliant(&self) -> Vec<&BcpTarget> {
        self.targets
            .iter()
            .filter(|t| !t.is_mas_compliant())
            .collect()
    }

    /// Returns `true` if the next scheduled BCP test is overdue.
    pub fn is_test_overdue(&self) -> bool {
        Utc::now() > self.next_test_due
    }
}
