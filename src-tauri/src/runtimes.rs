use crate::models::{
    new_id, now, AppResult, Environment, InstallJob, InstallPlan, Runtime, Script,
};
use crate::storage::Store;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PROBE_LIMIT: usize = 256 * 1024;
const PYTHON_PROBE: &str = r#"import sys,json,struct,platform,os
try:
 import pip
 pip_version=pip.__version__
except Exception:
 pip_version=None
print(json.dumps(dict(product='python',implementation=sys.implementation.name,executable=sys.executable,version=platform.python_version(),architecture='x64' if struct.calcsize('P')==8 and platform.machine().lower() not in ('arm64','aarch64') else platform.machine().lower(),prefix=sys.prefix,base_prefix=sys.base_prefix,pip=pip_version,devnull=os.devnull)))"#;
const POWERSHELL_PROBE: &str = "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);[pscustomobject]@{product='powershell';version=$PSVersionTable.PSVersion.ToString();prerelease=[string]$PSVersionTable.PSVersion.PreReleaseLabel;edition=[string]$PSVersionTable.PSEdition;architecture=if([IntPtr]::Size -eq 8){'x64'}else{'x86'};executable=[Diagnostics.Process]::GetCurrentProcess().MainModule.FileName;policy=[string](Get-ExecutionPolicy)}|ConvertTo-Json -Compress";
const REGISTRY_PROBE: &str = r#"[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);$paths=@();foreach($root in @('Registry::HKEY_CURRENT_USER\Software\Python','Registry::HKEY_LOCAL_MACHINE\Software\Python','Registry::HKEY_LOCAL_MACHINE\Software\WOW6432Node\Python')){Get-ChildItem -LiteralPath $root -ErrorAction SilentlyContinue|ForEach-Object{Get-ChildItem -LiteralPath $_.PSPath -ErrorAction SilentlyContinue|ForEach-Object{$key=Get-Item -LiteralPath ($_.PSPath+'\InstallPath') -ErrorAction SilentlyContinue;if($key){$exe=$key.GetValue('ExecutablePath');if(!$exe){$exe=Join-Path $key.GetValue('') 'python.exe'};$paths+=([string]$exe)}}}};ConvertTo-Json -InputObject @($paths) -Compress"#;

#[derive(Clone)]
struct ConfirmedPlan {
    visible: InstallPlan,
    winget: Option<PathBuf>,
    manager: Option<PathBuf>,
}

pub struct RuntimeService {
    store: Arc<Store>,
    plans: Mutex<HashMap<String, ConfirmedPlan>>,
    mutation: Mutex<()>,
    live_jobs: Mutex<HashSet<String>>,
    log_warnings: Mutex<HashMap<String, String>>,
}

impl RuntimeService {
    pub fn new(store: Arc<Store>) -> Self {
        // A restarted application cannot infer that an external installer has stopped from its old PID.
        if let Ok(jobs) = store.list::<InstallJob>("install_jobs") {
            for mut job in jobs {
                if matches!(job.state.as_str(), "installing" | "verifying" | "creating") {
                    job.state = "verification_required".into();
                    job.message =
                        "上次环境变更被应用中断，请确认外部安装器已结束，再复检解除锁定。".into();
                    let _ = store.put("install_jobs", &job.id, &job);
                }
            }
        }
        Self {
            store,
            plans: Mutex::new(HashMap::new()),
            mutation: Mutex::new(()),
            live_jobs: Mutex::new(HashSet::new()),
            log_warnings: Mutex::new(HashMap::new()),
        }
    }

    pub fn jobs(&self) -> AppResult<Vec<InstallJob>> {
        let mut jobs = self.store.list::<InstallJob>("install_jobs")?;
        jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(jobs)
    }

    pub fn environments(&self) -> AppResult<Vec<Environment>> {
        self.store.list("environments")
    }

    pub fn has_mutation_lock(&self) -> AppResult<bool> {
        Ok(self.jobs()?.iter().any(|job| unresolved(&job.state)))
    }

    pub fn is_environment_locked(&self, id: &str) -> AppResult<bool> {
        Ok(self
            .jobs()?
            .iter()
            .any(|job| job.environment_id.as_deref() == Some(id) && unresolved(&job.state)))
    }

