"""Out-of-band controls; every mailbox mutation is performed by Dovecot."""

import asyncio
import base64
import hashlib
import imaplib
import mailbox
import re
import ssl
from contextlib import contextmanager
from email.message import EmailMessage
from email.policy import SMTP

from proxy import Fault


class MailControl:
    def __init__(self, services, proxy):
        self.services = services
        self.proxy = proxy
        self.faults = {}
        self.maintenance = asyncio.Lock()
        self.auto_sent = None
        self.tls = ssl.create_default_context(cafile=str(services.directory / "ca.crt"))

    @contextmanager
    def connection(self, account):
        if account not in self.services.passwords:
            raise ValueError("unknown synthetic account")
        mail = imaplib.IMAP4_SSL(
            "127.0.0.1", self.services.ports["imaps"], ssl_context=self.tls, timeout=5
        )
        try:
            mail.login(account, self.services.passwords[account])
            yield mail
        finally:
            mail.shutdown()

    def schema(self, q, fields):
        if not isinstance(q, dict) or set(q) != {"command", *fields}:
            raise ValueError("invalid control fields")

    async def handle(self, q):
        if not isinstance(q, dict):
            raise ValueError("control must be an object")
        action = q.get("command")
        if action == "seed":
            self.schema(q, [])
            self.proxy.accepting = False
            try:
                async with self.maintenance:
                    await self.proxy.drop_connections()
                    await asyncio.to_thread(self.services.reset)
                    await asyncio.to_thread(self.seed)
                    self.proxy.requests.clear()
                    self.proxy.accepted.clear()
                    self.proxy.smtp_deliveries.clear()
                    self.proxy.faults.clear()
                    self.faults.clear()
                    if self.auto_sent is not None:
                        self.auto_sent.reset()
                return {"seed": "two_accounts"}
            finally:
                self.proxy.accepting = True
        if action == "server_sent_status":
            self.schema(q, [])
            return (
                self.auto_sent.status()
                if self.auto_sent is not None
                else {"enabled": False, "completed": 0, "failed": False}
            )
        if action == "wait_server_sent":
            self.schema(q, ["count"])
            if self.auto_sent is None:
                raise ValueError("server-managed Sent is disabled")
            return await self.auto_sent.wait(q["count"])
        if action == "mailbox":
            self.schema(q, ["account", "mailbox"])
            return await asyncio.to_thread(self.observe, q["account"], q["mailbox"])
        if action == "raw":
            self.schema(q, ["account", "mailbox", "uid"])
            self.uid(q["uid"])
            return await asyncio.to_thread(self.raw, q["account"], q["mailbox"], q["uid"])
        if action == "flags":
            self.schema(q, ["account", "mailbox", "uid", "add", "remove"])
            self.uid(q["uid"])
            allowed = {"\\Seen", "\\Flagged", "\\Answered", "\\Draft", "\\Deleted"}
            for key in ("add", "remove"):
                if (
                    not isinstance(q[key], list)
                    or any(f not in allowed for f in q[key])
                    or len(set(q[key])) != len(q[key])
                ):
                    raise ValueError("invalid fixture flags")
            if set(q["add"]) & set(q["remove"]):
                raise ValueError("overlapping flags")
            return await asyncio.to_thread(self.flags, q)
        if action == "uidvalidity":
            self.schema(q, ["account", "mailbox", "value"])
            self.uid(q["value"])
            return await asyncio.to_thread(self.uidvalidity, q["account"], q["mailbox"], q["value"])
        if action == "append":
            self.schema(q, ["account", "mailbox", "raw_base64"])
            if not isinstance(q["raw_base64"], str) or len(q["raw_base64"]) > 94 * 1024 * 1024:
                raise ValueError("fixture too large")
            data = base64.b64decode(q["raw_base64"], validate=True)
            if len(data) > 70 * 1024 * 1024:
                raise ValueError("fixture too large")
            return await asyncio.to_thread(self.append, q["account"], q["mailbox"], data)
        if action in ("folder_create", "folder_delete", "folder_rename"):
            fields = ["account", "mailbox"] + (["destination"] if action == "folder_rename" else [])
            self.schema(q, fields)
            return await asyncio.to_thread(self.folder, q)
        if action in ("copy", "expunge"):
            fields = ["account", "mailbox", "uid"] + (["destination"] if action == "copy" else [])
            self.schema(q, fields)
            self.uid(q["uid"])
            return await asyncio.to_thread(self.placement, q)
        if action == "smtp":
            self.schema(q, ["transport"] if "transport" in q else [])
            tls = self.smtp_transport(q)
            return await asyncio.to_thread(self.services.api, "/api/v1/messages", tls=tls)
        if action in ("smtp_message", "smtp_raw"):
            self.schema(q, ["transport", "id"])
            tls = self.smtp_transport(q)
            if not isinstance(q["id"], str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", q["id"]):
                raise ValueError("invalid capture identity")
            path = "/api/v1/message/" + q["id"]
            if action == "smtp_raw":
                data = await asyncio.to_thread(self.services.api, path + "/raw", raw=True, tls=tls)
                return {"raw_base64": base64.b64encode(data).decode("ascii")}
            return await asyncio.to_thread(self.services.api, path, tls=tls)
        if action == "snapshot":
            self.schema(q, [])
            return self.proxy.snapshot()
        if action == "inject":
            self.schema(q, ["name", "protocol", "verb", "phase", "action"])
            name = q["name"]
            if (
                not isinstance(name, str)
                or not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", name)
                or name in self.faults
                or len(self.faults) >= 100
            ):
                raise ValueError("invalid fault identity")
            fault = Fault(q["protocol"], q["verb"], q["phase"], q["action"])
            self.proxy.inject(fault)
            self.faults[name] = fault
            return {"name": name}
        if action in ("wait_fault", "release"):
            self.schema(q, ["name"])
            fault = self.faults.get(q["name"])
            if fault is None:
                raise ValueError("unknown fault")
            if action == "wait_fault":
                await asyncio.wait_for(fault.entered.wait(), 10)
            else:
                fault.released.set()
            return {
                "name": q["name"],
                "entered": fault.entered.is_set(),
                "released": fault.released.is_set(),
            }
        raise ValueError("unsupported control")

    def uid(self, value):
        if type(value) is not int or not 0 < value <= 0xFFFFFFFF:
            raise ValueError("invalid UID/UIDVALIDITY")

    def smtp_transport(self, q):
        if q.get("transport", "starttls") not in ("starttls", "tls"):
            raise ValueError("invalid SMTP capture transport")
        return q.get("transport") == "tls"

    def observe(self, account, folder):
        with self.connection(account) as mail:
            if mail.select(mailbox.quote(folder), readonly=True)[0] != "OK":
                raise ValueError("missing fixture mailbox")
            epoch = mailbox.validity(mail)
            status, result = mail.uid("search", None, "ALL")
            if status != "OK" or len(result) != 1 or len(result[0]) > 128000:
                raise ValueError("invalid independent UID list")
            uids = result[0].split()
            if len(uids) > 10000:
                raise ValueError("too many fixture placements")
            messages = []
            for uid in uids:
                value = mailbox.fetch(mail, int(uid))
                raw = base64.b64decode(value.pop("raw_base64"))
                value.update(sha256=hashlib.sha256(raw).hexdigest(), size=len(raw))
                messages.append(value)
            return {"mailbox": folder, "uidvalidity": epoch, "messages": messages}

    def raw(self, account, folder, uid):
        with self.connection(account) as mail:
            if mail.select(mailbox.quote(folder), readonly=True)[0] != "OK":
                raise ValueError("missing fixture mailbox")
            return mailbox.fetch(mail, uid)

    def flags(self, q):
        with self.connection(q["account"]) as mail:
            if mail.select(mailbox.quote(q["mailbox"]))[0] != "OK":
                raise ValueError("missing fixture mailbox")
            mailbox.fetch(mail, q["uid"])
            for key, verb in [("add", "+FLAGS.SILENT"), ("remove", "-FLAGS.SILENT")]:
                if (
                    q[key]
                    and mail.uid("store", str(q["uid"]), verb, "(" + " ".join(q[key]) + ")")[0]
                    != "OK"
                ):
                    raise ValueError("fixture STORE failed")
        return self.observe(q["account"], q["mailbox"])

    def append(self, account, folder, data):
        with self.connection(account) as mail:
            if mail.append(mailbox.quote(folder), None, None, data)[0] != "OK":
                raise ValueError("fixture APPEND failed")
        return self.observe(account, folder)

    def uidvalidity(self, account, folder, value):
        if (
            account not in self.services.passwords
            or not isinstance(folder, str)
            or folder.startswith("-")
        ):
            raise ValueError("invalid fixture identity")
        mailbox.quote(folder)
        self.services.compose(
            "exec",
            "-T",
            "dovecot",
            "doveadm",
            "mailbox",
            "update",
            "-u",
            account,
            "--uid-validity",
            str(value),
            folder,
        )
        return self.observe(account, folder)

    def folder(self, q):
        with self.connection(q["account"]) as mail:
            source = mailbox.quote(q["mailbox"])
            if q["command"] == "folder_create":
                status, _ = mail.create(source)
            elif q["command"] == "folder_delete":
                status, _ = mail.delete(source)
            else:
                status, _ = mail.rename(source, mailbox.quote(q["destination"]))
            if status != "OK":
                raise ValueError("independent mailbox change failed")
        return {"applied": True}

    def placement(self, q):
        with self.connection(q["account"]) as mail:
            if mail.select(mailbox.quote(q["mailbox"]))[0] != "OK":
                raise ValueError("missing fixture mailbox")
            mailbox.fetch(mail, q["uid"])
            if q["command"] == "copy":
                status, _ = mail.uid("copy", str(q["uid"]), mailbox.quote(q["destination"]))
            else:
                # Never remove unrelated messages already marked Deleted.
                if b"UIDPLUS" not in mail.capability()[1][0].split():
                    raise ValueError("independent server lacks scoped expunge")
                if mail.uid("store", str(q["uid"]), "+FLAGS.SILENT", "(\\Deleted)")[0] != "OK":
                    raise ValueError("fixture delete flag failed")
                status, _ = mail.uid("expunge", str(q["uid"]))
            if status != "OK":
                raise ValueError("independent placement change failed")
        return self.observe(q["account"], q.get("destination", q["mailbox"]))

    def seed(self):
        for ordinal, account in enumerate(self.services.passwords):
            with self.connection(account) as mail:
                for number in range(2):
                    message = EmailMessage(policy=SMTP)
                    message["From"] = account
                    message["To"] = "recipient@example.test"
                    message["Date"] = "Sat, 07 Mar 2026 15:00:00 +0000"
                    message["Message-ID"] = f"<fixture-{number}-{account}>"
                    message["Subject"] = f"Independent MailPlus fixture {number}"
                    if number == 0:
                        message.set_content("Plain body with PDF attachment.\n")
                        message.add_attachment(
                            b"%PDF-1.4\nSynthetic independent fixture\n%%EOF\n",
                            maintype="application",
                            subtype="pdf",
                            filename="fixture.pdf",
                        )
                        message.set_boundary("nuncio-independent-boundary")
                    else:
                        message.set_content("<p>HTML only body &amp; context</p>\n", subtype="html")
                    data = message.as_bytes()
                    (self.services.directory / f"fixture-{ordinal}-{number}.eml").write_bytes(data)
                    if mail.append("INBOX", None, None, data)[0] != "OK":
                        raise ValueError("fixture seed failed")
            self.uidvalidity(account, "INBOX", 9001 + ordinal)
