"""Keep an unrelated same-user controller reachable while test descendants are denied."""

import errno
import json
import os
import socket
import subprocess
import sys
import time
from contextlib import ExitStack
from pathlib import Path

ADDRESSES = [(socket.AF_INET, "192.0.2.2"), (socket.AF_INET6, "2001:db8::2")]


def child(directory: Path, ports: list[int], expected: int) -> int:
    denied = []
    for (family, address), port in zip(ADDRESSES, ports, strict=True):
        with socket.socket(family) as connection:
            connection.settimeout(2)
            result = connection.connect_ex((address, port))
            assert result in (errno.EPERM, errno.EACCES, errno.ECONNREFUSED), result
            denied.append({"address": address, "errno": result})
    (directory / "ready.json").write_text(json.dumps({"uid": os.getuid(), "denied": denied}))
    deadline = time.monotonic() + 30
    while not (directory / "release").exists():
        if time.monotonic() >= deadline:
            raise RuntimeError("Controller did not release the owned test child")
        time.sleep(0.02)
    return expected


def check(directory: Path, expected: int, guard: Path) -> dict:
    directory.mkdir(parents=True)
    observations = []
    with ExitStack() as stack:
        listeners = []
        for family, address in ADDRESSES:
            listener = stack.enter_context(socket.socket(family))
            listener.settimeout(2)
            listener.bind((address, 0))
            listener.listen()
            listeners.append(listener)

        def controller_exchange(phase: str) -> None:
            for listener in listeners:
                with socket.create_connection(listener.getsockname()[:2], timeout=2) as client:
                    peer, _ = listener.accept()
                    with peer:
                        client.sendall(phase.encode())
                        observed = peer.recv(64).decode()
                        assert observed == phase
                        observations.append({"address": listener.getsockname()[0], "phase": phase})

        controller_exchange("before")
        ports = [listener.getsockname()[1] for listener in listeners]
        with (directory / "command.log").open("w") as log:
            process = subprocess.Popen(
                [
                    sys.executable,
                    str(guard),
                    "--evidence",
                    str(directory / "egress.json"),
                    "--",
                    sys.executable,
                    str(Path(__file__).resolve()),
                    "--child",
                    str(directory),
                    json.dumps(ports),
                    str(expected),
                ],
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            try:
                deadline = time.monotonic() + 30
                while not (directory / "ready.json").exists():
                    if process.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError(f"Test child never became ready: {process.poll()}")
                    time.sleep(0.02)
                ready = json.loads((directory / "ready.json").read_text())
                assert ready["uid"] == os.getuid()
                controller_exchange("during")
            finally:
                (directory / "release").write_text("release")
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            assert process.returncode == expected, process.returncode
        controller_exchange("after")
    report = {"uid": os.getuid(), "exit_status": expected, "controller": observations}
    (directory / "controller.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main() -> int:
    if sys.argv[1] == "--child":
        return child(Path(sys.argv[2]), json.loads(sys.argv[3]), int(sys.argv[4]))
    installed = []
    try:
        for family, address in ADDRESSES:
            selector = "-4" if family == socket.AF_INET else "-6"
            prefix = address + ("/32" if family == socket.AF_INET else "/128")
            subprocess.run(
                ["sudo", "-n", "ip", selector, "address", "add", prefix, "dev", "lo"],
                check=True,
            )
            installed.append((selector, prefix))
        guard = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else Path("/egress.py")
        reports = [check(Path(sys.argv[1]) / f"exit-{code}", code, guard) for code in (0, 17)]
        print(json.dumps(reports, indent=2))
        return 0
    finally:
        for selector, prefix in reversed(installed):
            subprocess.run(
                ["sudo", "-n", "ip", selector, "address", "del", prefix, "dev", "lo"],
                check=True,
            )


if __name__ == "__main__":
    raise SystemExit(main())
