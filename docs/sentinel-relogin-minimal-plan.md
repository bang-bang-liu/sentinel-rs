# Sentinel 重新登录与简单 UI：最小变更计划书

版本：v1.0

日期：2026-11-03（北京时间）

状态：已按本计划实施。实际部署、验证和限制见[实施记录](../tools/relogin/VERIFICATION.md)，下文保留计划基线。

后续邮箱自动解析方案见[重登自动获取邮箱取码地址：最小变更计划书](sentinel-relogin-auto-mail-plan.md)，已在 0.1.1 实施，实际范围见上述实施记录。

## 1. 目标与方案

在 `sentinel-rs` 仓库内增加独立工具 `sentinel-relogin`：用户在桌面 Web UI 搜索并指定已有账号，执行密码登录，按登录响应处理邮箱验证码或已有 TOTP 验证，必要时补绑 2FA，最后更新原账号的 Access Token、Cookie 等字段。

采用“同仓库、独立 Cargo 子项目、独立可执行程序、独立服务”的方式。编译时引用 `sentinel-rs` 已公开的协议模块；运行时不启动原注册程序。原注册源码、请求接口、配置、启动命令与数据库表结构均无需修改。

| 项目 | 最小方案 |
| --- | --- |
| 界面 | 普通 HTML/CSS/JavaScript，仅电脑端 |
| 后端 | Rust + Axum，静态页面编译进新二进制 |
| 操作范围 | 按账号 ID 发起单账号重登，同一服务同时只运行一个任务 |
| 数据来源 | 原 SQLite `accounts` 表 |
| 写入范围 | 原账号行的登录凭据、状态和更新时间 |
| 邮箱取码 | 重登专用映射与读取模块，只在明确要求邮箱验证码时调用 |
| 任务进度 | 内存记录，UI 轮询；首版不增加任务数据库 |
| 部署 | 新目录、新服务、新端口，通过 SSH 隧道访问 |

## 2. 已确认的业务约定

以下约定直接作为实施要求，不额外增加服务端检查或新的确认流程：

1. 数据库 `totp_secret` 非空表示已有 2FA，保留并用于登录挑战；为空表示没有启用，登录后执行补绑。
2. 补绑复用现有 enroll → 本地计算 TOTP → activate 流程，沿用当前完成判定。不调用 `mfa_info`，不增加激活后复查，不新增待激活状态。
3. 密码登录为主流程。只有登录响应明确要求邮箱验证码，才进入独立邮箱分支；收到 TOTP 挑战时提交已有密钥生成的动态码。
4. 补绑接口需要有效 Access Token，因此先完成登录并在内存取得 Token，再补绑。补绑后补读一次 session，最后保存账号。
5. 不额外执行第二轮完整登录，也不要求最终 Token 字符串必须与旧值不同。
6. 本期不涉及 refresh_token，不调用 `/oauth/token` 作为刷新机制。

## 3. 改动范围与目录

所有新增程序文件集中在 `tools/relogin/`，本计划书保存在 `docs/`。

```text
sentinel-rs/
  Cargo.toml / Cargo.lock                保持现状
  src/                                  原注册和协议源码保持现状
  tools/
    relogin/
      Cargo.toml                        独立 workspace，成员 backend
      Cargo.lock                        新工具自己的依赖锁文件
      .gitignore                        排除新工具配置、构建和私有数据
      README.md                         构建与使用说明
      backend/
        Cargo.toml                      新程序包 sentinel-relogin
        src/
          main.rs                       启动 serve，载入独立配置
          config.rs                     LOGIN_* 设置
          api.rs                        HTTP 接口、管理口令及静态资源
          tasks.rs                      单任务调度、阶段进度、执行结果
          login/
            mod.rs                      重登流程编排
            protocol.rs                 登录响应解析、回调与 session 收尾
            twofa.rs                    已有 TOTP 验证、缺失密钥时补绑
          mail/
            mod.rs                      重登专用邮箱分支入口
            mapping.rs                  邮箱 → 取码地址映射
            reader.rs                   请求取码地址、解析与轮询验证码
          accounts.rs                   按 ID 读取、搜索和定向 UPDATE
        tests/
          login.rs                      密码、TOTP、邮箱分支与写库测试
      frontend/
        index.html / app.js / styles.css 桌面账号列表和任务进度
        tests/ui-smoke.cjs               可选桌面浏览器验证
      deploy/
        build.sh / build.ps1            只构建新 workspace
        start                           只启动 sentinel-relogin
        login.env.example               空凭据配置模板
        mail-map.example.json           合成邮箱映射示例
        sentinel-relogin.service        独立 systemd 服务
```

