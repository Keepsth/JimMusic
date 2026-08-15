# JimMusic 产品与软件需求规格说明书

| 元数据   | 内容                                                                |
| -------- | ------------------------------------------------------------------- |
| 文档版本 | 2.0                                                                 |
| 文档状态 | 需求基线                                                            |
| 更新日期 | 2026-08-15                                                          |
| 适用产品 | JimMusic 2.x                                                        |
| 适用读者 | 产品、架构、Rust/Flutter/Web 开发、插件作者、测试、发布与安全维护者 |

---

## 0. 版本历史

| 版本 | 日期       | 说明                                                                                                                                               |
| ---- | ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1.0  | 2025-07-23 | 初版，确定跨平台、模块化音乐播放器方向                                                                                                             |
| 1.1  | 2025-07-23 | 增加 IPFS 网络接入需求                                                                                                                             |
| 1.2  | 2026-08-14 | 移除独立 ArkUI 开发，HarmonyOS 统一使用 Flutter 适配                                                                                               |
| 1.3  | 2026-08-14 | 增加 AudioOutput、Playback Engine 与 Web Audio 桥                                                                                                  |
| 1.4  | 2026-08-14 | 增加音量、插件配置、音乐目录与自动识别需求                                                                                                         |
| 2.0  | 2026-08-15 | 重构为可追踪 PRD + SRS；定义七端发布、内置 IPFS 节点、社区发现/过滤双 Feed、可信微内核、三级插件载体、实时音频图、Audio ABI v2 与 Bit-perfect 能力 |

## 1. 文档约定

### 1.1 需求编号

| 前缀 | 子系统                 |
| ---- | ---------------------- |
| PROD | 产品与发布原则         |
| PLR  | 基础播放器             |
| AGR  | 实时音频图             |
| PLG  | 插件平台               |
| BPT  | Bit-perfect 与硬件音频 |
| NOD  | IPFS 节点              |
| DST  | 音乐发布、下载与分发   |
| COM  | 社区源、发现与治理     |
| UI   | Flutter/Web 用户界面   |
| API  | 服务接口与数据契约     |
| SEC  | 安全、隐私与内容治理   |
| NFR  | 非功能需求             |
| REL  | 测试、验收与发布       |

需求编号一经发布不得复用。被废弃的需求保留编号并标记为“废弃”。

### 1.2 优先级

- **P0**：JimMusic 2.0 发布阻断项。
- **P1**：2.x 版本必须实现，但不阻断首个 2.0 稳定版。
- **P2**：远期能力或生态扩展项。

### 1.3 状态

- **已实现**：当前仓库已形成真实用户闭环并有自动化或实机证据。
- **部分实现**：存在代码或界面，但链路、平台覆盖或验收证据不完整。
- **未实现**：仅有需求、原型、占位或尚无代码。
- **待验证**：已有实现，但缺少目标平台或硬件实测。

文档中的“已实现”不能由 README 描述、模拟播放、静态列表或仅单元测试替代。

### 1.4 平台缩写

| 缩写 | 平台      |
| ---- | --------- |
| WIN  | Windows   |
| MAC  | macOS     |
| LNX  | Linux     |
| AND  | Android   |
| IOS  | iOS       |
| HOS  | HarmonyOS |
| WEB  | Web       |

“ALL”表示以上七个平台。“用户结果一致”不表示底层运行时、系统 API 或可安装插件制品完全相同。

### 1.5 验收规则

每项 P0 需求必须同时具备：

1. 可执行的自动化测试或明确的实机步骤；
2. 对应平台构建产物；
3. 结构化成功、失败与不支持状态；
4. 可追踪的日志、指标或测试报告；
5. 不依赖演示数据、模拟定时器或开发机外部服务才能成立。

---

## 2. 产品定义

### 2.1 产品愿景

JimMusic 是一款轻量、开放、跨平台、本地优先的音乐播放器。它以 Rust 可信微内核和 Flutter 单一界面代码库为基础，提供：

- 成熟可靠的本地与 IPFS 音乐播放；
- 不依赖中心账号的发布者身份和音乐分发；
- 可订阅、可验证、可替换的社区音乐发现与内容治理来源；
- 可下载、验证、授权、升级、回滚的插件生态；
- 由插件提供的高质量 DSP、硬件输出、DSD/DoP 和其他发烧级能力。

### 2.2 目标用户

- 需要跨设备播放本地和去中心化音乐的普通用户；
- 需要发布原创或已获授权音乐的创作者；
- 维护音乐目录与治理策略的社区；
- 开发解码器、DSP、发现源和输出插件的开发者；
- 使用独占输出、ASIO、DSD/DoP 等硬件能力的高级用户。

### 2.3 P0 产品范围

- 七个平台完成同一套基础播放器用户流程；
- 七个平台均包含可参与 IPFS 的应用内节点能力；
- 完成音乐按 CID 获取、边下边播、离线保存、发布与 Pin；
- 完成发布者签名 Feed、本地索引和社区源双 Feed；
- 完成插件发现、下载、验签、安装、启停、授权、升级、回滚、卸载和撤销；
- 建立实时音频图和 Audio ABI v2，允许发烧级功能作为插件接入；
- 建立显式 Bit-perfect 状态模型和硬件能力协商；
- 形成跨平台、网络、插件、安全和硬件验收证据。

### 2.4 P1 范围

- 后台播放与系统媒体控制；
- 随机、单曲循环、列表循环和智能队列；
- Gapless、Crossfade、ReplayGain、参数均衡器、卷积和频谱分析参考插件；
- 更多 Pin 服务、社区源发现方式和跨设备身份备份；
- 受控的 VST3、Audio Unit 或 LV2 Adapter Host 可行性验证。

### 2.5 非目标

JimMusic 2.0 不包含：

- DRM 或绕过 DRM 的能力；
- 付费音乐商店、插件付费或中心账户体系；
- 对 IPFS 上内容的全局删除承诺；
- 对所有平台提供完全相同的硬件插件；
- 在未实测的设备上宣称 Bit-perfect；
- 不受权限控制的任意原生代码商店；
- 完整兼容 VST3、Audio Unit、AAX 或 LV2 生态。

### 2.6 产品原则

| ID       | 优先级 | 平台 | 状态     | 要求                                                             | 验收                                                      |
| -------- | ------ | ---- | -------- | ---------------------------------------------------------------- | --------------------------------------------------------- |
| PROD-001 | P0     | ALL  | 部分实现 | 七个平台以同一 P0 用户结果作为同日发布门槛                       | 任一平台缺少 P0 闭环时不得发布 2.0 稳定版                 |
| PROD-002 | P0     | ALL  | 部分实现 | 本地播放在断网和 IPFS 不可用时保持可用                           | 断网启动、扫描、播放、Seek、歌单均成功                    |
| PROD-003 | P0     | ALL  | 部分实现 | 网络对象内容寻址、签名可验证，不以单一 HTTP 服务作为唯一可信来源 | 关闭默认网关后仍可通过至少一种 P2P 路径获取并验证测试对象 |
| PROD-004 | P0     | ALL  | 部分实现 | 不受支持的能力必须显式返回 unsupported，不得静默模拟成功         | UI、API 和日志均展示结构化不支持原因                      |
| PROD-005 | P0     | ALL  | 部分实现 | 所有“完成”声明必须关联需求 ID 和证据                           | 发布清单可追踪到测试与构建产物                            |

---

## 3. 平台与发布矩阵

| 平台      | UI           | IPFS 节点形态                                   | 可执行插件载体                                 | 音频输出                        | P0 结果                         |
| --------- | ------------ | ----------------------------------------------- | ---------------------------------------------- | ------------------------------- | ------------------------------- |
| Windows   | Flutter      | Rust Core 内置节点能力                          | 声明式、WASM、签名原生；高级模式可授权社区原生 | WASAPI，共享/独占；ASIO 插件    | 全部 P0                         |
| macOS     | Flutter      | Rust Core 内置节点能力                          | 声明式、WASM、签名原生                         | CoreAudio，共享/独占或 Hog 能力 | 全部 P0                         |
| Linux     | Flutter      | Rust Core 内置节点能力                          | 声明式、WASM、签名原生；高级模式可授权社区原生 | ALSA/PipeWire                   | 全部 P0                         |
| Android   | Flutter      | Rust Core 内置节点能力                          | 声明式、WASM、随包或平台允许的原生模块         | AAudio/OpenSL 或平台适配        | 全部 P0                         |
| iOS       | Flutter      | Rust Core 内置节点能力                          | 声明式；随应用审核的 WASM/原生模块             | AudioUnit/AVAudioSession 适配   | 全部 P0；不承诺下载执行新代码   |
| HarmonyOS | Flutter 适配 | Rust Core 内置节点能力                          | 声明式、WASM、随包或平台允许的原生模块         | 平台音频 API 适配               | 全部 P0                         |
| Web       | Flutter Web  | Helia/Verified Fetch 与浏览器支持的 libp2p 传输 | 声明式、WASM                                   | Web Audio Worklet               | 全部 P0；后台提供能力为尽力而为 |

