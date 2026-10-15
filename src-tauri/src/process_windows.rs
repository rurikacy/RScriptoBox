//! The only script process creation path: the root cannot execute before its Job owns it.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{self, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::*,
    System::{Console::*, JobObjects::*, Pipes::*, Threading::*},
};

fn last_error() -> String {
    io::Error::last_os_error().to_string()
}
fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>, String> {
    let mut result: Vec<u16> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err("字符串包含 NUL".into());
    }
    result.push(0);
    Ok(result)
}

struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub struct ReadPipe(File);
impl ReadPipe {
    /// A dedicated reader calls this without ever blocking on a pipe held by an escaped process.
    pub fn read_available(&mut self, buffer: &mut [u8]) -> Result<Option<usize>, String> {
        let handle = self.0.as_raw_handle() as HANDLE;
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                handle,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        } == 0
        {
            let error = unsafe { GetLastError() };
            return if error == ERROR_BROKEN_PIPE {
                Ok(Some(0))
            } else {
                Err(format!("读取日志管道失败 ({error}): {}", last_error()))
            };
        }
        if available == 0 {
            return Ok(None);
        }
        let mut read = 0;
        let count = available.min(buffer.len() as u32);
        if unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                count,
                &mut read,
                ptr::null_mut(),
            )
        } == 0
        {
            let error = unsafe { GetLastError() };
            return if error == ERROR_BROKEN_PIPE {
                Ok(Some(0))
            } else {
                Err(format!("读取日志失败 ({error}): {}", last_error()))
            };
        }
        Ok(Some(read as usize))
    }
}

#[derive(Clone, Debug)]
pub struct SpawnSpec {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
    pub console: bool,
    pub terminal: bool,
    /// CMD /C syntax is deliberately built separately from ordinary argv escaping.
    pub cmd_payload: Option<String>,
}

pub struct JobProcess {
    job: Handle,
    process: Handle,
    #[allow(dead_code)] // The standalone Windows lifecycle tests inspect this ID.
    pub pid: u32,
    pub stdout: Option<ReadPipe>,
    pub stderr: Option<ReadPipe>,
    pub input: Option<File>,
    terminal: Option<Arc<PseudoConsole>>,
}
struct PseudoConsole {
    handle: HPCON,
    size: AtomicU64,
    closing: AtomicBool,
}
unsafe impl Send for PseudoConsole {}
unsafe impl Sync for PseudoConsole {}
impl Drop for PseudoConsole {
    fn drop(&mut self) {
        // Windows 10 may wait for output draining during close; never close on the reader thread.
        let handle = self.handle;
        std::thread::spawn(move || unsafe { ClosePseudoConsole(handle) });
    }
}
impl JobProcess {
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let terminal = self.terminal.as_ref().ok_or("此任务没有内置终端")?;
        terminal
            .size
            .store(((cols as u64) << 16) | rows as u64, Ordering::Release);
        Ok(())
    }
    pub fn close_terminal(&mut self) {
        if let Some(terminal) = self.terminal.take() {
            terminal.closing.store(true, Ordering::Release);
        }
    }
    pub fn terminate(&self) -> Result<(), String> {
        if unsafe { TerminateJobObject(self.job.0, 1) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }
    pub fn active_processes(&self) -> Result<u32, String> {
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            QueryInformationJobObject(
                self.job.0,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(info.ActiveProcesses)
    }
    pub fn root_exit_code(&self) -> Result<Option<u32>, String> {
        match unsafe { WaitForSingleObject(self.process.0, 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
                    Err(last_error())
                } else {
                    Ok(Some(code))
                }
            }
            _ => Err(last_error()),
        }
    }
}
impl Drop for JobProcess {
    fn drop(&mut self) {
        self.close_terminal();
    }
}

/// Microsoft CRT argv quoting, used by Python and the PowerShell -File adapter.
pub fn quote_arg(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("参数包含 NUL".into());
    }
    let mut output = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            output.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            output.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        output.push(ch);
    }
    output.extend(std::iter::repeat_n('\\', slashes * 2));
    output.push('"');
    Ok(output)
}

pub fn validate_cmd_value(value: &str, field: &str) -> Result<(), String> {
    if let Some(ch) = value
        .chars()
        .find(|ch| !ch.is_alphanumeric() && !" \\/:._-+=,@[]{}~".contains(*ch))
    {
        return Err(format!(
            "CMD_ARGUMENT_UNSUPPORTED: {field} 含有不支持的字符 {ch:?}，请改名或使用配置文件传递"
        ));
    }
    Ok(())
}

