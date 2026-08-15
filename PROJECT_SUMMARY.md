# JimMusic 2.0 实施总结

基线日期：2026-08-15。本文概述当前工作区的真实实现，不作为单独完成证据。逐项结论以
`JimMusic需求文档.md`、`docs/REQUIREMENTS_TRACEABILITY.md` 和
`docs/RELEASE_ACCEPTANCE.md` 为准。

## 本次形成的实现基线

### 协议与可信核心

- 新增 `backend/protocol`：版本化公共 DTO、网络对象、稳定错误信封、严格 canonical
  DAG-CBOR、CIDv1 和输入大小/深度/数量限制。
- Audio ABI v2 与类型化 Audio Graph 支持 DAG 预检、格式转换、延迟补偿、预分配 planar f32
  缓冲、sample-position 参数队列、非实时编译、原子提交/回滚和实时统计。
- 播放引擎改为 Symphonia 增量解码与有界 PCM 队列；只有输出真实启动后才报告 Played。缺失、
  解码、设备等失败通过结构化模型返回。队列停止与自动续播竞态已修复；双解码时间线提供无额外
  空帧的 gapless，以及 linear/equal-power crossfade。
- DSD/Encoded 使用独立类型端口与 passthrough 节点，普通 PCM Processor 接线会在编译期拒绝；
  输出 ABI 从真实打开句柄报告协商格式、设备、驱动、共享/独占、缓冲和时钟来源。
- `LibraryService` 持久保存本地/Manifest Track Source、扫描状态、收藏、歌单和停止态会话；
  缺失文件不会被静默删除。

### 节点、传输与分发

- 内置本地 verified CAS 支持 raw/DAG-CBOR add/cat、逐对象 CID 验证、Pin/Unpin、配额、LRU、
  持久恢复和 ProviderHealth；Ed25519 PeerId 重启稳定。
- 原生 rust-ipfs 节点提供持久仓库、UnixFS、Bitswap、Kademlia、mDNS、TCP/WebSocket/QUIC；
  Web 使用 Helia、IndexedDB、Bitswap 和浏览器传输，并删除 delegated HTTP 路由。Rust 与 Helia
  已直连取回 600 KB UnixFS 对象并校验整文件摘要。
- 传输任务具备持久优先级调度、暂停、恢复、取消、重试、进度、限速和崩溃恢复；下载写入 part 文件，
  流式验证 CID 后才原子提交。
- Publisher Identity 使用 Ed25519；导出 bundle 由 Argon2id 与 XChaCha20-Poly1305 保护，并支持
  导入、轮换和撤销。
- PublicationService 支持签名 Manifest 与 publish/update/tombstone Feed，检查 sequence、previous
  CID、签名、许可证/权利声明并默认 Pin。
- CommunitySourceService 支持签名 Catalog/Policy 双 Feed、独立开关、增量回溯、去重、本地索引、
  策略严重度合并、信任顺序、本地 block、双钥换钥/撤销、签名加密举报与离线退避。

### 插件与控制面

- PluginManifest 按 platform/architecture/runtime 选择制品；安装前校验 ABI、core version、权限、
  依赖、冲突、签名、CID、摘要、长度和信任通道。
- 插件安装采用 staging + fsync + 原子状态提交，保留 rollback 版本；状态、配置、审计、连续失败、
  quarantine 与 safe mode 持久化。
- Wasmtime 47 执行社区 WASM，默认不提供 WASI/环境 import，限制 fuel、线性内存和表，并只通过
  owner-scoped 不透明 capability handle 授权；猜测句柄、撤销、文件/网络 import 和耗尽攻击均有
  负向测试。
- `/v1` 提供节点、Pin、传输、身份、发布、社区、策略、插件、Audio Graph 和曲库服务。所有写
  请求受持久幂等中间件保护；相同 key 的不同请求返回冲突。
- 服务默认回环并强制 bearer token；SSE 使用单调 sequence 与 snapshot.required 恢复语义。
- `/v1/diagnostics` 产生可分享快照，明确排除 token、私钥、口令、媒体路径、插件配置与安装路径，
  并有路径泄漏回归测试。
