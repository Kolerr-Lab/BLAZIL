//! Voice Activity Detection engine (barge-in + utterance-end detection).
//!
//! Design: single struct, zero external dependencies, runs on the 20 ms \u03bc-law \u2192 PCM16
//! frames that the Twilio media stream delivers.
//!
//! # Detection layers (lowest to highest latency)
//!
//! 1. **RMS gate** \u2014 energy below the adaptive noise floor is silence immediately (< 0.01 ms).
//! 2. **Autocorrelation (Lag 1)** \u2014 fast time-domain proxy for spectral shape.
//!    Voiced speech has high correlation with its delayed self (low frequency dominance);
//!    noise has low or negative correlation. Computed in < 0.05 ms.
//! 3. **Zero-Crossing Rate (ZCR)** \u2014 sample sign-flip count in < 0.05 ms.  True voiced speech
//!    sits in a low-ZCR band (e.g. 2\u201360 crossings per 20 ms frame at 8 kHz). Unvoiced / noise
//!    goes high (often > 80).
//! 4. **Adaptive noise floor** \u2014 exponential moving average updated only during confirmed
//!    silence, so loud line noise never pulls the threshold up into false-silence territory.
//! 5. **Barge-in hold-off** \u2014 speech must be classified for `barge_in_ms` consecutive
//!    milliseconds before BargeIn fires.

const SPEECH_MARGIN: f64 = 2.5;

/// Barge-in RMS must exceed the noise floor by this factor.
const BARGE_MARGIN: f64 = 3.5;

/// Minimum Lag 1 Autocorrelation (r1/r0) for audio to be classified as voiced speech.
/// 1.0 = perfectly smooth (low frequency); 0.0 = uncorrelated noise; -1.0 = alternating noise.
/// Telephone-band voiced speech is typically > 0.8.
const AUTOCORR_VOICED_MIN: f64 = 0.70;

/// Zero-crossing rate bands for voiced speech at 8 kHz / 160 samples per frame.
/// Voiced speech (85-255 Hz) typically yields 3 to ~12 crossings per frame, up to maybe 40 for higher frequencies.
const ZCR_VOICED_MIN: u32 = 3;
const ZCR_VOICED_MAX: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Speech,
    Silence,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct FrameFeatures {
    pub rms: f64,
    pub autocorr: f64,
    pub zcr: u32,
    pub is_speech: bool,
    pub is_barge_candidate: bool,
}

#[allow(dead_code)]
pub struct VadEngine {
    consecutive_speech_ms: u64,
    consecutive_silence_ms: u64,
    consecutive_barge_ms: u64,
    energy_threshold: i32,
    noise_floor: f64,
}

#[allow(dead_code)]
impl VadEngine {
    pub fn new(aggressiveness: u8) -> Self {
        let energy_threshold = match aggressiveness {
            0 => 500,
            1 => 1000,
            2 => 2000,
            _ => 4000,
        };
        Self {
            consecutive_speech_ms: 0,
            consecutive_silence_ms: 0,
            consecutive_barge_ms: 0,
            energy_threshold,
            noise_floor: energy_threshold as f64,
        }
    }

    /// Computes Lag 1 Autocorrelation (r1 / r0).
    fn compute_autocorr(samples: &[i16]) -> f64 {
        if samples.len() < 2 {
            return 0.0;
        }
        let mut r0 = 0i64;
        let mut r1 = 0i64;

        // Compute r0 (energy) for the whole frame
        for &s in samples {
            let v = s as i64;
            r0 += v * v;
        }

        if r0 == 0 {
            return 0.0;
        }

        // Compute r1 (lag 1 correlation)
        for i in 1..samples.len() {
            let a = samples[i] as i64;
            let b = samples[i - 1] as i64;
            r1 += a * b;
        }

        (r1 as f64) / (r0 as f64)
    }

    fn compute_zcr(samples: &[i16]) -> u32 {
        if samples.len() < 2 {
            return 0;
        }
        let mut crossings: u32 = 0;
        for window in samples.windows(2) {
            let a = window[0] >= 0;
            let b = window[1] >= 0;
            if a != b {
                crossings += 1;
            }
        }
        crossings
    }

