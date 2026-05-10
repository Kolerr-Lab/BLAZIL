// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

// This file is `mod tests` (declared in lib.rs with `#[cfg(test)]`).
// No inner module wrapper needed.

use chrono::{Duration, Utc};

use crate::{
    bcp::{BcpAssessment, BcpTarget, SystemCriticality},
    region::{DataClassification, Region},
    residency::{ResidencyCheck, ResidencyPolicy, ResidencyRule},
    retention::{RetentionClass, RetentionRecord},
    risk::{
        Impact, Likelihood, RiskAssessment, RiskRating, RiskScore, TreatmentPlan, TreatmentStrategy,
    },
};

// ── Shared test helpers ────────────────────────────────────────────────────────

fn make_risk_assessment(residual: RiskScore) -> RiskAssessment {
    RiskAssessment {
        id: "RISK-2026-001".to_string(),
        title: "Ransomware attack on core banking".to_string(),
        category: "Cybersecurity".to_string(),
        inherent_score: RiskScore::new(Likelihood::Possible, Impact::Catastrophic),
        treatment: TreatmentPlan {
            strategy: TreatmentStrategy::Mitigate,
            description: "Deploy EDR, air-gap backups, and immutable ledger snapshots.".to_string(),
            owner: "CISO".to_string(),
            target_date: Utc::now() + Duration::days(90),
        },
        residual_score: residual,
        reviewer: "Board Risk Committee".to_string(),
        reviewed_at: Utc::now(),
    }
}

fn make_bcp_target(name: &str, criticality: SystemCriticality, rto: u32, rpo: u32) -> BcpTarget {
    BcpTarget {
        system_name: name.to_string(),
        criticality,
        rto_hours: rto,
        rpo_hours: rpo,
        mtpd_hours: rto * 2,
    }
}

fn make_bcp_assessment(targets: Vec<BcpTarget>) -> BcpAssessment {
    BcpAssessment {
        targets,
        last_tested: Utc::now() - Duration::days(30),
        next_test_due: Utc::now() + Duration::days(335),
    }
}

/// Creates a TransactionRecord that was created `years_ago` years in the past.
fn past_tx_record(years_ago: i64) -> RetentionRecord {
    RetentionRecord::new(
        "tx_test",
        RetentionClass::TransactionRecord,
        Region::Singapore,
        Utc::now() - Duration::days(years_ago * 365),
        None,
    )
}

// ── Region ─────────────────────────────────────────────────────────────────────

#[test]
fn test_region_concrete_variants_are_known() {
    assert!(Region::Singapore.is_known());
    assert!(Region::UnitedStates.is_known());
    assert!(Region::Europe.is_known());
}

#[test]
fn test_region_unknown_is_not_known() {
    assert!(!Region::Unknown.is_known());
}

// ── ResidencyPolicy ────────────────────────────────────────────────────────────

#[test]
fn test_residency_personal_data_permitted_in_sg() {
    let policy = ResidencyPolicy::mas_compliant();
    assert!(policy
        .check(DataClassification::PersonalData, Region::Singapore)
        .is_permitted());
}

#[test]
fn test_residency_personal_data_denied_in_us() {
    let policy = ResidencyPolicy::mas_compliant();
    let check = policy.check(DataClassification::PersonalData, Region::UnitedStates);
    assert!(!check.is_permitted());
    assert!(matches!(check, ResidencyCheck::Denied { .. }));
}

#[test]
fn test_residency_personal_data_denied_in_europe() {
    let policy = ResidencyPolicy::mas_compliant();
    assert!(!policy
        .check(DataClassification::PersonalData, Region::Europe)
        .is_permitted());
}

#[test]
fn test_residency_sensitive_personal_data_sg_only() {
    let policy = ResidencyPolicy::mas_compliant();
    assert!(policy
        .check(DataClassification::SensitivePersonalData, Region::Singapore)
        .is_permitted());
    assert!(!policy
        .check(
            DataClassification::SensitivePersonalData,
            Region::UnitedStates
        )
        .is_permitted());
    assert!(!policy
        .check(DataClassification::SensitivePersonalData, Region::Europe)
        .is_permitted());
}

#[test]
fn test_residency_confidential_permitted_in_sg_and_us() {
    let policy = ResidencyPolicy::mas_compliant();
    assert!(policy
        .check(DataClassification::Confidential, Region::Singapore)
        .is_permitted());
    assert!(policy
        .check(DataClassification::Confidential, Region::UnitedStates)
        .is_permitted());
}

