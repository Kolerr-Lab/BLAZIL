use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Probability that a risk event will occur, per MAS TRM Chapter 3 risk matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Likelihood {
    /// Extremely unlikely; not expected to occur over the planning horizon.
    Rare = 1,
    /// Unlikely to occur but conceivable given the threat landscape.
    Unlikely = 2,
    /// May occur occasionally under normal operating conditions.
    Possible = 3,
    /// Will probably occur in most circumstances.
    Likely = 4,
    /// Expected to occur frequently or on demand.
    AlmostCertain = 5,
}

/// Severity of the consequence if a risk event materialises, per MAS TRM Chapter 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Impact {
    /// Negligible financial, operational, or reputational effect.
    Negligible = 1,
    /// Minor disruption with rapid self-recovery; no regulatory notification required.
    Minor = 2,
    /// Moderate impact requiring managed recovery; possible regulatory notification.
    Moderate = 3,
    /// Major disruption with significant regulatory or financial consequences.
    Major = 4,
    /// Catastrophic impact; potential threat to solvency, licence, or market stability.
    Catastrophic = 5,
}

/// Composite risk score derived from `Likelihood × Impact` (range 1–25).
///
/// Only constructible via [`RiskScore::new`] to preserve the 1–25 invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RiskScore(u8);

impl RiskScore {
    /// Computes the score as `likelihood × impact`.
    ///
    /// The result is always in the range 1–25 (1×1 through 5×5).
    pub fn new(likelihood: Likelihood, impact: Impact) -> Self {
        RiskScore(likelihood as u8 * impact as u8)
    }

    /// Returns the raw numeric score (1–25).
    pub fn raw(self) -> u8 {
        self.0
    }

    /// Maps the raw score to a [`RiskRating`] band per MAS TRM Chapter 3.
    ///
    /// | Score | Rating   |
    /// |-------|----------|
    /// | 1–4   | Low      |
    /// | 5–9   | Medium   |
    /// | 10–16 | High     |
    /// | 17–25 | Critical |
    pub fn rating(self) -> RiskRating {
        match self.0 {
            1..=4 => RiskRating::Low,
            5..=9 => RiskRating::Medium,
            10..=16 => RiskRating::High,
            _ => RiskRating::Critical,
        }
    }
}

/// Qualitative risk band derived from a [`RiskScore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskRating {
    Low,
    Medium,
    High,
    Critical,
}

/// Risk treatment strategy per ISO 31000 / MAS TRM best practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreatmentStrategy {
    /// Accept the risk without further action (only valid for Low/Medium residual risk).
    Accept,
    /// Implement controls to reduce likelihood or impact.
    Mitigate,
    /// Transfer risk to a third party (insurance, outsourcing with SLA).
    Transfer,
    /// Discontinue the activity that gives rise to the risk.
    Avoid,
}

/// A plan describing how an identified risk will be treated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreatmentPlan {
    /// Treatment strategy selected for this risk.
    pub strategy: TreatmentStrategy,
    /// Description of specific controls or actions to be taken.
    pub description: String,
    /// Name or role of the person accountable for executing this plan.
    pub owner: String,
    /// Date by which the treatment must be fully implemented.
    pub target_date: DateTime<Utc>,
}

/// A complete IT risk assessment record per MAS TRM Chapter 3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    /// Unique identifier for this assessment (e.g. `"RISK-2026-001"`).
    pub id: String,
    /// Short human-readable title of the risk.
    pub title: String,
    /// Risk domain (e.g. `"Cybersecurity"`, `"Third-party"`, `"Operational"`).
    pub category: String,
    /// Risk score before any controls are applied.
    pub inherent_score: RiskScore,
    /// Plan for treating the inherent risk.
    pub treatment: TreatmentPlan,
    /// Expected risk score after the treatment plan is fully implemented.
    pub residual_score: RiskScore,
    /// Name or role of the reviewer who signed off on this assessment.
    pub reviewer: String,
    /// Date the assessment was reviewed and approved.
    pub reviewed_at: DateTime<Utc>,
}

impl RiskAssessment {
    /// Returns `true` if the residual risk rating is Low or Medium.
    ///
    /// Per MAS TRM §3, residual risk must not exceed Medium for an assessment
    /// to be considered acceptable without escalation to the Risk Committee.
    pub fn is_acceptable(&self) -> bool {
        matches!(
            self.residual_score.rating(),
            RiskRating::Low | RiskRating::Medium
        )
    }
}
