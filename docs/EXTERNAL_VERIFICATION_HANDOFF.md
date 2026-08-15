# JimMusic 交付受阻后的行动清单（给项目所有者）

> 当前状态：P0 矩阵 **本机通过 100 / 部分实现 22 / 缺失 0 / 待外证 12**。
> 本清单逐项说明：还需要谁做什么、具体怎么做、以及反馈给我时用什么格式。
> 收到反馈后我会直接更新 `docs/REQUIREMENTS_TRACEABILITY.md` 与
> `docs/RELEASE_ACCEPTANCE.md`，并重新运行全部门禁。

---

## A. 立即事项（仓库层面，今天就能做）

### A1. 验证 Android CI 修复

commit `f45bc0e`（Gradle 8.14.3 + AGP 8.9.1）只改了两个文件，本地无 Android SDK 无法自验。

- **操作**：`git push origin main` → GitHub Actions 重跑 `Build (android-apk)`。
- **反馈格式**：成功就发“android-apk ✅”；失败就贴从 `Running Gradle task 'assembleRelease'...`
  到 `Process completed with exit code` 之间的完整段落。

### A2. 决定 17 个未提交改动的处置

工作区有 17 个**不是本轮产生**的未提交改动（workflow 加固、control-token 权限校验、
Flutter 若干修复等），我未擅自提交：

```
.github/workflows/{backend,flutter,harmonyos,release}.yml  .gitignore  README.md
scripts/ci_build.sh  flutter_app/web_node/package.json
backend/plugin-manager/src/lib.rs
flutter_app/lib/screens/player_screen.dart
flutter_app/lib/services/control_api_{sse,types,web}.dart
flutter_app/lib/widgets/plugin_config_form.dart
flutter_app/test/{control_api_sse_test,control_cancel_test,plugin_config_form_test}.dart
```

- **反馈格式**（三选一）：
  - “全部由我提交”→ 我不动；
  - “交给你审查” → 我审查、补测试、按轮次提交；
  - “丢弃除 X 外” → 明确列出保留文件。

---

## B. 七端实机验收（解除 PROD-001/003/004、NOD-002/008、REL-006 与 12 项“待外证”）

这是最大的一块。仓库已定义完整协议（`docs/acceptance/README.md`），报告会被
`node scripts/validate_acceptance_report.mjs --require-all-platforms` 自动校验，
**手工描述或截图不代替报告 JSON**。

- **需要**：七台带 `self-hosted` + `jimmusic-acceptance` + 平台标签的物理 runner
  （windows/macos/linux/android/ios/harmonyos/web），`PATH` 提供
  `jimmusic-acceptance run --platform … --commit … --artifacts … --output …`。
- **runner 必须输出**：`<platform>.json` + 引用的证据文件（日志/截图/抓包/测量），
  报告关键约束：`runner.physical_device: true`、`p0.exemptions: []`、
  `ALL` 范围 P0 只能 `pass`、每个 P0/场景至少一条带 SHA-256 的证据、M0 每项 ≥5 样本且
  回退 ≤15%。报告骨架（字段名必须完全一致）：

```json
{
  "schema_version": 1,
  "candidate": {"version": "2.0.0", "commit": "<40位小写hex>", "artifact_sha256": "<64位小写hex>"},
  "platform": "android",
  "generated_at": "2026-08-15T12:00:00Z",
  "runner": {"id": "…", "os_version": "…", "device_model": "…", "physical_device": true},
  "p0": {"result": "pass", "exemptions": [], "requirements": [
    {"id": "PLR-003", "result": "pass", "evidence": [{"uri": "…", "sha256": "…"}]}
  ]},
  "scenarios": [
    {"id": "offline_library_and_playlist", "result": "pass", "evidence": [{"uri": "…", "sha256": "…"}]}
  ],
  "resources": [
    {"name": "startup_ms", "baseline": 1200, "candidate": 1300, "unit": "ms", "samples": 5, "evidence": {"uri": "…", "sha256": "…"}}
  ],
  "audio_capabilities": [
    {"capability": "wasapi_exclusive", "declaration": "unsupported", "reason": "本实验室无 Windows 音频设备（≥20字）"}
  ]
}
```

- **本地自检命令**（提交前先跑，报错信息就是格式问题）：
  `node scripts/validate_acceptance_report.mjs --commit <commit> --require-all-platforms reports/*.json`
