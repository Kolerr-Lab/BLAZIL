//! Predictive endpointing (Bước 3): Smart Turn v2 semantic end-of-turn detection.
//!
//! Smart Turn v2 (pipecat) is a wav2vec2 model: input = 16 kHz mono waveform (up to 8 s), output =
//! a single probability that the speaker has FINISHED their turn (≥ threshold ⇒ complete). We run
//! it locally on CPU via ONNX Runtime (`ort`), so the session can commit a turn the instant the
//! caller is done instead of waiting out a fixed silence window.
//!
//! Telephony audio is μ-law 8 kHz → PCM16 8 kHz; this module upsamples to 16 kHz and applies
//! wav2vec2-style normalization (zero mean, unit variance) before inference.
//!
//! Fail-safe: any load/inference error makes `is_complete` return false, so the caller simply
//! falls back to its silence-timeout commit — a bad model never cuts a caller off mid-sentence.

use std::sync::Mutex;

use ort::session::Session;

const TARGET_HZ: usize = 16_000;
const MAX_SAMPLES: usize = TARGET_HZ * 8; // model accepts up to 8 seconds

pub struct SmartTurn {
    session: Mutex<Session>,
    threshold: f32,
}

impl SmartTurn {
    /// Load the ONNX model. Returns Err if the file is missing/invalid (caller should then leave
    /// predictive endpointing off and keep the silence-based path).
    pub fn load(model_path: &str, threshold: f32) -> anyhow::Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("builder failed: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| anyhow::anyhow!("with_intra_threads failed: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| anyhow::anyhow!("commit_from_file failed: {e}"))?;
        tracing::info!("Smart Turn model loaded from {model_path} (threshold {threshold})");
        Ok(Self {
            session: Mutex::new(session),
            threshold,
        })
    }

    /// Probability that the utterance (PCM16 mono @ 8 kHz) is a completed turn, in [0, 1].
    pub fn probability(&self, pcm8k: &[i16]) -> anyhow::Result<f32> {
        if pcm8k.is_empty() {
            return Ok(0.0);
        }
        // 8 kHz → 16 kHz (linear 2×), to f32, wav2vec2 normalize, cap to the last 8 s.
        let up = upsample_2x(pcm8k);
        let mut x: Vec<f32> = up.iter().map(|&s| s as f32 / 32768.0).collect();
        if x.len() > MAX_SAMPLES {
            let drop = x.len() - MAX_SAMPLES;
            x.drain(0..drop);
        }
        normalize(&mut x);
        let n = x.len();

        let mut sess = self.session.lock().expect("smart_turn session lock");
        // Use the model's declared first input name (wav2vec2 exports call it "input_values").
        let input_name = sess.inputs()[0].name().to_string();
        let tensor = ort::value::Tensor::from_array(([1_i64, n as i64], x))
            .map_err(|e| anyhow::anyhow!("tensor creation failed: {e}"))?;
        let outputs = sess
            .run(ort::inputs![input_name.as_str() => tensor])
            .map_err(|e| anyhow::anyhow!("sess.run failed: {e}"))?;

        let data = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("try_extract_tensor failed: {e}"))?;
        let raw = data.1.first().copied().unwrap_or(0.0);
        // Model may emit a probability directly or a logit; sigmoid only if it's out of [0,1].
        let prob = if (0.0..=1.0).contains(&raw) {
            raw
        } else {
            1.0 / (1.0 + (-raw).exp())
        };
        Ok(prob)
    }

    /// True if the utterance looks complete. Fail-safe: any error ⇒ false (fall back to silence).
    pub fn is_complete(&self, pcm8k: &[i16]) -> bool {
        match self.probability(pcm8k) {
            Ok(p) => {
                tracing::debug!("smart_turn prob={p:.3} (threshold {})", self.threshold);
                p >= self.threshold
            }
            Err(e) => {
                tracing::warn!("smart_turn inference failed, falling back to silence: {e}");
                false
            }
        }
    }
}

/// Linear 2× upsample 8 kHz → 16 kHz (insert one interpolated sample between each pair).
fn upsample_2x(pcm: &[i16]) -> Vec<i16> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for i in 0..pcm.len() {
        let cur = pcm[i];
        out.push(cur);
        let next = if i + 1 < pcm.len() { pcm[i + 1] } else { cur };
        out.push(((cur as i32 + next as i32) / 2) as i16);
    }
    out
}

/// wav2vec2 feature normalization: zero mean, unit variance.
fn normalize(x: &mut [f32]) {
    let n = x.len() as f32;
    if n == 0.0 {
        return;
    }
    let mean = x.iter().sum::<f32>() / n;
    let var = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
    let std = (var + 1e-7).sqrt();
    for v in x.iter_mut() {
        *v = (*v - mean) / std;
    }
}
