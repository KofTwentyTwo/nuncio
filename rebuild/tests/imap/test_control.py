import asyncio
import base64
import tempfile
import unittest
from pathlib import Path

from control import MailControl
from proxy import MailProxy
from services import MailServices, run_directory


class IndependentControls(unittest.IsolatedAsyncioTestCase):
    async def test_seed_external_flag_and_uidvalidity_controls_are_real_server_changes(self):
        runs = run_directory()
        service = MailServices(Path(tempfile.mkdtemp(prefix="control-", dir=runs)) / "services")
        proxy = None
        try:
            await asyncio.to_thread(service.start)
            proxy = MailProxy(service)
            await proxy.start()
            control = MailControl(service, proxy)
            await control.handle({"command": "seed"})
            first = await control.handle(
                {"command": "mailbox", "account": "alpha@example.test", "mailbox": "INBOX"}
            )
            second = await control.handle(
                {"command": "mailbox", "account": "beta@example.test", "mailbox": "INBOX"}
            )
            self.assertEqual([m["uid"] for m in first["messages"]], [1, 2])
            self.assertEqual([m["uid"] for m in second["messages"]], [1, 2])
            self.assertEqual(first["uidvalidity"], 9001)
            self.assertNotEqual(first["messages"][0]["sha256"], second["messages"][0]["sha256"])
            raw = await control.handle(
                {"command": "raw", "account": "alpha@example.test", "mailbox": "INBOX", "uid": 1}
            )
            self.assertIn(b"Content-Type: application/pdf", base64.b64decode(raw["raw_base64"]))
            await control.handle(
                {
                    "command": "flags",
                    "account": "alpha@example.test",
                    "mailbox": "INBOX",
                    "uid": 1,
                    "add": ["\\Seen", "\\Flagged"],
                    "remove": [],
                }
            )
            changed = await control.handle(
                {"command": "mailbox", "account": "alpha@example.test", "mailbox": "INBOX"}
            )
            self.assertEqual(set(changed["messages"][0]["flags"]), {"\\Seen", "\\Flagged"})
            await control.handle(
                {
                    "command": "uidvalidity",
                    "account": "alpha@example.test",
                    "mailbox": "INBOX",
                    "value": 9101,
                }
            )
            reset = await control.handle(
                {"command": "mailbox", "account": "alpha@example.test", "mailbox": "INBOX"}
            )
            self.assertEqual(reset["uidvalidity"], 9101)
            self.assertEqual(reset["messages"][0]["sha256"], changed["messages"][0]["sha256"])
            target = 'Archive "external" & 日本語'
            renamed = "Renamed 日本語"
            await control.handle(
                {"command": "folder_create", "account": "alpha@example.test", "mailbox": target}
            )
            copy = await control.handle(
                {
                    "command": "copy",
                    "account": "alpha@example.test",
                    "mailbox": "INBOX",
                    "uid": 1,
                    "destination": target,
                }
            )
            self.assertEqual(len(copy["messages"]), 1)
            self.assertEqual(copy["messages"][0]["sha256"], first["messages"][0]["sha256"])
            await control.handle(
                {
                    "command": "folder_rename",
                    "account": "alpha@example.test",
                    "mailbox": target,
                    "destination": renamed,
                }
            )
            copied = await control.handle(
                {"command": "mailbox", "account": "alpha@example.test", "mailbox": renamed}
            )
            self.assertEqual(copied["messages"], copy["messages"])
            await control.handle(
                {"command": "folder_delete", "account": "alpha@example.test", "mailbox": renamed}
            )
            with self.assertRaises(ValueError):
                await control.handle(
                    {"command": "mailbox", "account": "alpha@example.test", "mailbox": renamed}
                )
            await control.handle(
                {
                    "command": "flags",
                    "account": "alpha@example.test",
                    "mailbox": "INBOX",
                    "uid": 2,
                    "add": ["\\Deleted"],
                    "remove": [],
                }
            )
            removed = await control.handle(
                {
                    "command": "expunge",
                    "account": "alpha@example.test",
                    "mailbox": "INBOX",
                    "uid": 1,
                }
            )
            self.assertEqual([v["uid"] for v in removed["messages"]], [2])
            self.assertEqual(removed["messages"][0]["flags"], ["\\Deleted"])
            empty = await control.handle(
                {"command": "mailbox", "account": "beta@example.test", "mailbox": "Sent"}
            )
            self.assertEqual(empty["messages"], [])
            with self.assertRaises(ValueError):
                await control.handle(
                    {
                        "command": "uidvalidity",
                        "account": "not-configured@example.test",
                        "mailbox": "INBOX",
                        "value": 9102,
                    }
                )
            with self.assertRaises(ValueError):
                await control.handle(
                    {
                        "command": "mailbox",
                        "account": "alpha@example.test",
                        "mailbox": "INBOX",
                        "unknown": True,
                    }
                )
            await control.handle({"command": "seed"})
            seeded = await control.handle(
                {"command": "mailbox", "account": "alpha@example.test", "mailbox": "INBOX"}
            )
            self.assertEqual(seeded, first)
            self.assertEqual((await control.handle({"command": "smtp"}))["total"], 0)
        finally:
            if proxy:
                await proxy.stop()
            await asyncio.to_thread(service.stop)


if __name__ == "__main__":
    unittest.main()
