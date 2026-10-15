"""Exercise real Win32 console handles and the same input path as a typed key."""
import ctypes
from ctypes import wintypes as w

kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.GetStdHandle.restype = w.HANDLE
kernel.GetStdHandle.argtypes = [w.DWORD]
handle = kernel.GetStdHandle(-10 & 0xFFFFFFFF)
mode = w.DWORD()
kernel.GetConsoleMode.argtypes = [w.HANDLE, ctypes.POINTER(w.DWORD)]
assert kernel.GetConsoleMode(handle, ctypes.byref(mode)), ctypes.get_last_error()

class KeyEvent(ctypes.Structure):
    _fields_ = [("down", w.BOOL), ("repeat", w.WORD), ("virtual_key", w.WORD),
                ("scan", w.WORD), ("character", w.WCHAR), ("control", w.DWORD)]

class InputRecord(ctypes.Structure):
    _fields_ = [("event_type", w.WORD), ("event", KeyEvent)]

records = (InputRecord * 2)(InputRecord(1, KeyEvent(1, 1, 82, 0, "R", 0)),
                            InputRecord(1, KeyEvent(1, 1, 13, 0, "\r", 0)))
kernel.WriteConsoleInputW.argtypes = [w.HANDLE, ctypes.POINTER(InputRecord), w.DWORD,
                                     ctypes.POINTER(w.DWORD)]
written = w.DWORD()
assert kernel.WriteConsoleInputW(handle, records, 2, ctypes.byref(written)), ctypes.get_last_error()
assert input() == "R"
