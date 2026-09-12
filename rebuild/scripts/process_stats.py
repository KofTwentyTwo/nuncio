"""Read only numeric RSS/thread counts for a test-owned process."""

import argparse
import ctypes
import json
import os
import platform
from pathlib import Path


class TaskInfo(ctypes.Structure):
    # macOS SDK sys/proc_info.h, struct proc_taskinfo / PROC_PIDTASKINFO.
    _fields_ = [
        (name, ctypes.c_uint64)
        for name in ("virtual", "resident", "user", "system", "threads_user", "threads_system")
    ] + [
        (name, ctypes.c_int32)
        for name in (
            "policy",
            "faults",
            "pageins",
            "cow",
            "sent",
            "received",
            "mach",
            "unix",
            "switches",
            "threads",
            "running",
            "priority",
        )
    ]


def process_stats(pid: int) -> dict[str, int]:
    if pid <= 0:
        raise ValueError("A positive, individual process ID is required")
    if platform.system() == "Darwin":
        # The setuid /bin/ps cannot start in Seatbelt. Native libproc observation
        # needs no privilege elevation and retains the test's egress sandbox.
        library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        library.proc_pidinfo.argtypes = [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_uint64,
            ctypes.c_void_p,
            ctypes.c_int,
        ]
        library.proc_pidinfo.restype = ctypes.c_int
        info = TaskInfo()
        size = ctypes.sizeof(info)
        if size != 96:
            raise RuntimeError("Unexpected macOS proc_taskinfo ABI size")
        if library.proc_pidinfo(pid, 4, 0, ctypes.byref(info), size) != size:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))
        return {"rss_kib": info.resident // 1024, "threads": info.threads}
    if platform.system() == "Linux":
        fields = dict(
            line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines()
        )
        rss, unit = fields["VmRSS"].split()
        if unit != "kB":
            raise RuntimeError("Unexpected procfs RSS unit")
        return {"rss_kib": int(rss), "threads": int(fields["Threads"])}
    raise RuntimeError("Process sampling supports macOS and Linux")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    print(json.dumps(process_stats(parser.parse_args().pid)))
