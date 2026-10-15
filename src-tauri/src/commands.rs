use crate::{
    library,
    models::*,
    runner::Runner,
    runtimes::{verify_runtime, RuntimeService},
    storage::Store,
};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{ipc::Channel, AppHandle, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use windows::{
    core::PCWSTR,
    Win32::UI::{
        Shell::{IsUserAnAdmin, ShellExecuteW},
        WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONERROR, MB_OK, MB_YESNO, SW_SHOWNORMAL},
    },
};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub runner: Arc<Runner>,
    pub runtimes: Arc<RuntimeService>,
    gate: Arc<Mutex<()>>,
    portable: bool,
}
async fn work<T: Send + 'static>(
    f: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}
fn script(store: &Store, id: &str) -> AppResult<Script> {
    store
        .get("scripts", id)?
        .ok_or("SCRIPT_MISSING: 脚本不存在".into())
}
fn idle(state: &AppState) -> AppResult<()> {
    if state.runner.active_count() > 0 {
        return Err("ENVIRONMENT_BUSY: 请先结束活动脚本任务".into());
    }
    Ok(())
}
fn script_unlocked(state: &AppState, id: &str) -> AppResult<()> {
    if state.runtimes.jobs()?.iter().any(|job| {
        job.script_id.as_deref() == Some(id)
            && [
                "installing",
                "creating",
                "verifying",
                "verification_required",
                "reboot_required",
            ]
            .contains(&job.state.as_str())
    }) {
        return Err("ENVIRONMENT_BUSY: 此脚本的环境正在变更或等待复检，请稍后再修改".into());
    }
    Ok(())
}

