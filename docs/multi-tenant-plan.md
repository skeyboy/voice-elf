# Voice Elf 多租户自建服务与授权总线方案

## 结论

本次采用“总线控制面 + 租户自建数据面”，而不是把所有租户数据放进一套共享业务库。

```text
授权总线（平台部署）
  租户、部署实例、凭据哈希、授权期限、心跳与审计
                  │ OAuth 2.0 Client Credentials 风格认证
                  │ 短期 Bearer Token + 有期限离线租约
                  ▼
租户后端（租户自建） ─── 本地数据库、用户、密码、会话、会议、字幕、音频
        │ HttpOnly 本地登录会话 + 本地授权快照
        ▼
租户前端（与现有客户端相同）
```

总线不接收租户用户资料，租户前端也不持有总线 `client_secret` 或访问令牌。浏览器只调用同源的 `/api/instance/authorization` 和现有 `/api/auth/*`：前者说明部署实例是否获得许可，后者返回租户自建服务中的本地用户。因此现有房间、媒体与 WebSocket 协议无需携带 `tenant_id`，客户端对租户路由保持无感。

## 运行模式

服务端通过 `VOICE_ELF_AUTHORITY_MODE` 使用三种模式：

| 模式 | 用途 | 业务数据 | 授权行为 |
| --- | --- | --- | --- |
| `standalone` | 当前默认和本地开发 | 当前 PostgreSQL | 不连接总线，保持向后兼容 |
| `bus` | 平台控制面 | 平台管理库 | 开放凭据签发、令牌与授权检查接口 |
| `tenant` | 租户自建部署 | 租户自己的 PostgreSQL/媒体目录 | 后端定时向总线校验，前端读取本地快照 |

租户部署配置：

```dotenv
VOICE_ELF_AUTHORITY_MODE=tenant
VOICE_ELF_AUTHORITY_URL=https://authority.example.com
VOICE_ELF_AUTHORITY_CLIENT_ID=vei_...
VOICE_ELF_AUTHORITY_CLIENT_SECRET=ves_...
VOICE_ELF_AUTHORITY_CHECK_SECONDS=300
VOICE_ELF_AUTHORITY_TIMEOUT_SECONDS=10
```

非回环地址必须使用 HTTPS。密钥只配置在后端环境中，不得写入构建产物、浏览器存储、URL 或日志。总线部署使用 `VOICE_ELF_AUTHORITY_MODE=bus`；未配置时仍是 `standalone`。

## 认证与授权流程

1. 平台管理员在总线管理页创建租户，设置授权到期、宽限期、提前提醒天数和离线租约。
2. 管理员为租户签发部署实例。总线返回 `client_id` 和仅显示一次的高熵 `client_secret`，数据库只保存其 Argon2 哈希。
3. 租户后端以表单方式调用 `/api/authority/oauth/token`，使用 `client_credentials` 换取 10 分钟不透明 Bearer Token。
4. 租户后端调用 `/api/authority/entitlements/check`。总线同时验证令牌、实例状态、租户状态、授权期限和宽限期，记录心跳与审计。
5. 校验结果保存在租户后端内存快照中。浏览器定时读取同源快照；业务 REST、媒体、WebSocket、注册和登录则由后端统一强制执行。
6. 密钥轮换或实例撤销会立即删除该实例的现有访问令牌。租户后端在下次校验时重新认证或进入阻断状态。

这是机器到机器的授权，不代替租户本地用户认证。租户用户仍使用 Argon2 密码、HttpOnly 会话 Cookie 和现有人员状态管理；总线系统管理员也使用总线本地账号登录后操作授权管理页。

## 到期、提醒与故障策略

授权状态只有以下几类：

| 状态 | 业务是否可用 | 处理 |
| --- | --- | --- |
| `authorized` | 是 | 正常运行 |
| `warning` | 是 | 到达提前提醒窗口，前端展示到期提示 |
| `grace` | 是 | 正式授权已到期但仍在宽限期，持续提示 |
| `blocked` | 否 | 租户/实例撤销、暂停、宽限期结束或无法取得有效租约 |

