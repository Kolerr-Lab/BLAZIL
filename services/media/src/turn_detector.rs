//! Predictive endpointing (Bước 3): Smart Turn v3 semantic end-of-turn detection.
//!
//! Smart Turn v3 (pipecat) = a Whisper-tiny encoder + linear head that reads the raw waveform and
//! outputs a single sigmoid probability that the speaker has FINISHED their turn (≥ threshold ⇒
//! complete). We run it locally on CPU via ONNX Runtime (`ort`), committing a turn the instant the
//! caller is done instead of waiting out a fixed silence window.
//!
//! The ONNX graph takes Whisper LOG-MEL FEATURES ("input_features", shape [1, 80, 800]), not raw
//! audio, so this module replicates `WhisperFeatureExtractor(chunk_length=8)`:
//!   telephony μ-law 8 kHz → PCM16 → upsample 16 kHz → last 8 s (pad/truncate) → zero-mean/unit-var
//!   → STFT (n_fft 400, hop 160, Hann, reflect-centered) → power → 80-bin Slaney mel → log10 →
//!   clamp(max−8) → (x+4)/4.
//!
//! Fail-safe: any load/inference error makes `is_complete` return false, so the caller simply falls
//! back to its silence-timeout commit — a bad model never cuts a caller off mid-sentence.

use std::sync::{Arc, Mutex};

use ort::session::Session;
use rustfft::{num_complex::Complex, Fft, FftPlanner};

const SR: usize = 16_000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 80;
const N_SAMPLES: usize = 8 * SR; // 8-second window the model expects
const N_FRAMES: usize = N_SAMPLES / HOP; // 800
const N_FREQS: usize = N_FFT / 2 + 1; // 201

pub struct SmartTurn {
    session: Mutex<Session>,
    threshold: f32,
    mel: Vec<f32>,  // Slaney mel filterbank, row-major [N_MELS * N_FREQS]
    hann: Vec<f32>, // periodic Hann window, length N_FFT
    fft: Arc<dyn Fft<f32>>,
}

impl SmartTurn {
    /// Load the ONNX model. Err if the file is missing/invalid (caller keeps the silence path).
    pub fn load(model_path: &str, threshold: f32) -> anyhow::Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("builder failed: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| anyhow::anyhow!("with_intra_threads failed: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| anyhow::anyhow!("commit_from_file failed: {e}"))?;

        let hann: Vec<f32> = (0..N_FFT)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos())
            .collect();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(N_FFT);

        tracing::info!("Smart Turn v3 loaded from {model_path} (threshold {threshold})");
        Ok(Self {
            session: Mutex::new(session),
            threshold,
            mel: mel_filterbank(),
            hann,
            fft,
        })
    }

    /// Probability the utterance (PCM16 mono @ 8 kHz) is a completed turn, in [0, 1].
    pub fn probability(&self, pcm8k: &[i16]) -> anyhow::Result<f32> {
        if pcm8k.is_empty() {
            return Ok(0.0);
        }

        // 8 kHz → 16 kHz, to f32 [-1, 1], keep the last 8 s.
        let up = upsample_2x(pcm8k);
        let mut audio: Vec<f32> = up.iter().map(|&s| s as f32 / 32768.0).collect();
        if audio.len() > N_SAMPLES {
            audio.drain(0..audio.len() - N_SAMPLES);
        }

        // do_normalize=True over the REAL samples, then zero-pad to the fixed 8 s window.
        zero_mean_unit_var(&mut audio);
        audio.resize(N_SAMPLES, 0.0);

        let features = self.log_mel(&audio); // [N_MELS * N_FRAMES], mel-major

        let mut sess = self.session.lock().expect("smart_turn session lock");
        let input_name = sess.inputs()[0].name().to_string(); // "input_features"
        let tensor =
            ort::value::Tensor::from_array(([1_i64, N_MELS as i64, N_FRAMES as i64], features))
                .map_err(|e| anyhow::anyhow!("tensor creation failed: {e}"))?;
        let outputs = sess
            .run(ort::inputs![input_name.as_str() => tensor])
            .map_err(|e| anyhow::anyhow!("sess.run failed: {e}"))?;
        let data = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("try_extract_tensor failed: {e}"))?;
        let raw = data.1.first().copied().unwrap_or(0.0);
        // v3 already emits a sigmoid probability; sigmoid again only if it's somehow out of [0,1].
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

    /// Whisper log-mel features for a fixed-length (N_SAMPLES) audio buffer → [N_MELS * N_FRAMES].
    fn log_mel(&self, audio: &[f32]) -> Vec<f32> {
        let padded = reflect_pad(audio, N_FFT / 2); // center=True

        // Power spectrogram: [N_FRAMES][N_FREQS].
        let mut power = vec![0.0f32; N_FRAMES * N_FREQS];
        let mut buf = vec![
            Complex {
                re: 0.0f32,
                im: 0.0f32
            };
            N_FFT
        ];
        for t in 0..N_FRAMES {
            let start = t * HOP;
            for i in 0..N_FFT {
                buf[i].re = padded[start + i] * self.hann[i];
                buf[i].im = 0.0;
            }
            self.fft.process(&mut buf);
            for (k, item) in buf.iter().take(N_FREQS).enumerate() {
                power[t * N_FREQS + k] = item.norm_sqr();
            }
        }

        // Mel projection (mel-major output [m][t]) + log10.
        let mut mel_spec = vec![0.0f32; N_MELS * N_FRAMES];
        let mut global_max = f32::MIN;
        for m in 0..N_MELS {
            let filt = &self.mel[m * N_FREQS..(m + 1) * N_FREQS];
            for t in 0..N_FRAMES {
                let mut acc = 0.0f32;
                let frame = &power[t * N_FREQS..(t + 1) * N_FREQS];
                for k in 0..N_FREQS {
                    acc += filt[k] * frame[k];
                }
                let v = acc.max(1e-10).log10();
                mel_spec[m * N_FRAMES + t] = v;
                if v > global_max {
                    global_max = v;
                }
            }
        }

        // Whisper normalization: clamp to (max − 8), then (x + 4) / 4.
        let floor = global_max - 8.0;
        for v in mel_spec.iter_mut() {
            *v = (v.max(floor) + 4.0) / 4.0;
        }
        mel_spec
    }
}

