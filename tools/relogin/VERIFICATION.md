# 实施与验证记录

## 服务器源码发布检查

本次同步以 SG 上运行的注册发布源码及已部署的独立重登源码为准。发布副本已移除实际邮箱服务域名、账号标识及库存统计，排除私有配置、数据库、日志和源码压缩包。原服务器运行配置与数据保持现状。

注册源码的 release 离线回归共 80 项通过，包括邮箱直连、注册代理路由和代理日志脱敏检查。重登工具位于本仓库的 `tools/relogin/`，继续独立构建与部署；本次在 SG 隔离测试目录针对同步后的根源码执行其 23 项 release 测试，全部通过。

## 0.1.1：自动获取邮箱取码地址

日期：2026-11-03（北京时间）。方案见[最小变更计划书](../../docs/sentinel-relogin-auto-mail-plan.md)。

### 实现与测试

- Workbench 新增独立签发接口、读取接口和 `mail_api_relogin_accesses` 表；沿用原账号/标签范围，原领取、公开链接及 IMAP 实现保持现状。
- 重登工具新增 `mail/resolver.rs`，配置 `LOGIN_MAIL_RESOLVE_URL` 和 `LOGIN_MAIL_RESOLVE_TOKEN` 后自动签发本轮链接；保留未配置 URL 时的 JSON 模式。
- `Mailbox::prepare/poll` 和登录编排保持现状。只有明确邮箱挑战才请求接口；签发密钥只附在 POST，取码 GET 使用链接 Token。邮箱继续直连，客户端使用自身 `sentinel-relogin/0.1.1` User-Agent。
- 没有新增运行依赖、UI 改动或账号数据库迁移。

| 检查 | 结果 |
| --- | --- |
| Workbench 全量测试 | 78 项通过，包含 10 项新增重登接口测试 |
| 重登工具 release 测试 | 23 项通过，包含 6 项新增远程解析/完整模拟登录测试 |
| 干净原项目 release 离线回归 | 77 项通过 |
| 正式 Rust 邮箱读取器从 SG 请求生产接口 | 签发与 HTML 取码成功，无账号登录或写库 |
| 原本地注册文件基线 | 29 个已跟踪文件保持实施前内容，原有未提交修改保留 |

新增测试覆盖重复签发、跨日及过期、原库存和链接不变、精确邮箱及归属范围、密钥隔离、慢同步、API 失败不回退 JSON，以及“解析 → 基线 → 登录 resend → 新码 → 验证”的完整模拟编排。

### 阿里云 Workbench 部署

- SSH 主机别名：`AlibabaCloud`；目录：`/root/icloud-create-workbench`。
- 服务：Docker Compose 的 `app`，容器 `icloud-create-workbench-app-1`。
- 在原运行镜像上仅覆盖本次后端增量，前端、运行依赖、数据挂载及其他既有配置保留。
- 新接口通过现有邮箱域名的 `/api/v1/relogin/` 提供；重登密钥在两台服务器的私有配置中，不进入源码或浏览器。
- 迁移前使用 SQLite 在线备份创建 `/app/data/backups/relogin-auto-mail-20261103/workbench-before.db`；部署快照保存在 `/root/sentinel-relogin-mail-deploy/20261103/`。
- 部署前后数据库完整性均为 `ok`，原邮箱记录、领取记录及公开访问记录的完整摘要一致。
- SG 对同一账号连续两次签发成功，返回不同链接，两次取码均成功；随后正式 Rust 客户端再次验证签发及读取成功。

公网入口不需要调整。Python 默认 User-Agent 的探测请求被入口返回 1010；使用工具自身 User-Agent 后通过，正式 Rust 客户端已实测通过。

### SG 部署与单账号验证

- release：`/opt/sentinel-relogin/releases/0.1.1-20261103/`。
- 二进制 SHA256：`a6d1fb139ecff184442dfc531c5ad6d193770834d14625204647f73e42c91f88`。
- `sentinel-relogin.service` 已重启且 active，回环健康接口返回 `{"status":"ok","version":"0.1.1"}`。UI、端口和 SSH 隧道方式沿用原部署。
- 原 release 保留；旧配置备份为 `/opt/sentinel-relogin/env-before-0.1.1`，原 release 路径记录在 `/opt/sentinel-relogin/CURRENT_BEFORE_0.1.1`。
- 选定存量账号单次重登成功，经过密码、已有 TOTP、callback/session，最终更新 Access Token 与 Cookie；本次没有补绑 2FA。
- 全表核对仅选定账号变化，原账号 ID 集合保持一致；原 SG 注册入口、配置及发布源码的文件摘要一致。
- 私有验证记录位于 `/var/lib/sentinel-relogin/auto-mail-*.json`，不包含取码链接或密钥。

