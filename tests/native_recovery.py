"""Native conflict recovery and repaired settings application, QA identity only."""
from native_edge_cases import fixture
from native_smoke import *


def main():
    results=[]
    with sync_playwright() as p:
        fixture()
        proc,browser,page=start(p)
        try:
            page.wait_for_selector('li.todo')
            external=[{'text':'外部改动版本','done':False,'created':7,'notes':[]}]
            raw=json.dumps(external,ensure_ascii=False)
            (DATA/'todos.json').write_text(raw,encoding='utf-8')
            page.locator('#new-todo').fill('本地未保存版本');page.locator('#new-todo').press('Enter')
            page.wait_for_function("document.querySelector('#todos-status').textContent.includes('保存失败')")
            before=set(DATA.glob('todos.json.unsaved-*'))
            page.locator('#todos-preserve-reload').click()
            page.wait_for_function("document.querySelector('#list').textContent.includes('外部改动版本')")
            archives=set(DATA.glob('todos.json.unsaved-*'))-before
            assert len(archives)==1
            archived=json.loads(next(iter(archives)).read_text(encoding='utf-8'))
            assert any(t['text']=='本地未保存版本' for t in archived)
            assert (DATA/'todos.json').read_text(encoding='utf-8')==raw
            page.locator('#new-todo').fill('重读后正常保存');page.locator('#new-todo').press('Enter')
            page.wait_for_function("document.querySelector('#todos-status').textContent.includes('已保存')")
            assert len(call(page,'load_todos'))==2
            results.append('external_and_unsaved_snapshots_preserved_reload_unblocks')
            stop(proc,page)
        finally:
            if proc.poll() is None:proc.terminate();proc.wait(timeout=10)
        fixture()
        invalid='settings-corrupt-QA-only'
        (DATA/'settings.json').write_text(invalid,encoding='utf-8')
        proc,browser,page=start(p)
        try:
            page.wait_for_function("!document.querySelector('#settings-load-error').hidden")
            page.locator('#list .check').check()
            page.locator('#completed-toggle').click()
            assert page.locator('#completed-list li.todo').is_visible()
            assert (DATA/'settings.json').read_text(encoding='utf-8')==invalid
            fixed={'side':'left','collapsed':True,'ontop':False,'completed_expanded':True}
            raw=json.dumps(fixed)
            (DATA/'settings.json').write_text(raw,encoding='utf-8')
            page.locator('#settings-retry-load').click()
            page.wait_for_function("document.body.classList.contains('collapsed')")
            def repaired_native():
                g=geometry(proc.pid)
                return g if g and g['x']==0 and not g['ontop'] and abs(g['width']-round(26*g['dpi']/96))<=2 else None
            wait_until(repaired_native)
            assert (DATA/'settings.json').read_text(encoding='utf-8')==raw
            results.append('completed_access_without_settings_and_native_repair_reload')
            stop(proc,page)
        finally:
            if proc.poll() is None:proc.terminate();proc.wait(timeout=10)
    print(json.dumps({'passed':results},ensure_ascii=False,indent=2))


if __name__=='__main__':main()
