"""Native edge cases: collapsed corruption/recovery and WM_CLOSE with save errors.
Fixtures belong only to com.yan.sideline.qa20260908. No OS cursor input.
"""
import ctypes
import json
import os
import stat
from native_smoke import *


def fixture(collapsed=False, corrupt=False):
    DATA.mkdir(parents=True,exist_ok=True)
    for path in [DATA/'todos.json',DATA/'todos.json.bak']:
        if path.exists():os.chmod(path,stat.S_IWRITE|stat.S_IREAD)
    value=[{'text':'边界测试原数据','done':False,'created':1,'notes':[]}]
    raw=json.dumps(value,ensure_ascii=False)
    (DATA/'todos.json').write_text('malformed QA only' if corrupt else raw,encoding='utf-8')
    (DATA/'todos.json.bak').write_text(raw,encoding='utf-8')
    (DATA/'settings.json').write_text(json.dumps({'side':'right','collapsed':collapsed,'ontop':True,'completed_expanded':False}),encoding='utf-8')


def wmclose(pid):
    handles=[]
    @CALLBACK
    def enum(h,_):
        owner=wintypes.DWORD();user32.GetWindowThreadProcessId(h,ctypes.byref(owner))
        if owner.value==pid and user32.IsWindowVisible(h):handles.append(h)
        return True
    user32.EnumWindows(enum,0)
    assert handles
    user32.PostMessageW.argtypes=[wintypes.HWND,wintypes.UINT,wintypes.WPARAM,wintypes.LPARAM]
    assert user32.PostMessageW(handles[0],0x0010,0,0)


def main():
    results=[]
    with sync_playwright() as p:
        fixture(collapsed=True,corrupt=True)
        proc,browser,page=start(p)
        try:
            page.wait_for_function("!document.querySelector('#todos-load-error').hidden")
            def expanded():
                g=geometry(proc.pid);return g if g and abs(g['width']-round(320*g['dpi']/96))<=2 else None
            wait_until(expanded,4)
            page.locator('#todos-recover').click()
            page.wait_for_function("document.querySelector('#todos-load-error').hidden")
            def collapsed():
                g=geometry(proc.pid);return g if g and abs(g['width']-round(26*g['dpi']/96))<=2 else None
            wait_until(collapsed)
            assert call(page,'load_settings')['collapsed'] is True
            assert list(DATA.glob('todos.json.corrupt-*'))
            results.append('native_error_expands_and_recovery_restores_collapse')
            stop(proc,page)
        finally:
            if proc.poll() is None:proc.terminate();proc.wait(timeout=10)
        fixture()
        proc,browser,page=start(p)
        try:
            page.wait_for_selector('li.todo')
            bak=DATA/'todos.json.bak'
            os.chmod(bak,stat.S_IREAD)
            page.locator('#new-todo').fill('拒绝关闭前的待保存项');page.locator('#new-todo').press('Enter')
            page.wait_for_function("document.querySelector('#todos-status').textContent.includes('保存失败')")
            wmclose(proc.pid)
            page.wait_for_function("document.querySelector('#exit-error').textContent.includes('未退出')",timeout=5000)
            assert proc.poll() is None
            os.chmod(bak,stat.S_IWRITE|stat.S_IREAD)
            page.locator('#todos-retry-save').click()
            page.wait_for_function("document.querySelector('#todos-status').textContent.includes('已保存')")
            wmclose(proc.pid)
            # Pump WebView events while frontend asynchronously flushes then exits.
            end=time.monotonic()+10
            while proc.poll() is None and time.monotonic()<end:
                try:page.wait_for_timeout(100)
                except Exception:break
            proc.wait(timeout=10)
            assert proc.returncode==0
            assert len(json.loads((DATA/'todos.json').read_text(encoding='utf-8')))==2
            results.append('wm_close_blocked_until_failed_write_is_retried')
        finally:
            if (DATA/'todos.json.bak').exists():os.chmod(DATA/'todos.json.bak',stat.S_IWRITE|stat.S_IREAD)
            if proc.poll() is None:proc.terminate();proc.wait(timeout=10)
    print(json.dumps({'passed':results},ensure_ascii=False,indent=2))


if __name__=='__main__':main()
