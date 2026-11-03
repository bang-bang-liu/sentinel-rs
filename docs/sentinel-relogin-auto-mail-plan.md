# 重登自动获取邮箱取码地址：最小变更计划书

版本：v1.0

日期：2026-11-03（北京时间）

状态：已实施并部署；Workbench 在阿里云、重登工具在华为云 SG。实际测试和未覆盖的真实分支见[实施记录](../tools/relogin/VERIFICATION.md)，下文保留设计基线。

## 1. 目标与结论

用户在现有桌面 UI 指定账号重登；登录明确要求邮箱验证码时，工具自动向 Workbench 提交该账号邮箱，取得本轮有效的取码地址，然后继续现有取码与验证步骤。用户只需配置一次接口地址和密钥，不再逐个填写、定期更新 JSON 邮箱映射。

最小方案涉及两个后端：

| 项目 | 改动 |
| --- | --- |
| `icloud-create-workbench` | 新增按已有邮箱签发临时取码链接的独立接口、凭据表和读取接口 |
| `sentinel-rs/tools/relogin` | 增加远程解析器，接入现有独立邮箱分支 |
| 两边前端 | 沿用现有界面，无需改动 |

本计划是在[原重登计划](sentinel-relogin-minimal-plan.md)基础上替换邮箱地址获取方式。密码登录、TOTP、补绑 2FA、session、Cookie 和账号 UPDATE 的既定流程继续沿用。

## 2. 当前机制与需要解决的问题

根据本地 Workbench 源码，原注册程序调用 `POST /api/v1/mailboxes` 时，服务从库存选择一个邮箱，标记为 `used`，同时生成领取 ID 和随机 Token，返回 `email`、`code_url`、`expires_at`。链接有效期为一小时，数据库只保存 Token 哈希。

因此，旧链接过期后不能从邮箱或哈希中恢复原始链接；再次调用领取接口还会消耗另一个库存邮箱。自动化应当为指定的已有邮箱签发新链接。

邮箱与收件账号的关系已经保存在 Workbench 的 `hidden_addresses.account_id` 中；新接口直接查询这些现有数据，不增加人工映射表，也不依赖注册账号库保存过旧的 `code_url`。

以上结论已核对本地源码与实际部署。Workbench 生产实例位于阿里云 `/root/icloud-create-workbench`，由 Docker Compose 运行；实施前比对的四个接入文件与本地基线一致。

## 3. 调用流程

```text
用户在 UI 选择已有账号，执行重登
  → 密码登录
  → 服务端明确要求邮箱验证码
  → POST Workbench 重登专用 resolve 接口，提交该账号邮箱
  → Workbench 查已有邮箱，生成本轮短期取码链接
  → 重登读取器 GET 该链接，记录已有验证码及时间基线
  → 沿用现有登录专用 resend 请求
  → 轮询同一个链接，按现有时间/基线规则选取验证码
  → 提交登录验证码，继续原重登流程
  → 成功后更新原账号行
```

没有邮箱挑战时，不请求 resolve、不签发链接、不访问邮箱服务。每次进入邮箱挑战只解析一次；轮询不反复签发。三天后重登如再次要求邮箱验证，就重新签发当时有效的新链接。

保留现有“准备邮箱 → 登录 resend → 轮询”顺序，避免在验证码发出后才采集基线。两边继续使用真实邮件时间和已有五秒容差，不改写邮件时间。

## 4. Workbench 新接口契约

### 4.1 按邮箱签发本轮取码地址

```http
POST /api/v1/relogin/mailboxes/resolve
Authorization: Bearer <MAIL_API_RELOGIN_TOKEN>
Content-Type: application/json

{"email":"first@example.invalid"}
```

成功返回 `201`，示例值仅用于说明格式：

```json
{
  "email": "first@example.invalid",
  "code_url": "https://mail.example.invalid/api/v1/relogin/access/11111111-1111-4111-8111-111111111111/code?token=<random-token>",
  "expires_at": "2026-11-03T08:00:00.000Z"
}
```

