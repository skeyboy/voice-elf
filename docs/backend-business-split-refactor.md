# Voice Elf 后端业务拆分改造记录

## 1. 改造范围

本文件记录 `docs/service-splitting-analysis.md` 阶段 A 的实际落地结果。改造没有删除原单体入口，也没有执行破坏性数据库迁移。

当前提供三个二进制：

| 二进制 | 作用 | 默认监听 |
| --- | --- | --- |
| `voice-elf-public` | 对外业务数据面 | HTTP `0.0.0.0:3001` |
| `voice-elf-admin` | 内部管理控制面 | HTTP `127.0.0.1:3002`、gRPC `127.0.0.1:50051` |
| `voice-elf-server` | 迁移期兼容单体 | HTTP `0.0.0.0:3001` |

### 1.1 远端基线合并检查

2026-08-11 执行 `git fetch --prune origin` 后，当前 `HEAD` 与 `origin/main` 均为
`488a43b fix(mac): restore native and web system audio capture`，分支差异为
`0 ahead / 0 behind`。因此没有待拉取或待手工合并的远端提交；当前未提交的服务拆分改造
直接建立在远端最新的 macOS 原生/Web 系统音频采集修复之上，并保留该提交涉及的 Tauri、
Web 音频采集和服务端音频处理功能。

## 2. 已完成内容

### 2.1 进程与构建边界

- 原 `server/src/main.rs` 的实现提取到 `server/src/lib.rs`，公共初始化逻辑由 library 复用。
- `server/src/bin/voice-elf-public.rs` 只启动公网 Router 和 gRPC client。
- `server/src/bin/voice-elf-admin.rs` 并行启动管理 HTTP 与 tonic gRPC server。
- `server/src/main.rs` 保留为兼容入口，调用 `run_combined()`。
- `Makefile` 新增 `public-server`、`admin-server`，release build 同时构建三个入口。

### 2.2 HTTP 业务边界

`api::public_router()` 包含：

- 用户注册、登录、登出、当前用户和密码找回。
- 房间、成员发言权限、会议记录、导出。
- 参考音色和当前 TTS 音色目录。
- 实例授权和初始化状态只读快照。

`api::admin_router()` 包含：

- 系统初始化。
- 人员列表、创建、导入、角色/状态、密码重置。
- 会议管理与检查。
- ASR/TTS 系统和租户配置、IndexTTS 运行管理。
- 租户、实例、Key 签发/轮换/撤销、entitlement 检查。

admin 独立 HTTP 额外合并最小登录会话路由，使管理员可在独立端口登录；它不开放注册、业务房间、媒体和 WebSocket。public 不合并 `/api/admin/*`、`/api/setup/initialize` 或 Key 签发接口，但保留前端启动必需的只读 `/api/setup/status`。

### 2.3 tonic gRPC 控制面

契约位置：`server/proto/control.proto`。

服务：`voiceelf.control.v1.ControlPlane`。

- `GetRuntimeSnapshot`：取得管理服务和依赖的结构化快照。
- `CheckReadiness`：供 public 或编排系统判断管理控制面是否 ready。
- `WatchRuntimeCommands`：向 public 持续推送人员会话撤销和会议关闭命令。

`server/build.rs` 使用 vendored protoc 生成 client/server 代码，避免部署机器必须预装 protoc。gRPC 请求使用 `x-voice-elf-control-token` metadata；非回环监听未配置 Token 时 admin 拒绝启动。

### 2.4 环境部署检测

admin 当前检测以下项目：

| 检查 | 必需 | 判定来源 |
| --- | --- | --- |
| PostgreSQL | 是 | 数据库连接和 `system_installations` 查询 |
| 系统初始化 | 是 | 是否存在 installation profile |
| 实例授权 | 是 | `AuthorityService` 当前快照 |
| ASR provider | 是 | 当前生效 provider 是否可解析 |
| TTS provider | 是 | 当前生效 provider 是否可解析 |
| FunASR 流式服务 | 否 | 启用后执行实际 WebSocket 握手 |
| Public 命令流 | 否 | 当前连接数与最近连接时间 |
| SMTP | 否 | MailService 配置是否完整 |

