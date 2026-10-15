# 贡献指南 / Contributing

欢迎提交问题、文档修正和代码改进。请先阅读 [行为准则 / Code of Conduct](CODE_OF_CONDUCT.md)；漏洞按 [安全说明 / Security](SECURITY.md) 报告。

## 中文

### 反馈问题

先搜索已有 Issue，再使用问题报告或功能建议模板。提供版本、Windows 版本、解释器版本、最小复现步骤、预期与实际结果。脚本、日志和截图请使用可公开的最小样例，移除凭据、私人路径与业务数据。

### 开发与检查

工具链及命令见 [README](README.md#-源码构建)。代码修改运行相关测试；涉及 Windows 执行时还要运行 `tests/runner` 套件。重点覆盖中文与空格路径、输入交互、任务停止、退出码，以及副本导入和迁移后的完整性。不要在真实用户数据上做破坏性测试。

文档修改检查中英文一致性、相对链接和图片显示，无需为了文档重跑全部程序测试。维护原始截图打码；更新依赖时重新生成许可清单并检查遗漏。

### Pull Request

- 一个 PR 解决一个明确问题，说明修改后的行为和实际完成的验证。
- 缺陷修复补充可重现的回归检查；未验证的场景明确列出。
- 不提交构建目录、发行包、`data/`、个人脚本、日志、虚拟环境或凭据；保留依赖锁文件。
- 自有贡献按 [The Unlicense](LICENSE) 提供；第三方材料注明来源、版本及许可证，并保留版权声明。

## English

Search existing issues before using a bug or feature template. Include application, Windows and interpreter versions, minimal reproduction steps, and expected versus actual behavior. Share only public-safe samples; remove credentials, private paths and business data from scripts, logs and screenshots.

See [README](README_EN.md#-building-from-source) for tools and commands. Run checks relevant to the change. Process changes also need the `tests/runner` suite, covering Unicode/space paths, interactive input, cancellation, exit codes and workspace-copy migration. Do not use real user data for destructive tests.

For documentation changes, verify language parity, relative links and images; a full application test run is unnecessary. Preserve screenshot redactions. Regenerate and inspect the license inventory when dependencies change.

Keep each PR focused, describe the resulting behavior and checks performed, add regression coverage for bugs, and disclose untested scenarios. Exclude build outputs, release archives, `data/`, personal scripts, logs, virtual environments and secrets; retain lockfiles. By submitting your own contribution, you confirm you may provide it under [The Unlicense](LICENSE). Third-party materials retain their original license and attribution requirements.
