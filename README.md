# Voice Elf

Local-first realtime speech translator with a Rust/Axum WebSocket server and a SvelteKit client.

## Architecture

```text
Browser AudioWorklet (native-rate mono Float32 capture)
  -> Web Worker + Lele/Silero Rust WASM (16 kHz PCM16, 32 ms frames, 512 ms/1 s pre-roll)
  -> start(tc_id + languages) / speech PCM / end(tc_id + is_silent_vad) over WebSocket /ws
  -> server boundary, frame, rate, and duration validation
  -> primary Qwen ASR live stream + post-segment parallel ASR consensus
  -> translation queue: Qwen3 local LLM
  -> translated text returned without waiting for TTS
  -> pluggable server TTS: MOSS-TTS-Nano ONNX stream, then Kokoro/Supertonic fallback
  -> room broadcast: transcript/translation events + source/translated media URLs + typed PCM16 audio chunks
  -> every connected room member receives the same ordered realtime stream
```

Each utterance reports five timestamps: `t0` speech start, `t1` VAD endpoint, `t2` STT complete, `t3` translation complete, and `t4` TTS complete.

Recording is continuous until the client sends `flush`. The browser's Rust/WASM VAD gives every sentence a UUID `tc_id`, closes it after roughly three seconds of silence or the configured 20-second hard limit, and uploads only speech PCM. Operators can enable enhanced voice filtering in Settings; this mode calibrates the first second, locks out sustained low-frequency hum, and buffers a candidate until six voice frames are confirmed. The primary ASR starts with the accepted segment and streams partial text while the user is speaking. At `end`, the server saves the WAV, runs any additional ASR channels in parallel, selects a consensus result, and queues translation. Silent, sub-200 ms, and segments without enough confirmed human speech are discarded before persistence and ASR. Automatic and manual translated-audio playback temporarily suppress microphone VAD input to prevent speaker feedback from becoming a new utterance. The server treats browser boundaries as untrusted hints and enforces the authenticated room lifecycle, exact frame format, near-realtime ingress rate, and maximum duration.

Qwen ASR remains server-side. Its model is too large and device-sensitive for the default browser path, and keeping it on the server protects model files and preserves authoritative persisted results. See [the browser VAD architecture](docs/browser-vad-architecture.md) for the deployment and security decisions.

Server TTS is a chain of `TtsEngine` implementations. MOSS-TTS-Nano ONNX is attempted first for every supported target language and forwards its 48 kHz stereo PCM chunks while synthesis is still running. If it fails before emitting audio, Chinese falls back to lazy-loaded Kokoro v1.1 INT8 and `en/ja/ko/fr/de/es/it/pt/ru` fall back to Supertonic 3 INT8. Once an engine emits its first chunk, the chain is committed to that engine so a later failure cannot replay the sentence from the beginning. Translation text remains independent of TTS and is sent immediately.

The synthesis pipeline depends only on the business-level `TtsEngine` trait. Provider implementations live under `server/src/backends/tts/`; `FallbackTtsEngine` owns routing and failure policy, while persistence and WebSocket delivery remain in the pipeline. The audio events declare engine, codec, sample rate, and channel count, so future MOSS Realtime/v1.5 and Opus delivery adapters can be added without changing translation or storage ownership.

Rooms define `max_utterance_seconds` with a default of 20 seconds and an allowed range of 5 through 20 seconds. Continuous speech is force-segmented at that duration and immediately continues in a new utterance record.

The backend pipeline is organized by runtime responsibility under `server/src/pipeline/`: `session` validates browser speech segments, while `transcription`, `translation`, and `synthesis` own their stage workers. PostgreSQL utterance persistence is isolated in `server/src/storage/history.rs`. The room owner is the only WebSocket publisher; authenticated room members are read-only realtime subscribers. The in-process room hub fans out ordered text events, protected source/translated media URLs, and translated PCM chunks. Clients reconcile persisted history after each subscription or reconnect to close the connection gap.

## Run the demo

Prepare the server TTS models once. The script downloads pinned Kokoro and Supertonic INT8 archives into the ignored `.local` directory:

```bash
./scripts/setup-server-tts.sh
./scripts/moss-nano-tts.sh setup
./scripts/moss-nano-tts.sh start
./scripts/moss-nano-tts.sh doctor
cd web
npm install
npm run build
cd ..
VOICE_ELF_BACKEND=demo cargo run --bin voice-elf-server
```