/// Linear 2× upsample 8 kHz → 16 kHz.
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

/// Zero-mean / unit-variance over the samples (HuggingFace `do_normalize`, eps 1e-7).
fn zero_mean_unit_var(x: &mut [f32]) {
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

/// `reflect` padding of `pad` samples on each side (matches torch.stft center=True).
fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    let mut out = Vec::with_capacity(n + 2 * pad);
    for i in (1..=pad).rev() {
        out.push(x[i]); // x[pad], x[pad-1], …, x[1]
    }
    out.extend_from_slice(x);
    for i in 1..=pad {
        out.push(x[n - 1 - i]); // x[n-2], x[n-3], …, x[n-1-pad]
    }
    out
}

/// Slaney-normalized mel filterbank (librosa `mel(sr=16000, n_fft=400, n_mels=80, htk=False)`),
/// row-major [N_MELS * N_FREQS] — the exact filters Whisper's feature extractor uses.
fn mel_filterbank() -> Vec<f32> {
    fn hz_to_mel(f: f32) -> f32 {
        let f_sp = 200.0 / 3.0;
        let min_log_hz = 1000.0;
        let min_log_mel = min_log_hz / f_sp;
        let logstep = (6.4f32).ln() / 27.0;
        if f >= min_log_hz {
            min_log_mel + (f / min_log_hz).ln() / logstep
        } else {
            f / f_sp
        }
    }
    fn mel_to_hz(m: f32) -> f32 {
        let f_sp = 200.0 / 3.0;
        let min_log_hz = 1000.0;
        let min_log_mel = min_log_hz / f_sp;
        let logstep = (6.4f32).ln() / 27.0;
        if m >= min_log_mel {
            min_log_hz * (logstep * (m - min_log_mel)).exp()
        } else {
            f_sp * m
        }
    }

    let fmax = (SR / 2) as f32;
    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(fmax);
    // N_MELS + 2 band edges on the mel scale, back to Hz.
    let edges: Vec<f32> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f32 / (N_MELS + 1) as f32))
        .collect();
    let fftfreqs: Vec<f32> = (0..N_FREQS)
        .map(|k| k as f32 * SR as f32 / N_FFT as f32)
        .collect();

    let mut weights = vec![0.0f32; N_MELS * N_FREQS];
    for m in 0..N_MELS {
        let lower_edge = edges[m];
        let center = edges[m + 1];
        let upper_edge = edges[m + 2];
        let fdiff_lo = center - lower_edge;
        let fdiff_hi = upper_edge - center;
        let enorm = 2.0 / (upper_edge - lower_edge); // Slaney area normalization
        for k in 0..N_FREQS {
            let f = fftfreqs[k];
            let lower = (f - lower_edge) / fdiff_lo;
            let upper = (upper_edge - f) / fdiff_hi;
            let w = lower.min(upper).max(0.0);
            weights[m * N_FREQS + k] = w * enorm;
        }
    }
    weights
}
