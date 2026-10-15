# 使用与支持 / Support

[中文 README](README.md) · [English README](README_EN.md) · [安全说明 / Security](SECURITY.md)

## 常见问题

| 问题 | 处理方式 |
| --- | --- |
| 启动提示缺少 WebView2 | 从 README 中的 Microsoft 官方入口安装 Evergreen Runtime。 |
| 脚本无法运行 | 检查环境绑定、解释器路径和退出码；缺失解释器时先检测、登记或安装。 |
| PowerShell 被策略阻止 | 默认遵循系统策略；需要时明确选择仅本次进程的执行策略，企业组策略仍优先。 |
| 内置终端无法输入 | 点击终端获得焦点；日志回放是只读的，运行中选择“返回实时”。已结束任务不能继续输入。 |
| 迁移后文件或模块找不到 | 只自动复制入口脚本。将资源放进副本目录，修正外部绝对路径，重新绑定解释器并重建 venv。 |
| 原脚本修改未同步 | 工作台使用独立副本；编辑副本或使用“重新导入副本”。 |
| 删除管理记录后文件仍在 | 属于预期行为，原文件和副本均保留，避免误删用户资料。 |
| 字体偏小 | 在偏好设置中选择 18px 或 20px，也可使用 Windows 显示缩放。 |
| 无法用外部编辑器打开 | 配置 Windows 对应文件类型的 edit 关联；程序不会回退到可能执行脚本的默认打开动作。 |
| 安装取消后仍有进程 | 外部安装器可能继续运行；确认关闭后重新检测，再解除未决安装状态。 |

### 数据库恢复

1. 如果因数据损坏无法启动，错误窗口会显示数据目录。先退出所有实例，将原数据目录改名保留；不要删除。
2. 重启创建空数据目录，在“偏好设置 → 恢复数据库”选择旧目录 `backups` 中的有效备份或自行保存的备份。
3. 退出应用，将旧目录中的 `scripts` 文件夹复制到新 `data` 中，再启动并重新确认脚本信任；重建应用管理的 Python venv。
4. 数据库备份不含脚本或日志。保留旧目录供恢复这些文件；若错误是“数据库版本过新”，请改用对应新版程序，不要当作损坏处理。

反馈时提供版本、Windows / 解释器版本、复现步骤和已脱敏的错误信息。不要上传真实秘密、整个数据库或私人脚本库。项目不保证即时回复。

## Troubleshooting

| Problem | Action |
| --- | --- |
| WebView2 missing | Install Evergreen Runtime from the Microsoft link in the README. |
| Script will not run | Check its runtime binding, interpreter path and exit code; detect, register or install missing runtimes. |
| PowerShell policy blocks execution | System policy applies by default. Explicitly select a process-only policy if appropriate; group policy still takes precedence. |
| Terminal does not accept input | Click it to focus. Saved logs are read-only; return to live output for running tasks. Finished tasks cannot receive input. |
| Files/modules missing after a move | Only the entry script is copied automatically. Add resources to its folder, fix external absolute paths, rebind interpreters and recreate venvs. |
| Original changes are not reflected | Edit the workspace copy or use Reimport copy. |
| Files remain after record removal | Expected: originals and workspace copies are retained to avoid accidental data loss. |
| Text is too small | Select 18px or 20px in Preferences, or use Windows display scaling. |
| External editor does not open | Configure the file type's Windows edit association; the app does not fall back to an action that could execute scripts. |
| Installer continues after cancellation | External installers may continue; confirm they have exited, recheck runtimes and then clear pending installation state. |

### Database recovery

If corrupted data prevents startup, the error window shows the data directory. Exit every instance, rename the directory to preserve it, and restart to create an empty one. In Preferences, restore a valid database backup from the old `backups` folder or your own backup.

Exit again, copy the preserved `scripts` folder into the new `data` directory, then restart and reconfirm script trust. Recreate managed Python venvs. Database backups do not include scripts or logs; keep the old directory to recover those files. A “database too new” error requires a newer compatible application, not corruption recovery.

Provide application, Windows and interpreter versions, reproduction steps and sanitized errors when requesting help. Do not upload real secrets, whole databases or private script libraries. Immediate responses are not guaranteed.