#[tauri::command]
async fn list_scripts(state: State<'_, AppState>) -> AppResult<Vec<Script>> {
    let store = state.store.clone();
    work(move || {
        let mut values = store.load_scripts()?;
        for s in &mut values {
            s.exists = Path::new(&s.path).is_file();
        }
        values.sort_by(|a, b| b.favorite.cmp(&a.favorite).then(a.name.cmp(&b.name)));
        Ok(values)
    })
    .await
}
#[tauri::command]
async fn add_scripts(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> AppResult<serde_json::Value> {
    let store = state.store.clone();
    work(move || library::add(&store, paths)).await
}
#[tauri::command]
async fn update_script(state: State<'_, AppState>, script: Script) -> AppResult<Script> {
    let state = state.inner().clone();
    work(move || {
        let _gate = state.gate.lock().map_err(|e| e.to_string())?;
        script_unlocked(&state, &script.id)?;
        library::update(&state.store, script)
    })
    .await
}
#[tauri::command]
async fn remove_script(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let state = state.inner().clone();
    work(move || {
        let _gate = state.gate.lock().map_err(|e| e.to_string())?;
        script_unlocked(&state, &id)?;
        if state.runner.is_script_active(&id) {
            return Err("SCRIPT_BUSY: 请先结束此脚本的运行任务".into());
        }
        state.store.remove_script(&id)
    })
    .await
}
#[tauri::command]
async fn relocate_script(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> AppResult<Script> {
    let state = state.inner().clone();
    work(move || {
        let _gate = state.gate.lock().map_err(|e| e.to_string())?;
        script_unlocked(&state, &id)?;
        if state.runner.is_script_active(&id) {
            return Err("SCRIPT_BUSY: 请先结束此脚本的运行任务".into());
        }
        library::relocate(&state.store, &id, &path)
    })
    .await
}
#[tauri::command]
async fn open_script_location(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<()> {
    let store = state.store.clone();
    work(move || {
        let s = script(&store, &id)?;
        app.opener()
            .reveal_item_in_dir(library::local_path(&s.path, true)?)
            .map_err(|e| e.to_string())
    })
    .await
}
#[tauri::command]
async fn edit_script(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let store = state.store.clone();
    work(move || {
        let s = script(&store, &id)?;
        library::require_workspace_copy(&store, &s)?;
        let path = library::local_path(&s.path, true)?;
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let verb: Vec<u16> = "edit".encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(wide.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            Err(
                "EDITOR_UNAVAILABLE: 没有可用的 edit 编辑关联，请在系统中配置编辑器；未执行该脚本"
                    .into(),
            )
        } else {
            Ok(())
        }
    })
    .await
}

#[tauri::command]
async fn run_script(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    overrides: Option<Script>,
    secret_values: BTreeMap<String, String>,
    channel: Channel<RunEvent>,
) -> AppResult<String> {
    let state = state.inner().clone();
    work(move||{
        let _gate=state.gate.lock().map_err(|e|e.to_string())?;
        if state.runtimes.has_mutation_lock()? { return Err("ENVIRONMENT_BUSY: 运行环境正在变更或等待复检".into()); }
        if state.runner.is_script_active(&id) { return Err("SCRIPT_BUSY: 此脚本正在运行，请先结束当前任务".into()); }
        let original=script(&state.store,&id)?;
        let mut effective=overrides.unwrap_or_else(||original.clone());
        if effective.id!=original.id || effective.path!=original.path || effective.kind!=original.kind {
            return Err("INVALID_CONFIG: 临时配置不得改变脚本身份".into());
        }
        library::validate(&effective)?;
        library::require_workspace_copy(&state.store,&effective)?;
        let path=library::local_path(&effective.path,true)?;
        if !path.is_file() || library::kind(&path)?!=effective.kind { return Err("SCRIPT_MISSING: 文件类型已变化".into()); }
        let cwd=library::working_directory(&path,&effective.cwd)?;
        effective.cwd=cwd.to_string_lossy().into_owned();
        let runtime_id=effective.runtime_id.clone().ok_or("RUNTIME_MISSING: 请在脚本配置中选择运行环境")?;
        let saved:Runtime=state.store.get("runtimes",&runtime_id)?.ok_or("RUNTIME_MISSING: 运行环境记录不存在")?;
        if !library::compatible(&effective.kind,&saved.kind) { return Err("RUNTIME_UNSUPPORTED: 解释器与脚本类型不匹配".into()); }
        let mut runtime=verify_runtime(&saved.kind,Path::new(&saved.executable_path),"bound")?;
        if runtime.version!=saved.version || runtime.architecture!=saved.architecture {
            return Err("RUNTIME_CHANGED: 解释器版本或架构已变化，请刷新环境并重新确认绑定".into());
        }
        runtime.id=saved.id;
        if let Some(environment_id)=&effective.environment_id {
            if effective.kind!="python" {return Err("INVALID_CONFIG: 仅 Python 可绑定虚拟环境".into());}
            let environment:Environment=state.store.get("environments",environment_id)?.ok_or("RUNTIME_MISSING: 虚拟环境不存在")?;
            if environment.base_runtime_id!=runtime_id {return Err("INVALID_CONFIG: 虚拟环境与基础解释器不匹配".into());}
            if environment.dependency_status!="ready" || state.runtimes.is_environment_locked(environment_id)? {
                return Err("ENVIRONMENT_BUSY: 虚拟环境未就绪，请查看依赖安装记录".into());
            }
            if let (Some(requirements),Some(expected))=(&environment.requirements_path,&environment.requirements_hash) {
                if library::hash_file(Path::new(requirements))?!=*expected {
                    return Err("DEPENDENCIES_CHANGED: 依赖清单已变更，请明确重新安装依赖".into());
                }
            }
            let verified=verify_runtime("python",Path::new(&environment.interpreter_path),"venv")?;
            runtime.executable_path=verified.executable_path;
            runtime.prefix=verified.prefix;runtime.base_prefix=verified.base_prefix;
        }
        for environment in state.runtimes.environments()? {
            if library::same_file(Path::new(&environment.interpreter_path),Path::new(&runtime.executable_path)) {
                if environment.dependency_status!="ready" || state.runtimes.is_environment_locked(&environment.id)? {
                    return Err("ENVIRONMENT_BUSY: 此解释器对应的虚拟环境依赖未就绪".into());
                }
                if let (Some(requirements),Some(expected))=(&environment.requirements_path,&environment.requirements_hash) {
                    if library::hash_file(Path::new(requirements))?!=*expected {
                        return Err("DEPENDENCIES_CHANGED: 依赖清单已变更，请重新确认安装".into());
                    }
                }
            }
        }
        let hash=library::hash_file(&path)?;
        if original.trusted_hash.as_deref()!=Some(&hash) {
            let permission=if unsafe{IsUserAnAdmin()}.as_bool() {"管理员权限"}else{"当前用户权限"};
            let accepted=app.dialog().message(format!(
                "仅运行您信任的脚本。它能够访问当前用户的文件和网络。\n\n脚本：{}\n解释器：{}\n权限：{}\n\n入口文件首次运行或已变更。是否信任并运行？",
                effective.path,runtime.executable_path,permission))
                .title("确认脚本执行").kind(MessageDialogKind::Warning).buttons(MessageDialogButtons::OkCancel).blocking_show();
            if !accepted {return Err("CANCELLED: 已取消运行".into());}
            let mut persisted=original.clone();persisted.trusted_hash=Some(hash.clone());state.store.save_script(&persisted)?;
        }
        effective.trusted_hash=Some(hash);
        if secret_values.len()>384 || secret_values.values().any(|v|v.len()>16384 || v.contains('\0')) {
            return Err("INVALID_SECRET: 敏感输入过多或无效".into());
        }
        state.store.cleanup_logs()?;
        state.runner.start(effective,runtime,state.store.settings()?,secret_values,Some(channel))
    }).await
}
#[tauri::command]
async fn stop_run(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runner.stop(&id)
}
#[tauri::command]
async fn write_terminal_input(
    state: State<'_, AppState>,
    id: String,
    text: String,
) -> AppResult<()> {
    state.runner.input(&id, &text)
}
#[tauri::command]
async fn resize_terminal(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    state.runner.resize(&id, cols, rows)
}
#[tauri::command]
async fn ack_run_output(state: State<'_, AppState>, id: String, seq: u64) -> AppResult<()> {
    state.runner.ack(&id, seq)
}
#[tauri::command]
async fn list_runs(state: State<'_, AppState>) -> AppResult<Vec<Run>> {
    let store = state.store.clone();
    work(move || {
        let mut runs = store.load_runs()?;
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(runs)
    })
    .await
}
#[tauri::command]
async fn read_run_log(
    state: State<'_, AppState>,
    id: String,
    cursor: u64,
    encoding: String,
    limit: usize,
) -> AppResult<LogPage> {
    let runner = state.runner.clone();
    work(move || runner.read_log(&id, cursor, &encoding, limit.min(1024 * 1024))).await
}
#[tauri::command]
async fn export_run_log(
    state: State<'_, AppState>,
    id: String,
    path: String,
    encoding: String,
) -> AppResult<()> {
    let runner = state.runner.clone();
    work(move || {
        let target = library::local_path(&path, false)?;
        if !["txt", "log"].contains(&target.extension().and_then(|x| x.to_str()).unwrap_or("")) {
            return Err("INVALID_PATH: 日志导出仅支持 .txt 或 .log".into());
        }
        let first = runner.read_log(&id, 0, &encoding, 1024 * 1024)?;
        let mut destination =
            std::io::BufWriter::new(fs::File::create(target).map_err(|e| e.to_string())?);
        destination
            .write_all(first.text.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut cursor = first.next_cursor;
        if !first.has_more {
            return destination.flush().map_err(|e| e.to_string());
        }
        loop {
            let page = runner.read_log(&id, cursor, &encoding, 1024 * 1024)?;
            destination
                .write_all(page.text.as_bytes())
                .map_err(|e| e.to_string())?;
            if !page.has_more || page.next_cursor == cursor {
                break;
            }
            cursor = page.next_cursor;
        }
        destination.flush().map_err(|e| e.to_string())
    })
    .await
}
#[tauri::command]
async fn list_runtimes(state: State<'_, AppState>) -> AppResult<Vec<Runtime>> {
    let store = state.store.clone();
    work(move || store.load_runtimes()).await
}
#[tauri::command]
async fn refresh_runtimes(state: State<'_, AppState>) -> AppResult<Vec<Runtime>> {
    let service = state.runtimes.clone();
    work(move || service.discover()).await
}
#[tauri::command]
async fn register_runtime(
    state: State<'_, AppState>,
    kind: String,
    path: String,
) -> AppResult<Runtime> {
    let service = state.runtimes.clone();
    work(move || service.register(&kind, &path)).await
}
#[tauri::command]
async fn list_environments(state: State<'_, AppState>) -> AppResult<Vec<Environment>> {
    state.runtimes.environments()
}
#[tauri::command]
async fn plan_runtime_install(
    state: State<'_, AppState>,
    recipe: String,
    version: Option<String>,
) -> AppResult<InstallPlan> {
    let service = state.runtimes.clone();
    work(move || service.plan_install(&recipe, version)).await
}
#[tauri::command]
async fn start_runtime_install(
    state: State<'_, AppState>,
    plan_id: String,
) -> AppResult<InstallJob> {
    let state = state.inner().clone();
    work(move || {
        let _gate = state.gate.lock().map_err(|e| e.to_string())?;
        idle(&state)?;
        state.runtimes.start_install(&plan_id)
    })
    .await
}
#[tauri::command]
async fn list_install_jobs(state: State<'_, AppState>) -> AppResult<Vec<InstallJob>> {
    state.runtimes.jobs()
}
#[tauri::command]
async fn read_install_log(state: State<'_, AppState>, id: String) -> AppResult<String> {
    let service = state.runtimes.clone();
    work(move || service.read_install_log(&id)).await
}
#[tauri::command]
async fn cancel_runtime_install(state: State<'_, AppState>, id: String) -> AppResult<InstallJob> {
    state.runtimes.cancel(&id)
}
#[tauri::command]
async fn recheck_install(
    state: State<'_, AppState>,
    id: String,
    installer_closed: bool,
) -> AppResult<InstallJob> {
    let service = state.runtimes.clone();
    work(move || service.recheck(&id, installer_closed)).await
}
#[tauri::command]
async fn create_python_environment(
    state: State<'_, AppState>,
    script_id: String,
    base_runtime_id: String,
) -> AppResult<InstallJob> {
    let state = state.inner().clone();
    work(move || {
        let _gate = state.gate.lock().map_err(|e| e.to_string())?;
        idle(&state)?;
        state.runtimes.create_venv(&script_id, &base_runtime_id)
    })
    .await
}
#[tauri::command]
async fn install_requirements(
    state: State<'_, AppState>,
    environment_id: String,
    path: String,
) -> AppResult<InstallJob> {
    let state = state.inner().clone();
    work(move || {
        let _gate = state.gate.lock().map_err(|e| e.to_string())?;
        idle(&state)?;
        state.runtimes.install_requirements(&environment_id, &path)
    })
    .await
}
#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    state.store.settings()
}
#[tauri::command]
async fn update_settings(state: State<'_, AppState>, settings: Settings) -> AppResult<Settings> {
    let store = state.store.clone();
    work(move || {
        store.save_settings(&settings)?;
        Ok(settings)
    })
    .await
}
#[tauri::command]
async fn backup_database(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let store = state.store.clone();
    work(move || store.backup(&library::local_path(&path, false)?)).await
}
#[tauri::command]
async fn restore_database(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<()> {
    let state = state.inner().clone();
    work(move||{
        let _gate=state.gate.lock().map_err(|e|e.to_string())?;idle(&state)?;
        if state.runtimes.has_mutation_lock()? {return Err("ENVIRONMENT_BUSY: 请先完成未决安装复检".into());}
        if !app.dialog().message("将使用备份替换当前配置和历史。当前数据库会自动备份，原脚本和环境不会删除。是否继续？")
            .title("恢复数据库").kind(MessageDialogKind::Warning).buttons(MessageDialogButtons::OkCancel).blocking_show() {return Err("CANCELLED: 已取消恢复".into());}
        state.store.restore(&library::local_path(&path,true)?)
    }).await
}
#[tauri::command]
fn app_info(state: State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"dataDir":state.store.root.to_string_lossy(),
        "portable":state.portable,"elevated":unsafe{IsUserAnAdmin()}.as_bool()})
}
#[tauri::command]
fn open_data_directory(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    app.opener()
        .open_path(state.store.root.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn open_official_page(app: AppHandle, product: String) -> AppResult<()> {
    let url = match product.as_str() {
        "python" => "https://www.python.org/downloads/windows/",
        "powershell" => {
            "https://learn.microsoft.com/powershell/scripting/install/install-powershell-on-windows"
        }
        "winget" => "https://learn.microsoft.com/windows/package-manager/winget/",
        _ => return Err("INVALID_PRODUCT".into()),
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

fn initial_window_size(work_width: f64, work_height: f64) -> tauri::LogicalSize<f64> {
    // Leave room for window borders and the title bar at the current DPI.
    tauri::LogicalSize::new(
        (work_width - 32.0).clamp(520.0, 1320.0),
        (work_height - 48.0).clamp(240.0, 860.0),
    )
}

fn startup_error(error: impl std::fmt::Display, directory: Option<&Path>) -> ! {
    use std::os::windows::ffi::OsStrExt;
    let recovery = directory.map(|path| format!("\n\n数据目录：{}\n已有数据未被替换。恢复方法：关闭程序，将上述数据目录改名保留，重新启动后在设置中从旧目录 backups 内的有效备份恢复。若提示数据库版本过新，请使用对应新版程序。\n\n是否打开数据目录？", path.display()))
        .unwrap_or_else(|| "\n\n请检查 WebView2 和启动环境。".into());
    let message: Vec<u16> = format!("RScriptoBox 无法启动：\n{error}{recovery}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let title: Vec<u16> = "RScriptoBox 启动错误"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let buttons = if directory.is_some() { MB_YESNO } else { MB_OK };
    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            buttons | MB_ICONERROR,
        )
    };
    if let Some(path) = directory.filter(|_| result == IDYES) {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        unsafe {
            let _ = ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(path.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            );
        }
    }
    std::process::exit(1);
}

pub fn launch() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app,_,_|{if let Some(window)=app.get_webview_window("main"){let _=window.show();let _=window.set_focus();}}))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app|{
            let executable=std::env::current_exe().unwrap_or_else(|error|startup_error(error,None));
            let portable=executable.parent().unwrap_or(Path::new(".")).join("portable.flag").is_file()
                || std::env::args().any(|v|v=="--portable");
            let root=if portable {executable.parent().unwrap().join("data")}else{app.path().app_local_data_dir().unwrap_or_else(|error|startup_error(error,None))};
            // Tauri panics on setup-hook errors, so report data failures here before returning.
            let store=Arc::new(Store::open(root.clone()).unwrap_or_else(|error|startup_error(error,Some(&root))));
            let runner=Arc::new(Runner::new(store.clone()));
            let runtimes=Arc::new(RuntimeService::new(store.clone()));
            app.manage(AppState{store,runner,runtimes:runtimes.clone(),gate:Arc::new(Mutex::new(())),portable});
            std::thread::spawn(move||{let _=runtimes.discover();});
            if let Some(window)=app.get_webview_window("main") {
                if let Ok(Some(monitor))=window.current_monitor() {
                    let area=monitor.work_area().size.to_logical::<f64>(monitor.scale_factor());
                    let _=window.set_size(initial_window_size(area.width,area.height));
                    let _=window.center();
                }
            }
            Ok(())
        })
        .on_window_event(|window,event|{
            if let tauri::WindowEvent::CloseRequested{api,..}=event {
                let Some(state)=window.try_state::<AppState>() else {return;};
                let state=state.inner().clone();
                let active=state.runner.active_count()>0;
                let installing=state.runtimes.has_mutation_lock().unwrap_or(true);
                if active||installing {
                    api.prevent_close();
                    let app=window.app_handle().clone();
                    let message=if installing {"存在正在安装或待复检的环境。退出后安装可能继续，下次启动仍需复检。活动脚本将强制停止。是否退出？"}else{"仍有脚本运行。退出会强制结束任务树，脚本的数据写入可能中断。是否退出？"};
                    window.dialog().message(message).title("退出 RScriptoBox").kind(MessageDialogKind::Warning)
                        .buttons(MessageDialogButtons::OkCancel).show(move|confirmed|{
                            if confirmed {state.runner.stop_all();std::thread::spawn(move||{
                                for _ in 0..100 {if state.runner.active_count()==0 {app.exit(0);return;}std::thread::sleep(Duration::from_millis(100));}
                                app.dialog().message("任务尚未完成停止，请检查任务状态后重试退出。").title("等待任务停止").show(|_|{});
                            });}
                        });
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_scripts,add_scripts,update_script,remove_script,relocate_script,open_script_location,edit_script,
            run_script,stop_run,write_terminal_input,resize_terminal,ack_run_output,list_runs,read_run_log,export_run_log,list_runtimes,refresh_runtimes,register_runtime,
            list_environments,plan_runtime_install,start_runtime_install,list_install_jobs,read_install_log,cancel_runtime_install,recheck_install,
            create_python_environment,install_requirements,get_settings,update_settings,backup_database,restore_database,
            app_info,open_data_directory,open_official_page
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error|startup_error(error,None));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_window_fits_1280_by_720_at_100_and_200_percent() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let work_width = 1280.0 / scale;
            let work_height = 680.0 / scale;
            let size = initial_window_size(work_width, work_height);
            assert!(size.width >= 520.0 && size.height >= 240.0);
            assert!(size.width + 32.0 <= work_width);
            assert!(size.height + 48.0 <= work_height);
        }
    }
}
