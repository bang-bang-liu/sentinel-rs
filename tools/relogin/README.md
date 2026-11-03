# Sentinel Relogin

在 Sentinel 仓库中独立构建、运行的单账号重新登录工具。桌面网页用于搜索已有账号、选择一行重新登录及查看阶段。启动服务、打开页面或查询列表不会自动登录。

## 模块

- `backend/src/login/`：密码登录、条件邮箱/TOTP 挑战、回调、session 和按需补绑 2FA。
- `backend/src/mail/`：重登专用 JSON 映射与直连取码读取器，登录明确要求邮箱验证码时才读取。
- `backend/src/accounts.rs`：读取原账号库并按 ID 更新同一行，不创建账号、不迁移表。
- `backend/src/tasks.rs`：内存任务及单任务门控，重启后记录清空。
- `frontend/`：普通 HTML/CSS/JavaScript，仅桌面布局，编译进新二进制。
- `deploy/`：独立构建、配置、启动及 systemd 模板。

编译时通过 path 引用仓库公开的 HTTP、认证构造器、Sentinel、指纹与 TOTP 模块。根 manifest、源码、注册入口无需修改。需要完整仓库源码才能构建，新二进制可独立部署。

## 构建与验证

需要 Rust、C/C++、CMake、libclang；Windows 还需 NASM 和已配置的 MSVC/LLVM 环境，与原项目 BoringSSL 要求相同。

```bash
cargo test --manifest-path tools/relogin/Cargo.toml --locked
cargo build --manifest-path tools/relogin/Cargo.toml --release --locked -p sentinel-relogin
```

Linux 可执行 `bash tools/relogin/deploy/build.sh`；Windows 在配置好工具链的终端执行 `tools/relogin/deploy/build.ps1`，加 `-Test` 运行测试。

格式化仅限本包，使用 `cargo fmt --manifest-path tools/relogin/Cargo.toml -p sentinel-relogin`；不要加 `--all`，Cargo 会同时格式化本地 path 依赖。

可选桌面浏览器验证需另外提供 Node、Playwright 及 Edge：

```bash
node tools/relogin/frontend/tests/ui-smoke.cjs
```

测试只使用合成账号和本地 HTTP 模拟；不会访问真实账号或认证站点。部署和运行无需 Node。

## 独立配置与启动

复制 `deploy/login.env.example` 为工具自己的私有配置。程序只显式加载 `--env-file`，不自动查找 `.env`，不读取注册邮箱配置。进程启动时清除继承的 HTTP 代理变量，认证链代理只由 `LOGIN_PROXY` 指定；邮箱链始终直连。

| 设置 | 说明 |
| --- | --- |
| `LOGIN_DB_PATH` | 已有 SQLite 账号库；必须已经存在 |
| `LOGIN_LISTEN` | 回环地址，默认 `127.0.0.1:8788` |
| `LOGIN_PROXY` | 认证链 HTTP/SOCKS 代理，空值直连 |
| `LOGIN_MAIL_MAP_FILE` | 重登专用 JSON 文件路径 |
| `LOGIN_MAIL_API_TOKEN` | 取码 GET 请求可选 Bearer 口令 |
| `LOGIN_OTP_MAX_WAIT` / `LOGIN_OTP_POLL_INTERVAL` | 邮箱取码最长等待/轮询秒数，默认 `180` / `3` |
| `LOGIN_ADMIN_TOKEN_FILE` | 独立管理口令文件，不存在时随机生成 |
| `LOGIN_LOG` | `error/warn/info/debug/trace`；只记录本工具阶段摘要 |

```bash
tools/relogin/target/release/sentinel-relogin serve --env-file /path/to/login.env
```

UI 口令文件在 Linux 要求 `0600`。口令仅保存在当前浏览器会话存储中，退出管理后清除。API 不返回密码、Token、TOTP 密钥、Cookie、取码地址或代理凭据。

## 邮箱映射

JSON 对象按邮箱精确匹配，示例只含虚构地址：

```json
{"first@example.invalid":"https://mail.example.invalid/inbox/first"}
```