    pub fn discover(&self) -> AppResult<Vec<Runtime>> {
        let previous = self.store.list::<Runtime>("runtimes")?;
        let mut candidates: Vec<(String, PathBuf, String)> = previous
            .iter()
            .map(|r| {
                (
                    r.kind.clone(),
                    PathBuf::from(&r.executable_path),
                    r.source.clone(),
                )
            })
            .collect();
        let system = system_directory();
        candidates.push(("cmd".into(), system.join("cmd.exe"), "system".into()));
        let windows_ps = system.join("WindowsPowerShell/v1.0/powershell.exe");
        candidates.push((
            "windows_powershell".into(),
            windows_ps.clone(),
            "system".into(),
        ));
        for path in executable_candidates("pwsh.exe") {
            candidates.push(("powershell".into(), path, "path".into()));
        }
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push((
                "powershell".into(),
                PathBuf::from(program_files).join("PowerShell/7/pwsh.exe"),
                "known_location".into(),
            ));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let apps = PathBuf::from(local).join("Microsoft/WindowsApps");
            candidates.push((
                "powershell".into(),
                apps.join("pwsh.exe"),
                "app_alias".into(),
            ));
        }
        for path in appx_executables("powershell") {
            candidates.push(("powershell".into(), path, "appx_registration".into()));
        }
        for manager in manager_candidates() {
            if let Ok(result) = probe(&manager, &["list", "--format=json"], Duration::from_secs(5))
            {
                if result.code == Some(0) {
                    for path in manager_json_paths(&result.stdout) {
                        candidates.push(("python".into(), path, "python_manager".into()));
                    }
                }
            }
        }
        if windows_ps.is_file() {
            if let Ok(result) = probe(
                &windows_ps,
                &[
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    REGISTRY_PROBE,
                ],
                Duration::from_secs(5),
            ) {
                if let Ok(paths) = serde_json::from_slice::<Vec<String>>(&result.stdout) {
                    for path in paths {
                        candidates.push((
                            "python".into(),
                            PathBuf::from(path),
                            "pep514_registry".into(),
                        ));
                    }
                }
            }
        }
        for name in ["python.exe", "python3.exe"] {
            for path in executable_candidates(name) {
                if !is_store_python_alias(&path) {
                    candidates.push(("python".into(), path, "path".into()));
                }
            }
        }
        let mut seen = HashSet::new();
        let mut discovered = Vec::new();
        for (kind, path, source) in candidates {
            let key = normalized_path(&path);
            if !seen.insert((kind.clone(), key.clone())) {
                continue;
            }
            let old = previous
                .iter()
                .find(|r| r.kind == kind && normalized_path(Path::new(&r.executable_path)) == key);
            if !path.is_file() && old.is_none() {
                continue;
            }
            let runtime = match verify_runtime(&kind, &path, &source) {
                Ok(mut runtime) => {
                    if let Some(old) = old {
                        runtime.id = old.id.clone();
                    }
                    runtime
                }
                Err(_) => match old {
                    Some(old) => {
                        let mut runtime = old.clone();
                        runtime.status = "unavailable".into();
                        runtime.checked_at = now();
                        runtime
                    }
                    None => continue,
                },
            };
            if discovered.iter().any(|existing: &Runtime| {
                existing.kind == runtime.kind
                    && normalized_path(Path::new(&existing.executable_path))
                        == normalized_path(Path::new(&runtime.executable_path))
            }) {
                continue;
            }
            self.store.put("runtimes", &runtime.id, &runtime)?;
            discovered.push(runtime);
        }
        discovered.sort_by(|a, b| {
            (&a.kind, &a.version, &a.executable_path).cmp(&(
                &b.kind,
                &b.version,
                &b.executable_path,
            ))
        });
        Ok(discovered)
    }

    pub fn register(&self, kind: &str, path: &str) -> AppResult<Runtime> {
        let path = checked_file(path)?;
        if is_store_python_alias(&path) && kind == "python" {
            return Err("RUNTIME_UNSUPPORTED:请选择实际 Python 解释器，WindowsApps 的商店别名可能触发安装。".into());
        }
        let mut runtime = verify_runtime(kind, &path, "manual")?;
        if let Some(existing) = self
            .store
            .list::<Runtime>("runtimes")?
            .into_iter()
            .find(|r| {
                r.kind == kind
                    && normalized_path(Path::new(&r.executable_path))
                        == normalized_path(Path::new(&runtime.executable_path))
            })
        {
            runtime.id = existing.id;
        }
        self.store.put("runtimes", &runtime.id, &runtime)?;
        if kind == "python"
            && runtime.prefix != runtime.base_prefix
            && !self.environments()?.iter().any(|env| {
                normalized_path(Path::new(&env.interpreter_path)) == normalized_path(&path)
            })
        {
            let environment = Environment {
                id: new_id(),
                base_runtime_id: runtime.id.clone(),
                interpreter_path: runtime.executable_path.clone(),
                ownership: "external".into(),
                requirements_path: None,
                requirements_hash: None,
                dependency_status: "ready".into(),
                created_at: now(),
                checked_at: now(),
            };
            self.store
                .put("environments", &environment.id, &environment)?;
        }
        Ok(runtime)
    }

    pub fn plan_install(&self, recipe: &str, version: Option<String>) -> AppResult<InstallPlan> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:环境锁不可用。")?;
        if self.has_mutation_lock()? {
            return Err("ENVIRONMENT_BUSY:已有环境变更或待复检记录，请先处理。".into());
        }
        let (product, target_version, source, package_id, license_url) = match recipe {
            "powershell" => ("PowerShell 7", "stable".to_string(), "winget", "Microsoft.PowerShell", "https://github.com/PowerShell/PowerShell/blob/master/LICENSE.txt"),
            "python" => {
                let target = version.unwrap_or_else(|| "3.14".into());
                if !["3.12", "3.13", "3.14"].contains(&target.as_str()) {
                    return Err("INVALID_ARGUMENT:Python 安装版本只能选择 3.12、3.13 或 3.14。".into());
                }
                ("Python Install Manager + Python", target, "msstore", "9NQ7512CXL7T", "https://docs.python.org/3/license.html")
            }
            _ => return Err("RUNTIME_UNSUPPORTED:只支持安装 PowerShell 7 和 Python；CMD 与 Windows PowerShell 是系统组件。".into()),
        };
        let manager = if recipe == "python" {
            verified_manager()
        } else {
            None
        };
        let winget = if manager.is_some() {
            None
        } else {
            Some(executable_candidates("winget.exe").into_iter().find(|path| {
                probe(path, &["--version"], Duration::from_secs(5)).map(|r| r.code == Some(0)).unwrap_or(false)
            }).ok_or_else(|| "WINGET_MISSING:未找到可用 WinGet，请安装 Microsoft App Installer，或从官方下载运行时后手动登记。".to_string())?)
        };
        let query = if let Some(winget) = &winget {
            probe(
                winget,
                &[
                    "show",
                    "--id",
                    package_id,
                    "--exact",
                    "--source",
                    source,
                    "--disable-interactivity",
                ],
                Duration::from_secs(25),
            )
        } else {
            let tag = format!("{target_version}-64");
            probe(
                manager
                    .as_deref()
                    .ok_or("RUNTIME_MISSING:Python 管理器不存在。")?,
                &[
                    "list",
                    "--format=json",
                    "--source=https://www.python.org/ftp/python/index-windows.json",
                    &tag,
                ],
                Duration::from_secs(25),
            )
        };
        if manager.is_some() {
            if let Ok(result) = &query {
                if result.code == Some(0)
                    && serde_json::from_slice::<Value>(&result.stdout)
                        .ok()
                        .and_then(|data| data["versions"].as_array().map(Vec::is_empty))
                        .unwrap_or(true)
                {
                    return Err(
                        "INSTALL_NOT_APPLICABLE:Python 官方索引中没有选定版本的 x64 运行时。"
                            .into(),
                    );
                }
            }
        }
        let query_note = match query {
            Ok(result) if result.code == Some(0) => format!(
                "已查询安装源：{}",
                String::from_utf8_lossy(&result.stdout)
                    .chars()
                    .take(2400)
                    .collect::<String>()
            ),
            Ok(result) if result.code == Some(0x8A150046) => format!(
                "安装源要求接受下列协议，确认后才接受并查询目标：\n{}\n{}",
                String::from_utf8_lossy(&result.stdout)
                    .chars()
                    .take(2400)
                    .collect::<String>(),
                String::from_utf8_lossy(&result.stderr)
                    .chars()
                    .take(1000)
                    .collect::<String>()
            ),
            Ok(result) => {
                return Err(format!(
                    "INSTALL_SOURCE_UNAVAILABLE:无法查询安装配方（{}）。{}",
                    exit_display(result.code),
                    String::from_utf8_lossy(&result.stderr)
                ))
            }
            Err(error) => return Err(format!("INSTALL_SOURCE_UNAVAILABLE:{error}")),
        };
        let source = if manager.is_some() {
            "python.org"
        } else {
            source
        };
        let package_id = if manager.is_some() {
            "PythonCore"
        } else {
            package_id
        };
        let install_action = if manager.is_some() {
            "使用已验证的 pymanager 从 Python 官方索引安装选定运行时；保留现有管理器。"
        } else {
            "确认后通过 WinGet 安装；Python 另使用 pymanager 安装所选版本并解析实际 python.exe。"
        };
        let warning = if manager.is_some() {
            "确认即接受 Python 产品许可；需要联网下载选定运行时。启动后不保证能取消或回滚，请等待结果或复检。"
        } else {
            "确认即接受产品许可和所选 WinGet 源协议。需要联网，Windows 可能请求 UAC、商店确认或重启。启动后不保证能取消或回滚，请等待结果或复检。"
        };
        let plan = InstallPlan {
            id: new_id(),
            recipe_key: recipe.into(),
            product: product.into(),
            version: target_version,
            architecture: "x64".into(),
            scope: "user".into(),
            source: source.into(),
            package_id: package_id.into(),
            license_url: license_url.into(),
            actions: vec![
                query_note,
                install_action.into(),
                "重新检测版本、架构与可执行路径，不自动运行脚本。".into(),
            ],
            warning: warning.into(),
        };
        self.plans
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:计划锁不可用。")?
            .insert(
                plan.id.clone(),
                ConfirmedPlan {
                    visible: plan.clone(),
                    winget,
                    manager,
                },
            );
        Ok(plan)
    }

    pub fn start_install(self: &Arc<Self>, plan_id: &str) -> AppResult<InstallJob> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:环境锁不可用。")?;
        if self.has_mutation_lock()? {
            return Err("ENVIRONMENT_BUSY:已有环境变更或待复检记录。".into());
        }
        let plan = self
            .plans
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:计划锁不可用。")?
            .remove(plan_id)
            .ok_or_else(|| "INVALID_ARGUMENT:安装计划不存在或已使用，请重新生成。".to_string())?;
        let job = self.new_job(&plan.visible.recipe_key, &plan.visible.version, None, None)?;
        self.launch(job.clone(), move |service, job| {
            service.execute_install(job, plan)
        });
        Ok(job)
    }

    pub fn cancel(&self, id: &str) -> AppResult<InstallJob> {
        if self
            .plans
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:计划锁不可用。")?
            .remove(id)
            .is_some()
        {
            return Ok(InstallJob {
                id: id.into(),
                kind: "plan".into(),
                target: "".into(),
                state: "cancelled".into(),
                message: "尚未启动安装，已取消计划。".into(),
                raw_exit_code: None,
                created_at: now(),
                finished_at: Some(now()),
                environment_id: None,
                script_id: None,
            });
        }
        let mut job = self.get_job(id)?;
        if unresolved(&job.state) {
            job.message = "外部安装器已启动，无法保证立即取消或回滚。请在安装器中取消，结束后使用复检确认实际状态。".into();
        }
        Ok(job)
    }

    pub fn recheck(&self, id: &str, installer_closed: bool) -> AppResult<InstallJob> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:环境锁不可用。")?;
        let mut job = self.get_job(id)?;
        if !unresolved(&job.state) {
            return Ok(job);
        }
        if self
            .live_jobs
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:任务锁不可用。")?
            .contains(id)
        {
            return Err("ENVIRONMENT_BUSY:安装进程仍在运行，请等待结束。".into());
        }
        if !installer_closed && unresolved(&job.state) {
            return Err(
                "INSTALL_VERIFICATION_REQUIRED:必须先确认外部安装器已关闭，再复检并解除锁定。"
                    .into(),
            );
        }
        if matches!(job.kind.as_str(), "venv" | "requirements") {
            let env_id = job
                .environment_id
                .as_ref()
                .ok_or("INVALID_ARGUMENT:环境记录缺失。")?;
            let mut env = self
                .store
                .get::<Environment>("environments", env_id)?
                .ok_or("RUNTIME_MISSING:环境不存在。")?;
            match verify_runtime("python", Path::new(&env.interpreter_path), "environment") {
                Ok(runtime)
                    if runtime.has_pip
                        && runtime.prefix != runtime.base_prefix
                        && (job.kind != "venv"
                            || (env.ownership == "application"
                                && runtime.version == job.target
                                && runtime
                                    .prefix
                                    .as_ref()
                                    .map(|prefix| {
                                        normalized_path(Path::new(prefix))
                                            == normalized_path(
                                                &self.store.root.join("envs").join(&env.id),
                                            )
                                    })
                                    .unwrap_or(false))) =>
                {
                    if job.kind == "venv" && job.raw_exit_code == Some(0) {
                        env.dependency_status = "ready".into();
                        self.store.put("environments", &env.id, &env)?;
                        self.bind_environment(&job, &env)?;
                        job.state = "ready".into();
                        job.message = "虚拟环境验证通过，已绑定脚本。".into();
                    } else {
                        env.dependency_status = "failed".into();
                        self.store.put("environments", &env.id, &env)?;
                        job.state = "failed".into();
                        job.message = "解释器可用，但中断的依赖变更无法证明成功；已解除全局锁，环境保持失败锁定，请重新安装清单修复。".into();
                    }
                }
                _ => {
                    env.dependency_status = "failed".into();
                    self.store.put("environments", &env.id, &env)?;
                    job.state = "failed".into();
                    job.message = "目标环境不可用，请新建虚拟环境或重新登记。".into();
                }
            }
        } else {
            let runtimes = self.discover()?;
            if matching_runtime(&runtimes, &job.kind, &job.target).is_some() {
                job.state = "ready".into();
                job.message = "运行时复检通过，已解除环境变更锁。".into();
            } else {
                job.state = "failed".into();
                job.message = "已确认安装器结束，但未发现目标版本。请检查诊断日志，重试安装或手动登记解释器。".into();
            }
        }
        job.finished_at = Some(now());
        self.store.put("install_jobs", id, &job)?;
        Ok(job)
    }

    pub fn create_venv(
        self: &Arc<Self>,
        script_id: &str,
        base_runtime_id: &str,
    ) -> AppResult<InstallJob> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:环境锁不可用。")?;
        if self.has_mutation_lock()? {
            return Err("ENVIRONMENT_BUSY:已有环境变更或待复检记录。".into());
        }
        let script = self
            .store
            .get::<Script>("scripts", script_id)?
            .ok_or("SCRIPT_MISSING:脚本不存在。")?;
        if script.kind != "python" {
            return Err("INVALID_ARGUMENT:只有 Python 脚本可以创建 venv。".into());
        }
        let base = self
            .store
            .get::<Runtime>("runtimes", base_runtime_id)?
            .ok_or("RUNTIME_MISSING:基础解释器不存在。")?;
        let mut base = verify_runtime("python", Path::new(&base.executable_path), &base.source)?;
        base.id = base_runtime_id.into();
        self.store.put("runtimes", base_runtime_id, &base)?;
        if base.prefix != base.base_prefix {
            return Err(
                "INVALID_ARGUMENT:请选择基础 Python 解释器，不要从另一个 venv 创建。".into(),
            );
        }
        let id = new_id();
        let final_directory = self.store.root.join("envs").join(&id);
        fs::create_dir_all(
            final_directory
                .parent()
                .ok_or("INVALID_ARGUMENT:环境根目录不存在。")?,
        )
        .map_err(io_error)?;
        if final_directory.exists() {
            return Err("ENVIRONMENT_BUSY:环境目录已存在。".into());
        }
        let env = Environment {
            id: id.clone(),
            base_runtime_id: base_runtime_id.into(),
            interpreter_path: final_directory
                .join("Scripts/python.exe")
                .to_string_lossy()
                .into_owned(),
            ownership: "application".into(),
            requirements_path: None,
            requirements_hash: None,
            dependency_status: "creating".into(),
            created_at: now(),
            checked_at: now(),
        };
        self.store.put("environments", &id, &env)?;
        let job = self.new_job("venv", &base.version, Some(id), Some(script_id.into()))?;
        self.launch(job.clone(), move |service, job| {
            service.execute_venv(job, base, env, final_directory)
        });
        Ok(job)
    }

    pub fn install_requirements(
        self: &Arc<Self>,
        environment_id: &str,
        path: &str,
    ) -> AppResult<InstallJob> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "ENVIRONMENT_BUSY:环境锁不可用。")?;
        if self.has_mutation_lock()? {
            return Err("ENVIRONMENT_BUSY:已有环境变更或待复检记录。".into());
        }
        let path = checked_file(path)?;
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| !s.eq_ignore_ascii_case("txt"))
            .unwrap_or(true)
        {
            return Err("INVALID_ARGUMENT:请明确选择 requirements 文本文件（.txt）。".into());
        }
        if fs::metadata(&path).map_err(io_error)?.len() > 4 * 1024 * 1024 {
            return Err("INVALID_ARGUMENT:依赖清单不能超过 4 MiB。".into());
        }
        let mut env = self
            .store
            .get::<Environment>("environments", environment_id)?
            .ok_or("RUNTIME_MISSING:环境不存在。")?;
        let runtime = verify_runtime("python", Path::new(&env.interpreter_path), "environment")?;
        if runtime.prefix == runtime.base_prefix || !runtime.has_pip {
            return Err("RUNTIME_UNSUPPORTED:依赖只能安装到已验证且有 pip 的虚拟环境，不能写入系统 Python。".into());
        }
        env.dependency_status = "installing".into();
        env.requirements_path = Some(path.to_string_lossy().into_owned());
        env.requirements_hash = Some(format!(
            "{:x}",
            Sha256::digest(fs::read(&path).map_err(io_error)?)
        ));
        self.store.put("environments", &env.id, &env)?;
        let job = self.new_job(
            "requirements",
            &runtime.version,
            Some(environment_id.into()),
            None,
        )?;
        self.launch(job.clone(), move |service, job| {
            service.execute_requirements(job, env, path)
        });
        Ok(job)
    }

    pub fn read_install_log(&self, id: &str) -> AppResult<String> {
        let job = self.get_job(id)?;
        if job.id != id || !safe_id(id) {
            return Err("INVALID_ARGUMENT:任务 ID 无效。".into());
        }
        let directory = self.store.root.join("install-logs").join(id);
        let mut text = String::new();
        for stream in ["stdout", "stderr"] {
            let path = directory.join(format!("{stream}.bin"));
            if !path.exists() {
                continue;
            }
            let mut file = File::open(&path).map_err(io_error)?;
            let size = file.metadata().map_err(io_error)?.len();
            if size > 1024 * 512 {
                file.seek(SeekFrom::End(-(1024 * 512))).map_err(io_error)?;
            }
            let mut bytes = Vec::new();
            file.take(1024 * 512)
                .read_to_end(&mut bytes)
                .map_err(io_error)?;
            text.push_str(&format!(
                "[{stream}]\n{}\n",
                String::from_utf8_lossy(&bytes)
            ));
        }
        Ok(text)
    }

    fn get_job(&self, id: &str) -> AppResult<InstallJob> {
        self.store
            .get("install_jobs", id)?
            .ok_or_else(|| "INVALID_ARGUMENT:环境任务不存在。".into())
    }

    fn new_job(
        &self,
        kind: &str,
        target: &str,
        env: Option<String>,
        script: Option<String>,
    ) -> AppResult<InstallJob> {
        let job = InstallJob {
            id: new_id(),
            kind: kind.into(),
            target: target.into(),
            state: "installing".into(),
            message: "环境变更已启动，请等待完成。".into(),
            raw_exit_code: None,
            created_at: now(),
            finished_at: None,
            environment_id: env,
            script_id: script,
        };
        fs::create_dir_all(self.store.root.join("install-logs").join(&job.id)).map_err(io_error)?;
        self.store.put("install_jobs", &job.id, &job)?;
        Ok(job)
    }

    fn launch<F>(self: &Arc<Self>, job: InstallJob, work: F)
    where
        F: FnOnce(&RuntimeService, &mut InstallJob) -> AppResult<()> + Send + 'static,
    {
        if let Ok(mut live) = self.live_jobs.lock() {
            live.insert(job.id.clone());
        }
        let service = self.clone();
        thread::spawn(move || {
            let mut job = job;
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&service, &mut job)));
            match result {
                Ok(Ok(())) => (),
                Ok(Err(error)) => {
                    if unresolved(&job.state) {
                        job.state = "verification_required".into();
                    }
                    job.message = error;
                }
                Err(_) => {
                    job.state = "verification_required".into();
                    job.message = "环境任务意外中断，请确认安装器已结束后复检。".into();
                }
            }
            if let Ok(mut warnings) = service.log_warnings.lock() {
                if let Some(warning) = warnings.remove(&job.id) {
                    job.message.push_str(&format!("\n{warning}"));
                }
            }
            if !unresolved(&job.state) {
                job.finished_at = Some(now());
            }
            let _ = service.store.put("install_jobs", &job.id, &job);
            if let Ok(mut live) = service.live_jobs.lock() {
                live.remove(&job.id);
            }
        });
    }

    fn execute_install(&self, job: &mut InstallJob, plan: ConfirmedPlan) -> AppResult<()> {
        let p = &plan.visible;
        if let Some(winget) = &plan.winget {
            let mut query = hidden_command(winget);
            query.args([
                "show",
                "--id",
                &p.package_id,
                "--exact",
                "--source",
                &p.source,
                "--accept-source-agreements",
                "--disable-interactivity",
            ]);
            if self.logged_command(job, &mut query)? != 0 {
                job.state = "failed".into();
                return Err("INSTALL_SOURCE_UNAVAILABLE:确认后的源查询失败，请查看诊断。".into());
            }
            let mut install = hidden_command(winget);
            install.args([
                "install",
                "--id",
                &p.package_id,
                "--exact",
                "--source",
                &p.source,
                "--scope",
                "user",
                "--accept-package-agreements",
                "--accept-source-agreements",
                "--disable-interactivity",
                "--no-upgrade",
            ]);
            if p.recipe_key == "powershell" {
                install.args(["--architecture", "x64"]);
            }
            let code = self.logged_command(job, &mut install)?;
            job.raw_exit_code = Some(code);
            if !matches!(code, 0 | 0x8A150061 | 0x8A15002B) {
                let (state, message) = installation_failure(code);
                job.state = state.into();
                job.message = message.into();
                return Ok(());
            }
        }
        if p.recipe_key == "python" {
            let manager = plan.manager.or_else(verified_manager)
                .ok_or("INSTALL_VERIFICATION_REQUIRED:管理器安装结果需复检：未找到可用 pymanager。请检查应用执行别名与安装器状态。")?;
            let mut command = hidden_command(&manager);
            clean_manager_environment(&mut command);
            let tag = format!("{}-64", p.version);
            command.args([
                "install",
                "--source=https://www.python.org/ftp/python/index-windows.json",
                &tag,
            ]);
            let code = self.logged_command(job, &mut command)?;
            job.raw_exit_code = Some(code);
            if code != 0 {
                job.state = "failed".into();
                job.message = format!(
                    "PYTHON_INSTALL_FAILED:Python {} 安装失败（{}），查看诊断日志。",
                    p.version,
                    exit_display(Some(code))
                );
                return Ok(());
            }
        }
        job.state = "verifying".into();
        job.message = "安装器已返回，正在验证实际运行时。".into();
        self.store.put("install_jobs", &job.id, job)?;
        let runtimes = self.discover()?;
        if matching_runtime(&runtimes, &p.recipe_key, &p.version).is_some() {
            job.state = "ready".into();
            job.message = "运行时已安装并验证，可在脚本配置中选择；脚本未自动运行。".into();
        } else {
            job.state = "verification_required".into();
            job.message =
                "安装器返回成功，但目标运行时尚未通过验证，请确认安装已结束后复检。".into();
        }
        Ok(())
    }

    fn execute_venv(
        &self,
        job: &mut InstallJob,
        base: Runtime,
        mut env: Environment,
        final_directory: PathBuf,
    ) -> AppResult<()> {
        let mut command = hidden_command(Path::new(&base.executable_path));
        command
            .args(["-I", "-X", "utf8", "-m", "venv"])
            .arg(&final_directory);
        let code = self.logged_command(job, &mut command)?;
        job.raw_exit_code = Some(code);
        let verified = verify_runtime(
            "python",
            Path::new(&env.interpreter_path),
            "application_venv",
        );
        match verified {
            Ok(runtime)
                if code == 0
                    && runtime.has_pip
                    && runtime.version == base.version
                    && runtime.prefix != runtime.base_prefix
                    && runtime
                        .prefix
                        .as_ref()
                        .map(|prefix| {
                            normalized_path(Path::new(prefix)) == normalized_path(&final_directory)
                        })
                        .unwrap_or(false) =>
            {
                env.dependency_status = "ready".into();
                env.checked_at = now();
                self.store.put("environments", &env.id, &env)?;
                self.bind_environment(job, &env)?;
                job.state = "ready".into();
                job.message = "venv 已在最终目录创建并验证，已绑定脚本。".into();
            }
            _ => {
                env.dependency_status = "failed".into();
                env.checked_at = now();
                self.store.put("environments", &env.id, &env)?;
                job.state = "failed".into();
                job.message = format!(
                    "VENV_CREATE_FAILED:虚拟环境未通过验证（{}）；原绑定保持不变，可新建环境重试。",
                    exit_display(Some(code))
                );
            }
        }
        Ok(())
    }

    fn bind_environment(&self, job: &InstallJob, env: &Environment) -> AppResult<()> {
        if let Some(id) = &job.script_id {
            if let Some(mut script) = self.store.get::<Script>("scripts", id)? {
                script.environment_id = Some(env.id.clone());
                script.runtime_id = Some(env.base_runtime_id.clone());
                script.updated_at = now();
                self.store.put("scripts", id, &script)?;
            }
        }
        Ok(())
    }

    fn execute_requirements(
        &self,
        job: &mut InstallJob,
        mut env: Environment,
        path: PathBuf,
    ) -> AppResult<()> {
        let before_hash = env.requirements_hash.clone();
        let mut command = pip_install_command(Path::new(&env.interpreter_path), &path);
        let code = self.logged_command(job, &mut command)?;
        job.raw_exit_code = Some(code);
        let same_file = fs::read(&path)
            .map(|bytes| Some(format!("{:x}", Sha256::digest(bytes))) == before_hash)
            .unwrap_or(false);
        if code == 0
            && same_file
            && verify_runtime("python", Path::new(&env.interpreter_path), "environment")
                .map(|r| r.has_pip)
                .unwrap_or(false)
        {
            let mut check = hidden_command(Path::new(&env.interpreter_path));
            configure_pip(&mut check);
            check.args([
                "-I",
                "-X",
                "utf8",
                "-m",
                "pip",
                "--isolated",
                "--require-virtualenv",
                "--no-input",
                "check",
            ]);
            let check_code = self.logged_command(job, &mut check)?;
            if check_code == 0 {
                env.dependency_status = "ready".into();
                job.state = "ready".into();
                job.message = "依赖安装完成，pip check 通过。".into();
            } else {
                env.dependency_status = "failed".into();
                job.state = "failed".into();
                job.message =
                    "DEPENDENCY_CHECK_FAILED:pip check 发现依赖冲突，环境保持锁定，请修复后重试。"
                        .into();
            }
        } else {
            env.dependency_status = "failed".into();
            job.state = "failed".into();
            job.message = if !same_file {
                "REQUIREMENTS_CHANGED:安装期间依赖清单发生变化，环境保持锁定，请重新选择清单安装。"
                    .into()
            } else {
                format!("DEPENDENCY_INSTALL_FAILED:依赖安装失败（{}）；环境可能已部分改变，修复前禁止运行。", exit_display(Some(code)))
            };
        }
        env.checked_at = now();
        self.store.put("environments", &env.id, &env)?;
        Ok(())
    }

    fn logged_command(&self, job: &mut InstallJob, command: &mut Command) -> AppResult<u32> {
        let directory = self.store.root.join("install-logs").join(&job.id);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("INSTALL_START_FAILED:{error}"))?;
        let settings = self.store.settings()?;
        let previous_bytes = ["stdout.bin", "stderr.bin"]
            .iter()
            .map(|name| {
                fs::metadata(directory.join(name))
                    .map(|m| m.len())
                    .unwrap_or(0)
            })
            .sum();
        let counter = Arc::new(Mutex::new((previous_bytes, false, false)));
        let single_limit = settings.log_limit_mib.min(20) * 1024 * 1024;
        let total_limit = settings.total_log_mib * 1024 * 1024;
        let out = stream_log(
            child
                .stdout
                .take()
                .ok_or("INSTALL_LOG_FAILED:stdout 不可用。")?,
            directory.join("stdout.bin"),
            self.store.clone(),
            counter.clone(),
            single_limit,
            total_limit,
        );
        let err = stream_log(
            child
                .stderr
                .take()
                .ok_or("INSTALL_LOG_FAILED:stderr 不可用。")?,
            directory.join("stderr.bin"),
            self.store.clone(),
            counter.clone(),
            single_limit,
            total_limit,
        );
        let status = child.wait().map_err(|error| {
            format!("INSTALL_VERIFICATION_REQUIRED:无法确定安装器结果：{error}")
        })?;
        // Inherited handles can outlive an external installer; never wait forever for its descendant pipes.
        let out_done = out.recv_timeout(Duration::from_secs(2)).is_ok();
        let err_done = err.recv_timeout(Duration::from_secs(2)).is_ok();
        let flags = counter
            .lock()
            .map_err(|_| "INSTALL_LOG_FAILED:日志状态不可用。")?;
        if let Ok(mut warnings) = self.log_warnings.lock() {
            if flags.2 {
                warnings.insert(
                    job.id.clone(),
                    "诊断日志写入失败，已停止保存；外部安装器未被终止。".into(),
                );
            } else if flags.1 {
                warnings.insert(
                    job.id.clone(),
                    "诊断日志达到配额，已截断；安装输出仍被排空。".into(),
                );
            }
        }
        drop(flags);
        let code = status.code().map(|code| code as u32).unwrap_or(u32::MAX);
        job.raw_exit_code = Some(code);
        if !out_done || !err_done {
            job.state = "verification_required".into();
            return Err("INSTALL_VERIFICATION_REQUIRED:外部进程仍持有安装输出管道，请确认安装器已结束后复检。".into());
        }
        Ok(code)
    }
}