The demo backend is opt-in and returns placeholder transcripts only. Normal startup defaults to the `local` backend and fails fast when its ASR model is unavailable, preventing placeholder text from being mistaken for real recognition.

System administrators can switch the effective ASR provider from `/admin` without restarting the service. The database stores only stable provider IDs; model paths and provider credentials remain server-side environment configuration. A change applies when a new room audio pipeline starts, while an already running pipeline keeps its provider snapshot until the room disconnects. In authorization-bus mode, administrators can assign a tenant override or leave it inheriting the system default. Tenant instances receive the resolved provider ID in their regular authorization check and verify that the provider is configured locally.

After the initial model setup, the complete development stack can be managed from one terminal. `make dev` verifies PostgreSQL, the project-local Python/MOSS environment, Web dependencies, Rust VAD, and the server build before starting MOSS-TTS-Nano, the Rust server, and Vite. The processes run in project-specific detached sessions, so they remain available after the command exits without requiring a global service installation. Stop only removes processes owned by this stack and waits for ports `18083`, `3001`, and `5173` to be released; an unrelated process occupying one of those ports is reported and left untouched.

```bash
make dev             # start everything
make dev-status      # inspect processes and dependency health
make dev-logs        # print recent component logs
make dev-restart     # stop and start again
make dev-stop        # stop managed services and release their ports

# The same controls are available from web/:
npm run stack:start
npm run stack:status
npm run stack:stop
```

Open <http://127.0.0.1:3001>. Microphone access works on localhost in current browsers.

With `VOICE_ELF_BIND=0.0.0.0:3001`, other devices on the same LAN can open `http://<server-lan-ip>:3001` for account, room, history, and audio playback testing. Browser microphone capture requires a secure context: `localhost` works over HTTP, while a LAN IP normally requires a trusted HTTPS certificate. The client reports this explicitly instead of failing silently.

For frontend development, run `npm run dev` from `web/`; Vite listens on all interfaces and proxies `/ws`, `/api`, and `/media` to port 3001. LAN clients can use `http://<server-lan-ip>:5173` with HMR. Run `npm run deploy:watch` to compile the Rust VAD once and continuously rebuild `web/dist`; the Axum service on port 3001 serves each new frontend build without a backend restart.

For temporary Internet testing over trusted HTTPS, install `cloudflared` and use the local tunnel manager. It starts the tunnel in the background, waits for the public health check, and prints the resulting address:

```bash
cd web
npm run deploy:public          # Axum production site
npm run deploy:public:dev      # Vite/HMR site
npm run deploy:public:status
npm run deploy:public:stop

# The underlying commands can also be run from the repository root:
cd ..
./scripts/public-tunnel.sh start production  # Axum production site
./scripts/public-tunnel.sh start dev         # Vite/HMR site
./scripts/public-tunnel.sh status all
./scripts/public-tunnel.sh stop all
```

The equivalent Make targets are `make web-public`, `make web-dev-public`, `make web-public-status`, and `make web-public-stop`. Runtime PIDs, URLs, and logs are stored under `.local/run/public-tunnel/`. The printed `trycloudflare.com` address supports secure microphone capture and WebSocket traffic. Quick Tunnel addresses are temporary and change whenever the tunnel process restarts; use an authenticated named tunnel and access policy before treating this as a permanent deployment.

## Local models

On macOS, the setup script builds the two server inference binaries with Apple Accelerate and downloads the Qwen3 ASR and translation weights into the ignored `.local/` directory:

```bash
./scripts/setup-local-models.sh
cp .env.example .env
# Set VOICE_ELF_BACKEND=local and point the model variables at .local/.
cargo run --release --bin voice-elf-server
```

The server loads `.env` automatically. Relative filesystem paths in the server configuration are resolved from the workspace root, so the server can be launched from the root or `server/` directory without changing model paths. The default installed configuration uses the open-source Qwen3-ASR-0.6B model through the MIT-licensed `qwen_asr` C runtime for offline streaming recognition, and `llama-completion` with Qwen3-0.6B for offline translation. It covers `zh/en/ja/ko/fr/de/es/it/pt/ru` and emits stable source tokens while audio is still arriving. You can instead configure an OpenAI-compatible translation endpoint with `LOCAL_LLM_BASE_URL` and `LOCAL_LLM_MODEL`.