    pub fn process_frame_detailed(&mut self, pcm_frame: &[i16]) -> (VadState, FrameFeatures) {
        let mut energy: i64 = 0;
        for &s in pcm_frame {
            let v = s as i64;
            energy += v * v;
        }
        let rms = if pcm_frame.is_empty() {
            0.0
        } else {
            (energy as f64 / pcm_frame.len() as f64).sqrt()
        };

        let speech_threshold = (self.noise_floor * SPEECH_MARGIN).max(self.energy_threshold as f64);
        let barge_threshold =
            (self.noise_floor * BARGE_MARGIN).max((self.energy_threshold * 2) as f64);

        let rms_passes_speech = rms > speech_threshold;
        let rms_passes_barge = rms > barge_threshold;

        let autocorr = if rms_passes_speech {
            Self::compute_autocorr(pcm_frame)
        } else {
            0.0
        };
        let autocorr_is_speech = autocorr >= AUTOCORR_VOICED_MIN;

        let zcr = if rms_passes_speech {
            Self::compute_zcr(pcm_frame)
        } else {
            0
        };
        let zcr_is_voiced = (ZCR_VOICED_MIN..=ZCR_VOICED_MAX).contains(&zcr);

        let is_speech = rms_passes_speech && autocorr_is_speech && zcr_is_voiced;
        let is_barge_candidate = rms_passes_barge && autocorr_is_speech && zcr_is_voiced;

        let features = FrameFeatures {
            rms,
            autocorr,
            zcr,
            is_speech,
            is_barge_candidate,
        };

        if is_barge_candidate {
            self.consecutive_barge_ms += 20;
        } else {
            self.consecutive_barge_ms = 0;
        }

        if is_speech {
            self.consecutive_speech_ms += 20;
            self.consecutive_silence_ms = 0;
            (VadState::Speech, features)
        } else {
            self.consecutive_silence_ms += 20;
            self.consecutive_speech_ms = 0;
            // Adapt noise floor only during silence
            self.noise_floor = 0.95 * self.noise_floor + 0.05 * rms;
            (VadState::Silence, features)
        }
    }

    pub fn process_frame(&mut self, pcm_frame: &[i16]) -> VadState {
        self.process_frame_detailed(pcm_frame).0
    }