fn stream_log<R: Read + Send + 'static>(
    mut reader: R,
    path: PathBuf,
    store: Arc<Store>,
    counter: Arc<Mutex<(u64, bool, bool)>>,
    single_limit: u64,
    total_limit: u64,
) -> mpsc::Receiver<()> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut file = OpenOptions::new().create(true).append(true).open(path).ok();
        if file.is_none() {
            if let Ok(mut flags) = counter.lock() {
                flags.2 = true;
            }
        }
        let mut buffer = [0u8; 8192];
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            if let Ok(mut flags) = counter.lock() {
                if flags.2 || flags.1 {
                    continue;
                }
                if flags.0.saturating_add(count as u64) > single_limit
                    || !store.reserve_log_bytes(count as u64, total_limit)
                {
                    flags.1 = true;
                    continue;
                }
                if file
                    .as_mut()
                    .map(|file| file.write_all(&buffer[..count]).is_err())
                    .unwrap_or(true)
                {
                    flags.2 = true;
                    file = None;
                } else {
                    flags.0 += count as u64;
                }
            }
        }
        drop(file);
        drop(store);
        let _ = sender.send(());
    });
    receiver
}

struct ProbeOutput {
    code: Option<u32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn probe(path: &Path, args: &[&str], timeout: Duration) -> AppResult<ProbeOutput> {
    let mut child = hidden_command(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("RUNTIME_PROBE_FAILED:{}：{error}", path.display()))?;
    let out = capped_read(
        child
            .stdout
            .take()
            .ok_or("RUNTIME_PROBE_FAILED:stdout 不可用。")?,
    );
    let err = capped_read(
        child
            .stderr
            .take()
            .ok_or("RUNTIME_PROBE_FAILED:stderr 不可用。")?,
    );
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "RUNTIME_PROBE_TIMEOUT:探测 {} 超时。",
                path.display()
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let remaining = timeout.saturating_sub(started.elapsed());
    Ok(ProbeOutput {
        code: status.code().map(|code| code as u32),
        stdout: out
            .recv_timeout(remaining)
            .map_err(|_| "RUNTIME_PROBE_TIMEOUT:stdout 探测超时。")?,
        stderr: err
            .recv_timeout(timeout.saturating_sub(started.elapsed()))
            .map_err(|_| "RUNTIME_PROBE_TIMEOUT:stderr 探测超时。")?,
    })
}

fn capped_read<R: Read + Send + 'static>(mut reader: R) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut data = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    let take = size.min(PROBE_LIMIT.saturating_sub(data.len()));
                    data.extend_from_slice(&buffer[..take]);
                }
            }
        }
        let _ = sender.send(data);
    });
    receiver
}