行为约定：

1. 请求体只接收一个 `email`，去除首尾空白后按已保存邮箱精确匹配。
2. 查询范围沿用现有 `MAIL_API_ACCOUNT_ID` 和 `MAIL_API_LABEL_PREFIX`；邮箱不在垃圾箱、所属账号未删除且收件配置存在。
3. 不要求邮箱为 `unused`，已用于注册的 `used` 邮箱可以查询。没有旧领取记录也可查询，但仍须满足上述范围。
4. 查找成功后生成独立随机 Token，只保存哈希；固定有效期一小时，从本次签发开始计算。
5. 只新增重登取码凭据；不生成隐藏邮箱，不领取库存，不修改邮箱状态、标签及其更新时间。
6. 同一邮箱可重复签发。新链接不会使尚未过期的旧重登链接失效，也不会轮换原注册链接或公开链接。

已有邮箱在范围外或不存在时返回 `404`，本次任务显示“该邮箱没有可用的自动取码地址”。接口不会尝试领取新邮箱或自动扩大范围。

### 4.2 读取本轮验证码

```http
GET /api/v1/relogin/access/:accessId/code?token=<random-token>
```

凭链接中的独立 Token 访问，不需要全局 Bearer 密钥。成功返回兼容现有读取器的 HTML：

```html
<article>
  <h2>邮件主题</h2><p>发件人</p>
  <div class="meta">Time: 2026-11-03 07:00:02</div>
  <div class="code">123456</div>
</article>
```

只返回该邮箱自本轮链接签发前五秒起、按真实邮件时间倒序排列的最多 50 封邮件中的有效六位码。HTML 时间为 UTC；无验证码时返回 `200` 空卡片页，原样保留邮件时间，不伪造验证码。

读取时仍检查邮箱所属范围和可用状态。沿用现有收件服务 `inboxService.sync(accountId)`，新服务按账号最多每八秒触发一次后台同步；同步忙时使用现有互斥机制，HTTP 先返回当前缓存，后续轮询读取更新结果。不修改 IMAP 配置、同步算法或原注册取码服务。

### 4.3 状态码与请求处理

| 状态码 | 含义及客户端处理 |
| --- | --- |
| `201` | resolve 成功，开始使用返回链接 |
| `200` | 取码页成功；没有新码则继续现有轮询 |
| `400` / `413` | 请求格式错误或请求体过大，结束当前任务 |
| `401` | resolve 接口密钥错误，提示检查重登专用配置 |
| `404` | 邮箱不可用，或取码链接/Token 无效，结束当前任务 |
| `410` | 正确的取码链接已过期，结束当前任务，下次重登重新签发 |
| `429` / `503` | 请求过频或服务未就绪，显示对应失败原因 |

沿用既有限流量级：签发每 IP 每分钟 60 次，读取每 IP 每分钟 600 次，使用新路由自己的限流器。响应禁用缓存；异常输出只保留简短原因。首版不增加自动重新签发、自动重登或新的重试框架。

## 5. 数据库最小变更

只在 **Workbench 自己的数据库**新增一张表：

```sql
CREATE TABLE IF NOT EXISTS mail_api_relogin_accesses (
  id TEXT PRIMARY KEY,
  address_id TEXT NOT NULL REFERENCES hidden_addresses(id),
  token_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL
);
```

`address_id` 不设置唯一约束，以支持同一邮箱多次重登。签发时在一个短事务中查邮箱并插入凭据，网络和 IMAP 操作不占用该事务。

原 `mail_api_claims` 及 `address_public_access` 的结构和记录均保持现状。原账号库、`offers.db` 不迁移，也不增加取码地址字段。临时 URL 只保存在当前重登任务内存中，不写回账号库、JSON 文件或浏览器。

过期记录始终不能取码；首版不增加定时清理任务。

## 6. 文件改动清单

### 6.1 Workbench

在既有后端层次中新增重登模块，避免重构原注册邮箱服务：