平台规则：

- 插件管理 UI、兼容性判断、权限展示和生命周期结果必须七端一致。
- 某插件没有当前平台制品时，显示“不兼容”及缺失能力，不下载错误制品。
- iOS App Store 构建不得下载并执行会改变应用功能的独立原生代码；可下载声明式数据包，执行模块须满足 Apple 审核规则。
- Web 节点在页面活跃期间完整参与检索、验证和浏览器允许的提供行为；页面关闭后不承诺持续在线。
- HarmonyOS 构建工具链必须锁定版本，并由独立 CI/实机证据验证。

参考：

- Apple App Review Guidelines：https://developer.apple.com/app-store/review/guidelines/
- IPFS in Web Applications：https://docs.ipfs.tech/how-to/ipfs-in-web-apps/
- IPFS Nodes：https://docs.ipfs.tech/concepts/nodes/

---

## 4. 总体架构

### 4.1 架构视图

```text
+---------------------------------------------------------------+
| Flutter UI: Player / Library / Transfers / Publish / Plugins  |
|             Community Sources / Audio Path / Settings          |
+-----------------------------+---------------------------------+
                              |
                    Versioned Service API
                 Native FFI/IPC | Web JS/WASM
                              |
+-----------------------------v---------------------------------+
|                     Trusted Microkernel                        |
| Identity | Crypto | Permission | Storage Isolation | ABI       |
| Install Transaction | Revocation | Safe Mode | Event Routing   |
+------+-------------+----------------+--------------------+-----+
       |             |                |                    |
       v             v                v                    v
 PlaybackService  NodeService   PluginLifecycleService  IndexService
       |             |                |                    |
       v             v                v                    v
 Audio Graph     IPFS/libp2p      Plugin Runtimes      Community Feeds
       |
       +--> Async Source/Demux/Decode/Prefetch
       |
       +--> Real-time Timeline -> Typed DSP DAG -> Output
                              +-> Analyzer/Meter Taps
```

### 4.2 可信微内核边界

可信微内核不可被插件替换，只负责：

- 本地身份、密钥存储、签名与验签；
- 权限判定、能力句柄和隔离存储；
- 插件包验证、事务安装、版本回滚和撤销；
- ABI/协议版本协商；
- Host 缓冲池、实时线程规则与故障隔离；
- 核心配置、审计日志和安全模式；
- 服务注册、依赖解析和结构化事件路由。

以下业务服务允许插件替换，但只能通过能力接口访问微内核：

- PlaybackService；
- AudioGraphService；
- LibraryService；
- TransferService；
- PublicationService；
- CommunitySourceService；
- IndexService；
- NodePolicyService。

安全、签名、权限、插件加载器和恢复机制不得由普通插件覆盖。

### 4.3 控制面与数据面

- **控制面**：曲目选择、图构建、插件生命周期、权限、预设、下载任务、社区源和参数更新。
- **异步数据面**：文件/IPFS I/O、Demux、Decode、预取、离线分析和数据库写入。
- **实时数据面**：固定块音频处理、时间线、DSP DAG、混音和设备输出。
- 控制面不得直接占用音频回调线程；实时数据面不得调用网络、文件、数据库或 UI。

---

## 5. 功能需求

### 5.1 基础播放器

| ID      | 优先级 | 平台     | 状态     | 要求                                                                 | 验收                                           |
| ------- | ------ | -------- | -------- | -------------------------------------------------------------------- | ---------------------------------------------- |
| PLR-001 | P0     | ALL      | 部分实现 | 支持从系统选择器、应用音乐目录和 IPFS Manifest 导入音乐              | 三种来源均进入同一媒体库模型并可播放           |
| PLR-002 | P0     | ALL      | 部分实现 | 媒体库持久化保存曲目、来源、CID、本地路径、可用 rendition 和扫描状态 | 重启后曲库一致；丢失文件标记不可用而非静默删除 |
| PLR-003 | P0     | ALL      | 部分实现 | 支持播放、暂停、恢复、停止、Seek、上一首和下一首                     | 真实音频状态与 UI 状态一致，不使用模拟计时器   |
| PLR-004 | P0     | ALL      | 部分实现 | 支持 0.0 至 1.0 音量、静音和偏好持久化                               | 重启恢复；Bit-perfect 时按 BPT 规则旁路并说明  |
| PLR-005 | P0     | ALL      | 部分实现 | 支持简单播放队列和命名歌单的增删改                                   | 队列切歌、歌单重启恢复和文件失效处理通过       |
| PLR-006 | P0     | ALL      | 部分实现 | 支持 MP3、AAC/M4A、FLAC、WAV、OGG/Opus；PCM 与 DSD 由能力插件声明    | 格式语料库在声明支持的平台全部通过             |
| PLR-007 | P0     | ALL      | 部分实现 | 本地文件、缓存对象和 IPFS 流使用同一 Track/Source 抽象               | 切换来源不改变上层播放 API                     |
| PLR-008 | P0     | ALL      | 本机通过 | 播放错误返回来源、阶段、错误码、可重试性和用户建议                   | 文件损坏、网络中断、解码失败、设备丢失均可区分 |
| PLR-009 | P0     | ALL      | 部分实现 | 保存当前曲目、队列、播放位置和用户选择的音频路径                     | 异常退出后恢复到可解释状态，不自动播放         |
| PLR-010 | P0     | ALL      | 部分实现 | 不兼容 rendition 时选择兼容版本或提示所需解码插件                    | 不允许静默转码或模拟播放                       |
| PLR-101 | P1     | 原生平台 | 未实现   | 后台播放、锁屏/通知栏控制和音频焦点处理                              | 来电、耳机拔出、后台恢复场景通过实机测试       |
| PLR-102 | P1     | ALL      | 已实现   | 随机、单曲循环、列表循环和队列编辑                                   | 模式切换及队列边界行为有自动化测试             |
| PLR-103 | P1     | 支持平台 | 未实现   | Gapless、Crossfade、ReplayGain 等由插件提供                          | 禁用插件后 Core 仍能基础播放                   |

### 5.2 实时音频图

#### 5.2.1 目标拓扑

```text
Async Plane:
  ByteSource -> Demuxer -> Decoder -> Bounded Prefetch Buffers
                                     | Track A
                                     | Track B for gapless/crossfade

Real-time Plane:
  Timeline
     -> Format Negotiation
     -> Typed Audio DAG
          -> Processor Chain
          -> Parallel Analyzer Taps
          -> Mixer / Limiter
     -> Output Endpoint
```

#### 5.2.2 图需求

| ID      | 优先级 | 平台     | 状态     | 要求                                                                   | 验收                                       |
| ------- | ------ | -------- | -------- | ---------------------------------------------------------------------- | ------------------------------------------ |
| AGR-001 | P0     | ALL      | 待验证   | 解码改为增量流式处理，不得默认将整首曲目 PCM 常驻内存                  | 播放两小时音频时 PCM 内存保持有界          |
| AGR-002 | P0     | ALL      | 待验证   | 实时处理使用类型化有向无环图，端口声明音频类型和格式约束               | 非法环、端口不匹配和缺失依赖在提交前被拒绝 |
| AGR-003 | P0     | ALL      | 部分实现 | 默认 PCM 内部格式为 planar f32；支持整数 PCM、DSD 和 Encoded 类型端口  | 格式协商测试覆盖所有声明类型               |
| AGR-004 | P0     | ALL      | 待验证   | Host 持有预分配缓冲池，插件仅使用 AudioBufferViewV2                    | process 路径无堆分配且无悬空缓冲           |
| AGR-005 | P0     | ALL      | 部分实现 | 实时线程禁止阻塞锁、文件/网络 I/O、日志和不可控系统调用                | 测试构建可检测分配和阻塞违规               |
| AGR-006 | P0     | ALL      | 待验证   | 图在非实时线程解析、验证、协商和编译，成功后原子切换                   | 无效图不影响当前播放；切换不崩溃           |
| AGR-007 | P0     | ALL      | 待验证   | Core 根据能力插入必要的重采样和声道映射节点                            | 自动插入节点在 Audio Path UI 可见          |
| AGR-008 | P0     | ALL      | 待验证   | 节点声明固定/动态延迟和尾音，Core 执行全图延迟补偿                     | 并行路径输出对齐到声明容差                 |
| AGR-009 | P0     | ALL      | 待验证   | 参数事件通过无锁队列传递，支持时间戳和 sample-accurate automation      | Golden Test 验证事件落在目标采样帧         |
| AGR-010 | P0     | ALL      | 部分实现 | Gapless/Crossfade 使用双解码时间线，不由输出后端推断曲目边界           | 连续语料无额外空帧；淡化曲线符合预设       |
| AGR-011 | P0     | ALL      | 部分实现 | 插件失败时按策略旁路节点、回退旧图或停止，不传播未初始化数据           | 注入崩溃和超时后行为与策略一致             |
| AGR-012 | P0     | ALL      | 部分实现 | 暴露 deadline miss、underrun、overrun、图延迟、缓冲占用和节点 CPU 指标 | UI/诊断包可读取并关联到节点                |
| AGR-013 | P0     | ALL      | 部分实现 | 图和节点状态可序列化，插件升级支持状态迁移                             | 升级、降级和迁移失败回滚场景通过           |
| AGR-014 | P0     | 原生平台 | 部分实现 | Output ABI v1 经适配器接入 v2 图，但不得声明 v2 高级能力               | 现有 null/ALSA 输出可播放且能力标记准确    |
| AGR-101 | P1     | ALL      | 未实现   | 支持旁链、反馈延迟节点和多总线扩展                                     | 只有显式有界 Delay 节点可以形成受控反馈    |