状态值固定为 `ready`、`degraded`、`unavailable`、`unknown`。public 启动时立即调用 admin gRPC 并逐项写入日志；连接失败不会让 public 进程退出，而是生成 `admin_control_plane=unavailable` 的结构化快照。

HTTP 可视化数据入口：

```text
GET /api/runtime/dependencies
```

管理页新增“部署检测”标签，显示整体 readiness、初始化/授权状态、每个依赖的必需性、状态和诊断消息。响应与日志不包含数据库密码、SMTP 密码、client secret 或控制 Token。

部署检测页面默认每 15 秒重新请求依赖快照并执行后端校验，可切换为 5、15、30、60 秒或暂停。
页面显示下次检测倒计时、最近校验时间、正常/降级/异常计数，并支持手动立即检测。自动检测失败时
保留最近一次成功快照并明确标记为陈旧，避免短暂网络错误导致观测窗口内容清空。

独立观测入口为 `GET /admin/dependencies`。该页面按基础设施、控制面、语音能力和通知服务分组，
并在 Admin 标题区提供固定入口；原“部署检测”标签继续作为紧凑视图。启用 Qwen3-TTS 后，观测页
会增加 `qwen_tts` 检查，并通过兼容服务的 `/v1/audio/voices` 验证实际可用性。

TTS Registry 新增 `qwen3-tts` Provider，使用 vLLM-Omni 的 OpenAI 兼容
`POST /v1/audio/speech` 接口，当前支持 CustomVoice 模式、九种官方预置音色和十种语言。服务返回的
24 kHz WAV 被转换为现有 `TtsAudioChunk` 流，不改变房间翻译与音频广播协议。Provider 默认关闭，
只有 `TTS_QWEN_ENABLED=true` 且健康检查通过后才允许管理员选为新房间管线的 TTS 后端。

#### ASR 兼容性评估与首批接入

现有实时管线固定输入 16 kHz、单声道 PCM16，并要求 provider 在 VAD 语音开始时建流、持续接收音频帧、
回传增量文本，在语音结束后给出最终文本。按这个契约评估候选方案：

