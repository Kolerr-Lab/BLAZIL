# Media assets

## `quota.ulaw` — quota-outage fallback clip

Played to the caller when ElevenLabs returns a quota error (STT or TTS), right before the call is
ended gracefully — so an account-level outage produces a short spoken message instead of dead air.

Format: **G.711 μ-law, 8 kHz, mono, raw (headerless)** — the exact wire format Twilio Media Streams
expects. Keep it short (~3–5s).

This file is optional. If it's absent, the runtime still ends the call cleanly (no clip, no dead air).

### Render it (brand voice, best quality)

Render the line in your ElevenLabs voice (e.g. the account default), then convert:

```bash
ffmpeg -i quota.mp3 -ar 8000 -ac 1 -f mulaw quota.ulaw
```

Drop `quota.ulaw` in this folder and redeploy — the Dockerfile bakes it to `/opt/assets/quota.ulaw`
(overridable via `QUOTA_FALLBACK_AUDIO_PATH`).

Suggested line: "Sorry — we're having a technical issue right now. Please try your call again in a
few minutes."