### 5.3 发烧级插件类型

| 插件类型           | 数据方向                   | 典型能力                                 | 实时要求                   |
| ------------------ | -------------------------- | ---------------------------------------- | -------------------------- |
| AudioDecoder       | 压缩/封装数据到 PCM 或 DSD | FLAC、AAC、FFmpeg、DSD 解码              | 解码运行在异步层           |
| AudioProcessor     | PCM 到 PCM                 | EQ、卷积、Crossfeed、压缩器              | process 必须实时安全       |
| AudioAnalyzer      | PCM 只读 Tap               | 频谱、VU、响度、波形                     | 不得阻塞主图；允许降采样   |
| AudioResampler     | PCM 到不同采样率 PCM       | 高质量 SRC、时钟补偿                     | 声明延迟和质量模式         |
| PlaybackTransition | 双时间线到混合时间线       | Gapless、Crossfade、预卷                 | 声明所需前后缓冲           |
| AudioOutput        | 音频流到设备               | WASAPI、ASIO、CoreAudio、ALSA、Web Audio | 声明共享/独占和设备格式    |
| EncodedPassthrough | Encoded/DSD 到设备         | DSD Native、DoP                          | 禁止进入普通 PCM DSP       |
| ControlPanel       | 参数 Schema 到声明式 UI    | 滑块、枚举、仪表、预设                   | 不执行任意 Flutter UI 代码 |

### 5.4 插件平台

#### 5.4.1 插件载体

| 载体             | 适用场景                            | 权限与限制                                         | 平台                                      |
| ---------------- | ----------------------------------- | -------------------------------------------------- | ----------------------------------------- |
| DeclarativeGraph | 内置原子 DSP 的组合、预设和控制面板 | 数据包，不执行新机器码；节点必须来自 Host 能力目录 | ALL                                       |
| WasmComponent    | 可移植 DSP、分析、元数据和索引算法  | 默认无文件、网络、设备权限；只使用显式 Host 能力   | WIN/MAC/LNX/AND/HOS/WEB；IOS 仅随包且合规 |
| NativeArtifact   | 驱动、硬件输出、极致 SIMD、特殊格式 | 官方签名；桌面高级模式可人工授权社区原生插件       | 声明支持的平台                            |
| ServicePackage   | 可替换业务服务及配置                | 通过能力接口调用微内核，不得替换安全边界           | 声明支持的平台                            |

#### 5.4.2 生命周期需求

| ID      | 优先级 | 平台 | 状态     | 要求                                                                                | 验收                                                                                                                              |
| ------- | ------ | ---- | -------- | ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| PLG-001 | P0     | ALL  | 待验证   | 插件以内容寻址 PluginManifestV1 为入口，制品按平台和运行时分离                      | 同一 Manifest 可解析不同平台制品                                                                                                  |
| PLG-002 | P0     | ALL  | 待验证   | 插件必须声明 ID、版本、发布者、ABI、能力、权限、依赖、配置 Schema、许可证和制品 CID | 缺少必填字段或未知关键字段时拒绝安装                                                                                              |
| PLG-003 | P0     | ALL  | 待验证   | 所有安装包必须校验 CID、摘要、发布者签名和信任策略                                  | 无签名、错误签名、内容不匹配均拒绝                                                                                                |
| PLG-004 | P0     | ALL  | 待验证   | 下载、验证、暂存、激活采用事务；失败保留旧版本                                      | 任意阶段中断后可恢复或回滚                                                                                                        |
| PLG-005 | P0     | ALL  | 本机通过 | 支持浏览、搜索、安装、启用、停用、配置、升级、回滚和卸载                            | UI 与服务状态一致，重启后保持                                                                                                     |
| PLG-006 | P0     | ALL  | 部分实现 | 权限按能力授予，安装前展示，运行时可撤销                                            | 撤销后插件访问立即失败且返回 PermissionDenied                                                                                     |
| PLG-007 | P0     | ALL  | 本机通过 | 官方原生与社区沙箱形成双通道信任；桌面高级授权有持续警告和审计记录                  | 默认不能静默运行社区原生代码                                                                                                      |
| PLG-008 | P0     | ALL  | 待验证   | 插件只能替换已注册业务服务，不能覆盖微内核安全服务                                  | 恶意服务注册请求被拒绝                                                                                                            |
| PLG-009 | P0     | ALL  | 部分实现 | 普通兼容升级由用户确认；撤销或恶意版本必须立即停用                                  | 发布撤销事件后客户端在下一次刷新内执行                                                                                            |
| PLG-010 | P0     | ALL  | 待验证   | 插件连续崩溃或超时进入隔离并触发安全模式                                            | 重启后可禁用问题插件进入 Core 基础模式                                                                                            |
| PLG-011 | P0     | ALL  | 本机通过 | 插件配置依据 JSON Schema/等价声明渲染并持久化                                       | 类型、范围、默认值、敏感字段和迁移均通过                                                                                          |
| PLG-012 | P0     | ALL  | 待验证   | 不兼容平台、ABI、依赖或权限必须在下载前判定                                         | 不下载明显不兼容制品                                                                                                              |
| PLG-013 | P0     | ALL  | 部分实现 | 插件状态机必须可观测                                                                | 状态覆盖 available、downloading、verifying、staged、installed、enabled、disabled、update_available、revoked、failed、incompatible |
| PLG-014 | P0     | ALL  | 部分实现 | 声明式 ControlPanel 只能使用 Host 组件和参数绑定                                    | 插件不能注入任意 Flutter 路由或脚本                                                                                               |
| PLG-101 | P1     | 桌面 | 未实现   | 评估受控 Adapter Host 接入 VST3/AU/LV2                                              | 单独进程、权限、崩溃隔离和许可证评审通过后方可纳入                                                                                |

### 5.5 Bit-perfect 与硬件能力

| ID      | 优先级 | 平台     | 状态     | 要求                                                                         | 验收                                           |
| ------- | ------ | -------- | -------- | ---------------------------------------------------------------------------- | ---------------------------------------------- |
| BPT-001 | P0     | 支持平台 | 部分实现 | 用户显式开启 Bit-perfect，不使用不可解释的自动模式                           | UI 明确显示开启、失败或不可用                  |
| BPT-002 | P0     | 支持平台 | 部分实现 | 开启后锁定源格式、输出格式和独占输出，旁路音量、混音、重采样和修改样本的 DSP | Audio Path 列出每个旁路节点及原因              |
| BPT-003 | P0     | 支持平台 | 部分实现 | DSD Native/DoP 使用 DSD 或 Encoded 端口，不经过 f32 PCM 图                   | 类型检查拒绝 DSD 接入普通 Processor            |
| BPT-004 | P0     | 支持平台 | 部分实现 | 输出插件报告设备、驱动、共享/独占、协商格式、缓冲、时钟和能力来源            | 能力数据来自真实打开的会话而非静态平台字符串   |
| BPT-005 | P0     | ALL      | 待验证   | UI 仅声明“链路满足 Bit-perfect 条件”，不保证驱动后 DAC 行为                | 文案、API 和日志使用同一状态模型               |
| BPT-006 | P0     | ALL      | 待验证   | 不支持能力返回 unsupported 与原因，不得静默重采样后显示成功                  | Web/无硬件设备测试通过                         |
| BPT-007 | P0     | 支持平台 | 部分实现 | 记录实际采样率、位深、声道、时钟源、缓冲大小与掉音计数                       | 诊断包可复现本次会话路径                       |
| BPT-101 | P1     | WIN      | 未实现   | 提供 WASAPI Exclusive 和 ASIO 参考插件                                       | 真实设备通过 44.1/48/96/192 kHz 与设备拔插测试 |
| BPT-102 | P1     | MAC      | 未实现   | 提供 CoreAudio 独占/Hog 能力插件                                             | 真实设备完成格式切换和恢复                     |
| BPT-103 | P1     | LNX      | 部分实现 | 提供 ALSA hw 与 PipeWire 专业配置插件                                        | 绕过不期望的软件混音并报告实际路径             |
| BPT-104 | P1     | 支持平台 | 未实现   | 提供 DSD64/128 Native/DoP 与设备能力探测                                     | 支持设备实测；不支持设备明确拒绝               |
| BPT-105 | P1     | 支持平台 | 未实现   | 提供设备缓冲、采样率和时钟控制                                               | 越界值、运行中切换和设备丢失均有定义行为       |