pub fn verify_runtime(kind: &str, path: &Path, source: &str) -> AppResult<Runtime> {
    if !path.is_file() {
        return Err("RUNTIME_MISSING:解释器文件不存在。".into());
    }
    let path = checked_file(&path.to_string_lossy())?;
    let mut execution_policy = None;
    let (version, architecture, actual_path, prefix, base_prefix, has_pip) = match kind {
        "python" => {
            if is_store_python_alias(&path) {
                return Err("RUNTIME_UNSUPPORTED:不执行 WindowsApps Python 商店别名。".into());
            }
            let result = probe(&path, &["-I", "-c", PYTHON_PROBE], Duration::from_secs(5))?;
            let data: Value = serde_json::from_slice(&result.stdout)
                .map_err(|_| "RUNTIME_UNSUPPORTED:不是可验证的 Python 解释器。")?;
            let version = field(&data, "version")?;
            if result.code != Some(0)
                || field(&data, "product")? != "python"
                || !version.starts_with("3.")
            {
                return Err("RUNTIME_UNSUPPORTED:只支持 Python 3 解释器。".into());
            }
            (
                version,
                field(&data, "architecture")?,
                field(&data, "executable")?,
                Some(field(&data, "prefix")?),
                Some(field(&data, "base_prefix")?),
                data["pip"].is_string(),
            )
        }
        "powershell" | "windows_powershell" => {
            let result = probe(
                &path,
                &[
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    POWERSHELL_PROBE,
                ],
                Duration::from_secs(5),
            )?;
            let data: Value = serde_json::from_slice(&result.stdout).map_err(|error| {
                format!(
                    "RUNTIME_UNSUPPORTED:PowerShell 探测未返回有效 JSON（{}，{error}）：{}",
                    exit_display(result.code),
                    String::from_utf8_lossy(&result.stderr)
                        .chars()
                        .take(2000)
                        .collect::<String>()
                )
            })?;
            execution_policy = data["policy"].as_str().map(str::to_owned);
            let version = field(&data, "version")?;
            let edition = field(&data, "edition")?;
            if result.code != Some(0)
                || field(&data, "product")? != "powershell"
                || (kind == "powershell" && (edition != "Core" || !version.starts_with("7.")))
                || (kind == "windows_powershell"
                    && (edition != "Desktop" || !version.starts_with("5.1.")))
            {
                return Err("RUNTIME_UNSUPPORTED:PowerShell 产品或版本与所选引擎不一致。".into());
            }
            let version = match data["prerelease"]
                .as_str()
                .filter(|label| !label.is_empty())
            {
                Some(label) if !version.contains('-') => format!("{version}-{label}"),
                _ => version,
            };
            (
                version,
                field(&data, "architecture")?,
                field(&data, "executable")?,
                None,
                None,
                false,
            )
        }
        "cmd" => {
            let result = probe(&path, &["/d", "/q", "/c", "ver"], Duration::from_secs(5))?;
            if result.code != Some(0) || result.stdout.is_empty() {
                return Err("RUNTIME_UNSUPPORTED:CMD 探测失败。".into());
            }
            let expected = system_directory().join("cmd.exe");
            if normalized_path(&path) != normalized_path(&expected) {
                return Err("RUNTIME_UNSUPPORTED:CMD 必须来自 Windows 系统目录。".into());
            }
            let text = String::from_utf8_lossy(&result.stdout);
            let version = text
                .split(|c: char| !(c.is_ascii_digit() || c == '.'))
                .find(|part| part.matches('.').count() >= 2)
                .unwrap_or("Windows CMD")
                .into();
            (
                version,
                pe_architecture(&path)?,
                path.to_string_lossy().into_owned(),
                None,
                None,
                false,
            )
        }
        _ => return Err("RUNTIME_UNSUPPORTED:不支持的运行时种类。".into()),
    };
    let actual_path = checked_file(&actual_path)?.to_string_lossy().into_owned();
    let actual_architecture = pe_architecture(Path::new(&actual_path))?;
    if architecture != "x64" || actual_architecture != "x64" {
        return Err(format!("RUNTIME_UNSUPPORTED:首版只支持 x64 解释器，检测到 {architecture}/{actual_architecture}。"));
    }
    Ok(Runtime {
        id: new_id(),
        kind: kind.into(),
        executable_path: actual_path,
        version,
        architecture,
        source: source.into(),
        status: "ready".into(),
        prefix,
        base_prefix,
        has_pip,
        checked_at: now(),
        execution_policy,
    })
}

