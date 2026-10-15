use crate::{models::*, storage::Store};
use sha2::{Digest, Sha256};
use std::os::windows::io::AsRawHandle;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
};

pub fn local_path(value: &str, must_exist: bool) -> AppResult<PathBuf> {
    if value.is_empty()
        || value.len() > 240
        || value.chars().any(|c| c < ' ')
        || value.starts_with("\\\\")
        || value.starts_with("//")
        || value.get(2..).unwrap_or("").contains(':')
    {
        return Err(
            "INVALID_PATH: 仅支持 240 字符以内的本地磁盘绝对路径，不支持 UNC、设备路径或 ADS"
                .into(),
        );
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() || !matches!(path.components().next(), Some(Component::Prefix(_))) {
        return Err("INVALID_PATH: 请选择本地磁盘绝对路径".into());
    }
    if !must_exist {
        return Ok(path);
    }
    let canonical = fs::canonicalize(path).map_err(|e| format!("PATH_MISSING: {e}"))?;
    let display = canonical.to_string_lossy().to_string();
    if display.starts_with("\\\\?\\UNC\\") {
        return Err("INVALID_PATH: 不支持网络目标".into());
    }
    let plain = display.strip_prefix("\\\\?\\").unwrap_or(&display);
    if plain.len() > 240 {
        return Err("INVALID_PATH: 实际路径超过首版支持长度".into());
    }
    Ok(PathBuf::from(plain))
}

pub fn kind(path: &Path) -> AppResult<String> {
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "cmd" | "bat" => Ok("cmd".into()),
        "ps1" => Ok("powershell".into()),
        "py" => Ok("python".into()),
        _ => Err("UNSUPPORTED_SCRIPT: 仅支持 .cmd、.bat、.ps1 和 .py 文件".into()),
    }
}
fn identity(path: &Path) -> AppResult<(u32, u32, u32)> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|e| e.to_string())?;
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}
pub fn same_file(first: &Path, second: &Path) -> bool {
    match (identity(first), identity(second)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
pub fn hash_file(path: &Path) -> AppResult<String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024 {
        return Err("SCRIPT_TOO_LARGE: 入口脚本不得超过 32 MiB".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 16384];
    loop {
        let size = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn working_directory(source: &Path, configured: &str) -> AppResult<PathBuf> {
    let directory = if configured.trim().is_empty() {
        source
            .parent()
            .ok_or("INVALID_CWD: 脚本缺少父目录")?
            .to_path_buf()
    } else {
        local_path(configured, true)?
    };
    if !directory.is_dir() {
        return Err("INVALID_CWD: 工作目录不存在或不是文件夹".into());
    }
    Ok(directory)
}

pub fn add(store: &Store, paths: Vec<String>) -> AppResult<serde_json::Value> {
    if paths.len() > 256 {
        return Err("TOO_MANY_FILES: 每次最多添加 256 个文件".into());
    }
    let existing = store.load_scripts()?;
    let mut identities: std::collections::HashSet<_> = existing
        .iter()
        .filter_map(|s| identity(Path::new(&s.path)).ok())
        .collect();
    identities.extend(
        existing
            .iter()
            .filter_map(|s| s.source_path.as_deref())
            .filter_map(|p| identity(Path::new(p)).ok()),
    );
    let settings = store.settings()?;
    let runtimes = store.load_runtimes()?;
    let mut added = Vec::new();
    let mut errors = Vec::new();
    for value in paths {
        let result = (|| -> AppResult<Script> {
            let path = local_path(&value, true)?;
            if !path.is_file() {
                return Err("请选择普通脚本文件".into());
            }
            let kind = kind(&path)?;
            let file_identity = identity(&path)?;
            if identities.contains(&file_identity) {
                return Err("脚本已经在工具箱中".into());
            }
            let runtime = runtimes
                .iter()
                .find(|r| r.status == "ready" && compatible(&kind, &r.kind));
            let time = now();
            let managed = copy_into_workspace(store, &path)?;
            let script = Script {
                id: new_id(),
                name: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                path: managed.to_string_lossy().into(),
                source_path: Some(path.to_string_lossy().into()),
                kind: kind.clone(),
                description: String::new(),
                tags: vec![],
                favorite: false,
                runtime_id: runtime.map(|r| r.id.clone()),
                environment_id: None,
                cwd: String::new(),
                args: vec![],
                env: Default::default(),
                run_mode: settings.default_mode.clone(),
                timeout_seconds: None,
                encoding: if kind == "python" {
                    "utf-8".into()
                } else {
                    settings.default_encoding.clone()
                },
                success_codes: vec![0],
                execution_policy: None,
                trusted_hash: None,
                created_at: time.clone(),
                updated_at: time,
                exists: true,
            };
            store.save_script(&script)?;
            identities.insert(file_identity);
            Ok(script)
        })();
        match result {
            Ok(script) => {
                added.push(script);
            }
            Err(error) => errors.push(format!("{value}：{error}")),
        }
    }
    Ok(serde_json::json!({"added":added,"errors":errors}))
}
pub fn compatible(script: &str, runtime: &str) -> bool {
    match script {
        "cmd" => runtime == "cmd",
        "powershell" => ["powershell", "windows_powershell"].contains(&runtime),
        "python" => runtime == "python",
        _ => false,
    }
}
pub fn require_workspace_copy(store: &Store, script: &Script) -> AppResult<()> {
    if !Path::new(&script.path).starts_with(store.root.join("scripts")) {
        return Err(
            "WORKSPACE_COPY_REQUIRED: 旧记录尚未成功复制到工作台，请使用重新导入副本操作".into(),
        );
    }
    Ok(())
}
pub fn validate(script: &Script) -> AppResult<()> {
    if let Some(source) = &script.source_path {
        local_path(source, false)?;
    }
    if script.name.trim().is_empty()
        || script.name.chars().count() > 128
        || script.description.len() > 16384
        || script.tags.len() > 24
        || script.tags.iter().any(|t| t.chars().count() > 32)
        || script.args.len() > 256
        || script.env.len() > 128
        || !["captured", "terminal", "console"].contains(&script.run_mode.as_str())
        || !["system", "utf-8", "gb18030"].contains(&script.encoding.as_str())
        || script.success_codes.is_empty()
        || script.success_codes.len() > 32
        || script.execution_policy.as_deref().is_some_and(|v| {
            script.kind != "powershell" || !["RemoteSigned", "Bypass"].contains(&v)
        })
        || script
            .timeout_seconds
            .is_some_and(|t| t == 0 || t > 365 * 24 * 3600)
    {
        return Err("INVALID_CONFIG: 名称、参数、模式、超时或编码配置无效".into());
    }
    let mut slots = std::collections::HashSet::new();
    let mut length = 0usize;
    for input in script.args.iter().chain(script.env.values()) {
        match input {
            InputValue::Text(text) => {
                length += text.encode_utf16().count();
                if text.contains('\0') || text.len() > 16384 {
                    return Err("INVALID_ARGUMENT: 参数包含 NUL 或超过长度限制".into());
                }
            }
            InputValue::Secret(slot) => {
                if slot.secret_slot.is_empty()
                    || slot.secret_slot.len() > 128
                    || !slots.insert(&slot.secret_slot)
                {
                    return Err("INVALID_SECRET: 敏感槽位标识为空、重复或过长".into());
                }
            }
        }
    }
    if length > 24000
        || script
            .env
            .keys()
            .any(|k| k.is_empty() || k.len() > 128 || k.contains(['=', '\0']))
    {
        return Err("INVALID_ARGUMENT: 参数总量或环境变量名无效".into());
    }
    Ok(())
}
pub fn update(store: &Store, mut script: Script) -> AppResult<Script> {
    let original: Script = store
        .get("scripts", &script.id)?
        .ok_or("SCRIPT_MISSING: 记录不存在")?;
    if script.path != original.path || script.kind != original.kind {
        return Err("INVALID_CONFIG: 文件路径须通过重新定位操作修改".into());
    }
    validate(&script)?;
    script.created_at = original.created_at;
    script.source_path = original.source_path;
    script.trusted_hash = original.trusted_hash;
    script.updated_at = now();
    script.exists = Path::new(&script.path).is_file();
    store.save_script(&script)?;
    Ok(script)
}
pub fn relocate(store: &Store, id: &str, value: &str) -> AppResult<Script> {
    let mut script: Script = store
        .get("scripts", id)?
        .ok_or("SCRIPT_MISSING: 记录不存在")?;
    let path = local_path(value, true)?;
    if !path.is_file() || kind(&path)? != script.kind {
        return Err("INVALID_SCRIPT: 新文件必须为相同脚本类型".into());
    }
    let file_id = identity(&path)?;
    if store
        .load_scripts()?
        .iter()
        .any(|s| s.id != id && identity(Path::new(&s.path)).ok() == Some(file_id))
    {
        return Err("DUPLICATE_SCRIPT: 目标文件已登记".into());
    }
    if script.cwd
        == Path::new(&script.path)
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
    {
        script.cwd.clear();
    }
    script.path = copy_into_workspace(store, &path)?.to_string_lossy().into();
    script.source_path = Some(path.to_string_lossy().into());
    script.trusted_hash = None;
    script.exists = true;
    script.updated_at = now();
    store.save_script(&script)?;
    Ok(script)
}

fn copy_into_workspace(store: &Store, source: &Path) -> AppResult<PathBuf> {
    local_path(&source.to_string_lossy(), true)?;
    let expected = hash_file(source)?;
    let root = store.root.join("scripts");
    crate::storage::safe_owned_directory(&store.root, &root)?;
    // Each import owns a directory, so identically named scripts never overwrite each other.
    let directory = root.join(new_id());
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let destination = directory.join(source.file_name().ok_or("脚本缺少文件名")?);
    let result = (|| {
        local_path(&destination.to_string_lossy(), false)?;
        let mut input = File::open(source)
            .map_err(|e| e.to_string())?
            .take(32 * 1024 * 1024 + 1);
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        let size = std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        output.flush().map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        if size > 32 * 1024 * 1024 || hash_file(&destination)? != expected {
            return Err("SCRIPT_CHANGED: 复制期间脚本发生变化，请重新导入".into());
        }
        Ok(destination.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&destination);
        let _ = fs::remove_dir(&directory);
    }
    result
}

pub fn migrate_workspace(store: &Store) -> AppResult<()> {
    for mut script in store.load_scripts()? {
        let source = PathBuf::from(&script.path);
        let mut changed = false;
        if !source.starts_with(store.root.join("scripts"))
            && local_path(&script.path, false).is_ok()
            && source.is_file()
        {
            // Failed copies leave the old record usable; opening the app never removes a source.
            if let Ok(destination) = copy_into_workspace(store, &source) {
                if source.parent().is_some_and(|p| p == Path::new(&script.cwd)) {
                    script.cwd.clear();
                }
                script.path = destination.to_string_lossy().into();
                script.source_path = Some(source.to_string_lossy().into());
                script.trusted_hash = None;
                changed = true;
            }
        }
        if script.run_mode == "captured" {
            script.run_mode = "terminal".into();
            changed = true;
        }
        if changed {
            store.save_script(&script)?;
        }
    }
    let mut settings = store.settings()?;
    if settings.default_mode == "captured" {
        settings.default_mode = "terminal".into();
        store.save_settings(&settings)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_copy_survives_source_delete_and_directory_move() {
        let root = std::env::temp_dir().join(format!("rscripto-portable-{}", new_id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("工具.py");
        fs::write(&source, b"print('portable')").unwrap();
        let old_root = root.join("old-data");
        let store = Store::open(old_root.clone()).unwrap();
        add(&store, vec![source.to_string_lossy().into()]).unwrap();
        let record = store.load_scripts().unwrap().remove(0);
        assert!(record
            .path
            .starts_with(&old_root.to_string_lossy().to_string()));
        assert!(record.cwd.is_empty());
        assert_eq!(
            add(&store, vec![source.to_string_lossy().into()]).unwrap()["errors"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        fs::remove_file(&source).unwrap();
        assert_eq!(fs::read(&record.path).unwrap(), b"print('portable')");
        drop(store);
        let new_root = root.join("moved-data");
        fs::rename(&old_root, &new_root).unwrap();
        let store = Store::open(new_root.clone()).unwrap();
        let moved = store.load_scripts().unwrap().remove(0);
        assert_eq!(moved.id, record.id);
        assert!(moved
            .path
            .starts_with(&new_root.to_string_lossy().to_string()));
        assert_eq!(fs::read(&moved.path).unwrap(), b"print('portable')");
        fs::write(&moved.path, b"print('edited copy')").unwrap();
        assert!(!source.exists());
        let read: Script = store.get("scripts", &moved.id).unwrap().unwrap();
        assert_eq!(fs::read(&read.path).unwrap(), b"print('edited copy')");
        store
            .put(
                "scripts",
                &new_id(),
                &serde_json::json!({"path":"scripts/../../outside.py"}),
            )
            .unwrap();
        assert!(store.load_scripts().is_err());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn legacy_import_migrates_without_modifying_original() {
        let root = std::env::temp_dir().join(format!("rscripto-migrate-{}", new_id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("legacy.cmd");
        fs::write(&source, b"@echo legacy").unwrap();
        let data = root.join("data");
        let store = Store::open(data.clone()).unwrap();
        add(&store, vec![source.to_string_lossy().into()]).unwrap();
        let mut script = store.load_scripts().unwrap().remove(0);
        script.path = source.to_string_lossy().into();
        script.cwd = root.to_string_lossy().into();
        script.run_mode = "captured".into();
        store.save_script(&script).unwrap();
        drop(store);
        let store = Store::open(data.clone()).unwrap();
        let migrated = store.load_scripts().unwrap().remove(0);
        assert_eq!(migrated.run_mode, "terminal");
        assert!(migrated
            .path
            .starts_with(&data.to_string_lossy().to_string()));
        assert!(migrated.cwd.is_empty());
        assert_eq!(fs::read(source).unwrap(), b"@echo legacy");
        assert_eq!(fs::read(migrated.path).unwrap(), b"@echo legacy");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn path_boundaries_and_script_kinds() {
        assert!(local_path(r"\\server\share\evil.py", false).is_err());
        assert!(local_path(r"C:\safe.py:stream", false).is_err());
        assert!(local_path("relative.py", false).is_err());
        assert_eq!(kind(Path::new("hello.CMD")).unwrap(), "cmd");
        assert!(kind(Path::new("a.exe")).is_err());
        assert!(compatible("powershell", "windows_powershell"));
        assert!(!compatible("python", "cmd"));
    }
    #[test]
    fn empty_working_directory_uses_script_parent() {
        let root = std::env::temp_dir().join(format!("rscripto-cwd-{}", new_id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("example.py");
        assert_eq!(working_directory(&source, "").unwrap(), root);
        assert_eq!(working_directory(&source, "   ").unwrap(), root);
        fs::write(&source, b"pass").unwrap();
        assert!(working_directory(&source, &source.to_string_lossy()).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn import_deduplicates_file_identity_and_restore_never_deletes_sources() {
        let root = std::env::temp_dir().join(format!("rscripto-library-{}", new_id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("示例 script.cmd");
        let alias = root.join("alias.cmd");
        fs::write(&source, b"@echo hello\r\n").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let store = Store::open(root.join("data")).unwrap();
        let result = add(
            &store,
            vec![
                source.to_string_lossy().into(),
                alias.to_string_lossy().into(),
            ],
        )
        .unwrap();
        assert_eq!(result["added"].as_array().unwrap().len(), 1);
        assert_eq!(result["errors"].as_array().unwrap().len(), 1);
        let mut record = store.load_scripts().unwrap().remove(0);
        record.trusted_hash = Some("remembered".into());
        store.save_script(&record).unwrap();
        let backup = root.join("backup.db");
        store.backup(&backup).unwrap();
        record.name = "新的展示名称".into();
        record.trusted_hash = Some("forged".into());
        let updated = update(&store, record.clone()).unwrap();
        assert_eq!(updated.trusted_hash.as_deref(), Some("remembered"));
        fs::write(root.join("invalid.db"), b"not a database").unwrap();
        assert!(store.restore(&root.join("invalid.db")).is_err());
        assert_eq!(store.load_scripts().unwrap()[0].name, "新的展示名称");
        store.remove_script(&record.id).unwrap();
        assert!(source.is_file());
        store.restore(&backup).unwrap();
        let restored = store.load_scripts().unwrap();
        assert_eq!(restored.len(), 1);
        assert!(restored[0].trusted_hash.is_none());
        assert_eq!(fs::read(&source).unwrap(), b"@echo hello\r\n");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
