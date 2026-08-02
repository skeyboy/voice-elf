# Voice Elf

Local-first realtime speech translator with a Rust/Axum WebSocket server and a SvelteKit client.

## Architecture

```text
Browser AudioWorklet (native-rate mono Float32 capture)
  -> Web Worker + Lele/Silero Rust WASM (16 kHz PCM16, 32 ms frames, 224 ms pre-roll)
  -> speech_start / speech PCM / speech_end over WebSocket /ws
  -> server boundary, frame, rate, and duration validation
  -> ASR queue: Qwen ASR CLI
  -> translation queue: Qwen3 local LLM
  -> low-priority TTS queue: qwen3-tts-rs CLI (24 kHz mono PCM16)
  -> WebSocket playback
```

Each utterance reports five timestamps: `t0` speech start, `t1` VAD endpoint, `t2` STT complete, `t3` translation complete, and `t4` TTS complete.

Recording is continuous until the client sends `flush`. The browser's Rust/WASM VAD closes each sentence after its silence endpoint and sends only speech PCM as an independent utterance. The server has no VAD implementation; it treats browser boundaries as untrusted hints and enforces the authenticated room lifecycle, exact frame format, near-realtime ingress rate, and maximum duration. If WASM initialization fails, recording does not start and the UI reports the error. ASR and translation have independent per-connection workers. TTS starts only after pending text work is complete; a newly queued sentence preempts an in-progress TTS process and the voice job is retried after the text queues become idle.

Qwen ASR remains server-side. Its model is too large and device-sensitive for the default browser path, and keeping it on the server protects model files and preserves authoritative persisted results. See [the browser VAD architecture](docs/browser-vad-architecture.md) for the deployment and security decisions.

Rooms define `max_utterance_seconds` with a default of 20 seconds and an allowed range of 5 through 120 seconds. Continuous speech is force-segmented at that duration and immediately continues in a new utterance record.

The backend pipeline is organized by runtime responsibility under `server/src/pipeline/`: `session` validates browser speech segments, `transcription`, `translation`, and `synthesis` own their stage workers, and `jobs`, `events`, `latency`, and `config` contain shared scheduling concerns. PostgreSQL utterance persistence is isolated in `server/src/storage/history.rs`.

## Run the demo

The demo backend exercises browser capture and VAD, WebSocket transport, translation events, latency reporting, and audio playback without model downloads. On macOS it speaks the demo translation with the system voice; speech recognition and translation remain deterministic samples until the local Qwen backends are configured.

```bash
cd web
npm install
npm run build
cd ..
cargo run --bin voice-elf-server
```

Open <http://127.0.0.1:3000>. Microphone access works on localhost in current browsers.

With `VOICE_ELF_BIND=0.0.0.0:3000`, other devices on the same LAN can open `http://<server-lan-ip>:3000` for account, room, history, and audio playback testing. Browser microphone capture requires a secure context: `localhost` works over HTTP, while a LAN IP normally requires a trusted HTTPS certificate. The client reports this explicitly instead of failing silently.

For frontend development, run `npm run dev` from `web/`; Vite listens on all interfaces and proxies `/ws` and `/api` to port 3000. LAN clients can use `http://<server-lan-ip>:5173` with HMR. Run `npm run deploy:watch` to compile the Rust VAD once and continuously rebuild `web/dist`; the Axum service on port 3000 serves each new frontend build without a backend restart.

For temporary Internet testing over trusted HTTPS, install `cloudflared` and use the local tunnel manager. It starts the tunnel in the background, waits for the public health check, and prints the resulting address:

```bash
./scripts/public-tunnel.sh start production  # Axum production site
./scripts/public-tunnel.sh start dev         # Vite/HMR site
./scripts/public-tunnel.sh status all
./scripts/public-tunnel.sh stop all
```

The equivalent Make targets are `make web-public`, `make web-dev-public`, `make web-public-status`, and `make web-public-stop`. Runtime PIDs, URLs, and logs are stored under `.local/run/public-tunnel/`. The printed `trycloudflare.com` address supports secure microphone capture and WebSocket traffic. Quick Tunnel addresses are temporary and change whenever the tunnel process restarts; use an authenticated named tunnel and access policy before treating this as a permanent deployment.

## Local models

On macOS, the setup script builds the three inference binaries with Apple Accelerate and downloads the Qwen3 ASR, translation, and TTS weights into the ignored `.local/` directory:

```bash
./scripts/setup-local-models.sh
cp .env.example .env
# Set VOICE_ELF_BACKEND=local and point the model variables at .local/.
cargo run --release --bin voice-elf-server
```

The server loads `.env` automatically. The local configuration uses `qwen_asr` for speech recognition, `llama-completion` with Qwen3-0.6B for translation, and `generate_audio` from qwen3-tts-rs for speech synthesis. You can instead configure an OpenAI-compatible translation endpoint with `LOCAL_LLM_BASE_URL` and `LOCAL_LLM_MODEL`.

The ASR adapter starts at the VAD speech-start edge and receives PCM continuously while the speaker is still talking. Its low-latency defaults can be tuned with `QWEN_ASR_STREAM_UNFIXED_CHUNKS`, `QWEN_ASR_STREAM_MAX_NEW_TOKENS`, and `QWEN_ASR_ENCODER_WINDOW_SECONDS`; the adapter invokes:

