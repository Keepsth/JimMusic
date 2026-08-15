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

- Rust format、Clippy（all targets/all features，`-D warnings`）和 225 项完整测试通过；FFI 门禁先
  构建真实 `cdylib`，再验证 Output/Host/Node 符号与行为，避免旧增量产物或静默跳过；
  节点 stop 等待 rust-ipfs 仓库锁释放，同进程重启门禁稳定通过；
- Flutter analyze 零问题，37 项测试通过（含 Rust 音频/节点 FFI、控制面 SSE 流与
  边下边播代理链路）；
- 边下边播（DST-007）闭环：`/v1/transfers/{id}/stream` 跟随 part 文件增长流式输出并支持
  Range；Flutter 经 just_audio 代理注入 Bearer 鉴权播放、Seek 只取已下载前缀，传输页提供
  边下边播入口；整块路径（本地 CAS/P2P）落地后写入 part 供播放交接，孤儿文件启动/流端点清理；
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
