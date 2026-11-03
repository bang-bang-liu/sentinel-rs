# Sentinel Offers 使用与部署

设计版本 v1.1；当前程序版本 0.1.1。两者是不同的版本号。构建和线上实测结论记录在 `verification.md`。

## 目录与边界

源码仓库可克隆到任意工作目录，以下目录均为部署模板示例。

```text
/opt/sentinel-offers/
  source/                     完整独立源码
    backend/                  Rust 后端、测试、结果库迁移
    frontend/                 HTML、CSS、JavaScript UI
    deploy/                   构建、安装、启动和服务模板
    docs/                     设计与操作文档
  releases/<版本>/bin/         独立二进制
  current -> releases/<版本>
  start                       手动入口
  .env                        OFFERS_* 配置
/var/lib/sentinel-offers/
  offers.db                   优惠结果与任务历史
  offers.lock                 单进程写入锁
  admin.token                 管理口令，0600
```

前后端通过 `/api/` JSON 接口交互。前端源码可以单独维护；当前发布方式将静态文件编译进后端，修改 UI 后需要重新构建。后续可以改为同源静态托管，无需修改查询业务层。当前不启用跨域 API。

只读来源：`/var/lib/sentinel-rs/sentinel-rs.db`。工具不读原 `.env`，不调用原二进制，不刷新登录或绑定 2FA。启动和刷新看板均不触发上游检测。

## 打开看板

先在 SSH 配置中将 `offers-host` 设置为自己的服务器别名。在 Windows PowerShell 保持以下 SSH 命令运行：

```powershell
ssh -N -o ExitOnForwardFailure=yes -L 8787:127.0.0.1:8787 offers-host
```

浏览器打开 `http://127.0.0.1:8787`，输入管理口令。另开一个终端读取口令，不要将它贴到聊天或公共日志中：

```powershell
ssh offers-host "cat /var/lib/sentinel-offers/admin.token"
```

页面选中账号后点击“检测所选账号”，或主动点击“全量检测”。全量检测包含源库全部当前账号，缺少令牌的记录会直接标明原因。没有自动定时检测。空闲页面每 15 秒刷新本地记录，任务执行时每 2 秒刷新。

四个活动分别展示。半价统计只要一种期限明确符合就计入；只有三种都明确不符合时才判定没有半价。异常、未知状态、活动 offline、403 和 401 均不等于“不符合”。历史可信结果会在详情中标明旧时间，不能充当当前结论。

## 命令行

SSH 进入服务器 后执行。所有写入命令使用 `sentinel` 用户，避免生成 root 所有的数据库。

```bash
# 只读列出账号范围，不创建结果库或发送活动请求
runuser -u sentinel -- /opt/sentinel-offers/start check --all --dry-run

# 查看新服务状态
systemctl status sentinel-offers --no-pager

# 最近日志，不含 Token 或上游原文
journalctl -u sentinel-offers -n 40 --no-pager
```

看板服务已占用结果库写入锁时，CLI `check` 会拒绝运行。此时在页面发起查询；需要独立 CLI 批次时，先停止新看板服务：

```bash
systemctl stop sentinel-offers
runuser -u sentinel -- /opt/sentinel-offers/start check --account-id 123
# 多账号：--account-id 123 --account-id 456
# 全量：--all
systemctl start sentinel-offers
```

`123` 为示例账号 ID，请从看板或 dry-run 输出中获取。检测退出码：0 = 全部活动得到明确结论，2 = 含未知、缺凭据或中断，1 = 任务级错误。明确“不符合”属于成功查询。

导出不触发网络请求，也不占写入锁，服务运行中可用：

```bash
runuser -u sentinel -- /opt/sentinel-offers/start export \
  --task-id <任务ID> --format csv --output /var/lib/sentinel-offers/report.csv
```

可用 `--format json`。默认拒绝覆盖已有文件，显式 `--overwrite` 才允许覆盖普通导出目标；源库、结果库、辅助文件和管理口令禁止覆盖。导出时间为 ISO 8601 UTC，页面显示本地时区。导出不含账号令牌、密码、Cookie 或 TOTP。

## 配置

