//! Target-speaker isolation (noise robustness): a speaker-embedding model that lets the pipeline
//! lock onto the ONE caller it enrolled and reject segments dominated by a DIFFERENT voice
//! (background people, TV) — which generic denoise/VAD can't do because that interference IS speech.
//!
//! Model: 3D-Speaker / WeSpeaker CAM++ exported to ONNX (input `x` = fbank features `[N, T, 80]`,
//! f32; output = an L2-normalizable speaker embedding). Run locally on CPU via `ort` (same runtime
//! as Smart Turn). We reproduce the kaldi-native-fbank front-end sherpa-onnx feeds these models:
//!   telephony μ-law 8 kHz → PCM16 → upsample 16 kHz → f32 → per-25ms/10ms frame:
//!   remove-DC → preemphasis(0.97) → Povey window → |rFFT(512)|² → 80 HTK-mel triangles (low 20 Hz)
//!   → ln(max(e, 1e-10)) → per-utterance cepstral mean subtraction (CMN over time).
//!
//! IMPORTANT: the fbank config MUST match the exact ONNX or embeddings are garbage. This is validated
//! against a kaldi-native-fbank / torchaudio.compliance.kaldi reference before the gate is enabled
//! (see `SPEAKER_GATE_ENABLED`, default off). Any load/inference error ⇒ `None`/passthrough — the
//! gate never rejects audio when it can't be sure.
//!
//! STAGED: the embedder + fbank front-end land first and are validated against a reference; the
//! enrollment + per-utterance gate wiring into the endpoint/commit path comes in a follow-up once
//! the front-end is confirmed. `allow(dead_code)` until then.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ort::session::Session;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use tokio::sync::mpsc;

const SR: usize = 16_000;
const FRAME_LEN: usize = 400; // 25 ms @ 16 kHz
const FRAME_SHIFT: usize = 160; // 10 ms @ 16 kHz
const N_FFT: usize = 512; // kaldi rounds frame_length up to a power of two
const N_FREQS: usize = N_FFT / 2 + 1; // 257
const N_MELS: usize = 80;
const LOW_FREQ: f32 = 20.0;
const PREEMPH: f32 = 0.97;
// Match torchaudio.compliance.kaldi / kaldi-native-fbank: floor mel energy at f32 epsilon before log.
const LOG_FLOOR: f32 = f32::EPSILON;

pub struct SpeakerEmbedder {
    session: Mutex<Session>,
    mel: Vec<f32>,   // HTK mel filterbank, row-major [N_MELS * N_FREQS]
    povey: Vec<f32>, // Povey window, length FRAME_LEN
    fft: Arc<dyn Fft<f32>>,
}

impl SpeakerEmbedder {
    pub fn load(model_path: &str) -> anyhow::Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("builder failed: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| anyhow::anyhow!("with_intra_threads failed: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| anyhow::anyhow!("commit_from_file failed: {e}"))?;

        // Povey window: (Hann)^0.85 — kaldi's default window for fbank.
        let povey: Vec<f32> = (0..FRAME_LEN)
            .map(|i| {
                let hann = 0.5
                    - 0.5
                        * (2.0 * std::f32::consts::PI * i as f32 / (FRAME_LEN as f32 - 1.0)).cos();
                hann.powf(0.85)
            })
            .collect();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(N_FFT);

