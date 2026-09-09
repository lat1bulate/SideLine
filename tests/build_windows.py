"""Windows-only repeatable QA/production build. Run with the Windows Python.
QA identity isolates native tests from actual Sideline data; production never gets
remote-debugging flags embedded. Does not deploy or start an executable.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
QA_ID = 'com.yan.sideline.qa20260908'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=['qa', 'production'])
    args = parser.parse_args()
    if sys.platform != 'win32':
        raise SystemExit('Run with Windows Python/MSVC toolchain')
    env = os.environ.copy()
    env.pop('TAURI_CONFIG', None)
    if args.mode == 'qa':
        env['TAURI_CONFIG'] = json.dumps({'identifier': QA_ID})
    cargo = Path.home() / '.cargo' / 'bin' / 'cargo.exe'
    result = subprocess.run([str(cargo), 'build', '--release', '--offline'],
                            cwd=ROOT / 'src-tauri', env=env)
    if result.returncode:
        raise SystemExit(result.returncode)
    out = ROOT / 'tests' / 'artifacts'
    out.mkdir(parents=True, exist_ok=True)
    dest = out / ('sideline-qa.exe' if args.mode == 'qa' else 'Sideline.exe')
    shutil.copy2(ROOT / 'src-tauri' / 'target' / 'release' / 'sideline.exe', dest)
    print(json.dumps({'mode': args.mode, 'exe': str(dest), 'bytes': dest.stat().st_size}), flush=True)


if __name__ == '__main__':
    main()
