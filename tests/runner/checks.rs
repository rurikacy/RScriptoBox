#![cfg(test)]
#[path = "../../src-tauri/src/process_windows.rs"]
mod process_windows;
mod terminal_text;

#[cfg(test)]
mod integration {
    use super::{process_windows::*, terminal_text};
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    fn python() -> PathBuf {
        if let Some(path) = std::env::var_os("RSCRIPTO_TEST_PYTHON") {
            return PathBuf::from(path);
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|p| p.join("python.exe"))
            .find(|p| p.is_file() && !p.to_string_lossy().contains("WindowsApps"))
            .expect("真实 Python 未安装，请设置 RSCRIPTO_TEST_PYTHON")
    }
    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }
    fn spec(executable: PathBuf, args: Vec<String>) -> SpawnSpec {
        let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        environment.insert("RSCRIPTO_TEST".into(), "中文 value".into());
        environment.insert("PYTHONIOENCODING".into(), "utf-8".into());
        SpawnSpec {
            executable,
            arguments: args,
            cwd: fixtures(),
            environment,
            console: false,
            terminal: false,
            cmd_payload: None,
        }
    }
    fn capture(mut process: JobProcess) -> (u32, Vec<u8>, Vec<u8>) {
        let read = |mut pipe: ReadPipe| {
            thread::spawn(move || {
                let start = Instant::now();
                let mut output = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    match pipe.read_available(&mut buffer).unwrap() {
                        Some(0) => return output,
                        Some(n) => output.extend_from_slice(&buffer[..n]),
                        None => thread::sleep(Duration::from_millis(5)),
                    };
                    assert!(start.elapsed() < Duration::from_secs(15), "pipe timeout");
                }
            })
        };
        let stdout = read(process.stdout.take().unwrap());
        let stderr = process.stderr.take().map(read);
        let start = Instant::now();
        while process.active_processes().unwrap() != 0 {
            assert!(start.elapsed() < Duration::from_secs(15), "process timeout");
            thread::sleep(Duration::from_millis(10));
        }
        process.close_terminal();
        (
            process.root_exit_code().unwrap().unwrap(),
            stdout.join().unwrap(),
            stderr
                .map(|reader| reader.join().unwrap())
                .unwrap_or_default(),
        )
    }
    #[test]
    fn conpty_owns_real_console_input_and_output() {
        let mut command = spec(python(), vec!["-u".into(), "-c".into(), "import os; print('TTY', os.isatty(0), os.isatty(1)); print('VALUE=' + input('READY>'))".into()]);
        command.terminal = true;
        let mut process = spawn(command).unwrap();
        process.resize(112, 35).unwrap();
        write_input(process.input.as_mut().unwrap(), b"roundtrip\r").unwrap();
        let (code, output, _) = capture(process);
        let output = terminal_text::text(&String::from_utf8(output).unwrap());
        assert_eq!(code, 0);
        assert!(output.contains("TTY True True"), "{output:?}");
        assert!(output.contains("VALUE=roundtrip"), "{output:?}");
    }
    #[test]
    fn actual_python_argv_roundtrip_cwd_environment_and_stderr() {
        let directory =
            std::env::temp_dir().join(format!("RScripto Python 中文 {}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let script = directory.join("参数 回显.py");
        fs::copy(fixtures().join("echo.py"), &script).unwrap();
        let expected = vec![
            "",
            "空 格",
            "a\"b",
            "tail\\",
            "a\\\"b",
            "%PATH%",
            "& | < > ! ^ ( )",
            "中文🙂",
        ];
        let mut args = vec!["-u".into(), script.to_string_lossy().into_owned()];
        args.extend(expected.iter().map(|s| s.to_string()));
        let mut command = spec(python(), args);
        command.cwd = directory.clone();
        let (exit, stdout, stderr) = capture(spawn(command).unwrap());
        assert_eq!(exit, 0);
        let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(value["args"], serde_json::json!(expected));
        assert_eq!(value["env"], "中文 value");
        assert_eq!(value["cwd"], directory.to_string_lossy().as_ref());
        assert!(String::from_utf8(stderr).unwrap().contains("stderr 可见"));
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn actual_cmd_argv_with_spaces_chinese_empty_and_trailing_slash() {
        let root = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let directory = std::env::temp_dir().join(format!("RScripto 中文 {}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let script = directory.join("echo 中文.cmd");
        fs::copy(fixtures().join("echo.cmd"), &script).unwrap();
        let args = vec!["空 格".into(), "".into(), "tail\\".into(), "中文".into()];
        let mut command = spec(root.join("System32/cmd.exe"), vec![]);
        command.cwd = directory.clone();
        command.cmd_payload = Some(cmd_payload(&script, &args, &directory).unwrap());
        let (exit, stdout, stderr) = capture(spawn(command).unwrap());
        let stdout = String::from_utf8(stdout).unwrap();
        assert_eq!(
            exit,
            0,
            "stdout={stdout}, stderr={}",
            String::from_utf8_lossy(&stderr)
        );
        for expected in [
            "first=[空 格]",
            "second=[]",
            "third=[tail\\]",
            "fourth=[中文]",
        ] {
            assert!(stdout.contains(expected), "{stdout}");
        }
        assert!(!stderr.is_empty());
        let batch = directory.join("echo 中文.bat");
        fs::copy(&script, &batch).unwrap();
        let mut command = spec(root.join("System32/cmd.exe"), vec![]);
        command.cwd = directory.clone();
        command.cmd_payload = Some(cmd_payload(&batch, &args, &directory).unwrap());
        let (batch_exit, batch_stdout, _) = capture(spawn(command).unwrap());
        assert_eq!(batch_exit, 0);
        assert_eq!(String::from_utf8(batch_stdout).unwrap(), stdout);
        fs::remove_dir_all(directory).unwrap();
    }
    fn powershell(executable: &Path) {
        let directory = std::env::temp_dir().join(format!(
            "RScripto PowerShell 中文 {} {}",
            std::process::id(),
            executable.file_stem().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&directory).unwrap();
        let script = directory.join("参数 回显.ps1");
        fs::copy(fixtures().join("echo.ps1"), &script).unwrap();
        let mut args = vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            script.to_string_lossy().into_owned(),
        ];
        let expected = [
            "空 格",
            "",
            "tail\\",
            "a\"b",
            "a\\\"b",
            "$env:RSCRIPTO_TEST",
            "& | ; < > ( )",
            "'单引号'",
        ];
        args.extend(expected.iter().map(|value| value.to_string()));
        let mut command = spec(executable.to_path_buf(), args);
        command.cwd = directory.clone();
        let (exit, stdout, stderr) = capture(spawn(command).unwrap());
        assert_eq!(exit, 0, "{}", String::from_utf8_lossy(&stderr));
        let result: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(result["args"], serde_json::json!(expected));
        assert_eq!(result["env"], "中文 value");
        assert_eq!(result["cwd"], directory.to_string_lossy().as_ref());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn actual_windows_powershell_file_arguments() {
        powershell(
            &PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        );
    }
    #[test]
    fn actual_powershell7_file_arguments() {
        powershell(Path::new("C:/Program Files/PowerShell/7/pwsh.exe"));
    }
    #[test]
    fn simultaneous_large_stdout_stderr_do_not_deadlock() {
        let (exit, stdout, stderr) = capture(
            spawn(spec(
                python(),
                vec![
                    "-u".into(),
                    fixtures().join("flood.py").to_string_lossy().into_owned(),
                ],
            ))
            .unwrap(),
        );
        assert_eq!(exit, 0);
        assert_eq!(stdout.len(), 2 * 1024 * 1024);
        assert_eq!(stderr.len(), 2 * 1024 * 1024);
    }
    #[test]
    fn new_console_has_real_input_and_no_redirected_output() {
        let mut command = spec(
            python(),
            vec![
                "-u".into(),
                fixtures()
                    .join("console_input.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
        );
        command.console = true;
        let process = spawn(command).unwrap();
        assert!(process.stdout.is_none());
        assert!(process.stderr.is_none());
        let start = Instant::now();
        while process.active_processes().unwrap() != 0 {
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(process.root_exit_code().unwrap(), Some(0));
    }
    #[test]
    fn parent_exits_first_and_entire_job_is_terminated() {
        let mut process = spawn(spec(
            python(),
            vec![
                "-u".into(),
                fixtures().join("tree.py").to_string_lossy().into_owned(),
            ],
        ))
        .unwrap();
        let start = Instant::now();
        while process.root_exit_code().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            process.active_processes().unwrap() > 0,
            "root exit must not mean completion"
        );
        let mut buffer = [0; 8192];
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("child-live") {
            if let Some(n) = process
                .stdout
                .as_mut()
                .unwrap()
                .read_available(&mut buffer)
                .unwrap()
            {
                output.extend_from_slice(&buffer[..n]);
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(10));
        }
        assert!(process.active_processes().unwrap() >= 2);
        process.terminate().unwrap();
        let start = Instant::now();
        while process.active_processes().unwrap() != 0 {
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn dropping_last_job_handle_kills_children() {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        let process = spawn(spec(
            python(),
            vec![
                "-u".into(),
                "-c".into(),
                "import time; time.sleep(60)".into(),
            ],
        ))
        .unwrap();
        let pid = process.pid;
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null());
        drop(process);
        assert_eq!(unsafe { WaitForSingleObject(handle, 3000) }, WAIT_OBJECT_0);
        unsafe {
            CloseHandle(handle);
        }
    }
    #[test]
    fn cmd_injection_inputs_never_spawn() {
        let marker = fixtures().join("injection-marker.txt");
        let script = fixtures().join("echo.cmd");
        for value in [
            "%PATH%",
            "!VAR!",
            "& echo injected > injection-marker.txt",
            "|whoami",
            "<file",
            ">file",
            "^",
            "\"",
            "(echo x)",
            "\r",
            "\n",
        ] {
            assert!(cmd_payload(&script, &[value.into()], &fixtures()).is_err());
        }
        assert!(!marker.exists());
    }
}
