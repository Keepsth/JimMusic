# JimMusic 2.0 P0 需求追踪矩阵

基线日期：2026-08-15。本文是 `JimMusic需求文档.md` 的交付证据索引，不改变需求的
Definition of Done。`ALL` 或多平台需求只有在声明平台全部取得同一候选版本的构建、测试和
实机/浏览器证据后才可标记“已实现”。

状态含义：

- **本机通过**：代码入口和自动化测试已在当前 Linux 工作区通过，但仍可能缺七端证据。
- **部分实现**：已有可运行实现，但需求中的关键闭环仍缺失。
- **待外证**：代码/发布门禁已配置，结果必须由 CI、签名环境、浏览器或真实设备产生。
- **缺失**：当前仓库没有满足该需求的实现，不得在 2.0 稳定版中宣称支持。

通用验证命令：`cargo fmt --all --check`、`cargo clippy --locked --workspace --all-targets
--all-features -- -D warnings`、`cargo build --locked --workspace --all-features`、
`cargo test --locked --workspace --all-targets --all-features`、
`flutter analyze`、`flutter test`、Web Node 的 `npm audit --omit=dev` / `npm run build` /
`npm run test:interop`，以及验收报告校验器的 Node 测试。发布候选还必须通过
`.github/workflows/release.yml`。

## 产品与播放器

| ID | 状态 | 实现/证据入口 | 未闭环项 |
|---|---|---|---|
| PROD-001 | 部分实现 | 七端构建矩阵、物理设备验收矩阵与阻断式 release workflow | 七端同候选实机 P0 报告尚未取得 |
| PROD-002 | 待外证 | 本地曲库、文件播放、歌单均不依赖网关；Flutter/Rust 测试 | 七端断网场景未执行 |
| PROD-003 | 部分实现 | 原生 rust-ipfs Bitswap/Kademlia 与 Web Helia Bitswap 均不依赖 HTTP 网关；CID/签名校验和跨实现取回测试 | Android/iOS/HarmonyOS/Web 真机浏览器闭环待外证，移动 UI 的全部网络对象操作尚未统一到直连入口 |
| PROD-004 | 部分实现 | `ErrorEnvelopeV1`、播放失败、bit-perfect `unsupported`、节点 limitations | 七端 UI 错误语义未做一致性验收 |
| PROD-005 | 本机通过 | 本矩阵、`docs/RELEASE_ACCEPTANCE.md`、发布元数据 | 发布后需绑定实际 run、commit 与产物 URL |
| PLR-001 | 部分实现 | `LibraryService` 支持本地与 Manifest；Flutter 系统选择器 | Flutter 曲库尚未与后端 Manifest 曲库统一 |
| PLR-002 | 部分实现 | `library.json` 原子持久化，缺失文件保留并标记；单测 | Flutter 仍维护自己的本地曲库状态 |
| PLR-003 | 本机通过 | `Player`/`PlaybackEngine`、Rust FFI、just_audio 回退；状态/队列/FFI 测试 | 七端真实声卡/浏览器验收待执行 |
| PLR-004 | 部分实现 | Flutter 音量、静音、偏好持久化 | bit-perfect 下真实驱动会话旁路未实现 |
| PLR-005 | 本机通过 | Core 队列自动续播、`LibraryService` 命名歌单、Flutter 歌单；重启测试 | 七端 UI E2E 待执行 |
| PLR-006 | 待外证 | Symphonia 增量解码声明 MP3/AAC/FLAC/WAV/OGG/Opus | 格式语料和七端声明矩阵未运行，DSD 不支持 |
| PLR-007 | 部分实现 | `LibraryTrackV1`/`TrackSourceV1` 统一描述来源 | 网络 Source 尚未接入同一播放入口 |
| PLR-008 | 部分实现 | `PlaybackFailure` 含来源、阶段、码、重试、建议；失败测试 | 网络中断和设备热拔插矩阵不完整 |
| PLR-009 | 本机通过 | `PlaybackSessionV1` 保存曲目、队列、位置、路径且强制 `auto_play=false`；重启测试 | Flutter 与后端会话尚未统一 |
| PLR-010 | 本机通过 | `choose_source` 按 codec/质量选择，失败返回所需 codec；单测 | Flutter 尚未展示插件安装直达入口 |

