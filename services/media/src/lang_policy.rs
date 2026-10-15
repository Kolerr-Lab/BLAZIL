//! Deciding what language to ANSWER in.
//!
//! The speech-to-text provider tells us what it heard, one utterance at a time. That is evidence,
//! not an instruction, and the difference is the whole reason this module exists.
//!
//! **Why not just follow the last transcript?** Vietnamese callers borrow English constantly —
//! "cho em *book* lịch", "*confirm* giùm em". A single English-looking fragment must not flip a
//! conversation into English, because the caller did not switch languages; they spoke Vietnamese.
//!
//! But guarding every utterance that way was too blunt: it also meant a caller who opened with a
//! whole Vietnamese sentence was answered in English, every time, because turn 1 can never have a
//! streak behind it. So the guard now scales with how much the detector had to work with — a full
//! sentence switches immediately, a fragment still has to repeat itself (`SWITCH_STREAK`). See
//! `STRONG_MIN_WORDS`.
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
//!
//! That auto-detect is a precondition, not a detail. For agents that follow the caller, `session.rs`
//! now sends the recogniser no language at all — before that, it was pinned to the agent's
//! configured language, which does not merely bias transcription but rewrites Vietnamese audio as
//! English. Everything in this module was dead code in production until that was fixed: the
//! detected language simply never changed.

/// Consecutive SHORT utterances in a new language before the answer language follows. One short
/// utterance is probably a loan word ("OK", "dạ", "thank you"); two in a row is a real switch.
const SWITCH_STREAK: u8 = 2;

/// A utterance this long is treated as strong evidence and switches the language on its own.
///
/// The streak existed to survive loan words, but applying it to every utterance meant the FIRST
/// turn was always answered in the wrong language — a caller who opens with a full Vietnamese
/// sentence got an English reply, which is the single most visible way this feature can fail.
///
/// Length is the discriminator because it correlates with how much signal the detector had: "dạ"
/// gives it almost nothing and is easy to mislabel, while a five-word sentence is hard to get wrong
/// even when it contains a borrowed English word. A confidence score from the provider would be a
/// better signal, but `Transcript` does not carry one today — adding it means touching both STT
/// backends, so this stays length-based until there is a reason to pay that cost.
const STRONG_MIN_WORDS: usize = 3;

/// Character fallback for languages that do not put spaces between words.
///
/// Japanese, Chinese, Thai and Khmer write "予約をお願いしたいのですが" as a single whitespace-
/// delimited token, so word counting rates a perfectly clear sentence as a fragment and the agent
/// would never switch into those languages at all. Counting characters catches them. The threshold
/// is deliberately higher than `STRONG_MIN_WORDS` so a short Latin-script fragment ("OK", "dạ")
/// cannot sneak past it: "thank you" is 9 characters and still correctly weak.
const STRONG_MIN_CHARS: usize = 12;

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

    /// Feed one utterance's detected language and its text. Returns what to answer in.
    ///
    /// `detected` is `None` when the provider said nothing — which is common and not an error. An
    /// unknown language is treated as no evidence rather than as evidence of the default, because
    /// those are different things: the first should leave a streak undisturbed, the second would
    /// silently reset it and make switching nearly impossible on a noisy line.
    ///
    /// `text` is used only for its length, to tell a throwaway "OK" from a real sentence. See
    /// `STRONG_MIN_WORDS`.
    pub fn observe(&mut self, detected: Option<&str>, text: &str) -> Decision {
        let unchanged = |p: &Self| Decision {
            answer_language: p.current.clone(),
            changed: false,
        };

        let strong = is_strong_evidence(text);

        let Some(lang) = detected.map(normalize).filter(|l| !l.is_empty()) else {
            return unchanged(self);
        };

        // Agents that do not follow the caller still observe, so the dashboard can show how often
        // callers speak something other than the configured language. This is what turns "should we
        // build this?" into a measured question.
        if !self.follow_caller {
            return unchanged(self);
        }

        if self.current.as_deref() == Some(lang.as_str()) {
            // Already answering in this language. Only a CLEAR utterance cancels a pending switch.
            //
            // This asymmetry is the other half of the turn-1 fix. A caller mid-switch to English
            // says "dạ" — two letters, trivially mislabelled as the language we are already in —
            // and under the old rule that single token wiped a streak the caller had genuinely
            // built. Switching then needed an unbroken run that a real conversation rarely
            // produces. A short utterance now leaves the streak standing.
            if strong {
                self.streak_language = None;
                self.streak = 0;
            }
            return unchanged(self);
        }

        match &self.streak_language {
            Some(s) if *s == lang => self.streak += 1,
            _ => {
                self.streak_language = Some(lang.clone());
                self.streak = 1;
            }
        }

        // A full sentence stands on its own; a fragment has to repeat itself.
        let needed = if strong { 1 } else { SWITCH_STREAK };
        if self.streak >= needed {
            self.current = Some(lang);
            self.streak_language = None;
            self.streak = 0;
            return Decision {
                answer_language: self.current.clone(),
                changed: true,
            };
        }

        unchanged(self)
    }
}

