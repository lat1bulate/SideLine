"""External-conflict recovery and settings-failure accessibility regressions."""
import json
from browser_smoke import ROOT, INIT
from playwright.sync_api import sync_playwright


def main():
    results=[]
    with sync_playwright() as p:
        browser=p.chromium.launch(channel='msedge',headless=True)
        for name,extra in [
            ('settings_failure_completed_access',r"""
const orig=window.__TAURI__.core.invoke;
window.__TAURI__.core.invoke=(c,a)=>c==='load_settings'?Promise.reject('设置损坏'):orig(c,a);
"""),
            ('preserve_reload_external_conflict',r"""
const orig=window.__TAURI__.core.invoke;window.__qa.conflict=true;
window.__TAURI__.core.invoke=async(c,a)=>{
 if(c==='save_todos' && window.__qa.conflict)throw '文件已被外部修改';
 if(c==='preserve_and_reload_todos'){
  window.__qa.archived=structuredClone(a.todos);window.__qa.conflict=false;
  const value=[{text:'外部版本',done:false,created:7,notes:[]}];
  window.__qa.todos=structuredClone(value);
  return {value,error:null,preserved_path:'QA/todos.json.unsaved-test'};
 }
 return orig(c,a);
};
"""),
            ('archive_failure_keeps_dirty_state',r"""
const orig=window.__TAURI__.core.invoke;
window.__TAURI__.core.invoke=(c,a)=>['save_todos','preserve_and_reload_todos'].includes(c)?Promise.reject('模拟磁盘满'):orig(c,a);
"""),
        ]:
            ctx=browser.new_context(viewport={'width':320,'height':820});ctx.add_init_script(INIT+'\n'+extra)
            pg=ctx.new_page();pg.goto((ROOT/'dist'/'index.html').as_uri());pg.wait_for_selector('li.todo')
            try:
                if name=='settings_failure_completed_access':
                    assert not pg.locator('#completed-toggle').is_disabled(),'completed section inaccessible'
                    pg.locator('#completed-toggle').click()
                    assert pg.locator('#completed-list').is_visible()
                    assert not pg.evaluate("window.__qa.calls.some(c=>c.cmd==='save_settings')")
                else:
                    pg.locator('#new-todo').fill('本地尚未保存');pg.locator('#new-todo').press('Enter')
                    pg.wait_for_function("document.querySelector('#todos-status').textContent.includes('保存失败')")
                    assert pg.locator('#todos-preserve-reload').count(),'no preserve/reload escape path'
                    pg.locator('#todos-preserve-reload').click()
                    if name=='preserve_reload_external_conflict':
                        pg.wait_for_function("document.querySelector('#list').textContent.includes('外部版本')")
                        assert pg.evaluate("window.__qa.archived.some(t=>t.text==='本地尚未保存')")
                        assert 'unsaved-test' in pg.locator('#recovery-notice').inner_text()
                        pg.locator('#new-todo').fill('重读后新增');pg.locator('#new-todo').press('Enter')
                        pg.wait_for_function('window.__qa.todos.length===2')
                    else:
                        pg.wait_for_function("!document.querySelector('#todos-preserve-reload').disabled")
                        assert '本地尚未保存' in pg.locator('#list').inner_text()
                        pg.locator('#btn-quit').click()
                        pg.wait_for_function("document.querySelector('#exit-error').textContent.includes('未退出')")
                        assert not pg.evaluate("window.__qa.calls.some(c=>c.cmd==='quit_app')")
                results.append({'test':name,'passed':True})
            except Exception as e:results.append({'test':name,'passed':False,'error':str(e)[:200]})
            ctx.close()
        browser.close()
    print(json.dumps(results,ensure_ascii=False,indent=2))
    if any(not r['passed'] for r in results):raise SystemExit(1)


if __name__=='__main__':main()