## Audio Graph 与插件平台

| ID | 状态 | 实现/证据入口 | 未闭环项 |
|---|---|---|---|
| AGR-001 | 本机通过 | Symphonia `StreamingDecoder` + 有界 `PcmQueue`，不缓存整曲 PCM | 缺两小时 RSS 实测 |
| AGR-002 | 本机通过 | `AudioGraphSpecV1` 类型化 DAG、拓扑/端口/依赖预检；环路测试 | 七端同契约测试待 CI |
| AGR-003 | 部分实现 | planar f32 默认图；PCM/DSD/Encoded 端口类型在协议中 | DSD/Encoded 实际处理链未实现 |
| AGR-004 | 本机通过 | `PlanarBuffer` 由 Host 预分配；process 容量稳定测试 | 未接入系统级分配探针 |
| AGR-005 | 部分实现 | process 路径不做文件/网络 I/O，参数队列无锁 | 没有自动检测所有插件阻塞/分配违规的门禁 |
| AGR-006 | 本机通过 | 非实时编译、`ArcSwap` 原子提交、失败保留旧图与回滚测试 | 七端压力切换待验收 |
| AGR-007 | 本机通过 | 编译器插入格式转换并暴露到 Audio Path；单测/UI | 当前仅覆盖已声明转换类型 |
| AGR-008 | 本机通过 | 累积延迟、并行路径补偿和 UI 快照；对齐单测 | 动态延迟更新未实现 |
| AGR-009 | 本机通过 | 有界参数队列、timeline frame、未来事件无丢失；sample-position 测试 | 插件 DSP 处理器尚未消费完整参数协议 |
| AGR-010 | 部分实现 | Core 双解码时间线、sample-contiguous gapless、linear/equal-power crossfade、曲目边界事件与队列/FFI 测试 | Web just_audio 回退和七端音频语料验收待补 |
| AGR-011 | 部分实现 | 图提交回滚、节点 failure policy 契约 | 尚无可执行 DSP 节点崩溃/超时注入链路 |
| AGR-012 | 部分实现 | 图延迟、缓冲、deadline/underrun/overrun 统计与诊断/UI | 缺逐节点 CPU 指标 |
| AGR-013 | 部分实现 | 图/节点 DTO 可序列化；插件状态 schema 与回滚 | 音频节点状态迁移和持久图恢复不完整 |
| AGR-014 | 本机通过 | Output ABI 适配 `null-output`、CPAL system output；FFI 测试 | 非 Linux 原生输出仅待平台构建/实机证据 |
| PLG-001 | 本机通过 | 内容寻址 `PluginManifestV1` 与按 platform/arch/runtime 选择制品 | Web/iOS 目录实测待 CI |
| PLG-002 | 本机通过 | Manifest Validate 覆盖 ID、版本、发布者、ABI、权限、依赖、Schema、许可证、CID | 未知字段严格策略仍需跨版本契约测试 |
| PLG-003 | 本机通过 | 安装前同时校验 Manifest 签名、CID、SHA-256、长度与信任通道；负向测试 | 社区撤销 Feed 尚未接入 |
| PLG-004 | 本机通过 | staging、fsync、原子版本目录/状态提交、旧版本保留、启动清孤儿 | 断电故障注入仅有逻辑/文件级测试 |
| PLG-005 | 部分实现 | v1 API/UI 支持安装、启停、配置、回滚、卸载，状态持久化 | 浏览/搜索与“升级可用”远端目录不完整 |
| PLG-006 | 本机通过 | 权限声明/授权/撤销持久化；Wasmtime Host 使用 owner-scoped 不透明句柄并即时撤销；越权测试 | 七端 UI 与运行时集成证据待 RC |
| PLG-007 | 部分实现 | 官方原生、社区沙箱、社区原生高级通道与审计 | 桌面二次确认/持续警告 E2E 不完整 |
| PLG-008 | 本机通过 | service owner 注册冲突与微内核保留服务拒绝测试 | 尚无跨进程 service host |
| PLG-009 | 本机通过 | revoked manifest 在 preflight 被拒绝，可停用/隔离；社区 Policy Revoke 事件在摄取与刷新后自动应用：`active_revoke_targets` 收集生效（未过期、来源启用）目标，`revoke_release` 按 manifest CID 匹配已安装版本并置 Revoked + 推送 `plugin.state_changed`，幂等；API 测试 | 撤销 Feed 快照防回滚与七端交互待验收 |
| PLG-010 | 本机通过 | 连续失败、quarantine/safe mode；Wasmtime fuel、内存/表限制和 trap 测试 | 社区原生插件仍需独立进程超时探针 |
| PLG-011 | 部分实现 | JSON Schema 子集验证与持久配置；UI 可编辑 JSON | 敏感字段控件和完整迁移策略未实现 |
| PLG-012 | 本机通过 | platform/arch/core ABI/依赖/冲突/权限在下载前 preflight | 七端目录数据待实际验证 |
| PLG-013 | 部分实现 | lifecycle state、错误、版本、信任、权限可观测 | downloading/verifying 等中间态未持久呈现 |
| PLG-014 | 部分实现 | 配置 UI 只使用 Host Flutter 组件且不注入路由/脚本 | 尚未按声明式 Schema 渲染全部控件 |