#[test]
fn test_residency_confidential_denied_in_europe() {
    let policy = ResidencyPolicy::mas_compliant();
    assert!(!policy
        .check(DataClassification::Confidential, Region::Europe)
        .is_permitted());
}

#[test]
fn test_residency_public_data_permitted_in_all_known_regions() {
    let policy = ResidencyPolicy::mas_compliant();
    for region in [Region::Singapore, Region::UnitedStates, Region::Europe] {
        assert!(
            policy
                .check(DataClassification::Public, region)
                .is_permitted(),
            "Public data must be permitted in {region:?}"
        );
    }
}

#[test]
fn test_residency_internal_data_permitted_in_all_known_regions() {
    let policy = ResidencyPolicy::mas_compliant();
    for region in [Region::Singapore, Region::UnitedStates, Region::Europe] {
        assert!(
            policy
                .check(DataClassification::Internal, region)
                .is_permitted(),
            "Internal data must be permitted in {region:?}"
        );
    }
}

#[test]
fn test_residency_fail_closed_for_unknown_region() {
    // Unknown region must be denied for every classification.
    let policy = ResidencyPolicy::mas_compliant();
    for cls in [
        DataClassification::Public,
        DataClassification::Internal,
        DataClassification::Confidential,
        DataClassification::PersonalData,
        DataClassification::SensitivePersonalData,
    ] {
        assert!(
            !policy.check(cls, Region::Unknown).is_permitted(),
            "{cls:?} must be denied in Unknown region"
        );
    }
}

#[test]
fn test_residency_denied_check_contains_reason() {
    let policy = ResidencyPolicy::mas_compliant();
    let check = policy.check(DataClassification::PersonalData, Region::UnitedStates);
    match check {
        ResidencyCheck::Denied { reason } => {
            assert!(!reason.is_empty(), "denial reason must not be empty");
        }
        ResidencyCheck::Permitted => panic!("expected Denied"),
    }
}

#[test]
fn test_residency_add_rule_appends_without_overriding_existing() {
    let mut policy = ResidencyPolicy::mas_compliant();
    // Append a rule that would permit PersonalData in Europe.
    // Because add_rule appends and first-match-wins, the existing SG-only
    // rule for PersonalData still takes precedence.
    policy.add_rule(ResidencyRule {
        classification: DataClassification::PersonalData,
        allowed_regions: vec![Region::Europe],
    });
    // Original SG permission still works.
    assert!(policy
        .check(DataClassification::PersonalData, Region::Singapore)
        .is_permitted());
    // Europe is still denied (first SG-only rule matched before the new one).
    assert!(!policy
        .check(DataClassification::PersonalData, Region::Europe)
        .is_permitted());
}

// ── RetentionClass ─────────────────────────────────────────────────────────────

#[test]
fn test_retention_years_all_classes() {
    assert_eq!(RetentionClass::TransactionRecord.retention_years(), 5);
    assert_eq!(RetentionClass::KycRecord.retention_years(), 5);
    assert_eq!(RetentionClass::AuditLog.retention_years(), 5);
    assert_eq!(RetentionClass::SarReport.retention_years(), 5);
    assert_eq!(RetentionClass::SystemLog.retention_years(), 1);
    assert_eq!(RetentionClass::ConsentRecord.retention_years(), 2);
}

// ── RetentionRecord — general ──────────────────────────────────────────────────

#[test]
fn test_retention_not_eligible_when_fresh() {
    // Created 1 day ago — well inside any retention window.
    let record = past_tx_record(0);
    assert!(!record.is_eligible_for_purge());
    assert!(record.days_until_purge() > 0);
}

#[test]
fn test_retention_eligible_after_5_years_for_tx_record() {
    // Created 6 years ago → past the 5-year window.
    let record = past_tx_record(6);
    assert!(record.is_eligible_for_purge());
    assert!(record.days_until_purge() <= 0);
}

#[test]
fn test_retention_system_log_eligible_after_1_year() {
    let record = RetentionRecord::new(
        "syslog_001",
        RetentionClass::SystemLog,
        Region::Singapore,
        Utc::now() - Duration::days(400), // ~13 months ago
        None,
    );
    assert!(record.is_eligible_for_purge());
}

#[test]
fn test_retention_system_log_not_eligible_before_1_year() {
    let record = RetentionRecord::new(
        "syslog_002",
        RetentionClass::SystemLog,
        Region::Singapore,
        Utc::now() - Duration::days(30), // 1 month ago
        None,
    );
    assert!(!record.is_eligible_for_purge());
}

