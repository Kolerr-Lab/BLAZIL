#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Speech,
    Silence,
}

#[allow(dead_code)]
pub struct VadEngine {
    consecutive_speech_ms: u64,
    consecutive_silence_ms: u64,
    energy_threshold: i32,
}

#[allow(dead_code)]
impl VadEngine {
    pub fn new(aggressiveness: u8) -> Self {
        // aggressiveness: 0 (least) to 3 (most aggressive)
        // Energy threshold: lower is more sensitive to speech
        let energy_threshold = match aggressiveness {
            0 => 500,
            1 => 1000,
            2 => 2000,
            _ => 4000,
        };

        Self {
            consecutive_speech_ms: 0,
            consecutive_silence_ms: 0,
            energy_threshold,
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

        // Heuristic: Speech generally has higher energy.
        // Zero crossings can distinguish voiced vs unvoiced, but for simple VAD we just use energy.
        let is_speech = rms > self.energy_threshold;

        if is_speech {
            self.consecutive_speech_ms += 20;
            self.consecutive_silence_ms = 0;
            VadState::Speech
        } else {
            self.consecutive_silence_ms += 20;
            self.consecutive_speech_ms = 0;
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
        assistant_speaking && self.consecutive_speech_ms >= threshold_ms
    }

    pub fn reset_counters(&mut self) {
        self.consecutive_speech_ms = 0;
        self.consecutive_silence_ms = 0;
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
