# RScriptoBox 技术设计方案

版本：0.2.0 实现规格  
日期：2026 年 10 月 15 日  
技术栈：Rust、Tauri 2、Vue 3、TypeScript、SQLite  
配套文档：[产品需求文档](requirements.md)

## 1 关键设计决定

| 决定 | 原因 |
| --- | --- |
| 单个 Tauri 应用，业务能力放在 Rust 后端 | 本地文件、Windows 进程和安装器都应由统一后端控制 |
| Vue 3 加 TypeScript，Vite 构建静态资源 | 前端只负责交互和展示；不引入本地 HTTP 服务或 Node 运行时 |
| 三类解释器适配分别实现 | CMD、PowerShell、Python 的参数、编码和兼容性不同 |
| 两种执行模式共用进程生命周期内核 | 内置终端支持交互与回放，系统控制台保留独立窗口 |
| 每次业务脚本运行一个 Windows Job Object | 控制正常子孙进程，避免只结束直接子进程 |
| SQLite 保存元数据，磁盘文件保存日志和 venv | 事务可靠，避免将大量日志塞进数据库 |
| WinGet 为首版自动安装入口 | 复用现有安装机制，不首版自建通用下载与安装平台 |
| Python Install Manager 安装基础解释器，venv 隔离依赖 | 区分运行时与项目依赖，并允许继续使用用户已有环境 |
| 复制入口脚本到工作台，不自动收集项目 | 删除原文件或移动工作台仍可使用副本；关联文件由用户放入副本目录 |

这些是本项目的设计选择，不代表 Tauri 自带脚本管理、终端、环境安装或进程树回收能力。

## 2 应用结构

~~~mermaid
flowchart TB
  UI["Vue 界面<br/>脚本库、环境中心、任务、历史"]
  IPC["Tauri 业务命令与定向 Channel"]
  Library["脚本与配置"]
  Runner["运行监督器<br/>解释器适配、Win32 进程、Job"]
  Env["环境检测与安装<br/>WinGet、pymanager、venv"]
  Store["SQLite 与磁盘日志"]
  OS["Windows<br/>解释器、控制台、安装器"]
  UI --> IPC
  IPC --> Library
  IPC --> Runner
  IPC --> Env
  Library --> Store
  Runner --> Store
  Env --> Store
  Runner --> OS
  Env --> OS
  Runner -->|"分块输出与状态"| IPC
~~~

采用少量业务模块，不建设插件系统、依赖注入框架、远程服务或语言通用执行引擎。

建议源代码结构：

~~~text
RScriptoBox/
  docs/
    requirements.md
    design.md
  src/
    views/               脚本、环境、任务、历史、设置
    components/          参数编辑、环境选择、日志查看
    api.ts               IPC 类型和业务调用
    stores/              当前任务与页面状态
  src-tauri/
    capabilities/
    permissions/
    migrations/
    src/
      lib.rs             注册命令、初始化状态与窗口
      commands.rs        IPC 边界校验与业务入口
      library.rs         脚本记录和配置
      runner.rs          执行计划、状态监督
      process_windows.rs Win32 进程与 Job 句柄
      runtimes.rs        发现、探测、安装与 venv
      storage.rs         SQLite、日志、备份与清理
      error.rs           结构化错误
  tests/
    fixtures/            最小 cmd、ps1、py 及子进程测试脚本
~~~

文件在实现时按实际体量拆分，不预先增加空目录与空模块。

## 3 依赖与边界

| 能力 | 选择 |
| --- | --- |
| UI | Vue 3、TypeScript、Vite；布局与控件采用一套一致方案 |
| IPC | Tauri commands；运行输出使用 Tauri Channel |
| Windows API | windows crate，仅启用进程、Job、句柄、系统路径等所需特性 |
| 异步监督 | 复用 Tauri 异步运行时；同步管道读取放入阻塞任务 |
| 数据库 | rusqlite；事务、绑定参数、schema migration |
| 序列化 | serde、serde_json |
| 内容指纹 | 成熟 SHA256 实现，用于入口脚本变更检测 |
| 编码 | 能处理 UTF-8、Windows 代码页与 GB18030 的成熟库或平台 API，不手写解码器 |
| 文件选择、打开目录与固定 URL | 使用必要的 Tauri dialog、opener 等官方能力并限制范围；脚本编辑单独调用 edit verb |
| 单实例 | Tauri 官方 single-instance 插件，第二实例唤起主窗口 |
| 包安装 | 外部 WinGet 与官方 Python Install Manager |

不向前端开放通用 shell 插件执行权限，也不使用前端 SQL 插件直接读写数据库。普通进程的参数构造可借助 Rust 标准库规则，但进程创建与 Job 绑定由 Windows 层统一处理。

依赖具体版本在开发启动时选择相互兼容的稳定版本，提交锁文件。运行时配方的产品版本和包标识在发布时验证，不能在设计中把“最新”当作永久稳定规则。

