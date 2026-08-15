# JimMusic 候选版本实机验收协议

稳定版发布必须由七个受控验收 runner 各自产生一个 JSON 报告。报告不是人工勾选表：runner
必须对待发布产物执行安装、断网播放、无公共网关 P2P、篡改拒绝、插件沙箱、升级/异常退出恢复、
后台生命周期、无障碍和诊断隐私场景，并把日志、截图、抓包或测量文件的 SHA-256 写入报告。

## Runner 接口

每个带有 `self-hosted`、`jimmusic-acceptance` 和平台标签的 runner 必须在 `PATH` 提供：

```text
jimmusic-acceptance run \
  --platform <windows|macos|linux|android|ios|harmonyos|web> \
  --commit <40 位 Git commit> \
  --artifacts <候选产物目录> \
  --output <报告目录>
```

Runner 还必须提供 POSIX `bash`、`tar` 和 GitHub Actions 所安装的 Node.js；Windows 实验室
runner 因此需要可从 `PATH` 调用的 Git for Windows Bash。验收命令不得依赖开发者个人目录或
未记录的交互式会话状态。

命令成功时在输出目录写入 `<platform>.json` 和其引用的证据。它必须使用物理设备；模拟器、
缺失证据、平台豁免或失败场景都不得生成通过报告。仓库不附带伪造的样例通过报告。

## 报告的强制内容

- 候选版本、精确 commit 和实际安装产物 SHA-256；
- 从需求文档自动读取的全部适用 P0 结果。`ALL` 不可标记 unsupported；`支持平台` 若无能力，
  必须明确标记 unsupported、给出原因和 UI/API 证据；
- 十二个端到端场景全部通过且至少有一份内容摘要证据；
- M0 启动时间、峰值内存、每小时耗电和每小时带宽基线与候选值，每项至少五个样本，回退不超过
  15%；
- 所有宣称支持的发烧音频能力必须记录设备、驱动、实际协商格式和证据。没有实机证据只能声明
  unsupported；
- `p0.exemptions` 必须为空。

校验器会重新计算适用需求和资源回退，不信任报告内的汇总数字：

```bash
node scripts/validate_acceptance_report.mjs \
  --commit "$(git rev-parse HEAD)" \
  --require-all-platforms reports/*.json
```

Release Candidate 工作流只有在七份同 commit 报告全部通过校验后才会发布；报告与原始证据也会
作为发布资产并进入 provenance attestation。
