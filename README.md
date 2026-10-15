<div align="center">
  <img src="docs/app-icon.png" alt="RScriptoBox" width="88" height="88" />
  <h1>RScriptoBox</h1>
  <p>Windows 脚本工作台 · 管理、配置与交互执行</p>
  <p>
    <img alt="Rust" src="https://img.shields.io/badge/Rust-2021_Edition-CE422B" />
    <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2-24C8DB" />
    <img alt="Vue" src="https://img.shields.io/badge/Vue-3-42B883" />
    <img alt="Platform" src="https://img.shields.io/badge/Windows-10%20%2F%2011%20x64-7856C7" />
    <img alt="Project License" src="https://img.shields.io/badge/License-Unlicense-0F766E" />
  </p>
  <p>by <a href="https://github.com/rurikacy">Rurikacy</a> · <a href="README_EN.md">English</a></p>
  <p><a href="#-界面预览">界面预览</a> · <a href="#-快速开始">快速开始</a> · <a href="#-源码构建">源码构建</a> · <a href="SUPPORT.md">使用支持</a></p>
</div>

---

## 📖 简介

**RScriptoBox** 使用 Rust、Tauri 2 和 Vue 3 构建，将 CMD / BAT、PowerShell、Python 脚本放在同一个工作台中管理。导入时创建脚本副本，绑定明确的解释器，配置参数与工作目录，在内置交互终端中执行并查看历史。

应用采用暗色紫色界面与便携目录分发。运行发布版无需安装 Rust 或 Node.js；脚本解释器与 WebView2 是独立运行前提。当前界面语言为简体中文，文档提供中英文版本。

## 🖼️ 界面预览

<p align="center">
  <img src="docs/screenshots/script-library.png" alt="RScriptoBox 0.2.0：暗色脚本库、分类筛选与运行配置入口" width="1120" />
</p>

<table>
  <tr><th>环境中心</th><th>运行历史</th></tr>
  <tr>
    <td><a href="docs/screenshots/runtime-center.png"><img src="docs/screenshots/runtime-center.png" alt="检测并管理 CMD、PowerShell 与 Python 解释器" width="560" /></a></td>
    <td><a href="docs/screenshots/run-history.png"><img src="docs/screenshots/run-history.png" alt="按结果和时间筛选执行记录，查看状态、耗时与退出码" width="560" /></a></td>
  </tr>
</table>

截图由项目维护者提供，展示 0.2.0 实际界面；脚本名称等内容已在原图中打码。点击缩略图可查看原尺寸，截图中的解释器版本与路径仅代表该设备。

## ✨ 功能特性

| 功能 | 说明 |
| --- | --- |
| **脚本库** | 添加或拖入 `.cmd`、`.bat`、`.ps1`、`.py`；搜索、分类、标签、收藏、卡片与列表。 |
| **工作台副本** | 导入至 `data/scripts/<ID>/`，运行和编辑使用副本，删除原入口文件后仍可使用。 |
| **执行配置** | 绑定解释器，设置参数、工作目录、环境变量、超时及成功退出码。 |
| **交互终端** | ConPTY + xterm.js，支持键盘输入、`input()`、`Read-Host`、`pause` 与 Ctrl+C；也可使用系统控制台。 |
| **环境中心** | 检测或手动登记解释器；查看安装计划后准备 Python / PowerShell 7，安装后复检。 |
| **Python 依赖** | 创建独立 venv，并明确选择 `requirements.txt` 安装依赖。 |
| **任务与历史** | 查看实时输出、状态、耗时和退出码，回放或导出日志，停止受控任务树。 |
| **显示与数据** | 默认 16px 正文，可选 14 / 16 / 18 / 20px，支持 DPI 缩放；便携数据、数据库备份与恢复。 |

## 📥 快速开始

