# JimMusic 2.0 API、ABI 与状态迁移说明

## 兼容边界

- HTTP 控制面稳定入口为 `/v1`。公共 DTO、网络对象、错误和事件包含
  `schema_version: 1`；未知主版本必须拒绝。
- `plugin-abi` 与 Audio ABI v2 使用显式 ABI 常量和 `#[repr(C)]` 布局。加载器必须在调用
  任何插件函数前拒绝 ABI 不匹配。
- Canonical DAG-CBOR 字节是 CID 和 Ed25519 签名的唯一输入。不得用普通 JSON 重编码后验签。
- 2.0 之前的未版本化写接口不再执行副作用，返回 HTTP 410；客户端必须迁移到 `/v1`。

## HTTP 客户端迁移

1. 基址改为 `http://127.0.0.1:8787/v1`。远端访问必须由用户显式配置 HTTPS 终结层。
2. 从仓库目录的 `control-token` 读取本机 token，或通过 `JIMMUSIC_API_TOKEN` 注入；每次请求发送
   `Authorization: Bearer <token>`。token 不应写入 Flutter SharedPreferences 或日志。
3. 每个 `POST`、`PUT`、`PATCH`、`DELETE` 必须发送唯一 `Idempotency-Key`，也可在支持的 DTO
   中提供相同 `request_id`。相同 key 与相同请求返回原响应；相同 key 复用于不同请求返回 409。
4. 错误按 `ErrorEnvelopeV1.code/subsystem/operation/retryable/unsupported_reason` 处理，不解析文案。
5. 长任务先读取快照，再订阅 `/v1/events?after=<sequence>`。收到 `snapshot.required` 时重新读取
   服务端给出的快照端点，不能猜测漏失事件。
6. 社区维护者换钥/撤销通过 `/v1/community-sources/{id}/maintainer-key-events` 提交连续签名
   事件；不得用重新 `POST /community-sources` 绕过旧钥授权。举报先 `POST /moderation-reports`
   持久排队；服务按 30 秒至 1 小时指数退避自动重试，也可调用 `/moderation-reports/{id}/retry`。
   请求设置 `encrypt_for_recipient=true`
   时，Core 使用源 Manifest 的 X25519 公钥生成 XChaCha20-Poly1305 envelope；远端只收到密文与
   最小路由元数据，明文报告仍只留在本地鉴权控制面。
7. `/v1/community-sources/import` 接受裸 CID、`ipfs://`、`ipns://` 和
   `jimmusic://community/...` 定位符；二维码应编码同一 JimMusic URI。导入仍必须提供维护者
   Ed25519 公钥并通过 Manifest 签名校验，定位符本身不是信任根。
8. 创建传输可提供 `priority`（-100..100，默认 0），排队值越高越先占用并发槽；仅 queued/paused
   任务可通过 `PATCH /v1/transfers/{id}/priority` 调整，已经运行的任务不会被静默抢占。

## 持久状态

状态以带 `schema_version` 的 JSON 文件存储，并通过同目录临时文件、`fsync`、原子替换提交；Unix
文件权限为 0600。已知状态包括节点、传输、发布、社区、曲库、插件生命周期和幂等结果。

- 同 schema 的新增可选字段使用 serde default，旧数据可读取。
- 未知或损坏状态不会被默认值静默覆盖；启动返回错误并保留原文件。
- 不支持降级读取高版本 schema。降级前必须备份整个 repo 目录，并使用对应版本生成的备份恢复。
- 插件升级先暂存和校验制品，再提交版本指针；失败保留 active/rollback 版本。
- 播放会话恢复强制 `auto_play=false`，避免异常退出后自动出声。

## 回滚步骤

1. 停止 `plugin-manager`，确认没有 `.part` 下载正在提交。
2. 复制 repo 目录作为只读取证备份；不要只复制单个 JSON 文件。
3. 若仅回滚插件，优先调用 `/v1/plugins/{id}/rollback`，不要手工修改 lifecycle 状态。
4. 若回滚应用版本，确认目标版本支持当前 schema；否则恢复由该版本创建的完整 repo 备份。
5. 重启后检查 `/v1/health`、`/v1/node/status`、`/v1/transfers`、`/v1/audio/path` 和
   `/v1/diagnostics`，确认没有 integrity_failed 或 safe mode 异常。

当前仍缺自动化的跨版本升级/降级矩阵与断电恢复实验，因此本文件是操作契约，不是 REL-006 的
完成证据。
