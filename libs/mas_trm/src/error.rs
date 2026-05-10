/// Errors produced by the `blazil-mas-trm` crate.
#[derive(Debug, thiserror::Error)]
pub enum MasTrmError {
    /// A data transfer or storage operation would violate the active residency policy.
    #[error("data residency violation: {reason}")]
    ResidencyViolation { reason: String },

    /// A retention record could not be created or its policy applied.
    #[error("retention policy error: {reason}")]
    RetentionPolicy { reason: String },

    /// A risk score value is outside the valid 1–25 range.
    #[error("invalid risk score {score}: must be between 1 and 25 inclusive")]
    InvalidRiskScore { score: u8 },

    /// A BCP target does not meet MAS TRM Chapter 7 thresholds.
    #[error("BCP compliance gap in '{system}': {gap}")]
    BcpComplianceGap { system: String, gap: String },
}
