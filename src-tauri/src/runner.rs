use crate::{
    models::*,
    process_windows::{self, ReadPipe, SpawnSpec},
    storage::Store,
};
use encoding_rs::{CoderResult, Decoder, Encoding};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::ipc::Channel;
use windows_sys::Win32::Globalization::{GetCPInfo, GetOEMCP, MultiByteToWideChar, CPINFO};

const BLOCK: usize = 8192;
const UI_WINDOW: usize = 512 * 1024;
const MIB: u64 = 1024 * 1024;

struct Active {
    script_id: String,
    stop: Arc<AtomicBool>,
    ack: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
    input: Option<SyncSender<Vec<u8>>>,
    size: Arc<AtomicU64>,
}
struct TerminalControl {
    input: mpsc::Receiver<Vec<u8>>,
    size: Arc<AtomicU64>,
}
struct Inner {
    store: Arc<Store>,
    active: Mutex<HashMap<String, Active>>,
}
#[derive(Clone)]
pub struct Runner {
    inner: Arc<Inner>,
}

pub fn entry_hash(path: &Path) -> AppResult<String> {
    crate::library::hash_file(path)
}

pub fn validate_local_path(path: &Path, directory: bool) -> AppResult<PathBuf> {
    let text = path.to_str().ok_or("路径不是有效 Unicode")?;
    if text.contains('\0')
        || text.starts_with("\\\\")
        || text.starts_with("//")
        || text.encode_utf16().count() > 240
        || !path.is_absolute()
    {
        return Err("仅支持 240 字符以内的本地磁盘绝对路径，不支持 UNC 或设备路径".into());
    }
    if text.get(2..).is_some_and(|tail| tail.contains(':'))
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err("路径不能包含 ADS 或父目录跳转".into());
    }
    let canonical = fs::canonicalize(path).map_err(|e| format!("路径不可用 {text}: {e}"))?;
    let resolved = canonical.to_string_lossy();
    let resolved = resolved.strip_prefix("\\\\?\\").unwrap_or(&resolved);
    if resolved.starts_with("UNC\\") || resolved.encode_utf16().count() > 240 {
        return Err("解析后的路径不在支持的本地路径范围内".into());
    }
    let result = PathBuf::from(resolved);
    if if directory {
        !result.is_dir()
    } else {
        !result.is_file()
    } {
        return Err(format!("路径类型不正确: {}", result.display()));
    }
    Ok(result)
}

fn resolve(value: &InputValue, secrets: &BTreeMap<String, String>) -> AppResult<String> {
    let text = match value {
        InputValue::Text(text) => text.clone(),
        InputValue::Secret(slot) => match secrets.get(&slot.secret_slot) {
            Some(value) => value.clone(),
            None if !slot.required => String::new(),
            None => return Err(format!("缺少敏感输入: {}", slot.secret_slot)),
        },
    };
    if text.contains('\0') || text.len() > 32768 {
        return Err("参数或环境值包含 NUL 或超过长度限制".into());
    }
    Ok(text)
}

fn plan(
    script: &Script,
    runtime: &Runtime,
    secrets: &BTreeMap<String, String>,
) -> AppResult<SpawnSpec> {
    let source = validate_local_path(Path::new(&script.path), false)?;
    let current_hash = entry_hash(&source)?;
    if script.trusted_hash.as_deref() != Some(current_hash.as_str()) {
        return Err("SCRIPT_CHANGED: 脚本首次运行或内容已变更，请重新确认".into());
    }
    let executable = validate_local_path(Path::new(&runtime.executable_path), false)?;
    if !matches!(runtime.status.as_str(), "ready" | "verified" | "available") {
        return Err("RUNTIME_MISSING: 请先复检绑定的执行环境".into());
    }
    let cwd = crate::library::working_directory(&source, &script.cwd)?;
    if script.args.len() > 256 || script.env.len() > 128 || secrets.len() > 384 {
        return Err("参数、环境变量或敏感输入数量超过限制".into());
    }
    if !matches!(
        script.run_mode.as_str(),
        "captured" | "terminal" | "console"
    ) {
        return Err("不支持的运行模式".into());
    }
    if let Some(policy) = &script.execution_policy {
        if !matches!(script.kind.as_str(), "powershell" | "ps1") {
            return Err("仅 PowerShell 脚本可设置本次进程执行策略".into());
        }
        if !matches!(policy.as_str(), "RemoteSigned" | "Bypass") {
            return Err("本次进程执行策略仅支持 RemoteSigned 或 Bypass".into());
        }
    }
    if script.timeout_seconds == Some(0)
        || script.timeout_seconds.is_some_and(|v| v > 31_536_000)
        || script.success_codes.is_empty()
        || script.success_codes.len() > 256
    {
        return Err("超时或成功退出码配置不合法".into());
    }
    TextDecoder::new(&script.encoding)?;
    let args: Vec<_> = script
        .args
        .iter()
        .map(|v| resolve(v, secrets))
        .collect::<AppResult<_>>()?;
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    let python = matches!(script.kind.as_str(), "python" | "py");
    if python && script.run_mode == "captured" {
        set_env(&mut environment, "PYTHONIOENCODING", "utf-8");
    }
    for (key, value) in &script.env {
        if key.is_empty() || key.len() > 256 || key.contains(['=', '\0']) {
            return Err("环境变量键不能为空或包含 = / NUL".into());
        }
        set_env(&mut environment, key, &resolve(value, secrets)?);
    }
    let mut arguments = Vec::new();
    let mut cmd_payload = None;
    match script.kind.as_str() {
        "cmd" | "bat" => {
            if runtime.kind != "cmd" {
                return Err("CMD 脚本必须绑定系统 CMD".into());
            }
            cmd_payload = Some(process_windows::cmd_payload(&source, &args, &cwd)?);
        }
        "powershell" | "ps1" => {
            if !matches!(
                runtime.kind.as_str(),
                "windows_powershell" | "powershell7" | "powershell" | "pwsh" | "powershell51"
            ) {
                return Err("PowerShell 脚本必须绑定明确的 PowerShell 引擎".into());
            }
            // A host launched from another PowerShell engine can inherit incompatible built-in modules.
            let mut module_path = executable
                .parent()
                .ok_or("PowerShell 路径缺少父目录")?
                .join("Modules")
                .into_os_string();
            if let Some((_, inherited)) = environment
                .iter()
                .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PSModulePath"))
            {
                if !inherited.is_empty() {
                    module_path.push(";");
                    module_path.push(inherited);
                }
            }
            set_env(&mut environment, "PSModulePath", module_path);
            arguments.extend(["-NoLogo".into(), "-NoProfile".into()]);
            if script.run_mode == "captured" {
                arguments.push("-NonInteractive".into());
            }
            if let Some(policy) = &script.execution_policy {
                arguments.extend(["-ExecutionPolicy".into(), policy.clone()]);
            }
            arguments.extend(["-File".into(), source.to_string_lossy().into_owned()]);
            arguments.extend(args);
        }
        "python" | "py" => {
            if runtime.kind != "python" {
                return Err("Python 脚本必须绑定 Python 解释器".into());
            }
            arguments.extend(["-u".into(), source.to_string_lossy().into_owned()]);
            arguments.extend(args);
        }
        _ => return Err("不支持的脚本类型".into()),
    }
    let extension = source
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !match script.kind.as_str() {
        "cmd" | "bat" => matches!(extension.as_str(), "cmd" | "bat"),
        "powershell" | "ps1" => extension == "ps1",
        "python" | "py" => extension == "py",
        _ => false,
    } {
        return Err("SCRIPT_CHANGED: 文件扩展名与登记类型不匹配".into());
    }
    Ok(SpawnSpec {
        executable,
        arguments,
        cwd,
        environment,
        console: script.run_mode == "console",
        terminal: script.run_mode == "terminal",
        cmd_payload,
    })
}