### 5.6 IPFS 节点

| ID      | 优先级 | 平台            | 状态     | 要求                                                       | 验收                                     |
| ------- | ------ | --------------- | -------- | ---------------------------------------------------------- | ---------------------------------------- |
| NOD-001 | P0     | ALL             | 部分实现 | 应用内提供 NodeService，不要求用户另行安装 Kubo            | 干净设备安装后可查询节点状态并获取 CID   |
| NOD-002 | P0     | WEB             | 部分实现 | 使用 Helia/Verified Fetch 和浏览器支持的传输，逐块验证内容 | 禁用公共网关后通过测试中继完成获取       |
| NOD-003 | P0     | 原生平台        | 部分实现 | Rust Core 提供 UnixFS、内容路由、传输、Pin 和本地仓库能力  | 与 Kubo/Helia 测试节点互通               |
| NOD-004 | P0     | ALL             | 部分实现 | 所有读取按 CID 验证，不信任网关返回内容                    | 篡改块被拒绝且任务进入 integrity_failed  |
| NOD-005 | P0     | ALL             | 待验证   | 支持 Pin、Unpin、列举、持久化和副本健康度                  | 重启后 Pin 状态一致                      |
| NOD-006 | P0     | ALL             | 部分实现 | 支持存储、缓存、并发、上下行和计量网络策略                 | 超额时暂停并提示，不破坏持久化音乐       |
| NOD-007 | P0     | ALL             | 部分实现 | 暴露 Peer、传输、路由、Provider 和仓库诊断信息             | 用户可导出脱敏诊断包                     |
| NOD-008 | P0     | AND/IOS/HOS/WEB | 部分实现 | 前台完整参与，后台按平台能力尽力运行                       | UI 不承诺关闭应用后的持续提供            |
| NOD-009 | P0     | ALL             | 部分实现 | 网关和 Pin 服务仅作可配置补充，不成为唯一可信来源          | 网关响应仍按 CID 验证                    |
| NOD-010 | P0     | ALL             | 待验证   | 节点身份使用本地 Ed25519 密钥并安全存储                    | 重启身份稳定；导出/轮换行为符合 SEC 需求 |

### 5.7 音乐发布、下载与分发

| ID      | 优先级 | 平台 | 状态     | 要求                                                                                | 验收                                         |
| ------- | ------ | ---- | -------- | ----------------------------------------------------------------------------------- | -------------------------------------------- |
| DST-001 | P0     | ALL  | 待验证   | MusicManifestV1 描述作品、作者、专辑、曲目、rendition、封面、歌词、许可证和内容标签 | Canonical 编码与跨平台 CID 一致              |
| DST-002 | P0     | ALL  | 本机通过 | 保留发布者原文件，并允许附加 AAC/Opus 等兼容 rendition                              | 客户端按能力、质量和网络策略选择版本         |
| DST-003 | P0     | ALL  | 部分实现 | 发布流程包含编辑、校验、CID 生成、签名、Pin、Feed 事件和副本检查                    | 发布完成后另一节点可解析并播放               |
| DST-004 | P0     | ALL  | 待验证   | 发布者身份首次启动生成，可加密导出、导入、轮换和撤销                                | 密钥恢复与错误密码测试通过                   |
| DST-005 | P0     | ALL  | 待验证   | PublicationEvent 支持 publish、update、tombstone，具有序号、前序 CID 和签名         | 分叉、重放和回滚事件被检测                   |
| DST-006 | P0     | ALL  | 待验证   | 下载任务支持排队、暂停、恢复、取消、重试、优先级、进度和持久化                      | 杀进程后任务可恢复且不重复损坏数据           |
| DST-007 | P0     | ALL  | 部分实现 | 支持边下载边播放和有界预取                                                          | 网络抖动时不无限增长内存；缓存完整后转离线源 |
| DST-008 | P0     | ALL  | 待验证   | 完整下载后必须验证 CID，再原子移动至持久化目录                                      | 校验前文件不得进入正式媒体库                 |
| DST-009 | P0     | ALL  | 部分实现 | 发布端默认 Pin；收藏者可选择帮助 Pin；支持自建/第三方 Pin 服务                      | UI 显示本机 Pin 与已知 Provider 健康度       |
| DST-010 | P0     | ALL  | 部分实现 | 用户可配置仅 Wi-Fi、蜂窝限额、并发、缓存上限和自动复刻策略                          | 网络切换时行为符合配置                       |
| DST-011 | P0     | ALL  | 待验证   | Tombstone 只停止默认发现，不承诺删除已复制 CID                                      | UI 和文档明确不可变内容限制                  |
| DST-012 | P0     | ALL  | 待验证   | 公开发布必须填写许可证或权利声明                                                    | 缺失时不得发布到公共 Feed                    |

### 5.8 社区源、发现与过滤

#### 5.8.1 双 Feed 模型

每个社区源以 CommunitySourceManifestV1 为入口，并独立声明：

- **CatalogFeed**：社区收录的音乐、专辑、合集和发布者 Feed；
- **PolicyFeed**：对 CID、Manifest、发布者或 Feed 的提示、降权、隐藏、阻止和撤销决策。

CatalogFeed 与 PolicyFeed 使用相同的签名事件基础设施，但语义、开关和本地存储相互独立。

#### 5.8.2 需求

| ID      | 优先级 | 平台 | 状态   | 要求                                                                                | 验收                                         |
| ------- | ------ | ---- | ------ | ----------------------------------------------------------------------------------- | -------------------------------------------- |
| COM-001 | P0     | ALL  | 待验证 | 社区源 Manifest 声明维护者、密钥、Catalog 头、Policy 头、版本、语言和描述           | 独立开关 Catalog 与 Policy 后行为正确        |
| COM-002 | P0     | ALL  | 待验证 | Feed 事件使用序号、前序 CID、时间、签名和可选到期时间                               | 回滚、重放、错误签名和缺失前序被检测         |
| COM-003 | P0     | ALL  | 部分实现 | 客户端将已启用 CatalogFeed、直接关注发布者和本地曲库合并为搜索候选                  | 禁用全部 Catalog 后精确 CID 与直接关注仍可用 |
| COM-004 | P0     | ALL  | 待验证 | 同一 MusicManifest CID 去重；发布者签名元数据与社区注释分层存储                     | 社区不能覆盖发布者原始签名字段               |
| COM-005 | P0     | ALL  | 部分实现 | 本地索引支持标题、艺人、专辑、标签、发布者和社区来源查询                            | 索引删除重建后结果一致                       |
| COM-006 | P0     | ALL  | 本机通过 | Policy 动作为 warn、demote、hide、block、revoke，并声明目标、原因、证据、范围和到期 | 搜索、详情、精确打开三种入口均应用策略       |
| COM-007 | P0     | ALL  | 待验证 | 多 PolicyFeed 默认取最高严重度；用户可配置来源信任顺序；本地屏蔽优先                | 冲突矩阵测试通过并展示决策来源               |
| COM-008 | P0     | ALL  | 待验证 | 用户可分别订阅、暂停、刷新和删除社区源的发现与过滤部分                              | 删除后本地索引和策略引用正确清理             |
| COM-009 | P0     | ALL  | 部分实现 | 客户端预置一个可禁用的官方启动源，并支持 URI、二维码、CID/IPNS 添加其他来源         | 禁用预置源后应用仍可使用                     |
| COM-010 | P0     | ALL  | 待验证 | ModerationReport 包含目标、理由、证据 CID、时间和举报者签名，可加密提交给指定维护者 | 离线排队、隐私选项和重试通过                 |
| COM-011 | P0     | ALL  | 部分实现 | 每个过滤结果展示来源、动作、理由和有效期，并允许本地申诉或覆盖非强制策略            | 用户可解释为何内容不可见                     |
| COM-012 | P0     | ALL  | 部分实现 | Feed 支持增量同步、快照、压缩和大小上限                                             | 大型 Feed 不要求每次全量下载                 |
| COM-013 | P0     | ALL  | 待验证 | 社区维护者密钥支持轮换和撤销                                                        | 合法轮换连续；未知替换被拒绝                 |
| COM-101 | P1     | ALL  | 未实现 | 支持多个社区源的本地相关度排序和可解释推荐                                          | 推荐不依赖秘密远程画像                       |

### 5.9 Flutter/Web 界面

