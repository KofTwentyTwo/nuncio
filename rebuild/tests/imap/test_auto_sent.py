import asyncio
import base64
import json
import smtplib
import ssl
import tempfile
import unittest
from pathlib import Path

from auto_sent import AutoSent
from control import MailControl
from proxy import Fault, MailProxy
from services import MailServices, run_directory


class IndependentServerSent(unittest.IsolatedAsyncioTestCase):
    async def test_server_saved_sent_is_separate_from_smtp_ack_and_client_append(self):
        services = MailServices(
            Path(tempfile.mkdtemp(prefix="auto-sent-", dir=run_directory())) / "services"
        )
        proxy = auto = None
        try:
            await asyncio.to_thread(services.start)
            proxy = MailProxy(services)
            await proxy.start()
            control = MailControl(services, proxy)
            await control.handle({"command": "seed"})
            auto = AutoSent(control)
            control.auto_sent = auto
            await auto.start()
            tls = ssl.create_default_context(cafile=str(services.directory / "ca.crt"))
            raw = b"X-Tags: alpha@example.test, beta@example.test\r\nFrom: alias@example.test\r\nTo: public@example.test\r\nMessage-ID: <same-id@example.test>\r\nSubject: Server saved copy\r\n\r\nExact wire payload\r\n"
            held = Fault("smtp", "DATA", "after", "withhold")
            proxy.inject(held)

            def submit(account, implicit=False):
                smtp = (
                    smtplib.SMTP_SSL("127.0.0.1", proxy.ports["smtps"], context=tls, timeout=10)
                    if implicit
                    else smtplib.SMTP("127.0.0.1", proxy.ports["smtp"], timeout=10)
                )
                try:
                    if not implicit:
                        smtp.starttls(context=tls)
                    smtp.login(account, services.passwords[account])
                    self.assertEqual(
                        smtp.sendmail(account, ["public@example.test", "hidden@example.test"], raw),
                        {},
                    )
                finally:
                    smtp.close()

            sending = asyncio.create_task(asyncio.to_thread(submit, "alpha@example.test"))
            try:
                await asyncio.wait_for(held.entered.wait(), 5)
                await control.handle({"command": "wait_server_sent", "count": 1})
                self.assertFalse(sending.done())
                observed = await control.handle(
                    {"command": "mailbox", "account": "alpha@example.test", "mailbox": "Sent"}
                )
                self.assertEqual([v["uid"] for v in observed["messages"]], [1])
                sent = await control.handle(
                    {"command": "raw", "account": "alpha@example.test", "mailbox": "Sent", "uid": 1}
                )
                copied = base64.b64decode(sent["raw_base64"])
                capture = await control.handle({"command": "smtp", "transport": "starttls"})
                captured = await control.handle(
                    {
                        "command": "smtp_raw",
                        "transport": "starttls",
                        "id": capture["messages"][0]["ID"],
                    }
                )
                self.assertEqual(sent["raw_base64"], captured["raw_base64"])
                self.assertEqual(copied[-len(raw) :], raw)
                self.assertNotIn("imap APPEND", proxy.snapshot()["requests"])
            finally:
                held.released.set()
                await sending
            # A duplicate Message-ID is a second accepted delivery and a second
            # server copy. Neither provider observer deduplicates by that header.
            await asyncio.to_thread(submit, "alpha@example.test")
            await asyncio.to_thread(submit, "beta@example.test", True)
            await control.handle({"command": "wait_server_sent", "count": 3})
            for account, expected in [("alpha@example.test", 2), ("beta@example.test", 1)]:
                observed = await control.handle(
                    {"command": "mailbox", "account": account, "mailbox": "Sent"}
                )
                self.assertEqual(len(observed["messages"]), expected)
            self.assertEqual(
                (await control.handle({"command": "server_sent_status"}))["completed"], 3
            )
            await control.handle({"command": "seed"})
            self.assertEqual(
                (await control.handle({"command": "server_sent_status"}))["completed"], 0
            )
            for account in services.passwords:
                self.assertEqual(
                    (
                        await control.handle(
                            {"command": "mailbox", "account": account, "mailbox": "Sent"}
                        )
                    )["messages"],
                    [],
                )
            await asyncio.to_thread(services.compose, "pause", "dovecot")
            try:
                await asyncio.to_thread(submit, "alpha@example.test")
                with self.assertRaises(RuntimeError):
                    await control.handle({"command": "wait_server_sent", "count": 1})
                self.assertTrue((await control.handle({"command": "server_sent_status"}))["failed"])
                diagnostic = json.loads((services.directory / "auto-sent-error.json").read_text())
                self.assertEqual(set(diagnostic), {"type", "frames"})
                self.assertTrue(diagnostic["frames"])
                self.assertFalse(
                    any(
                        password in json.dumps(diagnostic)
                        for password in services.passwords.values()
                    )
                )
                self.assertEqual((await control.handle({"command": "smtp"}))["total"], 1)
            finally:
                await asyncio.to_thread(services.compose, "unpause", "dovecot")
            self.assertEqual(
                (
                    await control.handle(
                        {"command": "mailbox", "account": "alpha@example.test", "mailbox": "Sent"}
                    )
                )["messages"],
                [],
            )
            await control.handle({"command": "seed"})
            self.assertFalse((await control.handle({"command": "server_sent_status"}))["failed"])
            await asyncio.to_thread(submit, "beta@example.test", True)
            await control.handle({"command": "wait_server_sent", "count": 1})
        finally:
            if auto is not None:
                await auto.stop()
            if proxy is not None:
                await proxy.stop()
            await asyncio.to_thread(services.stop)


if __name__ == "__main__":
    unittest.main()