- **反馈格式**：把七个 `*.json` 和证据放入仓库（如 `docs/acceptance/reports/<commit>/`），
  或先发一个平台的报告样例给我确认格式，再批量执行。

---

## C. 硬件音频实验室（解除 PLR-004、BPT-002/003/007、UI-008）

- **需要设备**：Windows（WASAPI Exclusive/ASIO 声卡）、macOS（CoreAudio Hog 设备）、
  Linux（ALSA hw/PipeWire 设备）、DSD 解码器（DSD Native 与 DoP 各至少一台）及
  DSD64 测试源。
- **操作**：在对应平台开启 Bit-perfect，逐项验证
  独占会话、音量旁路（PLR-004/BPT-002）、DSD 不经 f32 PCM（BPT-003）、
  会话复现（BPT-007）；每项导出控制台 Audio Path 快照 + 会话 JSON。
- **反馈格式**：每项一条 `audio_capabilities` 条目（`capability/declaration/device/driver/
  negotiated_format/evidence`，见 B 节骨架），加控制台 Audio Path 截图与打开会话证据。

---

## D. 移动系统集成（解除 SEC-001、COM-009、NOD-008）

- **SEC-001 Keychain/Keystore**：需先决策——是否要我把 Keychain 封装代码写出来
  （代码可本地实现并配单元测试，但**真机验证**必须在 Android/iOS 设备上做）。
  反馈格式：“实现 Keychain 封装（我来验证）” 或 “维持 0600 文件并调整验收口径”。
- **COM-009**：
  1. 相机二维码扫描：Android/iOS 真机扫码导入社区源（含导入失败/无效码负向案例）。
     反馈：机型+OS+扫码结果录屏。
  2. 远端 Feed 头：官方启动源目前无远端 Catalog/Policy 头。需要你提供托管域名/服务器，
     并决定维护者私钥的离线保管方案；我方可生成签名 Feed 与部署脚本。
- **NOD-008**：Android/iOS 真机后台矩阵——锁屏/杀进程/省电模式下传输与节点行为，
  反馈每例“机型/OS/操作/期望/实际/日志 SHA-256”。

---

## E. 无障碍人工审计（解除 NFR-011）

- **操作矩阵**：键盘全流程、读屏（iOS VoiceOver / Android TalkBack / NVDA / macOS
  VoiceOver）、200% 缩放、对比度，覆盖播放页与控制中心七个标签页。
- **反馈格式**：每行
  `平台 | 设备/读屏版本 | 检查项 | 通过/失败 | 问题描述（含复现步骤） | 证据文件`。

---

## F. 长期稳定性与口径决策（解除 NFR-009，为 NFR-004 补历史）

- **NFR-009**：99.5% 可用性需要七端长期样本。二选一并反馈：
  a) 提供采样方案与周期，我们定义收集格式；b) 由你决策**下调声明口径**（改需求文档），
  我同步矩阵与验收文案。
- **REL-006**：升级/降级/杀进程/断电/安全模式矩阵按平台执行，反馈同 B 节证据格式。

---

## G. 大型架构决策（这些**不需要外部资源，我可以继续在会话内实现**）

阻塞说明把它们归为大型开发而非环境缺失。若你希望我继续，请按序点名（建议顺序 1→5）：

1. **AGR-012 逐节点 CPU 指标**（小）：图执行按节点计时 + API/诊断 UI；
2. **AGR-011 DSP 崩溃/超时注入链路**（中）：可执行节点 + 注入 harness；
3. **AGR-003 DSD/Encoded 实际处理链**（大）：非 PCM 图端口执行；
4. **BPT-006 Web Audio PCM 桥接入 Flutter**（大）；
5. **SEC-004/005 Web/iOS/HarmonyOS 插件执行载体与独立进程隔离**（最大，跨轮次）。

反馈格式示例：“继续实现 G-1 和 G-2”，我会按既有节奏（实现+测试+文档+提交）逐轮推进，
并更新追踪矩阵。

---

## 反馈通用格式

任何一条反馈请尽量带：**平台/设备型号 + OS 版本 + Git commit(40位) + 结论 + 证据路径或
SHA-256**。验收类证据优先走 B 节 JSON 报告；排障类（如 A1）直接贴日志段落。
