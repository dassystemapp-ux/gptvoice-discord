# DAS Ecosystem — Current Audit, Target Architecture & Migration Plan

Version: 1.0  
Assessment date: 2026-11-24  
Scope: current Windows/Tauri + Discord learning experience

## A. Current System Audit

### Existing runtime
- Native desktop shell: Tauri 2, Rust 2021.
- Discord gateway and interactions: Serenity 0.12; voice relay: Songbird 0.6.
- Voice assistant currently uses a dedicated local browser integration for ChatGPT voice.
- Study assistance is called from the Discord message handler.
- AI provider before this change: direct Gemini REST call from the study module.
- Gemini key source: `%USERPROFILE%\Downloads\API GEMINI.txt`, with a local config fallback.
- Study progress: JSON file `study-progress.json` under the GPTVoice app data directory.
- UI: local HTML/CSS/JavaScript in `tauri-ui`.
- Study attachments: Discord CDN download, bounded in-memory payload, image/PDF inline data; PPT/PPTX converted through installed Microsoft PowerPoint to PDF.

### Features present in source
- Student flows for abilities, tahseeli, university, quizzes, and study timer.
- Focus/break cycles, pause/resume/stop, user/channel-scoped in-memory active timers.
- Progress totals for completed focus sessions, focus/break minutes, points and daily points cap.
- Per-user AI opt-out, one-time privacy notice, usage cooldown and daily/server caps.
- Attachment limits, MIME/extension allow-list, Discord CDN host allow-list, PowerPoint conversion.
- Slash commands `/study` and `/studyboard`.

### Gaps and risks confirmed in source
- No central DAS Core service/API exists in this desktop project yet.
- No Firebase SDK or Firebase-backed data repository is wired into the current Rust runtime.
- Study progress is local JSON, not a multi-device canonical database.
- Quiz state is held in runtime memory; completed attempts and answer keys are not a durable quiz ledger.
- Active timers are held in memory and are not restored after a process restart.
- Attachments are passed to the AI provider as inline data; there is no durable secure file store, extracted-text index, or file-analysis record.
- Achievement points do not yet have a durable idempotent event ledger.
- Usage tracking added in the provider layer is process-local telemetry; it is not yet a billing-grade persistent usage ledger.
- Website, Salla, OAuth account linking, entitlements, and event outbox are future integrations, not active integrations.
- Discord Message Content Intent is requested by code and must also be enabled in the Discord Developer Portal.
- PowerPoint attachments require Microsoft PowerPoint/Office COM availability on the machine running the bot.

The current application binary is not replaced by this source update. Deployment remains blocked pending a successful Windows MSVC build and verification.

## B. Target Architecture

### Runtime boundaries
1. **DAS Core / Domain** — identity, user profile, learning state, achievements, entitlements, domain events. Platform-neutral; no Discord SDK types.
2. **AI Intelligence** — AIProvider contract, ModelRouter, PromptManager, ContextManager, provider configuration, usage accounting and evaluation.
3. **Learning Engine** — Focus Engine, Quiz Engine, File Intelligence and Achievement Engine.
4. **Persistence** — repository interfaces first; current JSON store remains the migration adapter until a tested repository is ready.
5. **Adapters** — Discord adapter today; Website/Core API, Salla webhook and OAuth linking later.
6. **Presentation** — Discord interactions and Tauri desktop panel; both consume domain services rather than owning domain rules.

### AI Provider Layer delivered in this revision
- `AIProvider` defines the shared generation contract.
- `GeminiProvider` implements the active Gemini REST integration with a bounded timeout and retry.
- `OpenAIProvider` is an explicit disabled scaffold; it returns a clear not-configured error and does not pretend to work.
- `ModelRouter` chooses the provider from `DAS_AI_PROVIDER` (default: Gemini), checks provider readiness and accounts for requests/results.
- `PromptManager` centralizes DAS identity, student-help and quiz prompts.
- `ContextManager` centralizes Unicode-safe text bounds.
- `UsageTracker` records process-local request, result and token metrics.

All current study generation and analysis calls now route through `ModelRouter`. Replacing Gemini later should require implementing/configuring the OpenAI adapter and testing it, not rewriting the timer, points, quiz flow or Discord event handler.

### Migration target
- Keep Firebase as the preferred candidate if it is already part of the wider DAS ecosystem; verify its live schema and security rules before moving any records.
- Introduce repository contracts for users, progress, focus sessions, quizzes, files, achievements, usage and events.
- Use server-authoritative timestamps and idempotency keys for completed sessions, quiz submissions, points and entitlement changes.
- Add a transactional points ledger and outbox before synchronizing achievements to the website.
- Add account linking through OAuth2; never treat Discord display names or email as stable primary identifiers.
- Process Salla payment changes only from verified, idempotent webhooks.