#[test]
fn test_retention_consent_record_eligible_after_2_years() {
    let record = RetentionRecord::new(
        "consent_001",
        RetentionClass::ConsentRecord,
        Region::Singapore,
        Utc::now() - Duration::days(3 * 365), // 3 years ago
        None,
    );
    assert!(record.is_eligible_for_purge());
}

#[test]
fn test_retention_kyc_record_not_eligible_at_4_years() {
    let record = RetentionRecord::new(
        "kyc_001",
        RetentionClass::KycRecord,
        Region::Singapore,
        Utc::now() - Duration::days(4 * 365), // 4 years ago, window = 5
        None,
    );
    assert!(!record.is_eligible_for_purge());
    assert!(record.days_until_purge() > 0);
}

// ── RetentionRecord — SAR dual-date logic ──────────────────────────────────────

#[test]
fn test_sar_retention_uses_filed_date_not_transaction_date() {
    // Transaction: 4 years ago. Filing: 3 years ago. → 2 years remain.
    let sar = RetentionRecord::new(
        "sar_001",
        RetentionClass::SarReport,
        Region::Singapore,
        Utc::now() - Duration::days(4 * 365),
        Some(Utc::now() - Duration::days(3 * 365)),
    );
    // Retention clock starts from filing date (3 years ago). 5 - 3 = 2 years remain.
    assert!(!sar.is_eligible_for_purge());
    assert!(sar.days_until_purge() > 0);
}

#[test]
fn test_sar_retention_eligible_5_years_after_filing() {
    // Transaction: 7 years ago. Filing: 6 years ago. → filing window expired.
    let sar = RetentionRecord::new(
        "sar_002",
        RetentionClass::SarReport,
        Region::Singapore,
        Utc::now() - Duration::days(7 * 365),
        Some(Utc::now() - Duration::days(6 * 365)),
    );
    assert!(sar.is_eligible_for_purge());
}

#[test]
fn test_sar_retention_falls_back_to_transaction_date_when_not_filed() {
    // sar_filed_date = None → fall back to transaction_date (6 years ago → eligible).
    let sar = RetentionRecord::new(
        "sar_003",
        RetentionClass::SarReport,
        Region::Singapore,
        Utc::now() - Duration::days(6 * 365),
        None,
    );
    assert!(sar.is_eligible_for_purge());
}

#[test]
fn test_sar_retention_not_eligible_when_recently_filed() {
    // Filed 1 year ago → 4 more years to go.
    let sar = RetentionRecord::new(
        "sar_004",
        RetentionClass::SarReport,
        Region::Singapore,
        Utc::now() - Duration::days(2 * 365),
        Some(Utc::now() - Duration::days(365)),
    );
    assert!(!sar.is_eligible_for_purge());
    assert!(sar.days_until_purge() > 0);
}

#[test]
fn test_sar_filed_later_than_transaction_extends_window() {
    // Transaction 6 years ago but filed only 2 years ago → still 3 years to go.
    let sar = RetentionRecord::new(
        "sar_005",
        RetentionClass::SarReport,
        Region::Singapore,
        Utc::now() - Duration::days(6 * 365), // would be eligible if anchor = tx_date
        Some(Utc::now() - Duration::days(2 * 365)), // filed 2 years ago → not yet eligible
    );
    assert!(!sar.is_eligible_for_purge());
}

// ── RiskScore ──────────────────────────────────────────────────────────────────

#[test]
fn test_risk_score_minimum_is_low() {
    // Rare(1) × Negligible(1) = 1 → Low
    let score = RiskScore::new(Likelihood::Rare, Impact::Negligible);
    assert_eq!(score.raw(), 1);
    assert_eq!(score.rating(), RiskRating::Low);
}

#[test]
fn test_risk_score_4_is_low_upper_boundary() {
    // Unlikely(2) × Minor(2) = 4 → Low
    let score = RiskScore::new(Likelihood::Unlikely, Impact::Minor);
    assert_eq!(score.raw(), 4);
    assert_eq!(score.rating(), RiskRating::Low);
}

#[test]
fn test_risk_score_5_is_medium_lower_boundary() {
    // Rare(1) × Catastrophic(5) = 5 → Medium
    let score = RiskScore::new(Likelihood::Rare, Impact::Catastrophic);
    assert_eq!(score.raw(), 5);
    assert_eq!(score.rating(), RiskRating::Medium);
}