        tracing::info!("Speaker embedder (CAM++) loaded from {model_path}");
        Ok(Self {
            session: Mutex::new(session),
            mel: htk_mel_filterbank(),
            povey,
            fft,
        })
    }

    /// L2-normalized speaker embedding for a PCM16 mono @ 8 kHz utterance. Empty ⇒ error.
    pub fn embed(&self, pcm8k: &[i16]) -> anyhow::Result<Vec<f32>> {
        if pcm8k.len() < FRAME_LEN {
            return Err(anyhow::anyhow!("utterance too short for embedding"));
        }
        let up = upsample_2x(pcm8k);
        let audio: Vec<f32> = up.iter().map(|&s| s as f32).collect(); // kaldi works on int-scale f32
        let fbank = self.fbank(&audio); // [T * N_MELS], time-major, CMN-applied
        let n_frames = fbank.len() / N_MELS;
        if n_frames == 0 {
            return Err(anyhow::anyhow!("no frames"));
        }

        let mut sess = self.session.lock().expect("speaker session lock");
        let input_name = sess.inputs()[0].name().to_string(); // "x"
        let tensor =
            ort::value::Tensor::from_array(([1_i64, n_frames as i64, N_MELS as i64], fbank))
                .map_err(|e| anyhow::anyhow!("tensor creation failed: {e}"))?;
        let outputs = sess
            .run(ort::inputs![input_name.as_str() => tensor])
            .map_err(|e| anyhow::anyhow!("sess.run failed: {e}"))?;
        let data = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("try_extract_tensor failed: {e}"))?;
        let mut emb: Vec<f32> = data.1.to_vec();
        l2_normalize(&mut emb);
        Ok(emb)
    }

    /// kaldi-native-fbank compatible 80-bin log-mel fbank with per-utterance CMN. Time-major output.
    fn fbank(&self, audio: &[f32]) -> Vec<f32> {
        compute_fbank(audio, &self.mel, &self.povey, self.fft.as_ref())
    }
}

/// Free-function fbank (so it can be golden-tested against a kaldi reference without loading ONNX).
/// `audio` is int-scale f32 @ 16 kHz (kaldi convention). Output: [T * N_MELS], time-major, CMN-applied.
fn compute_fbank(audio: &[f32], mel: &[f32], povey: &[f32], fft: &dyn Fft<f32>) -> Vec<f32> {
    if audio.len() < FRAME_LEN {
        return Vec::new();
    }
    let n_frames = 1 + (audio.len() - FRAME_LEN) / FRAME_SHIFT;
    let mut out = vec![0.0f32; n_frames * N_MELS];
    let mut buf = vec![
        Complex {
            re: 0.0f32,
            im: 0.0f32
        };
        N_FFT
    ];

    for t in 0..n_frames {
        let start = t * FRAME_SHIFT;
        let mut frame = [0.0f32; FRAME_LEN];
        frame.copy_from_slice(&audio[start..start + FRAME_LEN]);

        // 1) remove DC offset (subtract the frame mean).
        let mean = frame.iter().sum::<f32>() / FRAME_LEN as f32;
        for v in frame.iter_mut() {
            *v -= mean;
        }
        // 2) preemphasis (kaldi order: x[i] -= c*x[i-1] descending, then x[0] -= c*x[0]).
        for i in (1..FRAME_LEN).rev() {
            frame[i] -= PREEMPH * frame[i - 1];
        }
        frame[0] -= PREEMPH * frame[0];
        // 3) Povey window.
        for i in 0..FRAME_LEN {
            frame[i] *= povey[i];
        }
        // 4) rFFT over a 512-point zero-padded frame → power spectrum.
        for i in 0..N_FFT {
            buf[i].re = if i < FRAME_LEN { frame[i] } else { 0.0 };
            buf[i].im = 0.0;
        }
        fft.process(&mut buf);
        let mut power = [0.0f32; N_FREQS];
        for (k, item) in buf.iter().take(N_FREQS).enumerate() {
            power[k] = item.norm_sqr();
        }
        // 5) mel projection + natural log with floor.
        for m in 0..N_MELS {
            let filt = &mel[m * N_FREQS..(m + 1) * N_FREQS];
            let mut acc = 0.0f32;
            for k in 0..N_FREQS {
                acc += filt[k] * power[k];
            }
            out[t * N_MELS + m] = acc.max(LOG_FLOOR).ln();
        }
    }

    // 6) per-utterance CMN: subtract the mean over time for each mel bin (WeSpeaker/3D-Speaker).
    for m in 0..N_MELS {
        let mut mean = 0.0f32;
        for t in 0..n_frames {
            mean += out[t * N_MELS + m];
        }
        mean /= n_frames as f32;
        for t in 0..n_frames {
            out[t * N_MELS + m] -= mean;
        }
    }
    out
}

/// Cosine similarity of two L2-normalized embeddings (= dot product). Used to gate: high ⇒ same
/// speaker as the enrolled caller, low ⇒ a different voice (background person / TV).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

