# 发布维护说明

本仓库为 [rurikacy/RScriptoBox](https://github.com/rurikacy/RScriptoBox)，默认分支为 `main`。本文用于维护后续发布；私有漏洞报告是否可用，以仓库 Security 页面的实际入口为准。

## 发布前检查

- 确认 `package.json`、锁文件、Cargo.toml、tauri.conf.json、程序版本和发行文件名一致。
- 运行受改动影响的检查，并更新 [验证记录](docs/verification.md)；不要将旧测试结果写成新验证。
- 检查两份 README、相对链接、截图与程序界面一致；截图避免暴露个人数据。
- 更新依赖后运行 `npm ci`、`cargo fetch --manifest-path src-tauri/Cargo.toml --locked`，然后运行 `node scripts/generate-notices.mjs`。生成器离线读取本地包；检查无独立许可文本的条目，必要时保留上游补充声明。
- 保留 `LICENSE`、`THIRD-PARTY-NOTICES.txt`、完整依赖许可和两个 README。项目使用 The Unlicense，不改变第三方许可。
- 用 `git status --short` 检查候选文件；不要上传 `data`、`release`、测试输出、虚拟环境、日志或凭据。`.gitignore` 不能移除已经跟踪的文件。

## 构建与打包

```powershell
pwsh -File scripts/package-portable.ps1
# 构建已完成时，只更新文档和发行包
pwsh -File scripts/package-portable.ps1 -SkipBuild
# 原发行目录正在使用或含有 data 时，改用独立输出目录
pwsh -File scripts/package-portable.ps1 -SkipBuild -OutputDirectory release/documented
```

打包脚本只收集程序、标志文件、根目录公开文档、`docs` 和 `examples`，并生成 SHA256。如果目标发行文件夹含有 `data`，脚本会停止，避免覆盖或泄露用户数据；请选择新的输出位置，不要删除用户数据来绕过检查。

标准便携包不包含 Python、PowerShell 7、WinGet 或 WebView2；这些软件由用户单独准备。发布时说明程序未签名、Win11 未完成实机验收，以及脚本关联资源和 venv 的迁移要求。

## 文档维护

`README.md` 与 `README_EN.md` 内容应同步。三张截图位于 `docs/screenshots/`，由维护者提供并保留原有打码；不要把 README 中的路径换成本机绝对路径。`docs/app-icon.png` 与 `src-tauri/icons/256.png` 一致，图标改变时同步。

`docs/licenses/dependencies.txt` 是 Windows Rust 依赖图与当前主机已安装 npm 包的源码/构建许可清单，包含开发工具，不应称为二进制链接分析。平台可选但未安装的 npm 包另列，不宣称已收齐它们的许可文本。

## 远端操作

发布时将 Release 标签固定到已检查的提交，附上便携包、对应提交的源码 ZIP、SHA256SUMS.txt 与中英文说明。Issue / PR 模板在 GitHub 上使用；CI 是否通过以真实运行记录为准。安全反馈渠道启用后，可更新 SECURITY.md 中的说明。