MOSS-TTS-Nano runs as its official Python 3.12 ONNX service on `127.0.0.1:18083`; `scripts/moss-nano-tts.sh` pins the tested upstream revision and manages setup and runtime state. Setup bootstraps pinned `uv 0.11.32`, managed CPython 3.12.13, the locked Python dependencies, virtual environment, source, dependency cache, and model cache below the ignored project `.local/` directory. On macOS ARM, it also builds the OpenFST dependency for WeTextProcessing inside `.local/openfst/`; it does not install a global Homebrew package. No system Python or shell profile is modified, so a new checkout can recreate the same isolated runtime with `setup`; use `doctor` to verify it. Configure its URL, CPU threads, timeout, and the application voice-to-demo mapping with `TTS_MOSS_NANO_*`. Set `TTS_MOSS_NANO_ENABLED=false` to use only the stable Kokoro/Supertonic engines. `TTS_KOKORO_MODEL_DIR`, `TTS_SUPERTONIC_MODEL_DIR`, and `TTS_THREADS` override fallback defaults.

The ASR adapter starts at the VAD speech-start edge and receives PCM continuously while the speaker is still talking. Its low-latency defaults can be tuned with `QWEN_ASR_STREAM_UNFIXED_CHUNKS`, `QWEN_ASR_STREAM_MAX_NEW_TOKENS`, and `QWEN_ASR_ENCODER_WINDOW_SECONDS`; the adapter invokes:

```text
qwen_asr -d <model-dir> --stdin --stream \
  --stream-unfixed-chunks 0 --stream-max-new-tokens 12 \
  --enc-window-sec 4 [--language <language>]
```

The server validates both fallback TTS model directories at startup; MOSS availability is checked per request so an unavailable optional service cannot prevent startup. Local mode also validates ASR and translation paths. Runtime inference failures are returned to the client as record-scoped recoverable errors, so the WebSocket session can keep listening.

Qwen's stable token callback is forwarded immediately as real `transcript_delta` events. After VAD closes the sentence and ASR produces its final text, `llama-completion` stdout is filtered and forwarded token-by-token as real `translation_delta` events. Translation intentionally starts from the finalized sentence rather than repeatedly translating unstable ASR prefixes.

On the tested 2019 Intel Mac, a warm 6.5-second continuous sample produced its first source delta at about 3.9 seconds and completed ASR at about 8.8 seconds. A cold model can add roughly five seconds. CPU inference can therefore still lag behind live capture even though audio transport and event delivery are genuinely streaming. Server TTS runs in its own queue stage, so later ASR and translation tasks can continue while audio is synthesized.

Qwen streaming uses two-second audio chunks. The configured 12-token decode budget bounds each streaming step while retaining enough capacity for normal Chinese and English speech rates. If live recognition fails or returns no text, the adapter retries the preserved utterance PCM with faster `--silent` batch recognition before reporting an error. In automatic-language rooms, an empty auto-detection result gets a bounded Chinese-then-English retry so short human utterances are not discarded; rooms that primarily use another supported language should select it explicitly.

### Optional accurate transcription