    pub fn consecutive_speech_ms(&self) -> u64 {
        self.consecutive_speech_ms
    }
    pub fn consecutive_silence_ms(&self) -> u64 {
        self.consecutive_silence_ms
    }
    pub fn is_utterance_end(&self, threshold_ms: u64) -> bool {
        self.consecutive_silence_ms >= threshold_ms
    }
    pub fn is_barge_in(&self, threshold_ms: u64, assistant_speaking: bool) -> bool {
        assistant_speaking && self.consecutive_barge_ms >= threshold_ms
    }
    pub fn reset_counters(&mut self) {
        self.consecutive_speech_ms = 0;
        self.consecutive_silence_ms = 0;
        self.consecutive_barge_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn make_engine() -> VadEngine {
        VadEngine::new(2)
    }
    fn silence_frame() -> Vec<i16> {
        vec![0i16; 160]
    }

    fn ambient_noise_frame(amplitude: i16) -> Vec<i16> {
        (0..160)
            .map(|i| {
                let t = i as f64 / 160.0;
                let v = (2.0 * PI * 70.0 * t).sin()
                    + (2.0 * PI * 450.0 * t).sin()
                    + (2.0 * PI * 1800.0 * t).sin();
                ((v / 3.0) * amplitude as f64) as i16
            })
            .collect()
    }

    fn voiced_speech_frame(amplitude: i16) -> Vec<i16> {
        (0..160)
            .map(|i| {
                let t = i as f64 / 8000.0;
                let f0 = 150.0_f64;
                let v = 1.0 * (2.0 * PI * f0 * t).sin()
                    + 0.6 * (2.0 * PI * 2.0 * f0 * t).sin()
                    + 0.3 * (2.0 * PI * 3.0 * f0 * t).sin()
                    + 0.15 * (2.0 * PI * 4.0 * f0 * t).sin();
                ((v / 2.05) * amplitude as f64) as i16
            })
            .collect()
    }

    fn click_frame() -> Vec<i16> {
        let mut frame = vec![0i16; 160];
        frame[10] = 20000;
        frame[11] = -20000;
        frame
    }

    #[test]
    fn silence_classifies_as_silence() {
        let mut e = make_engine();
        assert_eq!(e.process_frame(&silence_frame()), VadState::Silence);
        assert_eq!(e.consecutive_silence_ms(), 20);
        assert_eq!(e.consecutive_speech_ms(), 0);
    }

    #[test]
    fn autocorr_broadband_noise_is_low() {
        let noisy: Vec<i16> = (0..160)
            .map(|i| if i % 2 == 0 { 8000 } else { -8000 })
            .collect();
        let ac = VadEngine::compute_autocorr(&noisy);
        assert!(ac < 0.0, "broadband noise autocorr {ac:.4} should be < 0.0");
    }

    #[test]
    fn autocorr_pure_sine_is_high() {
        let sine: Vec<i16> = (0..160)
            .map(|i| ((2.0 * PI * 500.0 * i as f64 / 8000.0).sin() * 10000.0) as i16)
            .collect();
        let ac = VadEngine::compute_autocorr(&sine);
        assert!(
            ac > 0.8,
            "pure 500 Hz tone autocorr {ac:.4} should be > 0.8"
        );
    }

    #[test]
    fn autocorr_voiced_speech_above_threshold() {
        let ac = VadEngine::compute_autocorr(&voiced_speech_frame(8000));
        assert!(
            ac >= AUTOCORR_VOICED_MIN,
            "voiced speech autocorr {ac:.4} should be >= {AUTOCORR_VOICED_MIN}"
        );
    }

    #[test]
    fn zcr_voiced_speech_in_band() {
        let zcr = VadEngine::compute_zcr(&voiced_speech_frame(8000));
        assert!(
            (ZCR_VOICED_MIN..=ZCR_VOICED_MAX).contains(&zcr),
            "voiced speech ZCR {zcr} should be in [{ZCR_VOICED_MIN}, {ZCR_VOICED_MAX}]"
        );
    }

    #[test]
    fn zcr_click_below_voiced_min() {
        let zcr = VadEngine::compute_zcr(&click_frame());
        assert!(
            zcr < ZCR_VOICED_MIN,
            "click ZCR {zcr} should be < {ZCR_VOICED_MIN}"
        );
    }

    #[test]
    fn ambient_noise_does_not_trigger_barge_in() {
        let mut e = make_engine();
        let noise = ambient_noise_frame(1500);
        for _ in 0..15 {
            e.process_frame(&noise);
        }
        assert!(
            !e.is_barge_in(120, true),
            "ambient noise must not trigger barge-in"
        );
    }

    #[test]
    fn click_does_not_trigger_barge_in() {
        let mut e = make_engine();
        for _ in 0..5 {
            e.process_frame(&silence_frame());
        }
        e.process_frame(&click_frame());
        assert!(
            !e.is_barge_in(20, true),
            "a single click must not trigger barge-in"
        );
    }

    #[test]
    fn voiced_speech_triggers_barge_in() {
        let mut e = make_engine();
        for _ in 0..5 {
            e.process_frame(&silence_frame());
        }
        let speech = voiced_speech_frame(25000); // ensure RMS beats 3.5 * noise floor
        for _ in 0..7 {
            e.process_frame(&speech);
        }
        assert!(
            e.is_barge_in(120, true),
            "140 ms of loud voiced speech must trigger barge-in"
        );
    }

    #[test]
    fn barge_in_gated_by_assistant_speaking() {
        let mut e = make_engine();
        let speech = voiced_speech_frame(25000);
        for _ in 0..10 {
            e.process_frame(&speech);
        }
        assert!(
            !e.is_barge_in(120, false),
            "barge-in must be gated by assistant_speaking"
        );
    }

    #[test]
    fn reset_clears_all_counters() {
        let mut e = make_engine();
        for _ in 0..10 {
            e.process_frame(&voiced_speech_frame(25000));
        }
        e.reset_counters();
        assert_eq!(e.consecutive_barge_ms, 0);
        assert_eq!(e.consecutive_speech_ms(), 0);
        assert_eq!(e.consecutive_silence_ms(), 0);
        assert!(!e.is_barge_in(20, true));
    }

    #[test]
    fn silence_after_speech_resets_speech_counter() {
        let mut e = make_engine();
        for _ in 0..5 {
            e.process_frame(&voiced_speech_frame(25000));
        }
        assert!(e.consecutive_speech_ms() > 0);
        e.process_frame(&silence_frame());
        assert_eq!(e.consecutive_speech_ms(), 0);
    }

    #[test]
    fn original_api_unchanged() {
        let mut engine = VadEngine::new(2);
        assert_eq!(engine.process_frame(&silence_frame()), VadState::Silence);
        assert_eq!(engine.consecutive_silence_ms(), 20);
        assert_eq!(engine.consecutive_speech_ms(), 0);
        assert!(!engine.is_barge_in(20, false));

        let state = engine.process_frame(&voiced_speech_frame(25000));
        assert_eq!(state, VadState::Speech);
        assert_eq!(engine.consecutive_speech_ms(), 20);
        assert_eq!(engine.consecutive_silence_ms(), 0);
    }
}
