"""Native Windows/WebView2 smoke test, QA build only (separate Tauri identifier).
The production data directory is never used for fixtures. Debug endpoint binds to
local loopback only and dies with the QA process. Run after build_windows.py qa.
"""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

from playwright.sync_api import sync_playwright

ROOT=Path(__file__).resolve().parents[1]
QA_ID='com.yan.sideline.qa20260908'
DATA=Path(os.environ['APPDATA'])/QA_ID
EXE=ROOT/'tests'/'artifacts'/'sideline-qa.exe'
PORT=19327
OUT=ROOT/'tests'/'artifacts'

user32=ctypes.WinDLL('user32',use_last_error=True)
user32.SetProcessDPIAware()
user32.GetWindowRect.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.RECT)]
user32.GetWindowThreadProcessId.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.DWORD)]
user32.GetDpiForWindow.argtypes=[wintypes.HWND]
user32.GetDpiForWindow.restype=wintypes.UINT
user32.IsWindowVisible.argtypes=[wintypes.HWND]
user32.GetWindowLongW.argtypes=[wintypes.HWND,ctypes.c_int]
CALLBACK=ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
user32.EnumWindows.argtypes=[CALLBACK,wintypes.LPARAM]


def work_area():
    rect=wintypes.RECT()
    if not user32.SystemParametersInfoW(0x0030,0,ctypes.byref(rect),0):
        raise ctypes.WinError(ctypes.get_last_error())
    return [rect.left,rect.top,rect.right,rect.bottom]


def geometry(pid):
    result=[]
    @CALLBACK
    def enum(h,_):
        owner=wintypes.DWORD();user32.GetWindowThreadProcessId(h,ctypes.byref(owner))
        if owner.value==pid and user32.IsWindowVisible(h):
            rect=wintypes.RECT();user32.GetWindowRect(h,ctypes.byref(rect))
            if rect.right>rect.left:
                result.append({'x':rect.left,'y':rect.top,'width':rect.right-rect.left,
                               'height':rect.bottom-rect.top,'dpi':user32.GetDpiForWindow(h),
                               'ontop':bool(user32.GetWindowLongW(h,-20)&8)})
        return True
    user32.EnumWindows(enum,0)
    return result[0] if result else None


def wait_until(fn,timeout=20):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        try:
            result=fn()
            if result:return result
        except (OSError,ValueError):pass
        time.sleep(.1)
    raise AssertionError('readiness/condition timed out')


def start(p):
    import socket
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    endpoint=f'http://127.0.0.1:{port}'
    env=os.environ.copy()
    env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS']=f'--remote-debugging-port={port} --remote-debugging-address=127.0.0.1'
    env['WEBVIEW2_USER_DATA_FOLDER']=str(OUT/f'qa-webview-{port}')
    proc=subprocess.Popen([str(EXE)],env=env,cwd=EXE.parent)
    try:
        wait_until(lambda: urllib.request.urlopen(endpoint+'/json/version',timeout=1).status==200)
        browser=p.chromium.connect_over_cdp(endpoint)
        cdp=browser.new_browser_cdp_session()
        def discover_page():
            # Pump the synchronous Playwright connection while WebView2 creates
            # its page; sleeping alone does not dispatch target-created events.
            cdp.send('Target.getTargets')
            return next((pg for c in browser.contexts for pg in c.pages if pg.url!='about:blank'),None)
        page=wait_until(discover_page)
        page.wait_for_function('!!window.__TAURI__?.core?.invoke')
        return proc,browser,page
    except Exception:
        proc.terminate();proc.wait(timeout=10);raise


def call(page,name,args=None):
    return page.evaluate('([name,args])=>window.__TAURI__.core.invoke(name,args)',[name,args])


def stop(proc,page):
    try:page.evaluate("()=>{window.__TAURI__.core.invoke('quit_app');}")
    except Exception:
        if proc.poll() is None:raise
    proc.wait(timeout=10)
    assert proc.returncode==0,proc.returncode


def main():
    assert DATA.name==QA_ID and '.qa' in QA_ID
    assert EXE.exists(),'Build the QA executable first'
    DATA.mkdir(parents=True,exist_ok=True);OUT.mkdir(parents=True,exist_ok=True)
    # Only overwrite this disposable QA identity's data.
    (DATA/'todos.json').write_text(json.dumps([
        {'text':'原生验收 A（旧格式）','done':False,'created':1},
        {'text':'原生验收 B','done':True,'created':2,'notes':['注释保留']}
    ],ensure_ascii=False),encoding='utf-8')
    initial={'side':'left','collapsed':True,'ontop':False,'completed_expanded':True}
    (DATA/'settings.json').write_text(json.dumps(initial),encoding='utf-8')
    results={}
    original_work_area=work_area()
    with sync_playwright() as p:
        proc,browser,page=start(p)
        try:
            assert call(page,'load_settings')==initial
            page.wait_for_function("document.body.classList.contains('collapsed')")
            geo=wait_until(lambda:geometry(proc.pid))
            assert abs(geo['width']-round(26*geo['dpi']/96))<=2,geo
            assert geo['ontop'] is False,geo
            results['restored_collapsed_left_notop']=geo
            todos=call(page,'load_todos');assert todos[0]['notes']==[]
            todos[0]['text']='原生验收 A：已保存';call(page,'save_todos',{'todos':todos})
            assert json.loads((DATA/'todos.json').read_text(encoding='utf-8'))==todos
            assert (DATA/'todos.json.bak').exists()
            results['legacy_load_and_atomic_save']=True
            new={'side':'right','collapsed':False,'ontop':True,'completed_expanded':False}
            call(page,'save_settings',{'settings':new})
            page.reload();page.wait_for_selector('li.todo')
            page.wait_for_function("!document.body.classList.contains('collapsed')")
            def expanded():
                g=geometry(proc.pid)
                return g if g and abs(g['width']-round(320*g['dpi']/96))<=2 and g['ontop'] else None
            results['expanded_right_top']=wait_until(expanded)
            page.screenshot(path=str(OUT/'native-qa.png'))
            stop(proc,page)
        finally:
            if proc.poll() is None:proc.terminate();proc.wait(timeout=10)
        proc,browser,page=start(p)
        try:
            assert call(page,'load_settings')==new
            assert call(page,'load_todos')[0]['text']=='原生验收 A：已保存'
            page.wait_for_selector('li.todo')
            results['restart_persistence']=True
            stop(proc,page)
        finally:
            if proc.poll() is None:proc.terminate();proc.wait(timeout=10)
    wait_until(lambda:work_area()==original_work_area)
    results['work_area_restored_on_exit']=work_area()
    (OUT/'native-results.json').write_text(json.dumps(results,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps(results,ensure_ascii=False,indent=2))


if __name__=='__main__':main()