#[test]
fn test_risk_score_9_is_medium_upper_boundary() {
    // Possible(3) × Moderate(3) = 9 → Medium
    let score = RiskScore::new(Likelihood::Possible, Impact::Moderate);
    assert_eq!(score.raw(), 9);
    assert_eq!(score.rating(), RiskRating::Medium);
}

#[test]
fn test_risk_score_10_is_high_lower_boundary() {
    // Unlikely(2) × Catastrophic(5) = 10 → High
    let score = RiskScore::new(Likelihood::Unlikely, Impact::Catastrophic);
    assert_eq!(score.raw(), 10);
    assert_eq!(score.rating(), RiskRating::High);
}

#[test]
fn test_risk_score_16_is_high_upper_boundary() {
    // Likely(4) × Major(4) = 16 → High
    let score = RiskScore::new(Likelihood::Likely, Impact::Major);
    assert_eq!(score.raw(), 16);
    assert_eq!(score.rating(), RiskRating::High);
}

#[test]
fn test_risk_score_20_is_critical() {
    // Likely(4) × Catastrophic(5) = 20 → Critical
    let score = RiskScore::new(Likelihood::Likely, Impact::Catastrophic);
    assert_eq!(score.raw(), 20);
    assert_eq!(score.rating(), RiskRating::Critical);
}

#[test]
fn test_risk_score_maximum_is_critical() {
    // AlmostCertain(5) × Catastrophic(5) = 25 → Critical
    let score = RiskScore::new(Likelihood::AlmostCertain, Impact::Catastrophic);
    assert_eq!(score.raw(), 25);
    assert_eq!(score.rating(), RiskRating::Critical);
}

// ── RiskAssessment ─────────────────────────────────────────────────────────────

#[test]
fn test_risk_assessment_low_residual_is_acceptable() {
    // Rare(1) × Negligible(1) = 1 → Low → acceptable
    let residual = RiskScore::new(Likelihood::Rare, Impact::Negligible);
    assert!(make_risk_assessment(residual).is_acceptable());
}

#[test]
fn test_risk_assessment_medium_residual_is_acceptable() {
    // Possible(3) × Moderate(3) = 9 → Medium → acceptable
    let residual = RiskScore::new(Likelihood::Possible, Impact::Moderate);
    assert!(make_risk_assessment(residual).is_acceptable());
}

#[test]
fn test_risk_assessment_high_residual_is_not_acceptable() {
    // Unlikely(2) × Catastrophic(5) = 10 → High → requires escalation
    let residual = RiskScore::new(Likelihood::Unlikely, Impact::Catastrophic);
    assert!(!make_risk_assessment(residual).is_acceptable());
}

#[test]
fn test_risk_assessment_critical_residual_is_not_acceptable() {
    // Likely(4) × Catastrophic(5) = 20 → Critical → requires escalation
    let residual = RiskScore::new(Likelihood::Likely, Impact::Catastrophic);
    assert!(!make_risk_assessment(residual).is_acceptable());
}

// ── SystemCriticality ─────────────────────────────────────────────────────────

#[test]
fn test_system_criticality_rto_thresholds() {
    assert_eq!(SystemCriticality::Critical.max_rto_hours(), 4);
    assert_eq!(SystemCriticality::High.max_rto_hours(), 8);
    assert_eq!(SystemCriticality::Medium.max_rto_hours(), 24);
    assert_eq!(SystemCriticality::Low.max_rto_hours(), 72);
}

#[test]
fn test_system_criticality_rpo_thresholds() {
    assert_eq!(SystemCriticality::Critical.max_rpo_hours(), 4);
    assert_eq!(SystemCriticality::High.max_rpo_hours(), 8);
    assert_eq!(SystemCriticality::Medium.max_rpo_hours(), 24);
    assert_eq!(SystemCriticality::Low.max_rpo_hours(), 72);
}

// ── BcpTarget ─────────────────────────────────────────────────────────────────

#[test]
fn test_bcp_target_critical_exactly_at_limit_is_compliant() {
    let t = make_bcp_target("Payment Rails", SystemCriticality::Critical, 4, 4);
    assert!(t.is_mas_compliant());
    assert!(t.compliance_gap().is_none());
}

#[test]
fn test_bcp_target_critical_rto_exceeded() {
    let t = make_bcp_target("Payment Rails", SystemCriticality::Critical, 5, 4);
    assert!(!t.is_mas_compliant());
    let gap = t
        .compliance_gap()
        .expect("gap must be Some when non-compliant");
    assert!(gap.contains("RTO"), "gap must mention RTO");
    assert!(
        !gap.contains("RPO"),
        "gap must not mention RPO when RPO is compliant"
    );
}

