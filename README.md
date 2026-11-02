# Sentinel Offers

按 v1.1 设计开发的独立 Rust 账号优惠查询工具与 Web 看板（程序版本 0.1.0）。

读取现有 SQLite 账号库的邮箱、状态和 access_token，查询免费一个月及三种期限的 Plus 半价活动，写入独立 offers.db。源库连接强制只读。无注册、登录刷新、2FA、兑换或支付操作。

## 源码

- `backend/`：Rust 后端；domain、application、infrastructure、api 分层，独立 migrations 和 tests。
- `frontend/`：纯 HTML/CSS/ES module JavaScript，看板通过 JSON API 与后端通信。
- `deploy/`：独立构建、安装、启动与 systemd 模板。
- `docs/`：v1.1 设计快照、实施设计、复制来源、操作手册与验证记录。

前端静态文件当前编译进二进制，不需要 Node 运行时。复制的 HTTP 代码只在本项目独立维护，无 sentinel-rs path/git 依赖或运行时调用。

## 构建

Linux 需要 Rust、C/C++、CMake、libclang；执行 `bash deploy/build.sh`。Windows 环境要求与入口见 `docs/operations.md`。低内存服务器建议使用单任务编译。

## 使用

服务只监听 `127.0.0.1:8787`，通过 SSH 隧道访问；页面要求独立管理口令。启动服务和打开页面均不自动查询上游。命令行提供 check、serve、export；CLI 查询和 Web 服务通过写入锁互斥。

完整操作见 [操作手册](docs/operations.md)，脱敏验证结果见 [验证记录](docs/verification.md)，架构见 [设计文档](docs/design.md)。

活动接口不是稳定公共 API。工具分别展示明确资格与未知状态，不将认证失败、限流、网络故障或协议变化解释为没有优惠。

源码发布与敏感数据边界见 [发布说明](docs/publishing.md)。仓库不包含生产数据库、运行配置或真实账号资料。