```text
src/
  relogin-mail-config.mjs                 新增：读取和校验重登接口密钥
  relogin-mail-schema.mjs                 新增：创建独立凭据表
  repositories/relogin-mail-repository.mjs 新增：查已有邮箱、保存/校验凭据、查邮件
  services/relogin-mail-service.mjs       新增：签发、有效期、取码页和后台同步触发
  routes/relogin-mail-routes.mjs          新增：两个接口、鉴权和错误响应
  config.mjs                             小改：载入新增配置
  db.mjs                                 小改：调用新表初始化
  server.mjs                             小改：创建并关闭新服务
  app.mjs                                小改：挂载 /api/v1/relogin
test/
  relogin-mail.test.mjs                   新增：接口与数据隔离测试
docs/
  relogin-mail-api.md                     新增：契约与部署说明
```

新路由挂在通用管理端 Origin/Cookie 中间件之前，沿用现有机器接口的挂载方式。配置错误只关闭新增接口，不阻止原管理端和原邮箱领取接口启动。

HTML 卡片输出按现有格式在新模块内实现，复用现有 IMAP 收件实例。无需为这次功能抽取公共框架，也不修改原 `mail-api-service.mjs`、`mail-api-repository.mjs`、`mail-api-routes.mjs` 和公开链接服务。

### 6.2 sentinel-relogin

```text
tools/relogin/
  backend/src/config.rs                  增加 resolve URL 和独立密钥配置
  backend/src/mail/resolver.rs            新增：调用 resolve，解析 email/code_url/expires_at
  backend/src/mail/mod.rs                 声明新模块
  backend/src/mail/reader.rs              在 prepare 内选择 API 或原文件映射
  backend/tests/mail_resolve.rs           新增：自动解析与取码集成测试
  deploy/login.env.example                补充两项配置
  README.md / VERIFICATION.md             更新使用说明与实施验证记录
```

保持现有 `Mailbox::prepare/poll` 接口。登录引擎继续调用同一接口，邮件解析、登录 resend 与最终保存逻辑无需改动。原 `mapping.rs` 保留，作为未启用 API 时的文件模式。

## 7. 配置与模式选择

Workbench 增加一项：

```dotenv
MAIL_API_RELOGIN_TOKEN=<重登接口专用随机密钥>
```

新接口读取现有 `MAIL_API_ENABLED`、`MAIL_API_ORIGIN`、`MAIL_API_ACCOUNT_ID` 和 `MAIL_API_LABEL_PREFIX` 来确定启用状态、返回地址及范围，不更改这些配置值。新增密钥与注册的 `MAIL_API_TOKEN` 分开；未设置时只关闭新增接口。

重登工具增加两项：

```dotenv
LOGIN_MAIL_RESOLVE_URL=https://mail.example.invalid/api/v1/relogin/mailboxes/resolve
LOGIN_MAIL_RESOLVE_TOKEN=<与MAIL_API_RELOGIN_TOKEN一致的密钥>
```

| 配置情况 | 行为 |
| --- | --- |
| 未设置 `LOGIN_MAIL_RESOLVE_URL` | 保持原 JSON 文件模式，兼容现有部署 |
| 已设置 URL 和密钥 | 进入 API 模式，不读取邮箱映射文件 |
| 设置 URL 但缺少密钥，或 resolve 请求失败 | 只在邮箱挑战时提示失败；不悄悄切回旧 JSON 链接 |

`LOGIN_MAIL_RESOLVE_TOKEN` 只发送给 resolve 接口。API 模式的取码 GET 仅使用链接 Token，不附带全局 Bearer；原 `LOGIN_MAIL_API_TOKEN` 仍只用于旧文件模式，含义不变。现有部署切换后可以保留旧 JSON 文件，无需继续维护。

邮箱解析、取码继续直连，不受 `LOGIN_PROXY` 影响；账号认证继续按 `LOGIN_PROXY` 执行。两项新配置只写入重登工具自己的配置文件，不读取 `REG_MAIL_*`。URL、密钥不返回 UI，不进入任务摘要。

