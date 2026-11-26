# DAS Voice — Approved Architecture and Test Plan

Status: implementation specification for the existing GPTVoice Discord app. Not deployed.

## User-approved product behavior
- Smart Economy is the default operating mode:
  1. Local VAD while the room is silent; do not send cloud audio while silent.
  2. Capture speech only from the selected session owner.
  3. Low-cost speech recognition.
  4. Higher-quality reasoning/answer model when needed.
  5. Natural Arabic speech synthesis.
- Direct Realtime is a selectable natural-voice mode for live conversation.
- The selected voice mode must be switchable from both the DAS website and Discord.
- When ChatGPT Voice allowance is exhausted, Gemini Live may be used as the fallback.
- Interruption must cancel the current generation and playback immediately.
- Do not bill or attribute audio to another room member.
- Target first audible response: 1.5–2.0 seconds after end-of-turn where practical; measure p50/p95 instead of promising it.
- Usage is metered by actual input/output consumption, with reservation/reconciliation and daily/monthly user caps.
- No cloud model invocation during silence.
- The Discord worker must be an always-on service for production; the current Windows desktop app is a development/test host only.

## Critical findings in current implementation
- The existing Discord app currently relays a dedicated ChatGPT browser session; it does not consume the OpenAI API balance for its voice conversation.
- The existing OpenAIProvider is a placeholder for text/file analysis, not a voice implementation.
- The current Discord README explicitly says multiple Discord members are mixed into one ChatGPT microphone stream. This violates the approved selected-speaker-only rule and prevents per-member attribution.
- The current desktop runtime exits the relay after five minutes of signal inactivity; this is not the approved speech-aware VAD behavior.
- Do not simply set DAS_AI_PROVIDER=openai: the adapter is not implemented and this switch would fail.

## Initial voice model choice
### Direct Realtime (natural mode)
- Model: gpt-realtime-2.1-mini for the first controlled API pilot.
- Transport: server-side WebSocket from the trusted voice worker.
- Audio: native speech-to-speech; no separate STT/TTS hops in this mode.
- Output voice: evaluate marin and cedar with the same Saudi Arabic test phrases; select only after listening tests. OpenAI documents these as recommended high-quality voices.
- Configure turn detection/VAD, interruption handling, input noise reduction, and short natural Saudi Arabic instructions.
- Keep response output concise and avoid verbose spoken paragraphs.
- Do not use the higher-cost full Realtime model for the initial $5 test.

### Smart Economy (default product mode)
- Local VAD with a short pre-roll and hangover; no cloud audio on silence.
- Per-speaker Discord stream isolation before any transcription.
- Low-cost STT; route the resulting text to the reasoning provider selected by DAS policy.
- Stream natural TTS audio back to Discord as soon as safe chunks are ready.
- Cache stable prompt/context prefixes and cap response length.
- Preserve Gemini Live as an explicit fallback when the user's ChatGPT Voice allowance is unavailable; do not silently switch billing providers.

## Discord speaker isolation requirement
- Bind each voice session to an explicit owner_user_id selected by the join action.
- Subscribe to the selected user's decoded Opus/PCM stream only; do not mix all SSRC streams into a single microphone input.
- Ignore other users' packets for model input. They must not update the owner's usage meter.
- On owner departure, stop the session or require explicit reassignment; do not automatically choose the loudest speaker.
- Support explicit handoff only through a visible/authorized action.
- Add tests with two simultaneous talkers, a non-owner talker, owner mute, owner disconnect, and SSRC changes.

## Cost guardrails for the $5 pilot
- Treat $5 as a hard experiment budget, not a recurring allowance.
- Use gpt-realtime-2.1-mini only in short, explicitly started test sessions.
- Enforce an application-side spend circuit breaker before each session/response; stop new model turns before the configured limit.
- Display the estimated/actual session cost and remaining test budget to the operator.
- Do not enable auto-reload, background sessions, or automatic retries that can create repeated billable turns.
- Reconcile the provider's actual usage after each session. A local estimate is not the source of truth.
- Do not hard-code the $5 balance as a provider-side cap; the actual spend limit must be set in the OpenAI project dashboard as well.

## Voice quality evaluation
Run a fixed test script with the same room, microphone, speaker, network and phrases:
1. Saudi greeting and natural reply.
2. Short question in Saudi colloquial Arabic.
3. Longer utterance with a pause mid-sentence.
4. User interruption during the assistant response.
5. Background noise while the owner is silent.
6. A second member speaks while the owner is silent.
7. Two members speak simultaneously.
8. Owner disconnects and reconnects.
9. A long answer that should be shortened naturally.
10. Fallback activation and recovery.

Record:
- First audible response latency p50/p95 (target 1.5–2 seconds).
- Speech recognition accuracy on the fixed Arabic phrases.
- Pronunciation, Saudi accent naturalness, prosody and pacing, clipping and robotic artifacts.
- Barge-in cancellation time.
- False activation during silence/background noise.
- Dropped/late audio frames and reconnect success.
- Input/output token usage and actual billed USD per minute/session.
- Whether any non-owner audio reached the model (must be zero).

Pass criteria:
- No non-owner audio is forwarded.
- Silence does not trigger cloud usage.
- Interruption cancels promptly and does not continue stale speech.
- No clipped or missing initial/ending audio.
- Arabic is intelligible and conversational; compare voice samples blind before choosing voice.
- Cost remains inside the configured test cap.
- Latency is measured and reported; do not mark pass solely from a local code test.

## Rollout sequence
1. Keep the current ChatGPT browser relay intact as a separate legacy path until the new API path passes tests.
2. Implement the Direct Realtime pilot behind a disabled-by-default feature flag.
3. Implement per-speaker isolation before enabling any API audio forwarding.
4. Add the Smart Economy pipeline and voice-mode setting.
5. Add spend reservation/reconciliation and project-level spend controls.
6. Test on two accounts / two Discord members in a private test room.
7. Only after all gates pass, prepare a release candidate for user review.
8. Production deployment and paid ongoing usage require separate explicit approval.

## API pricing reference at implementation time
OpenAI's published model page lists gpt-realtime-2.1-mini audio input at $10 / 1M audio tokens and audio output at $20 / 1M audio tokens; text input/output are billed separately. Actual session costs vary with audio duration, generated speech, text context, and cached tokens. Never promise a fixed number of minutes from the $5 credit.