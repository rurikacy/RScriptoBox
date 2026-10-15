# Windows 执行器验证

在 Windows x64、已安装 Python 与 PowerShell 7 的开发机运行：

```powershell
cargo test --manifest-path tests/runner/Cargo.toml -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib real_runner_sustained_output -- --ignored --nocapture --test-threads=1
```

Python 默认从 PATH 中查找真实的 `python.exe`，排除 WindowsApps 商店别名；也可设置 `RSCRIPTO_TEST_PYTHON` 为绝对路径。PowerShell 7 使用默认的 `C:\Program Files\PowerShell\7\pwsh.exe`。测试目录为带随机标识的系统临时目录，不读写用户脚本。

独立套件直接使用产品的 `process_windows.rs`，验证实际参数往返、中文和空格目录、空参数、引号和尾反斜杠、cwd 与环境、CMD 注入拒绝、双流大输出、父进程提前退出、整 Job 终止、关闭最后 Job 句柄终止，以及真实新控制台 `input()` 输入。

PowerShell 参数测试显式使用**仅该测试进程**的 `-ExecutionPolicy Bypass`，不修改任何持久策略。产品默认遵循机器既有策略，仅在用户明确选择后才为该次进程传递 `RemoteSigned` 或 `Bypass`。

产品库中的 runner 测试另外验证 SQLite 状态落盘、日志配额后排空、超时、立即停止、同脚本不重入、并发限制、入口文件变化、敏感输入元数据脱敏、UTF-8/GB18030 跨块与双流跨分页回放，以及未确认 IPC 输出窗口的内存上限。

故障测试会强制结束专门创建的测试宿主，验证 Job 子进程退出和重启后的 interrupted 历史；另用临时日志文件上的 Windows 文件范围锁制造真实写入失败，验证停止与错误落盘。它们不终止用户正在使用的 GUI，不修改用户数据库。

60 秒测试默认忽略，使用上面的第三条命令显式执行。它按 1 MiB/s 向两个输出流交替写入，检查默认 20 MiB 配额及数据库响应；不以后台结果替代 WebView 全链路性能验收。

Windows Terminal 与传统控制台的人工关闭、Ctrl+C、PowerShell `Read-Host`、批处理 `pause`，以及磁盘已满和应用被强制结束的系统级故障矩阵，仍应在干净的 Windows 虚拟机做发布验收。

0.2.0 新增 ConPTY 真实控制台句柄与输入输出往返测试；主 crate 的 real_terminal 测试覆盖中文输入、getpass、Ctrl+C、尺寸变化、CMD set /p/pause 及 PowerShell 5.1/7 Read-Host/ReadKey。

终端测试通过共享的 `terminal_text.rs` 去掉 ConPTY 插入的 CSI/OSC 控制序列，再断言顺序文本。回归用例包含 GitHub Actions 在 `TTY` 与 `True True` 之间插入窗口标题的真实输出；标题内的文本不应被当作程序输出。该辅助函数仅用于测试，不模拟光标重绘，也不修改产品原始日志；界面渲染仍需单独验证。

故障恢复夹具通过临时文件加原子重命名发布 PID，避免父进程读到空文件；临时目录清理仅对 Windows 共享/锁冲突限时等待，以容纳 ConPTY 异步释放句柄。其他错误直接失败，真实文件独占锁用例覆盖清理等待路径。

工作目录断言先规范化双方路径，兼容 GitHub Windows 主机上的 `RUNNER~1` 8.3 短路径与 PowerShell 返回的长路径，同时仍要求实际目录一致。
