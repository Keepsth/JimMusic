# JimMusic 2.0 发布验收记录

基线日期：2026-08-15

## 当前本地证据

| 门禁 | 当前结果 | 证据范围 |
|---|---|---|
| Rust format | 通过 | workspace |
| Rust Clippy `-D warnings` | 通过 | workspace、all targets、all features |
| Rust FFI artifact build | 通过 | 先构建 workspace `cdylib` 再运行 ABI 测试；拒绝旧增量动态库掩盖当前符号表 |
| Rust tests | 通过（251） | 单元、动态库 FFI、本地 HTTP/CAS/加密/P2P 集成、传输流端点、网络类别与蜂窝额度、自动复刻、策略撤销自动停用与防回滚、收藏协助 Pin、发布者关注、策略本地覆盖、发布者全文索引、离线刷新队列、安装日志、配置 Schema、插件目录浏览/搜索/详情、三入口策略标注、关联 ID、CORS 与状态版本保护，workspace/all targets/all features |
| 原生 FFI/节点 | 通过 | ALSA/null/Web Output ABI、打开会话证据、应用内节点启动/前后台/停止/同进程重开与稳定 PeerId；stop 等待仓库锁释放，重启门禁连续 8 次通过 |
| Rust TLS 依赖边界 | 通过 | workspace 依赖树不含 `native-tls` 或 `openssl-sys` |
| Flutter analyze | 通过（0 issue） | 当前 Linux SDK |
| Flutter tests | 通过（94） | provider/model/widget、Rust 播放/输出会话/节点 FFI、控制面 SSE 解析/真实 HTTP 流/Provider 缺口重读、边下边播代理链路、播放页来源/缓冲/传输状态、关注发布者与策略覆盖 mutation、曲库统一同步、网络曲目播放入口、Rendition 选择、音乐目录设置、发布向导（含多 rendition 编辑）、三入口策略应用、错误本地化、发布者索引、离线队列提示、播放模式与队列边界、声明式配置控件与敏感字段、操作取消 |
| Rust release build | 通过 | 当前 Linux host，workspace |
| Flutter Web release build | 通过 | 当前 Linux host，包含 Worklet 静态资源；Rust PCM 桥仍未接通 |
| Flutter Linux release build | 通过 | 当前 Linux host，已注入 Core/null/system 三个动态库，`ldd` 无缺失项 |
| Helia 浏览器节点 | 通过 | 生产依赖审计 0 漏洞、bundle 构建通过；Rust 节点到 Helia 直连取回并验证 600,000 字节 UnixFS 对象 |
| Release binary smoke | 通过 | health/未授权拒绝/原生节点传输状态；优雅退出后以同一 repo 重启并保持稳定 PeerId |
| 验收报告校验器 | 通过（5） | 自动读取 134 项 P0，拒绝模拟器、P0 unsupported、资源回退超限与不完整报告 |
| 控制面 SSE 消费 | 通过 | Flutter 消费 `/v1/events`：sequence 缺口与 snapshot.required 触发整体重读、事件分组 300ms 合并定向刷新、断开退避重连与 30s 兜底轮询；IO(HttpClient)/Web(fetch 流式) 双传输；13 项测试 |
| Feed 快照压缩 | 通过 | 社区源快照支持 `Accept-Encoding: gzip`、`x-snapshot-sha256`/字节数完整性头与 32 MiB 未压缩上限（413 结构化错误）；API 测试 |
| 边下边播流端点 | 通过 | `/v1/transfers/{id}/stream` 跟随 part 增长输出、单范围 Range、终结后尾部交接、整块路径落盘交接与孤儿清理；Flutter 经 just_audio 代理注入鉴权播放并支持 Seek；Rust 4 项 + Flutter 4 项测试 |
| 网络类别策略 | 通过 | 网络类别声明驱动仅 Wi-Fi/计量开关的传输暂停与自动恢复（只恢复网络暂停任务）；runner 执行前复查；上传限速按 PROD-004 显式拒绝（`unsupported` + reason），UI 明示；服务 2 项 + API 2 项测试 |
| 插件撤销自动停用 | 通过 | 社区 Policy Revoke 事件在摄取与刷新后自动应用到已安装插件（manifest CID 匹配 → Revoked + 事件推送，幂等）；API 测试 |
| 播放页状态展示 | 通过 | 播放页显示真实来源标签、缓冲位置与边下边播下载状态（字节/状态/Provider），无模拟数据；4 项测试 |
| 收藏协助 Pin | 通过 | 收藏时按显式开关协助 Pin 内容 CID（本地直 Pin / 幂等 Pin 传输任务）；显式 Pin 与发布后推送第三方 Kubo 兼容 Pin 服务，端点校验；API 测试 2 项 |
| 发布者关注 | 通过 | 关注发布者后其目录内 Manifest 经解析/验签导入媒体库，禁用全部 Catalog 后仍可搜索播放；关注/取消/列表 API + 社区页 UI；API 测试 1 项 + Flutter 1 项 |
| 曲库统一同步 | 通过 | 本地文件推送（路径派生稳定 ID，跨语言黄金向量）、Manifest/社区拉取合并、收藏/歌单双向、会话推送或恢复（绝不自动播放）；控制台曲库同步页 + 列表来源图标；Flutter 6 项测试 |
| 撤销防回滚 | 通过 | enable/rollback 对已撤销发布拒绝（含手工改回 Disabled 的绕过）；mutate_record 保留域错误语义；生命周期测试 2 项 |
| 策略本地覆盖 | 通过 | 非强制策略（warn/demote/hide）可本地覆盖/取消，block/revoke 强制拒绝；社区页策略查询对话框；API 1 项 + Flutter 1 项测试 |
| 三入口策略应用 | 通过 | 搜索/详情/精确打开统一应用社区策略：`/v1/library/tracks` 与 `/v1/search` 标注曲目策略（manifest CID 优先、发布者次之）；搜索移除 hide/block/revoke、降权 demote、标记 warn；长按详情展示策略与本地覆盖；播放前 block/revoke 拒绝、warn 确认；API 1 项 + Flutter 9 项测试 |
| 蜂窝额度 | 通过 | 每任务蜂窝额度持久计量，超限暂停（结构化原因）并在回 Wi-Fi 后自动恢复；runner 按块计量并安全中止；服务测试 2 项 |
| 错误本地化 | 通过 | Flutter 统一把稳定错误信封映射为本地化文案 + 重试建议 + 恢复提示，控制台横幅与传输错误行统一消费；5 项测试 |
| 发布者全文索引 | 通过 | 曲库索引增加发布者身份 CID（Manifest 导入记录），标题/艺人/专辑/标签/发布者统一匹配；Flutter 同步映射并纳入列表搜索；服务 1 项 + Flutter 1 项测试 |
| 离线刷新队列 | 通过 | 网络不可用时社区源刷新进入持久队列（503 + retryable 显式告知），恢复后自动排空；社区页展示排队与立即重试；API 1 项 + Flutter 1 项测试 |
| 播放模式（P1） | 通过 | 顺序/列表循环/单曲循环/随机（just_audio LoopMode + 桥边界决策：单曲循环回拉当前曲目、顺序模式绕回队首即停止）；播放页模式按钮；5 项测试 |
| 插件安装日志 | 通过 | 安装中间态（downloading/verifying/staging/committing）持久化，失败保留结构化错误、中断重启标记 interrupted；插件页展示；生命周期测试 2 项 |
| 声明式配置控件 | 通过 | Schema 端点（JSON/DAG-CBOR 双编码解析），Flutter 按 Schema 渲染开关/枚举/滑杆/文本框（默认值计算），不可解析回退 JSON；API 1 项 + Flutter 3 项测试 |
| 关联 ID | 通过 | 每个 HTTP 请求一个 v1_request span（method/path/request_id），Idempotency-Key 提取长度受限且不读取秘密头；2 项测试 |
| 操作取消 | 通过 | 控制台“取消当前操作”关闭进行中操作的专属客户端，操作以“操作已取消”结束并可重新发起；1 项测试 |
| 发布自动复刻 | 通过 | auto_replicate_published 开启后发布成功即把各 rendition 内容 CID 建为幂等 Pin 传输任务并推送第三方服务；API 1 项测试 |
| 敏感配置字段 | 通过 | Schema 声明 sensitive 的字段遮罩显示并标注（敏感）；Flutter 1 项测试 |
| CORS | 通过 | 浏览器客户端跨源访问控制面（预检 + 响应头），认证仍由 Bearer token 强制；1 项测试 |
| 状态版本保护 | 通过 | 五个核心存储拒绝未来 schema_version（降级保护、保留原文件），旧版本前向兼容；2 项测试 |
| 网络曲目播放入口 | 通过 | 网络曲目（Manifest/社区）与本地曲目同一入口：按内容 CID 建幂等 fetch 传输并边下边播；Flutter 1 项测试 |
| Rendition 选择 | 通过 | 客户端按平台能力（Web 容器/编解码白名单）、质量（lossless/original 优先）与网络策略（计量网络偏好有损流式小体积）从全部 rendition 源选择播放 CID；全部源随曲库同步映射；Flutter 7 项测试 |
| 音乐目录设置 | 通过 | 曲库页查询/设置音乐目录（仅切换语义并明示复制/移动未实现）；Flutter 1 项测试 |
| 发布向导 | 通过 | 元数据/权利声明 + 多 rendition 编辑（增删、ID/容器/编解码/采样率/位深/声道/字节长度、唯一 original）表单校验生成 Manifest（byte_length 正整数，满足后端校验），签名发布后展示回执与副本健康度（本机 Pin/Provider/第三方服务）；5 项测试 |
| 插件目录浏览 | 通过 | `GET /v1/plugins/catalog` 列出社区目录收录的 PluginManifest 条目并支持 `q` 搜索（CID/分类/标签/注解），`/catalog/{cid}` 详情解析 Manifest（JSON/DAG-CBOR 双编码）并返回 artifact_available/installed_state/active_version/update_available/revoked 摘要；插件页目录浏览/搜索/详情/安装（发布者公钥与权限确认，`ipfs://CID` 直取制品）；API 1 项测试 |
| 社区原生二次确认 | 通过 | 社区原生默认拒绝（CommunityNativeDenied）；高级授权安装前二次确认（持续警告文案），已安装插件列表永久标记警告条目；后端安装/撤销 E2E 测试 + Flutter 4 项测试 |
| GitHub Actions lint | 通过 | `actionlint` 1.7.7，含最终 HarmonyOS 验签步骤 |
| P0 追踪完整性 | 通过 | 134/134 已映射：本机通过 92、部分实现 30、缺失 0、待外证 12；“无缺失”不等于已满足跨平台 DoD |

