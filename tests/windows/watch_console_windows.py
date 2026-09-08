"""Record console-window show events during an isolated Windows acceptance run.

This observer never opens or hides windows. It writes process identity, not
terminal titles, commands or output. Run alongside the real SSH/browser suite.
"""
import argparse
import ctypes
import json
import sys
import time
from ctypes import wintypes
from pathlib import Path

import psutil


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ready", type=Path, required=True)
    parser.add_argument("--stop", type=Path, required=True)
    parser.add_argument("--seconds", type=float, default=180)
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("requires the interactive Windows desktop")
    if args.seconds <= 0:
        parser.error("--seconds must be positive")
    paths = [path.resolve() for path in (args.output, args.ready, args.stop)]
    if len(set(paths)) != 3 or any(path.exists() for path in paths):
        parser.error("use three distinct, fresh paths; a stale stop file invalidates the run")
    for path in paths:
        path.parent.mkdir(parents=True, exist_ok=True)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(None, wintypes.HANDLE, wintypes.DWORD,
                                      wintypes.HWND, wintypes.LONG, wintypes.LONG,
                                      wintypes.DWORD, wintypes.DWORD)
    user32.SetWinEventHook.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.HMODULE,
                                     callback_type, wintypes.DWORD, wintypes.DWORD,
                                     wintypes.DWORD]
    user32.SetWinEventHook.restype = wintypes.HANDLE
    user32.UnhookWinEvent.argtypes = [wintypes.HANDLE]
    user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.PeekMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND,
                                  wintypes.UINT, wintypes.UINT, wintypes.UINT]
    events = []
    baseline = []

    def record(window, collection):
        name = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(window, name, len(name))
        if name.value not in {"ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS"}:
            return
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
        ancestry = []
        try:
            current = psutil.Process(pid.value)
            for process in [current, *current.parents()]:
                ancestry.append({"pid": process.pid, "name": process.name(), "exe": process.exe()})
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
        collection.append({"at": time.time(), "pid": pid.value,
                           "class": name.value, "ancestry": ancestry})

    @callback_type
    def shown(_hook, _event, window, object_id, child_id, _thread, _time):
        if object_id == 0 and child_id == 0:
            record(window, events)

    # EVENT_OBJECT_SHOW; WINEVENT_OUTOFCONTEXT dispatches callbacks on this
    # thread's message pump, including windows shown only briefly.
    hook = user32.SetWinEventHook(0x8002, 0x8002, None, shown, 0, 0, 0)
    if not hook:
        raise ctypes.WinError(ctypes.get_last_error())
    enum_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    @enum_type
    def existing(window, _param):
        if user32.IsWindowVisible(window):
            record(window, baseline)
        return True

    user32.EnumWindows(existing, 0)
    started = time.monotonic()
    args.ready.write_text(json.dumps({"watching": True}), encoding="utf-8")
    message = wintypes.MSG()
    try:
        while time.monotonic() - started < args.seconds and not args.stop.exists():
            while user32.PeekMessageW(ctypes.byref(message), None, 0, 0, 1):
                user32.TranslateMessage(ctypes.byref(message))
                user32.DispatchMessageW(ctypes.byref(message))
            time.sleep(0.01)
    finally:
        user32.UnhookWinEvent(hook)
        result = {"durationSeconds": round(time.monotonic() - started, 3),
                  "baselineVisibleConsoles": baseline, "consoleShowEvents": events}
        args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"consoleShowEvents": len(events), "baselineVisibleConsoles": len(baseline)}))
    return 1 if events else 0


if __name__ == "__main__":
    raise SystemExit(main())