| ID     | 优先级 | 平台 | 状态     | 要求                                                                      | 验收                                 |
| ------ | ------ | ---- | -------- | ------------------------------------------------------------------------- | ------------------------------------ |
| UI-001 | P0     | ALL  | 部分实现 | 播放页显示真实状态、进度、音量、来源、缓存/网络状态和错误                 | 状态来自服务事件，不来自模拟计时器   |
| UI-002 | P0     | ALL  | 部分实现 | 曲库支持导入、扫描、搜索、排序、来源和可用性标记                          | 本地、IPFS 和社区条目可区分          |
| UI-003 | P0     | ALL  | 待验证   | 下载页展示任务状态、速度、Provider、校验、暂停/恢复和存储位置             | 所有 TransferTask 状态可操作         |
| UI-004 | P0     | ALL  | 本机通过 | 发布页完成元数据、权利声明、rendition、签名、Pin 和副本健康度流程         | 不完整发布不能进入公共 Feed；向导已落地（元数据/多 rendition 编辑/副本健康度），七端验收待补          |
| UI-005 | P0     | ALL  | 待验证   | 社区源页分别控制发现与过滤，展示维护者、签名和同步状态                    | 来源异常与策略冲突可解释             |
| UI-006 | P0     | ALL  | 待验证   | 插件页展示兼容性、信任通道、权限、版本、依赖、状态和回滚                  | 不兼容/撤销/失败状态不可误报为已安装 |
| UI-007 | P0     | ALL  | 待验证   | Audio Path 页面展示图节点、格式转换、延迟、旁路、输出会话和掉音统计       | 与 AudioGraphService 快照一致        |
| UI-008 | P0     | ALL  | 部分实现 | Bit-perfect 模式显示条件检查、失败原因和真实协商格式                      | 不支持平台明确显示 unsupported       |
| UI-009 | P0     | ALL  | 部分实现 | 设置页保存音乐目录、缓存/Pin 配额、网络策略、输出设备、节点和插件安全偏好 | 重启恢复且错误配置可回滚             |
| UI-010 | P0     | ALL  | 部分实现 | 所有关键操作具有进行中、成功、失败、取消和重试状态                        | 不使用无反馈按钮或吞掉异常           |
| UI-101 | P1     | ALL  | 部分实现 | 插件 ControlPanel 根据参数 Schema 渲染滑块、枚举、开关、仪表和预设        | 不允许任意代码注入 UI；滑块/枚举/开关/文本已落地，仪表与预设待补                |

---

## 6. Audio ABI v2

### 6.1 音频类型

#### AudioFormatV2

| 字段           | 类型          | 说明                                                   |
| -------------- | ------------- | ------------------------------------------------------ |
| media_kind     | enum          | pcm、dsd、encoded                                      |
| sample_kind    | enum          | f32、i16、i24_in_i32、i32、dsd_u8 或具体 encoded codec |
| sample_rate    | u32           | PCM 采样率；DSD 使用对应时钟率                         |
| channels       | u16           | 声道数                                                 |
| channel_layout | ChannelLayout | mono、stereo、5.1、7.1 或显式声道映射                  |
| packing        | enum          | planar、interleaved、dop                               |
| endian         | enum          | little、big、not_applicable                            |
| flags          | bitset        | bit_exact、silence、discontinuity 等                   |

未知枚举值不得回退为另一格式，必须返回 UnsupportedFormat。

#### AudioBufferViewV2

- 由 Host 创建并拥有；
- 包含一个或多个平面指针/句柄、容量、有效帧数、时间戳和格式；
- 生命周期仅覆盖一次 process 调用，插件不得保留指针；
- 输入默认只读，输出只允许写入 Host 指定范围；
- WASM 使用 Host 分配的线性内存窗口或等价安全句柄；
- 生产构建不得依赖插件自行分配每块缓冲。

### 6.2 节点描述

#### AudioNodeDescriptorV2

| 字段                     | 说明                                                                     |
| ------------------------ | ------------------------------------------------------------------------ |
| node_type                | decoder、processor、analyzer、resampler、transition、output、passthrough |
| interface_version        | 节点接口版本                                                             |
| input_ports/output_ports | 端口类型、数量和格式约束                                                 |
| supported_block_sizes    | 支持块大小或范围                                                         |
| latency_mode             | fixed、dynamic、unknown                                                  |
| latency_frames           | 当前固定延迟                                                             |
| tail_frames              | 停止输入后的尾音                                                         |
| realtime_safety          | verified、declared、non_realtime                                         |
| failure_policy           | bypass、rollback_graph、stop                                             |
| parameters               | ParameterDescriptorV2 列表                                               |
| capabilities             | 可查询能力键值                                                           |

#### ParameterDescriptorV2

必须声明：

- 稳定参数 ID；
- bool、integer、float、enum、string 或 meter 类型；
- 最小值、最大值、默认值和单位；
- linear、log、frequency 或自定义刻度；
- smoothing 策略；
- 是否支持实时变化和 sample-accurate automation；
- 是否影响延迟或需要重新编译图；
- 是否属于敏感配置。

### 6.3 处理上下文

ProcessContextV2 至少包含：

- 时间线帧位置；
- 本块帧数；
- playing、paused、seeking、draining 状态；
- discontinuity、end_of_stream 标记；
- 本块参数事件切片；
- Host 提供的实时安全服务句柄；
- deadline 和诊断计数器句柄。

实时错误必须为预定义整数码或位标记，不得在 process 中创建动态字符串。

### 6.4 生命周期

| 阶段            | 线程           | 可执行操作                           |
| --------------- | -------------- | ------------------------------------ |
| create          | 非实时         | 创建实例、校验配置                   |
| prepare         | 非实时         | 分配内存、加载 IR/表格、声明实际延迟 |
| activate        | 非实时边界     | 清零状态并加入已编译图               |
| process         | 实时           | 处理 Host 缓冲和本块参数事件         |
| reset           | 非实时或安全点 | Seek/时间线跳变后清理状态            |
| flush           | 非实时或安全点 | 丢弃缓存数据                         |
| drain           | 实时受限       | 消耗尾音，不再接受新输入             |
| deactivate      | 非实时边界     | 从图移除                             |
| serialize_state | 非实时         | 保存配置与预设                       |
| migrate_state   | 非实时         | 从旧 Schema 升级                     |
| destroy         | 非实时         | 释放实例资源                         |

### 6.5 图规格

GraphSpecV1 包含：

- 图 ID、版本和创建来源；
- 节点实例、插件版本和状态 CID；
- 带类型的端口连接；
- 输出目标和故障策略；
- 期望模式：normal、low_latency、bit_perfect；
- 用户允许的自动格式转换；
- 图级 CPU、内存和延迟预算。

提交步骤：

1. 解析并验证 Schema；
2. 解析插件与能力；
3. 校验 DAG、端口和权限；
4. 协商格式与块大小；
5. 插入允许的转换节点；
6. 计算延迟和缓冲；
7. 调用所有节点 prepare；
8. 生成不可变执行计划；
9. 在音频块边界原子切换；
10. 失败时释放候选图并保持旧图。

### 6.6 兼容策略

- Output ABI v1 保留读取能力，通过 LegacyOutputAdapter 接入；
- v1 的 i16 interleaved 输出不能声明 planar f32、DSD、动态延迟或 Bit-perfect 会话证明；
- ABI 按接口分别版本化，不能仅以单一全局 ABI 判断全部兼容；
- 新增可选字段必须可忽略；改变含义或内存布局必须提升主版本；
- PluginManifest 同时声明最低/最高微内核版本和接口版本范围。

---

## 7. 网络对象与数据契约

### 7.1 Canonical 编码与签名

- Manifest、Feed 事件和签名信封使用规范化 DAG-CBOR；
- 大型音频、封面、歌词和插件制品使用 UnixFS 或 CAR 承载；
- 默认使用 CIDv1 与 SHA-256；
- 签名算法为 Ed25519；
- 签名覆盖对象类型、Schema 版本、主体字段和防域混淆前缀；
- signature 字段本身不进入被签名字节；
- 相同输入必须在所有平台产生相同 CID；
- 网络对象大小、嵌套深度、字符串长度和引用数量必须设上限。

### 7.2 音乐模型

#### PublisherIdentityV1

- publisher_id：公钥派生稳定 ID；
- public_key；
- display_name；
- created_at；
- previous_key 与 rotation_proof；
- revoked_at 与 revocation_proof；
- 可选主页、社区证明和头像 CID。

#### MusicManifestV1

- schema_version；
- work_id 与 release_id；
- title、artists、album、track/disc number；
- duration、language、genres、tags；
- cover CID、lyrics CID、credits；
- LicenseDeclaration；
- content_labels；
- renditions；
- publisher identity CID；
- created_at、updated_at；
- publisher signature。

#### MusicRenditionV1