`tools/relogin/Cargo.toml` 设置独立 workspace。`backend/Cargo.toml` 通过 `sentinel-rs = { path = "../../.." }` 引用仓库根库。依赖解析、Cargo.lock 和 target 均属于新 workspace；不将 Axum 等依赖加入根项目。

| 现有位置 | 处理方式 |
| --- | --- |
| 根 `Cargo.toml`、`Cargo.lock` | 不修改 |
| `src/main.rs`、`src/lib.rs`、`src/ffi.rs` | 不修改，不给原注册入口添加 Web 服务 |
| `src/flow/mod.rs` | 不修改，不调用 `register_once` 或 `live_register` |
| `src/flow/email.rs`、注册使用的邮箱接口 | 不修改、不作为重登邮箱客户端调用 |
| `src/flow/auth.rs`、`authorize.rs` | 引用公开构造器与函数，不修改其实现 |
| `src/flow/http.rs`、`sentinel.rs`、`twofa.rs`、指纹模块 | 编译时引用公开能力，不修改原实现 |
| `src/flow/store.rs` | 不修改；新适配器自行定向更新账号行 |
| 原启动脚本、原 `.env`、原活动 release | 不修改、不替换 |
| `sentinel-rs-offers`、`offers.db` | 无需改动 |

这是编译时源码依赖，新工具构建需要仓库根源码；部署后的新二进制可独立运行，不调用原二进制，也不共享运行中的 HTTP 客户端。

现有 `complete_oauth`、重定向收尾和 Cookie 合并函数不是公开接口。最小方案是在新工具 `login/protocol.rs` 内参考并适配必要部分，只保留 workspace → callback → session 与本次 Cookie 导出，不为此修改原模块可见性，不复制 OAuth token 交换分支。

## 4. 重登执行流程

1. UI 提交 `account_id`，后端读取该行的邮箱、密码、`totp_secret`。不创建邮箱、不生成新密码。
2. 为本次账号建立独立 HTTP 客户端和 Cookie Jar，复用公开的 warmup、signin、authorize 能力，进入密码登录。
3. 使用已有 `login_password_verify` 请求构造器提交密码；根据响应中的页面类型、继续地址和挑战信息进入下一步。
4. 明确要求邮箱验证码时，调用 `mail::resolve_and_poll(email)`；要求 TOTP 时，使用原 `totp_secret` 计算动态码并调用已有 `mfa_verify`。验证后继续处理登录响应。
5. 完成 workspace 选择及登录回调，消费 callback 建立网页会话，通过 `/api/auth/session` 取得 Access Token。
6. 原 `totp_secret` 为空时，执行现有 enroll → TOTP → activate 顺序，在内存保留新密钥；然后补读一次 session。原密钥非空时跳过补绑。
7. 从本次客户端导出最终 Cookie，与最终 Access Token、需要新增的 TOTP 密钥一起定向更新原账号行。
8. UI 将同一条账号记录刷新为本次执行结果，显示更新时间；不新增账号行，不自动发起优惠检测。

登录响应解析需要新增，但登录相关请求构造已存在。密码校验、邮箱验证、TOTP 挑战以及 callback 的具体字段，以后续单账号联调响应为准；未知页面停止当前任务并显示阶段，不转去执行注册。

