"""Inside the disposable container, verify success, failure and owned-rule cleanup."""

import json
import subprocess
import sys
from pathlib import Path

evidence = Path(sys.argv[1])
for expected in (0, 17):
    path = evidence / f"linux-exit-{expected}.json"
    result = subprocess.run(
        [
            "python3",
            "/egress.py",
            "--evidence",
            str(path),
            "--",
            "python3",
            "-c",
            f"raise SystemExit({expected})",
        ],
        check=False,
    )
    assert result.returncode == expected, (result.returncode, expected)
    report = json.loads(path.read_text())
    assert report["exit_status"] == expected
    for program in ("iptables", "ip6tables"):
        assert report["firewall"][program]["rejected_packets"] >= 2
        rules = subprocess.check_output(["sudo", "-n", program, "-S"], text=True)
        assert "NUNCIO-" not in rules
    for family, address in [("-4", "192.0.2.1"), ("-6", "2001:db8::1")]:
        routes = subprocess.check_output(["sudo", "-n", "ip", family, "route", "show"], text=True)
        assert address not in routes
print("Linux parent/child denial, success/failure propagation and cleanup passed")
