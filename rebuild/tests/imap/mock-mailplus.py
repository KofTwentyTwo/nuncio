#!/usr/bin/env python3
"""Standalone independent mail mock, controlled only through inherited pipes."""

import argparse
import asyncio
import json
import re
import signal
import sys
from pathlib import Path

from auto_sent import AutoSent
from control import MailControl
from proxy import MailProxy
from services import MailServices

MAX_CONTROL_BYTES = 96 * 1024 * 1024


def strict_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate field")
        result[key] = value
    return result


def invalid_constant(_):
    raise ValueError("nonfinite number")


async def controls(control, reader, writer, stopping):
    async def reply(value):
        writer.write(json.dumps(value, separators=(",", ":"), allow_nan=False).encode() + b"\n")
        await asyncio.wait_for(writer.drain(), 30)

    await reply({"ready_file": str(control.services.directory / "mock-ready.json")})
    while not stopping.is_set():
        reading = asyncio.create_task(reader.readline())
        stopping_read = asyncio.create_task(stopping.wait())
        try:
            await asyncio.wait([reading, stopping_read], return_when=asyncio.FIRST_COMPLETED)
            if stopping_read.done():
                return 0
            line = reading.result()
        except ValueError:
            await reply({"id": None, "ok": False, "error": "control_too_large"})
            return 1
        finally:
            reading.cancel()
            stopping_read.cancel()
            await asyncio.gather(reading, stopping_read, return_exceptions=True)
        if not line:
            return 0
        identity = None
        try:
            if not line.endswith(b"\n"):
                raise ValueError("incomplete control")
            request = json.loads(
                line, object_pairs_hook=strict_object, parse_constant=invalid_constant
            )
            if not isinstance(request, dict) or set(request) != {"id", "control"}:
                raise ValueError("invalid request fields")
            if not isinstance(request["id"], str) or not re.fullmatch(
                r"[a-zA-Z0-9_-]{1,64}", request["id"]
            ):
                raise ValueError("invalid request identity")
            identity = request["id"]
            if request["control"] == {"command": "shutdown"}:
                await reply({"id": identity, "ok": True, "result": {"stopping": True}})
                return 0
            result = await control.handle(request["control"])
            await reply({"id": identity, "ok": True, "result": result})
        except (ValueError, TypeError, KeyError, UnicodeError, RecursionError):
            await reply({"id": identity, "ok": False, "error": "invalid_control"})
        except TimeoutError:
            await reply({"id": identity, "ok": False, "error": "control_timeout"})
        except Exception:
            # Protocol exceptions can contain AUTH data or submitted literals.
            # Report a fixed category; provider state is separately observable.
            await reply({"id": identity, "ok": False, "error": "backend_failed"})
    return 0


async def run(args):
    loop = asyncio.get_running_loop()
    stopping = asyncio.Event()
    failures = []

    def unexpected_error(_loop, _context):
        failures.append(True)
        stopping.set()

    loop.set_exception_handler(unexpected_error)
    for number in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(number, stopping.set)
    services = MailServices(args.directory)
    proxy = None
    auto = None
    input_transport = output_transport = None
    try:
        await asyncio.to_thread(services.start)
        proxy = MailProxy(services, hidden_capabilities=args.hide_capability)
        control = MailControl(services, proxy)
        await asyncio.to_thread(control.seed)
        await proxy.start()
        if args.server_auto_sent:
            auto = AutoSent(control)
            control.auto_sent = auto
            await auto.start()
        ready = {
            "schema_version": 1,
            "project": services.project,
            "ports": proxy.ports,
            "ca_file": str(services.directory / "ca.crt"),
            "credentials_file": str(services.directory / "credentials.json"),
            "hidden_capabilities": sorted(proxy.hidden),
            "seed": "two_accounts",
            "server_auto_sent": args.server_auto_sent,
        }
        temporary = services.directory / "mock-ready.tmp"
        temporary.write_text(json.dumps(ready, indent=2))
        temporary.chmod(0o600)
        temporary.replace(services.directory / "mock-ready.json")
        reader = asyncio.StreamReader(limit=MAX_CONTROL_BYTES)
        input_transport, _ = await loop.connect_read_pipe(
            lambda: asyncio.StreamReaderProtocol(reader), sys.stdin.buffer
        )
        output_transport, protocol = await loop.connect_write_pipe(
            lambda: asyncio.StreamReaderProtocol(asyncio.StreamReader()), sys.stdout.buffer
        )
        writer = asyncio.StreamWriter(output_transport, protocol, None, loop)
        # Finish an in-flight Docker reset before cleanup. Cancelling to_thread
        # would leave its worker able to launch containers after stop returned.
        result = await controls(control, reader, writer, stopping)
        return 1 if failures else result
    finally:
        if auto is not None:
            await auto.stop()
        if proxy is not None:
            await proxy.stop()
        await asyncio.to_thread(services.stop)
        for transport in (input_transport, output_transport):
            if transport is not None:
                transport.close()
        if auto is not None and auto.failed:
            raise RuntimeError("independent server Sent worker failed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--server-auto-sent", action="store_true")
    parser.add_argument(
        "--hide-capability",
        action="append",
        default=[],
        choices=["MOVE", "UIDPLUS", "CONDSTORE", "QRESYNC"],
    )
    args = parser.parse_args()
    try:
        return asyncio.run(run(args))
    except Exception:
        # Retained startup diagnostics use local files. Never echo credentials
        # embedded in a third-party protocol exception to shared stderr.
        print(
            "Independent mail mock failed; inspect its private runtime diagnostics.",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    sys.exit(main())