- 本地 reliability ledger 记录正常/未清理会话并输出聚合 crash-free rate，不做远端画像。

### Flutter 与发布工程

- 移除演示曲目和模拟播放；本地播放器基于 Rust FFI/just_audio 的真实状态，保存音量、静音、
  曲库、收藏、歌单和停止态会话。
- 控制中心覆盖节点配置/Pin、传输、身份与发布、社区源、插件生命周期/配置、Audio Path、
  bit-perfect 模式开关/条件和安全诊断快照；Audio Path 同时显示应用内已打开输出会话的设备、驱动、
  实际协商格式、缓冲、时钟和证据来源。原生 app 内节点经 FFI 启动并随前后台生命周期更新；
  Web 节点随 pagehide/pageshow/visibility 更新，二者均明确不承诺进程/页面关闭后持续提供。
- 产品名、bundle/application ID 和版本统一到 JimMusic 2.0。
- 发布工作流按同一提交构建七端，要求平台签名，生成源码包、SHA256、SPDX SBOM 和 provenance；
  HarmonyOS 工具链必须固定 commit。新增七个物理验收 runner，校验同一 commit 的 134 项 P0、
  十二个 E2E 场景、四项 M0 资源指标与硬件会话证据，任一缺失则不发布。

## 当前验证

最终结果记录在 `docs/RELEASE_ACCEPTANCE.md`。最终一轮已经观察到：

- Rust format、Clippy（all targets/all features，`-D warnings`）和 255 项完整测试通过；FFI 门禁先
  构建真实 `cdylib`，再验证 Output/Host/Node 符号与行为，避免旧增量产物或静默跳过；
  节点 stop 等待 rust-ipfs 仓库锁释放，同进程重启门禁稳定通过；
- Flutter analyze 零问题，96 项测试通过（含 Rust 音频/节点 FFI、控制面 SSE 流、
  边下边播代理链路、播放页来源/缓冲/传输状态展示、发布者关注、策略覆盖、错误
  本地化、发布者索引、离线队列提示、播放模式边界、声明式配置控件与敏感字段、
  网络曲目播放入口、音乐目录设置、发布向导、操作取消与曲库统一同步）；
- 边下边播（DST-007）闭环：`/v1/transfers/{id}/stream` 跟随 part 文件增长流式输出并支持
  Range；Flutter 经 just_audio 代理注入 Bearer 鉴权播放、Seek 只取已下载前缀，传输页提供
  边下边播入口；整块路径（本地 CAS/P2P）落地后写入 part 供播放交接，孤儿文件启动/流端点清理；
- 网络类别策略（NOD-006）：网络类别声明（Wi-Fi/蜂窝/有线/未知）+ 计量开关驱动传输的
  自动暂停与恢复（只恢复网络暂停任务，用户手动暂停不打扰），结构化原因与事件推送；
  上传限速因内嵌 Bitswap 无带宽节流而按 PROD-004 显式拒绝（`unsupported` + reason），
  UI 明示暂不支持；
- 插件撤销自动停用（PLG-009）：社区 Policy Revoke 事件在摄取与刷新后自动应用到已安装
  插件（manifest CID 匹配 → Revoked + `plugin.state_changed` 事件，幂等）；
- 收藏协助 Pin 与第三方 Pin 服务（DST-009）：收藏时按显式开关协助 Pin 内容 CID（本地直
  Pin / 幂等 Pin 传输任务，服从网络策略）；显式 Pin 与发布后把 CID 推送给用户配置的
  Kubo 兼容第三方服务；Provider 健康度回填配置的服务列表；
- 直接关注发布者（COM-003）：关注/取消/列表 API 持久化关注记录，关注后把目录中该发布者
  的 Music Manifest 解析（本地 CAS/P2P、签名校验）导入媒体库，禁用全部 Catalog 后仍可
  搜索播放，取消关注不删除已导入的用户数据；