1. 从 [Releases](https://github.com/rurikacy/RScriptoBox/releases/latest) 下载 `RScriptoBox-0.2.0-windows-x64-portable.zip`；源码仓库不包含发行 ZIP。
2. 解压到可写的本地目录，保留 `RScriptoBox.exe` 旁的 `portable.flag`，双击程序启动。
3. 在 **环境中心** 检测已有解释器，缺失时查看安装计划，或手动登记实际解释器路径。
4. 在 **脚本库** 添加脚本，打开 **配置** 绑定运行环境，再点击 **运行脚本**。
5. 可先导入 `examples/interactive.py`、`interactive.ps1` 或 `interactive.cmd` 体验交互输入。

**运行环境：Windows 10 22H2 / Windows 11 x64，以及 [Microsoft Edge WebView2 Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。** 发布包不捆绑 WebView2、Python 或 PowerShell 7。程序未做数字签名。Windows 10 有本地验证记录，Windows 11 尚待实机验证，详见 [验证记录](docs/verification.md)。

CMD 与 Windows PowerShell 5.1 属于系统组件，应用只检测、不替换。环境安装使用 WinGet 或官方 Python Install Manager；安装计划列出来源与可能的系统交互，UAC、网络和企业策略可能影响完成。

## 🧭 使用与迁移

### 运行与交互

每个参数单独填写，空参数会保留。首次运行或脚本内容变更后需要确认信任。点击内置终端即可键入，Ctrl+C 交给脚本处理，**强制停止** 则结束受控任务树。查看运行中的日志回放时，先点击 **返回实时** 才能输入。

终端使用 UTF-8，合并 stdout / stderr；日志导出保留终端控制序列，复制按钮提供当前屏幕和滚动缓冲的纯文本。密码是否隐藏取决于脚本采用的输入方法，普通输入回显可能进入日志。

### 脚本副本

导入仅复制入口脚本，不自动收集同目录资源或依赖。通过 **打开所在目录** 将关联文件放入副本目录；默认工作目录就是该目录。原文件后续修改不会自动同步，**重新导入副本** 会创建新版本并保留旧副本。移除管理记录不会删除原文件或工作台副本。

### 升级与跨设备使用

- **升级已有便携版：** 退出程序并备份整个旧目录，将新包中的 EXE 替换到原目录，保留 `portable.flag` 和 `data`，避免改变已有 venv 的路径。
- **移动或换设备：** 退出后复制整个便携目录，包含 `data`。脚本路径及位于 `data/scripts` 内的工作目录以相对路径保存；在目标设备重新检测并绑定解释器，重建 Python venv。
- **旧版迁移：** 0.2.0 升级前备份数据库，复制仍可访问的旧脚本入口到工作台。源文件缺失或复制失败的记录需要重新导入。数据库升级至 schema 2，回退须使用升级前的完整目录备份。
- **备份边界：** 数据库备份不包含脚本文件，不能代替整个 `data` 的备份。外部工作目录、绝对路径参数和系统依赖需在目标设备重新配置。

单独复制 EXE 时，应用默认使用当前用户应用数据目录；`--portable` 参数也能启用便携模式。常见问题与数据库恢复步骤见 [使用支持](SUPPORT.md)。

## 🔧 源码构建

需要 Windows、Rust stable MSVC 工具链、Visual Studio C++ Build Tools / Windows SDK、Node.js 24 与 WebView2。npm 和 Rust 依赖分别由 `package-lock.json`、`src-tauri/Cargo.lock` 固定。

```powershell
npm ci
npm run tauri dev

# 检查与测试
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
cargo test --manifest-path tests/runner/Cargo.toml -- --test-threads=1

# 生成便携程序与 SHA256；运行测试需要对应解释器
pwsh -File scripts/package-portable.ps1
```

产物位于 `release/`。前端资源嵌入 EXE，运行发布版无需开发服务器。浏览器开发预览使用示例数据，不能执行桌面操作。更新依赖后执行 `node scripts/generate-notices.mjs` 更新许可清单。

## 🗂️ 项目结构

```text
RScriptoBox/
├── src/                       # Vue 界面与交互终端
├── src-tauri/                 # Rust 后端、Windows 进程与数据管理
├── examples/                  # 基础与交互示例
├── tests/runner/              # Windows 原生执行测试
├── scripts/                   # 构建、打包与依赖许可生成
├── docs/                      # 需求、设计、验证、截图及第三方许可
├── .github/                   # CI 与 Issue / PR 模板
├── README.md / README_EN.md
├── LICENSE / THIRD-PARTY-NOTICES.txt
└── CONTRIBUTING.md / SECURITY.md / SUPPORT.md
```

## ⚠️ 边界与兼容性

- 应用是受信任脚本的管理器，**不是沙箱**；脚本拥有启动程序的权限。请审阅脚本，按需使用管理员权限。
- CMD 路径与参数采用保守字符范围，含括号、百分号、感叹号、引号等特殊字符会被明确拒绝。复杂输入可由脚本读取配置文件。
- 不永久修改 PowerShell 执行策略；可明确选择仅本次进程使用 RemoteSigned 或 Bypass，企业组策略仍优先。
- 安装器可能在应用之外继续运行，取消不能保证回滚；强制停止脚本可能中断数据写入。
- 当前版本的测试结果与未验证范围见 [验证记录](docs/verification.md)，不能据此推定所有系统和显示设备已通过验收。

## 🤝 参与与反馈

请阅读 [贡献指南](CONTRIBUTING.md)、[行为准则](CODE_OF_CONDUCT.md) 与 [使用支持](SUPPORT.md)。可通过 [Issues](https://github.com/rurikacy/RScriptoBox/issues) 反馈；安全问题请遵循 [安全说明](SECURITY.md)，不要公开敏感脚本或日志。

[产品需求](docs/requirements.md) · [技术设计](docs/design.md) · [发布维护说明](GITHUB_PREPARATION.md)

## 📄 许可证

项目自有源码、文档与应用图标采用 [The Unlicense](LICENSE)。第三方库、UI 图标与随依赖提供的材料保留各自许可证，不受本项目公共领域声明替代。来源、版本及原始许可文本见 [第三方声明](THIRD-PARTY-NOTICES.txt) 与 [依赖许可清单](docs/licenses/dependencies.txt)。外部解释器、安装器和 WebView2 不随本便携包分发。
