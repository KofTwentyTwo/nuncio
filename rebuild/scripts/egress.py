#!/usr/bin/env python3
"""Run offline tests with external TCP egress denied and independently probed."""

import argparse
import errno
import json
import os
import platform
import signal
import socket
import subprocess
import sys
from pathlib import Path

MAC_POLICY = (
    "(version 1) (allow default) (deny network-outbound) "
    '(allow network-outbound (remote ip "localhost:*") (remote unix-socket))'
)


def probe() -> dict:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        with socket.create_connection(listener.getsockname(), timeout=2):
            peer, _ = listener.accept()
            peer.close()
    blocked = []
    for family, address in [
        (socket.AF_INET, ("192.0.2.1", 443)),
        (socket.AF_INET6, ("2001:db8::1", 443)),
    ]:
        with socket.socket(family) as connection:
            connection.settimeout(2)
            result = connection.connect_ex(address)
            allowed_errors = (
                (errno.EPERM, errno.EACCES)
                if platform.system() == "Darwin"
                else (
                    errno.EPERM,
                    errno.EACCES,
                    errno.EHOSTUNREACH,
                    errno.ENETUNREACH,
                    errno.ECONNREFUSED,
                )
            )
            if result not in allowed_errors:
                raise RuntimeError(
                    f"External denial was not established: {address[0]}, errno={result}"
                )
            blocked.append({"address": address[0], "errno": result})
    return {"loopback": "allowed", "blocked": blocked}


def run_inside(command: list[str], evidence: Path) -> int:
    observed = probe()
    child = subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), "--probe"],
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    if child.returncode:
        raise RuntimeError("Child process did not inherit egress denial")
    observed["child"] = json.loads(child.stdout)
    observed["platform"] = platform.system()
    observed["command"] = command
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text(json.dumps(observed, indent=2) + "\n")
    result = subprocess.run(command, env=dict(os.environ, CARGO_NET_OFFLINE="true"), check=False)
    observed["exit_status"] = result.returncode
    evidence.write_text(json.dumps(observed, indent=2) + "\n")
    return result.returncode


def linux_ci(command: list[str], evidence: Path) -> int:
    # Only disposable hosted runners may change host packet filters. Local Linux
    # machines need their own explicitly provisioned isolation environment.
    if (
        os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
    ):
        raise RuntimeError("Linux egress setup requires a disposable GitHub-hosted runner")
    if os.getuid() == 0:
        raise RuntimeError("Run as the unprivileged hosted-runner user")
    chain = f"NUNCIO-{os.getpid()}"
    installed = []
    probe_routes = []
    observed = {}

    def firewall(program: str, *args: str) -> str:
        return subprocess.run(
            ["sudo", "-n", program, "-w", "10", *args],
            check=True,
            text=True,
            capture_output=True,
            timeout=20,
        ).stdout

    def interrupted(signum, _frame):
        raise InterruptedError(f"Interrupted by signal {signum}")

    previous = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        # Dedicated TEST-NET routes ensure even an IPv6-disconnected runner sends
        # probes through OUTPUT. They cannot contact a real remote endpoint.
        for family, destination in [("-4", "192.0.2.1/32"), ("-6", "2001:db8::1/128")]:
            subprocess.run(
                ["sudo", "-n", "ip", family, "route", "add", destination, "dev", "lo"],
                check=True,
                capture_output=True,
                timeout=10,
            )
            probe_routes.append((family, destination))
        for program, loopback in [("iptables", "127.0.0.0/8"), ("ip6tables", "::1/128")]:
            firewall(program, "-N", chain)
            installed.append((program, False))
            firewall(program, "-A", chain, "-d", loopback, "-j", "RETURN")
            # Docker's published localhost ports are DNATed before OUTPUT filtering.
            firewall(
                program, "-A", chain, "-m", "conntrack", "--ctorigdst", loopback, "-j", "RETURN"
            )
            firewall(
                program, "-A", chain, "-p", "tcp", "-j", "REJECT", "--reject-with", "tcp-reset"
            )
            firewall(program, "-A", chain, "-j", "REJECT")
            firewall(
                program,
                "-I",
                "OUTPUT",
                "1",
                "-m",
                "owner",
                "--uid-owner",
                str(os.getuid()),
                "-j",
                chain,
            )
            installed[-1] = (program, True)
        result = run_inside(command, evidence)
        for program, _ in installed:
            rules = firewall(program, "-L", chain, "-n", "-v", "-x")
            packets = sum(
                int(line.split()[0])
                for line in rules.splitlines()
                if len(line.split()) > 2 and line.split()[2] == "REJECT"
            )
            if packets < 2:
                raise RuntimeError(f"{program} did not independently count both external probes")
            observed[program] = {"rejected_packets": packets, "rules": rules}
        data = json.loads(evidence.read_text())
        data["firewall"] = observed
        evidence.write_text(json.dumps(data, indent=2) + "\n")
        return result
    finally:
        failures = []
        for program, attached in reversed(installed):
            try:
                if attached:
                    firewall(
                        program,
                        "-D",
                        "OUTPUT",
                        "-m",
                        "owner",
                        "--uid-owner",
                        str(os.getuid()),
                        "-j",
                        chain,
                    )
                firewall(program, "-F", chain)
                firewall(program, "-X", chain)
            except (OSError, subprocess.SubprocessError) as error:
                failures.append(f"{program}: {type(error).__name__}")
        for family, destination in reversed(probe_routes):
            try:
                subprocess.run(
                    ["sudo", "-n", "ip", family, "route", "del", destination, "dev", "lo"],
                    check=True,
                    capture_output=True,
                    timeout=10,
                )
            except (OSError, subprocess.SubprocessError) as error:
                failures.append(f"probe route {destination}: {type(error).__name__}")
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        if failures:
            raise RuntimeError("Owned firewall cleanup failed: " + ", ".join(failures))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, default=Path("test-results/egress.json"))
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--probe", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.probe:
        print(json.dumps(probe()))
        return 0
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("provide a command after --")
    if args.inside:
        return run_inside(command, args.evidence)
    if platform.system() == "Darwin":
        return subprocess.run(
            [
                "/usr/bin/sandbox-exec",
                "-p",
                MAC_POLICY,
                sys.executable,
                str(Path(__file__).resolve()),
                "--inside",
                "--evidence",
                str(args.evidence),
                "--",
                *command,
            ],
            check=False,
        ).returncode
    if platform.system() == "Linux":
        return linux_ci(command, args.evidence)
    raise RuntimeError("No verified egress enforcement for this platform")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
        print(f"Egress isolation failed: {error}", file=sys.stderr)
        sys.exit(2)
