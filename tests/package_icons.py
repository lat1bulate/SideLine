"""Restore PNG-backed transparent application icons after tauri-winres.
Takes an explicit executable path; verifies EndUpdateResourceW and preserves the
original executable on any failure. Run only while target EXE is not running.
"""
import argparse
import ctypes
from ctypes import wintypes
from pathlib import Path
import shutil
import struct

ROOT=Path(__file__).resolve().parents[1]
SIZES=[16,32,48,64,128,256]


def main():
    parser=argparse.ArgumentParser();parser.add_argument('exe',type=Path);args=parser.parse_args()
    target=args.exe.resolve()
    if not target.is_file():raise SystemExit('Executable not found')
    staged=target.with_suffix('.icon-staged.exe')
    shutil.copy2(target,staged)
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    begin=kernel.BeginUpdateResourceW;begin.argtypes=[wintypes.LPCWSTR,wintypes.BOOL];begin.restype=wintypes.HANDLE
    update=kernel.UpdateResourceW;update.argtypes=[wintypes.HANDLE,wintypes.LPCWSTR,wintypes.LPCWSTR,wintypes.WORD,wintypes.LPVOID,wintypes.DWORD];update.restype=wintypes.BOOL
    end=kernel.EndUpdateResourceW;end.argtypes=[wintypes.HANDLE,wintypes.BOOL];end.restype=wintypes.BOOL
    intres=lambda x:ctypes.cast(ctypes.c_void_p(x),wintypes.LPCWSTR)
    handle=begin(str(staged),False)
    if not handle:staged.unlink(missing_ok=True);raise ctypes.WinError(ctypes.get_last_error())
    committed=False
    try:
        group=struct.pack('<HHH',0,1,len(SIZES))
        for i,size in enumerate(SIZES,1):
            data=(ROOT/'icon_pngs'/f'icon_{size}.png').read_bytes()
            assert data.startswith(b'\x89PNG\r\n\x1a\n')
            buf=ctypes.create_string_buffer(data)
            if not update(handle,intres(3),intres(i),1033,buf,len(data)):raise ctypes.WinError(ctypes.get_last_error())
            group+=struct.pack('<BBBBHHIH',size%256,size%256,0,0,1,32,len(data),i)
        buf=ctypes.create_string_buffer(group)
        if not update(handle,intres(14),intres(32512),1033,buf,len(group)):raise ctypes.WinError(ctypes.get_last_error())
        ok=end(handle,False);handle=None
        if not ok:raise ctypes.WinError(ctypes.get_last_error())
        staged.replace(target);committed=True
    finally:
        if handle:end(handle,True)
        if not committed:staged.unlink(missing_ok=True)
    print(f'Icon update committed: {target}')


if __name__=='__main__':main()