只使用本工具显式参数或独立配置 `/opt/sentinel-offers/.env`：

```dotenv
OFFERS_PROXY=
OFFERS_HTTP_EMULATION=Chrome149
OFFERS_LOG=info
```

看板中的“查询代理”可展开填写代理地址，点击“保存代理”后，从下一次手动检测生效。支持 `http://`、`https://`、`socks5://`、`socks5h://`，可带代理用户名和密码；只有 `host:port` 时按 HTTP 处理。点击“切换直连”保存直连模式。保存只校验地址格式，不发起网络检测，也不创建优惠任务。

代理由运行查询后端的机器连接。远程部署时，`127.0.0.1` 指远程机器自身，不能直接指向浏览器所在电脑的本地代理。

看板设置存入独立 `offers.db` 的 `offer_meta.query_proxy`，重启后保留；不会写回源账号库、原项目配置或本工具的 `.env`。数据库可能包含代理认证信息，应按私有配置保管。API 和页面只回显去掉用户名、密码的代理地址，输入框保存后清空；浏览器不持久化代理凭据。每批任务启动时取得设置快照，修改不改变运行中的任务。代理故障沿用查询失败重试与 unknown 规则，不自动回退直连。

优先级：CLI `check --proxy` / `check --direct` > 看板已保存设置 > 启动 `--proxy` / `--direct` > OFFERS 进程环境 > 本工具配置文件。没有保存过看板设置时，空 `OFFERS_PROXY` 表示直连；在看板切换直连后，重启仍直连。`serve` 的启动参数作为初始默认值，后续可以由看板覆盖。不读取 SENTINEL_*、TURB_*、HTTP_PROXY、HTTPS_PROXY、ALL_PROXY 或原项目 .env。

每个账号单独创建客户端和 Cookie Jar。请求顺序执行，间隔 1 秒，单次超时 20 秒，每项最多 3 次；不重复叠加传输层重试。Retry-After 超过 60 秒或重试耗尽后停止本批次，将冷却时间存入结果库。重启不会自动重跑中断任务。

## 构建、发布、回滚

服务器使用独立 Rust 工具链。可先在新环境执行 `git clone https://github.com/bang-bang-liu/sentinel-rs-offers.git`；新项目拥有自己的 Cargo.lock 和 target。下面从已配置 Rust 的终端运行，非默认工具链安装路径应由部署方自行配置：

```bash
cd /opt/sentinel-offers/source
bash deploy/build.sh
# 使用一个全新 release 名称，禁止覆盖旧版本
sudo bash deploy/install.sh 0.1.1-YYYYMMDD-HHMMSS
systemctl enable --now sentinel-offers
```

后续升级已运行服务时，安装完执行 `systemctl restart sentinel-offers`。编译需要 C/C++、CMake、libclang；本机 Windows 可在已安装 LLVM、NASM、CMake 和 Rust 的 MSVC 开发者终端运行 `deploy/build.ps1`。本次交付以服务器 Linux 构建实测为准。

停止工具：`systemctl stop sentinel-offers`。同时取消开机启动：`systemctl disable sentinel-offers`。只操作新工具服务，不影响原注册程序。

回滚示例：先停止服务，再将新工具 `current` 指向已经存在的兼容旧版本，最后启动。绝不修改 `/opt/sentinel-rs/current`。

```bash
systemctl stop sentinel-offers
ln -sfn /opt/sentinel-offers/releases/<旧版本> /opt/sentinel-offers/current
systemctl start sentinel-offers
```

结果库 schema v1；旧程序不认识更新 schema 时会拒绝启动。升级前可用 Python sqlite3 backup API 备份结果库；不能在有 WAL 写入时只复制主 DB 文件。

源库和新结果目录都属于 sentinel，保持原库既有权限。本工具以 SQLite READ_ONLY + query_only 强制账号库只读；同一 OS 用户不代表操作系统完全剥夺了源库写权限。systemd 配置保留 WAL 读取所需的共享内存辅助文件访问。

界面按用户要求仅提供电脑端布局，建议使用宽度 1180px 以上的桌面浏览器窗口，不提供手机和平板专用布局。