## Bit-perfect 与节点

| ID | 状态 | 实现/证据入口 | 未闭环项 |
|---|---|---|---|
| BPT-001 | 本机通过 | Audio Path 提供显式 Bit-perfect 开关，并展示 disabled/failed/unsupported/satisfied | 七端 UI E2E 待 RC |
| BPT-002 | 部分实现 | 编译器拒绝转换/处理节点，状态列出条件和失败原因 | 真实独占会话和音量旁路未接入 |
| BPT-003 | 部分实现 | DSD/Encoded 类型端口和 EncodedPassthrough 链；图编译器拒绝接入普通 PCM Processor 的测试 | DSD/DoP 解码器与支持设备实测仍缺 |
| BPT-004 | 本机通过 | Output ABI 从实际 open 句柄返回设备、后端、共享/独占、协商格式、缓冲、时钟和 capability_source；null/CPAL/Web 插件及 FFI 测试 | 专业独占驱动与真实设备证据归 REL-004 待外证 |
| BPT-005 | 本机通过 | Core statement 与 Flutter UI 明确只声明可观察链路条件；单测 | 需七端文案截图/辅助功能证据 |
| BPT-006 | 部分实现 | 原生链路未证明独占时返回 unsupported 和原因，不静默成功 | Web Audio PCM 桥未接 Flutter，浏览器负向验收未执行 |
| BPT-007 | 部分实现 | Audio Path、FFI 和诊断包含实际打开会话格式/时钟、图延迟、缓冲和掉音统计 | 七端会话复现与专业驱动外证待补 |
| NOD-001 | 本机通过 | 本地 CAS + 持久 rust-ipfs 仓库、Pin、配额、Bitswap/Kademlia、mDNS、TCP/WebSocket/QUIC，无需 Kubo 进程 | 七端产物和网络环境待 RC |
| NOD-002 | 部分实现 | Helia 7 + IndexedDB + Bitswap + WebSocket/WebTransport/WebRTC/relay；删除 delegated HTTP router；生成 bundle 和 Rust 节点互操作测试 | 真实浏览器经测试中继的关闭网关验收待外证 |
| NOD-003 | 本机通过 | Rust Core UnixFS、Bitswap、Kademlia、mDNS、TCP/WebSocket/QUIC、Pin/持久仓库；600 KB Helia 互操作与整文件摘要测试 | Android/iOS/HarmonyOS 原生构建和 Kubo 额外实测待外证 |
| NOD-004 | 本机通过 | raw/DAG-CBOR CIDv1 及 rust-ipfs Block 校验；错误 CID 提交拒绝、跨节点 Bitswap/UnixFS 测试 | 大规模恶意 peer/fuzz 待后续强化 |
| NOD-005 | 本机通过 | Pin/Unpin/list 持久化与 ProviderHealth；重启测试/UI | 第三方 pin service 未实现 |
| NOD-006 | 部分实现 | 存储/缓存/并发/上下行/计量配置；下载限速与新任务并发更新；网络类别声明（wifi/cellular/ethernet/unknown）+ 网络策略暂停/恢复：蜂窝且未允许计量 → 全部暂停、蜂窝下 `wifi_only` 任务无条件暂停、回到允许类别只自动恢复网络暂停任务（用户手动暂停不打扰），结构化原因 `paused_wifi_only`/`paused_metered_network` 并随传输事件推送；runner 执行前复查并在被重新排队时安全中止；服务 2 项 + API 2 项测试 | 上传限速：内嵌 rust-ipfs Bitswap 无带宽节流，API 按 PROD-004 显式拒绝（`unsupported` + reason），UI 明示暂不支持；自动复刻未实现 |
| NOD-007 | 本机通过 | `/v1/diagnostics` + UI 脱敏快照包含真实传输/路由/计数；精确 peer/listener 地址刻意不进入可分享报告 | 七端抓包与隐私复验待 RC |
| NOD-008 | 部分实现 | Web pagehide/pageshow/visibility 与 Android/iOS/HarmonyOS/桌面 Rust FFI 生命周期；UI 明示后台降级且关闭后不持续 | 移动系统后台限制和打包结果待物理设备外证 |
| NOD-009 | 本机通过 | 内置 Bitswap 是首选网络路径，显式配置的 Kubo HTTP 仅作兼容回退；所有路径提交前验 CID | 网关关闭七端 E2E 待验收 |
| NOD-010 | 本机通过 | Ed25519 protobuf 节点密钥 0600 持久化，PeerId 重启稳定测试 | 系统安全存储/节点轮换 UI 未实现 |