```text
qwen_asr -d <model-dir> --stdin --stream \
  --stream-unfixed-chunks 0 --stream-max-new-tokens 12 \
  --enc-window-sec 4 [--language <language>]
```

The TTS adapter invokes `generate_audio` with `--model-dir`, `--text`, `--speaker`, `--language`, `--device`, and `--output`.

The local backend validates both model directories at startup. Runtime model failures are returned to the client as recoverable errors, so the WebSocket session can keep listening.

Qwen's stable token callback is forwarded immediately as real `transcript_delta` events. After VAD closes the sentence and ASR produces its final text, `llama-completion` stdout is filtered and forwarded token-by-token as real `translation_delta` events. Translation intentionally starts from the finalized sentence rather than repeatedly translating unstable ASR prefixes.

On the tested 2019 Intel Mac, a warm 6.5-second continuous sample produced its first source delta at about 3.9 seconds and completed ASR at about 8.8 seconds. A cold model can add roughly five seconds. CPU inference can therefore still lag behind live capture even though audio transport and event delivery are genuinely streaming. TTS is substantially slower on this machine and remains preemptible, low-priority work so it cannot block newer source or translated text.

Qwen streaming uses two-second audio chunks. The configured 12-token decode budget bounds each streaming step while retaining enough capacity for normal Chinese and English speech rates. If live recognition fails or returns no text, the adapter retries the preserved utterance PCM with faster `--silent` batch recognition before reporting an error.

## PostgreSQL history

Set `DATABASE_URL` to enable asynchronous persistence through Diesel and its bb8 connection pool. The database must already exist; the server applies the idempotent table and index definitions during startup.

Each completed utterance is stored as two mono PCM16 WAV files: the received source audio and the translated TTS audio. The source WAV and processing record are persisted before ASR. Transcript and translation fields are updated as their stages finish, and the translated WAV path and final TTS latency are added later by the low-priority synthesis worker. A failed or preempted stage therefore does not discard the source recording or earlier results.

Every VAD utterance is now persisted before ASR with its user, room, session, utterance ID, source WAV, and processing status. Empty ASR results become a record-scoped `recognition_failed` event rather than a global pipeline error, so the client keeps the failed row and its playable source audio for diagnosis.

```bash
createdb voice_elf
echo 'DATABASE_URL=postgres://localhost/voice_elf' >> .env
```

Accounts use Argon2 password hashes and seven-day HTTP-only session cookies. A user can create and control rooms; other registered users can search for a room, join it, and browse its read-only record preview. `users`, `auth_sessions`, `rooms`, and `room_members` hold this authorization state.

Each authorized WebSocket connection creates a row in `voice_sessions`. Completed utterances are stored in `voice_utterances` with their account/room ownership, source and translated text, language pair, audio duration, all `t0` through `t4` latency measurements, and the two audio file paths and URLs. Audio samples remain in WAV files rather than PostgreSQL binary columns. Media URLs require a logged-in owner or room member.

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
GET    /api/rooms?q=room-name
POST   /api/rooms
GET    /api/rooms/{room_id}?q=transcript-text
PATCH  /api/rooms/{room_id}
DELETE /api/rooms/{room_id}
POST   /api/rooms/{room_id}/join
```

Room update/delete and realtime translation are owner-only. Joined members can read room details and protected audio files. The WebSocket endpoint requires the session cookie and the owner room ID: `ws://localhost:3000/ws?room_id={room_id}`.

Client text frames:

```json
{"type":"configure","source_language":"auto","target_language":"zh","voice":"ryan","max_utterance_seconds":20}
{"type":"start"}
{"type":"speech_start"}
{"type":"speech_end"}
{"type":"flush"}
```

Client binary frames are fixed 512-sample little-endian PCM16 at 16 kHz, mono, and are sent only between `speech_start` and `speech_end`. The server does not accept a continuous non-VAD audio mode. Server text frames carry state, incremental `transcript_delta` and `translation_delta` updates, final text, media URLs, audio metadata, and latency events. Media is returned incrementally: source audio first, translated audio after TTS.

```json
{"type":"media","utterance_id":"...","source_audio_url":"/media/...-source.wav","translated_audio_url":null}
{"type":"media","utterance_id":"...","source_audio_url":null,"translated_audio_url":"/media/...-translated.wav"}
```

Binary server frames contain mono PCM16 at the sample rate announced by the preceding `audio_start` event.

## Web routes

The frontend uses SvelteKit file routing and is split into route pages, reusable components, and a voice-session controller:

```text
/login             account login and registration
/rooms             searchable room directory
/rooms/{room_id}   translation, room records, and read-only preview
/settings          voice and automatic playback preferences
```

Refreshing any of these paths is handled by the Axum SPA fallback. SvelteKit route modules are in `web/src/routes/`, existing feature pages are in `pages/`, reusable UI is in `components/`, and WebSocket/microphone ownership is in `controllers/voice-session.ts`. The static adapter writes the deployable SPA to `web/dist`.

## Checks

```bash
make test
```
