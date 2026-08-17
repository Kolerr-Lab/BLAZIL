use webrtc_vad::{Vad, SampleRate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Speech,
    Silence,
}

#[allow(dead_code)]
pub struct VadEngine {
    vad: Vad,
    consecutive_speech_ms: u64,
    consecutive_silence_ms: u64,
}

#[allow(dead_code)]
impl VadEngine {
    pub fn new(aggressiveness: u8) -> Self {
        let mode = match aggressiveness {
            0 => webrtc_vad::VadMode::Quality,
            1 => webrtc_vad::VadMode::LowBitrate,
            2 => webrtc_vad::VadMode::Aggressive,
            _ => webrtc_vad::VadMode::VeryAggressive,
        };
        
        Self {
            vad: Vad::new_with_rate_and_mode(SampleRate::Rate8kHz, mode),
            consecutive_speech_ms: 0,
            consecutive_silence_ms: 0,
        }
    }

    /// Process a 20ms PCM16 frame.
    /// Frame must be exactly 160 samples (20ms at 8kHz).
    pub fn process_frame(&mut self, pcm_frame: &[i16]) -> VadState {
        // webrtc-vad expects 10, 20, or 30 ms frames. We use 20ms.
        let is_speech = self.vad.is_voice_segment(pcm_frame).unwrap_or(false);

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
        for i in 0..160 {
            speech_frame[i] = if i % 2 == 0 { 8000 } else { -8000 };
        }
        
        let state = engine.process_frame(&speech_frame);
        assert_eq!(state, VadState::Speech);
        assert_eq!(engine.consecutive_speech_ms(), 20);
        assert_eq!(engine.consecutive_silence_ms(), 0);
        
        assert!(engine.is_barge_in(20, true));
        assert!(!engine.is_barge_in(20, false));
    }
}