## 发布、分发与社区

| ID | 状态 | 实现/证据入口 | 未闭环项 |
|---|---|---|---|
| DST-001 | 本机通过 | 版本化音乐 DTO、严格 canonical DAG-CBOR、CIDv1、签名测试 | 跨语言/七端 golden vectors 待补 |
| DST-002 | 部分实现 | Manifest 支持多个 rendition、original/lossless/codec 元数据 | 发布 UI 不会生成兼容 rendition |
| DST-003 | 部分实现 | 校验、CID、签名、Pin、Feed、receipt 同节点事务；发布块同步到内置 P2P 仓库并可由 Helia 取回 | 另一节点从 Feed 解析到播放器的 UI 闭环待补 |
| DST-004 | 本机通过 | Ed25519 身份生成、Argon2id+XChaCha20 加密导出/导入/轮换/撤销；负向测试/UI | 系统钥匙串集成未实现 |
| DST-005 | 本机通过 | publish/update/tombstone 序列、previous CID、签名、防重放；测试 | 远端 Feed 分叉协调策略待 P2P |
| DST-006 | 本机通过 | -100..100 持久优先级、并发槽前确定性选队、运行时调优先级、暂停/恢复/取消/重试/进度与重启恢复；API/UI/测试 | 真正 kill -9/断电矩阵待七端 RC |
| DST-007 | 本机通过 | 下载与解码均有界流式；`/v1/transfers/{id}/stream` 跟随 part 文件增长流式输出（64 KiB 块、单范围 `Range`、任务终结后服务完尾部并结束、失败/取消 409）；整块路径（本地 CAS/P2P）落地后写入 part 供播放交接；完成 part 保留，孤儿文件由启动/流端点清理；Flutter 边下边播音源经 just_audio 代理注入 Bearer 鉴权（令牌不交给平台播放器），播放器 Seek 走 Range 只取已下载前缀；传输页提供“边下边播”入口；Rust API 测试 4 项 + Flutter 4 项（真实代理链路 + 鉴权头 + 失败路径） | 七端实机与真实网络抖动下的续播/切离线源待验收；P2P 为整块落地后起播（字节级渐进流为已知限制）；Web 端整段缓冲（浏览器限制） |
| DST-008 | 本机通过 | `.part` 流式校验后原子提交本地 CAS；错误内容永不入库；集成测试 | 目标音乐目录提交尚未统一 |
| DST-009 | 本机通过 | 发布默认 Pin，UI 显示本机 Pin/Provider 健康（configured_pin_services 回填配置）；收藏协助 Pin（显式开关 assist_pin_favorites：本地已有对象直接 Pin、否则幂等 Pin 传输任务并服从网络策略）、显式 Pin 与发布后把 CID 推送第三方 Kubo 兼容 Pin 服务；端点校验（http(s)/无凭据/≤16 个/长度上限）；API 测试 2 项 | 第三方服务可用性监控与七端交互待验收 |
| DST-010 | 部分实现 | 并发、下载限速、计量网络、缓存配置；网络类别声明（Wi-Fi/蜂窝/有线/未知）驱动仅 Wi-Fi 与计量开关的暂停/恢复 | 蜂窝额度与自动复刻未实现；网络类别目前由用户在设置中声明（未接系统连通性监听） |
| DST-011 | 本机通过 | tombstone 只更新 Feed；UI/文档明确 CID 不可删除 | 七端文案验收待执行 |
| DST-012 | 本机通过 | Public Manifest 缺许可证/权利声明时发布失败；单测 | 发布向导仍是高级 JSON 输入 |
| COM-001 | 本机通过 | CommunitySourceManifest、维护者签名、Catalog/Policy 独立开关与 UI | 启动源当前为空 Feed |
| COM-002 | 本机通过 | 双 Feed 序列/previous CID/时间/签名/到期校验；负向测试 | 大型远端 Feed 跨版本回放待验收 |
| COM-003 | 本机通过 | 已启用 Catalog 与本地索引可合并搜索，精确 CID API 存在；直接关注发布者：`POST/GET/DELETE /v1/community-sources/follows` 持久化关注（上限 4096），关注后把目录中该发布者的 Music Manifest 解析（本地 CAS/P2P、签名校验、单次 ≤128）导入媒体库，禁用/删除全部 Catalog 后仍可搜索播放（关注记录与曲库保留，取消关注不删用户数据）；社区页新增关注发布者 UI；API 测试 1 项 + Flutter 测试 1 项 | 关注后的增量刷新与七端交互待验收 |
| COM-004 | 本机通过 | 同 Manifest CID 去重，社区 annotation 与签名 Manifest 分层 | 跨源大数据集重建待验收 |
| COM-005 | 部分实现 | 本地索引支持目标、类别、标签、来源查询 | 标题/艺人/专辑/发布者的统一全文索引不完整 |
| COM-006 | 部分实现 | warn/demote/hide/block/revoke 决策、范围/到期、policy API | 详情与精确打开入口尚未统一应用策略 |
| COM-007 | 本机通过 | 最高严重度、信任顺序、本地 block 优先与来源解释测试 | UI 冲突矩阵 E2E 待补 |
| COM-008 | 本机通过 | Catalog/Policy 分别启停、刷新、删除并清理索引/策略；API/UI | 离线刷新队列未实现 |
| COM-009 | 部分实现 | 内置签名 bootstrap 可独立禁用/永久移除且重启不复现；支持裸 CID、ipfs://、ipns://、jimmusic:// URI 与粘贴式 UI；测试 | 启动源尚未发布远端 Feed 头，缺相机二维码扫描与 IPNS/Kubo 互操作实测 |
| COM-010 | 本机通过 | ModerationReport 验签/匿名约束；X25519 + XChaCha20-Poly1305 封装/解密/篡改拒绝；持久离线队列、30s–1h 指数退避、显式重试；明文不出站测试；API/UI | 七端 UX、真实维护者端和抓包证据待 RC |
| COM-011 | 部分实现 | policy decision 返回来源、动作、原因、到期和本地覆盖 | UI 申诉/逐条覆盖非强制策略未实现 |
| COM-012 | 本机通过 | 按头 CID 增量回溯并限制 hop、已见事件不重复摄取；FeedLimits（事件数/字节）在摄取时强制；紧凑快照（每目标最新未过期事件）锚定签名事件链头，快照端点支持 `Accept-Encoding: gzip` 传输压缩、`x-snapshot-sha256`/`x-snapshot-bytes` 完整性头与 32 MiB 未压缩上限（超限 413 结构化错误）；API 测试 | 大型远端 Feed 的跨版本压缩快照与七端消费实测待验收 |
| COM-013 | 本机通过 | 连续 key event Feed；轮换需旧/新双签名，撤销终止后续 Feed，未知直接替换被拒；API/UI/负向测试 | 七端交互与恢复证据待 RC |

