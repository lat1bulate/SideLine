"""Deploy an already verified production build, retaining rollback artifacts.
Refuses to overwrite an active Sideline. No synthetic production data is written.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
from datetime import datetime
from native_smoke import ROOT, geometry, wait_until


def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    running=subprocess.check_output(['powershell.exe','-NoProfile','-Command',
        "Get-Process | Where-Object {$_.ProcessName -eq 'Sideline'} | ForEach-Object {$_.Id}"],text=True).strip()
    if running:raise SystemExit('Sideline is running; refusing unsafe replacement: '+running)
    source=ROOT/'tests'/'artifacts'/'Sideline.exe'
    dest=Path.home()/'Desktop'/'Sideline.exe'
    data=Path(os.environ['APPDATA'])/'com.yan.sideline'
    previous={p.name:digest(p) for p in data.glob('*.json*')} if data.exists() else {}
    backup=ROOT/'backups'/('predeploy-'+datetime.now().strftime('%Y%m%d-%H%M%S'))
    backup.mkdir(parents=True,exist_ok=False)
    if dest.exists():shutil.copy2(dest,backup/'Sideline.exe')
    if data.exists():shutil.copytree(data,backup/'appdata')
    staged=dest.with_name('.Sideline-update.exe')
    shutil.copy2(source,staged)
    assert digest(staged)==digest(source)
    os.replace(staged,dest)
    env=os.environ.copy()
    for key in ['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS','WEBVIEW2_USER_DATA_FOLDER','TAURI_CONFIG']:
        env.pop(key,None)
    proc=subprocess.Popen([str(dest)],cwd=dest.parent,env=env,
                          stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    # Ignore tiny transient startup/helper windows; wait for the real dock.
    g=wait_until(lambda:(lambda v:v if v and v['height']>200 and v['width']>=26 else None)(geometry(proc.pid)))
    assert proc.poll() is None
    after={p.name:digest(p) for p in data.glob('*.json*')} if data.exists() else {}
    assert after==previous,'Production JSON changed unexpectedly during read-only startup'
    result={'deployed':str(dest),'backup':str(backup),'pid':proc.pid,'geometry':g,
            'sha256':digest(dest),'bytes':dest.stat().st_size,'existing_json_files':len(previous),
            'production_json_unchanged':after==previous}
    (ROOT/'tests'/'artifacts'/'deployment.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps(result,ensure_ascii=False,indent=2))


if __name__=='__main__':main()