启动服务、打开页面、搜索账号、刷新进度均不会登录账号。只有点击指定账号的重登按钮才创建任务。

## 5. 邮箱验证码分支独立设计

### 5.1 调用边界

```text
登录响应明确要求邮箱验证码
  → mail/mapping.rs 按该账号邮箱解析取码地址
  → mail/reader.rs 读取取码内容并轮询本次验证码
  → login/protocol.rs 向登录认证接口提交验证码
  → 返回登录流程
```

重登分支不调用注册的 `create_mailbox`、`MailApiClient`，不读取或修改 `REG_MAIL_*`，不修改注册的发码/取码实现，也不依赖注册 `email_pool` 必须保存映射。登录认证端需要的 OTP 提交可以使用已有公开请求构造器，注册端的调用方式不变。

### 5.2 初始映射方式

目前尚未提供实际映射接口的请求和响应契约。为使首版可以独立落地，最小默认实现使用私有 JSON 映射文件，内容形如：

```json
{
  "first@example.invalid": "https://mail.example.invalid/inbox/first"
}
```

文件路径由 `LOGIN_MAIL_MAP_FILE` 指定。只有进入邮箱验证码分支才读取，并按邮箱精确匹配。更新映射文件后，下一次进入该分支即读取新值；无需改注册代码。

`mapping.rs` 对登录流程只提供 `resolve(email) -> code_url`。后续若改成远程 API、数据库或其他映射，只替换此实现及其独立配置，不改变登录流程、UI 或注册模块。

`reader.rs` 独立负责取码地址的 HTTP 请求、所需鉴权、正文解析与等待。新映射若连取码返回格式也变化，只改 `mapping.rs`、`reader.rs`。纯验证码解析函数可按需要引用原 `otp` 的计算/解析能力，不引用原邮箱业务客户端。

本期邮箱链路使用独立直连客户端；账号认证链路代理由 `LOGIN_PROXY` 控制。没有邮箱挑战时不查映射、不访问邮箱服务，也不要求配置取码服务。

若希望首版直接对接某个已有映射 API，实施前补充其地址、请求参数、返回示例及鉴权方式，即可替换上述默认文件实现；这些内容不会写入注册配置。

## 6. 原账号记录如何更新

无需数据库迁移，不新增账号列或任务表。新 `accounts.rs` 直接读取既有账号，在最终保存阶段执行一条 `UPDATE accounts ... WHERE id = ?`，不调用全字段 upsert，也不执行 INSERT。

| 字段 | 更新规则 |
| --- | --- |
| `access_token` | 覆盖为本次最终取得的 Access Token |
| `cookies` | 用本次客户端最终导出的 Cookie 整份覆盖旧值 |
| `totp_secret` | 原来已有则不改；本次补绑时写入新密钥 |
| `status` | 成功取得登录凭据后设置为 `registered` |
| `updated_at` | 更新为本次完成时间 |
| `id`、`email`、`password`、`created_at` | 保持原值 |

Cookie 沿用当前合并 `auth.openai.com` 与 `chatgpt.com` Cookie 头的存储格式；只合并本次会话的数据，不掺入旧 Cookie。不新增独立 session_token 列。

登录失败时不以空 Token 或空 Cookie 覆盖旧账号数据，失败阶段和原因保存在当前任务结果中。首版不增加任务恢复、待激活密钥表、额外 2FA 状态字段或自动补偿流程。

写库只占用最终短暂 UPDATE 的时间，联网期间不持有账号库事务。`sentinel-rs-offers` 下一次检测时会读取新 Token；其已经运行中的检测保持原有行为。

## 7. 桌面 UI 与 API

### 7.1 首版页面

| 区域 | 内容 |
| --- | --- |
| 管理入口 | 输入新服务自己的管理口令 |
| 搜索区 | 邮箱关键词或精确账号 ID |
| 账号表格 | ID、邮箱、原账号状态、密码/Token/TOTP 是否存在、更新时间、重登按钮 |
| 当前任务 | 账号、执行阶段、开始时间、成功/失败及简短原因 |

