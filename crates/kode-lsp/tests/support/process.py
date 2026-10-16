"""Probe only: exit 0 while running, 1 after exit, 2 if observation fails."""
import os
import sys
import traceback

try:
    pid = int(sys.argv[1])
    if os.name == 'nt':
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        # SYNCHRONIZE permits a zero-timeout wait without reading process memory.
        handle = kernel.OpenProcess(0x00100000, False, pid)
        if not handle:
            error = ctypes.get_last_error()
            if error != 87:  # ERROR_INVALID_PARAMETER: no process with this PID.
                raise ctypes.WinError(error)
            running = False
        else:
            try:
                state = kernel.WaitForSingleObject(handle, 0)
                if state not in (0, 258):  # WAIT_OBJECT_0, WAIT_TIMEOUT.
                    raise ctypes.WinError(ctypes.get_last_error())
                running = state == 258
            finally:
                if not kernel.CloseHandle(handle):
                    raise ctypes.WinError(ctypes.get_last_error())
    else:
        try:
            os.kill(pid, 0)
            running = True
        except ProcessLookupError:
            running = False
except Exception:
    traceback.print_exc()
    sys.exit(2)
sys.exit(0 if running else 1)