## C. Implementation Roadmap

### P0 — Discord stability and provider isolation
1. Preserve existing voice and desktop behavior.
2. Compile the current Windows target and resolve all compiler errors.
3. Verify Discord token/guild configuration and Message Content Intent in the Developer Portal.
4. Finish provider abstraction and regression tests.
5. Verify study message routing, file limits, quiz generation, focus/break cycling, pause/resume/stop and progress persistence.
6. Release only after an isolated test build passes; then stage a controlled replacement with rollback.

### P1 — Durable learning data
1. Define versioned schemas and repository interfaces.
2. Migrate local progress JSON with a one-time backup, validation, idempotent import and reconciliation report.
3. Persist focus sessions and active timer state.
4. Persist quiz definitions, attempts, answer keys and grading results.
5. Add an append-only points/achievement ledger and idempotency tests.
6. Add user-controlled personal memory, explicit consent, read/edit/delete operations and retention rules.
7. Persist usage and operational failures with privacy-minimized telemetry.

### P2 — Ecosystem integrations
1. DAS Core API and authenticated student progress endpoints.
2. Website account identity and Discord OAuth2 linking.
3. Entitlements independent from Discord roles.
4. Salla webhook verification, refund/cancellation handling and idempotent subscription updates.
5. OpenAI provider implementation and evaluation suite.
6. Admin controls, monitoring, model/prompt versioning and quality regression datasets.

Do not start P1/P2 integrations by creating placeholder endpoints that appear operational.

## D. Migration & Risk Plan

### Data safety
- Keep the existing config and progress JSON untouched during build validation.
- Create a timestamped source backup before each code migration.
- Before persistence migration, make a byte-for-byte backup of `config.json` and `study-progress.json`; do not log tokens or API keys.
- Version all imported records. Reject malformed records into a quarantine report rather than silently discarding them.
- Compare record counts and per-user totals before/after migration.
- Keep the old store readable until parity checks pass and rollback is approved.

### Release safety
- Build to a separate staging output; do not overwrite the installed executable in place.
- Preserve the existing installation and process until the staging build starts successfully.
- Run source-format checks, JavaScript syntax checks, automated tests, Rust unit tests, and a Discord staging smoke test.
- Verify voice join/leave, slash commands, study messages, opt-out, attachments, timer transitions, and progress records.
- Roll back by restoring the prior signed/known-good executable and its matching configuration/data backup.
- No automatic role grants are part of this migration. Existing roles and permissions are left unchanged.

### Current release gate
- Source backup created: `backup-before-ai-layer-20261124-065354`.
- Rust formatting and Node checks passed.
- Existing Node test passed (1/1).
- Rust type-check/unit tests and Windows package build remain blocked because the machine does not currently expose the MSVC linker (`link.exe`) or Visual C++ Build Tools.
- The current Windows session is not elevated and the user account is not a local administrator. Do not bypass UAC or attempt an unattended privileged installation.
- The old installed executable remains untouched. The current source changes are not yet deployed.

## External configuration checklist
- In Discord Developer Portal, open the DAS Bot application > Bot > Privileged Gateway Intents, enable **Message Content Intent**, and save.
- The Rust gateway already requests `GatewayIntents::MESSAGE_CONTENT`; both the portal toggle and source intent are required.
- The bot needs access to the configured guild and study channels, including View Channel, Send Messages, Read Message History, Embed Links, Attach Files where relevant, and Use Application Commands.
- Confirm Microsoft PowerPoint is installed if PPT/PPTX upload conversion is expected.

### Windows build prerequisites — prepared install profile
The Visual Studio component profile is saved at `docs/vs-buildtools.vsconfig`.

From an **Administrator PowerShell** opened in the project root, run:

```powershell
$config = (Resolve-Path .\docs\vs-buildtools.vsconfig).Path
winget install --exact --id Microsoft.VisualStudio.BuildTools --override "--passive --wait --norestart --config `"$config`""
```

This profile selects the Desktop development with C++ workload, the x64/x86 MSVC toolset, and the Windows 11 SDK. The installer needs Windows elevation. After it completes, open a fresh PowerShell and verify `link.exe` is discoverable or run the build from the Visual Studio x64 Native Tools command prompt.

After installation, open **x64 Native Tools Command Prompt for Visual Studio** so the MSVC linker and SDK paths are loaded. From the project root, run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-and-build.ps1
```

The script performs formatting, locked metadata, JavaScript syntax and Node regression checks before creating an NSIS installer. It does not install the package or replace the running app.