只有单行“重新登录”操作，同一服务最多一个任务；忙碌时按钮禁用，重复提交返回任务忙。首版不做全量重登、多选批量、自动刷新 Token、定时任务、密码编辑或映射编辑器。

任务阶段显示为“建立会话 / 密码登录 / 邮箱验证 / TOTP 验证 / 获取 Token / 补绑 2FA / 更新账号 / 完成”，实际跳过的阶段不要求执行。界面显示阶段，不虚构百分比。任务期间每 1 秒读取本地任务接口；不会因轮询重新发起登录。

前端只接收字段是否存在、账号元数据和结果摘要，不返回密码、完整 Token、TOTP 密钥、Cookie 或取码地址。任务记录保存在内存，服务重启后清空，不自动恢复或重新执行登录。

### 7.2 接口

所有 `/api/*` 使用新服务管理口令；首版只绑定回环地址，沿用已有优惠看板的 Host/Origin 校验及口令使用方式。

| 方法 | 路径 | 行为 |
| --- | --- | --- |
| GET | `/health` | 服务健康检查 |
| GET | `/api/accounts?q=...&account_id=...&page=1&page_size=50` | 搜索和分页列出账号元数据 |
| POST | `/api/relogin` | 请求体 `{"account_id":123}`，返回 `202 {"task_id":"..."}` |
| GET | `/api/relogin/{task_id}` | 返回账号、当前阶段、任务状态及结果摘要 |

账号不存在返回 404，已有任务返回 409。任务运行在后端后台任务中，POST 不等待整个登录结束。账号列表按 ID 标识条目，完成后刷新同一行。

## 8. 依赖、配置与部署

### 8.1 依赖

| 依赖 | 用途 |
| --- | --- |
| `sentinel-rs`，本地 path 引用 | HTTP、认证请求、Sentinel、指纹和 TOTP 能力 |
| `axum = "0.8"` | 新 Web 服务及 JSON API |
| `tokio = "1"`，开启 `net/sync/signal/rt-multi-thread/macros/time` | HTTP 监听、后台任务、阶段共享与退出 |
| `rusqlite = "0.32"`，bundled | 新账号适配器，读写现有 SQLite |
| `serde`、`serde_json`、`anyhow`、`uuid`、`rand`、`dotenvy`、`tracing` | 配置、序列化、任务 ID、口令和日志 |

除 Axum 及 Web 所需特性外，主要沿用项目已经使用的依赖版本。新锁文件独立生成并在后续使用 `--locked`。构建仍需要现有 BoringSSL/C/C++/CMake/libclang 工具链。

前端静态资源通过 `include_str!` 编译进二进制，不引入 React/Vue、前端打包器、Node 运行时、Tauri、WebView 或桌面图形环境。可选浏览器测试单独使用 Playwright，部署程序不依赖它。

### 8.2 独立配置

| 设置 | 用途/初始值 |
| --- | --- |
| `LOGIN_DB_PATH` | 现有账号库绝对路径 |
| `LOGIN_LISTEN` | `127.0.0.1:8788`，与优惠看板端口分开 |
| `LOGIN_PROXY` | 账号认证链路代理；空值直连 |
| `LOGIN_MAIL_MAP_FILE` | 重登专用邮箱映射文件 |
| `LOGIN_MAIL_API_TOKEN` | 新邮箱读取器需要时单独配置，不读取注册邮箱 Token |
| `LOGIN_OTP_MAX_WAIT` / `LOGIN_OTP_POLL_INTERVAL` | 邮箱验证码等待时间与轮询间隔，初始 180 秒 / 3 秒 |
| `LOGIN_ADMIN_TOKEN_FILE` | 新 UI 管理口令文件 |
| `LOGIN_LOG` | 默认 info，只输出阶段及结果摘要 |