每次成功校验都会签发一个有上限的离线租约，默认 24 小时，且绝不晚于宽限期结束。总线短暂不可达时，租户可在最近一次租约内继续工作并显示告警；租约到期后失败关闭。进程重启不会凭空恢复旧租约，必须重新连接总线，避免通过重启无限延长授权。

总线按租户配置的 `warning_days` 判断即将到期。前端每分钟读取本地状态，租户后端默认每五分钟访问总线；这些轮询不包含用户资料、会议内容或音频。

## 控制面数据与管理能力

```text
authority_tenants
  name, slug, status, license_expires_at, grace_ends_at,
  warning_days, offline_lease_minutes, asr_backend_id

asr_system_settings
  backend_id, updated_by, updated_at

authority_instances
  tenant_id, name, client_id, secret_hash, status,
  last_seen_at, last_authorized_at

authority_access_tokens
  instance_id, token_hash, expires_at

authority_audit_events
  tenant_id, instance_id, event_type, detail, created_at
```

总线的“系统管理 / 授权管理”页面提供租户搜索、状态过滤、排序、分页、创建、期限调整；租户详情中可签发实例、查看最近心跳、轮换密钥、撤销和恢复实例。凭据明文只在签发或轮换响应中出现一次。

ASR 管理使用系统默认加租户覆盖。总线只保存稳定的 provider ID；租户覆盖为空时继承系统默认。租户后端在 entitlement check 响应中取得已解析的 provider ID 和配置来源，并在新建房间音频管线时应用。模型目录、云 ASR API Key 和其他 provider 参数只配置在租户自建服务端，不进入总线或浏览器。

## 安全边界

- 总线只能看到租户和部署实例元数据，不能查询租户用户、密码、会话、会议、字幕或音频。
- 浏览器不能直接执行 Client Credentials 流程；机密凭据与 Bearer Token 均停留在租户后端。
- 所有业务入口以服务端快照为准，前端阻断页只是用户体验，不是安全边界。
- 总线访问令牌为短期、不透明随机值，数据库只保存 SHA-256 哈希；实例密钥使用 Argon2。
- 租户和实例暂停或撤销、密钥轮换、授权和宽限期结束都由总线实时判定。
- 生产环境仍应在反向代理增加令牌端点限速、失败告警、审计留存和密钥托管。
- 租户实例未配置被分配的 ASR provider 时必须明确拒绝新音频管线，不能静默回退到 Demo。

## 与共享数据库多租户的关系

此前的 `tenant_id` 行级隔离方案适合由平台统一托管全部租户的 SaaS 形态，但不符合本次“租户自建服务、用户留在租户侧”的主要目标。本次每个租户部署天然拥有独立数据库和媒体目录，因此无需给现有业务表增加平台级 `tenant_id`。

如果未来同时提供平台托管版，可在托管数据面另行增加 `tenants`、`tenant_memberships`、会话租户上下文、业务表复合外键和 PostgreSQL RLS。该方案应作为另一种数据面实现，不能用客户端提交的 `tenant_id` 代替服务端会话隔离，也不能让自建租户数据回流到授权总线。

## 上线与验证

1. 先以 `bus` 模式部署总线并配置至少 16 位的 `VOICE_ELF_SETUP_TOKEN`。首次访问自动进入 `/setup`，完成环境检查、系统资料和平台管理员创建。
2. 平台管理员在 ASR 管理页确认系统默认 provider，再在授权管理页创建租户、配置可选 ASR 覆盖并签发部署实例。
3. 在租户后端注入实例凭据和独立的初始化口令，以 `tenant` 模式启动；通过租户自己的 `/setup` 创建本地系统资料及本地管理员。
4. 分别验证即将到期、宽限期、总线断网但租约有效、租约失效、实例撤销、密钥轮换和租户暂停。
5. 确认浏览器网络请求、日志和静态资源中不出现 `client_secret` 或总线 Bearer Token。
6. 确认总线数据库及审计中不包含租户用户名、会议内容、字幕和媒体路径。

关键负向测试包括：伪造 `client_id`、错误密钥、过期令牌、已撤销实例、跨实例令牌、授权到期、超过离线租约、未登录或非管理员访问总线管理接口。