- rendition_id；
- content CID；
- container、codec、profile；
- sample_rate、bit_depth、channels、channel_layout；
- duration、byte_length；
- lossless、original、streamable 标志；
- 分块/UnixFS 参数；
- 可选 ReplayGain 与编码器 delay/padding 元数据。

#### PublicationEventV1

- event_type：publish、update、tombstone；
- publisher_id；
- sequence；
- previous_event CID；
- manifest CID 或目标 CID；
- timestamp；
- reason；
- signature。

### 7.3 社区源模型

#### CommunitySourceManifestV1

- source_id、name、description、languages；
- maintainer identity；
- catalog_head 的 IPNS/CID；
- policy_head 的 IPNS/CID；
- 支持的 Schema；
- 提交/举报端点或 Feed；
- 密钥轮换信息；
- 更新时间与签名。

#### CatalogEventV1

- action：include、update、remove；
- target 类型与 CID；
- 分类、标签、社区注释；
- sequence、previous_event CID；
- optional expiration；
- maintainer signature。

remove 仅表示从该社区的发现索引移除，不等于有害或被封禁。

#### PolicyEventV1

- action：warn、demote、hide、block、revoke；
- target 类型与 CID/发布者 ID；
- reason_code；
- 人类可读说明；
- evidence CIDs；
- scope 与地区/年龄标签；
- issued_at、expires_at；
- sequence、previous_event CID；
- maintainer signature。

#### ModerationReportV1

- report_id；
- target；
- reason_code 与说明；
- evidence CIDs；
- reporter public identity 或匿名提交标记；
- recipient source ID；
- created_at；
- signature；
- 可选端到端加密信封。

### 7.4 插件模型

#### PluginManifestV1

- plugin_id、name、version、publisher；
- plugin_kind；
- interface_versions；
- minimum/maximum core version；
- artifacts；
- capabilities 与 permissions；
- dependencies 与 conflicts；
- configuration_schema CID；
- state_schema_version；
- license；
- release_notes CID；
- previous_release CID；
- signature 与撤销信息。

#### PluginArtifactV1

- artifact CID；
- platform、architecture；
- runtime：declarative、wasm、native；
- entrypoint；
- byte_length；
- build provenance/SBOM CID；
- sandbox profile；
- required host capabilities；
- optional hardware requirements。

#### PluginPermission

至少覆盖：

- music_library_read；
- music_library_write；
- ipfs_fetch；
- ipfs_publish；
- network_domains；
- isolated_storage；
- audio_realtime；
- audio_device；
- hardware_exclusive；
- user_interface_schema；
- diagnostics。

权限必须最小化，未声明即拒绝。

### 7.5 传输模型

#### TransferTaskV1

- task_id；
- kind：fetch、download、publish、pin、plugin；
- target CID；
- state：queued、resolving、transferring、paused、verifying、committing、completed、failed、cancelled；
- bytes_total、bytes_completed、speed；
- Provider 摘要；
- retry_count、next_retry_at；
- network_policy；
- destination；
- error；
- timestamps。

#### NodeStatusV1

- identity/Peer ID；
- lifecycle state；
- transports 与监听/拨出能力；
- connected peers；
- routing status；
- repository usage；
- cache/pin usage；
- bandwidth；
- browser/background limitations；
- last error。

#### ProviderHealthV1

- CID；
- observed providers；
- last_success_at；
- latency；
- local_pin；
- configured_pin_services；
- health：healthy、degraded、unknown、unavailable。

---

## 8. 服务接口

服务接口是传输无关契约。原生端通过 FFI/IPC，Web 通过 JS/WASM 映射；DTO 和错误语义必须一致。

| ID      | 优先级 | 平台 | 状态     | 要求                                                                  | 验收                                         |
| ------- | ------ | ---- | -------- | --------------------------------------------------------------------- | -------------------------------------------- |
| API-001 | P0     | ALL  | 部分实现 | 所有上层能力先定义传输无关服务契约，再映射到 FFI/IPC、HTTP 或 JS/WASM | 同一契约测试可对不同传输适配器运行           |
| API-002 | P0     | ALL  | 待验证   | 公共 DTO、事件、错误和网络对象必须显式版本化                          | 未知主版本拒绝；兼容次版本可忽略未知可选字段 |
| API-003 | P0     | ALL  | 待验证   | 写操作具有 request_id 或幂等键，重试不得重复产生副作用                | 安装、发布、Pin 和删除的重复请求测试通过     |
| API-004 | P0     | ALL  | 部分实现 | 错误使用稳定机器码并携带子系统、操作、可重试性和不支持原因            | 七端 UI 对相同错误呈现一致语义               |
| API-005 | P0     | ALL  | 待验证   | 长任务和播放状态通过可排序事件与快照组合提供                          | 丢失事件后消费者能检测 sequence 缺口并恢复   |
| API-006 | P0     | ALL  | 待验证   | 控制面默认只允许本地受认证访问，远程访问必须显式开启                  | 未授权和跨源请求安全测试通过                 |
| API-007 | P0     | ALL  | 部分实现 | API、ABI、Schema 和数据库迁移具有兼容范围与回滚说明                   | 发布前兼容性矩阵和迁移测试齐全               |

### 8.1 逻辑服务

| 服务                   | 主要能力                                                                                    |
| ---------------------- | ------------------------------------------------------------------------------------------- |
| PlaybackService        | load、play、pause、stop、seek、queue、volume、state                                         |
| AudioGraphService      | validate、compile、commit、rollback、parameters、stats、bit-perfect status                  |
| LibraryService         | scan、import、query、track availability、playlist、missing file repair                      |
| NodeService            | start、stop、status、peers、resolve、cat stream、add、pin、providers                        |
| TransferService        | create、pause、resume、cancel、retry、list、subscribe                                       |
| PublicationService     | draft、validate、publish、update、tombstone、replica health                                 |
| CommunitySourceService | add、remove、enable catalog、enable policy、refresh、inspect decision                       |
| IndexService           | ingest feed、rebuild、search、deduplicate、explain source                                   |
| PluginLifecycleService | discover、install、verify、enable、disable、configure、upgrade、rollback、uninstall、revoke |

### 8.2 HTTP 控制面

HTTP 控制面统一使用 /v1，默认仅绑定回环地址并受 Bearer token 或等价本地认证保护。

建议资源：

- GET /v1/health
- GET /v1/node/status
- GET /v1/node/peers
- PUT /v1/node/config
- POST /v1/transfers
- GET /v1/transfers
- GET /v1/transfers/{id}
- POST /v1/transfers/{id}/pause
- POST /v1/transfers/{id}/resume
- POST /v1/transfers/{id}/cancel
- POST /v1/pins/{cid}
- DELETE /v1/pins/{cid}
- POST /v1/publications
- POST /v1/publications/{id}/tombstone
- GET /v1/community-sources
- POST /v1/community-sources
- POST /v1/community-sources/import
- PATCH /v1/community-sources/{id}
- POST /v1/community-sources/{id}/refresh
- POST /v1/community-sources/{id}/maintainer-key-events
- GET/POST /v1/moderation-reports
- POST /v1/moderation-reports/{id}/retry
- GET /v1/search
- GET /v1/plugins
- POST /v1/plugins/install
- POST /v1/plugins/{id}/enable
- POST /v1/plugins/{id}/disable
- POST /v1/plugins/{id}/upgrade
- POST /v1/plugins/{id}/rollback
- DELETE /v1/plugins/{id}
- GET /v1/plugins/{id}/config
- PUT /v1/plugins/{id}/config
- GET /v1/audio/graph
- PUT /v1/audio/graph
- GET /v1/audio/path
- GET /v1/audio/stats

写操作必须支持 request_id 或 Idempotency-Key。重复请求不得重复安装、发布或删除。

### 8.3 错误信封

统一错误至少包含：

- code：稳定机器错误码；
- message：本地化前的开发者信息；
- subsystem；
- operation；
- retryable；
- unsupported_reason；
- details；
- request_id；
- cause chain 的脱敏摘要。

网络、插件和实时线程不得向 UI 直接暴露密钥、完整本地路径、Bearer token 或未脱敏 Peer 地址。

### 8.4 事件

所有长任务和播放状态通过版本化事件发布：

- playback.state_changed；
- playback.position；
- playback.track_changed；
- transfer.progress；
- transfer.state_changed；
- node.status_changed；
- community_source.updated；
- policy.decision_changed；
- plugin.state_changed；
- plugin.revoked；
- audio.graph_changed；
- audio.xrun；
- audio.device_changed。

事件必须包含 sequence、timestamp、schema_version 和关联实体 ID。消费者检测到 sequence 缺口后必须重新读取快照。

---

## 9. 存储与持久化

### 9.1 数据分类

