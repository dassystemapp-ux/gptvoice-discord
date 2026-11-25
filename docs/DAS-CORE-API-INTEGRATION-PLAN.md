# GPTVoice ↔ DAS Core API Integration Plan

Status: planning baseline only  
Target branch: `feat/das-core-api-foundation`  
Environment: experimental/staging only

## 1. Current application boundary
GPTVoice currently provides a Windows desktop Discord voice bridge and an existing Gemini Live path. Its current usage tracking is process-local and is not a durable billing ledger. Its current audio relay can mix Discord speakers into one stream, which is unsuitable for per-user billing. The existing app must remain functional while the DAS integration is introduced behind a feature flag.

## 2. Target responsibility split
- **Discord worker:** join/leave voice, capture the explicitly linked speaker, local VAD, audio buffering, STT/TTS transport, interruption handling, and session lifecycle.
- **DAS Core API:** identity resolution, entitlement checks, credit reservation, model routing policy, provider credentials, usage reconciliation, durable session/turn records, voice preferences, and audit.
- **Firebase:** source of user identity (verified ID tokens) and application data.
- **Provider adapters:** STT, reasoning LLM, TTS, and optional direct realtime audio. Adapters report provider usage; they do not decide customer prices.

The desktop UI may remain an administrator/test client. A production Discord worker must run independently on an always-on server; closing a desktop window must not terminate the hosted worker.

## 3. Voice modes
### Economy Hybrid
1. Local VAD observes audio frames; silence causes no cloud request.
2. Capture a small pre-roll and isolate the authenticated speaker.
3. Stream to low-cost STT; expose partial transcript only where useful.
4. End utterance after configurable silence; validate transcript/confidence.
5. Reserve credits through Core API before premium reasoning.
6. Send transcript and bounded context to premium text model.
7. Stream natural TTS in chunks; cancel immediately on interruption.
8. Reconcile actual STT, LLM, and TTS provider usage with Core API.

### Direct Realtime
- Optional audio-to-audio provider path for more natural prosody and lower orchestration complexity.
- Meter input and output audio separately, plus any billed tools/model usage.
- Require a separate cost/quality benchmark and entitlement gate.

## 4. Speaker and billing isolation
- Never attribute a mixed guild audio stream to the loudest or currently active speaker for billing.
- Map Discord user ID to Firebase UID only through a verified Core API link.
- If speaker identity is ambiguous or the member is not linked, do not run a paid turn.
- Use `session_id`, unique `turn_id`, and idempotency keys to avoid duplicate reservations/settlements.
- Apply per-user concurrency and rate limits.

## 5. Proposed Core API contract
All routes are versioned and require a verified Firebase ID token or service-to-service worker credential.

- `POST /v1/voice/sessions` — create a session for a linked Discord member.
- `POST /v1/voice/turns/reserve` — reserve estimated maximum credits for a turn.
- `POST /v1/voice/turns/{turnId}/settle` — submit provider usage and settle actual cost.
- `POST /v1/voice/turns/{turnId}/cancel` — cancel and release unused reservation.
- `GET /v1/voice/preferences` — retrieve preferred voice/mode.
- `PUT /v1/voice/preferences` — update default voice/mode.
- `GET /v1/voice/entitlement` — check plan and remaining credits.

The worker must not receive provider API secrets or direct write access to credit balances.

## 6. Discord UX
- Add a `/das link` or equivalent secure account-link flow that directs the member to the DAS website OAuth linking page.
- Add `/voice` command or panel for voice and mode selection.
- Voice selection updates the same preference record used by the website; session override is temporary and clearly indicated.
- Show a concise response when credits are insufficient, account is unlinked, or provider is unavailable.
- Preserve explicit join/leave behavior; do not start a paid session merely because a member enters a channel.

## 7. Reliability and quality
- Bounded pre-roll, utterance timeout, transcript confidence thresholds, retries with backoff, circuit breakers, and provider fallback policy.
- Interruption cancels in-flight LLM/TTS work and prevents stale audio from continuing.
- Track end-of-utterance-to-first-audio latency, STT confidence/WER, failed-turn rate, dropped audio, and per-provider usage.
- Initial latency target is a test objective, not a guarantee; benchmark Arabic and Saudi dialect before choosing providers.
- Never promise zero recognition errors. Offer repeat/clarify behavior for low-confidence transcripts.

## 8. Implementation gates
- **G0:** baseline tests and feature-flag isolation.
- **G1:** typed Core API client + mocked contract tests.
- **G2:** verified account-link resolution and per-speaker isolation.
- **G3:** credit reserve/settle/cancel with mocked usage fixtures.
- **G4:** VAD/STT/LLM/TTS pipeline behind disabled-by-default flag.
- **G5:** voice preference sync across website and Discord.
- **G6:** staging soak tests and release readiness review.

No production worker deployment, provider API usage, or paid infrastructure activation is authorized by this plan.

## 9. Security and data handling
- Secrets only in server-side secret storage; never in source, frontend bundles, logs, or Discord messages.
- Avoid storing raw audio. Usage records should contain metering metadata and only the minimum diagnostic data.
- Worker authentication uses short-lived service credentials and rotation.
- Enforce request signing or authenticated service identity, replay protection, and strict per-user ownership checks.
- The worker cannot grant credits, mark Salla orders paid, or override subscription state.

## 10. Compatibility requirements
- Keep the current GPTVoice desktop path operational while new DAS Core API behavior is introduced behind a feature flag.
- Do not force OpenAI or another billable provider as primary before the provider budget and model selection are approved.
- Existing Gemini provider behavior and tests remain regression gates.


## 11. Shared DAS identity and independent product engines

GPTVoice is one capability inside the unified DAS account, not a separate customer identity system. The Firebase UID remains canonical. Discord identity is linked to that account through the Core API. Voice preferences and entitlements are shared account capabilities.

DAS products retain independent experience engines under one account and dashboard. The Discord worker may surface product-aware learning interactions only after Core API confirms the user's identity and entitlement. Voice usage Credits remain a separate billing ledger from product purchase entitlements.