pub fn cmd_payload(script: &Path, args: &[String], cwd: &Path) -> Result<String, String> {
    let script = script.to_str().ok_or("CMD 路径不是有效 Unicode")?;
    validate_cmd_value(script, "脚本路径")?;
    validate_cmd_value(cwd.to_str().ok_or("工作目录不是有效 Unicode")?, "工作目录")?;
    let mut result = format!("\"\"{script}\"");
    for (index, arg) in args.iter().enumerate() {
        validate_cmd_value(arg, &format!("参数 {}", index + 1))?;
        result.push_str(&format!(" \"{arg}\""));
    }
    result.push('"');
    Ok(result)
}

struct Attributes {
    buffer: Vec<usize>,
    _handles: Vec<HANDLE>,
    initialized: bool,
}
impl Attributes {
    fn new(handles: &mut [HANDLE]) -> Result<Self, String> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        }
        let mut result = Self {
            buffer: vec![0; size.div_ceil(std::mem::size_of::<usize>())],
            _handles: handles.to_vec(),
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(result.list(), 1, 0, &mut size) } == 0 {
            return Err(last_error());
        }
        result.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                result.list(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                result._handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(result)
    }
    fn list(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }
    fn terminal(handle: HPCON) -> Result<Self, String> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        }
        let mut result = Self {
            buffer: vec![0; size.div_ceil(std::mem::size_of::<usize>())],
            _handles: vec![],
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(result.list(), 1, 0, &mut size) } == 0 {
            return Err(last_error());
        }
        result.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                result.list(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                handle as *const _,
                std::mem::size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(result)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.list());
            }
        }
    }
}

fn pipe() -> Result<(ReadPipe, Handle), String> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        return Err(last_error());
    }
    let write = Handle(write);
    let read = unsafe { File::from_raw_handle(read) };
    if unsafe { SetHandleInformation(read.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, 0) } == 0
    {
        return Err(last_error());
    }
    Ok((ReadPipe(read), write))
}