- 曲库统一同步（PLR-001/002/009、UI-002）：Flutter 曲库与控制面 LibraryService 双向同步
  （本地优先）：本地文件推送（路径派生稳定 ID，与后端 sha256 规则跨语言一致）、
  Manifest/社区曲目拉取合并、收藏与命名歌单双向、会话推送或恢复（恢复绝不自动播放）；
  控制台新增曲库同步页（结构化报告与错误），列表项按来源图标区分本地/IPFS/社区；
- 撤销防回滚（SEC-011）：已撤销发布无法重新启用或回滚回去（含状态被手工改回的绕过场景），
  域错误语义在 mutate_record 中完整保留；
- 策略本地覆盖（COM-011）：warn/demote/hide 可本地覆盖/取消（附申诉理由），block/revoke
  为强制决策拒绝覆盖；社区页提供策略查询与覆盖对话框；
- 三入口策略应用（COM-006）：`/v1/library/tracks` 与 `/v1/search` 统一标注曲目社区策略
  （manifest CID 优先、发布者身份次之）；搜索入口移除 hide/block/revoke 并降权 demote、
  标记 warn；长按曲目打开详情（策略信息 + 非强制动作本地覆盖）；精确打开（播放）前
  block/revoke 直接拒绝并解释、warn 二次确认；
- 策略匿名申诉（SEC-009）：曲目详情“申诉”入口——本机核心以一次性不可关联密钥代签
  匿名申诉（reason_code=appeal）进入持久审核队列；本地 block 无远端接收方被结构化拒绝；
- 蜂窝额度（DST-010）：每任务蜂窝额度持久计量，超限结构化暂停并在回 Wi-Fi 后自动恢复；
- 发布者全文索引（COM-005）：曲库索引记录发布者身份 CID，标题/艺人/专辑/标签/发布者
  统一匹配，Flutter 曲库同步映射并纳入列表搜索；
- 离线刷新队列（COM-008）：网络不可用时社区源刷新进入持久队列（503 + retryable 显式
  告知），网络恢复后下一次刷新自动排空；社区页展示排队条目与立即重试；
- 播放模式（PLR-102，P1）：顺序/列表循环/单曲循环/随机——just_audio LoopMode/Shuffle
  与桥边界决策（单曲循环回拉当前曲目、顺序模式绕回队首即停止、随机先洗牌再入队），
  播放页模式按钮，模式持久化；
- 插件安装日志（PLG-013）：安装中间态（下载/验证/暂存/提交）持久化，失败保留结构化
  错误、崩溃中断重启标记 interrupted（≤64 条滚动），插件页展示；
- 声明式配置控件（PLG-014/UI-101）：插件配置 Schema 端点（JSON/DAG-CBOR 双编码解析），
  Flutter 按 Schema 渲染开关/枚举/滑杆/文本框（默认值计算），不可解析回退 JSON 编辑；
- 状态 Schema 迁移（PLG-011）：升级同 schema 完整迁移配置；跨版本从新 Schema 默认值
  开始（内容寻址 Schema 解析，规则与 Flutter 一致）并封存旧配置（previous_configuration，
  插件页展示）；schema 降级拒绝（StateSchemaDowngrade）；
- 关联 ID（NFR-012）：每个 HTTP 请求一个 v1_request span（method/path/request_id），
  Idempotency-Key 提取长度受限且不读取秘密头；
- 发布自动复刻（NOD-006/DST-010）：auto_replicate_published 开启后发布成功即把各
  rendition 内容 CID 建为幂等 Pin 传输任务并推送第三方 Pin 服务；
- CORS（API-006）：浏览器客户端跨源访问控制面（预检 + 响应头），认证仍由 Bearer
  token 强制；
- 状态版本保护（API-007/NFR-014）：五个核心存储拒绝未来 schema_version（降级保护、
  保留原文件），旧版本按 serde 默认前向兼容；
- 网络曲目播放入口（PLR-007）：网络曲目（Manifest/社区）与本地曲目在列表同一入口
  播放——按内容 CID 建立幂等 fetch 传输并边下边播；
- Rendition 选择（DST-002）：全部 rendition 源随曲库同步映射到客户端，播放前按平台
  能力（Web 容器/编解码白名单）、质量（lossless/original 优先）与网络策略（计量网络
  偏好有损流式小体积）选择内容 CID；
