"""Integration regression gates for native-close/error-display and queue boundaries.
These use real Edge JS/DOM, with only the native bridge replaced.
"""
import json
from browser_smoke import ROOT, INIT
from playwright.sync_api import sync_playwright


def main():
    results=[]
    with sync_playwright() as p:
        browser=p.chromium.launch(channel='msedge',headless=True)
        for name,extra,body in [
            ('clean_load_failure_can_exit',r"""
const orig=window.__TAURI__.core.invoke;
window.__TAURI__.core.invoke=(c,a)=>c==='load_todos'?Promise.reject('损坏数据，仅测试'):orig(c,a);
""",'load_failure'),
            ('collapsed_error_requests_native_expansion',r"""
window.__qa.settings.collapsed=true;
const orig=window.__TAURI__.core.invoke;
window.__TAURI__.core.invoke=(c,a)=>c==='load_todos'?Promise.reject('损坏数据，仅测试'):orig(c,a);
""",'error_display'),
            ('native_close_uses_frontend_flush',r"""
window.__qa.handlers={};window.__TAURI__.event={listen:async(n,f)=>{window.__qa.handlers[n]=f;return ()=>{};}};
""",'native_close'),
            ('queue_microtask_boundary_does_not_stall',r"""
const orig=window.__TAURI__.core.invoke;
window.__qa.blockNext=true;
window.__TAURI__.core.invoke=(c,a)=>{
 if(c==='save_todos'&&window.__qa.blockNext){window.__qa.blockNext=false;
   window.__qa.savePromise=new Promise(resolve=>{window.__qa.release=()=>{orig(c,a);resolve();};});
   return window.__qa.savePromise;
 }
 return orig(c,a);
};
""",'queue_race'),
        ]:
            ctx=browser.new_context(viewport={'width':320,'height':820})
            ctx.add_init_script(INIT+'\n'+extra)
            pg=ctx.new_page();pg.goto((ROOT/'dist'/'index.html').as_uri())
            pg.wait_for_function("!document.querySelector('#settings-status').textContent.includes('读取中')")
            try:
                if body=='load_failure':
                    pg.locator('#btn-quit').click()
                    pg.wait_for_function("window.__qa.calls.some(c=>c.cmd==='quit_app')",timeout=2000)
                elif body=='error_display':
                    pg.wait_for_function("window.__qa.calls.some(c=>c.cmd==='set_error_display' && c.args.visible===true)",timeout=2000)
                elif body=='native_close':
                    pg.wait_for_function("typeof window.__qa.handlers['sideline-close-requested']==='function'",timeout=2000)
                    pg.locator('#new-todo').fill('原生关闭前提交');pg.locator('#new-todo').press('Enter')
                    pg.evaluate("window.__qa.handlers['sideline-close-requested']({payload:null})")
                    pg.wait_for_function("window.__qa.calls.some(c=>c.cmd==='quit_app')",timeout=2000)
                    assert pg.evaluate('window.__qa.todos.length')==4
                else:
                    pg.locator('#new-todo').fill('第一条');pg.locator('#new-todo').press('Enter')
                    pg.evaluate("""()=>{window.__qa.savePromise.then(()=>{
                        document.querySelector('#new-todo').value='边界第二条';
                        document.querySelector('#btn-add').click();
                    });window.__qa.release();}""")
                    pg.wait_for_function('window.__qa.todos.length===5',timeout=2000)
                results.append({'test':name,'passed':True})
            except Exception as e:
                results.append({'test':name,'passed':False,'error':str(e)[:220]})
            ctx.close()
        browser.close()
    print(json.dumps(results,ensure_ascii=False,indent=2))
    if any(not r['passed'] for r in results):raise SystemExit(1)


if __name__=='__main__':main()