#[test]
fn test_bcp_target_critical_rpo_exceeded() {
    let t = make_bcp_target("Payment Rails", SystemCriticality::Critical, 4, 5);
    assert!(!t.is_mas_compliant());
    let gap = t.compliance_gap().expect("gap must be Some");
    assert!(gap.contains("RPO"), "gap must mention RPO");
}

#[test]
fn test_bcp_target_critical_both_exceeded_gap_mentions_both() {
    let t = make_bcp_target("Payment Rails", SystemCriticality::Critical, 6, 6);
    assert!(!t.is_mas_compliant());
    let gap = t.compliance_gap().expect("gap must be Some");
    assert!(
        gap.contains("RTO") && gap.contains("RPO"),
        "gap must mention both RTO and RPO"
    );
}

#[test]
fn test_bcp_target_high_exactly_at_limit() {
    let t = make_bcp_target("Risk Engine", SystemCriticality::High, 8, 8);
    assert!(t.is_mas_compliant());
}

#[test]
fn test_bcp_target_high_rto_one_over_limit() {
    let t = make_bcp_target("Risk Engine", SystemCriticality::High, 9, 8);
    assert!(!t.is_mas_compliant());
}

#[test]
fn test_bcp_target_medium_exactly_at_limit() {
    let t = make_bcp_target("Reporting", SystemCriticality::Medium, 24, 24);
    assert!(t.is_mas_compliant());
}

#[test]
fn test_bcp_target_low_exactly_at_limit() {
    let t = make_bcp_target("Dev Tools", SystemCriticality::Low, 72, 72);
    assert!(t.is_mas_compliant());
}

#[test]
fn test_bcp_target_low_one_over_limit() {
    let t = make_bcp_target("Dev Tools", SystemCriticality::Low, 73, 72);
    assert!(!t.is_mas_compliant());
}

// ── BcpAssessment ─────────────────────────────────────────────────────────────

#[test]
fn test_bcp_assessment_all_compliant() {
    let targets = vec![
        make_bcp_target("Payment Rails", SystemCriticality::Critical, 4, 4),
        make_bcp_target("Risk Engine", SystemCriticality::High, 8, 8),
        make_bcp_target("Reporting", SystemCriticality::Medium, 24, 24),
    ];
    let assessment = make_bcp_assessment(targets);
    assert!(assessment.all_compliant());
    assert!(assessment.non_compliant().is_empty());
}

#[test]
fn test_bcp_assessment_one_non_compliant() {
    let targets = vec![
        make_bcp_target("Payment Rails", SystemCriticality::Critical, 4, 4),
        make_bcp_target("Risk Engine", SystemCriticality::High, 12, 8), // RTO 12h > 8h limit
        make_bcp_target("Reporting", SystemCriticality::Medium, 24, 24),
    ];
    let assessment = make_bcp_assessment(targets);
    assert!(!assessment.all_compliant());
    let nc = assessment.non_compliant();
    assert_eq!(nc.len(), 1);
    assert_eq!(nc[0].system_name, "Risk Engine");
}

#[test]
fn test_bcp_assessment_multiple_non_compliant() {
    let targets = vec![
        make_bcp_target("Payment Rails", SystemCriticality::Critical, 6, 6), // both exceeded
        make_bcp_target("Risk Engine", SystemCriticality::High, 12, 8),      // RTO exceeded
        make_bcp_target("Reporting", SystemCriticality::Medium, 24, 24),     // compliant
    ];
    let assessment = make_bcp_assessment(targets);
    assert!(!assessment.all_compliant());
    assert_eq!(assessment.non_compliant().len(), 2);
}

#[test]
fn test_bcp_assessment_is_test_overdue_when_past_due() {
    let assessment = BcpAssessment {
        targets: vec![],
        last_tested: Utc::now() - Duration::days(400),
        next_test_due: Utc::now() - Duration::days(35), // already past
    };
    assert!(assessment.is_test_overdue());
}

#[test]
fn test_bcp_assessment_is_not_overdue_when_future() {
    let assessment = make_bcp_assessment(vec![]);
    assert!(!assessment.is_test_overdue());
}

#[test]
fn test_bcp_assessment_empty_targets_is_trivially_compliant() {
    let assessment = make_bcp_assessment(vec![]);
    assert!(assessment.all_compliant());
    assert!(assessment.non_compliant().is_empty());
}