## 8. 实施顺序与验收

| 阶段 | 工作与完成标准 |
| --- | --- |
| A：Workbench 后端 | 新表、两个接口及配置完成；合成邮箱可以反复签发并读取，原接口行为不变 |
| B：重登适配 | 接入现有 prepare/poll；API 模式无需 JSON，文件模式继续可用 |
| C：本地联合验证 | 用隔离数据库和模拟邮件完成“解析 → 基线 → resend → 新码 → 提交”的现有重登分支 |
| D：部署 | 先确认并更新实际 Workbench 实例，再更新 SG 的独立重登服务 |
| E：单账号联调 | 在要求邮箱挑战的实际账号验证自动取码，记录成功或尚未覆盖的真实分支 |

关键验收用例：

1. 同一 `used` 邮箱重复签发得到不同链接；每条链接有效期分别计算，原未过期链接仍可用。
2. 签发前后邮箱数量、状态、标签、原领取记录和公开访问记录不变；没有消耗 `unused` 库存。
3. 原注册链接和公开链接在新签发后仍按原有效期和权限工作；新旧类型的 ID/Token 不能交叉使用。
4. 不存在、范围外、垃圾箱邮箱及删除账号不可访问；链接过期返回 `410`，密钥错误不能签发。
5. 无验证码先返回空页；新邮件出现后，卡片包含真实 UTC 时间，可被现有读取器解析。
6. 无邮箱挑战的密码/TOTP 重登产生零次 resolve、零次取码请求。
7. 邮箱挑战只调用一次 resolve，随后轮询同一 URL；跨日重登重新签发，不依赖上次 URL。
8. API 模式不读取 JSON、不把 resolve 密钥发送给取码 GET；设置认证代理仍保持邮箱直连。
9. 无 resolve 配置时原文件模式正常；resolve 失败时旧账号凭据不被覆盖。
10. 成功仍只更新原账号行的约定字段；原注册离线测试和 Workbench 原邮箱接口测试继续通过。

测试使用合成地址、隔离数据库和模拟邮件，不为接口测试领取真实库存。真实联调如未触发邮箱挑战，只能证明未触发分支正常，不能标记自动取码已验收。

## 9. 部署与回滚

实施时先确认 `mail-api.spxbase.com` 对应的实际主机、部署目录、运行版本和持久化数据库。已确认 Workbench 位于 SSH 别名 `AlibabaCloud` 的阿里云主机，数据挂载为 `/root/icloud-create-workbench/data:/app/data`；重登工具独立部署于 SG。

Workbench 仅部署本次代码增量，保留原数据目录、加密密钥和其他配置。迁移前使用现有备份工具生成一致性备份；不把本机开发数据库覆盖到生产实例。新表创建可重复执行。

先上线 Workbench 并用指定已有邮箱验证签发与取码接口，再构建部署 SG 的新重登 release，并在 `/opt/sentinel-relogin/.env` 增加两项配置。只重启涉及的新版本服务；原注册入口、配置、二进制及 offers 服务无需更新。沿用现有 UI 和 SSH 隧道访问方式，不增加公网管理端口。

回滚时切回重登工具旧 release，或移除两项 resolve 配置回到原文件模式；Workbench 清空重登专用密钥即可关闭新接口。保留新增表，避免代码回滚时恢复整库、丢失新邮件。回到文件模式后仍受原链接一小时有效期限制。

## 10. 交付范围

交付新增 Workbench 接口与独立凭据表、重登远程解析器、配置模板、接口文档和验证记录。使用现有 Node、SQLite、Rust HTTP 和 JSON 能力，计划不增加运行依赖、不修改前端、不新增后台定时任务。

完成后，已有且在授权范围内的邮箱由 Workbench 自动查找并签发本次链接；用户无需维护逐账号 JSON 映射。邮箱未入库、归属范围不符或收件配置缺失时，仍需在 Workbench 处理对应基础数据，接口不会凭邮箱猜测收件关系。
