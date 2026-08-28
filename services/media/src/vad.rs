#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Speech,
    Silence,
}

// Speech must exceed the adaptive noise floor by this factor to count as voice; barge-in needs a
// higher margin so only speech that's clearly above the line noise interrupts the agent (fewer
// false cuts from breaths/backchannel), which lets us use a shorter barge-in window safely.
const SPEECH_MARGIN: f64 = 2.5;
const BARGE_MARGIN: f64 = 3.5;

#[allow(dead_code)]
pub struct VadEngine {
    consecutive_speech_ms: u64,
    consecutive_silence_ms: u64,
    // Loud-speech run specifically for barge-in (rms above the barge margin), tracked separately
    // from ordinary speech so a soft continuation doesn't trip an interrupt.
    consecutive_barge_ms: u64,
    energy_threshold: i32,
    // Running estimate of background RMS, updated only during silence so it tracks the line noise
    // floor and adapts to quiet vs. noisy connections instead of a single fixed threshold.
    noise_floor: f64,
}

#[allow(dead_code)]
impl VadEngine {
    pub fn new(aggressiveness: u8) -> Self {
        // aggressiveness: 0 (least) to 3 (most aggressive)
        // Energy threshold: absolute floor; the adaptive noise floor takes over above it.
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

    /// Process a 20ms PCM16 frame.
    pub fn process_frame(&mut self, pcm_frame: &[i16]) -> VadState {
        let mut energy: i64 = 0;

        for &sample in pcm_frame {
            let s = sample as i64;
            energy += s * s;
        }

        // Root mean square
        let rms = if pcm_frame.is_empty() {
            0
        } else {
            (energy as f64 / pcm_frame.len() as f64).sqrt() as i32
        };

        let rms_f = rms as f64;
        // Thresholds relative to the adaptive noise floor, but never below the absolute floor.
        let speech_threshold = (self.noise_floor * SPEECH_MARGIN).max(self.energy_threshold as f64);
        let barge_threshold =
            (self.noise_floor * BARGE_MARGIN).max((self.energy_threshold * 2) as f64);

        let is_speech = rms_f > speech_threshold;
        let is_barge = rms_f > barge_threshold;

        if is_barge {
            self.consecutive_barge_ms += 20;
        } else {
            self.consecutive_barge_ms = 0;
        }

        if is_speech {
            self.consecutive_speech_ms += 20;
            self.consecutive_silence_ms = 0;
            VadState::Speech
        } else {
            self.consecutive_silence_ms += 20;
            self.consecutive_speech_ms = 0;
            // Adapt the noise floor toward the current (quiet) level during silence only, so speech
            // never inflates it. EMA with a slow rise / faster settle keeps it stable.
            self.noise_floor = 0.95 * self.noise_floor + 0.05 * rms_f;
            VadState::Silence
        }
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
        // Requires speech clearly above the noise floor (barge counter), not just any speech, so
        // breaths/backchannel while the agent talks don't cut it off.
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

    #[test]
    fn test_vad_engine_counters() {
        let mut engine = VadEngine::new(2);

        // Feed silence (all zeros)
        let silence_frame = vec![0i16; 160];
        let state = engine.process_frame(&silence_frame);

        assert_eq!(state, VadState::Silence);
        assert_eq!(engine.consecutive_silence_ms(), 20);
        assert_eq!(engine.consecutive_speech_ms(), 0);

        // Feed some "speech" (high amplitude noise to trigger VAD)
        let mut speech_frame = vec![0i16; 160];
        for (i, sample) in speech_frame.iter_mut().enumerate() {
            *sample = if i % 2 == 0 { 8000 } else { -8000 };
        }

        let state = engine.process_frame(&speech_frame);
        assert_eq!(state, VadState::Speech);
        assert_eq!(engine.consecutive_speech_ms(), 20);
        assert_eq!(engine.consecutive_silence_ms(), 0);

        assert!(engine.is_barge_in(20, true));
        assert!(!engine.is_barge_in(20, false));
    }
}