## CI 候选门禁

`.github/workflows/release.yml` 对同一 tag/commit 执行：

- Rust fmt、Clippy、完整测试；
- Linux、macOS、Windows 后端；
- Android、iOS、HarmonyOS、Windows、Linux、macOS、Web Flutter 产物；
- Android/iOS/macOS/Windows 发布签名和 Apple notarization；
- 对应提交源码包、SHA256SUMS、SPDX SBOM 与 build provenance attestation；
- 任一必需 job 失败时不创建稳定 Release。

HarmonyOS 需要固定到仓库变量 `FLUTTER_OHOS_REF` 指定的 commit，并由
`[self-hosted, harmonyos]` runner 产生 HAP。release job 使用 OpenHarmony hapsigner 验签，并把
提取的证书链 SHA-256 与 `HARMONY_RELEASE_CERT_CHAIN_SHA256` 比较；runner 必须预先配置对应
release 签名。签名密钥、证书和 runner 不在仓库中，因此工作流存在不等于产物已经验收。

## 尚未取得的必需证据

- 七个平台同一候选版本的安装、首次启动、断网曲库/播放/Seek/歌单恢复；
- 真实浏览器经测试中继、原生/Helia 对 Kubo，以及 Android/iOS/HarmonyOS 的关闭公共网关 P2P；
- Wasmtime 沙箱已通过本机恶意样本；仍缺 Web/iOS/HarmonyOS 执行载体和七端恶意插件外证；
- WASAPI Exclusive、CoreAudio Hog、ALSA hw/PipeWire、DSD Native/DoP 的真实设备报告；
- 两小时播放内存、起播/Seek/UI P95、耗电/带宽基线、无障碍审计；
- 升级、降级、schema 迁移、杀进程/断电和安全模式恢复矩阵；
- 受控 tag workflow 的签名产物、SBOM、摘要、provenance 和下载复验。

## 发布结论

当前仓库不得发布为“满足全部需求的 JimMusic 2.0 稳定版”。它可以发布为明确标注已知限制的
Release Candidate 源码基线；稳定版必须以 `docs/REQUIREMENTS_TRACEABILITY.md` 中所有 P0
阻断项关闭，并由七个物理 runner 对同一 commit 产生无豁免报告为前提。