## UI、API 与安全

| ID | 状态 | 实现/证据入口 | 未闭环项 |
|---|---|---|---|
| UI-001 | 本机通过 | 播放页来自 Rust Bridge/just_audio 真实事件，错误可见，无模拟 timer；来源标签（Rust Core 输出/本地文件/内存字节/IPFS 边下边播/CID/社区）、just_audio 缓冲位置进度与边下边播传输下载状态（字节/状态/Provider）全部来自真实服务状态；小屏改为可滚动布局；4 项测试 | 七端截图/无障碍证据待 RC |
| UI-002 | 部分实现 | 本地导入、扫描、搜索、排序、缺失标记 | 后端 IPFS/社区曲库未接入同一 Flutter 列表 |
| UI-003 | 本机通过 | 传输页显示状态、优先级、速度、Provider、校验/提交、目标和错误；可调优先级/暂停/恢复/取消/重试 | 七端截图与辅助功能证据待 RC |
| UI-004 | 部分实现 | 身份、签名、publish/update/tombstone、Pin receipt 入口 | 缺完整元数据/rendition 表单和副本向导 |
| UI-005 | 本机通过 | 社区页独立开关、维护者/密钥/Manifest/序列/错误与刷新 | 策略冲突的内容级解释入口不完整 |
| UI-006 | 本机通过 | 插件页展示信任、权限、版本、依赖、冲突、状态、错误、回滚和配置 | 远端兼容目录未实现 |
| UI-007 | 本机通过 | Audio Path 显示节点、转换、延迟补偿、缓冲、实时统计，并展示应用内 Rust Core 已打开输出会话的设备、驱动、协商格式、缓冲、时钟与证据来源 | 应用内会话和控制服务音频图仍是两个进程视图，七端同一会话快照待 RC 验收 |
| UI-008 | 部分实现 | 显示 bit-perfect 状态、逐项条件、失败原因和真实协商格式；无会话证据时明确不宣称成功 | 尚无被证明为 exclusive 的专业驱动会话与物理设备证据 |
| UI-009 | 部分实现 | 节点配额/并发/限速/计量、输出和插件设置可持久化 | 音乐目录、pin service 与全部安全偏好未统一 |
| UI-010 | 部分实现 | 全局加载/错误、传输细状态和重试、结构化失败 | 多个短操作仍无独立进度/取消状态 |
| API-001 | 部分实现 | protocol DTO + Rust 服务 + `/v1` HTTP + FFI 播放桥 | JS/WASM 与所有服务的契约一致性测试不全 |
| API-002 | 本机通过 | DTO/事件/错误/网络对象 `schema_version`，严格 canonical 解码限制；协议测试 | 跨语言兼容向量待补 |
| API-003 | 本机通过 | 所有 HTTP mutation 强制 Idempotency-Key/request_id，指纹冲突和持久 replay；API 测试 | 旧 legacy 路由只返回 410，不迁移副作用 |
| API-004 | 部分实现 | 稳定错误信封与播放失败模型 | Flutter 尚未按所有机器码做一致本地化动作 |
| API-005 | 本机通过 | 单调 sequence SSE、after 检测、snapshot.required 与快照端点；Flutter 已改为消费 `/v1/events`：sequence 缺口与 snapshot.required 触发整体重读、事件分组 300ms 合并定向刷新、断开退避重连与 30s 兜底轮询；解析器/真实 HTTP SSE/Provider 测试（`control_api_sse_test.dart`，13 项） | Web fetch 流式 SSE 路径与七端断线重连待实机/浏览器验收 |
| API-006 | 部分实现 | 默认回环、启动必需 bearer token、常量时间校验；Flutter 拒绝非 HTTPS 远程 | 服务端 TLS/CORS/远程开启审计未完整实现 |
| API-007 | 部分实现 | `docs/API_MIGRATION.md`、v1 路径、状态 schema、回滚原则 | 数据库升级/降级测试矩阵不完整 |
| SEC-001 | 部分实现 | 发布者 seed 仅存在于加密 bundle；状态文件 0600；明文搜索测试 | 未使用系统 Keychain/Keystore，节点 key 为 0600 文件 |
| SEC-002 | 本机通过 | Argon2id + XChaCha20-Poly1305，错误口令/篡改/roundtrip 测试 | KDF 参数迁移测试待补 |
| SEC-003 | 本机通过 | v1 插件签名强制，错误/缺失签名拒绝；测试 | legacy API 仍保留只读/410 兼容壳 |
| SEC-004 | 部分实现 | Wasmtime 47 默认无 WASI/环境 import；fuel、内存/表上限、owner-scoped capability handle、猜测/撤销/网络与文件越权负向测试；生命周期/API 已接 supervisor | Web/iOS/HarmonyOS 插件执行载体和七端恶意样本外证待补 |
| SEC-005 | 部分实现 | 社区原生默认拒绝、高级授权通道、审计、安全模式 | 原生插件仍未独立进程隔离，二次确认 E2E 不完整 |
| SEC-006 | 本机通过 | 默认 `127.0.0.1`、随机 256-bit token 0600、所有路由鉴权；测试 | 远程 TLS 需受控反向代理，发布环境测试待补 |
| SEC-007 | 本机通过 | CAS/网关/Bitswap 块 CID、Manifest/Feed/插件签名及重放回滚检查；篡改和错误 CID 负向测试 | 七端恶意 peer 压力验收待 RC |
| SEC-008 | 本机通过 | 许可证/权利、内容标签、发布者签名 Validate；发布测试 | UI 向导仍允许高级 JSON 编辑 |
| SEC-009 | 部分实现 | 本地 block、社区 policy、来源解释、签名举报队列与重试 | UI 申诉和跨入口策略执行仍不完整 |
| SEC-010 | 本机通过 | 脱敏诊断排除秘密/路径；匿名举报禁止身份字段，加密举报只发送 envelope；隐私测试 | 需七端抓包复验 |
| SEC-011 | 部分实现 | 身份轮换/撤销签名、防旧 Feed 重放；插件 revoked preflight | 插件撤销 Feed 快照防回滚未实现 |
| SEC-012 | 本机通过 | DAG-CBOR 深度/大小/集合/文本限制，HTTP body/response/artifact/feed hop 限制；负向测试 | 尚无持续 fuzz job |

