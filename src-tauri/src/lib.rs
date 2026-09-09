mod appbar;
mod storage;

use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use storage::{InstanceLock, JsonStore, Validate};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

/// Docked strip widths in LOGICAL (CSS) pixels. Win32 rects / AppBar calls
/// work in physical pixels, so convert via the window's scale factor before
/// use — never treat these as physical px (breaks on non-100% DPI).
const FULL_WIDTH: i32 = 320;
const COLLAPSED_WIDTH: i32 = 26;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct Todo {
    text: String,
    done: bool,
    created: u64,
    #[serde(default)]
    notes: Vec<String>,
}

impl Validate for Vec<Todo> {
    fn validate(&self) -> Result<(), String> { Ok(()) }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct Settings {
    side: String,
    collapsed: bool,
    ontop: bool,
    completed_expanded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { side: "right".into(), collapsed: false, ontop: true, completed_expanded: false }
    }
}

impl Validate for Settings {
    fn validate(&self) -> Result<(), String> {
        if self.side != "left" && self.side != "right" {
            return Err("无效停靠方向：side 必须是 left 或 right".into());
        }
        Ok(())
    }
}

// One lock, one ordering domain for ALL JSON reads/writes and native settings.
// No independently locked side/collapsed values and no read/write races.
struct Stores {
    todos: JsonStore<Vec<Todo>>,
    settings: JsonStore<Settings>,
    applied: Settings,
    error_display: bool,
    _instance: InstanceLock,
}

struct AppState(Mutex<Stores>);
impl AppState {
    fn lock(&self) -> Result<MutexGuard<'_, Stores>, String> {
        self.0.lock().map_err(|_| "存储状态锁异常；已停止写入，请重新启动应用".into())
    }
}

#[tauri::command]
fn load_todos(state: tauri::State<AppState>) -> Result<Vec<Todo>, String> {
    state.lock()?.todos.load()
}

#[tauri::command]
fn save_todos(state: tauri::State<AppState>, todos: Vec<Todo>) -> Result<(), String> {
    state.lock()?.todos.save(&todos)
}

#[tauri::command]
fn recover_todos(state: tauri::State<AppState>) -> Result<Vec<Todo>, String> {
    state.lock()?.todos.recover()
}

fn reload_settings(stores: &mut Stores, window: &WebviewWindow) -> Result<Settings, String> {
    let settings = stores.settings.load()?;
    let previous = display_settings(&stores.applied, stores.error_display);
    let next = display_settings(&settings, stores.error_display);
    if let Err(error) = apply_settings(window, Some(&previous), &next) {
        let rollback = apply_settings(window, Some(&next), &previous);
        return Err(match rollback { Ok(()) => error, Err(e) => format!("{error}；恢复窗口失败：{e}") });
    }
    stores.applied = settings.clone();
    Ok(settings)
}

#[tauri::command]
fn load_settings(state: tauri::State<AppState>, window: WebviewWindow) -> Result<Settings, String> {
    let mut stores = state.lock()?;
    reload_settings(&mut stores, &window)
}

#[derive(Serialize)]
struct ReloadResult<T> {
    value: Option<T>,
    error: Option<String>,
    preserved_path: String,
}

fn reload_result<T>(result: Result<T, String>, path: std::path::PathBuf) -> ReloadResult<T> {
    let (value, error) = match result { Ok(value) => (Some(value), None), Err(error) => (None, Some(error)) };
    ReloadResult { value, error, preserved_path: path.to_string_lossy().into_owned() }
}

#[tauri::command]
fn preserve_and_reload_todos(state: tauri::State<AppState>, todos: Vec<Todo>) -> Result<ReloadResult<Vec<Todo>>, String> {
    let mut stores = state.lock()?;
    let path = stores.todos.preserve_unsaved(&todos)?;
    Ok(reload_result(stores.todos.load(), path))
}

#[tauri::command]
fn preserve_and_reload_settings(state: tauri::State<AppState>, window: WebviewWindow, settings: Settings) -> Result<ReloadResult<Settings>, String> {
    let mut stores = state.lock()?;
    let path = stores.settings.preserve_unsaved(&settings)?;
    Ok(reload_result(reload_settings(&mut stores, &window), path))
}

fn primary_work_area() -> Result<(i32, i32, i32, i32), String> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::c_void;
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            SystemParametersInfoW, SPI_GETWORKAREA, SPIF_UPDATEINIFILE,
        };
        unsafe {
            let mut rect = RECT::default();
            let pv: *mut c_void = &mut rect as *mut RECT as *mut c_void;
            SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(pv), SPIF_UPDATEINIFILE)
                .map_err(|e| format!("无法读取工作区：{e}"))?;
            Ok((rect.left, rect.top, rect.right, rect.bottom))
        }
    }
    #[cfg(not(target_os = "windows"))]
    { Ok((0, 0, 1920, 1080)) }
}

