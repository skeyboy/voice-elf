# Voice Elf Tauri 跨平台架构

## 目标与结论

Tauri 2 壳工程位于 `web/src-tauri`，目标平台是 Android、iOS、macOS 和 Windows。四个平台使用同一套 SvelteKit 静态产物和同一个 Rust/Axum 壳，不维护平台专属 Web 副本。

仓库中现有 `server` 是业务服务，不适合直接链接进移动应用。它依赖 PostgreSQL、sherpa-onnx、本地模型目录以及外部推理进程。当前跨平台实现因此采用两层 Axum：

- App Axum：随 Tauri 应用打包，负责静态资源、SPA fallback 和同源代理。
- Business Axum：现有 `voice-elf-server`，负责账号、房间、WebSocket、ASR、翻译、TTS、持久化和媒体鉴权。

这种边界能够完成 Web/App 共用界面与协议，并保持当前服务端能力完整。如果后续要求手机完全离线运行，需要另立工作流替换 PostgreSQL、模型进程和 sherpa 目标库；这不是静态服务打包可以自然获得的能力。

## 执行链路

```text
npm run build
  -> web/dist/index.html + JS/CSS + AudioWorklet + VAD WASM
  -> rust-embed 编译进 voice_elf_app_lib
  -> Tauri CLI 生成/编译各平台宿主
  -> .app / Windows installer / APK-AAB / iOS app

App 启动
  -> 读取平台应用配置目录中的 app-settings.json
  -> Rust 在 127.0.0.1:0 绑定随机端口
  -> Axum 开始监听
  -> Tauri 创建 main WebView
  -> WebView 加载 http://127.0.0.1:<port>
  -> 静态请求由内嵌资源响应
  -> /api、/media、/ws 由 App Axum 代理到业务服务
```

Axum 先完成端口绑定，再创建 WebView，因此没有固定端口冲突，也没有 WebView 先于服务启动的竞态。监听地址仅为回环地址，不暴露到局域网。

## Web 资源与 HTTP 行为

`rust-embed` 在 Rust 编译时读取 `web/dist`。Tauri 的 `beforeDevCommand` 和 `beforeBuildCommand` 都先执行 `npm run build`，确保进入应用包的是最新静态产物。

静态服务行为：

- `/` 和真实资源路径返回内嵌文件。
- 不带扩展名且不存在的路径返回 `index.html`，支持 `/login`、`/rooms/:id`、`/settings` 刷新。
- 不存在的 `.js`、`.wasm` 等资源返回 404，避免把 HTML 误当资源解析。
- 保留 `Cross-Origin-Opener-Policy: same-origin` 和 `Cross-Origin-Embedder-Policy: require-corp`，满足 VAD WASM/Worker 的隔离要求。
- 带内容哈希的 Svelte 资源和 VAD WASM 使用 immutable cache；manifest、version 和普通资源使用 no-cache。

## 业务服务连接

App Axum 将以下路径透明代理到当前配置的 API 地址：

| App 路径 | 上游协议 | 用途 |
| --- | --- | --- |
| `/api/*` | HTTP/HTTPS | 登录、房间、成员、历史 |
| `/media/*` | HTTP/HTTPS stream | 鉴权音频 |
| `/ws` | WS/WSS | 实时 PCM、转写、翻译、TTS |

代理保持 Web 端同源，因此现有相对 URL、HttpOnly session Cookie 和 WebSocket 代码无需平台分支。HTTP 响应采用流式回传，WebSocket 双向转发文本、二进制、ping/pong 和关闭帧。

App 登录启动页和设置页通过本地 `GET/PUT /__voice_elf/config` 读取、验证并保存 API 地址。配置写入各平台的应用配置目录 `app-settings.json`；保存成功后 Axum 立即切换 HTTP 与 WebSocket 的代理目标，后续启动自动读取。该入口只在内置 Axum 存在时显示，普通 Web 部署不会出现 App 配置项。

配置优先级为运行时环境变量、用户保存值、编译时环境变量、当前局域网默认值 `http://192.168.0.63:3001`。运行时环境变量用于开发或运维显式覆盖；通常无法注入运行时环境变量的移动应用会默认使用用户保存值。局域网地址可能随网络变化，发布构建仍应设置稳定且可从设备访问的 HTTPS 服务，例如：

```bash
cd web
VOICE_ELF_APP_SERVER_URL=https://voice.example.com npm run app:android:build
```

生产环境应使用 HTTPS。Android 允许明文流量是为了 WebView 访问应用自身的回环 Axum，不代表生产上游应该使用 HTTP。

## 平台配置

| 平台 | 工程/配置 | 关键处理 |
| --- | --- | --- |
| macOS | `src-tauri/Info.plist` | 麦克风用途说明、本机网络、`.app` 打包 |
| Windows | Tauri Cargo/config | WebView2 宿主、ICO/Appx 图标；必须在 Windows 产出 MSI/NSIS |
| Android | `src-tauri/gen/android` | API 24、INTERNET、RECORD_AUDIO、回环 HTTP、NDK 28 的 16 KB page 支持 |
| iOS | `src-tauri/gen/apple` | iOS 14、麦克风用途说明、本机网络、设备签名由环境注入 |

Tauri 移动入口使用 `#[cfg_attr(mobile, tauri::mobile_entry_point)]`，crate 同时输出 `staticlib`、`cdylib` 和 `rlib`。这让 Android/iOS 原生宿主调用同一 Rust `run()`，桌面端则由小型 `main.rs` 调用它。

## 构建命令

```bash
cd web
npm install

npm run app:dev
npm run app:build

npm run app:android:dev
npm run app:android:build

APPLE_DEVELOPMENT_TEAM=<team-id> npm run app:ios:dev
APPLE_DEVELOPMENT_TEAM=<team-id> npm run app:ios:build
```

Android 首次重新生成工程可运行 `npm run app:android:init`；iOS 对应 `npm run app:ios:init`。重新 init 可能覆盖 `gen` 下的权限调整，执行后必须复核麦克风权限和回环网络设置。

## 验证计划

按风险从共享层到平台产物验证：

1. `npm run build`：Svelte 类型检查、VAD WASM 和静态产物。
2. `cargo test -p voice-elf-app`：资源嵌入、上游 URL 与壳逻辑。
3. `cargo test --workspace`：确认加入 workspace 后不影响原服务和 Web VAD。
4. macOS：运行/打包，检查 Axum 启动、SPA、API Cookie、WebSocket、麦克风。
5. Android/iOS：真机检查回环 HTTP、录音权限、锁屏/后台恢复与远端 HTTPS。
6. Windows：在 Windows CI/开发机检查 WebView2、录音权限、MSI/NSIS 安装与卸载。

单台 macOS 主机不能对 Windows 安装器和所有移动真机行为给出等价验证；这些项目应进入对应平台 CI 或发布前真机矩阵。
