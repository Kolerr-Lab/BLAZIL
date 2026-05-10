//! MAS TRM (Technology Risk Management) compliance library for Blazil.
//!
//! Implements controls required by the Monetary Authority of Singapore
//! Technology Risk Management Guidelines (MAS TRM 2021), covering:
//!
//! - **Data residency enforcement** — §6.3: SG personal/financial data must remain in Singapore.
//! - **Retention policy management** — MAS Notice 626 + FinCEN 31 CFR §1020.320(d).
//! - **IT Risk Assessment framework** — Chapter 3: likelihood × impact risk matrix.
//! - **Business Continuity Planning** — Chapter 7: RTO/RPO/MTPD targets per criticality tier.

pub mod bcp;
pub mod error;
pub mod region;
pub mod residency;
pub mod retention;
pub mod risk;

pub use bcp::{BcpAssessment, BcpTarget, SystemCriticality};
pub use error::MasTrmError;
pub use region::{DataClassification, Region};
pub use residency::{ResidencyCheck, ResidencyPolicy, ResidencyRule};
pub use retention::{RetentionClass, RetentionRecord};
pub use risk::{
    Impact, Likelihood, RiskAssessment, RiskRating, RiskScore, TreatmentPlan, TreatmentStrategy,
};

#[cfg(test)]
mod tests;
