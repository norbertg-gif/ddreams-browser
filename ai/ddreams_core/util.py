from __future__ import annotations

import ctypes
import os
import sys

from .constants import TEXT_PROB_FLOOR, TEXT_PROB_RELATIVE


def text_probability_cutoff(top_prob: float) -> float:
    return max(TEXT_PROB_FLOOR, TEXT_PROB_RELATIVE * top_prob)


class RequestError(ValueError):
    pass


def _stderr(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _set_background_priority() -> None:
    if os.name != "nt":
        return
    try:
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.SetPriorityClass(kernel.GetCurrentProcess(), 0x00004000)  # BELOW_NORMAL_PRIORITY_CLASS
        kernel.SetThreadPriority(kernel.GetCurrentThread(), 0x00010000)  # THREAD_MODE_BACKGROUND_BEGIN
    except Exception:
        pass


def process_memory() -> dict[str, int] | None:
    """Windows RSS/private commit; avoids adding psutil to the runtime."""
    if os.name != "nt":
        return None
    class Counters(ctypes.Structure):
        _fields_ = [("cb", ctypes.c_ulong), ("PageFaultCount", ctypes.c_ulong),
                    ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                    ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t), ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                    ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t),
                    ("PrivateUsage", ctypes.c_size_t)]
    try:
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        psapi.GetProcessMemoryInfo.argtypes = (ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong)
        psapi.GetProcessMemoryInfo.restype = ctypes.c_int
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(kernel.GetCurrentProcess(), ctypes.byref(counters), counters.cb):
            return None
        return {"working_set": int(counters.WorkingSetSize), "private": int(counters.PrivateUsage)}
    except Exception:
        return None