fn field(data: &Value, name: &str) -> AppResult<String> {
    data[name]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 4096)
        .map(str::to_owned)
        .ok_or_else(|| format!("RUNTIME_UNSUPPORTED:探测字段 {name} 无效。"))
}

fn checked_file(path: &str) -> AppResult<PathBuf> {
    if path.encode_utf16().count() > 240
        || path.contains('\0')
        || path.starts_with("\\\\")
        || path.starts_with("//")
    {
        return Err("INVALID_PATH:只支持不超过 240 字符的本地磁盘路径。".into());
    }
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_file() {
        return Err("INVALID_PATH:请选择存在的本地文件绝对路径。".into());
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    let display = canonical.to_string_lossy();
    if display.starts_with("\\\\?\\UNC\\") {
        return Err("INVALID_PATH:不支持 UNC 文件。".into());
    }
    let canonical = PathBuf::from(display.strip_prefix("\\\\?\\").unwrap_or(&display));
    Ok(canonical)
}

fn normalized_path(path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path.to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .replace('/', "\\")
        .to_lowercase()
}

fn is_store_python_alias(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    text.contains("\\microsoft\\windowsapps\\")
        && path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase().starts_with("python"))
            .unwrap_or(false)
}

fn executable_candidates(name: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join(name))
            .filter(|p| p.is_file())
            .collect();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let alias = PathBuf::from(local)
            .join("Microsoft/WindowsApps")
            .join(name);
        if alias.is_file() {
            paths.push(alias);
        }
    }
    paths
}

