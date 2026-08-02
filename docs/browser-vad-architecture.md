# 浏览器 VAD 与服务端 ASR 架构

## 最终方案

```text
浏览器
  getUserMedia
      |
      v
  AudioWorklet: 复制浏览器原生采样率 mono Float32 块
      |
      v (Transferable ArrayBuffer)
  Web Worker + Rust/WASM
      |-- 重采样 16 kHz、PCM16 转换、512 samples / 32 ms 分帧
      |-- Lele 0.1.12 AOT 推理 Silero VAD v6.2
      |-- 浏览器采集降噪 + 动态环境噪声底/SNR 门控
      |-- 静音: 仅保留约 224 ms pre-roll，不上传
      `-- 语音: speech_start -> PCM 帧 -> speech_end
                              |
                              v
服务端 Axum WebSocket（账号 + 房间 owner 授权）
  不执行 VAD，只验证不可信的浏览器分段
      |-- 会话状态与边界顺序
      |-- 每帧必须为 512 samples / 1024 bytes
      |-- 最大 1.5 倍实时发送速率（另允许 pre-roll）
      `-- 房间配置的 5~120 秒强制断句上限
                              |
                              v
  Qwen ASR（服务端） -> 本地翻译 LLM -> 低优先级 Qwen3 TTS
      |                    |                    |
      `-------- PostgreSQL 记录 + WAV 文件 URL -'
```

浏览器 VAD 是唯一分句实现。WASM 或 Worker 初始化失败时，录音不会开始，页面直接显示错误；系统不再静默切换到服务端 VAD，也不会上传连续静音 PCM。

## 方案选择

| 方案 | 优点 | 当前风险 | 结论 |
| --- | --- | --- | --- |
| `silero-vad-rust + wasm-bindgen` | 常见绑定工具，验证快 | 候选 `silero-vad-web 0.1.0` 仍是未实现脚手架；基于 ONNX Runtime 的路径还会引入另一套 Web runtime | 不采用该 crate 作为生产实现 |
| Lele + Silero | 纯 Rust AOT、无 ONNX Runtime 浏览器依赖、单一 WASM、支持 Silero | 约 4.3 MiB，社区和模型算子覆盖仍需持续验证 | 当前实现 |
| WebRTC VAD | 体积小、语音通话场景成熟 | C 依赖交叉编译复杂，噪声适应性弱于 Silero | 已从服务端和浏览器移除 |

当前实现固定使用 `lele = 0.1.12` 和 Silero VAD v6.2。模型以 512 样本窗口运行，Silero 负责人声开始判定；静音期持续学习环境噪声底，开始和活动保持阈值按信噪比动态抬升，直流偏移不会被当作有效能量。活动段约 448 ms 连续静音后结束，减少换气造成的误切。房间的 `max_utterance_seconds` 会强制形成下一条记录，但会保留短接续窗口，让未停止的发声立即进入下一段而不停止录音会话。

`audio-processor.js` 仍然必要。AudioWorklet 是浏览器实时音频线程到 Worker 的最小桥接层，但它只复制 Float32 输入；重采样、PCM 转换、音量、分帧、VAD、pre-roll 和断句全部在 Rust/WASM 中完成。

## ASR 保持在服务端

Qwen ASR 不迁移到浏览器：

1. 模型下载、内存和多标签页重复实例会明显增加终端负担。
2. 移动端 WebGPU、WASM 和内存能力差异会使时延不可预测。
3. 服务端需要集中保护模型、调度推理，并产生可信的用户、房间、WAV 和转写记录。
4. 端侧 VAD 已减少静音带宽和服务端逐帧计算；迁移 ASR 的收益与成本不匹配。

## 安全边界

WASM 不是可信边界。服务端仍执行以下限制：

- WebSocket 要求登录 cookie，且只有房主能开始实时翻译。
- 服务端从数据库读取房间断句上限，并限定在 5~120 秒。
- 只接受 `speech_start` 与 `speech_end` 之间的固定 1024-byte PCM16 帧。
- pre-roll 突发之后只允许有限的实时发送速率，超速帧会被丢弃。
- 客户端漏发结束边界时，服务端仍按房间上限或 `flush` 收束当前记录。
- 用户、房间、音频路径和最终转写始终由已授权的服务端会话落库。

## 构建与部署

```bash
./scripts/build-web-vad.sh
cd web && npm run build
```

WASM 源文件输出到 `web/static/wasm/voice_elf_web_vad.wasm`。SvelteKit 静态构建将其复制到 `web/dist/wasm/`，Axum 再从 `web/dist` 提供 SPA 路由和静态资源。
