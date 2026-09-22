//! STT provider selection + transparent mid-call failover (C1).
//!
//! The session hands its live μ-law audio + commit signals to `run_stt_with_failover`, which owns
//! those real receivers and forwards them to whichever provider is currently active. If the active
//! provider errors — at connect time or mid-call (e.g. ElevenLabs quota outage) — the supervisor
//! opens the NEXT provider and keeps the call alive; the caller may need to repeat the utterance in
//! flight, but the call is never dropped. Only when EVERY provider is exhausted does it return the
//! last error, at which point the session applies its existing graceful-end behavior (unchanged).
//!
//! Happy-path cost: exactly one extra in-process mpsc handoff (frames are already paced at 20 ms by
//! Twilio, so the handoff is drained immediately and adds no perceptible/turn latency). A healthy
//! primary never opens a fallback socket.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

/// A provider that ran at least this long before its socket ended is considered to have been
/// healthy — the close is a transient / vendor-cap event (e.g. ElevenLabs closing a realtime
/// socket after its session window), so we reset the consecutive-failure counter and reconnect
/// promptly. A provider that dies faster than this is counted as a failure toward the give-up cap.
const HEALTHY_RUN: Duration = Duration::from_secs(10);

/// Give up (return the last error → session applies its graceful-end behavior) only after this many
/// reconnect attempts fail back-to-back without any healthy run in between. A long, healthy call
/// that reconnects every so often never approaches this, because each healthy run resets the count.
const MAX_CONSECUTIVE_FAILURES: u32 = 6;

use crate::config::Config;
use crate::deepgram::{DeepgramParams, DeepgramStt};
use crate::error::MediaError;
use crate::stt::{ElevenLabsStt, Stt, SttParams};

/// A zero-arg factory that builds a fresh provider instance for one failover attempt. Boxed so the
/// supervisor can hold a heterogeneous, ordered list and rebuild a provider on each try.
type SttBuilder = Box<dyn Fn() -> Box<dyn Stt> + Send>;

/// Which STT vendor. Kept tiny; extend here when a third provider is added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SttProvider {
    ElevenLabs,
    Deepgram,
}

fn parse_provider(s: &str) -> Option<SttProvider> {
    match s.trim().to_lowercase().as_str() {
        "elevenlabs" | "eleven" | "11labs" | "el" => Some(SttProvider::ElevenLabs),
        "deepgram" | "dg" => Some(SttProvider::Deepgram),
        _ => None,
    }
}

/// True when the provider has the credentials it needs to be usable.
fn is_available(p: SttProvider, cfg: &Config) -> bool {
    match p {
        SttProvider::ElevenLabs => !cfg.elevenlabs_api_key.is_empty(),
        SttProvider::Deepgram => !cfg.deepgram_api_key.is_empty(),
    }
}

/// Ordered provider list: primary, then fallback (when `STT_FAILOVER` is on), deduped and filtered
/// to those with credentials. Falls back to ElevenLabs if config is empty/unknown.
pub fn provider_order(cfg: &Config) -> Vec<SttProvider> {
    let primary = parse_provider(&cfg.stt_provider).unwrap_or(SttProvider::ElevenLabs);
    let mut order = vec![primary];
    if cfg.stt_failover {
        if let Some(fb) = parse_provider(&cfg.stt_fallback_provider) {
            if fb != primary {
                order.push(fb);
            }
        }
    }
    order.retain(|p| is_available(*p, cfg));
    order
}

/// Build a provider instance from config + the shared `SttParams` the session already assembled.
fn build(kind: SttProvider, cfg: &Config, params: &SttParams) -> Box<dyn Stt> {
    match kind {
        SttProvider::ElevenLabs => Box::new(ElevenLabsStt::new(params.clone())),
        SttProvider::Deepgram => Box::new(DeepgramStt::new(DeepgramParams {
            api_key: cfg.deepgram_api_key.clone(),
            model: cfg.deepgram_stt_model.clone(),
            // Per-agent / global language wins; empty → Nova-3 multilingual ("multi").
            language: params.language_code.clone().filter(|s| !s.is_empty()),
            commit_strategy: params.commit_strategy.clone(),
            vad_silence_secs: params.vad_silence_secs,
            lexicon: params.lexicon.clone(),
        })),
    }
}

