//! Fail-closed JSON persistence. All instances are owned by the application's
//! single state mutex; InstanceLock additionally excludes cooperating processes.
//! Only an explicitly loaded (or recovered) snapshot may be overwritten.

use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Validate {
    fn validate(&self) -> Result<(), String>;
}

fn error(action: &str, path: &Path, cause: impl std::fmt::Display) -> String {
    format!("{} [{}]: {}", action, path.display(), cause)
}

// An existing unreadable file (including a broken symlink) is NEVER first use.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::symlink_metadata(path) {
            Err(meta) if meta.kind() == io::ErrorKind::NotFound => Ok(None),
            Ok(_) => Err(error("文件存在但无法读取", path, e)),
            Err(meta) => Err(error("无法检查文件", path, meta)),
        },
        Err(e) => Err(error("无法读取文件（不是空数据）", path, e)),
    }
}

fn decode<T: DeserializeOwned + Validate>(bytes: &[u8], path: &Path) -> Result<T, String> {
    let value: T = serde_json::from_slice(bytes)
        .map_err(|e| error("JSON 无效；已禁止覆盖，请恢复备份或修复原文件", path, e))?;
    value.validate().map_err(|e| error("数据无效；已禁止覆盖", path, e))?;
    Ok(value)
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique_sibling(path: &Path, kind: &str) -> PathBuf {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}-{}-{}-{}", kind, stamp, std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
    PathBuf::from(name)
}

// A failed write/replace cleans up only the temporary file that we created.
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

fn write_synced_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)
        .map_err(|e| error("无法创建新文件", path, e))?;
    if let Err(cause) = file.write_all(bytes).and_then(|_| file.flush()).and_then(|_| file.sync_all()) {
        // Only remove a file whose create_new succeeded in THIS call. In
        // particular, a name collision must never delete another writer's file.
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error("文件写入/同步失败", path, cause));
    }
    Ok(())
}

fn prepare(path: &Path, bytes: &[u8]) -> Result<TempFile, String> {
    let path = unique_sibling(path, "tmp");
    write_synced_new(&path, bytes)?;
    Ok(TempFile(path))
}

/// Same-directory rename only: never fall back to copy/delete or truncate.
#[cfg(windows)]
fn replace(temp: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)
    }.map_err(|e| error("原子替换失败；请重试，不会直接覆盖文件", destination, e))
}

#[cfg(not(windows))]
fn replace(temp: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(temp, destination).map_err(|e| error("原子替换失败", destination, e))?;
    if let Some(parent) = destination.parent() {
        File::open(parent).and_then(|f| f.sync_all()).map_err(|e| error("目录同步失败", parent, e))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = prepare(path, bytes)?;
    replace(&temp.0, path)
}

pub struct JsonStore<T> {
    path: PathBuf,
    // None = never successfully loaded / failed load / detected external edit.
    // Some(None) = successfully confirmed missing on first use.
    loaded: Option<Option<Vec<u8>>>,
    marker: PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned + Default + Validate> JsonStore<T> {
    pub fn new(path: PathBuf) -> Self {
        Self { path, loaded: None, marker: PhantomData }
    }

    fn backup_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".bak");
        PathBuf::from(name)
    }

    pub fn load(&mut self) -> Result<T, String> {
        self.loaded = None;
        let bytes = read_optional(&self.path)?;
        let value = match &bytes {
            Some(bytes) => decode(bytes, &self.path)?,
            None => T::default(),
        };
        self.loaded = Some(bytes);
        Ok(value)
    }

    pub fn check_writable(&mut self) -> Result<(), String> {
        let snapshot = self.loaded.as_ref().ok_or_else(||
            error("保存被阻止", &self.path, "必须先成功读取数据或显式恢复备份"))?;
        let current = read_optional(&self.path)?;
        if &current != snapshot {
            self.loaded = None;
            return Err(error("保存被阻止", &self.path, "文件已被外部修改；请重新读取或恢复备份，避免丢失数据"));
        }
        // The baseline was validated at load/recovery. Byte equality is stricter
        // than parsing again and also prevents overwriting unknown external edits.
        Ok(())
    }

    pub fn save(&mut self, value: &T) -> Result<(), String> {
        value.validate()?;
        self.check_writable()?;
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        let previous = self.loaded.as_ref().expect("checked above").as_ref();
        if previous == Some(&bytes) {
            // Do not rotate an older recovery point for an idempotent retry,
            // but seed one for an already-existing file with no backup yet.
            let backup = self.backup_path();
            match read_optional(&backup)? {
                Some(existing) => { decode::<T>(&existing, &backup)?; }
                None => atomic_write(&backup, &bytes)?,
            }
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| error("无法创建数据目录", parent, e))?;
        }
        // Prepare and sync new data FIRST. Commit a validated backup BEFORE
        // replacing the primary. If anything fails, the primary stays intact.
        // First save also seeds a backup, so even a one-save file is recoverable.
        let temp = prepare(&self.path, &bytes)?;
        // Commit a validated prior snapshot before replacing the primary. If the
        // primary is missing but a backup already exists, retain that recovery
        // point instead of silently replacing it with new first-use data.
        let backup = self.backup_path();
        match previous {
            Some(previous) => atomic_write(&backup, previous)?,
            None => match read_optional(&backup)? {
                Some(existing) => { decode::<T>(&existing, &backup)?; }
                None => atomic_write(&backup, &bytes)?,
            },
        }
        replace(&temp.0, &self.path)?;
        self.loaded = Some(Some(bytes));
        Ok(())
    }

    /// Preserve the caller's dirty snapshot without requiring write permission
    /// on the primary. Explicit UI action only; unique create_new + sync_all.
    pub fn preserve_unsaved(&self, value: &T) -> Result<PathBuf, String> {
        value.validate()?;
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        let preserved = unique_sibling(&self.path, "unsaved");
        write_synced_new(&preserved, &bytes)?;
        Ok(preserved)
    }

    pub fn recover(&mut self) -> Result<T, String> {
        self.loaded = None;
        let backup = self.backup_path();
        let bytes = read_optional(&backup)?.ok_or_else(|| error("无法恢复", &backup, "不存在备份"))?;
        let value = decode(&bytes, &backup)?;
        // Read failure is not permission to overwrite! Preserve the exact raw
        // primary bytes, even if invalid JSON, before changing anything.
        let original = read_optional(&self.path)?;
        let temp = prepare(&self.path, &bytes)?;
        if let Some(original) = original {
            let preserved = unique_sibling(&self.path, "corrupt");
            write_synced_new(&preserved, &original)?;
        }
        // Do not rotate backup during recovery: corrupt data must never become .bak.
        replace(&temp.0, &self.path)?;
        self.loaded = Some(Some(bytes));
        Ok(value)
    }
}