pub fn spawn(spec: SpawnSpec) -> Result<JobProcess, String> {
    let executable = wide(spec.executable.as_os_str())?;
    let cwd = wide(spec.cwd.as_os_str())?;
    // CMD scans forward slashes in argv[0] as switches even with lpApplicationName supplied.
    let mut command = quote_arg(
        &spec
            .executable
            .to_str()
            .ok_or("解释器路径不是有效 Unicode")?
            .replace('/', "\\"),
    )?;
    if let Some(payload) = &spec.cmd_payload {
        command.push_str(" /D /V:OFF /S /C ");
        command.push_str(payload);
    } else {
        for arg in &spec.arguments {
            command.push(' ');
            command.push_str(&quote_arg(arg)?);
        }
    }
    let mut command = wide(std::ffi::OsStr::new(&command))?;
    if command.len() > 32767 {
        return Err("命令行超过 Windows 32767 字符限制".into());
    }
    let mut environment = Vec::new();
    let mut entries: Vec<_> = spec.environment.into_iter().collect();
    entries.sort_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
    for (key, value) in entries {
        let mut pair = key;
        pair.push("=");
        pair.push(value);
        environment.extend(wide(&pair)?);
    }
    environment.push(0);
    if environment.len() == 1 {
        environment.push(0);
    }
    let job = Handle(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) });
    if job.0.is_null() {
        return Err(last_error());
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(last_error());
    }
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    let mut flags = CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT;
    let mut stdout = None;
    let mut stderr = None;
    let mut inherited = Vec::new();
    let mut attributes = None;
    let mut terminal = None;
    let mut terminal_input = None;
    if spec.console {
        flags |= CREATE_NEW_CONSOLE;
    } else if spec.terminal {
        // Null standard handles prevent redirected launcher handles bypassing ConPTY on Win10.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        let (input_read, input_write) = pipe()?;
        if unsafe { SetHandleInformation(input_write.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(last_error());
        }
        let (output_read, output_write) = pipe()?;
        if unsafe { SetHandleInformation(output_write.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(last_error());
        }
        let mut handle = 0;
        let result = unsafe {
            CreatePseudoConsole(
                COORD { X: 100, Y: 30 },
                input_read.0.as_raw_handle() as HANDLE,
                output_write.0,
                0,
                &mut handle,
            )
        };
        if result < 0 {
            return Err(format!("CreatePseudoConsole: {result:#x}"));
        }
        terminal = Some(Arc::new(PseudoConsole {
            handle,
            size: AtomicU64::new(0),
            closing: AtomicBool::new(false),
        }));
        attributes = Some(Attributes::terminal(handle)?);
        startup.lpAttributeList = attributes.as_mut().unwrap().list();
        flags |= EXTENDED_STARTUPINFO_PRESENT;
        stdout = Some(output_read);
        // ConPTY owns duplicated pipe ends; these local ends must close after process creation.
        inherited.push(Handle(input_read.0.as_raw_handle() as HANDLE));
        std::mem::forget(input_read);
        inherited.push(output_write);
        terminal_input = Some(unsafe { File::from_raw_handle(input_write.0) });
        std::mem::forget(input_write);
    } else {
        flags |= CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT;
        let (output, output_write) = pipe()?;
        let (error, error_write) = pipe()?;
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        let nul = wide(std::ffi::OsStr::new("NUL"))?;
        let input = Handle(unsafe {
            CreateFileW(
                nul.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &security,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        });
        if input.0 == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = input.0;
        startup.StartupInfo.hStdOutput = output_write.0;
        startup.StartupInfo.hStdError = error_write.0;
        inherited = vec![input, output_write, error_write];
        let mut handles: Vec<_> = inherited.iter().map(|h| h.0).collect();
        attributes = Some(Attributes::new(&mut handles)?);
        startup.lpAttributeList = attributes.as_mut().unwrap().list();
        stdout = Some(output);
        stderr = Some(error);
    }
    if spec.console {
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    }
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            executable.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            (!spec.console && !spec.terminal) as i32,
            flags,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(format!("CreateProcessW: {}", last_error()));
    }
    let process = Handle(info.hProcess);
    let thread = Handle(info.hThread);
    drop(inherited);
    drop(attributes);
    if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
        let error = last_error();
        unsafe {
            TerminateProcess(process.0, 1);
            WaitForSingleObject(process.0, 3000);
        }
        return Err(format!("JOB_ASSIGN_FAILED: {error}"));
    }
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        let error = last_error();
        unsafe {
            TerminateJobObject(job.0, 1);
        }
        return Err(format!("ResumeThread: {error}"));
    }
    if let Some(terminal) = terminal.as_ref().cloned() {
        std::thread::Builder::new()
            .name("terminal-resize".into())
            .spawn(move || {
                let mut last = 0;
                while !terminal.closing.load(Ordering::Acquire) {
                    let size = terminal.size.load(Ordering::Acquire);
                    if size != 0 && size != last {
                        // Resizing may block behind ConPTY output; the supervisor must keep draining.
                        unsafe {
                            ResizePseudoConsole(
                                terminal.handle,
                                COORD {
                                    X: (size >> 16) as i16,
                                    Y: size as i16,
                                },
                            );
                        }
                        last = size;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            })
            .map_err(|e| format!("创建终端尺寸线程失败: {e}"))?;
    }
    Ok(JobProcess {
        job,
        process,
        pid: info.dwProcessId,
        stdout,
        stderr,
        input: terminal_input,
        terminal,
    })
}

pub fn write_input(input: &mut File, bytes: &[u8]) -> Result<(), String> {
    input
        .write_all(bytes)
        .map_err(|e| format!("终端输入失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cmd_rejects_shell_syntax_before_spawn() {
        for value in [
            "%PATH%", "!VAR!", "a&b", "a|b", "a<b", "a>b", "a^b", "a\"b", "a(b)", "a\r", "a\n",
            "a\t", "a\0",
        ] {
            assert!(validate_cmd_value(value, "test").is_err(), "{value:?}");
        }
        for value in [
            "",
            "中文 文件",
            "C:\\Scripts\\路径 1.cmd",
            "--name=value",
            "tail\\",
        ] {
            assert!(validate_cmd_value(value, "test").is_ok(), "{value:?}");
        }
    }
    #[test]
    fn argv_quotes_empty_quotes_and_trailing_slashes() {
        assert_eq!(quote_arg("").unwrap(), "\"\"");
        assert_eq!(quote_arg("a\"b").unwrap(), "\"a\\\"b\"");
        assert_eq!(quote_arg("尾\\").unwrap(), "\"尾\\\\\"");
        assert!(quote_arg("\0").is_err());
    }
}
