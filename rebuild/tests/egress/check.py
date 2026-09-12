"""Inside the disposable container, verify success, failure and owned-rule cleanup."""

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path


def assert_cleanup() -> None:
    assert not list(Path("/sys/fs/cgroup").glob("NUNCIO-*"))
    for program in ("iptables", "ip6tables"):
        rules = subprocess.check_output(["sudo", "-n", program, "-S"], text=True)
        assert "NUNCIO-" not in rules
    for family, address in [("-4", "192.0.2.1"), ("-6", "2001:db8::1")]:
        routes = subprocess.check_output(["sudo", "-n", "ip", family, "route", "show"], text=True)
        assert address not in routes


def lingering_child(directory: Path, interrupted: bool) -> int:
    # A detached descendant must be stopped before its network filter is removed.
    with (directory / "child.log").open("w") as log:
        child = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            stdout=log,
            stderr=log,
            start_new_session=True,
        )
    (directory / "child.pid").write_text(str(child.pid))
    if interrupted:
        time.sleep(60)
    return 0


def main() -> int:
    if sys.argv[1] == "--child":
        return lingering_child(Path(sys.argv[2]), sys.argv[3] == "interrupt")
    evidence = Path(sys.argv[1])
    for expected in (0, 17):
        path = evidence / f"linux-exit-{expected}.json"
        result = subprocess.run(
            [
                sys.executable,
                "/egress.py",
                "--evidence",
                str(path),
                "--",
                sys.executable,
                "-c",
                f"raise SystemExit({expected})",
            ],
            check=False,
            timeout=30,
        )
        assert result.returncode == expected, (result.returncode, expected)
        report = json.loads(path.read_text())
        assert report["exit_status"] == expected
        assert report["isolation"]["kind"] == "cgroup-v2"
        assert report["isolation"]["uid"] == os.getuid()
        for program in ("iptables", "ip6tables"):
            assert report["firewall"][program]["rejected_packets"] >= 2
        assert_cleanup()
    for mode in ("return", "interrupt"):
        directory = evidence / mode
        directory.mkdir()
        with (directory / "command.log").open("w") as log:
            process = subprocess.Popen(
                [
                    sys.executable,
                    "/egress.py",
                    "--evidence",
                    str(directory / "egress.json"),
                    "--",
                    sys.executable,
                    str(Path(__file__).resolve()),
                    "--child",
                    str(directory),
                    mode,
                ],
                stdout=log,
                stderr=log,
            )
            deadline = time.monotonic() + 20
            while not (directory / "child.pid").exists():
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("Detached test child did not become ready")
                time.sleep(0.02)
            if mode == "interrupt":
                process.send_signal(signal.SIGTERM)
            result = process.wait(timeout=20)
        assert result == (2 if mode == "interrupt" else 0), result
        pid = int((directory / "child.pid").read_text())
        stat = Path(f"/proc/{pid}/stat")
        assert not stat.exists() or stat.read_text().split(") ", 1)[1].startswith("Z ")
        assert_cleanup()
        (directory / "cleanup.json").write_text(
            json.dumps(
                {"exit_status": result, "descendant_stopped": True, "owned_resources_removed": True}
            )
        )
    print("Linux denial, exit propagation, controller-safe scope and detached-child cleanup passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