本次真实登录没有触发邮箱挑战。因此生产取码接口和正式读取器已经验收，真实“登录要求邮箱码 → 接收新邮件 → 提交登录验证码”仍待实际出现挑战时验证；完整编排目前由本地 HTTP 模拟覆盖。没有为了触发该分支执行批量登录或修改认证流程。

以下保留 0.1.0 的历史交付记录；其“尚无自动映射”的描述属于当时状态，0.1.1 已按上述方案改为 API 自动获取。

## 0.1.0 历史记录

日期：2026-11-03，版本：sentinel-relogin 0.1.0。

## 交付范围

新增代码全部位于 `tools/relogin/`，分为 Rust 后端、独立邮箱模块、桌面前端和部署脚本；设计基线位于 `docs/sentinel-relogin-minimal-plan.md`。根注册源码、manifest、锁文件、启动脚本和原配置不纳入本次修改。

SG 构建使用仓库基线 `2c1b60f4f12bbd07900f05712988d920aa559002` 的干净根源码，加本工具文件。新工具没有依赖工作区尚未提交的原注册改动。实现只引用公开的协议构造、HTTP、Sentinel、指纹及 TOTP 能力，不调用注册编排或注册邮箱客户端。

## 离线与桌面验证

| 检查 | 结果 |
| --- | --- |
| 密码、邮箱、已有 TOTP、补绑与 session 回读等工具测试 | 17 项通过，debug 与 release 均通过 |
| 桌面 Edge / Playwright | 通过：口令、搜索、指定账号、忙碌禁用、刷新恢复、原行更新、退出与布局 |
| 干净根库已有离线测试 | 77 项通过 |
| 发布源码审计 | 已暂存源码扫描通过，无真实邮箱、Token、Cookie、代理口令、私有映射或数据库 |
| 原工作区差异 | 与实施前摘要一致，原有未提交改动保留 |

工具测试覆盖：没有邮箱挑战时不触碰映射；邮箱映射按邮箱精确匹配且重新读取；邮箱读取器基线过滤、直连及独立鉴权；TOTP 因子解析；workspace 与 callback；本地无密钥时 enroll → activate → 最终 session；禁止注册、`mfa_info`、激活复查和 `/oauth/token`；API 单任务门控及鉴权；成功定向更新原行、失败保留旧凭据、列表不返回秘密。

激活步骤按用户约定沿用原代码的请求完成判定，没有另加 HTTP 成功状态判定或服务端复查；测试也覆盖此约定。

## SG 真实单账号验证

- 已有 TOTP 密钥的一个存量账号：密码 → TOTP → callback → session 成功，更新了本次 Access Token 和 Cookie。
- 缺少本地 TOTP 密钥的另一个存量账号：密码接口返回 HTTP 401，任务停止，未修改该账号记录。因此真实补绑链路尚未完成联调；离线模拟覆盖补绑顺序与最终 session 回读。
- 比较完整账号表：只有成功选定的账号行发生变化，账号行数保持一致；所有账号 ID、邮箱、密码和创建时间均保留。没有执行批量重登。
- 原 SG 注册源码、原启动入口与原 `.env` 摘要保持一致，原优惠看板服务仍为 active。

## 独立部署

- 程序：`/opt/sentinel-relogin/current/bin/sentinel-relogin`
- 正式 release：`/opt/sentinel-relogin/releases/0.1.0-20261103/`
- 源码：`/opt/sentinel-relogin/source/tools/relogin/`
- 启动：`/opt/sentinel-relogin/start`
- 服务：`sentinel-relogin.service`，用户 `sentinel`
- 监听：`127.0.0.1:8788`
- 配置：`/opt/sentinel-relogin/.env`
- 私有管理口令：`/var/lib/sentinel-relogin/admin.token`
- 私有邮箱映射：`/var/lib/sentinel-relogin/mail-map.json`

正式 release 健康检查返回 `{"status":"ok","version":"0.1.0"}`，服务已启用开机启动；未带口令的 API 请求返回 401。部署后账号列表正常，启动时没有自动创建重登任务。

认证代理由新配置的 `LOGIN_PROXY` 控制，初始值从 SG 原认证代理只读复制；后续配置各自独立。邮箱读取器直连。真实配置及验证数据均在源码目录之外，未上传仓库。

## 尚未真实验证的分支

SG 目前没有可直接使用的账号到取码地址映射，因此独立 `mail-map.json` 初始为 `{}`，需按已确认的 JSON 格式填写真实账号与取码 URL。邮箱接口及取码解析已在本地 HTTP 模拟下验证，真实邮箱挑战尚未联调。

无本地 TOTP 密钥的实测账号在密码校验阶段失败，尚未实际执行补绑。工具不会因为此失败改用注册流程，也不会查询服务端 2FA 状态。补充有效账号/邮箱映射后，可通过同一 UI 指定单账号继续验证，无需改注册代码。
