import asyncio
import hashlib
import imaplib
import smtplib
import socket
import ssl
import tempfile
import unittest
from pathlib import Path

from proxy import Fault, MailProxy
from services import MailServices, run_directory


class IndependentFaultProxy(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.async_errors = []
        asyncio.get_running_loop().set_exception_handler(
            lambda loop, context: self.async_errors.append(context)
        )
        runs = run_directory()
        self.services = MailServices(Path(tempfile.mkdtemp(prefix="fault-", dir=runs)) / "services")
        await asyncio.to_thread(self.services.start)
        self.proxy = MailProxy(self.services)
        await self.proxy.start()
        self.context = ssl.create_default_context(cafile=str(self.services.directory / "ca.crt"))

    async def asyncTearDown(self):
        await self.proxy.stop()
        await asyncio.to_thread(self.services.stop)
        self.assertEqual(self.async_errors, [])

    async def test_unknown_protocol_tokens_are_not_logged_and_invalid_faults_are_rejected(self):
        canary = b"NUNCIO_PRIVATE_PROTOCOL_CANARY"

        def commands():
            with socket.create_connection(
                ("127.0.0.1", self.proxy.ports["imaps"]), timeout=5
            ) as tcp:
                with (
                    self.context.wrap_socket(tcp, server_hostname="127.0.0.1") as sock,
                    sock.makefile("rb") as reader,
                ):
                    self.assertTrue(reader.readline().startswith(b"* OK"))
                    sock.sendall(b"a1 " + canary + b"\r\n")
                    self.assertTrue(reader.readline().startswith(b"a1 BAD"))
            smtp = smtplib.SMTP("127.0.0.1", self.proxy.ports["smtp"], timeout=5)
            try:
                self.assertGreaterEqual(smtp.docmd(canary.decode())[0], 400)
            finally:
                smtp.close()

        await asyncio.to_thread(commands)
        self.assertNotIn(canary.decode(), str(self.proxy.snapshot()))
        self.assertEqual(self.proxy.snapshot()["requests"]["imap UNKNOWN"], 1)
        self.assertEqual(self.proxy.snapshot()["requests"]["smtp UNKNOWN"], 1)
        for protocol, command in [("smtp", "UID COPY"), ("imap", "DATA")]:
            with self.assertRaises(ValueError):
                self.proxy.inject(Fault(protocol, command, "before", "disconnect"))

    async def test_lost_copy_acknowledgement_has_exactly_one_independent_remote_copy(self):
        data = b"From: alpha@example.test\r\nTo: beta@example.test\r\nSubject: Copy fault\r\n\r\nOriginal\r\n"

        def seed():
            with imaplib.IMAP4_SSL(
                "127.0.0.1", self.services.ports["imaps"], ssl_context=self.context
            ) as mail:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                mail.append("INBOX", None, None, data)
                mail.create("Target")

        await asyncio.to_thread(seed)
        fault = Fault("imap", "UID COPY", "after", "disconnect")
        self.proxy.inject(fault)

        def copy():
            mail = imaplib.IMAP4_SSL(
                "127.0.0.1", self.proxy.ports["imaps"], ssl_context=self.context, timeout=5
            )
            try:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                mail.select("INBOX")
                with self.assertRaises(imaplib.IMAP4.abort):
                    mail.uid("copy", "1", "Target")
            finally:
                mail.shutdown()

        await asyncio.to_thread(copy)
        await asyncio.wait_for(fault.entered.wait(), 2)

        def observe():
            with imaplib.IMAP4_SSL(
                "127.0.0.1", self.services.ports["imaps"], ssl_context=self.context
            ) as mail:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                mail.select("Target")
                self.assertEqual(mail.uid("search", None, "ALL")[1], [b"1"])
                self.assertEqual(mail.uid("fetch", "1", "(BODY.PEEK[])")[1][0][1], data)

        await asyncio.to_thread(observe)
        self.assertEqual(self.proxy.snapshot()["accepted"]["imap UID COPY"], 1)

    async def test_native_move_lost_ack_preserves_copyuid_and_unrelated_deleted_messages(self):
        data = b"From: alpha@example.test\r\nTo: beta@example.test\r\nSubject: MOVE fault\r\n\r\nOriginal MOVE bytes\r\n"
        unrelated = b"Subject: Unrelated deleted message\r\n\r\nKeep this message\r\n"

        def seed():
            with imaplib.IMAP4_SSL(
                "127.0.0.1", self.services.ports["imaps"], ssl_context=self.context
            ) as mail:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                self.assertEqual(mail.append("INBOX", None, None, data)[0], "OK")
                self.assertEqual(mail.append("INBOX", "(\\Deleted)", None, unrelated)[0], "OK")
                self.assertEqual(mail.create("Target")[0], "OK")

        await asyncio.to_thread(seed)
        fault = Fault("imap", "UID MOVE", "after", "disconnect")
        self.proxy.inject(fault)

        def move():
            mail = imaplib.IMAP4_SSL(
                "127.0.0.1", self.proxy.ports["imaps"], ssl_context=self.context, timeout=5
            )
            try:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                self.assertIn("MOVE", mail.capabilities)
                self.assertEqual(mail.select("INBOX")[0], "OK")
                self.assertEqual(mail.uid("move", "1", "Missing")[0], "NO")
                self.assertFalse(fault.entered.is_set())
                with self.assertRaises(imaplib.IMAP4.abort):
                    mail.uid("move", "1", "Target")
                # MOVE may announce the mapping before its final acknowledgement.
                # A final-ack fault must preserve the already forwarded evidence.
                name, values = mail.response("COPYUID")
                self.assertEqual(name, "COPYUID")
                self.assertEqual(len(values), 1)
                validity, source, destination = values[0].split()
                self.assertGreater(int(validity), 0)
                self.assertEqual(source, b"1")
                self.assertEqual(destination, b"1")
                return int(validity)
            finally:
                mail.shutdown()

        validity = await asyncio.to_thread(move)
        await asyncio.wait_for(fault.entered.wait(), 2)

        def observe():
            with imaplib.IMAP4_SSL(
                "127.0.0.1", self.services.ports["imaps"], ssl_context=self.context
            ) as mail:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                self.assertEqual(mail.select("INBOX", readonly=True)[0], "OK")
                self.assertEqual(mail.uid("search", None, "ALL")[1], [b"2"])
                self.assertIn(b"\\Deleted", mail.uid("fetch", "2", "(FLAGS)")[1][0])
                self.assertEqual(mail.uid("fetch", "2", "(BODY.PEEK[])")[1][0][1], unrelated)
                self.assertEqual(mail.select("Target", readonly=True)[0], "OK")
                self.assertEqual(int(mail.response("UIDVALIDITY")[1][0]), validity)
                self.assertEqual(mail.uid("search", None, "ALL")[1], [b"1"])
                self.assertEqual(mail.uid("fetch", "1", "(BODY.PEEK[])")[1][0][1], data)

        await asyncio.to_thread(observe)
        observation = self.proxy.snapshot()
        self.assertEqual(observation["requests"]["imap UID MOVE"], 2)
        self.assertEqual(observation["accepted"]["imap UID MOVE"], 1)
        self.assertEqual(observation["requests"].get("imap EXPUNGE", 0), 0)
        self.assertEqual(observation["requests"].get("imap UID EXPUNGE", 0), 0)
        self.assertEqual(observation["smtp_deliveries"], [])

    async def test_smtp_accept_then_drop_preserves_remote_delivery_once(self):
        self.proxy.inject(Fault("smtp", "DATA", "after", "disconnect"))
        data = b"From: alpha@example.test\r\nTo: beta@example.test\r\nSubject: SMTP fault\r\nMessage-ID: <fault@example.test>\r\n\r\nHello\r\n.dot\r\n"

        def send():
            smtp = smtplib.SMTP("127.0.0.1", self.proxy.ports["smtp"], timeout=5)
            try:
                smtp.starttls(context=self.context)
                smtp.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                with self.assertRaises(smtplib.SMTPServerDisconnected):
                    smtp.sendmail("alpha@example.test", ["actual@example.test"], data)
            finally:
                smtp.close()

        await asyncio.to_thread(send)
        messages = await asyncio.to_thread(self.services.api, "/api/v1/messages")
        self.assertEqual(messages["total"], 1)
        raw = await asyncio.to_thread(
            self.services.api, "/api/v1/message/" + messages["messages"][0]["ID"] + "/raw", raw=True
        )
        self.assertEqual(raw[-len(data) :], data)
        self.assertEqual(self.proxy.snapshot()["accepted"]["smtp DATA"], 1)
        self.assertEqual(
            self.proxy.snapshot()["smtp_deliveries"],
            [
                {
                    "sender": "alpha@example.test",
                    "recipients": ["actual@example.test"],
                    "wire_sha256": hashlib.sha256(data).hexdigest(),
                    "wire_size": len(data),
                }
            ],
        )

    async def test_implicit_tls_submission_uses_independent_tls_server_and_lost_ack(self):
        fault = Fault("smtp", "DATA", "after", "disconnect")
        self.proxy.inject(fault)
        data = b"From: alpha@example.test\r\nTo: beta@example.test\r\nSubject: Implicit TLS\r\n\r\nExact body\r\n"

        def submit():
            smtp = smtplib.SMTP_SSL(
                "127.0.0.1", self.proxy.ports["smtps"], context=self.context, timeout=5
            )
            try:
                smtp.ehlo()
                self.assertFalse(smtp.has_extn("starttls"))
                smtp.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                with self.assertRaises(smtplib.SMTPServerDisconnected):
                    smtp.sendmail("alpha@example.test", ["beta@example.test"], data)
            finally:
                smtp.close()

        await asyncio.to_thread(submit)
        self.assertTrue(fault.entered.is_set())
        captured = await asyncio.to_thread(self.services.api, "/api/v1/messages", tls=True)
        self.assertEqual(captured["total"], 1)
        self.assertEqual(
            (await asyncio.to_thread(self.services.api, "/api/v1/messages"))["total"], 0
        )
        raw = await asyncio.to_thread(
            self.services.api,
            "/api/v1/message/" + captured["messages"][0]["ID"] + "/raw",
            raw=True,
            tls=True,
        )
        self.assertEqual(raw[-len(data) :], data)

    async def test_after_accept_fault_does_not_consume_or_replace_rejected_copy(self):
        fault = Fault("imap", "UID COPY", "after", "disconnect")
        self.proxy.inject(fault)

        def copy():
            mail = imaplib.IMAP4_SSL(
                "127.0.0.1", self.proxy.ports["imaps"], ssl_context=self.context, timeout=5
            )
            try:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                mail.append(
                    "INBOX", None, None, b"Subject: Rejection before acceptance\r\n\r\nBytes\r\n"
                )
                mail.select("INBOX")
                self.assertEqual(mail.uid("copy", "1", "Missing")[0], "NO")
                self.assertFalse(fault.entered.is_set())
                with self.assertRaises(imaplib.IMAP4.abort):
                    mail.uid("copy", "1", "Sent")
            finally:
                mail.shutdown()

        await asyncio.to_thread(copy)
        self.assertTrue(fault.entered.is_set())
        self.assertEqual(self.proxy.snapshot()["requests"]["imap UID COPY"], 2)
        self.assertEqual(self.proxy.snapshot()["accepted"]["imap UID COPY"], 1)

        def observe():
            mail = imaplib.IMAP4_SSL(
                "127.0.0.1", self.services.ports["imaps"], ssl_context=self.context, timeout=5
            )
            try:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                mail.select("Sent")
                self.assertEqual(mail.uid("search", None, "ALL")[1], [b"1"])
            finally:
                mail.shutdown()

        await asyncio.to_thread(observe)

    async def test_append_literal_withheld_ack_and_truncated_fetch_have_independent_state(self):
        data = (
            b"From: alpha@example.test\r\nTo: beta@example.test\r\nSubject: Literal boundary\r\n\r\n"
            + b"Payload0123456789" * 20
            + b"\r\n"
        )
        fault = Fault("imap", "APPEND", "after", "withhold")
        self.proxy.inject(fault)

        def append():
            with imaplib.IMAP4_SSL(
                "127.0.0.1", self.proxy.ports["imaps"], ssl_context=self.context, timeout=5
            ) as mail:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                self.assertEqual(mail.append("INBOX", None, None, data)[0], "OK")

        job = asyncio.create_task(asyncio.to_thread(append))
        try:
            await asyncio.wait_for(fault.entered.wait(), 3)
            self.assertFalse(job.done())

            def observe():
                with imaplib.IMAP4_SSL(
                    "127.0.0.1", self.services.ports["imaps"], ssl_context=self.context
                ) as mail:
                    mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                    mail.select("INBOX")
                    self.assertEqual(mail.uid("search", None, "ALL")[1], [b"1"])
                    self.assertEqual(mail.uid("fetch", "1", "(BODY.PEEK[])")[1][0][1], data)

            await asyncio.to_thread(observe)
        finally:
            fault.released.set()
            await job
        self.proxy.inject(Fault("imap", "UID FETCH", "after", "truncate"))

        def fetch():
            mail = imaplib.IMAP4_SSL(
                "127.0.0.1", self.proxy.ports["imaps"], ssl_context=self.context, timeout=5
            )
            try:
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                mail.select("INBOX")
                self.assertEqual(mail.uid("fetch", "1", "(UID FLAGS RFC822.SIZE)")[0], "OK")
                self.assertEqual(self.proxy.snapshot()["pending_faults"], 1)
                with self.assertRaises(imaplib.IMAP4.abort):
                    mail.uid("fetch", "1", "(BODY.PEEK[])")
            finally:
                mail.shutdown()

        await asyncio.to_thread(fetch)
        await asyncio.to_thread(observe)
        self.assertEqual(self.proxy.snapshot()["accepted"]["imap APPEND"], 1)

    async def test_capability_profile_rejects_unsafe_commands_and_smtp_rejection_has_no_delivery(
        self,
    ):
        await self.proxy.stop()
        self.proxy = MailProxy(
            self.services, hidden_capabilities=("MOVE", "UIDPLUS", "CONDSTORE", "QRESYNC")
        )
        await self.proxy.start()

        def commands():
            with imaplib.IMAP4("127.0.0.1", self.proxy.ports["imap"], timeout=5) as mail:
                mail.starttls(ssl_context=self.context)
                mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                capability_data = mail.capability()[1][0]
                self.assertNotIn(b"  ", capability_data)
                caps = capability_data.split()
                for absent in [b"MOVE", b"UIDPLUS", b"CONDSTORE", b"QRESYNC"]:
                    self.assertNotIn(absent, caps)
                self.assertEqual(mail.enable("CONDSTORE QRESYNC")[0], "OK")
                self.assertEqual(mail.response("ENABLED")[1], [b""])
                mail.select("INBOX")
                self.assertEqual(mail.response("HIGHESTMODSEQ")[1], [None])
                with self.assertRaises(imaplib.IMAP4.error):
                    mail._simple_command(
                        "UID", "FETCH", "1:*", "(UID FLAGS MODSEQ)", "(CHANGEDSINCE 1)"
                    )
                with self.assertRaises(imaplib.IMAP4.error):
                    mail._simple_command(
                        "UID", "STORE", "1", "(UNCHANGEDSINCE 1)", "+FLAGS.SILENT", "(\\Seen)"
                    )
                with self.assertRaises(imaplib.IMAP4.error):
                    mail._simple_command("SELECT", "INBOX", "(CONDSTORE)")
                with self.assertRaises(imaplib.IMAP4.error):
                    mail._simple_command("SELECT", "INBOX", "(QRESYNC (1 1))")
                with self.assertRaises(imaplib.IMAP4.error):
                    mail.uid("move", "1", "Sent")
                with self.assertRaises(imaplib.IMAP4.error):
                    mail.uid("expunge", "1")

        await asyncio.to_thread(commands)
        self.proxy.inject(Fault("smtp", "DATA", "before", "reject"))

        def send():
            with smtplib.SMTP("127.0.0.1", self.proxy.ports["smtp"], timeout=5) as smtp:
                smtp.starttls(context=self.context)
                smtp.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                with self.assertRaises(smtplib.SMTPDataError) as error:
                    smtp.sendmail(
                        "alpha@example.test",
                        ["beta@example.test"],
                        b"From: alpha@example.test\r\n\r\nData\r\n",
                    )
                self.assertEqual(error.exception.smtp_code, 451)

        await asyncio.to_thread(send)
        self.assertEqual(
            (await asyncio.to_thread(self.services.api, "/api/v1/messages"))["total"], 0
        )
        self.assertNotIn("smtp DATA", self.proxy.snapshot()["accepted"])

    async def test_synchronizing_append_has_exactly_one_continuation(self):
        data = b"Subject: Single continuation\r\n\r\nLiteral\r\n"

        def append():
            with socket.create_connection(
                ("127.0.0.1", self.proxy.ports["imaps"]), timeout=5
            ) as tcp:
                with self.context.wrap_socket(tcp, server_hostname="127.0.0.1") as sock:
                    stream = sock.makefile("rb")
                    self.assertTrue(stream.readline().startswith(b"* OK"))
                    password = self.services.passwords["alpha@example.test"].encode()
                    sock.sendall(b'a1 LOGIN "alpha@example.test" "' + password + b'"\r\n')
                    while True:
                        reply = stream.readline()
                        self.assertTrue(reply)
                        if reply.startswith(b"a1 "):
                            self.assertTrue(reply.startswith(b"a1 OK"))
                            break
                    sock.sendall(b"a2 APPEND INBOX {" + str(len(data)).encode() + b"}\r\n")
                    self.assertTrue(stream.readline().startswith(b"+"))
                    sock.sendall(data + b"\r\n")
                    reply = stream.readline()
                    self.assertTrue(reply.startswith(b"a2 OK"), repr(reply))
                    sock.sendall(b"a3 LOGOUT\r\n")
                    while stream.readline():
                        pass
                    stream.close()

        await asyncio.to_thread(append)

    async def test_sasl_authentication_and_idle_notifications_are_forwarded(self):
        def sasl():
            with imaplib.IMAP4_SSL(
                "127.0.0.1", self.proxy.ports["imaps"], ssl_context=self.context, timeout=3
            ) as mail:
                self.assertEqual(
                    mail.authenticate(
                        "PLAIN",
                        lambda _: (
                            b"\x00alpha@example.test\x00"
                            + self.services.passwords["alpha@example.test"].encode()
                        ),
                    )[0],
                    "OK",
                )

        await asyncio.to_thread(sasl)
        ready = asyncio.Event()
        loop = asyncio.get_running_loop()

        def idle():
            with socket.create_connection(
                ("127.0.0.1", self.proxy.ports["imaps"]), timeout=5
            ) as tcp:
                with (
                    self.context.wrap_socket(tcp, server_hostname="127.0.0.1") as sock,
                    sock.makefile("rb") as reader,
                ):
                    reader.readline()

                    def until(tag):
                        lines = []
                        while True:
                            line = reader.readline()
                            self.assertTrue(line)
                            lines.append(line)
                            if line.startswith(tag + b" "):
                                self.assertTrue(line.startswith(tag + b" OK"))
                                return lines

                    sock.sendall(
                        b'a1 LOGIN "alpha@example.test" "'
                        + self.services.passwords["alpha@example.test"].encode()
                        + b'"\r\n'
                    )
                    until(b"a1")
                    sock.sendall(b"a2 SELECT INBOX\r\n")
                    until(b"a2")
                    sock.sendall(b"a3 IDLE\r\n")
                    self.assertTrue(reader.readline().startswith(b"+"))
                    loop.call_soon_threadsafe(ready.set)
                    self.assertIn(b"EXISTS", reader.readline())
                    sock.sendall(b"DONE\r\n")
                    until(b"a3")
                    sock.sendall(b"a4 LOGOUT\r\n")
                    until(b"a4")

        job = asyncio.create_task(asyncio.to_thread(idle))
        try:
            await asyncio.wait_for(ready.wait(), 3)

            def append():
                with imaplib.IMAP4_SSL(
                    "127.0.0.1", self.services.ports["imaps"], ssl_context=self.context, timeout=3
                ) as mail:
                    mail.login("alpha@example.test", self.services.passwords["alpha@example.test"])
                    mail.append(
                        "INBOX", None, None, b"Subject: External during idle\r\n\r\nHello\r\n"
                    )

            await asyncio.to_thread(append)
        finally:
            await job


if __name__ == "__main__":
    unittest.main()