/// Lifetime, OS-enforced data-directory lock. No plugin or named global mutex
/// needed. Windows releases sharing restrictions even on process termination;
/// the harmless file stays, so there is no stale-lock/delete race. This guards
/// multiple instances of this version, not old builds or external editors.
pub struct InstanceLock { _file: File }
impl InstanceLock {
    pub fn acquire(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| error("无法创建数据目录", dir, e))?;
        let path = dir.join(".sideline-storage.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let file = options.open(&path).map_err(|e| error("无法独占数据目录（Sideline 可能已在运行）", &path, e))?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Settings, Todo};
    use std::sync::{Arc, Mutex};

    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            let path = unique_sibling(&std::env::temp_dir().join("sideline-storage-test"), "dir");
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf { self.0.join("todos.json") }
        fn store(&self) -> JsonStore<Vec<Todo>> { JsonStore::new(self.path()) }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
    }
    fn todo(text: &str) -> Vec<Todo> {
        vec![Todo { text: text.into(), done: false, created: 42, notes: vec!["笔记".into()] }]
    }
    fn raw(value: &[Todo]) -> Vec<u8> { serde_json::to_vec_pretty(value).unwrap() }

    #[test]
    fn conflict_archive_preserves_dirty_and_external_versions_then_reload_unblocks() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        store.load().unwrap();
        store.save(&todo("original")).unwrap();
        let external = raw(&todo("external"));
        fs::write(dir.path(), &external).unwrap();
        assert!(store.save(&todo("local dirty")).is_err());
        let first = store.preserve_unsaved(&todo("local dirty")).unwrap();
        let second = store.preserve_unsaved(&todo("another snapshot")).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(first).unwrap(), raw(&todo("local dirty")));
        assert_eq!(fs::read(second).unwrap(), raw(&todo("another snapshot")));
        assert_eq!(fs::read(dir.path()).unwrap(), external);
        assert_eq!(store.load().unwrap()[0].text, "external");
        store.save(&todo("after explicit reload")).unwrap();
    }

    #[test]
    fn archive_survives_failed_reload_and_does_not_change_corrupt_primary() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        fs::write(dir.path(), b"corrupt external").unwrap();
        let path = store.preserve_unsaved(&todo("unsaved memory")).unwrap();
        assert!(store.load().is_err());
        assert_eq!(fs::read(path).unwrap(), raw(&todo("unsaved memory")));
        assert_eq!(fs::read(dir.path()).unwrap(), b"corrupt external");
    }

    #[test]
    fn rejected_archive_does_not_revoke_loaded_snapshot_or_create_files() {
        let dir = Sandbox::new();
        let mut store = JsonStore::<Settings>::new(dir.0.join("settings.json"));
        let valid = store.load().unwrap();
        let invalid = Settings { side: "invalid".into(), ..valid.clone() };
        assert!(store.preserve_unsaved(&invalid).is_err());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
        store.save(&valid).unwrap();
    }

    #[test]
    fn missing_is_first_use_but_save_requires_load() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        assert!(store.save(&todo("blocked")).is_err());
        assert!(!dir.path().exists());
        assert!(store.load().unwrap().is_empty());
        store.save(&todo("first")).unwrap();
        assert_eq!(fs::read(dir.path()).unwrap(), raw(&todo("first")));
        assert_eq!(fs::read(store.backup_path()).unwrap(), raw(&todo("first")));
    }

    #[test]
    fn legacy_array_loads_with_notes_default_and_does_not_rewrite() {
        let dir = Sandbox::new();
        let old = br#"[{"text":"old","done":true,"created":123}]"#;
        fs::write(dir.path(), old).unwrap();
        let mut store = dir.store();
        let todos = store.load().unwrap();
        assert_eq!(todos[0].text, "old");
        assert!(todos[0].done);
        assert_eq!(todos[0].created, 123);
        assert!(todos[0].notes.is_empty());
        assert_eq!(fs::read(dir.path()).unwrap(), old);
        store.save(&todos).unwrap();
        assert_eq!(fs::read(store.backup_path()).unwrap(), old);
    }

    #[test]
    fn malformed_unknown_or_wrong_shape_files_are_never_empty_or_overwritten() {
        let dir = Sandbox::new();
        for data in [b"".as_slice(), b"{broken", b"null", b"{}", b"[1]", b"\xff", br#"[{"text":"x","done":false,"created":0,"future":"keep"}]"#] {
            fs::write(dir.path(), data).unwrap();
            let mut store = dir.store();
            assert!(store.load().is_err());
            assert!(store.save(&Vec::new()).is_err());
            assert_eq!(fs::read(dir.path()).unwrap(), data);
        }
    }

    #[test]
    fn failed_reload_revokes_previous_write_permission() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        store.load().unwrap();
        store.save(&todo("ok")).unwrap();
        fs::write(dir.path(), b"corrupt").unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&todo("no")).is_err());
        assert_eq!(fs::read(dir.path()).unwrap(), b"corrupt");
    }

    #[test]
    fn external_change_even_valid_json_requires_reload() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        store.load().unwrap();
        fs::write(dir.path(), raw(&todo("external"))).unwrap();
        assert!(store.save(&todo("stale")).is_err());
        assert!(store.save(&todo("retry-stale")).is_err());
        assert_eq!(store.load().unwrap()[0].text, "external");
        store.save(&todo("after-load")).unwrap();
    }

    #[test]
    fn existing_directory_is_read_error_not_missing() {
        let dir = Sandbox::new();
        fs::create_dir(dir.path()).unwrap();
        let mut store = dir.store();
        assert!(store.load().is_err());
        assert!(store.save(&todo("no")).is_err());
        assert!(dir.path().is_dir());
    }

    #[test]
    fn backup_rotates_only_valid_snapshots_and_recovery_preserves_raw_corruption() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        store.load().unwrap();
        store.save(&todo("one")).unwrap();
        store.save(&todo("two")).unwrap();
        assert_eq!(fs::read(store.backup_path()).unwrap(), raw(&todo("one")));
        fs::write(dir.path(), b"exact corrupt\0\xff original").unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&todo("no")).is_err());
        assert_eq!(store.recover().unwrap()[0].text, "one");
        let preserved: Vec<_> = fs::read_dir(&dir.0).unwrap().map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().contains(".corrupt-")).collect();
        assert_eq!(preserved.len(), 1);
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"exact corrupt\0\xff original");
        assert_eq!(fs::read(store.backup_path()).unwrap(), raw(&todo("one")));
        store.save(&todo("after recovery")).unwrap();
    }

    #[test]
    fn absent_or_corrupt_backup_does_not_change_primary() {
        let dir = Sandbox::new();
        fs::write(dir.path(), b"original").unwrap();
        let mut store = dir.store();
        assert!(store.recover().is_err());
        fs::write(store.backup_path(), b"bad backup").unwrap();
        assert!(store.recover().is_err());
        assert_eq!(fs::read(dir.path()).unwrap(), b"original");
        assert!(store.save(&todo("no")).is_err());
    }

    #[test]
    fn recovery_can_restore_missing_primary() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        fs::write(store.backup_path(), raw(&todo("backup"))).unwrap();
        assert_eq!(store.recover().unwrap()[0].text, "backup");
        assert_eq!(fs::read(dir.path()).unwrap(), raw(&todo("backup")));
    }

    #[test]
    fn backup_commit_failure_keeps_primary_and_cleans_temps_then_retry_works() {
        let dir = Sandbox::new();
        fs::write(dir.path(), raw(&todo("old"))).unwrap();
        let mut store = dir.store();
        store.load().unwrap();
        fs::create_dir(store.backup_path()).unwrap();
        assert!(store.save(&todo("new")).is_err());
        assert_eq!(fs::read(dir.path()).unwrap(), raw(&todo("old")));
        assert!(fs::read_dir(&dir.0).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".tmp-")));
        fs::remove_dir(store.backup_path()).unwrap();
        store.save(&todo("new")).unwrap();
    }

    #[test]
    fn temp_creation_failure_does_not_create_primary() {
        let dir = Sandbox::new();
        let parent = dir.0.join("nested");
        let mut store = JsonStore::<Vec<Todo>>::new(parent.join("todos.json"));
        store.load().unwrap();
        fs::write(&parent, b"not a directory").unwrap();
        assert!(store.save(&todo("no")).is_err());
        assert_eq!(fs::read(parent).unwrap(), b"not a directory");
    }

    #[test]
    fn settings_roundtrip_defaults_and_validation() {
        let dir = Sandbox::new();
        let path = dir.0.join("settings.json");
        let mut store = JsonStore::<Settings>::new(path.clone());
        let default = store.load().unwrap();
        assert_eq!(default.side, "right");
        assert!(!default.collapsed);
        assert!(default.ontop);
        assert!(!default.completed_expanded);
        let changed = Settings { side: "left".into(), collapsed: true, ontop: false, completed_expanded: true };
        store.save(&changed).unwrap();
        assert_eq!(store.load().unwrap(), changed);
        let bad = Settings { side: "top".into(), ..changed.clone() };
        assert!(store.save(&bad).is_err());
        assert_eq!(store.load().unwrap(), changed);
        fs::write(&path, br#"{"side":"up","collapsed":false,"ontop":true,"completed_expanded":false}"#).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&Settings::default()).is_err());
    }

    #[test]
    fn serialized_read_modify_write_keeps_all_concurrent_updates() {
        let dir = Sandbox::new();
        let mut store = dir.store();
        store.load().unwrap();
        let store = Arc::new(Mutex::new(store));
        let handles: Vec<_> = (0..12).map(|n| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                let mut store = store.lock().unwrap();
                let mut todos = store.load().unwrap();
                todos.extend(todo(&n.to_string()));
                store.save(&todos).unwrap();
            })
        }).collect();
        for handle in handles { handle.join().unwrap(); }
        let mut store = store.lock().unwrap();
        assert_eq!(store.load().unwrap().len(), 12);
        assert_eq!(decode::<Vec<Todo>>(&fs::read(store.backup_path()).unwrap(), &store.backup_path()).unwrap().len(), 11);
    }

    #[cfg(windows)]
    #[test]
    fn windows_sharing_denied_is_not_empty_and_recovery_cannot_overwrite_it() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = Sandbox::new();
        fs::write(dir.path(), b"unreadable original").unwrap();
        let mut store = dir.store();
        fs::write(store.backup_path(), raw(&todo("backup"))).unwrap();
        let lock = OpenOptions::new().read(true).share_mode(0).open(dir.path()).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&todo("no")).is_err());
        assert!(store.recover().is_err());
        drop(lock);
        assert_eq!(fs::read(dir.path()).unwrap(), b"unreadable original");
    }

    #[cfg(windows)]
    #[test]
    fn windows_primary_replace_failure_keeps_old_and_backup_and_allows_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = Sandbox::new();
        let mut store = dir.store();
        store.load().unwrap();
        store.save(&todo("old")).unwrap();
        // Allow reads, deny delete/rename. Temp/backup writes still succeed.
        let lock = OpenOptions::new().read(true).share_mode(1).open(dir.path()).unwrap();
        assert!(store.save(&todo("new")).is_err());
        assert_eq!(fs::read(dir.path()).unwrap(), raw(&todo("old")));
        assert_eq!(fs::read(store.backup_path()).unwrap(), raw(&todo("old")));
        drop(lock);
        store.save(&todo("new")).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_instance_lock_is_exclusive_and_released_on_drop() {
        let dir = Sandbox::new();
        let lock = InstanceLock::acquire(&dir.0).unwrap();
        assert!(InstanceLock::acquire(&dir.0).is_err());
        drop(lock);
        assert!(InstanceLock::acquire(&dir.0).is_ok());
    }
}