fn set_env(
    environment: &mut BTreeMap<OsString, OsString>,
    key: &str,
    value: impl AsRef<std::ffi::OsStr>,
) {
    let old: Vec<_> = environment
        .keys()
        .filter(|existing| existing.to_string_lossy().eq_ignore_ascii_case(key))
        .cloned()
        .collect();
    for name in old {
        environment.remove(&name);
    }
    environment.insert(key.into(), value.as_ref().to_os_string());
}

impl Runner {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                active: Mutex::new(HashMap::new()),
            }),
        }
    }
    pub fn active_count(&self) -> usize {
        self.inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }
    pub fn is_script_active(&self, script_id: &str) -> bool {
        self.inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|a| a.script_id == script_id)
    }
    pub fn start(
        &self,
        script: Script,
        runtime: Runtime,
        settings: Settings,
        secrets: BTreeMap<String, String>,
        channel: Option<Channel<RunEvent>>,
    ) -> AppResult<String> {
        if !(1..=4).contains(&settings.max_concurrent) {
            return Err("并发限制必须是 1 至 4".into());
        }
        let spec = plan(&script, &runtime, &secrets)?;
        let id = new_id();
        let stop = Arc::new(AtomicBool::new(false));
        let ack = Arc::new(AtomicU64::new(0));
        let sent = Arc::new(AtomicU64::new(0));
        let (input_tx, input_rx) = mpsc::sync_channel(32);
        let size = Arc::new(AtomicU64::new(0));
        let control = TerminalControl {
            input: input_rx,
            size: size.clone(),
        };
        let mut active = self.inner.active.lock().unwrap_or_else(|p| p.into_inner());
        if active.values().any(|a| a.script_id == script.id) {
            return Err("该脚本正在运行，不能重复启动".into());
        }
        if active.len() >= settings.max_concurrent as usize {
            return Err("已达到并发上限，请等待任务结束".into());
        }
        let run = Run {
            id: id.clone(),
            script_id: Some(script.id.clone()),
            script_name: script.name.clone(),
            script_path: script.path.clone(),
            runtime_path: runtime.executable_path.clone(),
            run_mode: script.run_mode.clone(),
            state: "starting".into(),
            reason: None,
            root_exit_code: None,
            started_at: now(),
            finished_at: None,
            log_truncated: false,
            plan_redacted: serde_json::json!({"args": script.args, "env": script.env, "cwd": spec.cwd, "timeoutSeconds": script.timeout_seconds, "encoding": if spec.terminal { "utf-8" } else { &script.encoding }, "successCodes": script.success_codes, "entryHash": script.trusted_hash, "executionPolicy": script.execution_policy}),
        };
        self.inner.store.put("runs", &id, &run)?;
        active.insert(
            id.clone(),
            Active {
                script_id: script.id.clone(),
                stop: stop.clone(),
                ack: ack.clone(),
                sent: sent.clone(),
                input: if spec.terminal { Some(input_tx) } else { None },
                size,
            },
        );
        drop(active);
        let inner = self.inner.clone();
        let thread_id = id.clone();
        let failure_run = run.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("run-{}", &id[..8]))
            .spawn(move || {
                let mut run = run;
                let mut events = Events::new(channel, ack, sent);
                events.state(&run);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute(
                        &inner,
                        &script,
                        &settings,
                        (spec, control),
                        &stop,
                        &mut run,
                        &mut events,
                    )
                }));
                match result {
                    Ok(Ok(())) => (),
                    Ok(Err(reason)) => {
                        run.state = if run.state == "starting" {
                            "spawn_failed"
                        } else {
                            "failed"
                        }
                        .into();
                        run.reason = Some(reason);
                    }
                    Err(_) => {
                        run.state = "failed".into();
                        run.reason = Some("执行监督器异常，任务已停止".into());
                    }
                }
                run.finished_at = Some(now());
                // Removing under the same mutex that accepts stop establishes exactly one terminal decision.
                let mut active = inner.active.lock().unwrap_or_else(|p| p.into_inner());
                if stop.load(Ordering::Acquire)
                    && matches!(run.state.as_str(), "succeeded" | "failed")
                    && run.reason.is_none()
                {
                    run.state = "cancelled".into();
                    run.reason = Some("用户强制停止".into());
                }
                active.remove(&thread_id);
                drop(active);
                if let Err(error) = inner.store.put("runs", &thread_id, &run) {
                    run.state = "failed".into();
                    run.reason = Some(format!("保存运行结果失败: {error}"));
                }
                events.state(&run);
                events.send(RunEvent::Finished {
                    run_id: thread_id,
                    state: run.state.clone(),
                    root_exit_code: run.root_exit_code,
                });
            })
        {
            self.inner
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            let mut run = failure_run;
            run.state = "spawn_failed".into();
            run.reason = Some(error.to_string());
            run.finished_at = Some(now());
            self.inner.store.put("runs", &id, &run)?;
            return Err(format!("创建执行线程失败: {error}"));
        }
        Ok(id)
    }
    pub fn stop(&self, id: &str) -> AppResult<()> {
        if let Some(active) = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
        {
            active.stop.store(true, Ordering::Release);
            return Ok(());
        }
        let _: Run = self.inner.store.get("runs", id)?.ok_or("运行记录不存在")?;
        Ok(())
    }
    pub fn input(&self, id: &str, text: &str) -> AppResult<()> {
        if text.is_empty() || text.len() > 4096 {
            return Err("终端输入每次须为 1 至 4096 字节".into());
        }
        let active = self.inner.active.lock().map_err(|e| e.to_string())?;
        let task = active.get(id).ok_or("任务已结束")?;
        if task.stop.load(Ordering::Acquire) {
            return Err("任务正在停止".into());
        }
        task.input
            .as_ref()
            .ok_or("此任务没有内置终端")?
            .try_send(text.as_bytes().to_vec())
            .map_err(|_| "终端输入队列已满或已关闭，请稍后重试".into())
    }
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> AppResult<()> {
        if !(4..=500).contains(&cols) || !(2..=300).contains(&rows) {
            return Err("终端尺寸超出范围".into());
        }
        let active = self.inner.active.lock().map_err(|e| e.to_string())?;
        let task = active.get(id).ok_or("任务已结束")?;
        if task.input.is_none() {
            return Err("此任务没有内置终端".into());
        }
        task.size
            .store(((cols as u64) << 16) | rows as u64, Ordering::Release);
        Ok(())
    }
    pub fn stop_all(&self) {
        for active in self
            .inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
        {
            active.stop.store(true, Ordering::Release);
        }
    }
    pub fn ack(&self, id: &str, seq: u64) -> AppResult<()> {
        if let Some(active) = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
        {
            if seq > active.sent.load(Ordering::Acquire) {
                return Err("输出确认序号超出已发送范围".into());
            }
            active.ack.fetch_max(seq, Ordering::Relaxed);
        }
        Ok(())
    }
    pub fn read_log(
        &self,
        id: &str,
        cursor: u64,
        encoding: &str,
        limit: usize,
    ) -> AppResult<LogPage> {
        let run: Run = self.inner.store.get("runs", id)?.ok_or("运行记录不存在")?;
        if run.run_mode == "console" {
            return Ok(LogPage {
                text: "系统控制台模式不采集输出".into(),
                next_cursor: cursor,
                has_more: false,
            });
        }
        if uuid::Uuid::parse_str(id).is_err() {
            return Err("运行 ID 不合法".into());
        }
        replay(
            &self.inner.store.root.join("logs").join(id),
            cursor,
            if run.run_mode == "terminal" {
                "utf-8"
            } else {
                encoding
            },
            limit.clamp(BLOCK, MIB as usize),
            !matches!(run.state.as_str(), "starting" | "running" | "stopping"),
        )
    }
}

