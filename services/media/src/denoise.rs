//! Background-noise suppression for the STT input (noise robustness, supersedes the CAM++ gate).
//!
//! WHY this instead of the speaker gate: the CAM++ target-speaker gate MUTED windows it judged
//! non-target, and on 8 kHz telephony its embeddings were unreliable → it intermittently muted the
//! REAL caller → dead-air. A denoiser is fundamentally safer: it only ATTENUATES background noise
//! and always passes speech through — there is no code path that produces silence, so it can never
//! cause dead-air.
//!
//! Engine: RNNoise via the pure-Rust `nnnoiseless` crate (its weights are baked into the crate — no
//! external model file / ONNX to download or bake, unlike CAM++). RNNoise runs at 48 kHz on 480-
//! sample (10 ms) frames, so we resample the telephony 8 kHz μ-law PCM up 6× → denoise → back down
//! 6×. A 20 ms Twilio frame (160 samples @ 8 kHz) becomes exactly 960 @ 48 kHz = two RNNoise frames,
//! so in steady state there is no sample drift. Leftover samples are buffered across calls so RNNoise
//! always sees aligned 480-sample frames.
//!
//! Applied only when DENOISE_ENABLED is on (default off) — landing it is inert until flipped, then
//! A/B on a real noisy call and compare STT accuracy.

use nnnoiseless::DenoiseState;

const RN_FRAME: usize = DenoiseState::FRAME_SIZE; // 480 samples @ 48 kHz (10 ms)
const RATIO: usize = 6; // 8 kHz → 48 kHz

pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    /// Last 8 kHz input sample, for linear-interpolation continuity across frames.
    last_in: f32,
    /// Upsampled 48 kHz samples awaiting a full RNNoise frame.
    in48: Vec<f32>,
    /// Denoised 48 kHz samples awaiting 6× decimation back to 8 kHz.
    out48: Vec<f32>,
}

impl Denoiser {
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            last_in: 0.0,
            in48: Vec::with_capacity(RN_FRAME * 2),
            out48: Vec::with_capacity(RN_FRAME * 2),
        }
    }

    /// Denoise a block of 8 kHz PCM (i16). Returns the cleaned block; output length tracks input in
    /// steady state (a few samples may lag through the internal buffers, which is inaudible/fine for
    /// STT). RNNoise samples are f32 in i16 range (NOT normalized to ±1).
    pub fn process_8k(&mut self, pcm: &[i16]) -> Vec<i16> {
        // 1) Upsample 6× with linear interpolation (p → c across 6 points, ending on c).
        for &s in pcm {
            let c = s as f32;
            for k in 1..=RATIO {
                self.in48
                    .push(self.last_in + (c - self.last_in) * (k as f32) / (RATIO as f32));
            }
            self.last_in = c;
        }

        // 2) Denoise every full 48 kHz frame; stash the denoised samples for decimation.
        let mut frame_in = [0.0f32; RN_FRAME];
        let mut frame_out = [0.0f32; RN_FRAME];
        while self.in48.len() >= RN_FRAME {
            frame_in.copy_from_slice(&self.in48[..RN_FRAME]);
            self.in48.drain(..RN_FRAME);
            self.state.process_frame(&mut frame_out, &frame_in);
            self.out48.extend_from_slice(&frame_out);
        }

        // 3) Downsample 6× (boxcar average of each group of 6 = a simple anti-alias lowpass).
        let groups = self.out48.len() / RATIO;
        let mut out = Vec::with_capacity(groups);
        for g in 0..groups {
            let sum: f32 = self.out48[g * RATIO..(g + 1) * RATIO].iter().sum();
            let v = (sum / RATIO as f32).round();
            out.push(v.clamp(i16::MIN as f32, i16::MAX as f32) as i16);
        }
        self.out48.drain(..groups * RATIO);
        out
    }
}
