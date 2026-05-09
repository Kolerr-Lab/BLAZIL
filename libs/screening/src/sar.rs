// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! FinCEN SAR (Suspicious Activity Report) generation.
//!
//! `SarReport::to_xml()` produces UTF-8 XML compatible with the FinCEN SAR
//! XML schema v2.0 used for BSA E-Filing submissions.
//!
//! Reference: FinCEN SAR XML User Guide
//! (https://www.fincen.gov/sites/default/files/shared/SARXMLUserGuide.pdf)
//!
//! # I/O ownership
//!
//! This module returns `Vec<u8>` — raw bytes. The caller is responsible for
//! all I/O decisions: writing to disk, uploading to BSA E-Filing, encrypting
//! in transit, or storing in an audit archive.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{ScreeningError, TransactionEvent};

/// Category of suspicious activity, aligned with FinCEN SAR filing codes.
#[derive(Debug, Clone, Serialize)]
pub enum SuspiciousActivityType {
    /// Potential money laundering activity (FinCEN code: A).
    MoneyLaundering,
    /// Transaction structuring to evade reporting thresholds (FinCEN code: B).
    Structuring,
    /// Fraud or material misrepresentation (FinCEN code: C).
    Fraud,
    /// Suspected terrorist financing (FinCEN code: D).
    TerroristFinancing,
    /// Other suspicious activity requiring narrative description (FinCEN code: Z).
    Other,
}

impl SuspiciousActivityType {
    fn fincen_code(&self) -> &'static str {
        match self {
            Self::MoneyLaundering => "A",
            Self::Structuring => "B",
            Self::Fraud => "C",
            Self::TerroristFinancing => "D",
            Self::Other => "Z",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::MoneyLaundering => "Money Laundering",
            Self::Structuring => "Structuring",
            Self::Fraud => "Fraud",
            Self::TerroristFinancing => "Terrorist Financing",
            Self::Other => "Other Suspicious Activity",
        }
    }
}

/// A FinCEN SAR-compatible suspicious activity report.
///
/// Fields map to the FinCEN SAR XML schema v2.0 elements. Construct via
/// `SarReport::from_transaction`, then call `to_xml()` to obtain the
/// submission-ready payload.
#[derive(Debug, Clone, Serialize)]
pub struct SarReport {
    /// Name of the institution filing the SAR (BSA filer).
    pub filing_institution: String,

    /// Identifier of the subject of the suspicious activity.
    pub subject_id: String,

    /// Total amount involved in the suspicious activity, in minor units
    /// (e.g. cents for USD). Stored as minor units to avoid floating-point
    /// precision issues; convert to major units for human-readable output.
    pub amount: u64,

    /// ISO 4217 currency code.
    pub currency: String,

    /// Classification of the suspicious activity.
    pub suspicious_activity_type: SuspiciousActivityType,

    /// Free-text narrative describing the suspicious activity.
    /// FinCEN requires a minimum of 1 sentence; maximum is 20,000 characters.
    pub narrative: String,

    /// UTC timestamp of the suspicious transaction.
    pub activity_date: DateTime<Utc>,

    /// UTC timestamp when this SAR was generated.
    pub filing_date: DateTime<Utc>,

    /// Original Blazil transaction identifier, used for internal cross-reference.
    pub transaction_id: String,
}

impl SarReport {
    /// Constructs a SAR report from a `TransactionEvent`.
    ///
    /// `filing_institution` should be the legal name of the BSA-registered
    /// institution (e.g. `"Blazil Financial Inc."`).
    pub fn from_transaction(
        tx: &TransactionEvent,
        filing_institution: impl Into<String>,
        suspicious_activity_type: SuspiciousActivityType,
        narrative: impl Into<String>,
    ) -> Self {
        Self {
            filing_institution: filing_institution.into(),
            subject_id: tx.sender_id.clone(),
            amount: tx.amount,
            currency: tx.currency.clone(),
            suspicious_activity_type,
            narrative: narrative.into(),
            activity_date: tx.timestamp,
            filing_date: Utc::now(),
            transaction_id: tx.transaction_id.clone(),
        }
    }

    /// Serializes the SAR to FinCEN SAR-compatible UTF-8 XML.
    ///
    /// The narrative is wrapped in a CDATA section so that it can contain
    /// arbitrary text (including angle brackets and ampersands) without
    /// escaping. The `]]>` sequence — which would prematurely close CDATA —
    /// is split via the standard CDATA escape technique.
    ///
    /// All other string fields are XML-escaped before insertion.
    ///
    /// # Returns
    ///
    /// `Ok(Vec<u8>)` — raw UTF-8 bytes ready for transmission or storage.
    ///
    /// # Errors
    ///
    /// Currently infallible; returns `Err` only if future schema validation
    /// is added.
    pub fn to_xml(&self) -> Result<Vec<u8>, ScreeningError> {
        let filing_institution = escape_xml(&self.filing_institution);
        let filing_date = self.filing_date.format("%Y-%m-%dT%H:%M:%SZ");
        let activity_date = self.activity_date.format("%Y-%m-%dT%H:%M:%SZ");
        let transaction_id = escape_xml(&self.transaction_id);
        let subject_id = escape_xml(&self.subject_id);
        let currency = escape_xml(&self.currency);
        let amount = self.amount;
        let code = self.suspicious_activity_type.fincen_code();
        let description = self.suspicious_activity_type.description();
        // CDATA escape: ]]> → ]]]]><![CDATA[>
        let safe_narrative = self.narrative.replace("]]>", "]]]]><![CDATA[>");

        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<SuspiciousActivityReport xmlns="urn:fincen:sar:2.0">
  <FilingInstitution>{filing_institution}</FilingInstitution>
  <FilingDate>{filing_date}</FilingDate>
  <ActivityDate>{activity_date}</ActivityDate>
  <TransactionIdentifier>{transaction_id}</TransactionIdentifier>
  <Subject>
    <Identifier>{subject_id}</Identifier>
  </Subject>
  <Amount currency="{currency}">{amount}</Amount>
  <SuspiciousActivity code="{code}">{description}</SuspiciousActivity>
  <Narrative><![CDATA[{safe_narrative}]]></Narrative>
</SuspiciousActivityReport>"#
        );

        Ok(xml.into_bytes())
    }
}

/// Escapes XML special characters for use in element text and attribute values.
///
/// Covers the five predefined XML entities: `&`, `<`, `>`, `"`, `'`.
fn escape_xml(s: &str) -> String {
    // Process in a single pass to avoid repeated allocations.
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}