fn manager_candidates() -> Vec<PathBuf> {
    let mut candidates = executable_candidates("pymanager.exe");
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        for suffix in [
            "PythonSoftwareFoundation.PythonManager_3847v3x7pw1km",
            "PythonSoftwareFoundation.PythonManager_qbz5n2kfra8p0",
        ] {
            let path = PathBuf::from(&local)
                .join("Microsoft/WindowsApps")
                .join(suffix)
                .join("pymanager.exe");
            if path.is_file() {
                candidates.push(path);
            }
        }
    }
    if candidates.is_empty() {
        candidates.extend(appx_executables("python_manager"));
    }
    candidates
}

fn appx_executables(kind: &str) -> Vec<PathBuf> {
    let (package, executable) = match kind {
        "powershell" => ("Microsoft.PowerShell", "pwsh.exe"),
        "python_manager" => ("PythonSoftwareFoundation.PythonManager", "pymanager.exe"),
        _ => return Vec::new(),
    };
    let script = format!("[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);$paths=@(Get-AppxPackage -Name '{package}' -ErrorAction SilentlyContinue|ForEach-Object{{Join-Path $_.InstallLocation '{executable}'}});ConvertTo-Json -InputObject $paths -Compress");
    probe(
        &system_directory().join("WindowsPowerShell/v1.0/powershell.exe"),
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ],
        Duration::from_secs(5),
    )
    .ok()
    .and_then(|output| serde_json::from_slice::<Vec<String>>(&output.stdout).ok())
    .unwrap_or_default()
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| path.is_file())
    .collect()
}

fn verified_manager() -> Option<PathBuf> {
    let usable = |path: &PathBuf| {
        probe(path, &["list", "--format=json"], Duration::from_secs(5))
            .map(|result| {
                result.code == Some(0)
                    && serde_json::from_slice::<Value>(&result.stdout)
                        .map(|data| data["versions"].is_array())
                        .unwrap_or(false)
            })
            .unwrap_or(false)
    };
    manager_candidates()
        .into_iter()
        .find(usable)
        .or_else(|| appx_executables("python_manager").into_iter().find(usable))
}

fn manager_json_paths(bytes: &[u8]) -> Vec<PathBuf> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|data| data["versions"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["executable"].as_str())
        .map(PathBuf::from)
        .filter(|p| !is_store_python_alias(p))
        .collect()
}

fn hidden_command(path: &Path) -> Command {
    let mut command = Command::new(path);
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("powershell.exe") || name.eq_ignore_ascii_case("pwsh.exe")
        })
    {
        let modules = path.parent().unwrap_or(Path::new("")).join("Modules");
        if modules.is_dir() {
            // Cross-engine launches can inherit PSModulePath from an incompatible PowerShell host.
            let mut module_path = modules.into_os_string();
            if let Some(inherited) = std::env::var_os("PSModulePath") {
                module_path.push(";");
                module_path.push(inherited);
            }
            command.env("PSModulePath", module_path);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

fn system_directory() -> PathBuf {
    #[cfg(windows)]
    {
        let mut buffer = [0u16; 32768];
        let length = unsafe {
            windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut buffer))
        };
        if length > 0 && (length as usize) < buffer.len() {
            return PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
        }
    }
    PathBuf::from("C:/Windows/System32")
}