/// Is this utterance long enough to trust its language label on its own?
///
/// Either enough words (Latin-script, Vietnamese, Cyrillic…) or enough characters (Japanese,
/// Chinese, Thai — no spaces between words, so word counting always returns 1).
fn is_strong_evidence(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.split_whitespace().count() >= STRONG_MIN_WORDS
        || trimmed.chars().count() >= STRONG_MIN_CHARS
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

    /// A full sentence — strong evidence (>= STRONG_MIN_WORDS).
    const SENTENCE: &str = "I would like to book an appointment please";
    /// A fragment — weak evidence.
    const FRAGMENT: &str = "OK";

    #[test]
    fn greeting_always_uses_the_configured_default() {
        // The greeting is spoken before the caller says anything, so detection is impossible.
        let p = follow("vi");
        assert_eq!(p.greeting_language().as_deref(), Some("vi"));
    }

    #[test]
    fn a_full_sentence_switches_on_the_very_first_turn() {
        // The headline fix. A caller who opens in another language is answered in it immediately,
        // instead of getting one guaranteed-wrong reply first.
        let mut p = follow("vi");
        let d = p.observe(Some("en"), SENTENCE);
        assert_eq!(d.answer_language.as_deref(), Some("en"));
        assert!(d.changed);
    }

    #[test]
    fn a_single_foreign_fragment_does_not_switch() {
        // "cho em book lịch nha" heard as a two-word English fragment — a loan word, not a switch.
        let mut p = follow("vi");
        let d = p.observe(Some("en"), FRAGMENT);
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(!d.changed);
    }

    #[test]
    fn two_fragments_in_a_row_switch() {
        let mut p = follow("vi");
        p.observe(Some("en"), FRAGMENT);
        let d = p.observe(Some("en"), "thank you");
        assert_eq!(d.answer_language.as_deref(), Some("en"));
        assert!(d.changed);
    }

    #[test]
    fn a_fragment_in_the_current_language_does_not_wipe_a_streak() {
        // The other half of the fix. Mid-switch the caller says "dạ", which is trivially
        // mislabelled as the language we are already answering in. That must not undo the streak,
        // or switching needs an unbroken run that real conversation does not produce.
        let mut p = follow("vi");
        p.observe(Some("en"), FRAGMENT); // streak: en = 1
        p.observe(Some("vi"), "dạ"); // fragment in current language — ignored
        let d = p.observe(Some("en"), "right"); // streak: en = 2 → switch
        assert_eq!(d.answer_language.as_deref(), Some("en"));
        assert!(d.changed);
    }

    #[test]
    fn a_full_sentence_in_the_current_language_does_wipe_a_streak() {
        // Symmetric to the above: a clear utterance in the language we are already in IS evidence
        // the caller never left it, so a pending switch should be abandoned.
        let mut p = follow("vi");
        p.observe(Some("en"), FRAGMENT); // streak: en = 1
        p.observe(Some("vi"), "em muốn đặt lịch hẹn vào ngày mai"); // clear → cancels
        let d = p.observe(Some("en"), FRAGMENT); // back to streak = 1, not 2
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(!d.changed);
    }

    #[test]
    fn unknown_language_is_no_evidence_and_preserves_a_streak() {
        // A silent/unclassifiable utterance must not undo progress toward a switch, or a noisy
        // line would make switching impossible.
        let mut p = follow("vi");
        p.observe(Some("en"), FRAGMENT);
        p.observe(None, SENTENCE);
        let d = p.observe(Some("en"), FRAGMENT);
        assert!(d.changed);
        assert_eq!(d.answer_language.as_deref(), Some("en"));
    }

    #[test]
    fn regions_are_ignored() {
        let mut p = follow("vi");
        p.observe(Some("en-US"), FRAGMENT);
        let d = p.observe(Some("en-GB"), FRAGMENT);
        assert!(d.changed);
        assert_eq!(d.answer_language.as_deref(), Some("en"));
    }

    #[test]
    fn switching_back_works_the_same_way() {
        let mut p = follow("vi");
        p.observe(Some("en"), SENTENCE); // now answering in English
        let d = p.observe(Some("vi"), "em muốn đổi lịch hẹn giúp em");
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(d.changed);
    }

    #[test]
    fn follow_disabled_never_changes_the_answer_language() {
        // The measurement mode: observations are accepted, behaviour is untouched.
        let mut p = LanguagePolicy::new(Some("vi".into()), false);
        p.observe(Some("en"), SENTENCE);
        let d = p.observe(Some("en"), SENTENCE);
        assert_eq!(d.answer_language.as_deref(), Some("vi"));
        assert!(!d.changed);
    }

    #[test]
    fn no_default_means_no_opinion_until_the_caller_shows_one() {
        let mut p = LanguagePolicy::new(None, true);
        assert!(p.greeting_language().is_none());
        let d = p.observe(Some("ja"), "予約をお願いしたいのですが今週の金曜日に");
        assert_eq!(d.answer_language.as_deref(), Some("ja"));
        assert!(d.changed);
    }

    #[test]
    fn spaceless_scripts_count_characters_instead_of_words() {
        // Japanese writes a whole sentence as one whitespace-delimited token. Without the character
        // fallback this rates as a fragment and the agent can never switch into Japanese at all.
        assert!(is_strong_evidence(
            "予約をお願いしたいのですが今週の金曜日に"
        ));
        assert!(is_strong_evidence("我想预约这个星期五的时间"));
    }

    #[test]
    fn short_latin_fragments_stay_weak_under_the_character_rule() {
        // Guards the threshold: if STRONG_MIN_CHARS were lowered to ~8 these would wrongly become
        // strong, and a single borrowed "thank you" would flip a Vietnamese call to English.
        assert!(!is_strong_evidence("OK"));
        assert!(!is_strong_evidence("dạ"));
        assert!(!is_strong_evidence("thank you"));
        assert!(!is_strong_evidence("   "));
    }

    #[test]
    fn repeated_detections_of_the_current_language_never_report_a_change() {
        // Guards against a switch being re-announced every turn, which would make the session
        // re-pick the voice over and over.
        let mut p = follow("vi");
        p.observe(Some("en"), SENTENCE);
        for _ in 0..3 {
            let d = p.observe(Some("en"), SENTENCE);
            assert_eq!(d.answer_language.as_deref(), Some("en"));
            assert!(!d.changed);
        }
    }
}