struct Events {
    channel: Option<Channel<RunEvent>>,
    ack: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
    outstanding: VecDeque<(u64, usize)>,
    bytes: usize,
    gap: bool,
}
impl Events {
    fn new(channel: Option<Channel<RunEvent>>, ack: Arc<AtomicU64>, sent: Arc<AtomicU64>) -> Self {
        Self {
            channel,
            ack,
            sent,
            outstanding: VecDeque::new(),
            bytes: 0,
            gap: false,
        }
    }
    fn send(&mut self, event: RunEvent) {
        if self
            .channel
            .as_ref()
            .is_some_and(|channel| channel.send(event).is_err())
        {
            self.channel = None;
        }
    }
    fn state(&mut self, run: &Run) {
        self.send(RunEvent::State {
            run_id: run.id.clone(),
            state: run.state.clone(),
            reason: run.reason.clone(),
        });
    }
    fn output(&mut self, id: &str, seq: u64, stream: &str, text: String) {
        let ack = self.ack.load(Ordering::Relaxed);
        while self.outstanding.front().is_some_and(|(seq, _)| *seq <= ack) {
            self.bytes -= self.outstanding.pop_front().unwrap().1;
        }
        if self.channel.is_none() {
            return;
        }
        if self.bytes + text.len() > UI_WINDOW {
            if !self.gap {
                self.send(RunEvent::Gap {
                    run_id: id.into(),
                    next_seq: seq,
                });
                self.gap = true;
            }
            return;
        }
        self.gap = false;
        self.bytes += text.len();
        self.outstanding.push_back((seq, text.len()));
        self.sent.store(seq, Ordering::Release);
        self.send(RunEvent::Output {
            run_id: id.into(),
            seq,
            stream: stream.into(),
            text,
        });
    }
}

