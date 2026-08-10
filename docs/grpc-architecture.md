# gRPC communication architecture

Voice Elf exposes its business API through `voice_elf.v1.ApiService`. The Rust server uses
Tonic and accepts both native gRPC and gRPC-Web on the same listener as the static Web app.

## Calls

- `Call` transports existing account, room, setup and administration operations as a unary RPC.
  Axum handlers remain the internal application boundary so validation, authorization and Diesel
  transactions have one implementation.
- `SubscribeRealtime` is a server-streaming RPC for transcript, translation, room and synthesized
  audio events.
- `SendRealtime` accepts ordered client events and short PCM batches. Browsers cannot use gRPC
  client streaming, so the Web client batches PCM for up to 64 ms and serializes unary calls.
- Every realtime subscription receives an opaque random session ID. Uplink messages are accepted
  only while that subscription remains active and are bound to its authenticated user and room.

## Browser and native shell

The browser uses binary gRPC-Web over same-origin `fetch`. The Tauri local server forwards the
gRPC-Web service path without interpreting Protobuf messages. A WebSocket fallback remains for
older WebKit/WebView versions that cannot consume a streaming Fetch response reliably.

Static assets, authenticated WAV media and exported files remain normal HTTP resources. They need
stable URLs for `<audio>`, browser downloads and cache semantics and are not business RPC methods.