| 方案 | 当前兼容性 | 中文/部署判断 | 本次决策 |
| --- | --- | --- | --- |
| Qwen3-ASR 本地运行时 | 已接入 | 多语种，本地模型和二进制随实例部署 | 保留 `qwen-local` |
| [FunASR](https://github.com/modelscope/FunASR) Paraformer | 高；官方 WebSocket 支持 16 kHz PCM、online/2pass | 中文实时场景成熟，可 CPU/GPU 私有部署 | 已接入 `funasr-streaming` |
| [WeNet](https://github.com/wenet-e2e/wenet) | 模型层支持流式，但部署端协议需另做适配和版本验证 | 适合已有 WeNet 服务的组织 | 暂不直接注册，后续按真实服务协议接入 |
| [FireRedASR2S](https://github.com/FireRedTeam/FireRedASR2S) | 官方仓库明确提供流式 VAD，未把 ASR 网络流协议作为稳定契约 | 中文、方言能力有吸引力 | 作为句末精修候选，不伪装成实时 provider |
| [Moonshine](https://github.com/moonshine-ai/moonshine) | 真流式、端侧友好，但需要原生库或自建侧车 | 英语端侧更合适；当前普通话模型不作为中文默认 | 暂不接入服务端默认链路 |
| Whisper/faster-whisper | 通常以分块重识别模拟流式，第三方协议分散 | 生态广，适合离线或句末精修 | 不进入低延迟主链路 |
| 商业 WebSocket/gRPC API | 技术上可接入 | 需要凭据、计费、数据出境和供应商协议决策 | 待确定供应商后独立 adapter 接入 |

FunASR adapter 遵循官方 2-pass WebSocket 协议：先发送流参数，再发送 PCM 二进制帧，结束时发送
`is_speaking=false, is_end=true`；`2pass-online` 文本转为 `transcript_delta`，`2pass-offline`
句末纠错结果作为最终 transcript。配置 `FUNASR_ENABLED=true` 后，Admin 的 ASR 管理页会显示连接检查，
并可在系统默认或租户覆盖下拉框选择。切换只影响新建房间管线，已有房间继续使用创建时快照。

依赖页新增 `funasr_streaming` 检查。它执行真实 WebSocket 握手；地址错误、服务未启动或连接超时均显示为
`unavailable`。FunASR 是可选侧车，其异常不会把 PostgreSQL 等核心依赖误判为失败。`local` 模式现在
允许 Qwen 模型目录或 FunASR 至少一个就绪；两者均存在时仍默认 Qwen，管理员可在线切换。

两项侧车现在都有项目内运行管理器：`scripts/funasr.sh` 和 `scripts/qwen-tts.sh`，统一提供
`setup|enable|start|status|stop|logs|doctor`。Apple Silicon 上 Qwen 使用 MLX-Audio 0.4.8 和
`mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-bf16`；Linux/CUDA 仍可配置外部 vLLM-Omni。
首次 `setup` 完成后，Public 或兼容单体在相应 `*_ENABLED=true` 时自动请求启动侧车。

### 2.5 gRPC 实时管理命令

`ControlPlane` 新增服务端流式 RPC `WatchRuntimeCommands`。public 主动连接 admin 并保持
命令流，因此网络依赖仍是 `public -> admin`，不要求 admin 访问 public 的内网地址。

当前命令包括：

- `RevokeUserSessions(user_id)`：管理员将人员改为非 active 后，关闭该人员在 public 的现有 WebSocket。
- `CloseRoom(room_id)`：管理员结束或归档会议后，关闭该会议在 public 的实时房间运行时。

每个命令包含 UUID `command_id`、admin 进程内递增 `sequence` 和 `issued_at`。Admin 保存最近
2048 条命令；Public 保存最后处理序号、对最近 4096 个命令 ID 去重，并在流中断后按
1-30 秒退避重连。重连携带最后序号，Admin 重启导致序号回绕时会自动从新队列起点恢复。
`RoomHub` 的撤销/关闭操作本身也是幂等的。兼容单体仍直接操作本地 `RoomHub`，同时发布
命令不会改变原有行为。依赖快照与命令流复用同一个 tonic `Channel`，由 h2 连接负责多路
复用和重连，不再为每次依赖查询新建 transport channel。

### 2.6 房间实时对话分页

房间详情不再在首次进入时加载完整历史。前端通过以下分页接口按每页 30 条读取：

```text
GET /api/rooms/{room_id}/utterances?page=1&page_size=30&q=
```

- 第 1 页固定代表时间上最新的一页，首次渲染后使用 `auto` 定位到最新记录，不执行平滑滚动动画。
- 列表到达顶部后可继续向上滚动或下拉，按第 2、3 页顺序加载更早记录。
- 旧页插入列表头部时按插入前后的高度差恢复阅读锚点，避免内容跳动。
- 顶部保留“加载更早记录”按钮，作为鼠标、键盘和触摸操作的显式后备入口。
- 查询条件变化时从第 1 页重新开始；返回房间时可复用当前页面的内存缓存，不重复请求已经加载的页。

### 2.7 管理员邮件配置

admin 新增邮件配置读取与修改接口：

```text
GET /api/admin/email/config
PUT /api/admin/email/config
```

配置包含启用状态、SMTP 主机/端口/安全模式、用户名、发件地址、发件人名称、系统访问地址和
重置链接有效期。密码字段默认不回传；修改时留空表示沿用上一个版本，也可显式选择清除。
保存成功后 admin 立即热更新 `MailService`，无需重启服务。依赖检测同步反映新配置是否完整，
但 readiness 检查不会主动发送测试邮件。

### 2.8 不可变变更历史与软删除

迁移 `202608110001_add_change_history_and_email_settings` 增加基础表和触发器，后续迁移
`202608110002_classify_soft_delete_history` 将状态删除统一归类为删除操作：

- `system_email_settings`：邮件配置版本表，旧版本由 `current` 转为 `historical`，新版本另行插入。
- `data_change_history`：保存业务实体每次创建、修改、删除的 `before_data` / `after_data` 快照。
- `tts_voice_aliases.record_status`：音色别名删除改为 `deleted` 状态，再次设置时恢复为 `current`。

数据库触发器覆盖人员、会议、成员、语音会话、对话、发言人、识别精修、参考音色、系统初始化、
邮件、ASR/TTS、音色别名和授权实体。每个实体最多有一条 `current` 历史记录；后续修改把上一条
标为 `historical` 并创建新记录；`deleted_at` 非空或 `record_status=deleted` 的状态变更与物理删除
统一创建 `delete / deleted` 记录。密码哈希、Token 哈希、client secret
和 SMTP 密码不会写入历史 JSON。

这里采用“当前投影 + 不可变历史”的实现：现有业务表仍用于高效读取当前状态，但每次修改都保留
修改前后的独立记录，不会覆盖审计证据。已经具有 `deleted_at` / `archived` 状态的人员、会议、
参考音色等继续使用状态删除；音色别名已从物理删除改为状态删除。登录会话、密码重置 Token、
访问 Token 属于可撤销的短期安全凭据，注销和过期清理由安全策略执行物理删除，不作为可恢复业务数据。

管理页新增“邮箱配置”和“变更历史”标签；历史接口支持对象类型和分页过滤：

```text
GET /api/admin/change-history?page=1&page_size=50&entity_type=rooms
```

## 3. 配置

新增环境变量：

```dotenv
VOICE_ELF_ADMIN_BIND=127.0.0.1:3002
VOICE_ELF_ADMIN_GRPC_BIND=127.0.0.1:50051
VOICE_ELF_ADMIN_GRPC_URL=http://127.0.0.1:50051
VOICE_ELF_CONTROL_TOKEN=replace-with-at-least-32-random-bytes
FUNASR_ENABLED=false
FUNASR_WEBSOCKET_URL=ws://127.0.0.1:10095/
FUNASR_MODE=2pass
```

- `VOICE_ELF_BIND` 只控制 public 或兼容单体 HTTP。
- `VOICE_ELF_ADMIN_BIND` 控制 admin HTTP。
- `VOICE_ELF_ADMIN_GRPC_BIND` 是 admin gRPC 监听地址。
- `VOICE_ELF_ADMIN_GRPC_URL` 是 public 使用的完整 tonic endpoint。
- Token 在两个服务中必须一致。回环开发允许省略；非回环 gRPC 监听必须配置。

生产环境应使用 mTLS 或服务网格身份替代仅共享 Token 的方案，并在网络策略中只允许 public workload 访问 admin gRPC。

## 4. 启动方式

先启动 admin，再启动 public：

```bash
export VOICE_ELF_CONTROL_TOKEN='replace-with-at-least-32-random-bytes'
VOICE_ELF_BACKEND=demo cargo run --bin voice-elf-admin
```

另一个终端：

```bash
export VOICE_ELF_CONTROL_TOKEN='replace-with-at-least-32-random-bytes'
VOICE_ELF_BACKEND=demo cargo run --bin voice-elf-public
```

访问地址：

- 对外业务：`http://127.0.0.1:3001`
- 内部管理：`http://127.0.0.1:3002/admin`
- 首次初始化：`http://127.0.0.1:3002/setup`
- public 依赖快照：`http://127.0.0.1:3001/api/runtime/dependencies`
- admin 本地依赖快照：`http://127.0.0.1:3002/api/runtime/dependencies`

迁移期回滚：

```bash
cargo run --bin voice-elf-server
```

## 5. 前端开发代理

Vite 开发环境按路径分流：

- `/api/admin/*`、`/api/setup/*`、`/api/runtime/dependencies` -> admin `3002`
- 其他 `/api/*`、`/media/*`、`/ws` -> public `3001`

生产环境建议使用两个域名或同一网关的路径策略。若使用两个域名，admin 的 Cookie、CSRF、CORS 和 CSP 必须独立配置；当前最简单的生产拓扑是同一受控网关按路径转发并禁止公网访问 admin 路径。

## 6. 验证结果

已执行：

```bash
cargo fmt --all
cargo check -p voice-elf-server --all-targets
cargo test -p voice-elf-server
cd web && npm run build
```

结果：Rust library、兼容单体、public、admin、tonic 生成代码全部通过检查；后端测试全部通过；Svelte 检查为 0 errors / 0 warnings，静态站点构建成功。双进程实跑验证了路由隔离、正确/错误 gRPC Token、admin 地址不可达时的 public 降级快照，以及 `public_command_stream` 长连接为 ready。真实 PostgreSQL 已应用变更历史迁移，并验证 15 个业务表的三类触发器、事务回滚和敏感字段脱敏。浏览器验证首次进入 1162 条记录的房间只加载最新 30 条且直接位于底部，加载上一页后变为 60 条；管理员邮件配置与历史页面可独立访问。

全 workspace 测试包含 Tauri 和平台依赖，本次运行时因系统磁盘空间耗尽中止；受改造直接影响的 server 测试已单独完整通过。仍需目标环境验证：PostgreSQL 双进程连接、真实管理员登录、admin 中断后的自动恢复、授权租约过期、反向代理网络隔离和生产 mTLS。

## 7. 当前限制与下一阶段

### 7.1 共享数据库

两个服务仍复用 `DATABASE_URL` 和同一 Diesel schema。这是迁移桥梁，不是最终数据隔离。下一阶段应：

1. 管理设置和人员变更只通过 admin command RPC。
2. public 使用版本化只读投影，不直接读 `authority_*` 和系统设置表。
3. migrations 改为独立 job 或明确由 admin 独占执行。
4. 最终给 public/admin 使用不同数据库角色，再物理拆库。

### 7.2 实时撤销的剩余边界

实时撤销命令通道已经完成，双进程运行时不再依赖 admin 进程内的空 `RoomHub`。当前命令
队列仍位于 admin 内存中：短时断连可续传，但 admin 在命令发布后、public 消费前崩溃会
丢失尚未执行的命令。生产高可用阶段应将 command/outbox 写入 PostgreSQL，由 public ACK，
并补充操作审计、积压指标和失败告警。

### 7.3 gRPC 高可用

运行时命令已使用长连接流和退避重连，依赖快照查询复用同一个 tonic channel。当前尚无
服务发现，也没有成功快照缓存；下一阶段应缓存最近成功快照并标记 `stale_at`，同时增加
Prometheus 指标和 readiness/liveness 分离。

## 8. 文件索引

- 分析方案：`docs/service-splitting-analysis.md`
- 本改造记录：`docs/backend-business-split-refactor.md`
- 服务组装：`server/src/lib.rs`
- tonic 实现：`server/src/control.rs`
- protobuf：`server/proto/control.proto`
- public 入口：`server/src/bin/voice-elf-public.rs`
- admin 入口：`server/src/bin/voice-elf-admin.rs`
- API 路由边界：`server/src/api.rs` 与 `server/src/api/*`
- 管理页部署检测：`web/src/pages/admin-page.ts`
- 独立依赖观测页：`web/src/pages/dependencies-page.ts`
- 房间历史分页：`web/src/pages/translator-page.ts` 与 `web/src/components/conversation-view.ts`
- 版本化设置和历史存储：`server/src/storage/settings.rs`
- 变更历史迁移：`server/migrations/202608110001_add_change_history_and_email_settings` 与 `server/migrations/202608110002_classify_soft_delete_history`
