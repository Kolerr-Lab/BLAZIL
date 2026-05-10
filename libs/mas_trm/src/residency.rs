use crate::region::{DataClassification, Region};

/// A single rule mapping a data classification to its permitted storage regions.
#[derive(Debug, Clone)]
pub struct ResidencyRule {
    /// The data classification this rule applies to.
    pub classification: DataClassification,
    /// Regions where data of this classification may legally reside.
    pub allowed_regions: Vec<Region>,
}

/// Outcome of a residency check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidencyCheck {
    /// The data transfer or storage is permitted under the active policy.
    Permitted,
    /// The data transfer or storage is denied.
    Denied {
        /// Human-readable explanation suitable for audit logs.
        reason: String,
    },
}

impl ResidencyCheck {
    /// Returns `true` if this check is [`ResidencyCheck::Permitted`].
    pub fn is_permitted(&self) -> bool {
        matches!(self, ResidencyCheck::Permitted)
    }
}

/// Policy governing where data of each classification may reside.
///
/// Construct via [`ResidencyPolicy::mas_compliant()`] for the MAS TRM §6.3
/// baseline, or build manually for custom deployments.
///
/// Rules are evaluated in insertion order; **the first matching classification wins**.
#[derive(Debug, Clone)]
pub struct ResidencyPolicy {
    rules: Vec<ResidencyRule>,
}

impl ResidencyPolicy {
    /// Returns the MAS TRM §6.3 compliant baseline policy.
    ///
    /// | Classification        | Permitted regions              |
    /// |-----------------------|-------------------------------|
    /// | SensitivePersonalData | Singapore only                 |
    /// | PersonalData          | Singapore only                 |
    /// | Confidential          | Singapore, United States       |
    /// | Internal              | Singapore, United States, EU   |
    /// | Public                | Singapore, United States, EU   |
    ///
    /// `Unknown` region is always denied (fail-closed).
    pub fn mas_compliant() -> Self {
        ResidencyPolicy {
            rules: vec![
                ResidencyRule {
                    classification: DataClassification::SensitivePersonalData,
                    allowed_regions: vec![Region::Singapore],
                },
                ResidencyRule {
                    classification: DataClassification::PersonalData,
                    allowed_regions: vec![Region::Singapore],
                },
                ResidencyRule {
                    classification: DataClassification::Confidential,
                    allowed_regions: vec![Region::Singapore, Region::UnitedStates],
                },
                ResidencyRule {
                    classification: DataClassification::Internal,
                    allowed_regions: vec![Region::Singapore, Region::UnitedStates, Region::Europe],
                },
                ResidencyRule {
                    classification: DataClassification::Public,
                    allowed_regions: vec![Region::Singapore, Region::UnitedStates, Region::Europe],
                },
            ],
        }
    }

    /// Checks whether storing `classification` data in `target` region is permitted.
    ///
    /// **Fail-closed**: if no rule is found for the given classification, or the
    /// target region is not in the rule's allow-list, returns [`ResidencyCheck::Denied`].
    pub fn check(&self, classification: DataClassification, target: Region) -> ResidencyCheck {
        for rule in &self.rules {
            if rule.classification == classification {
                if rule.allowed_regions.contains(&target) {
                    return ResidencyCheck::Permitted;
                }
                return ResidencyCheck::Denied {
                    reason: format!("{classification:?} data may not reside in {target:?}"),
                };
            }
        }
        // No rule found → fail-closed.
        ResidencyCheck::Denied {
            reason: format!("no residency rule configured for {classification:?}"),
        }
    }

    /// Appends a rule to this policy.
    ///
    /// Rules are evaluated in order. Appended rules will not override existing
    /// rules for the same classification because the first match wins.
    pub fn add_rule(&mut self, rule: ResidencyRule) {
        self.rules.push(rule);
    }
}
