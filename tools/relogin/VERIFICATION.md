# 实施与验证记录

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
