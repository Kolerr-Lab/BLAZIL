#!/usr/bin/env python3
"""Fake Twilio <Stream> client for the Blazil media plane (H12 gate).

Simulates Twilio Media Streams over the WebSocket the media plane exposes at
`/media/twilio`: sends `connected` + `start` (with customParameters), streams G.711 μ-law
8 kHz frames as `media` events, and prints everything the server sends back (assistant
audio, `mark`, `clear`). Requires no telephony and no Twilio number.

What it verifies:
  * handshake + start with tenant_id/agent_id custom parameters
  * greeting: the agent speaks first on connect (inbound audio frames arrive)
  * a turn: streaming caller speech yields an answer (needs a real ELEVENLABS_API_KEY on the
    media plane for STT/TTS; use --wav with actual speech to get a transcript)
  * barge-in: with --barge, caller audio during the greeting must trigger a `clear`

Usage:
  pip install websockets
  python fake_twilio.py --tenant <TENANT_UUID> --agent <AGENT_UUID> \
      --url ws://localhost:8080/media/twilio [--wav speech_8k_mono.wav] [--barge]

The media plane must be running with ORCH_BASE_URL pointed at a reachable backend and a valid
ELEVENLABS_API_KEY + ORCH_SERVICE_TOKEN.
"""
from __future__ import annotations

import argparse
import asyncio
import base64
import json
import math
import wave

import websockets

FRAME_SAMPLES = 160  # 20 ms @ 8 kHz
FRAME_MS = 20


def linear_to_ulaw(sample: int) -> int:
    """Standard G.711 μ-law encode of one PCM16 sample (matches the Rust codec)."""
    BIAS, CLIP = 0x84, 32635
    sign = 0x00
    if sample < 0:
        sample = -sample
        sign = 0x80
    if sample > CLIP:
        sample = CLIP
    sample += BIAS
    exponent = 7
    mask = 0x4000
    while (sample & mask) == 0 and exponent > 0:
        exponent -= 1
        mask >>= 1
    mantissa = (sample >> (exponent + 3)) & 0x0F
    return ~(sign | (exponent << 4) | mantissa) & 0xFF


def pcm_to_ulaw_frames(samples: list[int]) -> list[bytes]:
    frames = []
    for i in range(0, len(samples) - FRAME_SAMPLES + 1, FRAME_SAMPLES):
        chunk = samples[i : i + FRAME_SAMPLES]
        frames.append(bytes(linear_to_ulaw(s) for s in chunk))
    return frames


def load_wav_8k(path: str) -> list[int]:
    with wave.open(path, "rb") as w:
        assert w.getframerate() == 8000, "WAV must be 8000 Hz"
        assert w.getnchannels() == 1, "WAV must be mono"
        assert w.getsampwidth() == 2, "WAV must be PCM16"
        raw = w.readframes(w.getnframes())
    return [int.from_bytes(raw[i : i + 2], "little", signed=True) for i in range(0, len(raw), 2)]


def tone(seconds: float, freq: int = 300, amp: int = 8000) -> list[int]:
    """Loud sine — no words, but energetic enough to trip local VAD (transport/barge-in test)."""
    n = int(8000 * seconds)
    return [int(amp * math.sin(2 * math.pi * freq * i / 8000)) for i in range(n)]


async def reader(ws, stats: dict) -> None:
    try:
        async for raw in ws:
            msg = json.loads(raw)
            ev = msg.get("event")
            if ev == "media":
                stats["media_in"] += 1
            elif ev == "mark":
                stats["marks"].append(msg.get("mark", {}).get("name"))
                print(f"  <- mark: {msg.get('mark', {}).get('name')}")
            elif ev == "clear":
                stats["clear"] += 1
                print("  <- clear (barge-in flush)")
            else:
                print(f"  <- {ev}: {msg}")
    except websockets.ConnectionClosed:
        pass


async def stream_frames(ws, frames: list[bytes], sid: str) -> None:
    for f in frames:
        await ws.send(
            json.dumps(
                {
                    "event": "media",
                    "streamSid": sid,
                    "media": {"track": "inbound", "payload": base64.b64encode(f).decode()},
                }
            )
        )
        await asyncio.sleep(FRAME_MS / 1000)


async def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="ws://localhost:8080/media/twilio")
    ap.add_argument("--tenant", required=True)
    ap.add_argument("--agent", required=True)
    ap.add_argument("--wav", help="8kHz mono PCM16 WAV of caller speech (else a test tone)")
    ap.add_argument("--seconds", type=float, default=2.0, help="tone length when no --wav")
    ap.add_argument("--settle", type=float, default=4.0, help="seconds to let the greeting play")
    ap.add_argument("--barge", action="store_true", help="speak during greeting to test barge-in")
    args = ap.parse_args()

    sid = "MZfake0000000000000000000000000000"
    samples = load_wav_8k(args.wav) if args.wav else tone(args.seconds)
    frames = pcm_to_ulaw_frames(samples)
    stats = {"media_in": 0, "marks": [], "clear": 0}

    async with websockets.connect(args.url, max_size=None) as ws:
        rt = asyncio.create_task(reader(ws, stats))

        await ws.send(json.dumps({"event": "connected", "protocol": "Call", "version": "1.0.0"}))
        await ws.send(
            json.dumps(
                {
                    "event": "start",
                    "streamSid": sid,
                    "callSid": "CAfake",
                    "customParameters": {"tenant_id": args.tenant, "agent_id": args.agent},
                }
            )
        )
        print("-> start sent (tenant/agent). Listening for greeting...")

        if args.barge:
            # Speak immediately over the greeting → expect a `clear`.
            await asyncio.sleep(1.0)
            print(f"-> streaming {len(frames)} caller frames DURING greeting (barge-in test)")
            await stream_frames(ws, frames, sid)
        else:
            await asyncio.sleep(args.settle)
            greet = stats["media_in"]
            print(f"   greeting frames received: {greet}")
            print(f"-> streaming {len(frames)} caller frames")
            await stream_frames(ws, frames, sid)

        # Collect the response.
        await asyncio.sleep(5.0)
        await ws.send(json.dumps({"event": "stop", "streamSid": sid}))
        await asyncio.sleep(0.5)
        rt.cancel()

    print("\n=== summary ===")
    print(f"assistant audio frames received : {stats['media_in']}")
    print(f"marks                           : {stats['marks']}")
    print(f"clear (barge-in) events         : {stats['clear']}")
    if args.barge and stats["clear"] == 0:
        print("WARN: expected a `clear` from barge-in but saw none.")


if __name__ == "__main__":
    asyncio.run(main())