| 分类         | 内容                                 | 清理策略                     |
| ------------ | ------------------------------------ | ---------------------------- |
| 用户持久数据 | 曲库、歌单、插件配置、身份、发布草稿 | 不随缓存清理删除             |
| 音乐持久数据 | 用户导入、明确下载、明确 Pin 的音乐  | 仅用户确认后删除             |
| IPFS 缓存    | 临时块、流式预取、未 Pin 内容        | 按配额和 LRU 清理            |
| 插件仓库     | 已安装、上一可回滚版本、暂存包       | 事务清理，保留活动和回滚版本 |
| 搜索索引     | CatalogFeed、本地元数据索引          | 可重建                       |
| 审计与诊断   | 安装、权限、撤销、错误和性能摘要     | 按期限滚动并脱敏             |

### 9.2 音乐目录

- Windows：用户 Music/JimMusic 或用户选择目录；
- macOS/Linux：系统音乐目录下 JimMusic 或用户选择目录；
- Android/iOS/HarmonyOS：应用沙盒/系统允许的媒体目录；
- Web：浏览器持久存储能力允许的 OPFS/IndexedDB 等实现；
- 目录不存在时创建；不可写时不静默回退，必须提示并保持旧配置；
- 更换目录提供“仅切换”“复制”“移动”选项，并支持中断恢复；
- 缓存和持久音乐必须物理或逻辑隔离。

### 9.3 状态存储

- 结构化状态使用事务数据库或提供等价原子性；
- 插件记录、活动版本、权限、撤销和回滚点必须持久化；
- Feed 头、已处理 sequence 和签名验证结果必须持久化；
- 数据库迁移失败时只读启动并提供恢复，不得清空用户数据。

---

## 10. 安全、隐私与内容治理

| ID      | 优先级 | 平台 | 状态     | 要求                                                 | 验收                                     |
| ------- | ------ | ---- | -------- | ---------------------------------------------------- | ---------------------------------------- |
| SEC-001 | P0     | ALL  | 部分实现 | Ed25519 私钥使用系统安全存储或加密封装               | 明文搜索不得找到私钥                     |
| SEC-002 | P0     | ALL  | 待验证   | 身份导出使用用户口令和现代 KDF/AEAD                  | 错误口令、篡改和版本迁移测试通过         |
| SEC-003 | P0     | ALL  | 待验证   | 插件签名必须校验；无签名不再作为正常安装路径         | 开发模式例外有明显标志且不能进入发布构建 |
| SEC-004 | P0     | ALL  | 部分实现 | WASM 默认无环境权限，Host 能力按句柄授予             | 插件无法绕过权限直接访问网络/文件        |
| SEC-005 | P0     | 桌面 | 部分实现 | 社区原生插件高级模式需要二次确认、审计和安全模式恢复 | 恶意插件测试不破坏主配置和回滚点         |
| SEC-006 | P0     | ALL  | 待验证   | 控制面默认回环绑定并要求认证；跨设备访问默认关闭     | 未授权请求返回 401/403                   |
| SEC-007 | P0     | ALL  | 待验证   | IPFS/网关内容均按 CID 验证，Manifest 与 Feed 均验签  | 篡改、替换、重放和回滚攻击被拒绝         |
| SEC-008 | P0     | ALL  | 待验证   | 公开发布要求权利声明、内容标签和发布者签名           | 缺项发布失败                             |
| SEC-009 | P0     | ALL  | 本机通过 | 支持本地屏蔽、社区策略、举报和来源解释               | 被过滤内容可查询决策来源                 |
| SEC-010 | P0     | ALL  | 部分实现 | 诊断、举报和社区订阅最小化隐私泄露                   | 导出包不包含密钥、token 和未授权文件路径 |
| SEC-011 | P0     | ALL  | 部分实现 | 撤销信息具有签名、有效期和防回滚机制                 | 旧安全快照不能重新启用已撤销版本         |
| SEC-012 | P0     | ALL  | 部分实现 | 网络对象解析具有大小、深度、数量和解压上限           | 模糊测试不导致 OOM 或栈溢出              |

内容治理边界：

- PolicyFeed 影响客户端发现和展示，不改变 IPFS 内容本身；
- Tombstone 表达发布者撤回意愿，不能保证删除副本；
- 官方启动源可禁用，JimMusic 协议允许第三方社区源；
- 客户端不得把“未被某 Catalog 收录”显示为“违规”；
- 地区法律或应用商店要求的不可覆盖策略必须在具体发行渠道文档中单独声明。

---

## 11. 非功能需求

| ID      | 优先级 | 平台 | 状态     | 指标                                                              |
| ------- | ------ | ---- | -------- | ----------------------------------------------------------------- |
| NFR-001 | P0     | ALL  | 待验证   | UI 本地操作响应 P95 不高于 100 ms                                 |
| NFR-002 | P0     | ALL  | 待验证   | 已索引本地文件起播 P95 不高于 500 ms                              |
| NFR-003 | P0     | ALL  | 待验证   | 本地可 Seek 格式 Seek P95 不高于 250 ms                           |
| NFR-004 | P0     | ALL  | 部分实现 | 实时 process 在所选块 deadline 内完成，发布构建持续记录超时       |
| NFR-005 | P0     | ALL  | 待验证   | 两小时播放过程中 PCM 缓冲有界，无随时长线性增长                   |
| NFR-006 | P0     | ALL  | 待验证   | 完整内容 CID 校验成功率 100%；校验失败内容不得提交                |
| NFR-007 | P0     | ALL  | 待验证   | 插件安装/升级/图切换具有原子性，故障后可回滚                      |
| NFR-008 | P0     | ALL  | 待验证   | M0 建立各平台资源基线；后续启动、内存、耗电和带宽回退不得超过 15% |
| NFR-009 | P0     | ALL  | 部分实现 | 稳定版崩溃自由会话目标不低于 99.5%                                |
| NFR-010 | P0     | ALL  | 待验证   | 可信微内核、协议解析和插件生命周期代码行覆盖率不低于 80%          |
| NFR-011 | P0     | ALL  | 部分实现 | 核心用户流程支持键盘/读屏/缩放和足够对比度                        |
| NFR-012 | P0     | ALL  | 部分实现 | 日志具有关联 ID、级别、子系统和脱敏策略                           |
| NFR-013 | P0     | ALL  | 待验证   | 离线状态下本地播放、曲库、歌单和已安装插件可用                    |
| NFR-014 | P0     | ALL  | 部分实现 | Schema、ABI 和数据库迁移均有向前/回滚策略                         |

旧版“所有平台启动小于 1 秒、常驻内存小于 50 MB”的统一指标在内置节点和 Flutter 场景下缺少基准依据，2.0 改为先建立七端基线，再以明确的 P95 和回退比例作为发布门槛。

---

## 12. 测试与验收

| ID      | 优先级 | 平台     | 状态   | 要求                                                            | 验收                                   |
| ------- | ------ | -------- | ------ | --------------------------------------------------------------- | -------------------------------------- |
| REL-001 | P0     | ALL      | 部分实现 | 建立需求 ID、测试、平台、构建产物和证据的追踪矩阵               | 每个 P0 均能定位到最新结果             |
| REL-002 | P0     | ALL      | 待验证 | 七个平台必须在同一候选版本满足 P0，不采用长期功能降级发布       | Release Candidate 报告无平台级 P0 豁免 |
| REL-003 | P0     | ALL      | 待验证 | 发布产物必须从受控 CI 生成并附版本、依赖、摘要和 SBOM           | 下载产物可验证并追溯到提交             |
| REL-004 | P0     | 支持平台 | 待验证 | 硬件发烧能力必须具有对应设备和驱动的实机证据                    | 无实机证据的能力标记为待验证或不支持   |
| REL-005 | P0     | ALL      | 部分实现 | 发布前同步需求状态、用户文档、插件 SDK、隐私和已知限制          | 文档检查成为发布阻断项                 |
| REL-006 | P0     | ALL      | 部分实现 | 发布候选版本通过升级、降级、数据迁移、断电/杀进程和安全模式恢复 | 恢复测试不丢失用户持久数据             |

### 12.1 基础播放器矩阵

每个平台至少执行：

1. 全新安装并在无外部 Kubo 的环境启动；
2. 导入 MP3、AAC/M4A、FLAC、WAV、OGG/Opus；
3. 真实播放、暂停、Seek、音量、静音、上一首和下一首；
4. 创建歌单、重启、恢复；
5. 文件删除、损坏和权限撤销；
6. 网络断开时播放本地/缓存音乐；
7. 从 MusicManifest 选择兼容 rendition；
8. 设备拔出或浏览器音频上下文暂停后的恢复。

### 12.2 Audio ABI 与 DSP

- 脉冲响应、频率响应和固定输入 Golden Test；
- 跨平台相同预设的数值误差必须低于插件声明容差；
- process 零分配、无阻塞锁检测；
- 64/128/256/512/1024 等声明块大小测试；
- 图非法环、类型不匹配、缺失节点和预算超限；
- 图热切换、参数 automation、Seek reset 和 tail drain；
- 节点超时、崩溃、NaN/Inf 输出和越界写入；
- 并行路径延迟补偿；
- 两小时播放内存和 xrun 统计；
- Legacy Output ABI v1 适配回归。

