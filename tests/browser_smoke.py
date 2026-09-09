"""Real Edge DOM regressions, using synthetic fixtures and mocked Tauri IPC only.
Windows: tests/.venv/Scripts/python.exe tests/browser_smoke.py [--baseline path]
No production tasks, browser profile, or running Sideline instance is accessed.
"""
import argparse
import json
from pathlib import Path
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
INIT = r"""
window.__qa = {calls:[], todos:[
 {text:'任务 A：回归验证',done:false,created:1,notes:['A 的注释']},
 {text:'任务 B：中文编辑',done:false,created:2,notes:['B 的注释']},
 {text:'任务 C：已完成',done:true,created:3,notes:['完成项注释保留']}
], settings:{side:'right',collapsed:false,ontop:true,completed_expanded:false}};
window.__TAURI__={core:{invoke:async (cmd,args)=>{
 window.__qa.calls.push({cmd,args:structuredClone(args)});
 if(cmd==='load_todos'||cmd==='recover_todos')return structuredClone(window.__qa.todos);
 if(cmd==='load_settings')return structuredClone(window.__qa.settings);
 if(cmd==='save_todos')window.__qa.todos=structuredClone(args.todos);
 if(cmd==='save_settings')window.__qa.settings=structuredClone(args.settings);
}}};
"""


def notes_scope(page):
    b = page.locator('li.todo').filter(has_text='任务 B：中文编辑')
    b.locator('.note-text').dblclick()
    assert b.locator('.note-editor').count() == 1, 'note editor attached to wrong task'
    b.locator('.note-editor').fill('B 修改后')
    b.locator('.note-editor').press('Enter')
    page.wait_for_function("window.__qa.todos[1].notes[0] === 'B 修改后'")
    assert page.evaluate('window.__qa.todos[0].notes[0]') == 'A 的注释'


def ime_enter(page):
    page.locator('#new-todo').fill('中文候选')
    page.locator('#new-todo').dispatch_event('keydown', {'key':'Enter','isComposing':True,'keyCode':229,'bubbles':True})
    assert page.locator('#new-todo').input_value() == '中文候选', 'IME Enter cleared input'
    assert page.evaluate('window.__qa.todos.length') == 3, 'IME Enter added task'
    page.locator('#new-todo').press('Enter')
    page.wait_for_function('window.__qa.todos.length === 4')


def note_not_drag(page):
    note=page.locator('li.todo').filter(has_text='任务 B：中文编辑').locator('.note-text')
    a=page.locator('li.todo').filter(has_text='任务 A：回归验证').locator('.text')
    nbox=note.bounding_box(); abox=a.bounding_box()
    page.mouse.move(nbox['x']+10,nbox['y']+5)
    page.mouse.down()
    page.mouse.move(abox['x']+10,abox['y']+5,steps=5)
    page.mouse.up()
    page.wait_for_timeout(100)
    assert page.evaluate('window.__qa.todos[0].created') == 1, 'dragging note reordered tasks'


def main():
    args=argparse.ArgumentParser();args.add_argument('--baseline',type=Path); opts=args.parse_args()
    source=(opts.baseline or ROOT)/'dist'/'index.html'
    results=[]
    with sync_playwright() as p:
        browser=p.chromium.launch(channel='msedge',headless=True)
        for name,test in [('scoped_note_editor',notes_scope),('ime_enter',ime_enter),('note_not_drag',note_not_drag)]:
            context=browser.new_context(viewport={'width':320,'height':820})
            context.add_init_script(INIT)
            page=context.new_page();page.goto(source.as_uri())
            page.wait_for_selector('li.todo')
            try:
                test(page);results.append({'test':name,'passed':True})
            except Exception as e:
                results.append({'test':name,'passed':False,'error':str(e)[:300]})
            context.close()
        browser.close()
    print(json.dumps({'mode':'baseline' if opts.baseline else 'current','tests':results},ensure_ascii=False,indent=2))
    if not opts.baseline and any(not r['passed'] for r in results):
        raise SystemExit(1)
    if opts.baseline and any(r['passed'] for r in results):
        raise SystemExit('Expected all baseline regressions to reproduce')


if __name__=='__main__':main()
