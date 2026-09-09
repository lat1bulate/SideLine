//! Windows AppBar registration (reserve right/left edge space so maximized windows
//! avoid this window) + foreground-fullscreen detection (auto-hide).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewWindow};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Shell::{
    APPBARDATA, SHAppBarMessage, ABE_LEFT, ABE_RIGHT, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE,
    ABM_SETPOS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
};

use crate::{COLLAPSED_WIDTH, FULL_WIDTH};

/// Whether the AppBar registration succeeded (and thus the window should be
/// positioned via the AppBar API rather than the plain work-area fallback).
pub static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The last docked rect, so we can re-assert the window position without
/// re-running ABM_SETPOS (which makes Explorer re-layout the desktop icons).
static LAST_RECT: Mutex<Option<RECT>> = Mutex::new(None);

fn abd_init(hwnd: HWND, edge: u32, rc: RECT) -> APPBARDATA {
    APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: 0,
        uEdge: edge,
        rc,
        lParam: Default::default(),
    }
}

/// Extract the native HWND (works whether tauri's `windows` crate version
/// matches ours or not — both expose the pointer as field `.0`).
pub fn get_hwnd(window: &WebviewWindow) -> Option<HWND> {
    let h = window.hwnd().ok()?;
    Some(HWND(h.0))
}

pub fn register(window: &WebviewWindow) -> bool {
    let Some(hwnd) = get_hwnd(window) else {
        return false;
    };
    let mut abd = abd_init(hwnd, ABE_RIGHT, RECT::default());
    let ok = unsafe { SHAppBarMessage(ABM_NEW, &mut abd) != 0 };
    ACTIVE.store(ok, Ordering::SeqCst);
    ok
}

pub fn unregister_hwnd(hwnd: HWND) {
    let mut abd = abd_init(hwnd, ABE_RIGHT, RECT::default());
    unsafe {
        SHAppBarMessage(ABM_REMOVE, &mut abd);
    }
    ACTIVE.store(false, Ordering::SeqCst);
}

/// Ask the system for the appbar rect, move the window there, then reserve it.
pub fn dock(window: &WebviewWindow, side: &str, collapsed: bool) {
    let Some(hwnd) = get_hwnd(window) else {
        return;
    };
    // Width constants are logical (CSS) px — scale to physical for Win32.
    let scale = window.scale_factor().unwrap_or(1.0);
    let width = (((if collapsed { COLLAPSED_WIDTH } else { FULL_WIDTH }) as f64) * scale)
        .round() as i32;
    let Ok(Some(mon)) = window.current_monitor() else {
        return;
    };
    let p = mon.position();
    let s = mon.size();
    let ml = p.x;
    let mt = p.y;
    let mr = ml + s.width as i32;
    let mb = mt + s.height as i32;

    let edge = if side == "left" { ABE_LEFT } else { ABE_RIGHT };
    // Desired rect, anchored to the monitor edge at full height. The system's
    // ABM_QUERYPOS clamps the rect into the *current* work area — which has
    // already been shrunk by our own existing reservation — so the window
    // drifts away from the screen edge whenever the width changes (collapse/
    // expand). Take only the vertical span from QUERYPOS and re-anchor the
    // horizontal extents to the monitor edge ourselves.
    let mut abd = abd_init(hwnd, edge, RECT::default());
    unsafe {
        abd.rc = if side == "left" {
            RECT { left: ml, top: mt, right: ml + width, bottom: mb }
        } else {
            RECT { left: mr - width, top: mt, right: mr, bottom: mb }
        };
        SHAppBarMessage(ABM_QUERYPOS, &mut abd);
        let q = abd.rc;
        let r = if side == "left" {
            RECT { left: ml, top: q.top, right: ml + width, bottom: q.bottom }
        } else {
            RECT { left: mr - width, top: q.top, right: mr, bottom: q.bottom }
        };
        *LAST_RECT.lock().unwrap() = Some(r);
        // Move with raw Win32 call (true physical coords) — tao's set_position
        // mis-scales on high-DPI displays.
        let _ = SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        abd.rc = r;
        SHAppBarMessage(ABM_SETPOS, &mut abd);
    }
    // tao re-applies its own (DPI-mis-scaled) geometry right after a window
    // size change, briefly overriding the position we just set. Compensate
    // immediately a couple of times instead of waiting for the periodic
    // watcher (up to 600ms+).
    for ms in [120u64, 450] {
        let w = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            reposition(&w);
        });
    }
}

/// Re-assert the docked window position WITHOUT re-running ABM_SETPOS
/// (re-reserving repeatedly makes Explorer re-layout the desktop icons).
pub fn reposition(window: &WebviewWindow) {
    let Some(hwnd) = get_hwnd(window) else {
        return;
    };
    let Some(r) = *LAST_RECT.lock().unwrap() else {
        return;
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// True when the foreground window is a different window covering the whole
/// monitor (i.e. a real fullscreen app like a video player or a game).
fn foreground_is_fullscreen(app: &AppHandle, self_hwnd: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg.0 == self_hwnd.0 {
            return false;
        }
        let mut rc = RECT::default();
        if !GetWindowRect(fg, &mut rc).is_ok() {
            return false;
        }
        if let Some(window) = app.get_webview_window("main") {
            if let Ok(Some(mon)) = window.current_monitor() {
                let p = mon.position();
                let s = mon.size();
                let m = RECT {
                    left: p.x,
                    top: p.y,
                    right: p.x + s.width as i32,
                    bottom: p.y + s.height as i32,
                };
                let tol = 8;
                return rc.left <= m.left + tol
                    && rc.top <= m.top + tol
                    && rc.right >= m.right - tol
                    && rc.bottom >= m.bottom - tol;
            }
        }
    }
    false
}

/// Poll the foreground window; hide our window while a real fullscreen app is
/// active and bring it back (re-docked) once it's gone.
pub fn spawn_watcher(app: AppHandle, window: WebviewWindow) {
    std::thread::spawn(move || {
        let self_hwnd = get_hwnd(&window);
        let mut hidden = false;
        loop {
            std::thread::sleep(Duration::from_millis(600));
            let full = match self_hwnd {
                Some(h) => foreground_is_fullscreen(&app, h),
                None => false,
            };
            if full != hidden {
                hidden = full;
                let _ = if full { window.hide() } else { window.show() };
            }
            // Periodically re-assert the docked window position (tao can move
            // it when it shows / resizes / after DPI changes) without
            // re-reserving. Runs every tick (~600ms) so a stray re-position
            // is corrected within one heartbeat.
            if !hidden {
                reposition(&window);
            }
        }
    });
}