const SR_8K: usize = 8000;
/// Accumulate this much of the caller's own speech before enrolling their target embedding.
const ENROLL_MIN_MS: usize = 1200;
/// Rolling window over which the "is this the enrolled speaker right now?" decision is made.
const WINDOW_MS: usize = 600;
/// Re-score the window at most this often (bounds embedding CPU; 20 ms frames).
const CLASSIFY_EVERY_FRAMES: usize = 12; // ~240 ms
/// Require this many consecutive non-target windows before muting (hysteresis → never drop the real
/// caller on a single bad window; re-open immediately on a target hit).
const MISS_STREAK_TO_MUTE: u32 = 2;

/// Frame-level target-speaker gate (#3). Owns the enrollment + rolling-window classification off the
/// audio thread; publishes two booleans the audio path reads: `enrolled` (target known) and `active`
/// (the current speaker IS the target). When not enrolled or not active, the audio path feeds STT
/// silence instead of the frame, so a background voice/TV never reaches transcription.
///
/// Causal by design (no added end-to-end latency): the decision trails the audio by up to ~half the
/// window; `active` defaults true right after enrollment and only flips off after MISS_STREAK_TO_MUTE.
pub async fn speaker_gate_loop(
    embedder: Arc<SpeakerEmbedder>,
    mut rx: mpsc::Receiver<(Vec<i16>, bool)>,
    enrolled: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    threshold: f32,
) {
    let enroll_min = ENROLL_MIN_MS * SR_8K / 1000;
    let window_samples = WINDOW_MS * SR_8K / 1000;
    let mut enroll_buf: Vec<i16> = Vec::with_capacity(enroll_min + 320);
    let mut window: VecDeque<i16> = VecDeque::with_capacity(window_samples + 320);
    let mut target: Option<Vec<f32>> = None;
    let mut frames_since_classify = 0usize;
    let mut miss_streak = 0u32;

    while let Some((frame, is_speech)) = rx.recv().await {
        // Phase 1: enroll the caller's own voice from their first ~1.2 s of speech.
        if target.is_none() {
            if is_speech {
                enroll_buf.extend_from_slice(&frame);
                if enroll_buf.len() >= enroll_min {
                    let buf = std::mem::take(&mut enroll_buf);
                    let em = Arc::clone(&embedder);
                    let res = tokio::task::spawn_blocking(move || em.embed(&buf)).await;
                    match res {
                        Ok(Ok(e)) => {
                            target = Some(e);
                            enrolled.store(true, Ordering::Relaxed);
                            active.store(true, Ordering::Relaxed);
                            tracing::info!("target speaker enrolled ({ENROLL_MIN_MS} ms)");
                        }
                        _ => { /* embed failed → try again with the next speech */ }
                    }
                }
            }
            continue;
        }

        // Phase 2: rolling-window classification while the caller (or someone) is speaking.
        if is_speech {
            for &s in &frame {
                window.push_back(s);
            }
            while window.len() > window_samples {
                window.pop_front();
            }
            frames_since_classify += 1;
            if frames_since_classify >= CLASSIFY_EVERY_FRAMES && window.len() >= window_samples / 2
            {
                frames_since_classify = 0;
                let win: Vec<i16> = window.iter().copied().collect();
                let em = Arc::clone(&embedder);
                let tgt = target.clone().unwrap();
                // On any embedding/join error, sim = 1.0 → never gate out on uncertainty.
                let sim = tokio::task::spawn_blocking(move || {
                    em.embed(&win).map(|e| cosine(&e, &tgt)).unwrap_or(1.0)
                })
                .await
                .unwrap_or(1.0);
                if sim >= threshold {
                    miss_streak = 0;
                    active.store(true, Ordering::Relaxed);
                } else {
                    miss_streak += 1;
                    if miss_streak >= MISS_STREAK_TO_MUTE {
                        active.store(false, Ordering::Relaxed);
                    }
                }
            }
        }
        // During silence we leave `active` as-is; the audio path forwards μ-law silence anyway.
    }
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Linear 2× upsample 8 kHz → 16 kHz (telephony is band-limited, so this is adequate for fbank).
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

/// Kaldi HTK-style mel filterbank: 80 triangles from LOW_FREQ to Nyquist, mel = 1127·ln(1+f/700),
/// triangles peak at 1.0 (kaldi does NOT area-normalize like Slaney). Row-major [N_MELS * N_FREQS].
fn htk_mel_filterbank() -> Vec<f32> {
    fn hz_to_mel(f: f32) -> f32 {
        1127.0 * (1.0 + f / 700.0).ln()
    }
    let nyquist = (SR / 2) as f32;
    let mel_low = hz_to_mel(LOW_FREQ);
    let mel_high = hz_to_mel(nyquist);
    // N_MELS + 2 mel-spaced edges.
    let edges: Vec<f32> = (0..N_MELS + 2)
        .map(|i| mel_low + (mel_high - mel_low) * i as f32 / (N_MELS + 1) as f32)
        .collect();
    let bin_hz: Vec<f32> = (0..N_FREQS)
        .map(|k| k as f32 * SR as f32 / N_FFT as f32)
        .collect();

    let mut weights = vec![0.0f32; N_MELS * N_FREQS];
    for m in 0..N_MELS {
        let left = edges[m];
        let center = edges[m + 1];
        let right = edges[m + 2];
        for k in 0..N_FREQS {
            let mel = hz_to_mel(bin_hz[k]);
            let w = if mel <= left || mel >= right {
                0.0
            } else if mel <= center {
                (mel - left) / (center - left)
            } else {
                (right - mel) / (right - center)
            };
            weights[m * N_FREQS + k] = w;
        }
    }
    weights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_of_identical_is_one() {
        let v = vec![0.5f32, 0.5, 0.5, 0.5];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_is_zero() {
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn mel_filterbank_shape_and_range() {
        let mel = htk_mel_filterbank();
        assert_eq!(mel.len(), N_MELS * N_FREQS);
        assert!(mel.iter().all(|&w| (0.0..=1.0).contains(&w)));
        // Every triangle should have some non-zero weight.
        for m in 0..N_MELS {
            let row = &mel[m * N_FREQS..(m + 1) * N_FREQS];
            assert!(row.iter().any(|&w| w > 0.0), "mel bin {m} is all zero");
        }
    }

    // Golden test: our Rust fbank must match torchaudio.compliance.kaldi.fbank (+ CMN) bit-close, on
    // a deterministic 1 s signal. Reference produced on the model host (3D-Speaker CAM++ pipeline):
    //   FBANK_SHAPE [98, 80]
    //   FBANK[0][:8] = [0.5353, 0.0686, 0.0246, -0.0097, 0.0018, 0.0087, -0.0073, -0.9996]
    #[test]
    fn fbank_matches_kaldi_reference() {
        use rustfft::FftPlanner;
        use std::f32::consts::PI;

        let sr = 16_000usize;
        let audio: Vec<f32> = (0..sr)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (0.10 * (2.0 * PI * 150.0 * t).sin() + 0.05 * (2.0 * PI * 300.0 * t).sin())
                    * 32768.0
            })
            .collect();

        let povey: Vec<f32> = (0..FRAME_LEN)
            .map(|i| {
                let hann = 0.5 - 0.5 * (2.0 * PI * i as f32 / (FRAME_LEN as f32 - 1.0)).cos();
                hann.powf(0.85)
            })
            .collect();
        let mel = htk_mel_filterbank();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(N_FFT);

        let out = compute_fbank(&audio, &mel, &povey, fft.as_ref());
        assert_eq!(out.len() / N_MELS, 98, "frame count should be 98");

        let expect = [
            0.5353, 0.0686, 0.0246, -0.0097, 0.0018, 0.0087, -0.0073, -0.9996,
        ];
        for (k, &want) in expect.iter().enumerate() {
            let got = out[k]; // frame 0, mel k
            assert!(
                (got - want).abs() < 0.03,
                "fbank[0][{k}] = {got:.4}, expected {want:.4}"
            );
        }
        // Reference showed frame 10 identical to frame 0 (periodic signal, 10 frames = 15 periods).
        for (k, &want) in expect.iter().enumerate() {
            let got = out[10 * N_MELS + k];
            assert!(
                (got - want).abs() < 0.03,
                "fbank[10][{k}] = {got:.4}, expected {want:.4}"
            );
        }
    }
}