fn pe_architecture(path: &Path) -> AppResult<String> {
    let mut file = File::open(path).map_err(io_error)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(io_error)?;
    if &header[..2] != b"MZ" {
        return Err("RUNTIME_UNSUPPORTED:不是 Windows PE 程序。".into());
    }
    let offset = u32::from_le_bytes(
        header[60..64]
            .try_into()
            .map_err(|_| "RUNTIME_UNSUPPORTED:PE 头无效。")?,
    );
    if offset > 16 * 1024 * 1024 {
        return Err("RUNTIME_UNSUPPORTED:PE 偏移无效。".into());
    }
    file.seek(SeekFrom::Start(offset as u64))
        .map_err(io_error)?;
    let mut signature = [0u8; 6];
    file.read_exact(&mut signature).map_err(io_error)?;
    if &signature[..4] != b"PE\0\0" {
        return Err("RUNTIME_UNSUPPORTED:PE 签名无效。".into());
    }
    Ok(match u16::from_le_bytes([signature[4], signature[5]]) {
        0x8664 => "x64",
        0xaa64 => "arm64",
        0x014c => "x86",
        _ => "unknown",
    }
    .into())
}

fn configure_pip(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("PIP_")
            || name.to_string_lossy().eq_ignore_ascii_case("PYTHONPATH")
            || name.to_string_lossy().eq_ignore_ascii_case("PYTHONHOME")
        {
            command.env_remove(name);
        }
    }
    command.env("PIP_CONFIG_FILE", "nul");
}

fn clean_manager_environment(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("PYTHON_MANAGER_")
        {
            command.env_remove(name);
        }
    }
    command.env("PYTHON_MANAGER_AUTOMATIC_INSTALL", "false");
}

fn pip_install_command(executable: &Path, requirements: &Path) -> Command {
    let mut command = hidden_command(executable);
    configure_pip(&mut command);
    command
        .args([
            "-I",
            "-X",
            "utf8",
            "-m",
            "pip",
            "--isolated",
            "--require-virtualenv",
            "--no-input",
            "--disable-pip-version-check",
            "install",
            "-r",
        ])
        .arg(requirements);
    if let Some(parent) = requirements.parent() {
        command.current_dir(parent);
    }
    command
}

fn matching_runtime<'a>(runtimes: &'a [Runtime], kind: &str, target: &str) -> Option<&'a Runtime> {
    runtimes.iter().find(|r| {
        r.kind == kind
            && r.status == "ready"
            && r.version.chars().all(|c| c.is_ascii_digit() || c == '.')
            && (kind != "python" || r.version.starts_with(&format!("{target}.")))
            && (kind != "python" || r.prefix == r.base_prefix)
    })
}

fn installation_failure(code: u32) -> (&'static str, &'static str) {
    match code {
        3010 | 1641 | 0x8A150109 | 0x8A15010A | 0x8A15010B => ("reboot_required", "INSTALL_REBOOT_REQUIRED:安装要求重启，请重启后确认安装器结束并复检。"),
        1223 | 0x800704C7 | 0x8A150005 | 0x8A15010C => ("verification_required", "INSTALL_CANCELLED:安装器报告取消或 UAC 被拒绝，请确认其已关闭并复检；未承诺回滚。"),
        0x8A15001B | 0x8A15001C | 0x8A15003A | 0x8A15010F => ("failed", "INSTALL_POLICY_BLOCKED:企业策略限制安装，请联系设备管理员或手动登记已允许的解释器。"),
        0x8A150008 | 0x8A150045 | 0x8A15004B | 0x80072EE7 | 0x80072EE2 | 0x8A150107 => ("failed", "INSTALL_SOURCE_UNAVAILABLE:下载或安装源不可达，请检查网络后重试。"),
        0x8A150010 | 0x8A150014 | 0x8A150017 | 0x8A150113 => ("failed", "INSTALL_NOT_APPLICABLE:安装源没有适合此设备的配方，请使用官方安装页面或手动登记。"),
        0x8A150056 | 0x8A15007D => ("failed", "INSTALL_ELEVATION_NOT_SUPPORTED:此安装器不能从管理员会话启动，请以普通用户权限重新打开应用。"),
        0x8A150019 => ("failed", "INSTALL_REQUIRES_ADMIN:安装器要求管理员权限，首版不自动提权；请通过官方下载或设备管理员部署后手动登记。"),
        0x8A150105 => ("failed", "INSTALL_DISK_FULL:磁盘空间不足，请释放空间后重试，并检查是否留下未完成安装。"),
        _ => ("verification_required", "INSTALL_VERIFICATION_REQUIRED:安装器返回未知结果，请查看原始退出码和日志，确认安装器结束后复检。"),
    }
}

