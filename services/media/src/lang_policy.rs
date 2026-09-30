//! Deciding what language to ANSWER in.
//!
//! The speech-to-text provider tells us what it heard, one utterance at a time. That is evidence,
//! not an instruction, and the difference is the whole reason this module exists.
//!
//! **Why not just follow the last transcript?** Vietnamese callers borrow English constantly —
//! "cho em *book* lịch", "*confirm* giùm em". A single English-looking utterance must not flip a
//! conversation into English, because the caller did not switch languages; they spoke Vietnamese.
//! So a switch requires `SWITCH_STREAK` consecutive utterances agreeing on a different language.
//!
//! **Why lock at all, instead of deciding every turn?** Stability. A caller who hears the agent
//! answer in Vietnamese, then English, then Vietnamese has a worse experience than one who hears a
//! consistent language, even if a particular turn is "wrong". Deciding once and holding is also
//! what makes the behaviour explainable to the business owner looking at a transcript.
//!
//! **What this module deliberately does NOT do:** change the STT language mid-call. Reconnecting
//! the recogniser to constrain it costs a socket round-trip in the middle of a conversation, and a
//! failed reconnect is dead air — the exact failure mode that got the speaker gate removed. The
//! recogniser stays in auto-detect for the whole call; only the reply language and the voice change.

/// Consecutive utterances in a new language before the answer language follows. One is a loan word;
/// two in a row is a caller who actually switched.
const SWITCH_STREAK: u8 = 2;

/// What the session should answer in, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// ISO-639-1 code to answer in. `None` = no opinion; leave the reply language unconstrained.
    pub answer_language: Option<String>,
    /// True on the turn the decision changed — the session logs it and re-picks the voice.
    pub changed: bool,
}

/// Tracks the caller's language across one call.
#[derive(Debug, Clone)]
pub struct LanguagePolicy {
    /// The agent's configured default. Used for the greeting (which happens before the caller has
    /// said anything, so it can never be detected) and whenever we have no better information.
    default_language: Option<String>,
    /// Whether this agent is allowed to follow the caller at all. When false this type still
    /// records observations — which is what makes the measurement phase possible with no behaviour
    /// change — but always answers in `default_language`.
    follow_caller: bool,
    /// The language currently being answered in.
    current: Option<String>,
    /// Candidate language seen in a row, and how many times.
    streak_language: Option<String>,
    streak: u8,
}

impl LanguagePolicy {
    pub fn new(default_language: Option<String>, follow_caller: bool) -> Self {
        let current = default_language.clone();
        Self {
            default_language,
            follow_caller,
            current,
            streak_language: None,
            streak: 0,
        }
    }

    /// The language to answer in right now, before any caller audio (i.e. the greeting).
    pub fn greeting_language(&self) -> Option<String> {
        self.default_language.clone()
    }

    /// Currently selected answer language.
    pub fn current(&self) -> Option<String> {
        self.current.clone()
    }

    /// Feed one utterance's detected language. Returns what to answer in.
    ///
    /// `detected` is `None` when the provider said nothing — which is common and not an error. An
    /// unknown language is treated as no evidence rather than as evidence of the default, because
    /// those are different things: the first should leave a streak undisturbed, the second would
    /// silently reset it and make switching nearly impossible on a noisy line.
    pub fn observe(&mut self, detected: Option<&str>) -> Decision {
        let Some(lang) = detected.map(normalize).filter(|l| !l.is_empty()) else {
            return Decision {
                answer_language: self.current.clone(),
                changed: false,
            };
        };

        // Agents that do not follow the caller still observe, so the dashboard can show how often
        // callers speak something other than the configured language. This is what turns "should we
        // build this?" into a measured question.
        if !self.follow_caller {
            return Decision {
                answer_language: self.current.clone(),
                changed: false,
            };
        }

        if self.current.as_deref() == Some(lang.as_str()) {
            // Already answering in this language — any competing streak is stale.
            self.streak_language = None;
            self.streak = 0;
            return Decision {
                answer_language: self.current.clone(),
                changed: false,
            };
        }

        match &self.streak_language {
            Some(s) if *s == lang => self.streak += 1,
            _ => {
                self.streak_language = Some(lang.clone());
                self.streak = 1;
            }
        }

        if self.streak >= SWITCH_STREAK {
            self.current = Some(lang);
            self.streak_language = None;
            self.streak = 0;
            return Decision {
                answer_language: self.current.clone(),
                changed: true,
            };
        }

        Decision {
            answer_language: self.current.clone(),
            changed: false,
        }
    }
}

/// `en-US` → `en`, trimmed and lowercased. We answer in a language, not a locale.
fn normalize(code: &str) -> String {
    code.trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn follow(default: &str) -> LanguagePolicy {
        LanguagePolicy::new(Some(default.into()), true)
    }

    #[test]
    fn greeting_always_uses_the_configured_default() {
        // The greeting is spoken before the caller says anything, so detection is impossible.
        let p = follow("vi");
        assert_eq!(p.greeting_language().as_deref(), Some("vi"));
    }

    #[test]
    fn a_single_foreign_utterance_does_not_switch() {
        // "cho em book lịch nha" — one English-looking utterance in a Vietnamese call.
        let mut p = follow("vi");
        let d = p.observe(Some("en"));
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(!d.changed);
    }

    #[test]
    fn two_in_a_row_switches() {
        let mut p = follow("vi");
        p.observe(Some("en"));
        let d = p.observe(Some("en"));
        assert_eq!(d.answer_language.as_deref(), Some("en"));
        assert!(d.changed);
    }

    #[test]
    fn an_interruption_resets_the_streak() {
        // en, vi, en is a code-switching caller, not someone who moved to English.
        let mut p = follow("vi");
        p.observe(Some("en"));
        p.observe(Some("vi"));
        let d = p.observe(Some("en"));
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(!d.changed);
    }

    #[test]
    fn unknown_language_is_no_evidence_and_preserves_a_streak() {
        // A silent/unclassifiable utterance must not undo progress toward a switch, or a noisy
        // line would make switching impossible.
        let mut p = follow("vi");
        p.observe(Some("en"));
        p.observe(None);
        let d = p.observe(Some("en"));
        assert!(d.changed);
        assert_eq!(d.answer_language.as_deref(), Some("en"));
    }

    #[test]
    fn regions_are_ignored() {
        let mut p = follow("vi");
        p.observe(Some("en-US"));
        let d = p.observe(Some("en-GB"));
        assert!(d.changed);
        assert_eq!(d.answer_language.as_deref(), Some("en"));
    }

    #[test]
    fn switching_back_needs_the_same_streak() {
        let mut p = follow("vi");
        p.observe(Some("en"));
        p.observe(Some("en")); // now answering in English
        p.observe(Some("vi"));
        let d = p.observe(Some("vi"));
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(d.changed);
    }

    #[test]
    fn follow_disabled_never_changes_the_answer_language() {
        // The measurement mode: observations are accepted, behaviour is untouched.
        let mut p = LanguagePolicy::new(Some("vi".into()), false);
        p.observe(Some("en"));
        let d = p.observe(Some("en"));
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(!d.changed);
    }

    #[test]
    fn no_default_means_no_opinion_until_the_caller_shows_one() {
        let mut p = LanguagePolicy::new(None, true);
        assert!(p.greeting_language().is_none());
        p.observe(Some("ja"));
        let d = p.observe(Some("ja"));
        assert_eq!(d.answer_language.as_deref(), Some("ja"));
        assert!(d.changed);
    }
}
