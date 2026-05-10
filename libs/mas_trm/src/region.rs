use serde::{Deserialize, Serialize};

/// Geographic regions used for data residency enforcement.
///
/// Variants are stable — adding new regions is non-breaking because all match
/// arms must use `..` or explicit coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// Republic of Singapore — primary regulatory jurisdiction (MAS).
    Singapore,
    /// United States — FinCEN reporting obligation jurisdiction.
    UnitedStates,
    /// European Union member states — GDPR jurisdiction.
    Europe,
    /// Region could not be determined at classification time.
    Unknown,
}

impl Region {
    /// Returns `false` only for [`Region::Unknown`].
    pub fn is_known(self) -> bool {
        !matches!(self, Region::Unknown)
    }
}

/// MAS-aligned data sensitivity classifications.
///
/// Used by [`crate::residency::ResidencyPolicy`] to determine which storage
/// regions are permissible for each data class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    /// Non-sensitive data safe for public disclosure.
    Public,
    /// Internal operational data — not for external distribution.
    Internal,
    /// Commercially sensitive data (e.g. financial records, AML findings).
    Confidential,
    /// Personal data subject to PDPA / MAS Notice 626.
    PersonalData,
    /// Biometric, health, or other highly sensitive personal data.
    SensitivePersonalData,
}
