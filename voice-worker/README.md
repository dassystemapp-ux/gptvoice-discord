# DAS Realtime Voice Worker (pilot scaffold)

This is a trusted-server adapter for the DAS Discord voice client. The OpenAI API key stays in the worker environment and is never sent to Discord clients.

## Current scope

- Opens a server-owned WebSocket connection to `gpt-realtime-2.1-mini`.
- Configures Saudi Arabic DAS instructions, PCM16 24 kHz, near-field noise reduction, server VAD and interruption.
- Relays audio append/commit/cancel events and model events.
- Enforces one concurrent session, a maximum three-minute session and a localhost-only listener by default.
- Requires a separate high-entropy worker token.

## Not yet connected to Discord audio

The existing Rust Discord receive path currently mixes all speaking members into one PCM stream. That path must be replaced with per-user SSRC isolation before sending any audio to this worker. Do not connect the current mixed stream to this worker. The current Discord desktop client also needs a bridge client and PCM resampling from 48 kHz stereo to 24 kHz mono.

## Run locally

Use Node.js 22.6+.

1. Copy `.env.example` to `.env` outside version control.
2. Set `OPENAI_API_KEY` and a long random `DAS_WORKER_TOKEN` in the process environment (not in source files).
3. Run `npm install`, then `npm test`, then `npm start`.
4. Keep the listener on localhost for desktop integration tests. Do not expose it publicly.
5. The OpenAI project must also have an appropriate spend limit configured. The worker's three-minute session limit is an additional guard, not a billing cap.

## Protocol

The local client connects to `ws://127.0.0.1:8787` with header `x-das-worker-token`.

Client messages:
- `{"type":"audio.append","audio":"<base64 PCM16 24 kHz mono>"}`
- `{"type":"audio.commit"}`
- `{"type":"response.cancel"}`
- `{"type":"session.close"}`

Server relays Realtime API events, including `response.output_audio.delta` and `input_audio_buffer.speech_started`.

## Security

Never commit `.env`, API keys, Discord tokens, browser profiles, or user audio. Do not deploy or expose this worker to the public internet without authentication, TLS, rate limits, per-user entitlements, spend reservation/reconciliation, and a privacy review.
