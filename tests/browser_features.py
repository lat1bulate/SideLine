"""Focused end-to-end frontend feature acceptance in real headless Edge.
Synthetic tasks and mock Tauri bridge; no production data/files are loaded.
"""
import json
from browser_smoke import ROOT, INIT
from playwright.sync_api import sync_playwright


def setup(browser,extra=''):
    context=browser.new_context(viewport={'width':320,'height':820})
    context.add_init_script(INIT+'\n'+extra)
    page=context.new_page();page.goto((ROOT/'dist'/'index.html').as_uri())
    page.wait_for_function("document.querySelector('#todos-status').textContent.includes('已保存')")
    return context,page


def main():
    results=[]
    with sync_playwright() as p:
        browser=p.chromium.launch(channel='msedge',headless=True)
        ctx,pg=setup(browser)
        assert not pg.locator('#completed-list').is_visible()
        pg.locator('#completed-toggle').click()
        assert pg.locator('#completed-list').is_visible()
        pg.wait_for_function('window.__qa.settings.completed_expanded === true')
        c=pg.locator('#completed-list li.todo');c.locator('.check').uncheck()
        assert pg.locator('#list li.todo').count()==3
        pg.wait_for_function('window.__qa.todos[2].done === false')
        assert pg.evaluate('window.__qa.todos[2].notes[0]')=='完成项注释保留'
        results.append('completed_toggle_restore_notes')
        b=pg.locator('#list li.todo').filter(has_text='任务 B：中文编辑')
        b.hover();b.locator('.del').click()
        pg.wait_for_function('window.__qa.todos.length === 2')
        pg.locator('#undo-delete').click()
        pg.wait_for_function('window.__qa.todos.length === 3')
        assert pg.evaluate('window.__qa.todos[1].notes[0]')=='B 的注释'
        results.append('undo_task_preserves_position_and_notes')
        b=pg.locator('#list li.todo').filter(has_text='任务 B：中文编辑')
        b.locator('.note-line').hover();b.locator('.note-del').click()
        pg.wait_for_function('window.__qa.todos[1].notes.length === 0')
        pg.locator('#topbar').click();pg.keyboard.press('Control+z')
        pg.wait_for_function('window.__qa.todos[1].notes.length === 1')
        results.append('undo_note_keyboard')
        ctx.close()
        ctx,pg=setup(browser,r"""
const original=window.__TAURI__.core.invoke;
window.__qa.fail=true;
window.__TAURI__.core.invoke=async(cmd,args)=>{
 if(cmd==='save_todos' && window.__qa.fail)throw new Error('验收模拟：磁盘写入失败');
 return original(cmd,args);
};
""")
        pg.locator('#new-todo').fill('失败后仍保留的内容');pg.locator('#new-todo').press('Enter')
        pg.wait_for_function("document.querySelector('#todos-status').textContent.includes('保存失败')")
        assert pg.locator('#list').inner_text().find('失败后仍保留的内容')>=0
        pg.locator('#btn-quit').click()
        pg.wait_for_function("document.querySelector('#exit-error').textContent.includes('未退出')")
        assert not pg.evaluate("window.__qa.calls.some(c=>c.cmd==='quit_app')")
        pg.evaluate('window.__qa.fail=false');pg.locator('#todos-retry-save').click()
        pg.wait_for_function('window.__qa.todos.length === 4')
        pg.locator('#btn-quit').click()
        pg.wait_for_function("window.__qa.calls.some(c=>c.cmd==='quit_app')")
        results.append('save_error_retry_and_exit_guard')
        ctx.close();browser.close()
    print(json.dumps({'passed':results},ensure_ascii=False,indent=2))


if __name__=='__main__':main()