enum TextDecoder {
    Encoding(Decoder),
    SingleByte(u32),
}
impl TextDecoder {
    fn new(name: &str) -> AppResult<Self> {
        let encoding = match name.to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => Some(encoding_rs::UTF_8),
            "gb18030" => Some(encoding_rs::GB18030),
            "gbk" => Some(encoding_rs::GBK),
            "system" => {
                let cp = unsafe { GetOEMCP() };
                let encoding = match cp {
                    65001 => Some(encoding_rs::UTF_8),
                    936 => Some(encoding_rs::GBK),
                    932 => Some(encoding_rs::SHIFT_JIS),
                    949 => Some(encoding_rs::EUC_KR),
                    950 => Some(encoding_rs::BIG5),
                    866 => Some(encoding_rs::IBM866),
                    874 => Some(encoding_rs::WINDOWS_874),
                    1250..=1258 => Encoding::for_label(format!("windows-{cp}").as_bytes()),
                    _ => None,
                };
                if encoding.is_none() {
                    let mut info: CPINFO = unsafe { std::mem::zeroed() };
                    if unsafe { GetCPInfo(cp, &mut info) } != 0 && info.MaxCharSize == 1 {
                        return Ok(Self::SingleByte(cp));
                    }
                    return Err(format!("不支持的系统代码页 {cp}，请选择 UTF-8 或 GB18030"));
                }
                encoding
            }
            _ => return Err("编码仅支持 utf-8、system 或 gb18030".into()),
        };
        Ok(Self::Encoding(
            encoding.unwrap().new_decoder_without_bom_handling(),
        ))
    }
    fn decode(&mut self, bytes: &[u8], last: bool) -> String {
        match self {
            Self::Encoding(decoder) => {
                let mut output =
                    String::with_capacity(bytes.len().saturating_mul(4).saturating_add(32));
                let mut input = bytes;
                loop {
                    let (result, read, _) = decoder.decode_to_string(input, &mut output, last);
                    input = &input[read..];
                    if result == CoderResult::InputEmpty {
                        break;
                    }
                    output.reserve(input.len().saturating_mul(4).max(32));
                }
                output
            }
            Self::SingleByte(cp) => {
                if bytes.is_empty() {
                    return String::new();
                }
                let mut output = vec![0u16; bytes.len() * 2];
                let count = unsafe {
                    MultiByteToWideChar(
                        *cp,
                        0,
                        bytes.as_ptr(),
                        bytes.len() as i32,
                        output.as_mut_ptr(),
                        output.len() as i32,
                    )
                };
                String::from_utf16_lossy(&output[..count.max(0) as usize])
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Chunk {
    seq: u64,
    stream: String,
    offset: u64,
    length: usize,
    observed_at: String,
}
struct Logs {
    stdout: File,
    stderr: File,
    index: File,
    offsets: [u64; 2],
    used: u64,
    limit: u64,
    total_limit: u64,
    truncated: bool,
}
impl Logs {
    fn create(root: &Path, settings: &Settings) -> AppResult<Self> {
        fs::create_dir_all(root).map_err(|e| format!("创建日志目录失败: {e}"))?;
        let create = |name| {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(root.join(name))
                .map_err(|e| format!("创建日志文件失败: {e}"))
        };
        Ok(Self {
            stdout: create("stdout.bin")?,
            stderr: create("stderr.bin")?,
            index: create("chunks.jsonl")?,
            offsets: [0, 0],
            used: 0,
            limit: settings.log_limit_mib.saturating_mul(MIB),
            total_limit: settings.total_log_mib.saturating_mul(MIB),
            truncated: false,
        })
    }
    fn append(&mut self, store: &Store, seq: u64, stream: usize, data: &[u8]) -> AppResult<bool> {
        if self.truncated {
            return Ok(false);
        }
        let chunk = Chunk {
            seq,
            stream: if stream == 0 { "stdout" } else { "stderr" }.into(),
            offset: self.offsets[stream],
            length: data.len(),
            observed_at: now(),
        };
        let mut line = serde_json::to_vec(&chunk).map_err(|e| e.to_string())?;
        line.push(b'\n');
        let needed = (data.len() + line.len()) as u64;
        if self.used.saturating_add(needed) > self.limit
            || !store.reserve_log_bytes(needed, self.total_limit)
        {
            self.truncated = true;
            return Ok(false);
        }
        let file = if stream == 0 {
            &mut self.stdout
        } else {
            &mut self.stderr
        };
        file.write_all(data)
            .and_then(|_| self.index.write_all(&line))
            .map_err(|e| format!("LOG_WRITE_FAILED: 日志写入失败，正在停止任务: {e}"))?;
        self.offsets[stream] += data.len() as u64;
        self.used += needed;
        Ok(true)
    }
    fn flush(&mut self) -> AppResult<()> {
        self.stdout
            .flush()
            .and_then(|_| self.stderr.flush())
            .and_then(|_| self.index.flush())
            .map_err(|e| format!("LOG_WRITE_FAILED: {e}"))
    }
}

enum PipeMessage {
    Data(usize, Vec<u8>),
    End,
    Error(String),
}
fn reader(
    mut pipe: ReadPipe,
    stream: usize,
    tx: SyncSender<PipeMessage>,
    quit: Arc<AtomicBool>,
    terminal: bool,
) {
    let mut buffer = [0; BLOCK];
    while terminal || !quit.load(Ordering::Acquire) {
        match pipe.read_available(&mut buffer) {
            Ok(Some(0)) => break,
            Ok(Some(count)) => {
                if tx
                    .send(PipeMessage::Data(stream, buffer[..count].to_vec()))
                    .is_err()
                    && !terminal
                {
                    return;
                }
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = tx.send(PipeMessage::Error(error));
                break;
            }
        }
    }
    let _ = tx.send(PipeMessage::End);
}

fn execute(
    inner: &Inner,
    script: &Script,
    settings: &Settings,
    (spec, control): (SpawnSpec, TerminalControl),
    stop: &AtomicBool,
    run: &mut Run,
    events: &mut Events,
) -> AppResult<()> {
    let started = Instant::now();
    let terminal = spec.terminal;
    let mut logs = if spec.console {
        None
    } else {
        Some(Logs::create(
            &inner.store.root.join("logs").join(&run.id),
            settings,
        )?)
    };
    let mut process = process_windows::spawn(spec)?;
    run.state = "running".into();
    inner.store.put("runs", &run.id, run)?;
    events.state(run);
    let (tx, rx) = mpsc::sync_channel(16);
    if let Some(mut input) = process.input.take() {
        thread::Builder::new()
            .name("terminal-input".into())
            .spawn(move || {
                while let Ok(bytes) = control.input.recv() {
                    if process_windows::write_input(&mut input, &bytes).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    let quit = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();
    for (index, pipe) in [process.stdout.take(), process.stderr.take()]
        .into_iter()
        .enumerate()
    {
        if let Some(pipe) = pipe {
            let tx = tx.clone();
            let quit = quit.clone();
            readers.push(
                thread::Builder::new()
                    .name(format!("pipe-{index}"))
                    .spawn(move || reader(pipe, index, tx, quit, terminal))
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    drop(tx);
    let mut decoders = [
        TextDecoder::new(if terminal { "utf-8" } else { &script.encoding })?,
        TextDecoder::new(if terminal { "utf-8" } else { &script.encoding })?,
    ];
    let mut pending: Vec<(u64, usize, String)> = Vec::new();
    let mut pending_bytes = 0;
    let mut last_push = Instant::now();
    let mut last_poll = Instant::now() - Duration::from_secs(1);
    let mut seq = 0;
    let mut ended = 0;
    let mut empty_since = None;
    let mut reason: Option<(String, String)> = None;
    let mut query_failed = false;
    let mut last_size = 0;
    loop {
        if reason.is_none() {
            if stop.load(Ordering::Acquire) {
                reason = Some(("cancelled".into(), "用户强制停止".into()));
            } else if script
                .timeout_seconds
                .is_some_and(|seconds| started.elapsed() >= Duration::from_secs(seconds))
            {
                reason = Some(("timed_out".into(), "任务超时，已强制停止".into()));
            }
        }
        if reason.is_some() && run.state != "stopping" {
            run.state = "stopping".into();
            run.reason = reason.as_ref().map(|r| r.1.clone());
            let _ = inner.store.put("runs", &run.id, run);
            events.state(run);
        }
        if reason.is_some() && empty_since.is_none() {
            if let Err(error) = process.terminate() {
                run.reason = Some(format!("停止任务尚未完成: {error}"));
            }
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(PipeMessage::Data(stream, bytes)) => {
                seq += 1;
                if let Some(logs) = logs.as_mut() {
                    if let Err(error) = logs.append(&inner.store, seq, stream, &bytes) {
                        if reason.is_none() {
                            reason = Some(("failed".into(), error));
                        }
                    }
                    if logs.truncated && !run.log_truncated {
                        run.log_truncated = true;
                        events.send(RunEvent::LogTruncated {
                            run_id: run.id.clone(),
                        });
                        let _ = inner.store.put("runs", &run.id, run);
                    }
                }
                let text = decoders[stream].decode(&bytes, false);
                pending_bytes += text.len();
                if let Some((last_seq, last_stream, last_text)) =
                    pending.last_mut().filter(|v| v.1 == stream)
                {
                    *last_seq = seq;
                    let _ = last_stream;
                    last_text.push_str(&text);
                } else if !text.is_empty() {
                    pending.push((seq, stream, text));
                }
            }
            Ok(PipeMessage::End) => ended += 1,
            Ok(PipeMessage::Error(error)) => {
                if reason.is_none() {
                    reason = Some(("failed".into(), error));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(10)),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
        if pending_bytes >= 32768 || last_push.elapsed() >= Duration::from_millis(50) {
            for (seq, stream, text) in pending.drain(..) {
                events.output(
                    &run.id,
                    seq,
                    if stream == 0 { "stdout" } else { "stderr" },
                    text,
                );
            }
            pending_bytes = 0;
            last_push = Instant::now();
        }
        if last_poll.elapsed() >= Duration::from_millis(50) {
            last_poll = Instant::now();
            let size = control.size.load(Ordering::Acquire);
            if terminal && size != 0 && size != last_size && empty_since.is_none() {
                if let Err(error) = process.resize((size >> 16) as u16, size as u16) {
                    reason = Some(("failed".into(), error));
                }
                last_size = size;
            }
            match process.root_exit_code() {
                Ok(code) => {
                    run.root_exit_code = code;
                }
                Err(error) => {
                    if reason.is_none() {
                        reason = Some(("failed".into(), error));
                    }
                }
            }
            match process.active_processes() {
                Ok(0) => {
                    if empty_since.is_none() {
                        empty_since = Some(Instant::now());
                        process.close_terminal();
                    }
                }
                Ok(_) => {
                    if run.root_exit_code.is_some() && reason.is_none() && run.reason.is_none() {
                        run.reason = Some("根进程已退出，正在等待子进程结束".into());
                        inner.store.put("runs", &run.id, run)?;
                        events.state(run);
                    }
                }
                Err(error) => {
                    if !query_failed {
                        query_failed = true;
                        reason = Some(("failed".into(), format!("Job 状态查询失败: {error}")));
                    }
                }
            }
        }
        if let Some(empty) = empty_since {
            if ended >= readers.len() {
                break;
            }
            if empty.elapsed() >= Duration::from_secs(2) {
                run.log_truncated = true;
                if reason.is_none() {
                    run.reason =
                        Some("任务树已结束，外部进程仍持有日志管道，日志可能不完整".into());
                }
                break;
            }
        }
    }
    quit.store(true, Ordering::Release);
    drop(rx);
    for reader in readers {
        if !terminal || reader.is_finished() {
            let _ = reader.join();
        }
    }
    for (stream, decoder) in decoders.iter_mut().enumerate() {
        let text = decoder.decode(&[], true);
        if !text.is_empty() {
            seq += 1;
            pending.push((seq, stream, text));
        }
    }
    for (seq, stream, text) in pending {
        events.output(
            &run.id,
            seq,
            if stream == 0 { "stdout" } else { "stderr" },
            text,
        );
    }
    if let Some(logs) = logs.as_mut() {
        logs.flush()?;
    }
    if let Some((state, message)) = reason {
        run.state = state;
        run.reason = Some(message);
    } else {
        run.state = if run
            .root_exit_code
            .is_some_and(|code| script.success_codes.contains(&code))
        {
            "succeeded"
        } else {
            "failed"
        }
        .into();
        if run.reason.as_deref() == Some("根进程已退出，正在等待子进程结束") {
            run.reason = None;
        }
    }
    Ok(())
}

fn replay(
    root: &Path,
    cursor: u64,
    encoding: &str,
    limit: usize,
    finished: bool,
) -> AppResult<LogPage> {
    let index = match File::open(root.join("chunks.jsonl")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LogPage {
                text: String::new(),
                next_cursor: cursor,
                has_more: false,
            })
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut streams = [
        File::open(root.join("stdout.bin")).map_err(|e| e.to_string())?,
        File::open(root.join("stderr.bin")).map_err(|e| e.to_string())?,
    ];
    let index_size = index.metadata().map_err(|e| e.to_string())?.len();
    if cursor > index_size {
        return Err("日志游标超出文件范围".into());
    }
    let mut reader = BufReader::new(index);
    let mut position = 0;
    let mut next = cursor;
    let mut text = String::new();
    let mut truncated = false;
    let mut partial = false;
    let mut decoders = [TextDecoder::new(encoding)?, TextDecoder::new(encoding)?];
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take(4097)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if count > 4096 {
            truncated = true;
            break;
        }
        if line.last() != Some(&b'\n') {
            truncated = finished;
            partial = true;
            break;
        }
        let chunk: Chunk = match serde_json::from_slice(&line) {
            Ok(chunk) => chunk,
            Err(_) => {
                truncated = true;
                break;
            }
        };
        let stream = match chunk.stream.as_str() {
            "stdout" => 0,
            "stderr" => 1,
            _ => {
                truncated = true;
                break;
            }
        };
        if chunk.length > BLOCK
            || chunk
                .offset
                .checked_add(chunk.length as u64)
                .is_none_or(|end| {
                    streams[stream]
                        .metadata()
                        .map(|m| end > m.len())
                        .unwrap_or(true)
                })
        {
            truncated = true;
            break;
        }
        let mut bytes = vec![0; chunk.length];
        streams[stream]
            .seek(SeekFrom::Start(chunk.offset))
            .and_then(|_| streams[stream].read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
        let decoded = decoders[stream].decode(&bytes, false);
        if position >= cursor {
            if stream == 1 && !decoded.is_empty() {
                text.push_str("[stderr] ");
            }
            text.push_str(&decoded);
            next = position + count as u64;
        } else if position + count as u64 > cursor {
            return Err("日志游标必须指向索引记录边界".into());
        }
        position += count as u64;
        if text.len() >= limit {
            break;
        }
    }
    let has_more = next < index_size && !truncated && !partial;
    if finished && !has_more {
        for decoder in &mut decoders {
            text.push_str(&decoder.decode(&[], true));
        }
    }
    if truncated {
        text.push_str("\n[日志尾部损坏或不完整，已停止回放]\n");
    }
    Ok(LogPage {
        text,
        next_cursor: next,
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_multibyte_output_survives_independent_chunks() {
        let mut decoder = TextDecoder::new("utf-8").unwrap();
        let bytes = "中文🙂".as_bytes();
        let mut result = String::new();
        for byte in bytes {
            result.push_str(&decoder.decode(&[*byte], false));
        }
        result.push_str(&decoder.decode(&[], true));
        assert_eq!(result, "中文🙂");
        let (bytes, _, _) = encoding_rs::GB18030.encode("中文🙂");
        let mut decoder = TextDecoder::new("gb18030").unwrap();
        let mut result = String::new();
        for byte in bytes.iter() {
            result.push_str(&decoder.decode(&[*byte], false));
        }
        result.push_str(&decoder.decode(&[], true));
        assert_eq!(result, "中文🙂");
    }
    #[test]
    fn case_insensitive_environment_override_keeps_one_key() {
        let mut environment = BTreeMap::from([(OsString::from("Path"), OsString::from("first"))]);
        set_env(&mut environment, "PATH", "second");
        assert_eq!(environment.len(), 1);
        assert_eq!(environment.get(&OsString::from("PATH")).unwrap(), "second");
    }
    #[test]
    fn optional_secrets_do_not_require_input_and_required_secrets_do() {
        let values = BTreeMap::new();
        let mut slot = SecretSlot {
            secret_slot: "token".into(),
            required: false,
        };
        assert_eq!(
            resolve(&InputValue::Secret(slot.clone()), &values).unwrap(),
            ""
        );
        slot.required = true;
        assert!(resolve(&InputValue::Secret(slot), &values).is_err());
    }
    #[test]
    fn channel_unacknowledged_output_is_bounded_and_recovers_after_ack() {
        let delivered = Arc::new(AtomicU64::new(0));
        let count = delivered.clone();
        let channel = Channel::new(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        let ack = Arc::new(AtomicU64::new(0));
        let mut events = Events::new(Some(channel), ack.clone(), Arc::new(AtomicU64::new(0)));
        for seq in 1..=100 {
            events.output("test", seq, "stdout", "x".repeat(32768));
        }
        assert_eq!(events.bytes, UI_WINDOW);
        assert_eq!(events.outstanding.len(), 16);
        assert_eq!(delivered.load(Ordering::Relaxed), 17);
        ack.store(16, Ordering::Relaxed);
        events.output("test", 101, "stdout", "resumed".into());
        assert_eq!(events.bytes, 7);
        assert_eq!(events.outstanding.len(), 1);
        assert_eq!(delivered.load(Ordering::Relaxed), 18);
    }
    #[test]
    fn replay_reconstructs_separate_decoder_states_across_page_boundaries() {
        let root = std::env::temp_dir().join(format!("rscripto-replay-{}", new_id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("stdout.bin"), b"A\xe4\xb8\xadB").unwrap();
        fs::write(root.join("stderr.bin"), b"X\xe6\x96\x87Y").unwrap();
        let mut index = File::create(root.join("chunks.jsonl")).unwrap();
        for (seq, stream, offset, length) in [
            (1, "stdout", 0, 3),
            (2, "stderr", 0, 3),
            (3, "stdout", 3, 2),
            (4, "stderr", 3, 2),
        ] {
            let chunk = Chunk {
                seq,
                stream: stream.into(),
                offset,
                length,
                observed_at: now(),
            };
            writeln!(index, "{}", serde_json::to_string(&chunk).unwrap()).unwrap();
        }
        drop(index);
        let mut cursor = 0;
        let mut pages = Vec::new();
        loop {
            let page = replay(&root, cursor, "utf-8", 1, true).unwrap();
            assert!(page.next_cursor > cursor);
            cursor = page.next_cursor;
            pages.push(page.text);
            if !page.has_more {
                break;
            }
        }
        assert_eq!(pages, vec!["A", "[stderr] X", "中B", "[stderr] 文Y"]);
        assert!(!pages.concat().contains('\u{fffd}'));
        assert!(root.parent() == Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(root).unwrap();
    }
    fn python() -> PathBuf {
        if let Some(path) = std::env::var_os("RSCRIPTO_TEST_PYTHON") {
            return PathBuf::from(path);
        }
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join("python.exe"))
            .find(|p| p.is_file() && !p.to_string_lossy().contains("WindowsApps"))
            .expect("运行集成测试需要真实 Python，或设置 RSCRIPTO_TEST_PYTHON")
    }
    fn setup(filename: &str) -> (PathBuf, Arc<Store>, Runner, Script, Runtime) {
        let root = std::env::temp_dir().join(format!("rscripto-runner-{}", new_id()));
        fs::create_dir_all(&root).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/runner")
            .join(filename);
        let path = root.join(filename);
        fs::copy(fixture, &path).unwrap();
        let store = Arc::new(Store::open(root.join("data")).unwrap());
        let runner = Runner::new(store.clone());
        let script = Script {
            id: new_id(),
            name: filename.into(),
            path: path.to_string_lossy().into_owned(),
            source_path: None,
            kind: "python".into(),
            description: String::new(),
            tags: vec![],
            favorite: false,
            runtime_id: None,
            environment_id: None,
            cwd: root.to_string_lossy().into_owned(),
            args: vec![],
            env: BTreeMap::new(),
            run_mode: "captured".into(),
            execution_policy: None,
            timeout_seconds: Some(10),
            encoding: "utf-8".into(),
            success_codes: vec![0],
            trusted_hash: Some(entry_hash(&path).unwrap()),
            created_at: now(),
            updated_at: now(),
            exists: true,
        };
        let runtime = Runtime {
            id: new_id(),
            kind: "python".into(),
            executable_path: python().to_string_lossy().into_owned(),
            version: "test".into(),
            architecture: "x64".into(),
            source: "test".into(),
            status: "ready".into(),
            execution_policy: None,
            prefix: None,
            base_prefix: None,
            has_pip: true,
            checked_at: now(),
        };
        (root, store, runner, script, runtime)
    }
    fn finished(store: &Store, id: &str) -> Run {
        let start = Instant::now();
        loop {
            let run: Run = store.get("runs", id).unwrap().unwrap();
            if run.finished_at.is_some() {
                return run;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "run did not finish: {} {:?}",
                run.state,
                run.reason
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn wait_terminal(runner: &Runner, id: &str, expected: &str) -> String {
        let started = Instant::now();
        loop {
            let text = runner
                .read_log(id, 0, "utf-8", MIB as usize)
                .map(|p| p.text)
                .unwrap_or_default();
            if text.contains(expected) {
                return text;
            }
            assert!(
                started.elapsed() < Duration::from_secs(12),
                "terminal waiting for {expected:?}: {text:?}"
            );
            thread::sleep(Duration::from_millis(30));
        }
    }
    #[test]
    fn real_terminal_python_unicode_secret_input_resize_and_ctrl_c() {
        let (root, store, runner, mut script, runtime) = setup("echo.py");
        fs::write(&script.path, "import os, getpass, time\nprint('TTY', os.isatty(0), os.isatty(1), flush=True)\nvalue = input('READY>')\nprint('VALUE=' + value, flush=True)\nprint('SIZE', os.get_terminal_size().columns, flush=True)\nsecret = getpass.getpass('SECRET>')\nprint('SECRET_OK=' + str(secret == 'test-secret'), flush=True)\nprint('WAIT_CTRL_C', flush=True)\ntry:\n time.sleep(30)\nexcept KeyboardInterrupt:\n print('INTERRUPTED', flush=True)\n").unwrap();
        script.run_mode = "terminal".into();
        script.trusted_hash = Some(entry_hash(Path::new(&script.path)).unwrap());
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        let output = wait_terminal(&runner, &id, "READY>");
        assert!(output.contains("TTY True True"), "{output:?}");
        runner.resize(&id, 112, 35).unwrap();
        thread::sleep(Duration::from_millis(150));
        runner.input(&id, "中文 test\r").unwrap();
        let output = wait_terminal(&runner, &id, "SECRET>");
        assert!(output.contains("VALUE=中文 test"), "{output:?}");
        assert!(output.contains("SIZE 112"), "{output:?}");
        runner.input(&id, "test-secret\r").unwrap();
        let output = wait_terminal(&runner, &id, "WAIT_CTRL_C");
        assert!(output.contains("SECRET_OK=True"), "{output:?}");
        assert!(
            !output.contains("test-secret"),
            "password was echoed: {output:?}"
        );
        runner.input(&id, "\u{3}").unwrap();
        let run = finished(&store, &id);
        assert_eq!(run.state, "succeeded", "{:?}", run.reason);
        assert!(runner
            .read_log(&id, 0, "system", MIB as usize)
            .unwrap()
            .text
            .contains("INTERRUPTED"));
        assert!(runner.input(&id, "late").is_err());
        cleanup(root, store, runner);
    }
    #[test]
    fn real_terminal_read_host_read_key_and_cmd_set_input() {
        let engines = [
            (
                "cmd",
                "cmd",
                PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe"),
            ),
            (
                "ps1",
                "windows_powershell",
                PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                    .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            ),
            (
                "ps1",
                "powershell",
                PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            ),
        ];
        for (extension, kind, executable) in engines {
            assert!(
                executable.is_file(),
                "missing test engine: {}",
                executable.display()
            );
            let (root, store, runner, mut script, mut runtime) = setup("echo.py");
            let source = root.join(format!("interactive.{extension}"));
            let code = if kind == "cmd" {
                "@echo off\r\nset /p value=READY_INPUT:\r\necho VALUE=%value%\r\necho READY_KEY\r\npause >nul\r\necho KEY_OK\r\n"
            } else {
                "$value = Read-Host 'READY_INPUT'\nWrite-Output \"VALUE=$value\"\nWrite-Output 'READY_KEY'\n$key = [Console]::ReadKey($true)\nWrite-Output 'KEY_OK'\n"
            };
            fs::write(&source, code).unwrap();
            script.path = source.to_string_lossy().into();
            script.kind = if kind == "cmd" { "cmd" } else { "powershell" }.into();
            script.run_mode = "terminal".into();
            script.execution_policy = if kind == "cmd" {
                None
            } else {
                Some("Bypass".into())
            };
            script.trusted_hash = Some(entry_hash(&source).unwrap());
            runtime.kind = kind.into();
            runtime.executable_path = executable.to_string_lossy().into();
            let id = runner
                .start(script, runtime, Settings::default(), BTreeMap::new(), None)
                .unwrap();
            wait_terminal(&runner, &id, "READY_INPUT");
            runner.input(&id, "answer 42\r").unwrap();
            let output = wait_terminal(&runner, &id, "READY_KEY");
            assert!(output.contains("VALUE=answer 42"), "{kind}: {output:?}");
            runner.input(&id, "y").unwrap();
            let run = finished(&store, &id);
            assert_eq!(run.state, "succeeded", "{kind}: {:?}", run.reason);
            assert!(runner
                .read_log(&id, 0, "utf-8", MIB as usize)
                .unwrap()
                .text
                .contains("KEY_OK"));
            cleanup(root, store, runner);
        }
    }
    #[test]
    fn real_terminal_force_stop_while_waiting_for_input() {
        let (root, store, runner, mut script, runtime) = setup("echo.py");
        fs::write(&script.path, "input('WAIT_INPUT>')\n").unwrap();
        script.run_mode = "terminal".into();
        script.trusted_hash = Some(entry_hash(Path::new(&script.path)).unwrap());
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        wait_terminal(&runner, &id, "WAIT_INPUT>");
        assert!(runner.input(&id, &"x".repeat(4097)).is_err());
        assert!(runner.resize(&id, 0, 30).is_err());
        runner.stop(&id).unwrap();
        assert_eq!(finished(&store, &id).state, "cancelled");
        assert_eq!(runner.active_count(), 0);
        cleanup(root, store, runner);
    }
    fn cleanup(root: PathBuf, store: Arc<Store>, runner: Runner) {
        runner.stop_all();
        let start = Instant::now();
        while runner.active_count() > 0 && start.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        drop(runner);
        let start = Instant::now();
        while Arc::strong_count(&store) > 1 && start.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(10));
        }
        drop(store);
        assert!(
            root.is_absolute()
                && root.parent() == Some(std::env::temp_dir().as_path())
                && root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("rscripto-runner-")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn real_runner_persists_state_and_replays_both_streams() {
        let (root, store, runner, mut script, runtime) = setup("echo.py");
        script.args = vec![
            InputValue::Text("中文 空参数".into()),
            InputValue::Text(String::new()),
        ];
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        let run = finished(&store, &id);
        assert_eq!(run.state, "succeeded", "{:?}", run.reason);
        assert_eq!(run.root_exit_code, Some(0));
        let log = runner.read_log(&id, 0, "utf-8", 65536).unwrap();
        assert!(log.text.contains("中文 空参数"));
        assert!(log.text.contains("stderr 可见"));
        assert!(!log.has_more);
        assert!(log.next_cursor > 0);
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_drains_after_quota_without_failing_script() {
        let (root, store, runner, script, runtime) = setup("flood.py");
        let settings = Settings {
            log_limit_mib: 1,
            ..Settings::default()
        };
        let id = runner
            .start(script, runtime, settings, BTreeMap::new(), None)
            .unwrap();
        let run = finished(&store, &id);
        assert_eq!(run.state, "succeeded", "{:?}", run.reason);
        assert!(run.log_truncated);
        let size: u64 = fs::read_dir(store.root.join("logs").join(&id))
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        assert!(size <= MIB);
        assert!(size > 0);
        cleanup(root, store, runner);
    }
    #[test]
    #[ignore = "60-second release acceptance; run explicitly with --ignored"]
    fn real_runner_sustained_output_keeps_default_quota_and_storage_responsive() {
        let (root, store, runner, mut script, runtime) = setup("sustained_output.py");
        script.timeout_seconds = Some(75);
        let started = Instant::now();
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        let mut samples = Vec::new();
        let run = loop {
            let query = Instant::now();
            let run: Run = store.get("runs", &id).unwrap().unwrap();
            samples.push(query.elapsed());
            if run.finished_at.is_some() {
                break run;
            }
            assert!(started.elapsed() < Duration::from_secs(80));
            thread::sleep(Duration::from_millis(100));
        };
        let elapsed = started.elapsed();
        assert_eq!(run.state, "succeeded", "{:?}", run.reason);
        assert!(run.log_truncated);
        assert!(elapsed >= Duration::from_secs(60));
        assert!(elapsed < Duration::from_secs(70));
        let size: u64 = fs::read_dir(store.root.join("logs").join(&id))
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        assert!((19 * MIB..=20 * MIB).contains(&size));
        samples.sort();
        let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
        assert!(p95 < Duration::from_millis(200));
        println!("backend sustained output: elapsed={elapsed:?}, persisted_bytes={size}, database_read_p95={p95:?}; excludes WebView rendering");
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_stops_after_actual_log_write_failure() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY},
            System::IO::OVERLAPPED,
        };
        let (root, store, runner, mut script, runtime) = setup("echo.py");
        fs::write(&script.path, "import pathlib,time\nwhile not pathlib.Path('release-output').exists(): time.sleep(.01)\nprint('must fail to persist', flush=True)\ntime.sleep(60)\n").unwrap();
        script.trusted_hash = Some(entry_hash(Path::new(&script.path)).unwrap());
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        let started = Instant::now();
        while store.get::<Run>("runs", &id).unwrap().unwrap().state != "running" {
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(10));
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.root.join("logs").join(&id).join("stdout.bin"))
            .unwrap();
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                LockFileEx(
                    lock.as_raw_handle(),
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    1024 * 1024,
                    0,
                    &mut overlapped,
                )
            },
            0
        );
        let triggered = Instant::now();
        fs::write(root.join("release-output"), b"go").unwrap();
        let run = finished(&store, &id);
        assert_eq!(run.state, "failed", "{:?}", run.reason);
        assert!(run.reason.unwrap().contains("LOG_WRITE_FAILED"));
        assert!(triggered.elapsed() < Duration::from_secs(3));
        assert_eq!(runner.active_count(), 0);
        drop(lock);
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_recovers_after_host_process_is_killed() {
        use std::process::{Command, Stdio};
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        const HANDOFF: &str = "RSCRIPTO_CRASH_TEST_HANDOFF";
        if let Some(handoff) = std::env::var_os(HANDOFF) {
            let (root, store, runner, mut script, runtime) = setup("echo.py");
            fs::write(&script.path, "import json,os,pathlib,subprocess,sys,time\nchild=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])\npathlib.Path('pids.json').write_text(json.dumps([os.getpid(),child.pid]))\nprint('running',flush=True)\ntime.sleep(60)\n").unwrap();
            script.trusted_hash = Some(entry_hash(Path::new(&script.path)).unwrap());
            script.timeout_seconds = Some(60);
            store.put("scripts", &script.id, &script).unwrap();
            let id = runner
                .start(script, runtime, Settings::default(), BTreeMap::new(), None)
                .unwrap();
            let started = Instant::now();
            while !root.join("pids.json").is_file() {
                assert!(started.elapsed() < Duration::from_secs(10));
                thread::sleep(Duration::from_millis(10));
            }
            let manifest = serde_json::json!({"root":root,"id":id});
            let path = PathBuf::from(handoff);
            fs::write(
                path.with_extension("tmp"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            fs::rename(path.with_extension("tmp"), path).unwrap();
            thread::sleep(Duration::from_secs(30));
            panic!("parent did not terminate the test host");
        }
        let handoff = std::env::temp_dir().join(format!("rscripto-crash-{}.json", new_id()));
        let mut host = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runner::tests::real_runner_recovers_after_host_process_is_killed",
                "--nocapture",
            ])
            .env(HANDOFF, &handoff)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !handoff.is_file() {
            if started.elapsed() > Duration::from_secs(15) || host.try_wait().unwrap().is_some() {
                let _ = host.kill();
                let _ = host.wait();
                panic!("crash test host did not become ready");
            }
            thread::sleep(Duration::from_millis(10));
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&handoff).unwrap()).unwrap();
        let root = PathBuf::from(manifest["root"].as_str().unwrap());
        let id = manifest["id"].as_str().unwrap();
        let pids: Vec<u32> =
            serde_json::from_slice(&fs::read(root.join("pids.json")).unwrap()).unwrap();
        let handles: Vec<_> = pids
            .iter()
            .map(|pid| unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, *pid) })
            .collect();
        host.kill().unwrap();
        assert!(!host.wait().unwrap().success());
        for handle in handles {
            assert!(
                !handle.is_null(),
                "test descendant must be alive before host termination"
            );
            let wait = unsafe { WaitForSingleObject(handle, 3000) };
            unsafe {
                CloseHandle(handle);
            }
            assert_eq!(
                wait, WAIT_OBJECT_0,
                "Job descendant survived abrupt host termination"
            );
        }
        let store = Arc::new(Store::open(root.join("data")).unwrap());
        let run: Run = store.get("runs", id).unwrap().unwrap();
        assert_eq!(run.state, "interrupted");
        assert!(run.finished_at.is_some());
        assert_eq!(store.load_scripts().unwrap().len(), 1);
        assert!(root.join("echo.py").is_file());
        let runner = Runner::new(store.clone());
        cleanup(root, store, runner);
        fs::remove_file(handoff).unwrap();
    }
    #[test]
    fn real_runner_times_out_entire_tree_and_enforces_reentry_and_capacity() {
        let (root, store, runner, mut script, runtime) = setup("tree.py");
        script.timeout_seconds = Some(1);
        let settings = Settings {
            max_concurrent: 1,
            ..Settings::default()
        };
        let id = runner
            .start(
                script.clone(),
                runtime.clone(),
                settings.clone(),
                BTreeMap::new(),
                None,
            )
            .unwrap();
        assert!(runner
            .start(
                script.clone(),
                runtime.clone(),
                settings.clone(),
                BTreeMap::new(),
                None
            )
            .unwrap_err()
            .contains("重复"));
        script.id = new_id();
        assert!(runner
            .start(script, runtime, settings, BTreeMap::new(), None)
            .unwrap_err()
            .contains("并发"));
        let run = finished(&store, &id);
        assert_eq!(run.state, "timed_out", "{:?}", run.reason);
        assert_eq!(runner.active_count(), 0);
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_immediate_stop_has_one_terminal_state() {
        let (root, store, runner, script, runtime) = setup("tree.py");
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        runner.stop(&id).unwrap();
        runner.stop(&id).unwrap();
        let run = finished(&store, &id);
        assert_eq!(run.state, "cancelled", "{:?}", run.reason);
        runner.stop(&id).unwrap();
        assert_eq!(
            store.get::<Run>("runs", &id).unwrap().unwrap().state,
            "cancelled"
        );
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_rejects_changed_script_before_any_run_record() {
        let (root, store, runner, script, runtime) = setup("echo.py");
        let mut source = OpenOptions::new().append(true).open(&script.path).unwrap();
        source.write_all(b"\n# changed\n").unwrap();
        drop(source);
        assert!(runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap_err()
            .contains("SCRIPT_CHANGED"));
        assert!(store.load_runs().unwrap().is_empty());
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_keeps_resolved_secrets_out_of_persisted_execution_plan() {
        let (root, store, runner, mut script, runtime) = setup("echo.py");
        script.env.insert(
            "TOKEN".into(),
            InputValue::Secret(SecretSlot {
                secret_slot: "token".into(),
                required: true,
            }),
        );
        let id = runner
            .start(
                script,
                runtime,
                Settings::default(),
                BTreeMap::from([("token".into(), "PRIVATE_VALUE_729421".into())]),
                None,
            )
            .unwrap();
        let run = finished(&store, &id);
        assert_eq!(run.state, "succeeded");
        let serialized = serde_json::to_string(&run).unwrap();
        assert!(serialized.contains("secretSlot"));
        assert!(!serialized.contains("PRIVATE_VALUE_729421"));
        cleanup(root, store, runner);
    }
    fn powershell_setup() -> (PathBuf, Arc<Store>, Runner, Script, Runtime) {
        let (root, store, runner, mut script, mut runtime) = setup("echo.ps1");
        script.kind = "powershell".into();
        runtime.kind = "windows_powershell".into();
        runtime.executable_path = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe")
            .to_string_lossy()
            .into_owned();
        runtime.execution_policy = Some("Restricted".into());
        (root, store, runner, script, runtime)
    }
    #[test]
    fn execution_policy_is_explicit_process_only_and_powershell_only() {
        let (root, store, runner, mut script, runtime) = powershell_setup();
        let default = plan(&script, &runtime, &BTreeMap::new()).unwrap();
        assert!(!default
            .arguments
            .iter()
            .any(|arg| arg == "-ExecutionPolicy"));
        for policy in ["RemoteSigned", "Bypass"] {
            script.execution_policy = Some(policy.into());
            let explicit = plan(&script, &runtime, &BTreeMap::new()).unwrap();
            let index = explicit
                .arguments
                .iter()
                .position(|arg| arg == "-ExecutionPolicy")
                .unwrap();
            let file = explicit
                .arguments
                .iter()
                .position(|arg| arg == "-File")
                .unwrap();
            assert_eq!(explicit.arguments[index + 1], policy);
            assert!(index < file);
        }
        for policy in [
            "Unrestricted",
            "Restricted",
            "Bypass -Command evil",
            "",
            "bypass",
        ] {
            script.execution_policy = Some(policy.into());
            assert!(plan(&script, &runtime, &BTreeMap::new()).is_err());
        }
        script.execution_policy = Some("Bypass".into());
        for kind in ["python", "cmd"] {
            script.kind = kind.into();
            assert!(plan(&script, &runtime, &BTreeMap::new())
                .unwrap_err()
                .contains("仅 PowerShell"));
        }
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_explicit_process_policy_runs_windows_powershell() {
        let (root, store, runner, mut script, runtime) = powershell_setup();
        script.execution_policy = Some("Bypass".into());
        script.args = vec![
            InputValue::Text("中文 参数".into()),
            InputValue::Text(String::new()),
        ];
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        let run = finished(&store, &id);
        assert_eq!(run.state, "succeeded", "{:?}", run.reason);
        assert_eq!(run.root_exit_code, Some(0));
        assert_eq!(run.plan_redacted["executionPolicy"], "Bypass");
        assert!(runner
            .read_log(&id, 0, "utf-8", 65536)
            .unwrap()
            .text
            .contains("中文 参数"));
        cleanup(root, store, runner);
    }
    #[test]
    fn real_runner_windows_powershell_prefers_own_modules_over_inherited_ps7_modules() {
        let (root, store, runner, mut script, runtime) = powershell_setup();
        let original = std::env::var_os("PSModulePath");
        let polluted = r"C:\Program Files\PowerShell\7\Modules;C:\user-module-placeholder";
        script
            .env
            .insert("pSmOdUlEpAtH".into(), InputValue::Text(polluted.into()));
        script.execution_policy = Some("Bypass".into());
        fs::write(&script.path, b"$ErrorActionPreference='Stop'\r\n[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false)\r\n[Console]::WriteLine((ConvertTo-Json -Compress -InputObject @{policy=[string](Get-ExecutionPolicy);modulePath=$env:PSModulePath}))\r\n").unwrap();
        script.trusted_hash = Some(entry_hash(Path::new(&script.path)).unwrap());
        let spec = plan(&script, &runtime, &BTreeMap::new()).unwrap();
        let module_path = spec
            .environment
            .get(&OsString::from("PSModulePath"))
            .unwrap();
        assert_eq!(
            std::env::split_paths(module_path).next().unwrap(),
            spec.executable.parent().unwrap().join("Modules")
        );
        assert!(module_path.to_string_lossy().ends_with(polluted));
        let id = runner
            .start(script, runtime, Settings::default(), BTreeMap::new(), None)
            .unwrap();
        let run = finished(&store, &id);
        let log = runner.read_log(&id, 0, "utf-8", 65536).unwrap().text;
        assert_eq!(run.state, "succeeded", "{:?}: {log}", run.reason);
        let result: serde_json::Value = serde_json::from_str(log.trim()).unwrap();
        assert_eq!(result["policy"], "Bypass");
        assert!(result["modulePath"]
            .as_str()
            .unwrap()
            .contains("user-module-placeholder"));
        assert_eq!(std::env::var_os("PSModulePath"), original);
        cleanup(root, store, runner);
    }
}