/// Entry point used by the session: resolve the provider order from config and run the supervisor.
pub async fn run_stt_with_failover(
    cfg: Arc<Config>,
    params: SttParams,
    ulaw_rx: mpsc::Receiver<Vec<u8>>,
    transcript_tx: mpsc::Sender<String>,
    commit_rx: mpsc::Receiver<()>,
) -> Result<(), MediaError> {
    let order = provider_order(&cfg);
    if order.is_empty() {
        return Err(MediaError::SttError(
            "no STT provider configured (missing credentials)".into(),
        ));
    }
    let builders: Vec<(SttProvider, SttBuilder)> = order
        .into_iter()
        .map(|kind| {
            let cfg = Arc::clone(&cfg);
            let params = params.clone();
            let f: SttBuilder = Box::new(move || build(kind, &cfg, &params));
            (kind, f)
        })
        .collect();
    run_failover_core(builders, ulaw_rx, transcript_tx, commit_rx).await
}

/// Provider-agnostic supervisor. Owns the real audio + commit receivers for the whole call and
/// bridges them into a fresh per-attempt inner channel for the active provider, so a provider can
/// be swapped — or the SAME provider reconnected — mid-call without ever losing the microphone.
/// Injectable builders make this unit-testable.
///
/// Reconnect model: a provider's `stream()` returns only when (a) the call really ends — detected
/// here as the upstream audio channel closing, in which case we return `Ok(())` and never
/// reconnect — or (b) the provider's socket ends on its own while the call is still live (vendor
/// close such as ElevenLabs' realtime session window, or an error). Case (b) is transient by
/// nature, so we cycle to the next provider and open a fresh socket, keeping the call alive. We
/// only give up after `MAX_CONSECUTIVE_FAILURES` rapid failures with no healthy run in between.
pub async fn run_failover_core<L: std::fmt::Debug>(
    builders: Vec<(L, SttBuilder)>,
    mut ulaw_rx: mpsc::Receiver<Vec<u8>>,
    transcript_tx: mpsc::Sender<String>,
    mut commit_rx: mpsc::Receiver<()>,
) -> Result<(), MediaError> {
    let total = builders.len();
    if total == 0 {
        return Err(MediaError::SttError("no STT provider available".into()));
    }

    let mut last_err = MediaError::SttError("no STT provider available".into());
    let mut attempt: usize = 0;
    let mut consecutive_failures: u32 = 0;
    // Predictive endpointing may report "no more commits" once; remember it so we don't reopen the
    // commit bridge on a reconnect (the source is gone for the rest of the call).
    let mut commit_open = true;

    loop {
        let (label, make) = &builders[attempt % total];
        attempt += 1;

        // Fresh inner channels for this provider attempt (a reconnect gets a clean socket).
        let (in_ulaw_tx, in_ulaw_rx) = mpsc::channel::<Vec<u8>>(256);
        let (in_commit_tx, in_commit_rx) = mpsc::channel::<()>(8);

        let provider = make();
        let ttx = transcript_tx.clone();
        let mut handle =
            tokio::spawn(async move { provider.stream(in_ulaw_rx, ttx, in_commit_rx).await });

        let started = Instant::now();
        let mut upstream_closed = false;
        // Bridge real audio/commit → the active provider until either the provider task ends
        // (→ reconnect) or the call ends (→ return). `handle` is only borrowed inside the select;
        // it is joined AFTER the loop so there is no borrow/move conflict.
        let result: Result<(), MediaError> = loop {
            tokio::select! {
                maybe_audio = ulaw_rx.recv() => match maybe_audio {
                    // Bridge one frame to the active provider. try_send preserves the existing
                    // drop-if-busy behavior (never block the audio path).
                    Some(frame) => { let _ = in_ulaw_tx.try_send(frame); }
                    None => { upstream_closed = true; break Ok(()); }
                },
                maybe_commit = commit_rx.recv(), if commit_open => match maybe_commit {
                    Some(()) => { let _ = in_commit_tx.try_send(()); }
                    None => commit_open = false, // predictive endpointing off / no more commits
                },
                res = &mut handle => {
                    break res.unwrap_or_else(|e| Err(MediaError::SttError(format!("stt task join: {e}"))));
                }
            }
        };

        // Always release this attempt's inner senders so the provider task fully winds down.
        drop(in_ulaw_tx);
        drop(in_commit_tx);

        if upstream_closed {
            // The call ended: wrap up the provider and finish — no reconnect, regardless of exit.
            let _ = handle.await;
            return Ok(());
        }

        // Provider ended on its own while the call is still live → reconnect.
        let ran = started.elapsed();
        match result {
            Ok(()) => tracing::warn!(
                "STT provider {:?} closed mid-call after {:?} (call still live); reconnecting",
                label,
                ran
            ),
            Err(e) => {
                last_err = e;
                tracing::warn!(
                    "STT provider {:?} ended mid-call ({}) after {:?}; reconnecting",
                    label,
                    last_err,
                    ran
                );
            }
        }

        // A healthy run (socket lived a while) resets the failure budget so long calls that
        // reconnect periodically never exhaust it. Rapid, repeated failures do count and eventually
        // stop us hot-looping when every provider is genuinely down.
        if ran >= HEALTHY_RUN {
            consecutive_failures = 0;
        } else {
            consecutive_failures += 1;
            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                tracing::error!(
                    "STT: {} reconnect attempts failed back-to-back; giving up",
                    consecutive_failures
                );
                return Err(last_err);
            }
            // Brief backoff before the next attempt so we don't spin on an insta-failing provider.
            // Zero after a healthy run (fast reconnect on the common vendor-close case).
            let backoff = Duration::from_millis(100 * u64::from(consecutive_failures.min(5)));
            tokio::time::sleep(backoff).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Provider that fails immediately on connect (simulates ElevenLabs quota/connect outage).
    struct FailProvider(&'static str);
    #[async_trait]
    impl Stt for FailProvider {
        async fn stream(
            &self,
            _ulaw_rx: mpsc::Receiver<Vec<u8>>,
            _transcript_tx: mpsc::Sender<String>,
            _commit_rx: mpsc::Receiver<()>,
        ) -> Result<(), MediaError> {
            Err(MediaError::SttError(format!("{} down", self.0)))
        }
    }

    /// Provider that emits one transcript for the first frame it receives, then runs until the
    /// audio channel closes.
    struct EchoProvider;
    #[async_trait]
    impl Stt for EchoProvider {
        async fn stream(
            &self,
            mut ulaw_rx: mpsc::Receiver<Vec<u8>>,
            transcript_tx: mpsc::Sender<String>,
            _commit_rx: mpsc::Receiver<()>,
        ) -> Result<(), MediaError> {
            let mut said = false;
            while let Some(_frame) = ulaw_rx.recv().await {
                if !said {
                    said = true;
                    if transcript_tx
                        .send("hello from fallback".into())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            Ok(())
        }
    }

    /// Provider that emits `label` on its first frame, then returns `Ok(())` — simulating a vendor
    /// closing the socket mid-call (e.g. ElevenLabs' realtime session window) while the call lives.
    struct EmitThenClose(&'static str);
    #[async_trait]
    impl Stt for EmitThenClose {
        async fn stream(
            &self,
            mut ulaw_rx: mpsc::Receiver<Vec<u8>>,
            transcript_tx: mpsc::Sender<String>,
            _commit_rx: mpsc::Receiver<()>,
        ) -> Result<(), MediaError> {
            if let Some(_frame) = ulaw_rx.recv().await {
                let _ = transcript_tx.send(self.0.into()).await;
            }
            Ok(()) // socket "closed" — supervisor must reconnect, not treat this as call-end
        }
    }

    #[tokio::test]
    async fn fails_over_to_second_provider_and_keeps_transcribing() {
        let (ulaw_tx, ulaw_rx) = mpsc::channel::<Vec<u8>>(64);
        let (tx, mut rx) = mpsc::channel::<String>(4);
        let (_commit_tx, commit_rx) = mpsc::channel::<()>(4);

        let builders: Vec<(&str, SttBuilder)> = vec![
            (
                "primary",
                Box::new(|| Box::new(FailProvider("primary")) as Box<dyn Stt>),
            ),
            (
                "fallback",
                Box::new(|| Box::new(EchoProvider) as Box<dyn Stt>),
            ),
        ];

        let sup = tokio::spawn(run_failover_core(builders, ulaw_rx, tx, commit_rx));

        // Feed audio continuously (as a real caller does, one 20 ms frame at a time). Frames the
        // failing primary consumes before it errors are expected to drop — the fallback still
        // receives the ongoing stream and transcribes it.
        let feeder = tokio::spawn(async move {
            for _ in 0..50 {
                if ulaw_tx.send(vec![0u8; 160]).await.is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });

        let got = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("transcript should arrive via fallback")
            .expect("channel open");
        assert_eq!(got, "hello from fallback");

        feeder.abort(); // dropping ulaw_tx ends the call → supervisor returns
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), sup).await;
    }

    #[tokio::test]
    async fn reconnects_same_provider_when_socket_closes_mid_call() {
        // A single provider whose socket "closes" mid-call on the first attempt, then stays up on
        // the reconnect. The supervisor must reconnect (not return Ok on the first close) and keep
        // transcribing — this is the 90s-silence regression guard.
        let (ulaw_tx, ulaw_rx) = mpsc::channel::<Vec<u8>>(64);
        let (tx, mut rx) = mpsc::channel::<String>(4);
        let (_commit_tx, commit_rx) = mpsc::channel::<()>(4);

        let n = Arc::new(AtomicUsize::new(0));
        let builders: Vec<(&str, SttBuilder)> = vec![(
            "only",
            Box::new(move || {
                if n.fetch_add(1, Ordering::SeqCst) == 0 {
                    Box::new(EmitThenClose("first turn")) as Box<dyn Stt>
                } else {
                    Box::new(EchoProvider) as Box<dyn Stt>
                }
            }),
        )];

        let sup = tokio::spawn(run_failover_core(builders, ulaw_rx, tx, commit_rx));

        let feeder = tokio::spawn(async move {
            for _ in 0..80 {
                if ulaw_tx.send(vec![0u8; 160]).await.is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });

        // First instance emits, then closes; the supervisor reconnects and the second instance
        // keeps transcribing. We must observe BOTH — proving the mid-call close was recovered.
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("first transcript should arrive")
            .expect("channel open");
        assert_eq!(first, "first turn");
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("second transcript should arrive after reconnect")
            .expect("channel open");
        assert_eq!(second, "hello from fallback");

        feeder.abort();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), sup).await;
    }

    #[tokio::test]
    async fn returns_error_when_all_providers_fail() {
        let (_ulaw_tx, ulaw_rx) = mpsc::channel::<Vec<u8>>(16);
        let (tx, _rx) = mpsc::channel::<String>(4);
        let (_commit_tx, commit_rx) = mpsc::channel::<()>(4);

        let attempts = Arc::new(AtomicUsize::new(0));
        let a1 = Arc::clone(&attempts);
        let a2 = Arc::clone(&attempts);
        let builders: Vec<(&str, SttBuilder)> = vec![
            (
                "p1",
                Box::new(move || {
                    a1.fetch_add(1, Ordering::SeqCst);
                    Box::new(FailProvider("p1")) as Box<dyn Stt>
                }),
            ),
            (
                "p2",
                Box::new(move || {
                    a2.fetch_add(1, Ordering::SeqCst);
                    Box::new(FailProvider("p2")) as Box<dyn Stt>
                }),
            ),
        ];

        let res = run_failover_core(builders, ulaw_rx, tx, commit_rx).await;
        assert!(res.is_err());
        // Both providers fail instantly (< HEALTHY_RUN), so each reconnect counts toward the
        // give-up budget; the supervisor tries MAX_CONSECUTIVE_FAILURES times (alternating the two
        // providers) before returning the last error.
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            MAX_CONSECUTIVE_FAILURES as usize,
            "supervisor retries up to the failure cap before giving up"
        );
    }

    #[test]
    fn provider_parsing_is_lenient() {
        assert_eq!(parse_provider("Deepgram"), Some(SttProvider::Deepgram));
        assert_eq!(
            parse_provider(" elevenlabs "),
            Some(SttProvider::ElevenLabs)
        );
        assert_eq!(parse_provider("nope"), None);
    }
}
