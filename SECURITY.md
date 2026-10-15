# 安全说明 / Security Policy

## 中文

RScriptoBox 执行用户信任的本地脚本，不提供沙箱隔离。脚本与解释器拥有启动应用的权限；环境安装可能启动外部安装器。安全问题包括参数与路径处理、权限边界、数据读写、依赖和终端输出处理。

安全修复优先面向当前开发分支与最新版本，不承诺维护所有旧版或固定响应时间。

1. 若仓库启用了 GitHub 私有漏洞报告，使用 **Security → Advisories → Report a vulnerability**。
2. 若未启用，使用维护者公开提供的私下渠道；没有渠道时，仅创建不含漏洞细节的 Issue 请求联系方式。
3. 在建立合适渠道前，不要公开利用步骤、攻击样例、凭据或私人日志。报告应包含受影响版本、Windows / 解释器版本、影响及最小复现。

反馈脚本行为前请区分应用缺陷与脚本自身的正常权限。日志可能包含脚本输出和输入回显；环境变量、参数或隐藏输入也可能被脚本主动输出。

## English

RScriptoBox executes trusted local scripts and is not a sandbox. Scripts and interpreters inherit the application's privileges, and runtime installation may launch external installers. Relevant security areas include argument/path handling, privilege boundaries, storage, dependencies and terminal output processing.

Fixes prioritize the current development branch and latest version; there is no commitment to support every old release or a fixed response time.

If enabled, use GitHub's **Security → Advisories → Report a vulnerability**. Otherwise, use a private contact channel publicly provided by the maintainer. If none exists, open an issue without exploit details requesting a private channel. Do not publish exploit steps, attack samples, credentials or private logs before a suitable channel is established. Include affected versions, Windows/interpreter versions, impact and a minimal reproduction.

Distinguish application defects from the script's intended privileges. Logs can include script output and input echo; scripts may explicitly print arguments, environment variables or even values entered through hidden input.
