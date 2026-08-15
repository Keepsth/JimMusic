# JimMusic

JimMusic 2.0 是一个以 Rust 可信核心、版本化内容对象和 Flutter 七端 UI 为目标的去中心化音乐
播放器。当前仓库已经形成可运行的本地播放器、实时 Audio Graph、原生/Web P2P 节点、签名
发布/社区 Feed、持久传输和受限插件运行时；它是 Release Candidate 源码基线，在七端物理证据
到齐前不是满足全部 P0 的稳定版。缺口与证据见
[需求追踪矩阵](docs/REQUIREMENTS_TRACEABILITY.md)和[发布验收记录](docs/RELEASE_ACCEPTANCE.md)。

## 已落地的核心能力

- Rust 增量音频解码、有界 PCM 队列、真实播放状态、队列自动续播、双时间线
  gapless/crossfade 和结构化失败；
- Audio ABI v2、类型化 DAG、格式协商/转换、延迟补偿、sample-position 参数、原子切图和统计；
- 本地 verified CAS 与 rust-ipfs：UnixFS、Bitswap、Kademlia、mDNS、TCP/WebSocket/QUIC、Pin、
  配额/LRU、稳定 Ed25519 PeerId 和脱敏诊断；
- Web Helia + IndexedDB + Bitswap 节点，默认 P2P 路径不配置公共 HTTP 网关；
- 版本化 DAG-CBOR 网络对象、Music Manifest/rendition、加密发布者身份、签名 Feed 与 Tombstone；
- 持久传输任务：优先级调度、暂停/恢复/取消/重试、流式下载、限速、CID 校验和原子提交；
- CommunitySource 的 Catalog/Policy 双 Feed、签名/序列/前序检查、增量刷新和策略合并；
- 社区策略在搜索/详情/精确打开三入口统一应用：隐藏与阻止直接生效，警告需确认，非强制决策可本地覆盖；
- 插件签名/CID/摘要预检、权限/依赖/冲突、事务安装、启停、配置、回滚、隔离和安全模式；
- 社区插件目录：浏览/搜索 PluginManifest 条目，详情展示兼容性/已装状态/升级可用/撤销，可一键进入安装确认；
- Wasmtime 沙箱默认无 WASI/环境权限，使用 fuel、内存/表上限及 owner-scoped capability handle；
- `/v1` 受认证控制面、全 mutation 持久幂等、稳定错误信封、SSE sequence 与快照恢复；
- 本地可靠性账本区分正常与未清理退出，只输出聚合率，不上传媒体或身份数据；
- Flutter 播放器与控制中心：节点、Pin、传输、身份/发布、社区、插件和 Audio Path。

## 明确不支持或尚未验收

- 原生 rust-ipfs 与 Web Helia 已通过 600 KB UnixFS/Bitswap 直连互操作；真实浏览器中继、Kubo、
  Android/iOS/HarmonyOS 产物及移动端全部网络对象 UI 闭环仍需外证。
- Wasmtime 的无环境权限、句柄隔离、撤销和资源耗尽负向测试已通过；Web/iOS/HarmonyOS 的插件
  执行载体及社区原生插件独立进程隔离尚未闭环。
- Bit-perfect UI、类型化 DSD/Encoded 图与打开输出会话证据已实现。WASAPI Exclusive、
  CoreAudio Hog、ALSA hw、ASIO 与 DSD Native/DoP 仍只能在有对应设备和驱动的实验室验收。
- 原生 Core gapless/crossfade 已实现；Web just_audio 回退、ReplayGain、移动后台音频/锁屏控制
  尚未完成。节点后台参与仅为平台允许范围内的 best effort，UI 明确不承诺关闭应用后继续提供。
- 内置启动源已签名且可永久禁用/移除，但当前没有发布远端 Catalog/Policy 头，二维码相机扫描
  也尚未接入。