- 播放失败矩阵（PLR-008）：设备热拔插/丢失（write/device_write_failed 可重试）、
  打开失败（open/device_start_failed）、文件损坏（decode/decode_failed 不可重试）、
  网络中断（传输流读取端断开任务保持可恢复，播放器结构化失败且不伪装播放）；
- 音乐目录设置（UI-009）：曲库页查询/设置音乐目录（仅切换语义，明示复制/移动未实现）；
- 发布向导（UI-004）：元数据（标题/艺术家/专辑/许可证/内容标签）+ 多 rendition 编辑
  （增删、ID/容器/编解码/采样率/位深/声道/字节长度、唯一 original）校验并生成
  Manifest，签名发布后展示回执与副本健康度（本机 Pin/Provider/第三方服务数量）；
- 插件目录浏览（PLG-005）：`GET /v1/plugins/catalog` 列出社区目录收录的 PluginManifest
  条目并支持 `q` 搜索（CID/分类/标签/注解），`/catalog/{cid}` 详情解析 Manifest
  （JSON/DAG-CBOR 双编码）并给出制品可用性/已装状态/升级可用/撤销摘要；插件页
  目录浏览/搜索/详情/安装（发布者公钥与权限确认，`ipfs://CID` 直取制品）；
- 社区原生二次确认（PLG-007）：社区原生默认拒绝（CommunityNativeDenied）；高级授权
  安装前二次确认（持续警告文案），已安装插件列表永久标记“社区原生高级授权”警告条目；
- 操作取消（UI-010）：控制台“取消当前操作”关闭进行中操作的专属客户端，操作以
  “操作已取消”结束并可重新发起；
- 错误本地化（API-004）：Flutter 统一把稳定错误信封映射为本地化文案、重试建议与网络
  恢复提示，控制台横幅与传输错误行统一消费；
- Flutter 控制中心已消费 `/v1/events` SSE：sequence 缺口与 `snapshot.required` 触发整体快照
  重读、事件分组 300ms 合并定向刷新、断开退避重连与 30s 兜底轮询，不再依赖 5s 定时轮询；
- 社区源紧凑快照端点支持 gzip 传输压缩、SHA-256/字节数完整性头与 32 MiB 未压缩上限；
- Helia 依赖生产审计为 0 个漏洞；bundle 构建和 Rust↔Helia 无网关互操作通过；
- Rust workspace、Flutter Web、Flutter Linux release build 通过；Linux bundle 已注入三个
  原生库且动态依赖无缺失；
- 控制服务和应用内核心均使用 Rust TLS，workspace 依赖树不含 `native-tls`/OpenSSL；
- release 服务以旧 repo 启动并完成新增字段迁移，健康/鉴权/签名启动源冒烟通过；
- GitHub Actions 经 actionlint 1.7.7 校验通过。

## 稳定版阻断项

当前成果是 Release Candidate 源码基线，不是已经取得七端 DoD 的稳定交付。剩余阻断包括：

- Web 真实浏览器中继、Kubo、Android/iOS/HarmonyOS P2P 构建/实机及移动网络对象 UI 闭环待证；
- Web/移动插件执行载体与社区原生插件独立进程隔离待闭环；
- Web gapless/crossfade、真实独占输出、ASIO/CoreAudio Hog、ALSA hw、DSD Native/DoP 待实机；
- Flutter 本地曲库与后端 Manifest/社区曲库尚未统一；移动后台/锁屏控制缺失；
- 内置签名启动源当前没有远端 Feed 头，二维码相机扫描缺失（Feed 快照/gzip/上限已落地，远端实测待补）；
- 七端同候选安装/离线/E2E、硬件实验室、M0 资源/两小时、无障碍、迁移/断电恢复报告未取得；
- 受控 tag 的签名产物、SBOM、摘要和 provenance 尚未实际生成并复验。

上述项目关闭前，release workflow 和文档必须继续阻止“JimMusic 2.0 stable”声明。