新程序只显式加载自己的 `--env-file`，不自动查找根项目 `.env`。账号代理在后端配置，首版不追加 UI 代理编辑功能。既有底层模块的环境读取行为保持现状，由新服务运行环境单独设置。

### 8.3 SG 部署布局

```text
/opt/sentinel-relogin/
  source/                       构建用的完整 sentinel-rs 源码快照
    tools/relogin/              新工具 workspace
  releases/<版本>/bin/sentinel-relogin
  current -> releases/<版本>
  start                         仅启动新工具
  .env                          新工具配置
/var/lib/sentinel-relogin/
  admin.token                   新管理口令
  mail-map.json                 私有邮箱映射
```

由现有 `sentinel` 用户运行 `sentinel-relogin.service`，读取并更新现有账号库；原注册程序及优惠看板保持各自入口。构建和部署均在新目录执行，不编辑 `/opt/sentinel-rs/current`，不使用会触发注册的 `/opt/sentinel-rs/start`。

拟定构建入口：

```bash
cargo build --manifest-path tools/relogin/Cargo.toml --release --locked
cargo test --manifest-path tools/relogin/Cargo.toml --locked
```

拟定访问方式：

```powershell
ssh -N -o ExitOnForwardFailure=yes -L 8788:127.0.0.1:8788 sg
```

随后访问 `http://127.0.0.1:8788`。以上是后续实施命令，本轮不执行部署或重登。

## 9. 实施顺序与验收

| 阶段 | 工作 | 完成标准 |
| --- | --- | --- |
| A：目录与数据适配 | 新 workspace、独立配置、账号查询和定向 UPDATE | 原根 manifest/lock 及注册源码无变动；临时库更新同一行 |
| B：重登主链 | 接通已有认证构造器，新增响应解析、回调与 session 收尾 | 密码登录、TOTP 分支和无密钥补绑按约定执行 |
| C：独立邮箱分支 | 新映射文件/适配器与取码读取器 | 仅邮箱挑战调用；修改映射只影响重登分支 |
| D：简单 UI | 口令、搜索、单行重登、阶段进度 | 可明确选择账号，重复提交不会产生第二个任务 |
| E：SG 发布 | 独立构建、服务和隧道，验证单账号 | 服务正常；账号 ID 不变，凭据更新，原注册入口不变 |

必要验证集中在业务分支与数据结果，不额外扩展防御性框架：

1. 密码登录无需邮箱验证码时，不解析映射、不访问邮箱服务。
2. 明确邮箱挑战时，按本次邮箱映射取码；更换映射不改变注册接口与配置。
3. 原密钥非空时，按要求完成 TOTP 验证并跳过补绑；为空时执行一次现有 enroll/activate 顺序，且不请求 `mfa_info` 或激活复查。
4. 补绑后补读 session，最终保存本次 Cookie，不混入旧 Cookie；不调用 refresh_token 或 OAuth token 交换。
5. 成功后仅更新约定字段、保留账号 ID/密码/创建时间；普通登录失败保留原凭据。
6. UI 可搜索、指定单账号、查看阶段和结果；API 不回传账号秘密。
7. 对比实施前后原注册源码、配置与入口，执行已有离线测试确认注册行为未受改动。

在线验证按单账号进行，分别验证已有密钥与缺少密钥的实际分支；未完成某一分支实测时，在交付记录中明确注明。回滚只切回新工具自己的旧 release 或停止新服务，不将已成功更新的账号凭据恢复成旧值。

## 10. 本期交付边界

交付独立重登程序、桌面 Web UI、重登专用邮箱映射与取码模块、部署脚本和验证记录。实施不修改注册流程的任何接口，不迁移账号表，不改优惠查询工具。

目前需要落实的外部信息只有实际邮箱映射/取码契约；在未提供前，可用本计划约定的独立 JSON 映射完成最小版本，其余模块可并行按上述顺序实施。
