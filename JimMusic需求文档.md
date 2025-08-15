## JimMusic 需求文档

### 版本历史

- **1.0**（2025-07-23）初版 :contentReference[oaicite:1]{index=1}
- **1.1**（2025-07-23）增加 IPFS 网络接入需求

---

### 1. 引言

**项目名称**：JimMusic
**版本**：1.0
**编制日期**：2025年7月23日

#### 1.1 背景与目的

JimMusic 是一款跨平台、模块化、高性能且体积小巧的音乐播放软件，目标支持 Android、iOS、HarmonyOS、Windows、Linux、macOS 等多种操作系统。文档旨在明确项目的功能和非功能需求，为后续设计、开发、测试和部署提供依据。

#### 1.2 术语与缩略语

- **App Core**：应用程序核心，负责插件加载、调度与统一接口。
- **Plugin**：插件，动态库形式实现解码器、UI 桥、搜索、收藏等功能。
- **Plugin Manager**：插件管理器，负责联网获取、安装和更新插件。
- **ArkUI**：HarmonyOS 官方 UI 框架。
- **Flutter App**：基于 Flutter 的跨平台前端界面应用。

### 2. 范围与目标

#### 2.1 范围

- **平台支持**：Android、iOS、HarmonyOS（ArkUI）、Windows、Linux、macOS。
- **模块化**：所有功能（解码器、UI、搜索、收藏）通过插件形式提供，支持动态安装/卸载/更新。
- **高性能**：采用 Rust 语言开发核心，FFmpeg 或 Symphonia 解码器，保证低延迟、低内存占用。
- **最小体积**：核心及插件进行按需加载，减小基础安装包体积。

#### 2.2 项目目标

- 支持主流音频格式（MP3、AAC、FLAC、WAV、OGG 等）。
- 提供流畅的播放体验（seek <50ms、平均 CPU 占用<10%）。
- 插件热插拔：运行时加载/卸载，无需重启主程序。
- 内置插件管理器：可在线浏览、下载、更新插件。
- 支持用户自定义 UI 主题与布局。

### 3. 功能需求

### 3.1 应用程序核心（Rust）

- 插件加载与管理：动态发现、加载、卸载 `.so/.dll/.dylib` 插件。
- 统一 C ABI 接口：定义插件注册、调用、回调函数。
- 异步调度：基于 Tokio 处理消息总线和网络请求。
- 日志与错误处理：可配置日志级别，统一错误码规范。
- **IPFS 接入**
  - 集成 IPFS 客户端（`rust-ipfs` 或 HTTP API 方式）。
  - 在异步任务中并发执行 CID 查询、数据下载与流式传输。
  - 下载后进行内容签名校验。
  - 本地缓存与 Pin 管理，支持 LRU 或定期清理策略。

#### 3.2 解码器插件

- FFmpeg 解码器
  - 支持常见音频格式；
  - 动态编译为共享库，支持多平台；
- Symphonia 解码器
  - 纯 Rust 实现，静态链接；
  - 作为可选插件，减小体积。

#### 3.3 UI 桥接插件

- 提供 FFI 接口与 Flutter/ArkUI 通信；
- 事件总线：播放/暂停/进度回调；
- 资源管理：封面图、歌词同步。

#### 3.4 Flutter 应用

- 播放页：专辑封面、播放控制、进度条；
- 媒体库：本地扫描、搜索、分类；
- 收藏/播放列表管理；
- 可切换主题：深色/浅色/自定义。

#### 3.5 HarmonyOS ArkUI

- ArkTS 页面：与 Flutter 同类功能页；
- 与 Core Engine FFI 通信；
- 栈页面切换、页面状态保持。

#### 3.6 插件管理器（Rust/axum）

- RESTful 接口：列举、下载、安装、卸载、升级插件。
- **IPFS 源支持**：新增基于 CID 的查询与下载端点，优先 IPFS，再回退 HTTP 镜像。
- 元数据管理：版本号、签名校验。
- 本地仓库缓存与清理策略。

### 3.7 音乐文件获取

- **IPFS 网络**：用户在播放或收藏音乐时，可通过 CID 在 IPFS 网络检索并下载音频流。
- 支持边下载边播放的流式解码。

### 4. 非功能需求

#### 4.1 性能

- 启动时间<1s；
- 解码与播放延迟<50ms；
- 常驻内存<50MB；

#### 4.2 安全

- 插件文件签名校验；
- 防篡改机制；
- 网络传输 TLS 加密。
- TLS（HTTP），libp2p 节点认证，下载内容签名校验。

#### 4.3 可用性

- UI 响应时间<100ms；
- 断网情况下本地功能可用；
- 在 IPFS 网络不稳定情况下，自动重试与回退 HTTP。

#### 4.4 可维护性

- 代码覆盖率≥80%；
- 模块间低耦合，高内聚；
- 文档与注释完整。

#### 4.5 可扩展性

- 支持新增第三方插件；
- 提供 SDK 文档与示例。

### 5. 系统架构

```
+------------------------------+
|     Flutter / ArkUI / Web    |
+--------------+---------------+
               |
               | FFI / IPC
               |
+--------------v---------------+
|            Core             |
|   (Rust + Tokio 异步)       |
|   + IPFS 客户端模块         |
+------+----------+------------+
       |          |
       | 动态加载  | 调用 Plugin Manager (axum)
       v          v
+------+--+   +---+--------------+
| Plugins |   |  Plugin Manager  |
|（支持 HTTP & IPFS）|            |
+---------+   +------------------+
       |
       v
  IPFS 网络 / HTTP 镜像
```

### 6. 技术选型总结

- **应用程序核心**：Rust + Tokio + `rust-ipfs` 或 HTTP API；
- **解码**：FFmpeg（动态）+ Symphonia（静态）；
- **UI**：Flutter + ArkUI (HarmonyOS)；
- **通信**：FFI / IPC / libp2p / HTTP；
- **存储**：SQLite/RocksDB + 本地 IPFS 缓存；
- **构建**：Cargo + cross、Flutter build、HAP CLI；
- **CI/CD**：GitHub Actions，自动部署 IPFS 节点测试环境。

### 7. 目录结构示例

```
JimMusic/
├── arkui/
├── flutter_app/
├── backend/
│   ├── Cargo.toml
│   ├── app-core/
│   │   ├── ipfs_client/
│   │   └── ...
│   ├── plugin-manager/
│   └── plugins/
├── scripts/
├── third_party/ffmpeg/
├── .github/
├── README.md
└── .gitignore
```

### 8. 部署与发布

- **桌面**：生成跨平台安装包（MSI/DMG/DEB/RPM）；
- **移动**：APK/AAB、IPA；
- **HarmonyOS**：.hap/.app；
- **自动化**：CI 集成 IPFS 网络连通性测试与签名验证。

---

*文档版本 1.1，如有补充或调整请更新此文档。*