Tauri Channel 面向有序流式数据，比高频全局事件适合运行输出；它仍需要应用自行限流和控制接收窗口。见 [Tauri 前端通信文档](https://v2.tauri.app/develop/calling-frontend/)。

## 4 本地数据

### 4.1 文件布局

通过 Tauri 的 app_local_data_dir 获取应用私有数据根目录，不硬编码用户名称和磁盘。安装标识在首次发布前固定，避免升级后数据根目录变化。

~~~text
<app_local_data_dir>/
  library.db             schema 2
  scripts/
    <import_id>/<entry>   工作台副本，可放置关联文件
  backups/
    <timestamp>-schema<n>.db
  logs/
    <run_id>/
      stdout.bin
      stderr.bin
      chunks.jsonl
  envs/
    <environment_id>/     应用创建的 venv
  install-logs/
    <install_job_id>/
~~~

导入目录名来自后端 UUID，入口文件名取已验证的本地文件名。便携模式在程序旁 data 写配置与副本，普通模式使用 app_local_data_dir。副本路径及其内的工作目录在 SQLite 中使用 scripts/ 相对路径；读取时解析为当前数据根目录的绝对路径。拒绝相对路径中的父目录跳转和根目录越界。原文件保持不变。

### 4.2 数据表

首版将单个脚本的一套默认配置放在 scripts 表，不额外建立多配置系统。

| 表 | 主要字段 |
| --- | --- |
| scripts | id、name、path、file_identity、kind、description、tags_json、favorite、runtime_id、environment_id、cwd、args_json、env_json、run_mode、timeout_seconds、encoding、success_codes_json、trusted_entry_hash、created_at、updated_at |
| runtimes | id、kind、executable_path、version_json、architecture、discovery_source、distribution、status、checked_at |
| environments | id、base_runtime_id、interpreter_path、ownership、requirements_path、requirements_hash、dependency_status、created_at、checked_at |
| runs | id、script_id 可空、script_name_snapshot、script_path_snapshot、entry_hash、plan_redacted_json、mode、state、root_exit_code、reason_code、started_at、finished_at、log_directory、log_truncated |
| install_jobs | id、recipe_key、recipe_revision、target_version、target_arch、scope、state、raw_exit_code、reason_code、started_at、finished_at、log_directory |
| settings | key 主键、value_json |

约束与索引：

- kind、state、mode、ownership 使用受限枚举，在 Rust 和数据库迁移中保持一致。
- 参数、标签、成功码等以 JSON 数组保存，后端验证元素类型与大小；首版不需要独立标签关系表。
- args_json 的参数项允许普通文本或敏感槽位对象；env_json 的值使用相同约定。敏感槽位只保存 ID、位置或键名、提示和必填标记，不保存本次输入值。
- scripts 的去重依据已解析路径和可取得的 Windows 文件身份；使用 Unicode 排序比较接口处理路径大小写，不简单将整个路径转小写。
- 实际文件不存在或无法打开时不生成“可信”文件身份；新增与重新定位时都重新验证。
- runs 在 started_at、script_id 与 started_at 上建索引；删除脚本设置 script_id 为 NULL，保留历史快照。
- 环境和解释器被脚本引用时禁止直接删除绑定记录；先要求重新绑定或解除引用。
- 数据库启用外键和 WAL；设置有限 busy timeout；写入由受控存储入口串行执行。
- 时间统一保存 UTC，显示时转换为本地时间；耗时使用单调时钟，避免修改系统时间干扰超时。
- 路径和日志均为本机数据，不假定跨电脑可用。

### 4.3 配置示例

以下是数据形状示意，版本和 ID 均为占位值，不是可直接执行的配置。

~~~json
{
  "id": "script-id",
  "name": "整理下载目录",
  "kind": "python",
  "path": "D:\\Scripts\\整理文件.py",
  "runtimeId": "runtime-id",
  "environmentId": "venv-id",
  "cwd": "D:\\Scripts",
  "args": ["--source", "D:\\Downloads", ""],
  "env": {"APP_MODE": "local"},
  "runMode": "captured",
  "timeoutSeconds": 600,
  "encoding": "utf-8",
  "successCodes": [0]
}
~~~

示例没有敏感值。需要时用敏感槽位对象代替对应文本值，例如 {"secretSlot":"api-token","required":true}；槽位位置本身保存在参数列表或环境字典中。

本次敏感值通过 run_script 的独立 secret_values 字段提供，只保存在内存。update_script 拒绝槽位中的实际值，运行前检查必填槽位。运行历史只保存脱敏计划，敏感值替换为标记。

### 4.4 备份与恢复

使用 SQLite backup API 获取一致备份，不直接复制处于 WAL 模式的主数据库文件。迁移前生成备份；迁移在事务内完成，失败回滚并提示。

恢复先阻止新任务并处理活动脚本任务；存在未决环境安装时必须等待安装完成并复检，不能通过强杀安装器或恢复旧库绕过变更锁。验证备份 schema 和完整性，再关闭连接切换数据库。不得在恢复失败时覆盖当前库。旧版应用遇到更高 schema 时停止写入并给出版本提示。

备份保留最近 3 份；大体积日志和 venv 不包含在数据库备份中。恢复记录后逐项重新检查脚本与解释器路径。

## 5 脚本登记与执行计划

### 5.1 登记过程

文件选择或拖入 → 后端确认本地普通文件 → 解析实际路径和身份 → 校验扩展名 → 去重 → 保存元数据。

首版允许源文件为正常的本地链接，但必须解析并展示实际目标。原始输入不能作为可信路径再次拼接。UNC、设备路径、嵌入 NUL、ADS 和未受测长路径在边界拒绝。

打开目录使用登记记录中验证后的路径。编辑文件必须调用 Windows 明确的 edit verb；不存在编辑关联时提示配置编辑器并提供打开目录入口，不回退到默认 open，因为脚本默认打开动作可能直接执行。该动作与受控任务运行分开，不开放任意编辑器命令行模板。文件动作见 [Windows ShellExecute verbs](https://learn.microsoft.com/en-us/windows/win32/shell/launch)。

### 5.2 执行前检查

1. 取得数据库中的脚本与绑定环境，不接受前端传来的任意解释器命令。
2. 验证文件仍存在且类型一致，重新计算入口指纹，处理首次或变更确认。
3. 验证解释器绝对路径、真实产品、版本和架构；已失效的绑定不自动换版本。
4. 验证 cwd、参数长度、环境键值、编码、超时和成功码。
5. 检查脚本未重入、并发未达上限、依赖环境没有安装锁。
6. 由对应适配逻辑构造不可变 ExecutionPlan，先写入 starting 记录，再启动进程。

ExecutionPlan 至少包含：脚本 ID 与入口指纹、解释器绝对路径、结构化参数、cwd、环境块、运行模式、输出编码、超时与成功码。

后端与 UI 分别使用真实计划和脱敏计划，命令预览只用于展示。不能把预览文本交回 shell 执行。

## 6 Windows 执行层

### 6.1 解释器规则

表中的命令只说明参数顺序。实现必须构造 Windows 命令行，不能直接复制展示字符串作为执行接口。

| 类型 | 内置终端模式 | 系统控制台模式 |
| --- | --- | --- |
| Python | 绝对 python.exe，参数 -u、脚本绝对路径、用户参数 | 同样直接执行，连接新控制台 |
| PowerShell | 绝对 powershell.exe 或 pwsh.exe，-NoLogo -NoProfile -File、脚本、参数 | 相同，连接新控制台 |
| CMD 或 BAT | 系统 cmd.exe，/D /V:OFF /S /C，加专用适配结果 | 同样执行并连接新控制台，不加 /K |

Python 使用目标 venv 的 python.exe 时不再经过 py 或 pymanager。日志模式设置 PYTHONIOENCODING=utf-8，允许用户显式覆盖；不强制改变脚本文件系统编码或全局系统编码。-u 仅降低 Python 标准输出缓冲，不保证其子程序及时 flush。见 [Python 命令行说明](https://docs.python.org/3/using/cmdline.html)。

PowerShell 的 -NoProfile 使启动可重复；需要 profile 的脚本应显式引入所需依赖。不通过 -Command 拼接用户参数，不能将用户参数解释为 PowerShell 表达式。两代引擎对 -File 的类型绑定存在差异，首版参数定义为文本；不能安全表达的布尔、数组等绑定应提示使用脚本参数约定或配置文件。见 [PowerShell CLI](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_pwsh) 与 [Windows PowerShell CLI](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe)。

源码编码与输出解码是不同问题。Windows PowerShell 5.1 可能将无 BOM 的 UTF-8 中文脚本按系统 ANSI 解释；应提示使用兼容编码或由用户选择已验证的引擎，不能通过切换日志编码修复，也不自动重写源文件。见 [PowerShell 字符编码](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_character_encoding)。

脚本目录不作为解释器搜索目录。系统 CMD 和 Windows PowerShell 通过 Windows 系统目录 API 定位；pwsh 和 Python 使用探测并确认的路径，不仅执行裸命令名。

### 6.2 Windows 参数与 CMD

Windows CreateProcess 接收命令行字符串，而各程序负责解析。Rust 对常规 argv 提供的转义不能泛化为 CMD 的安全保证。见 [Rust Windows 参数说明](https://doc.rust-lang.org/std/process/index.html#windows-argument-splitting)。

普通解释器按受测 Windows argv 规则逐项编码，覆盖空参数、空格、引号和末尾反斜杠；lpApplicationName 使用明确的解释器绝对路径。

CMD 额外处理：

- 使用专门的构造函数；/D 禁用 AutoRun，/V:OFF 禁止延迟展开，/S 与 /C 的引号规则单独验证。
- 首版参数和入口路径的保守字符集合允许中文、字母数字、空格、普通路径分隔符、冒号、点、短横线、下划线等常见字符；空参数单独编码。
- 明确拒绝双引号、百分号、感叹号、&、|、<、>、^、圆括号、CR、LF、其他控制字符，以及不在受测集合中的字符。cwd 也采用受测本地路径范围。
- 这意味着包含括号或 & 的批处理路径可能需要用户移动或重命名；不能默默剥除字符或自动执行一个改写后的路径。
- 超出能力时返回 CMD_ARGUMENT_UNSUPPORTED，指出字段；可建议脚本改为读取配置文件，不能绕过检查再次拼接。
- 不提供“原始命令字符串”高级入口，也不使用 call、start、ShellExecute 或 wt.exe 二次代理执行。

这个限制以首版可验证性为代价缩小输入范围；若实际常用脚本受影响，应先扩展适配器与回归用例，再放开字符。/D、/S、/C 的具体行为见 [CMD 官方说明](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/cmd)。

必须用捕获实际参数和恶意载荷的测试夹具验证“参数一致或明确拒绝”，不能只比较构造出的命令行字符串。

### 6.3 创建进程与 Job

普通 std::process::Child::kill 只负责直接子进程。实现采用少量 Win32 代码，共用以下顺序：

1. 创建不可继承的 Job handle，设置 JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE，不开放 breakaway。
2. 设置解释器、cwd、Unicode 环境块与所需 stdio。
3. CreateProcessW 使用 CREATE_SUSPENDED 创建根进程。
4. AssignProcessToJobObject 成功后 ResumeThread。
5. 绑定失败立即结束仍挂起的进程，关闭资源并记为启动失败，不能失控运行。
6. 仅由后端监督器持有 Job handle；子进程不得继承。

这一顺序消除“进程先运行，再加入 Job”期间快速派生子进程的窗口。句柄使用 RAII 清理。详见 [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects) 与 [AssignProcessToJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject)。

先使用 QueryInformationJobObject 定期查询活动进程数，例如 200 ms 一次；不为首版增加 completion port 复杂度。不假设 WaitForSingleObject(job) 能在正常清空时唤醒。

根进程退出后保存其退出码；Job 中还有子进程时继续保持任务活动。Job 清空后再结束整个任务。子进程失败是否构成业务失败，由根脚本汇总到其退出码，应用不推断任意子进程的业务含义。

用户停止或超时调用 TerminateJobObject，再确认活动进程清空并处理剩余输出。强制停止不保证脚本 finally、文件写入或数据库提交完整完成，UI 应明确表达。

Job 不用于隔离恶意脚本，也不控制外部系统服务代为创建的工作。首版不支持提权启动，不通过 ShellExecute runas 启动后假装还能沿用相同的日志和控制能力。

### 6.4 两种模式的句柄设置

**内置终端模式（terminal）**

- CreatePseudoConsole 建立 UTF-8 双向管道，PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE 绑定子进程；维持挂起创建 → 加入 Job → 恢复的顺序。
- 使用 STARTF_USESTDHANDLES 且三个标准句柄为 null，防止 Win10 将启动器重定向句柄透传到脚本。
- 独立线程写入输入，队列最多 32 块、每块最多 4096 字节；超限明确拒绝。终端输入不写入配置或执行计划。
- 读取输出、调整尺寸和关闭 ConPTY 分属独立线程，监督器继续排空输出，避免 Win10 关闭或重绘时互相等待。
- xterm.js 按需加载，负责光标、颜色、输入法和 VT 展示；FitAddon 通知后端列数与行数。前端输入有界，终端滚动缓冲最多 10,000 行；显示写入串行并合并中间更新。
- 合并 stdout/stderr，历史保留原始 UTF-8/VT 数据；回放禁止输入，活动任务可返回实时交互。普通复制从终端缓冲提取纯文本，完整导出保留 VT。
- 旧版 captured 双管道内核保留，用于兼容测试与历史；升级时脚本的默认 captured 配置改为 terminal，不改变旧历史记录。

依据：[ConPTY 会话生命周期](https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session)、[微软关于重定向句柄继承的说明](https://github.com/microsoft/terminal/discussions/15814)、[xterm.js 插件](https://xtermjs.org/docs/guides/using-addons/)。

**系统控制台模式**

- 使用 CREATE_NEW_CONSOLE，不设置 STARTF_USESTDHANDLES，不重定向 stdio；由 Windows 为新控制台设置输入输出。
- 不与 CREATE_NO_WINDOW、DETACHED_PROCESS 混用，不通过 Windows Terminal 的外部代理启动。
- PowerShell 不加 -NonInteractive；所有类型均不附加 pause、/K 或 -NoExit。
- 共用 Job、超时和退出码记录；不采集输出，不提供日志回放。
- 用户在控制台按 Ctrl+C 是控制台行为；应用的停止按钮仍是强制终止。

创建标志与标准句柄行为见 [Process Creation Flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags) 和 [STARTUPINFOW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-startupinfow)。Windows 默认终端宿主和传统控制台都必须实测。

### 6.5 运行状态

~~~mermaid
stateDiagram-v2
  [*] --> starting
  starting --> running: 创建并绑定 Job
  starting --> spawn_failed: 创建或绑定失败
  starting --> stopping: 已接受停止
  running --> stopping: 停止或超时
  running --> succeeded: Job 清空且根退出码成功
  running --> failed: Job 清空且根退出码失败
  stopping --> cancelled: 用户停止完成
  stopping --> timed_out: 超时停止完成
  stopping --> failed: 采集或存储失败
~~~

应用重启时，将遗留 starting、running、stopping 记录置为 interrupted。操作系统强制退出后 Job 会清理其受控进程，但最后一段日志可能尚未落盘。

运行原因独立于退出码保存。由一个监督任务串行决定终态：正常完成先成立则后续停止无效；停止已被接受后，不能因为根进程恰好返回 0 改报成功。

停止失败或系统迟迟未结束任务时继续显示 stopping 和诊断，不伪造 cancelled。任务树已空但日志管道仍被外部进程持有时设置有限排空期限，随后关闭读取并标记采集未完整结束。

## 7 日志链路与编码

### 7.1 数据流

两个字节读取任务 → 有界采集队列 → 原始日志与索引写入 → 独立流增量解码 → 批量 Channel 推送 → 有界文本视图。

原始日志为 stdout.bin 与 stderr.bin；chunks.jsonl 记录 seq、stream、offset、length、observed_at。它记录宿主接收顺序，不能还原两个操作系统管道的绝对先后。

每个流维护独立解码状态，处理跨块字符。回放时按索引重新读取字节并用用户选择的编码解码。索引尾部不完整或越过实际文件长度时停止并标注截断，不能崩溃。

不自动用“这块字节能解 UTF-8”判断整个流编码。Python 预设 UTF-8；PowerShell 7 先采用 UTF-8；CMD 与 Windows PowerShell 5.1 提供系统代码页预设，实际输出仍可能随脚本和外部程序变化。混合编码无法保证自动正确识别，应允许人工切换。

### 7.2 流控与配额

- 建议读取块 8 KiB；UI 推送按 50 ms 或累计 32 KiB 触发，发布前根据负载测试调整。
- 采集队列、待发送 UI 数据、UI 文本均有上限；不能为每个字节或每一行发送 IPC。
- UI 跟不上时跳过部分实时展示并发送 gap 标记，可从已持久化日志回放；不因 UI 挂起无限积压。
- 日志配额包含两个原始文件与索引。达到单次配额后写入可辨认截断状态，继续排空管道，防止子进程写阻塞。
- 达到全局配额先清理已完成历史日志；无可清理目录时停止新增持久化并标记截断，继续排空。并发写入由存储入口统一计费，不能每个任务各自假设仍有剩余空间。
- 安装诊断日志纳入相同总配额与保留期，也设单次 20 MiB 上限。其写盘失败只中止诊断保存并报告，不能为日志故障强杀外部安装器。
- 清理只作用于受控日志根目录下由后端生成的任务目录。
- 原始日志写入失败触发任务停止；排空过程中不再依赖失败的写入队列，避免关停死锁。
- 不在持有数据库互斥锁时等待管道、进程或安装器。

### 7.3 文本展示

前端按文本节点渲染。首版不实现完整 ANSI 终端；忽略具有系统副作用的 OSC 等序列，正常文本仍可复制。对 CR 使用简单进度显示或保留可辨认换行，不承诺 TUI 布局。

日志导出只允许用户选择目标文件，导出的内容是文本或原始日志归档。敏感输入的元数据会脱敏，但原始 stdout 与 stderr 可能包含脚本主动输出的秘密，不能承诺完整脱敏。

## 8 环境发现与验证

### 8.1 发现来源

| 类型 | 候选来源 | 验证方式 |
| --- | --- | --- |
| CMD | Windows 系统目录 API | 存在、架构、启动短探针 |
| Windows PowerShell | 系统目录对应的 WindowsPowerShell 路径 | -NoProfile 短探针读取版本与引擎信息 |
| PowerShell 7 | 登记路径、已知应用注册信息、PATH 候选 | 运行版本探针，确认实际是受支持 pwsh |
| Python | 已登记路径、Python Install Manager JSON 列表、PEP 514 注册项、PATH 候选、用户选择 | 指定解释器运行隔离短探针，读取 executable、version、architecture、prefix、base_prefix |
| 已有 venv | 用户选择其 python.exe | 检查运行能力与 prefix；不接管文件所有权 |

发现和验证分开：不因名称类似就信任二进制。检测使用明确路径、固定探针、输出大小上限与约 5 秒超时，不执行用户脚本。

Python 探针可使用 -I -c 加应用内固定代码，避免当前脚本目录污染探测。实际运行业务脚本不默认使用 -I，以保留正常项目导入。

不执行 WindowsApps 中可能拉起商店的普通 python 别名来判断是否安装；优先读取管理器列表和真实解释器路径。手动选择的可执行文件也是本地代码，应在选择时明确其用途。

环境安装或用户刷新后重新发现并验证，不仅重新读取现有进程的 PATH。缺失运行时不会在点击业务脚本时隐式下载。

### 8.2 绑定与默认环境

每个脚本保存明确 runtime_id；Python 可另存 environment_id，实际执行以该环境的解释器为准。系统默认环境只帮助新脚本选择，不改变已有绑定。

运行前发现版本或架构变化时重新验证，并在兼容性超出绑定范围时要求重新选择。PS 5.1 与 PS 7 不自动互换。

## 9 环境安装

### 9.1 首版安装配方

使用应用内少量受测配方，不实现用户自定义安装脚本。每条配方保存产品标识、来源、包 ID、目标架构、版本策略、支持的安装范围、发现方法与探针。

| 配方 | 步骤 |
| --- | --- |
| PowerShell 7 | 验证 WinGet → 查询受测 Microsoft.PowerShell 包 → 展示安装计划 → 安装 → 发现实际 pwsh 路径并探测 |
| Python | 验证 WinGet → 安装或发现官方 Install Manager → 发现 pymanager → 安装选定受测版本标签 → 取得实际 python.exe 并探测 |
| CMD 与 Windows PowerShell 5.1 | 仅检测系统组件，缺失时提示系统修复或支持边界 |

包 ID、来源与版本需要在每次发布测试时核对。当前 Python 官方文档给出的管理器 Store 标识为 9NQ7512CXL7T；它只能作为实现时复核的起点，不能把包 ID 当作无须维护的永久接口。Python 自动化与列表能力见 [Python Windows 指南](https://docs.python.org/3/using/windows.html)。

PowerShell 的安装格式和目录会随发行机制变化，不能写死“总是 MSI”或“总在 Program Files 下”。安装方法见 [微软 PowerShell 安装说明](https://learn.microsoft.com/en-us/powershell/scripting/install/install-powershell-on-windows)。

### 9.2 WinGet 调用规则

1. 定位并验证 winget 可启动；安装前执行只读查询，确认配方目标仍适用。
2. 使用明确包 ID、--exact 和明确 --source；可用时限定受测版本、架构。
3. 优先用户范围，仅在安装器支持时使用 --scope user；需要机器范围时先在计划中显示。
4. 显示协议与来源，用户接受后才传相应协议接受参数。--silent 不解释为免 UAC。
5. 不传 --ignore-security-hash、绕过恶意软件检查、盲目 --force 或自动重启参数。
6. 保存 WinGet 与安装器诊断；退出码保留原始 32 位值，可同时展示十六进制。
7. 退出后重新验证实际解释器，复检结果决定是否就绪。

安装参数依据 [WinGet install](https://learn.microsoft.com/en-us/windows/package-manager/winget/install)；错误映射依据 [WinGet 故障诊断](https://learn.microsoft.com/en-us/windows/package-manager/winget/troubleshooting) 和 [官方返回码表](https://github.com/microsoft/winget-cli/blob/master/doc/windows/package-manager/winget/returnCodes.md)。

不解析本地化进度文字来生成精确百分比；只有真实下载进度可用时才显示百分比，其余显示阶段与原始日志。

### 9.3 安装状态与取消

状态集合：planning、awaiting_confirmation、installing、verifying、ready、failed、cancelled、reboot_required、verification_required。

外部安装器可能另起进程、请求 UAC 或继续后台安装，不能直接套用脚本 Job 的“应用退出即强杀”策略。启动前可取消；启动后只有安装器明确支持时协作取消，否则保留活动状态或标记 verification_required。

应用关闭时展示仍在进行的安装，允许返回等待；用户选择退出后说明安装可能继续。下次启动对未完成记录复检。只有安装器确认取消且验证没有目标改动时，才记录 cancelled；不自动卸载部分安装。

全局环境变更锁只允许一个安装或依赖变更任务。该锁由 install_jobs 中的未决记录恢复，不仅存在内存中；仅凭重启或旧 PID 不存在不能认定外部安装已结束。无法自动确认时保持 verification_required，由用户确认安装器已关闭并完成复检后解除。首版不能修改正在运行脚本绑定的目标环境。

### 9.4 无 WinGet 与受限网络

P0 提供打开官方产品下载页和手动登记解释器。不得自动修改安装源、TLS 配置、系统代理或策略。

P1 若增加官方下载后直接安装，需同时实现 HTTPS 与来源白名单、安装包签名和发布者验证、可信校验和、架构检查、缓存校验、断点恢复策略与失败清理。未具备这些能力前不提供任意 URL 安装器。

离线版需打包对应运行时及其依赖，独立验证许可、签名、目标 Windows 版本和 Python 管理器离线索引；“已安装 WinGet”不代表可离线安装产品。

## 10 Python venv 与依赖

创建流程：

1. 选择已验证基础解释器，生成新的 environment_id。
2. 在最终环境目录执行该解释器 -m venv，创建期间状态为 creating，暂不绑定。
3. 直接验证 envs/<id>/Scripts/python.exe 的版本、prefix、pip 与基本能力。
4. 用户需要依赖时，用该解释器 -I -m pip --isolated --require-virtualenv --no-input install -r 加用户选择的 requirements 路径。
5. 依赖安装 cwd 设为 requirements 所在目录，保留相对本地依赖语义。
6. 记录依赖文件指纹与结果，成功后在事务中绑定脚本。

venv 不采用“临时路径创建再重命名”的发布方式，因为其中的入口脚本可能绑定绝对路径。每次重建使用新的最终 ID，验证成功才切换绑定，旧环境在未引用且无活动任务后显式清理。见 [venv 的不可移动性说明](https://docs.python.org/3/library/venv.html)。

依赖安装子进程清理继承的 PIP_*，再将 PIP_CONFIG_FILE 设为目标 Python 的 os.devnull 值，Windows 通常为小写 nul。--isolated 单独仍可能读取系统或环境级 pip 配置；禁用配置文件可防止 target、prefix、user 等改变安装目标。私有源、代理和证书仅通过产品支持的明确白名单设置传入，不继承任意 pip.ini。实现时对受支持 pip 版本做安装目标回归，依据 [pip 配置文档](https://pip.pypa.io/en/stable/topics/configuration/) 与 [官方配置加载实现](https://github.com/pypa/pip/blob/26.2.1/src/pip/_internal/configuration.py)。

这些设置约束 pip 的安装目的地，不隔离不可信包的构建代码；依赖安装仍属于用户授权的本地代码执行。

失败的应用自建环境可重建，但不对用户已有环境自动执行删除。已有 venv 内依赖更新失败可能留下部分变更，应标记 dependency_status=failed 或 unknown，用户明确修复后再允许运行。

requirements 内容改变后标记“依赖清单已变更”；运行不会自动 pip install。已安装包版本可记录为诊断快照，但不宣称未锁版本的 requirements 能完全复现环境。

不把 Python embeddable distribution 当通用 pip 与 venv 安装方案。Conda 或其他环境首版只通过实际 python.exe 使用，不负责求解和创建。

## 11 IPC 接口

所有 command 输入采用 serde 结构体，设置字段数量、字符串和数组长度上限，拒绝未知类型与不支持值。路径类操作从已登记 ID 或用户明确选择解析，不开放任意 filesystem 或 execute 接口。

| 命令 | 主要输入 | 输出 |
| --- | --- | --- |
| list_scripts | 搜索、类型、标签、分页 | 脚本摘要与环境状态 |
| add_scripts | 文件选择产生的路径列表 | 成功、重复和失败逐项结果 |
| update_script | script_id、元数据与默认配置 | 更新记录 |
| remove_script | script_id | 移除记录结果，不删除源文件 |
| relocate_script | script_id、新文件选择 | 重新验证后的记录 |
| open_script_location / edit_script | script_id | 系统打开结果 |
| run_script | script_id、单次覆盖、secret_values、事件 Channel | run_id |
| stop_run | run_id | 已接受或当前终态 |
| list_runs / get_run | 条件分页或 run_id | 状态与脱敏计划 |
| read_run_log | run_id、索引游标、编码、字节上限 | 文本块与后续游标 |
| list_runtimes / refresh_runtimes | 类型 | 发现和验证结果 |
| register_runtime | 用户选择的解释器 | 已验证环境记录 |
| plan_runtime_install | 配方 key、受支持版本选项 | 安装计划 ID 与许可信息 |
| start_runtime_install | 已确认计划 ID、Channel | install_job_id |
| cancel_runtime_install | install_job_id | 取消或待复检状态 |
| create_python_environment | script_id、base_runtime_id | 环境任务 ID |
| install_requirements | environment_id、用户选择的清单 | 依赖任务 ID |
| get_settings / update_settings | 受限设置字段 | 设置 |
| backup_database / restore_database | 用户选择的备份目标或来源 | 结果与诊断 |

输出事件形状：

~~~typescript
type RunEvent =
  | { type: "state"; runId: string; state: string; reason?: string }
  | { type: "output"; runId: string; seq: number;
      stream: "stdout" | "stderr"; text: string }
  | { type: "gap"; runId: string; nextSeq: number }
  | { type: "log_truncated"; runId: string }
  | { type: "finished"; runId: string; state: string;
      rootExitCode?: number };

type AppError = {
  code: string;
  message: string;
  retryable: boolean;
  details?: Record<string, unknown>;
};
~~~

生产实现使用明确枚举代替示意中的任意 state 字符串。Channel 在启动请求时传入，避免先启动再订阅丢失最早输出；前端按 runId 缓存可能早于 invoke 返回的事件。

Channel 中断不改变脚本执行结果。用户重新打开任务页时先取状态，再按游标读日志；不必为首版另建通用消息订阅平台。

典型错误码：SCRIPT_MISSING、SCRIPT_CHANGED、RUNTIME_MISSING、RUNTIME_UNSUPPORTED、INVALID_CWD、INVALID_ARGUMENT、CMD_ARGUMENT_UNSUPPORTED、CONCURRENCY_LIMIT、ENVIRONMENT_BUSY、EXECUTION_POLICY_BLOCKED、JOB_ASSIGN_FAILED、LOG_WRITE_FAILED、INSTALL_SOURCE_UNAVAILABLE、INSTALL_REBOOT_REQUIRED、INSTALL_VERIFICATION_REQUIRED。

只有明确识别出的错误才使用专用分类；未知退出保留原始错误，不仅根据本地化输出片段推断策略或权限原因。

## 12 安全与授权

### 12.1 Tauri 边界

- 主界面只加载随应用打包的资源，不加载远端应用页面，不授予 remote capability。
- 只为主窗口列出所需 permissions；不向 WebView 授予通用 shell、任意文件系统和任意窗口创建权限。
- 将自定义 commands 通过 AppManifest::commands 纳入 ACL，再配置主窗口权限。不能认为 invoke_handler 注册后天然已受 capabilities 限制。
- 后端仍需校验业务 ID、路径、配置和状态；capabilities 不能修复后端任意命令执行入口。
- 配置生产 CSP，避免远端脚本、eval 和危险 HTML 注入；脚本内容、日志与异常都按纯文本显示。
- URL 打开只来自固定官方页面或经用户明确触发的受限 HTTPS 地址，不能由日志自动触发。

Tauri 自定义命令默认可被应用窗口调用，以及权限系统不能保护错误 Rust 逻辑的边界，见 [Tauri Capabilities](https://v2.tauri.app/security/capabilities/)。

### 12.2 脚本信任与权限

首次执行显示源路径、类型、引擎和权限；只有用户确认才允许启动。记住确认以入口文件指纹绑定，变更后失效。

这只是误操作和变更提示。未锁定依赖文件、运行时文件及整个目录，不宣称能抵御同权限恶意进程在检查与执行之间修改项目。强隔离需要另行设计，不属于首版。

当前用户权限意味着脚本能访问该用户本来能访问的文件和网络。应用不扫描并承诺识别恶意脚本，不以 Rust 内存安全或 Tauri capabilities 宣称脚本安全。

普通发行版启动时若检测到应用已被用户提升权限，显示明显权限状态，避免用户误以为仍是普通权限；应用不会主动长期提权。

### 12.3 秘密与删除边界

敏感参数默认不保存；普通环境变量可以保存，并明确提示本地数据库不是秘密保险箱。脚本通过命令行接收的秘密可能被本机其他有权限的进程观察，优先建议脚本使用合适的环境变量或其自身凭据机制。

自动清理只删除应用拥有的日志、备份和未引用 venv。清理前核对规范路径、所有权和目录范围，拒绝越界、重解析点跳转或不确定目标。不得递归删除源脚本目录或用户登记的外部环境。

## 13 发布与更新

首版在 Windows MSVC 工具链构建，按本次交付要求输出 x64 独立 EXE 与便携 ZIP，目标为 Windows 10 22H2 和 Windows 11。portable.flag 或 --portable 指定 EXE 旁的 data 为业务数据目录。保留的 NSIS 配置是可选发行方式，不是本次必交产物。开发机需要 Rust 工具链、Tauri 的 Windows 构建依赖和前端构建工具；最终用户不需要安装 Rust、Node 或 Visual Studio。

应用自身需要 WebView2。在线安装包检测缺失时使用 Tauri 支持的 bootstrapper；P1 提供包含 WebView2 安装器的离线包。它与 Python、PowerShell 环境安装是三个独立问题。发行配置依据 [Tauri Windows Installer](https://v2.tauri.app/distribute/windows-installer/)。

用户已明确无需数字签名，本次交付未签名便携构建，Windows 可能显示未知发布者。干净系统与两个目标平台的验证要求仍保留；Windows 11 由用户后续自行验证，不阻塞本次交付。首版手动替换程序升级，不自动修改运行时、不删除原始脚本和用户数据。

未来加入自动更新时还需 Tauri 更新签名、受控发布渠道、回滚与迁移兼容；Authenticode 和更新包签名不能互相代替。

## 14 验证策略

### 14.1 单元与边界测试

重点测试新增非平凡逻辑：参数编码、CMD 拒绝规则、配置验证、状态终结竞争、配额判断、增量解码、迁移与备份恢复。

参数测试使用输入到实际 argv 的往返结果；CMD 注入用例包含 %PATH%、!VAR!、&、|、<、>、^、引号、括号、CR 与 LF，断言无额外标记文件或进程产生。

### 14.2 Windows 集成测试

最小夹具包含：

- 四类脚本的参数、cwd、环境与退出码回显。
- 同时持续输出 stdout 与 stderr、无换行巨块、UTF-8 跨块、系统编码与 CR。
- input、Read-Host、pause 交互脚本。
- 根进程快速派生子孙并提前退出、子进程长期运行、立即停止、超时竞争。
- 非零退出码、stderr 但返回 0、PowerShell 非终止错误。
- 中文和空格目录、被拒绝的 CMD 路径、脚本变更与解释器被卸载。

### 14.3 干净系统与故障测试

使用 Windows 10 22H2 与 Windows 11 x64 虚拟机快照分别测试：已有或缺失 WebView2、PS 5.1 与 7 共存、Python 多版本与旧 py、无 WinGet、网络不可达、UAC 拒绝、企业策略、磁盘已满、安装取消与重启恢复。

安装测试会修改系统，放在专用 VM，不作为每次单元测试自动执行。关闭控制台、Ctrl+C、默认 Windows Terminal 与传统控制台宿主需要真实 UI 验收。

CI 执行格式检查、Rust 静态检查与测试、前端类型检查和构建、Windows 集成夹具。安装与兼容性验收在正式发布前完成并记录具体 Windows、运行时和安装配方版本；本次交付无需数字签名。

## 15 技术验证门槛与剩余风险

开始完整 UI 开发前必须拿到以下结果：

1. CMD 受测字符范围内参数正确，范围外启动前拒绝，无注入。
2. 进程创建时即加入 Job，立即派生、父先退出和应用崩溃场景没有受控进程残留。
3. 两种模式均可运行四类脚本，控制台的交互与关闭行为明确。
4. 两个管道大量输出时不死锁，UI 停止接收也不无限堆内存。
5. 干净系统完成 Python 管理器、指定 Python 和 PowerShell 安装复检，取消或拒绝 UAC 能正确恢复。

最主要的长期风险是企业策略与安装源变化、第三方脚本依赖不可复现、不同控制台与编码行为，以及脚本自身的数据副作用。它们需要可诊断失败、受测兼容矩阵和持续维护，不能靠增加一个通用 shell 调用接口解决。

当前实现与实测范围见 [验证记录](verification.md)。执行器与便携构建已在 Windows 10 上验证，Windows 11、真实系统安装故障矩阵与完整性能指标仍待验收；未完成的目标不能作为已验证的发布承诺。

## 0.2.0 工作台迁移与界面

导入先在 data/scripts 下创建 UUID 目录，以 create_new 写入入口副本；验证 32 MiB 上限和复制前后 SHA256 后才保存记录。同名脚本不覆盖，原始路径用于来源与重复检测。重新导入创建新副本并撤销信任，保留旧文件。副本为相对路径存储，API 返回当前根目录解析后的绝对路径。

schema 1 升级前保存 before-workspace 数据库备份，复制可用的外部入口并切换记录，默认工作目录改为副本目录；失败保留原记录，但运行及编辑要求有效工作台副本。升级与恢复完成后记录 schema 2，旧程序拒绝打开更新数据库。复制整目录才能携带脚本，单独数据库备份不能替代完整迁移；venv、绝对路径依赖与目标机解释器须重建或配置。

暗色主题替换原浅色样式；正文 16px、次要信息 14px，使用 rem 布局随字号缩放。字号设置保存在本机 WebView 偏好中，避免不同 DPI 设备共享不合适的显示设置。默认列表、收紧留白、取消装饰文案；侧栏在窄窗口折叠为图标，抽屉正文独立滚动并固定底部操作。
