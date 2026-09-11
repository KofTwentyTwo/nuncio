"""Optional server-managed Sent behavior, driven by independent SMTP captures.

This is a test-provider policy, not a claim about any live MailPlus version.
Mailpit authenticates and accepts SMTP; Dovecot owns every resulting Sent copy.
"""

import asyncio
import json
import os
import re
import traceback
from pathlib import Path


class AutoSent:
    def __init__(self, control):
        self.control = control
        self.completed = set()
        self.failed = False
        self.updated = asyncio.Event()
        self.stopping = asyncio.Event()
        self.task = None

    async def start(self):
        self.task = asyncio.create_task(self.run())

    async def stop(self):
        self.stopping.set()
        if self.task is not None:
            await self.task

    def reset(self):
        self.completed.clear()
        (self.control.services.directory / "auto-sent-error.json").unlink(missing_ok=True)
        if self.failed:
            self.failed = False
            self.task = asyncio.create_task(self.run())
        self.updated.set()

    def status(self):
        return {"enabled": True, "completed": len(self.completed), "failed": self.failed}

    async def wait(self, count):
        if type(count) is not int or not 0 <= count <= 10000:
            raise ValueError("invalid server Sent count")
        async with asyncio.timeout(15):
            while len(self.completed) < count:
                self.updated.clear()
                if self.failed:
                    raise RuntimeError("independent server Sent worker failed")
                await self.updated.wait()
        return self.status()

    async def run(self):
        try:
            while not self.stopping.is_set():
                async with self.control.maintenance:
                    await self.scan()
                try:
                    await asyncio.wait_for(self.stopping.wait(), 0.1)
                except TimeoutError:
                    pass
        except Exception as error:
            # An unknown APPEND result must never lead to a second blind copy.
            self.failed = True
            # Exception messages and source lines may contain protocol content.
            # Retain only bounded source locations and the exception type.
            frames = traceback.extract_tb(error.__traceback__)[-8:]
            diagnostic = {
                "type": type(error).__name__,
                "frames": [
                    {
                        "file": Path(frame.filename).name,
                        "line": frame.lineno,
                        "function": frame.name,
                    }
                    for frame in frames
                ],
            }
            path = self.control.services.directory / "auto-sent-error.json"
            with os.fdopen(
                os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w"
            ) as output:
                json.dump(diagnostic, output)
            self.updated.set()

    async def scan(self):
        for tls in (False, True):
            offset = 0
            while not self.stopping.is_set():
                page = await asyncio.to_thread(
                    self.control.services.api, f"/api/v1/messages?start={offset}&limit=100", tls=tls
                )
                if type(page.get("total")) is not int or not 0 <= page["total"] <= 10000:
                    raise ValueError("server Sent fixture limit exceeded")
                messages = page.get("messages")
                if not isinstance(messages, list) or len(messages) > 100:
                    raise ValueError("invalid independent capture page")
                for message in messages:
                    if self.stopping.is_set():
                        return
                    identity = message.get("ID")
                    if not isinstance(identity, str) or not re.fullmatch(
                        r"[A-Za-z0-9_-]{1,128}", identity
                    ):
                        raise ValueError("invalid independent capture identity")
                    key = (tls, identity)
                    if key in self.completed:
                        continue
                    # Mailpit persists Username with the message; tags are
                    # applied later and may also originate from message headers.
                    account = message.get("Username")
                    if (
                        not isinstance(account, str)
                        or account not in self.control.services.passwords
                        or len(self.completed) >= 10000
                    ):
                        raise ValueError("ambiguous authenticated capture owner or fixture limit")
                    await asyncio.to_thread(self.save, tls, identity, account)
                    self.completed.add(key)
                    self.updated.set()
                offset += len(messages)
                if not messages or offset >= page["total"]:
                    break

    def save(self, tls, identity, account):
        raw = self.control.services.api("/api/v1/message/" + identity + "/raw", raw=True, tls=tls)
        with self.control.connection(account) as mail:
            if mail.append("Sent", "(\\Seen)", None, raw)[0] != "OK":
                raise RuntimeError("independent server Sent APPEND rejected")
