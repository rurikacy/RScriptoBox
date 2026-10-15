use crate::models::*;
use rusqlite::{backup::Backup, params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime},
};

const TABLES: &[&str] = &[
    "scripts",
    "runtimes",
    "environments",
    "runs",
    "install_jobs",
];
pub struct Store {
    pub root: PathBuf,
    connection: Mutex<Connection>,
    log_bytes: AtomicU64,
    cleanup_lock: Mutex<()>,
}
impl Store {
    pub fn open(root: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        for name in ["logs", "install-logs", "envs", "backups", "scripts"] {
            fs::create_dir_all(root.join(name)).map_err(|e| e.to_string())?;
        }
        safe_owned_directory(&root, &root.join("backups"))?;
        let mut connection =
            Connection::open(root.join("library.db")).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if schema > 2 {
            return Err("DATABASE_TOO_NEW: 数据由更新版本创建，请使用新版程序".into());
        }
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| e.to_string())?;
        if schema == 0 {
            // An existing database is backed up before its first migration.
            let populated: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if populated > 0 {
                let mut backup = Connection::open(
                    root.join("backups")
                        .join(format!("{}-schema0.db", new_id())),
                )
                .map_err(|e| e.to_string())?;
                Backup::new(&connection, &mut backup)
                    .map_err(|e| e.to_string())?
                    .run_to_completion(64, Duration::from_millis(10), None)
                    .map_err(|e| e.to_string())?;
            }
            let transaction = connection.transaction().map_err(|e| e.to_string())?;
            for table in TABLES {
                transaction.execute_batch(&format!("CREATE TABLE IF NOT EXISTS {table} (id TEXT PRIMARY KEY NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)));")).map_err(|e| e.to_string())?;
            }
            transaction.execute_batch("CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, data TEXT NOT NULL CHECK(json_valid(data)));
                CREATE INDEX IF NOT EXISTS runs_started ON runs(json_extract(data,'$.startedAt'));
                CREATE INDEX IF NOT EXISTS runs_script ON runs(json_extract(data,'$.scriptId'));
                PRAGMA user_version=1;").map_err(|e| e.to_string())?;
            transaction.commit().map_err(|e| e.to_string())?;
        }
        let total = tree_size(&root.join("logs")) + tree_size(&root.join("install-logs"));
        let store = Self {
            root,
            connection: Mutex::new(connection),
            log_bytes: AtomicU64::new(total),
            cleanup_lock: Mutex::new(()),
        };
        for mut run in store.load_runs()? {
            if ["starting", "running", "stopping"].contains(&run.state.as_str()) {
                run.state = "interrupted".into();
                run.reason = Some("应用异常退出，任务未正常结束".into());
                run.finished_at = Some(now());
                store.save_run(&run)?;
            }
        }
        store.cleanup_logs()?;
        store.prune_backups()?;
        if schema == 1 {
            store.backup(
                &store
                    .root
                    .join("backups")
                    .join(format!("{}-before-workspace.db", new_id())),
            )?;
        }
        crate::library::migrate_workspace(&store)?;
        store
            .connection
            .lock()
            .map_err(|e| e.to_string())?
            .execute_batch("PRAGMA user_version=2;")
            .map_err(|e| e.to_string())?;
        Ok(store)
    }
    fn table(table: &str) -> AppResult<&str> {
        if TABLES.contains(&table) {
            Ok(table)
        } else {
            Err("INVALID_TABLE".into())
        }
    }
    pub fn get<T: DeserializeOwned>(&self, table: &str, id: &str) -> AppResult<Option<T>> {
        let table = Self::table(table)?;
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
        let mut statement = connection
            .prepare(&format!("SELECT data FROM {table} WHERE id=?1"))
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query([id]).map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => self
                .decode(table, &row.get::<_, String>(0).map_err(|e| e.to_string())?)
                .map(Some),
            None => Ok(None),
        }
    }
    pub fn list<T: DeserializeOwned>(&self, table: &str) -> AppResult<Vec<T>> {
        let table = Self::table(table)?;
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
        let mut statement = connection
            .prepare(&format!("SELECT data FROM {table}"))
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| self.decode(table, &r.map_err(|e| e.to_string())?))
            .collect()
    }
    pub fn put<T: Serialize>(&self, table: &str, id: &str, value: &T) -> AppResult<()> {
        let table = Self::table(table)?;
        let mut data = serde_json::to_value(value).map_err(|e| e.to_string())?;
        if table == "scripts" {
            for key in ["path", "cwd"] {
                if let Some(path) = data.get(key).and_then(|v| v.as_str()) {
                    if let Ok(relative) = Path::new(path).strip_prefix(&self.root) {
                        if relative.starts_with("scripts")
                            && relative
                                .components()
                                .all(|c| matches!(c, std::path::Component::Normal(_)))
                        {
                            data[key] = relative.to_string_lossy().replace('\\', "/").into();
                        }
                    }
                }
            }
        }
        let data = serde_json::to_string(&data).map_err(|e| e.to_string())?;
        self.connection.lock().map_err(|e| e.to_string())?
            .execute(&format!("INSERT INTO {table}(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data"), params![id, data])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn decode<T: DeserializeOwned>(&self, table: &str, data: &str) -> AppResult<T> {
        let mut value: serde_json::Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
        if table == "scripts" {
            for key in ["path", "cwd"] {
                if let Some(text) = value
                    .get(key)
                    .and_then(|v| v.as_str())
                    .filter(|p| !p.is_empty())
                {
                    let path = Path::new(text);
                    if path.is_relative() {
                        if !path.starts_with("scripts")
                            || !path
                                .components()
                                .all(|c| matches!(c, std::path::Component::Normal(_)))
                        {
                            return Err("INVALID_PATH: 工作台脚本路径越界".into());
                        }
                        value[key] = self.root.join(path).to_string_lossy().into_owned().into();
                    }
                }
            }
        }
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
    pub fn remove_script(&self, id: &str) -> AppResult<()> {
        let mut connection = self.connection.lock().map_err(|e| e.to_string())?;
        let transaction = connection.transaction().map_err(|e| e.to_string())?;
        transaction
            .execute("DELETE FROM scripts WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        transaction.execute("UPDATE runs SET data=json_set(data,'$.scriptId',NULL) WHERE json_extract(data,'$.scriptId')=?1", [id]).map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())
    }
    pub fn load_scripts(&self) -> AppResult<Vec<Script>> {
        self.list("scripts")
    }
    pub fn save_script(&self, value: &Script) -> AppResult<()> {
        self.put("scripts", &value.id, value)
    }
    pub fn load_runtimes(&self) -> AppResult<Vec<Runtime>> {
        self.list("runtimes")
    }
    pub fn load_runs(&self) -> AppResult<Vec<Run>> {
        self.list("runs")
    }
    pub fn save_run(&self, value: &Run) -> AppResult<()> {
        self.put("runs", &value.id, value)
    }
    pub fn settings(&self) -> AppResult<Settings> {
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
        let data: Option<String> = connection
            .query_row("SELECT data FROM settings WHERE key='app'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let settings = data
            .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
            .unwrap_or_else(|| Ok(Settings::default()))?;
        validate_settings(&settings)?;
        Ok(settings)
    }
    pub fn save_settings(&self, settings: &Settings) -> AppResult<()> {
        validate_settings(settings)?;
        let data = serde_json::to_string(settings).map_err(|e| e.to_string())?;
        self.connection.lock().map_err(|e| e.to_string())?.execute(
            "INSERT INTO settings(key,data) VALUES('app',?1) ON CONFLICT(key) DO UPDATE SET data=excluded.data", [data]).map_err(|e| e.to_string())?;
        self.cleanup_logs()
    }
    pub fn reserve_log_bytes(&self, amount: u64, total_limit: u64) -> bool {
        let reserve = || {
            self.log_bytes
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                    used.checked_add(amount).filter(|next| *next <= total_limit)
                })
                .is_ok()
        };
        if reserve() {
            return true;
        }
        if self.cleanup_with_headroom(amount, total_limit).is_err() {
            return false;
        }
        reserve()
    }
    pub fn cleanup_logs(&self) -> AppResult<()> {
        let settings = self.settings()?;
        let limit = settings.total_log_mib * 1024 * 1024;
        self.cleanup_with_headroom(0, limit)
    }
    fn cleanup_with_headroom(&self, required: u64, limit: u64) -> AppResult<()> {
        let _guard = self.cleanup_lock.lock().map_err(|e| e.to_string())?;
        let settings = self.settings()?;
        let oldest = chrono::Utc::now() - chrono::Duration::days(settings.retention_days as i64);
        let mut candidates: Vec<(String, String, &str)> = self
            .load_runs()?
            .into_iter()
            .filter(|r| r.finished_at.is_some())
            .map(|r| (r.started_at, r.id, "logs"))
            .collect();
        candidates.extend(
            self.list::<InstallJob>("install_jobs")?
                .into_iter()
                .filter(|j| {
                    j.finished_at.is_some()
                        && !["installing", "verifying", "verification_required"]
                            .contains(&j.state.as_str())
                })
                .map(|j| (j.created_at, j.id, "install-logs")),
        );
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        for (date, id, category) in candidates {
            uuid::Uuid::parse_str(&id)
                .map_err(|_| "UNSAFE_DELETE: 日志目录标识无效".to_string())?;
            let base = self.root.join(category);
            let directory = base.join(id);
            let expired = chrono::DateTime::parse_from_rfc3339(&date)
                .map(|t| t < oldest)
                .unwrap_or(false);
            if !expired
                && self
                    .log_bytes
                    .load(Ordering::SeqCst)
                    .saturating_add(required)
                    <= limit
            {
                continue;
            }
            if !directory.is_dir() {
                continue;
            }
            safe_owned_directory(&base, &directory)?;
            let size = tree_size(&directory);
            fs::remove_dir_all(&directory).map_err(|e| e.to_string())?;
            let _ = self
                .log_bytes
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                    Some(v.saturating_sub(size))
                });
        }
        Ok(())
    }
    pub fn backup(&self, target: &Path) -> AppResult<()> {
        self.validate_backup_path(target)?;
        if let Some(parent) = target.parent() {
            if !parent.is_dir() {
                return Err("备份目录不存在".into());
            }
        }
        let connection = self.connection.lock().map_err(|e| e.to_string())?;
        let mut destination = Connection::open(target).map_err(|e| e.to_string())?;
        Backup::new(&connection, &mut destination)
            .map_err(|e| e.to_string())?
            .run_to_completion(64, Duration::from_millis(5), None)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn validate_backup_path(&self, path: &Path) -> AppResult<()> {
        if !path.is_absolute()
            || !["db", "sqlite", "sqlite3"]
                .contains(&path.extension().and_then(|v| v.to_str()).unwrap_or(""))
        {
            return Err("INVALID_BACKUP: 请选择绝对路径的 .db 或 .sqlite 文件".into());
        }
        let absolute = if path.exists() {
            fs::canonicalize(path)
        } else {
            fs::canonicalize(path.parent().unwrap_or(Path::new("")))
                .map(|p| p.join(path.file_name().unwrap_or_default()))
        }
        .map_err(|e| e.to_string())?;
        if absolute == fs::canonicalize(self.root.join("library.db")).map_err(|e| e.to_string())?
            || crate::library::same_file(path, &self.root.join("library.db"))
        {
            return Err("INVALID_BACKUP: 不可选择当前正在使用的数据库".into());
        }
        Ok(())
    }
    pub fn restore(&self, source: &Path) -> AppResult<()> {
        self.validate_backup_path(source)?;
        if !source.is_file() {
            return Err("备份文件不存在".into());
        }
        let source_connection =
            Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        let check: String = source_connection
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let schema: u32 = source_connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if check != "ok" || !(1..=2).contains(&schema) {
            return Err("INVALID_BACKUP: 数据库损坏或版本不兼容".into());
        }
        for table in TABLES {
            let _: i64 = source_connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let mut statement = source_connection
                .prepare(&format!("SELECT id,data FROM {table}"))
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (id, data) = row.map_err(|e| e.to_string())?;
                uuid::Uuid::parse_str(&id)
                    .map_err(|_| "INVALID_BACKUP: 记录标识无效".to_string())?;
                let value: serde_json::Value =
                    serde_json::from_str(&data).map_err(|e| e.to_string())?;
                if value.get("id").and_then(|v| v.as_str()) != Some(id.as_str()) {
                    return Err("INVALID_BACKUP: 记录内外标识不一致".into());
                }
                match *table {
                    "scripts" => {
                        let s: Script = self.decode("scripts", &data)?;
                        crate::library::validate(&s)?;
                    }
                    "runtimes" => {
                        let _: Runtime = serde_json::from_str(&data).map_err(|e| e.to_string())?;
                    }
                    "environments" => {
                        let _: Environment =
                            serde_json::from_str(&data).map_err(|e| e.to_string())?;
                    }
                    "runs" => {
                        let _: Run = serde_json::from_str(&data).map_err(|e| e.to_string())?;
                    }
                    "install_jobs" => {
                        let _: InstallJob =
                            serde_json::from_str(&data).map_err(|e| e.to_string())?;
                    }
                    _ => unreachable!(),
                }
            }
        }
        let backup_settings: Option<String> = source_connection
            .query_row("SELECT data FROM settings WHERE key='app'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        validate_settings(
            &backup_settings
                .map(|data| serde_json::from_str::<Settings>(&data).map_err(|e| e.to_string()))
                .unwrap_or_else(|| Ok(Settings::default()))?,
        )?;
        let current_backup = self
            .root
            .join("backups")
            .join(format!("{}-before-restore.db", new_id()));
        self.backup(&current_backup)?;
        let mut destination = self.connection.lock().map_err(|e| e.to_string())?;
        Backup::new(&source_connection, &mut destination)
            .map_err(|e| e.to_string())?
            .run_to_completion(64, Duration::from_millis(5), None)
            .map_err(|e| e.to_string())?;
        destination.execute("UPDATE runs SET data=json_set(data,'$.state','interrupted','$.finishedAt',?1) WHERE json_extract(data,'$.state') IN ('starting','running','stopping')",[now()]).map_err(|e|e.to_string())?;
        destination
            .execute(
                "UPDATE scripts SET data=json_set(data,'$.trustedHash',NULL)",
                [],
            )
            .map_err(|e| e.to_string())?;
        drop(destination);
        crate::library::migrate_workspace(self)?;
        self.connection
            .lock()
            .map_err(|e| e.to_string())?
            .execute_batch("PRAGMA user_version=2;")
            .map_err(|e| e.to_string())?;
        self.prune_backups()?;
        Ok(())
    }
    pub fn prune_backups(&self) -> AppResult<()> {
        let directory = self.root.join("backups");
        safe_owned_directory(&self.root, &directory)?;
        let mut files: Vec<_> = fs::read_dir(directory)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter(|f| f.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter(|f| {
                let name = f.file_name();
                let name = name.to_string_lossy();
                ["-schema0.db", "-before-restore.db", "-before-workspace.db"]
                    .iter()
                    .any(|suffix| {
                        name.strip_suffix(suffix)
                            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                    })
            })
            .collect();
        files.sort_by_key(|f| {
            f.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        let remove = files.len().saturating_sub(3);
        for file in files.into_iter().take(remove) {
            fs::remove_file(file.path()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn validate_settings(settings: &Settings) -> AppResult<()> {
    if !(1..=4).contains(&settings.max_concurrent)
        || !(1..=3650).contains(&settings.retention_days)
        || !(1..=1024).contains(&settings.log_limit_mib)
        || !(20..=102400).contains(&settings.total_log_mib)
        || settings.total_log_mib < settings.log_limit_mib
        || !["captured", "terminal", "console"].contains(&settings.default_mode.as_str())
        || !["system", "utf-8", "gb18030"].contains(&settings.default_encoding.as_str())
    {
        return Err("INVALID_SETTINGS: 并发、保留期、配额或默认值无效".into());
    }
    Ok(())
}

pub fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return 0;
        }
    }
    if metadata.is_file() {
        return metadata.len();
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| tree_size(&e.path())).sum())
        .unwrap_or(0)
}
pub fn safe_owned_directory(root: &Path, path: &Path) -> AppResult<()> {
    let canonical_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let canonical_path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if canonical_path == canonical_root || !canonical_path.starts_with(&canonical_root) {
        return Err("UNSAFE_DELETE: 数据目录越界，已停止清理".into());
    }
    fn check(path: &Path) -> AppResult<()> {
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("UNSAFE_DELETE: 目录含重解析点，已停止清理".into());
            }
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
                check(&entry.map_err(|e| e.to_string())?.path())?;
            }
        }
        Ok(())
    }
    check(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_retention_keeps_three_owned_files_and_preserves_other_files() {
        let root = std::env::temp_dir().join(format!("rscripto-retention-{}", new_id()));
        let store = Store::open(root.clone()).unwrap();
        let backup_dir = root.join("backups");
        for _ in 0..5 {
            fs::write(
                backup_dir.join(format!("{}-schema0.db", new_id())),
                b"owned",
            )
            .unwrap();
        }
        for name in ["personal.db", "not-a-uuid-schema0.db", "notes.txt"] {
            fs::write(backup_dir.join(name), b"keep").unwrap();
        }
        store.prune_backups().unwrap();
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 6);
        for name in ["personal.db", "not-a-uuid-schema0.db", "notes.txt"] {
            assert_eq!(fs::read(backup_dir.join(name)).unwrap(), b"keep");
        }
        drop(store);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn storage_roundtrip_backup_and_limits() {
        let root = std::env::temp_dir().join(format!("rscripto-storage-{}", new_id()));
        let store = Store::open(root.clone()).unwrap();
        store
            .put("scripts", "test", &serde_json::json!({"name":"中文脚本"}))
            .unwrap();
        assert_eq!(
            store
                .get::<serde_json::Value>("scripts", "test")
                .unwrap()
                .unwrap()["name"],
            "中文脚本"
        );
        assert!(store.put("scripts; DROP TABLE scripts", "x", &1).is_err());
        assert!(store.reserve_log_bytes(8, 10));
        assert!(!store.reserve_log_bytes(3, 10));
        assert!(store.backup(&root.join("library.db")).is_err());
        store.backup(&root.join("backup.db")).unwrap();
        assert!(store
            .save_settings(&Settings {
                max_concurrent: 0,
                ..Settings::default()
            })
            .is_err());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn backup_rejects_hard_links_and_untrusted_record_paths() {
        let root = std::env::temp_dir().join(format!("rscripto-backup-{}", new_id()));
        let store = Store::open(root.join("data")).unwrap();
        let alias = root.join("alias.db");
        fs::hard_link(store.root.join("library.db"), &alias).unwrap();
        assert!(store.backup(&alias).is_err());
        assert!(store.restore(&alias).is_err());
        let backup = root.join("backup.db");
        store.backup(&backup).unwrap();
        let outside = root.join(new_id());
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"do not delete").unwrap();
        let record_id = new_id();
        let bad_run = Run {
            id: outside.to_string_lossy().into(),
            script_id: None,
            script_name: "bad backup".into(),
            script_path: String::new(),
            runtime_path: String::new(),
            run_mode: "captured".into(),
            state: "succeeded".into(),
            reason: None,
            root_exit_code: Some(0),
            started_at: "2000-01-01T00:00:00Z".into(),
            finished_at: Some(now()),
            log_truncated: false,
            plan_redacted: serde_json::Value::Null,
        };
        let connection = Connection::open(&backup).unwrap();
        connection
            .execute(
                "INSERT INTO runs(id,data) VALUES(?1,?2)",
                params![record_id, serde_json::to_string(&bad_run).unwrap()],
            )
            .unwrap();
        drop(connection);
        assert!(store.restore(&backup).is_err());
        assert!(outside.join("keep.txt").is_file());
        store.put("runs", &record_id, &bad_run).unwrap();
        assert!(store.cleanup_logs().is_err());
        assert!(outside.join("keep.txt").is_file());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn restore_validates_settings_before_replacing_database() {
        let root = std::env::temp_dir().join(format!("rscripto-settings-{}", new_id()));
        let store = Store::open(root.join("data")).unwrap();
        let backup = root.join("backup.db");
        store.backup(&backup).unwrap();
        let connection = Connection::open(&backup).unwrap();
        connection.execute("DROP TABLE settings", []).unwrap();
        drop(connection);
        assert!(store.restore(&backup).is_err());
        assert_eq!(store.settings().unwrap().max_concurrent, 4);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