fn apply_dock(window: &WebviewWindow, side: &str, collapsed: bool) -> Result<(), String> {
    // Keep the AppBar geometry and fullscreen watcher algorithm unchanged.
    if appbar::ACTIVE.load(Ordering::SeqCst) {
        window.hwnd().map_err(|e| format!("无法取得窗口：{e}"))?;
        window.current_monitor().map_err(|e| e.to_string())?
            .ok_or_else(|| "无法取得停靠显示器".to_string())?;
        appbar::dock(window, side, collapsed);
        return Ok(());
    }
    let (left, top, right, bottom) = primary_work_area()?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let width = (((if collapsed { COLLAPSED_WIDTH } else { FULL_WIDTH }) as f64) * scale)
        .round() as u32;
    let x = if side == "left" { left } else { right - width as i32 };
    window.set_size(PhysicalSize::new(width, (bottom - top) as u32)).map_err(|e| e.to_string())?;
    window.set_position(PhysicalPosition::new(x, top)).map_err(|e| e.to_string())?;
    Ok(())
}

fn apply_settings(window: &WebviewWindow, previous: Option<&Settings>, next: &Settings) -> Result<(), String> {
    next.validate()?;
    if previous.is_none_or(|old| old.ontop != next.ontop) {
        window.set_always_on_top(next.ontop).map_err(|e| format!("置顶设置失败：{e}"))?;
    }
    if previous.is_none_or(|old| old.side != next.side || old.collapsed != next.collapsed) {
        apply_dock(window, &next.side, next.collapsed)?;
    }
    Ok(())
}

#[tauri::command]
fn save_settings(state: tauri::State<AppState>, window: WebviewWindow, settings: Settings) -> Result<(), String> {
    settings.validate()?;
    let mut stores = state.lock()?;
    stores.settings.check_writable()?; // Reject corruption before native changes.
    let previous = display_settings(&stores.applied, stores.error_display);
    let next = display_settings(&settings, stores.error_display);
    let result = apply_settings(&window, Some(&previous), &next)
        .and_then(|_| stores.settings.save(&settings));
    if let Err(error) = result {
        // Roll back only attempted changes: a native operation may have partly
        // succeeded, but toggling completed_expanded must never re-dock the bar.
        return match apply_settings(&window, Some(&next), &previous) {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!("{error}；恢复窗口设置也失败：{rollback}")),
        };
    }
    stores.applied = settings;
    Ok(())
}

fn display_settings(saved: &Settings, error_display: bool) -> Settings {
    let mut effective = saved.clone();
    if error_display { effective.collapsed = false; }
    effective
}

/// Transient, non-persistent expansion keeps recovery/retry controls reachable
/// even when the user's saved layout is a narrow collapsed strip.
#[tauri::command]
fn set_error_display(state: tauri::State<AppState>, window: WebviewWindow, visible: bool) -> Result<(), String> {
    let mut stores = state.lock()?;
    if stores.error_display == visible { return Ok(()); }
    let previous = display_settings(&stores.applied, stores.error_display);
    let next = display_settings(&stores.applied, visible);
    apply_settings(&window, Some(&previous), &next)?;
    stores.error_display = visible;
    Ok(())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    // Frontend must flush its queue first; taking this same mutex also ensures
    // that an in-progress native save has finished before releasing the AppBar.
    let _stores = state.lock()?;
    #[cfg(windows)]
    if let Some(window) = app.get_webview_window("main") {
        if let Some(hwnd) = appbar::get_hwnd(&window) {
            appbar::unregister_hwnd(hwnd);
        }
    }
    app.exit(0);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let instance = InstanceLock::acquire(&dir).map_err(std::io::Error::other)?;
            let mut settings_store = JsonStore::<Settings>::new(dir.join("settings.json"));
            // Corruption is not default data: default ONLY the native appearance
            // so the error UI can be shown. The store stays write-gated, and the
            // frontend's load_settings call receives the real error again.
            let initial = match settings_store.load() {
                Ok(settings) => settings,
                Err(error) => { eprintln!("{error}"); Settings::default() }
            };
            app.manage(AppState(Mutex::new(Stores {
                todos: JsonStore::new(dir.join("todos.json")),
                settings: settings_store,
                applied: initial.clone(),
                error_display: false,
                _instance: instance,
            })));
            if let Some(window) = app.get_webview_window("main") {
                let registered = appbar::register(&window);
                // All persisted native settings apply while visible:false,
                // BEFORE first show and BEFORE starting the fullscreen watcher.
                if let Err(error) = apply_settings(&window, None, &initial) {
                    #[cfg(windows)]
                    if let Some(hwnd) = appbar::get_hwnd(&window) { appbar::unregister_hwnd(hwnd); }
                    return Err(std::io::Error::other(error).into());
                }
                window.show()?;
                std::thread::sleep(std::time::Duration::from_millis(60));
                appbar::reposition(&window);
                if registered {
                    appbar::spawn_watcher(app.handle().clone(), window.clone());
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // WM_CLOSE/Alt+F4 must not bypass unsent frontend snapshots.
                api.prevent_close();
                let _ = window.emit("sideline-close-requested", ());
            }
            if let tauri::WindowEvent::Destroyed = event {
                #[cfg(windows)]
                if let Ok(hwnd) = window.hwnd() {
                    appbar::unregister_hwnd(windows::Win32::Foundation::HWND(hwnd.0));
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_todos,
            save_todos,
            recover_todos,
            preserve_and_reload_todos,
            preserve_and_reload_settings,
            load_settings,
            save_settings,
            set_error_display,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