[MOSS-Transcribe-Diarize](https://github.com/OpenMOSS/MOSS-Transcribe-Diarize) can run beside Qwen as an optional accurate-transcription engine. Qwen remains the realtime source for translation and TTS; after each VAD utterance is saved, the server submits its WAV to MOSS on a separate bounded queue. MOSS failures therefore never replace or delay the realtime transcript. Completed text, timestamps, and speaker segments are stored as a separate refinement version and published through `transcript_refinement` WebSocket events.

Start an OpenAI-compatible MOSS service, for example with SGLang Omni:

```bash
sgl-omni serve \
  --model-path OpenMOSS-Team/MOSS-Transcribe-Diarize \
  --port 8000
```

Then enable the component in `.env`:

```dotenv
MOSS_TRANSCRIBE_ENABLED=true
MOSS_TRANSCRIBE_BASE_URL=http://127.0.0.1:8000/v1
MOSS_TRANSCRIBE_MODEL=OpenMOSS-Team/MOSS-Transcribe-Diarize
MOSS_TRANSCRIBE_TIMEOUT_SECONDS=300
MOSS_TRANSCRIBE_MAX_NEW_TOKENS=5120
```

The current scheduler refines each stored utterance independently. Room-member identities from the WebSocket session remain authoritative; MOSS speaker labels are retained as model metadata and do not overwrite known users.

## PostgreSQL history

Set `DATABASE_URL` to enable asynchronous persistence through Diesel and its bb8 connection pool. On startup, the server connects to PostgreSQL's `postgres` maintenance database, creates the configured database when it is missing, and then runs all pending embedded Diesel migrations before accepting traffic. The configured PostgreSQL role therefore needs `CREATEDB` only when automatic database creation is required.

Each completed utterance stores a mono PCM16 source WAV and a translated PCM16 WAV with the channel count produced by its TTS engine. The source WAV and processing record are persisted before ASR. Transcript, translation, translated WAV URL, and final TTS latency are updated as their stages finish. A failed synthesis therefore does not discard the source recording or text results.

Every VAD utterance is now persisted before ASR with its user, room, session, utterance ID, source WAV, and processing status. Empty ASR results become a record-scoped `recognition_failed` event rather than a global pipeline error, so the client keeps the failed row and its playable source audio for diagnosis.

```bash
createdb voice_elf
echo 'DATABASE_URL=postgres://localhost/voice_elf' >> .env
```

Accounts use Argon2 password hashes and seven-day HTTP-only session cookies. The setup wizard creates the first active system administrator; later registrations remain pending until an administrator verifies them. Administrators can approve, suspend, restore, or promote accounts. Suspending an account revokes its HTTP sessions and disconnects its active room connections. The meeting directory is restricted at the database query layer: users only see rooms they created or previously joined. A meeting creator is its administrator and can update the room, manage member speaking permissions, and remove the meeting from active views. Authenticated users can still join an active room through its direct meeting link; after joining, they can browse its records and subscribe to live transcripts, translations, protected source/translated audio URLs, and translated audio playback. `users`, `auth_sessions`, `rooms`, and `room_members` hold this authorization state.

On a new database, the browser is redirected to `/setup` before login or business routes are available. The four-step setup checks PostgreSQL and instance authorization, records the system and organization names plus public URL, and atomically creates the first active system administrator. Set `VOICE_ELF_SETUP_TOKEN` to a random value of at least 16 characters before first start; when it is omitted, the server prints a generated `vesetup_...` token in the startup log. This prevents another network user from claiming an uninitialized deployment. Existing databases with users are automatically marked initialized by migration and do not show the wizard.

Self-hosted tenant licensing uses a separate control plane and is disabled by default. Set `VOICE_ELF_AUTHORITY_MODE=bus` on the central authorization service, or set it to `tenant` together with `VOICE_ELF_AUTHORITY_URL`, `VOICE_ELF_AUTHORITY_CLIENT_ID`, and `VOICE_ELF_AUTHORITY_CLIENT_SECRET` on a tenant backend. Tenant users and business data remain in that tenant's PostgreSQL and media directory; only deployment credentials, entitlement state, expiry, and heartbeat metadata reach the bus. See [`docs/multi-tenant-plan.md`](docs/multi-tenant-plan.md) for the protocol, failure policy, and deployment sequence.

Room and private voice-reference removal uses `deleted_at` soft deletion. Historical database rows and meeting audio remain retained for recovery and audit instead of being physically removed. Authentication-session revocation remains a physical deletion because an invalidated credential must not stay usable.

Each room-owner publisher connection creates a row in `voice_sessions`; read-only member subscriptions do not create inference sessions. Completed utterances are stored in `voice_utterances` with their account/room ownership, source and translated text, language pair, audio duration, all `t0` through `t4` latency measurements, and the two audio file paths and URLs. Audio samples remain in WAV files rather than PostgreSQL binary columns. Media URLs require a logged-in owner or room member.

```sql
SELECT created_at, source_text, translated_text, total_ms
FROM voice_utterances
ORDER BY created_at DESC
LIMIT 20;
```

## Protocol

Account and room endpoints:

```text
POST   /api/auth/register
POST   /api/auth/login
DELETE /api/auth/logout
GET    /api/auth/me
GET    /api/admin/overview
GET    /api/admin/asr
PATCH  /api/admin/asr
GET    /api/admin/users?q=&status=&role=&sort=&order=&page=&page_size=
PATCH  /api/admin/users/{user_id}
GET    /api/admin/rooms?q=&status=&sort=&order=&page=&page_size=
PATCH  /api/admin/rooms/{room_id}
GET    /api/admin/rooms/{room_id}/inspect
GET    /api/instance/authorization
POST   /api/authority/oauth/token
POST   /api/authority/entitlements/check
GET    /api/admin/authority/tenants?q=&status=&sort=&order=&page=&page_size=
POST   /api/admin/authority/tenants
PATCH  /api/admin/authority/tenants/{tenant_id}
PATCH  /api/admin/authority/tenants/{tenant_id}/asr
GET    /api/admin/authority/tenants/{tenant_id}/instances
POST   /api/admin/authority/tenants/{tenant_id}/instances
PATCH  /api/admin/authority/instances/{instance_id}
POST   /api/admin/authority/instances/{instance_id}/rotate-secret
GET    /api/rooms?q=room-name
POST   /api/rooms
GET    /api/rooms/{room_id}?q=transcript-text
PATCH  /api/rooms/{room_id}
DELETE /api/rooms/{room_id}
POST   /api/rooms/{room_id}/join
PATCH  /api/rooms/{room_id}/members/{user_id}
```

System-management endpoints require an active system administrator session. Account status changes and meeting status changes are enforced server-side. The inspection endpoint reads meeting members and history without joining the room or opening a WebSocket, so it does not alter membership or online presence. The proposed client-transparent tenant isolation design is documented in [`docs/multi-tenant-plan.md`](docs/multi-tenant-plan.md).

Room update/delete and speaking-permission management are owner-only. Every joined, unmuted member can publish Web VAD boundaries and 16 kHz PCM16 frames through the room WebSocket. The server aligns 512-sample frames from active speakers, mixes them into one room-level ASR/translation/TTS pipeline, and broadcasts member presence, speaking state, speaker attribution, text, and audio to all participants. The endpoint requires the session cookie and an authorized room ID: `ws://localhost:3001/ws?room_id={room_id}`. The first server event confirms whether the current member can publish:

```json
{"type":"room_subscribed","room_id":"...","can_publish":true,"user_id":"...","backend":"local"}
```

Client text frames:

```json
{"type":"configure","source_language":"auto","target_language":"zh","voice":"ryan","max_utterance_seconds":20}
{"type":"start","tc_id":"550e8400-e29b-41d4-a716-446655440000","vad":{"engine":"silero-v6.2-lele","sample_rate":16000,"frame_samples":512,"pre_roll_samples":8192},"source_language":"auto","target_language":"zh","voice":"ryan","max_utterance_seconds":20}
{"type":"end","tc_id":"550e8400-e29b-41d4-a716-446655440000","is_silent_vad":false,"vad":{"reason":"silence","sample_count":24576}}
{"type":"flush"}
```

Every unmuted client sends fixed 512-sample little-endian PCM16 frames at 16 kHz, mono, only between its matching `start` and `end` messages. `start` declares the VAD engine/frame configuration; `end` carries the VAD boundary reason and sent sample count. The room mixer drains one frame per active speaker every 32 ms, averages simultaneous samples to avoid clipping, closes on aggregate silence, and enforces the room's 20-second maximum segment duration. Server text frames carry `room_members`, `utterance_speakers`, state, incremental `transcript_delta` and `translation_delta` updates, final text, media URLs, audio metadata, latency events, and `utterance_discarded` for silent placeholders. Source media is returned after persistence; after asynchronous TTS finishes, a second `media` event updates only the translated URL, followed by `audio_start`, binary PCM16 frames, and `audio_end`. These server frames are broadcast in order to every online member of the room.

```json
{"type":"media","utterance_id":"...","source_audio_url":"/media/...-source.wav","translated_audio_url":null}
{"type":"media","utterance_id":"...","source_audio_url":null,"translated_audio_url":"/media/...-translated.wav"}
```

Translated PCM can be played as soon as the server stream arrives; both persisted WAV files are also available through protected media URLs.

## Web routes

The frontend uses SvelteKit file routing and is split into route pages, reusable components, and a voice-session controller:

```text
/login             account login and registration
/admin             system personnel and meeting management
/rooms             searchable room directory
/rooms/{room_id}   translation, participant controls, live room state, and history
/rooms/{room_id}/subtitles
                   read-only realtime source/translated subtitle display
/settings          voice, automatic playback, and subtitle display preferences
```

Refreshing any of these paths is handled by the Axum SPA fallback. SvelteKit route modules are in `web/src/routes/`, existing feature pages are in `pages/`, reusable UI is in `components/`, and WebSocket/microphone ownership is in `controllers/voice-session.ts`. The static adapter writes the deployable SPA to `web/dist`.

The subtitle display can show source text, translated text, or both. Its color preset, custom colors, source and translation font sizes, line height, caption spacing, and screen padding are stored per user. Changes made in Settings are broadcast to open subtitle tabs and application windows immediately. The display keeps the latest three utterances available, prioritizes the current utterance when space is constrained, and recalculates text sizing whenever its window is resized.

The complete product flow, setting matrix, resize behavior, failure states, and acceptance checklist are documented in [`docs/subtitle-display.md`](docs/subtitle-display.md).

## Tauri applications

The Web client can also be packaged for macOS, Windows, Android, and iOS with Tauri 2. The application shell embeds `web/dist` in its Rust library and starts an Axum server on a random loopback port before creating the WebView. The WebView loads that Axum origin, so SPA fallback, workers, AudioWorklet, and the VAD WASM use normal HTTP paths on every platform.

The embedded Axum service owns only the application delivery layer. It serves static assets and proxies `/api`, `/media`, and `/ws` to the existing Voice Elf server. This keeps the Web client same-origin while avoiding an unsupported mobile link of PostgreSQL, sherpa-onnx, and external model binaries. In the packaged app, the API address can be changed from the login screen or Settings page and is persisted in the platform application-config directory for later launches. The current packaged default upstream is `http://192.168.0.63:3001`; set `VOICE_ELF_APP_SERVER_URL` while running or building the shell when the backend is on another host:

On desktop, the room toolbar opens the subtitle display in a reusable, always-on-top native window with a normal draggable title bar and resizable edges. In a browser, the same action opens the dedicated subtitle route in a reusable popup window. Opening subtitle settings from the desktop display uses a separate settings window so the active meeting page and microphone session remain intact.

```bash
# macOS development against a local backend
make server
make app-dev

# Production package against a deployed HTTPS backend
cd web
VOICE_ELF_APP_SERVER_URL=https://voice.example.com npm run app:build
VOICE_ELF_APP_SERVER_URL=https://voice.example.com npm run app:android:build
VOICE_ELF_APP_SERVER_URL=https://voice.example.com npm run app:ios:build
```

Run Windows packaging on Windows, macOS/iOS packaging on macOS, and Android packaging on a host with the Android SDK, NDK 28+, and JDK 17+. Apple device builds also require `APPLE_DEVELOPMENT_TEAM` or the corresponding Tauri iOS config. Android API 24 and iOS 14 are the configured minimums. See [the Tauri platform architecture](docs/tauri-platform-architecture.md) for the packaging flow, platform permissions, validation matrix, and design constraints.

## Continuous integration and releases

GitHub Actions runs the Rust and Web validations independently on the ARM-based `macos-15` runner. After both validations pass, Android and Intel macOS packaging run in parallel on the same runner. The macOS job cross-compiles for `x86_64-apple-darwin`; separate downstream jobs verify checksums, Android archive structure and ABIs, and the macOS disk image, Intel executable architecture, and 11.0 deployment target. This dependency graph keeps validation, packaging, and artifact failures isolated in the Actions history.

Every pull request, push to `main`, and manual run produces 14-day workflow artifacts. Pushing a version tag matching the Tauri version, such as `v0.1.0`, additionally creates or updates the matching GitHub Release with the verified APK, AAB, Intel DMG, checksums, and build manifests.

Android validation packages are unsigned unless all three repository Actions secrets are configured: `ANDROID_KEY_BASE64`, `ANDROID_KEY_ALIAS`, and `ANDROID_KEY_PASSWORD`. `ANDROID_KEY_BASE64` must contain the base64-encoded Java keystore. Unsigned artifact names include `-unsigned`; signed artifacts are verified with `apksigner` and `jarsigner` before publication. The macOS application uses an ad-hoc signature so CI can verify bundle integrity, but it is not notarized and macOS may still require manual approval in Privacy & Security.

## Checks

```bash
make test
```