仅进入邮箱挑战时读取文件，每次重新读取。修改 JSON 后下一次挑战立即使用新映射；不改变注册邮箱接口。读取器先记录旧验证码，再请求登录 OTP resend，随后等待新验证码，最后提交登录验证码。

支持原取码页 `<article>` / `class="meta"` / `class="code"` 格式，或纯六位码，或 JSON `{"code":"123456"}` / `{"otp":"123456"}`（也支持 `data` 对象下这两个字段）。HTML 卡片按时间/基线过滤；纯文本或 JSON 必须出现与基线不同的验证码。取码 URL 直接返回正文，不跟随重定向。未来映射接口变化只需替换 `mail/mapping.rs`；正文格式变化只改 `reader.rs`。

## 登录与保存约定

密码登录 → 服务端明确要求时处理邮箱或 TOTP → workspace / callback → session 取得 Access Token。数据库已有 TOTP 密钥时跳过补绑；没有密钥时执行原 enroll → TOTP → activate 顺序，再读一次 session，最后导出本次 Cookie。

按已确认约定，不查询服务端 2FA 状态、不调用 `mfa_info`、不增加激活复查，沿用原注册代码对 activate 请求完成的判断。没有额外完整登录、refresh_token 或 `/oauth/token` 交换。未知/重复页面停止，不回退注册。

最终单条 `UPDATE ... WHERE id=?` 覆盖 Access Token、整份新 Cookie、状态和更新时间；本次补绑时才更新 TOTP 密钥。ID、邮箱、密码、创建时间保持原值；不调用账号 upsert。失败不以空值覆盖原凭据。联网过程不持有数据库事务。

同一服务只运行一个任务，忙时返回 409。页面刷新通过 `/api/relogin/current` 恢复当前任务显示，不重新登录。任务在内存中保留最近 100 条，服务重启不恢复或重跑。补绑已在服务端完成后若最终 session/保存失败，本版本不提供额外补偿流程，沿用计划边界。

## API

`/api/*` 均要求 `Authorization: Bearer <独立管理口令>`，并限制 Host/Origin 为回环站点。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/health` | 健康及版本 |
| GET | `/api/accounts?q=&account_id=&page=1&page_size=50` | 搜索、分页及凭据存在标记 |
| POST | `/api/relogin` | `{"account_id":123}`，202 返回任务 ID |
| GET | `/api/relogin/current` | 最近任务，供刷新页面恢复 |
| GET | `/api/relogin/{task_id}` | 实际阶段、成功/失败及摘要 |

账号不存在为 404，同一服务有任务运行为 409。

## SG 部署

将完整仓库放到 `/opt/sentinel-relogin/source` 构建。二进制放到 `/opt/sentinel-relogin/releases/<版本>/bin/`，`current` 链接指向该 release；复制 `deploy/start` 为 `/opt/sentinel-relogin/start`，设置可执行权限。

私有配置为 `/opt/sentinel-relogin/.env`，数据目录 `/var/lib/sentinel-relogin` 归 `sentinel:sentinel` 所有、权限 `0700`。其中保存 `admin.token` 和 `mail-map.json`，私有文件权限 `0600`。原账号库仍是 `/var/lib/sentinel-rs/sentinel-rs.db`。

将 `deploy/sentinel-relogin.service` 安装到 `/etc/systemd/system/`，然后：

```bash
systemctl daemon-reload
systemctl enable --now sentinel-relogin
systemctl status sentinel-relogin --no-pager
```

本机建立隧道：

```powershell
ssh -N -o ExitOnForwardFailure=yes -L 8788:127.0.0.1:8788 sg
```

访问 `http://127.0.0.1:8788`。另开终端读取口令：

```powershell
ssh sg "cat /var/lib/sentinel-relogin/admin.token"
```

停止新服务即可停止此工具；原注册程序和优惠看板保持各自入口。回滚只切换本工具 release 或停止服务，不将已经更新的账号凭据恢复成旧值。禁止使用原 `/opt/sentinel-rs/start` 来启动此工具，它会执行注册。