- 七端 release workflow 现在强制物理设备报告、134 项 P0、资源回退和硬件证据；配置存在不等于
  七端产物已经签名、安装和验收。

## 目录

```text
backend/
  protocol/                 版本化 DTO、严格 DAG-CBOR、CID 与校验规则
  plugin-abi/               Legacy C ABI、Output ABI、Audio ABI v2
  app-core/                 播放/图/节点/曲库/传输/身份/发布/社区服务
  app-core-static/          iOS 静态链接封装（只由对应发布目标使用）
  plugin-manager/           /v1 控制面、生命周期、幂等、节点身份、传输 runner
  plugins/                  Symphonia、FFmpeg 适配、null、system/CPAL、Web Audio 参考桥、UI bridge
flutter_app/                Android/iOS/HarmonyOS/Windows/Linux/macOS/Web 单代码库
  web_node/                 Helia/UnixFS/Bitswap 源码、锁文件和互操作测试
docs/                       追踪矩阵、迁移说明、发布验收
.github/workflows/          检查、七端构建与受控发布
```

## 本地验证

需要 Rust stable 和 Flutter。当前工作区可用以下门禁：

```bash
cd backend
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo build --locked --workspace --all-features  # 先生成 FFI 动态库
cargo test --locked --workspace --all-targets --all-features
cargo build --locked --release --workspace
```

```bash
cd flutter_app/web_node
npm ci
npm audit --omit=dev --audit-level=high
npm run build
npm run test:interop
```

```bash
cd flutter_app
flutter pub get
flutter analyze
flutter test
flutter build web --release
flutter build linux --release
```

`scripts/ci_build.sh` 用于复现基础检查。只有受控 tag 的 release workflow 才负责正式签名、SBOM、
摘要、provenance、七端物理验收与产物汇总。验收 runner 契约见
[实机验收协议](docs/acceptance/README.md)。

## 启动控制面

```bash
cd backend
cargo run -p plugin-manager
```

默认监听 `127.0.0.1:8787`，仓库目录为 `./repo`，首次启动会生成 256-bit bearer token 并以
0600 保存到 `repo/control-token`。可配置：

- `JIMMUSIC_BIND_ADDR`：监听地址；默认只回环；
- `JIMMUSIC_REPO_DIR`：持久状态与本地 CAS；
- `JIMMUSIC_IPFS_LISTEN`：内置节点监听 multiaddr，逗号分隔；
- `JIMMUSIC_IPFS_BOOTSTRAP`：显式 bootstrap multiaddr，逗号分隔；
- `JIMMUSIC_IPFS_MDNS=0`：关闭局域网 mDNS；
- `JIMMUSIC_IPFS_GATEWAY`：可选 Kubo HTTP 兼容回退；不是默认可信/P2P 主路径；
- `JIMMUSIC_API_TOKEN`：显式提供控制面 token。

Flutter 控制中心默认连接 `http://127.0.0.1:8787/v1`。token 只保留在当前进程内，不写入普通
偏好存储。非回环连接必须使用 HTTPS；服务端 TLS 应由受控反向代理终结。

所有 v1 写请求必须发送 `Idempotency-Key` 或 DTO 中的 `request_id`。完整迁移与回滚说明见
[API_MIGRATION.md](docs/API_MIGRATION.md)。

## 平台与发布

目标平台为 Android、iOS、HarmonyOS、Windows、Linux、macOS 和 Web。HarmonyOS 使用固定
commit 的 Flutter OpenHarmony 工具链及自托管 runner。Android、iOS、macOS、Windows 的稳定
产物必须由 CI 使用平台签名密钥生成；无签名开发构建不能作为 release 证据。

只有当追踪矩阵中所有 P0 阻断项关闭、同一候选 commit 的七端产物和外部证据齐备时，才能创建
JimMusic 2.0 稳定标签。

## 许可

本项目以 [GNU GPL v3](LICENSE) 开源。发布二进制时必须同时提供与该提交对应的完整源码。