### 12.3 硬件音频

硬件实验室按支持平台验证：

- 44.1、48、88.2、96、176.4、192 kHz；
- 16、24、32 位 PCM；
- 共享与独占切换；
- ASIO/WASAPI Exclusive/CoreAudio/ALSA hw；
- DSD64/DSD128 Native 或 DoP；
- 设备缓冲调整；
- 默认设备变化、拔插、休眠和恢复；
- Bit-perfect 状态与真实会话参数一致。

无硬件 CI 可以使用虚拟设备验证协议，但不得替代实机发布证据。

### 12.4 IPFS 与分发

至少建立 Kubo、原生 JimMusic 节点和 Web Helia 的互操作测试网，覆盖：

- 跨实现 add/cat/Pin；
- Provider 上下线和路由波动；
- 网关关闭与中继路径；
- 错误块、错误 CID 和篡改 Manifest；
- 边下边播、暂停恢复和进程重启；
- 缓存配额、Pin 保护和清理；
- 发布、更新、Tombstone；
- 发布者密钥轮换和撤销；
- 自愿复刻和副本健康度。

### 12.5 社区源

- Catalog 与 Policy 独立启停；
- 多 Catalog 合并与 CID 去重；
- 多 Policy 冲突、到期和信任顺序；
- 未收录与被封禁的语义区分；
- Feed 分叉、回滚、重放、错误签名；
- 大 Feed 快照与增量恢复；
- 本地索引删除重建；
- 举报离线排队、加密和重试；
- 禁用全部社区源后精确 CID/发布者订阅仍可用。

### 12.6 插件安全与生命周期

- 无签名、错误签名、签名后篡改；
- 不兼容 OS、架构、ABI 和依赖；
- 下载、验证、暂存、提交各阶段中断；
- 权限拒绝和运行时撤销；
- 普通升级确认、失败回滚和安全撤销；
- 插件连续崩溃后的隔离与安全模式；
- 恶意原生/WASM 插件的越权与资源耗尽；
- 状态 Schema 迁移成功、失败和降级；
- iOS/Web 不得下载错误原生制品。

### 12.7 Definition of Done

需求只有同时满足以下条件才可标记“已实现”：

- 对应代码和用户入口存在；
- 单元、集成或端到端测试通过；
- 声明平台全部有证据；
- 错误、取消、不支持和恢复路径通过；
- 文档、Schema、API 和迁移说明同步；
- 没有模拟播放、静态能力列表或外部手工服务依赖；
- 安全审计与性能门槛通过；
- 需求追踪矩阵已更新。

---

## 13. 当前实现基线与差距

以下结论以 2026-08-15 工作区为基线。

| 子系统         | 当前状态                                                                 | 与 2.0 的主要差距                                                  |
| -------------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------ |
| Flutter 播放器 | 已移除演示曲目/模拟进度；本地曲库、歌单和真实 Bridge/just_audio 状态可用；曲库/收藏/歌单/会话与控制面双向同步（本地优先），来源可区分 | 缺七端实机闭环与多设备冲突策略       |
| 播放引擎       | 增量解码、有界 PCM、类型化 DAG、格式转换、延迟补偿、原子切换和统计；双时间线 gapless/crossfade | DSP 节点崩溃/超时注入链路、动态延迟与节点状态迁移未完整执行；七端音频语料待验收 |
| 音频输出       | null、CPAL system output 已接桌面 FFI；Web Audio 有 ABI/Worklet 参考实现 | Web Audio 未接 Flutter PCM；无独占、ASIO、CoreAudio Hog、DSD 证据  |
| 插件 ABI       | Audio ABI v2、Manifest、权限/依赖/配置/制品 DTO、legacy 输出适配与 Wasmtime 无 WASI/capability 沙箱 | Web/iOS/HarmonyOS 插件执行载体与完整声明式 ControlPanel 渲染未闭环 |
| 插件管理器     | 强制签名/CID/摘要、预检、事务安装、持久状态、启停、回滚、隔离、安全模式；社区目录浏览/搜索/详情/安装；社区撤销策略刷新后自动停用被撤销发布  | 运行时权限强制和独立进程隔离不完整            |
| 节点/IPFS      | rust-ipfs UnixFS/Bitswap/Kademlia/mDNS/TCP/WS/QUIC、Pin/配额/稳定 PeerId 与 Web Helia 直连互操作；网络类别策略暂停/恢复传输 | 真实浏览器中继、Kubo 额外互操作与移动端网络闭环待外证；上传限速显式不支持（PROD-004）、自动复刻未实现 |
| 音乐分发       | Manifest/rendition、签名 Feed、原子下载、持久优先级调度与任务恢复；收藏协助 Pin 与第三方 Pin 服务 | 缺另一节点从 Feed 解析到播放的 UI 闭环；发布向导已落地（元数据/多 rendition 编辑/副本健康度），边下边播已落地（传输 part 流端点 + 播放器接入），七端实测待补 |
| 社区发现       | 双 Feed、可禁用签名启动源、URI/CID/IPNS 导入、换钥/撤销与加密举报、直接关注发布者（关注后作品进媒体库并保留）        | 启动源尚无远端 Feed；缺相机扫码；Feed 快照/压缩已落地（紧凑快照 + gzip + 摘要 + 上限），远端实测待补                   |
| 音乐目录       | 后端持久曲库/扫描/缺失标记/歌单/会话和 Flutter 曲库经统一同步双向合并（本地优先，来源可区分） | 无目录监控、迁移和 Web 持久目录闭环                  |
| CI/发布        | 七端同提交 gate、签名、摘要、SBOM、provenance 和逐 P0 追踪已配置         | 尚无受控 tag 产物、Harmony runner、硬件实验室和七端 RC 实际证据    |

逐项证据与明确阻断项见 `docs/REQUIREMENTS_TRACEABILITY.md` 和
`docs/RELEASE_ACCEPTANCE.md`。PROJECT_SUMMARY.md 与 README.md 只能用于项目说明，不能单独作为完成证据。

---

## 14. 实施里程碑

### M0：契约与基线

- 冻结网络对象 Schema、服务 DTO、错误码、Audio ABI v2 初版；
- 建立七端构建矩阵、性能基线和需求追踪表；
- 建立 Kubo/Helia/JimMusic 节点测试网；
- 建立音频 Golden Test 和虚拟输出测试框架。

### M1：七端基础播放器闭环

- 去除演示/模拟完成路径；
- 统一媒体库、队列、持久化和错误模型；
- 七端真实播放与格式语料通过；
- 建立可用但不含高级 DSP 的基础 Audio Graph。

### M2：实时音频图与插件微内核

- 完成 Host 缓冲池、类型化 DAG、格式协商和原子图切换；
- 完成三级插件载体、权限、事务、回滚和安全模式；
- 完成 Legacy ABI 适配；
- 完成 Audio Path 和插件管理 UI。

### M3：IPFS 发布与社区源

- 七端应用内节点；
- MusicManifest、发布者身份、TransferTask、Pin 和副本健康度；
- Catalog/Policy 双 Feed、本地索引和举报；
- 发布、下载、边播和撤回闭环。

### M4：硬件发烧验证

- WASAPI Exclusive、ASIO、CoreAudio、ALSA hw/PipeWire；
- DSD Native/DoP 与设备能力；
- Bit-perfect 显式模式和实机会话证明；
- 硬件实验室报告。

### M5：2.0 Release Candidate

- 所有 P0 在七端通过；
- 安全、性能、迁移和恢复门槛通过；
- README、项目总结、SDK 与发布文档同步；
- 同日生成并验证七端发布产物。

---

## 15. 风险与明确假设

1. IPFS 内容不可变；JimMusic 只能撤回索引和发布 Tombstone，不能删除其他节点的副本。
2. 浏览器和移动系统限制后台网络活动；“内置节点”表示能力内置，不承诺应用关闭后持续提供。
3. iOS 对下载执行代码有限制；插件目录必须按平台过滤，声明式制品与随包模块是主要路径。
4. 七端要求基础用户结果一致，不要求每个硬件插件支持全部平台。
5. Bit-perfect 只证明 JimMusic 到驱动会话的可观察链路条件，不证明 DAC 内部实现。
6. 原生 IPFS 实现和移动端资源预算须在 M0 通过技术验证后锁定，但不得退回“要求用户安装外部 Kubo”。
7. HarmonyOS Flutter 适配不是 Flutter 官方主线平台，必须锁定工具链并维持独立构建证据。
8. 社区源是联邦化发现入口，不是唯一目录；官方启动源可被禁用。
9. PolicyFeed 是客户端策略，不是网络级删除或审判机制。
10. 2.0 不包含付费、DRM 和现有专业音频插件格式的完整兼容。