## 非功能与发布

| ID | 状态 | 实现/证据入口 | 未闭环项 |
|---|---|---|---|
| NFR-001 | 待外证 | UI 无模拟 ticker，常用操作本地化 | 未采集七端 P95 |
| NFR-002 | 待外证 | 增量解码与真实播放路径 | 未采集格式/平台 P95 |
| NFR-003 | 待外证 | seek 状态与引擎入口 | 未采集本地可 seek 格式 P95 |
| NFR-004 | 部分实现 | process elapsed/deadline miss 统计 | 发布构建没有持续 deadline 基准 |
| NFR-005 | 待外证 | PCM 队列/缓冲有固定上限 | 未完成两小时 RSS/泄漏测试 |
| NFR-006 | 本机通过 | 所有提交前 CID 校验，篡改负向测试 | P2P 大规模互操作待实现 |
| NFR-007 | 本机通过 | 插件 staging/回滚、图原子 swap/rollback、CAS 原子 commit；故障测试 | 断电实机矩阵不完整 |
| NFR-008 | 待外证 | 机器校验器强制七端 M0 启动/峰值内存/每小时耗电/每小时带宽、至少五样本并重算 15% 回退；release 依赖七个物理 runner | 当前仓库不含也不伪造七端 M0 测量值 |
| NFR-009 | 部分实现 | 本地隐私会话账本以启动 marker/正常关闭区分 clean 与 unclean，重启累计并在 health/诊断输出聚合 crash-free rate；测试 | 无七端长期样本，不能宣称达到 99.5% |
| NFR-010 | 待外证 | CI 配置 llvm-cov 80% 门槛 | 当前提交需由 CI 重新生成覆盖率报告 |
| NFR-011 | 部分实现 | Flutter Material/SelectableText/标准控件 | 未做键盘、读屏、200% 缩放和对比度审计 |
| NFR-012 | 部分实现 | 结构化错误/事件含子系统、操作和 request_id；tracing | 全链路 correlation ID 和日志脱敏审计不完整 |
| NFR-013 | 待外证 | 曲库/歌单/插件/本地 CAS 均为本地原子状态 | 七端离线场景未跑 |
| NFR-014 | 部分实现 | Schema version、原子状态、API migration 文档 | 自动向前迁移与降级恢复测试不完整 |
| REL-001 | 本机通过 | 本文逐项映射 ID、代码、测试、平台缺口 | 发布后需补实际构建产物链接 |
| REL-002 | 待外证 | release workflow 要求七个物理 runner 对同一 commit 提交全部适用 P0，`exemptions` 必须为空，聚合校验后方可发布 | 受控七端 runner 尚未执行本候选版本 |
| REL-003 | 待外证 | CI 生成七端制品、源码、SHA256、SPDX SBOM、provenance | 必须在受控 tag run 中实际产生并验证 |
| REL-004 | 待外证 | 验收校验器要求 supported 声明携带设备、驱动、实际协商格式和摘要证据；无证据只能明确 unsupported | 尚无 WASAPI Exclusive/CoreAudio Hog/ALSA hw/DSD 物理设备报告 |
| REL-005 | 本机通过 | README、项目总结、迁移、限制、验收与追踪文档；release 文档门禁 | 插件 SDK 独立文档仍需完善后才能稳定发布 |
| REL-006 | 部分实现 | 原子存储、幂等恢复、队列重启、插件回滚测试 | 七端升级/降级/杀进程/断电/安全模式 RC 矩阵未执行 |

## 稳定版阻断结论

当前代码可作为 **2.0 Release Candidate 源码基线**，但在七端物理报告到齐前不能作为满足需求文档
DoD 的 2.0 稳定版。代码侧已关闭原先完全缺失的 gapless/crossfade、打开会话证据、原生 P2P、
Web Helia 与 Wasmtime 沙箱；仍需关闭 PROD-001/003、AGR-010、BPT-002/003、NOD-002/008、
COM-009、SEC-004、NFR-009、REL-006 的剩余闭环，并取得 NFR-001/002/003/005/008/010/013、
REL-002/003/004 以及全部七端/硬件/浏览器外证。