fn unresolved(state: &str) -> bool {
    matches!(
        state,
        "installing" | "verifying" | "creating" | "verification_required" | "reboot_required"
    )
}
fn safe_id(id: &str) -> bool {
    id.len() <= 64 && !id.is_empty() && id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
}
fn exit_display(code: Option<u32>) -> String {
    code.map(|c| format!("{c} / 0x{c:08X}"))
        .unwrap_or_else(|| "未知退出码".into())
}
fn io_error(error: std::io::Error) -> String {
    format!("IO_ERROR:{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_json_uses_actual_executables_and_rejects_store_aliases() {
        let paths = manager_json_paths(br#"{"versions":[{"executable":"C:\\Python314\\python.exe"},{"executable":"C:\\Users\\A\\AppData\\Local\\Microsoft\\WindowsApps\\python.exe"},{"prefix":"C:\\not-an-executable"}]}"#);
        assert_eq!(paths, vec![PathBuf::from("C:\\Python314\\python.exe")]);
    }

    #[test]
    fn unresolved_installer_survives_cancellation_and_reboot() {
        for state in [
            "installing",
            "verifying",
            "verification_required",
            "reboot_required",
        ] {
            assert!(unresolved(state));
        }
        for state in ["ready", "failed", "cancelled"] {
            assert!(!unresolved(state));
        }
        assert_eq!(installation_failure(0x800704C7).0, "verification_required");
        assert_eq!(installation_failure(3010).0, "reboot_required");
    }

    #[test]
    fn pip_is_targeted_to_selected_environment_and_config_disabled() {
        let command = pip_install_command(
            Path::new("C:/env/Scripts/python.exe"),
            Path::new("C:/project/requirements.txt"),
        );
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-I",
                "-X",
                "utf8",
                "-m",
                "pip",
                "--isolated",
                "--require-virtualenv",
                "--no-input",
                "--disable-pip-version-check",
                "install",
                "-r",
                "C:/project/requirements.txt"
            ]
        );
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "PIP_CONFIG_FILE"
                && value == Some(std::ffi::OsStr::new("nul"))));
        assert_eq!(command.get_current_dir(), Some(Path::new("C:/project")));
    }

    #[test]
    fn rejects_network_device_and_relative_paths() {
        for path in [
            "relative/python.exe",
            "\\\\server\\share\\python.exe",
            "\\\\?\\C:\\python.exe",
            "\\\\.\\pipe\\x",
        ] {
            assert!(checked_file(path).is_err());
        }
        assert!(!safe_id("../elsewhere"));
    }

    #[test]
    fn restart_keeps_unresolved_installer_locked_and_cancel_does_not_claim_rollback() {
        let root = std::env::temp_dir().join(format!("rscripto-recovery-{}", new_id()));
        let store = Arc::new(Store::open(root.clone()).expect("temporary store"));
        let job = InstallJob {
            id: new_id(),
            kind: "python".into(),
            target: "3.14".into(),
            state: "installing".into(),
            message: "in progress".into(),
            raw_exit_code: None,
            created_at: now(),
            finished_at: None,
            environment_id: None,
            script_id: None,
        };
        store.put("install_jobs", &job.id, &job).unwrap();
        let service = RuntimeService::new(store.clone());
        assert!(service.has_mutation_lock().unwrap());
        assert_eq!(
            service.get_job(&job.id).unwrap().state,
            "verification_required"
        );
        assert_eq!(
            service.cancel(&job.id).unwrap().state,
            "verification_required"
        );
        assert!(service
            .recheck(&job.id, false)
            .unwrap_err()
            .starts_with("INSTALL_VERIFICATION_REQUIRED:"));
        drop(service);
        drop(store);
        let canonical = fs::canonicalize(root).unwrap();
        assert!(canonical.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(canonical).unwrap();
    }

    #[test]
    fn diagnostic_quota_drains_the_stream_without_exceeding_its_limit() {
        let root = std::env::temp_dir().join(format!("rscripto-diagnostic-{}", new_id()));
        let store = Arc::new(Store::open(root.clone()).expect("temporary store"));
        let counter = Arc::new(Mutex::new((0, false, false)));
        let path = root.join("limited.bin");
        let done = stream_log(
            std::io::Cursor::new(vec![b'x'; 100_000]),
            path.clone(),
            store.clone(),
            counter.clone(),
            8192,
            8192,
        );
        done.recv_timeout(Duration::from_secs(5))
            .expect("drained diagnostic reader");
        assert!(fs::metadata(path).unwrap().len() <= 8192);
        assert!(counter.lock().unwrap().1);
        assert!(!counter.lock().unwrap().2);
        drop(store);
        let canonical = fs::canonicalize(root).unwrap();
        assert!(canonical.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(canonical).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn system_interpreters_probe_without_policy_changes() {
        let system = system_directory();
        let cmd = verify_runtime("cmd", &system.join("cmd.exe"), "test").expect("system CMD");
        assert_eq!(cmd.kind, "cmd");
        let ps = verify_runtime(
            "windows_powershell",
            &system.join("WindowsPowerShell/v1.0/powershell.exe"),
            "test",
        )
        .expect("Windows PowerShell");
        assert!(ps.version.starts_with("5.1."));
        assert!(ps
            .execution_policy
            .as_deref()
            .is_some_and(|policy| !policy.is_empty()));
    }

    #[cfg(windows)]
    #[test]
    fn discovery_validates_real_system_and_existing_python_candidates() {
        let root = std::env::temp_dir().join(format!("rscripto-discovery-{}", new_id()));
        let store = Arc::new(Store::open(root.clone()).expect("temporary store"));
        let service = RuntimeService::new(store.clone());
        let runtimes = service.discover().expect("runtime discovery");
        assert!(runtimes
            .iter()
            .any(|r| r.kind == "cmd" && r.status == "ready"));
        assert!(runtimes
            .iter()
            .any(|r| r.kind == "windows_powershell" && r.status == "ready"));
        for runtime in &runtimes {
            assert!(Path::new(&runtime.executable_path).is_absolute());
            assert_eq!(runtime.architecture, "x64");
            if runtime.kind == "python" {
                assert!(!is_store_python_alias(Path::new(&runtime.executable_path)));
                assert!(runtime.prefix.is_some());
            }
            eprintln!(
                "discovered {} {} {} ({})",
                runtime.kind, runtime.version, runtime.executable_path, runtime.source
            );
        }
        let python_exists = executable_candidates("python.exe")
            .iter()
            .any(|p| !is_store_python_alias(p));
        if python_exists {
            assert!(runtimes
                .iter()
                .any(|r| r.kind == "python" && r.status == "ready"));
        }
        drop(service);
        drop(store);
        let canonical = fs::canonicalize(root).unwrap();
        assert!(canonical.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(canonical).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn venv_uses_final_path_and_pip_ignores_target_configuration() {
        let Some(python) = executable_candidates("python.exe")
            .into_iter()
            .find(|path| {
                !is_store_python_alias(path) && verify_runtime("python", path, "test").is_ok()
            })
        else {
            eprintln!("No existing Python available; skipping offline venv integration test.");
            return;
        };
        let root = std::env::temp_dir().join(format!("rscripto-runtime-{}", new_id()));
        let store = Arc::new(Store::open(root.clone()).expect("temporary database"));
        let service = Arc::new(RuntimeService::new(store.clone()));
        let runtime = service
            .register("python", &python.to_string_lossy())
            .expect("existing interpreter");
        let script: Script = serde_json::from_value(serde_json::json!({"id":new_id(),"name":"test","path":root.join("test.py").to_string_lossy(),"kind":"python"})).expect("script fixture");
        store
            .put("scripts", &script.id, &script)
            .expect("save script");
        let job = service
            .create_venv(&script.id, &runtime.id)
            .expect("create venv");
        let job = wait_job(&service, &job.id);
        assert_eq!(job.state, "ready", "{}", job.message);
        let environment = service
            .environments()
            .expect("environments")
            .pop()
            .expect("new environment");
        let final_directory = root.join("envs").join(&environment.id);
        assert_eq!(
            Path::new(&environment.interpreter_path),
            final_directory.join("Scripts/python.exe")
        );
        let bound = store
            .get::<Script>("scripts", &script.id)
            .expect("script query")
            .expect("script");
        assert_eq!(
            bound.environment_id.as_deref(),
            Some(environment.id.as_str())
        );
        let project = root.join("fixture project 中文");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace")
            .join("tests/runtimes/make_pip_fixture.py");
        let status = hidden_command(&python)
            .arg("-I")
            .arg(fixture)
            .arg(&project)
            .arg(&final_directory)
            .status()
            .expect("wheel fixture");
        assert!(status.success());
        let job = service
            .install_requirements(
                &environment.id,
                &project.join("requirements.txt").to_string_lossy(),
            )
            .expect("install local wheel");
        let job = wait_job(&service, &job.id);
        assert_eq!(
            job.state,
            "ready",
            "{}\n{}",
            job.message,
            service.read_install_log(&job.id).unwrap_or_default()
        );
        let installed = probe(Path::new(&environment.interpreter_path), &["-I", "-c", "import rscripto_probe,sys; assert rscripto_probe.value == 'installed-in-selected-venv'; print(rscripto_probe.__file__); assert rscripto_probe.__file__.startswith(sys.prefix)"], Duration::from_secs(5)).expect("package import");
        assert_eq!(
            installed.code,
            Some(0),
            "{}",
            String::from_utf8_lossy(&installed.stderr)
        );
        assert!(!project.join("poisoned-target").exists());
        assert!(!project.join("poisoned-prefix").exists());
        fs::write(project.join("requirements.txt"), "./missing.whl\n").expect("broken requirement");
        let failed = service
            .install_requirements(
                &environment.id,
                &project.join("requirements.txt").to_string_lossy(),
            )
            .expect("start failing install");
        let failed = wait_job(&service, &failed.id);
        assert_eq!(failed.state, "failed");
        assert_eq!(
            store
                .get::<Environment>("environments", &environment.id)
                .unwrap()
                .unwrap()
                .dependency_status,
            "failed"
        );
        assert!(!service.has_mutation_lock().unwrap());
        drop(service);
        drop(store);
        let canonical = fs::canonicalize(&root).expect("temporary root");
        assert!(
            canonical.starts_with(fs::canonicalize(std::env::temp_dir()).expect("temp directory"))
        );
        fs::remove_dir_all(canonical).expect("remove owned test files");
    }

    #[cfg(windows)]
    fn wait_job(service: &Arc<RuntimeService>, id: &str) -> InstallJob {
        let started = Instant::now();
        loop {
            let job = service.get_job(id).expect("job");
            let live = service.live_jobs.lock().unwrap().contains(id);
            if !live && Arc::strong_count(service) == 1 {
                return job;
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "environment task timed out"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}
