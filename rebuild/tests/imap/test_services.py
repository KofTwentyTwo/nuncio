import contextlib
import imaplib
import json
import mailbox
import os
import smtplib
import ssl
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from services import MailServices, run_directory


class IndependentMailServices(unittest.TestCase):
    def test_all_services_use_one_internal_network(self):
        runs = run_directory()
        service = MailServices(Path(tempfile.mkdtemp(prefix="isolation-", dir=runs)) / "services")
        try:
            service.start()
            self.assertEqual(len(service.compose("ps", "-q").decode().split()), 4)
            containers = (
                service.compose("ps", "-q", "dovecot", "mailpit", "mailpit_tls").decode().split()
            )
            self.assertEqual(len(containers), 3)
            networks = set()
            for container in containers:
                attached = json.loads(
                    service.command(
                        [
                            "docker",
                            "inspect",
                            "--format",
                            "{{json .NetworkSettings.Networks}}",
                            container,
                        ]
                    )
                )
                self.assertEqual(len(attached), 1)
                networks.update(value["NetworkID"] for value in attached.values())
            self.assertEqual(len(networks), 1)
            for network in networks:
                internal = json.loads(
                    service.command(
                        ["docker", "network", "inspect", "--format", "{{json .Internal}}", network]
                    )
                )
                self.assertIs(internal, True)
            relay = service.compose("ps", "-q", "relay").decode().strip()
            attached = json.loads(
                service.command(
                    ["docker", "inspect", "--format", "{{json .NetworkSettings.Networks}}", relay]
                )
            )
            self.assertEqual(len(attached), 2)
            self.assertTrue(networks < {value["NetworkID"] for value in attached.values()})
            controls = json.loads(
                service.command(
                    ["docker", "inspect", "--format", "{{json .HostConfig.Sysctls}}", relay]
                )
            )
            self.assertEqual(controls["net.ipv4.ip_forward"], "0")
            self.assertEqual(controls["net.ipv6.conf.all.forwarding"], "0")
        finally:
            service.stop()

    def test_remote_docker_context_is_rejected_before_service_mutation(self):
        service = MailServices(Path("unused-test-directory"))
        with (
            patch.dict(os.environ, {"DOCKER_CONTEXT": "synthetic-context"}),
            patch.object(service, "command") as command,
        ):
            command.return_value = b'"ssh://remote.example.test"'
            with self.assertRaises(ValueError):
                service.require_local_docker()
            self.assertEqual(command.call_count, 1)
            command.return_value = b'"unix:///synthetic/docker.sock"'
            service.require_local_docker()
        with (
            patch.dict(os.environ, {"DOCKER_CONTEXT": "", "DOCKER_HOST": "tcp://127.0.0.1:2375"}),
            patch.object(service, "command") as command,
        ):
            with self.assertRaises(ValueError):
                service.require_local_docker()
            command.assert_not_called()

    def test_tls_authentication_separate_users_and_smtp_capture(self):
        runs = run_directory()
        with contextlib.nullcontext(tempfile.mkdtemp(prefix="service-", dir=runs)) as temp:
            service = MailServices(Path(temp) / "services")
            try:
                service.start()
                ctx = ssl.create_default_context(cafile=str(service.directory / "ca.crt"))
                data = b"From: alpha@example.test\r\nTo: recipient@example.test\r\nSubject: Independent capture\r\nMessage-ID: <service-test@example.test>\r\n\r\nPayload\r\n.dot\r\n"
                with imaplib.IMAP4_SSL(
                    "127.0.0.1", service.ports["imaps"], ssl_context=ctx, timeout=5
                ) as mail:
                    self.assertEqual(
                        mail.login("alpha@example.test", service.passwords["alpha@example.test"])[
                            0
                        ],
                        "OK",
                    )
                    self.assertEqual(mail.append("INBOX", None, None, data)[0], "OK")
                    mail.select("INBOX")
                    self.assertEqual(mail.uid("search", None, "ALL")[1], [b"1"])
                    self.assertEqual(mail.uid("fetch", "1", "(BODY.PEEK[])")[1][0][1], data)
                with imaplib.IMAP4_SSL(
                    "127.0.0.1", service.ports["imaps"], ssl_context=ctx, timeout=5
                ) as mail:
                    mail.login("beta@example.test", service.passwords["beta@example.test"])
                    mail.select("INBOX")
                    self.assertEqual(mail.uid("search", None, "ALL")[1], [b""])
                with smtplib.SMTP("127.0.0.1", service.ports["smtp"], timeout=5) as smtp:
                    smtp.ehlo()
                    with self.assertRaises(smtplib.SMTPException):
                        smtp.login("alpha@example.test", service.passwords["alpha@example.test"])
                    smtp.starttls(context=ctx)
                    smtp.login("alpha@example.test", service.passwords["alpha@example.test"])
                    self.assertEqual(
                        smtp.sendmail(
                            "alpha@example.test",
                            ["recipient@example.test", "blind@example.test"],
                            data,
                        ),
                        {},
                    )
                messages = service.api("/api/v1/messages")
                self.assertEqual(messages["total"], 1)
                message_id = messages["messages"][0]["ID"]
                details = service.api("/api/v1/message/" + message_id)
                self.assertEqual([v["Address"] for v in details["Bcc"]], ["blind@example.test"])
                self.assertEqual(details["ReturnPath"], "alpha@example.test")
                self.assertIn("alpha@example.test", details["Tags"])
                captured = service.api("/api/v1/message/" + message_id + "/raw", raw=True)
                (service.directory / "smtp-captured.eml").write_bytes(captured)
                # SMTP reception prepends transport trace headers. Every submitted
                # header/body byte must still appear as the exact suffix.
                self.assertEqual(captured[-len(data) :], data)
                self.assertRegex(
                    captured[: -len(data)],
                    rb"\ABcc: blind@example.test\r\nReturn-Path: <alpha@example\.test>\r\nReceived:[^\r\n]*(?:\r\n[ \t][^\r\n]*)*\r\n\Z",
                )
                self.assertTrue((service.directory / "ready.json").is_file())
                self.assertEqual(
                    json.loads((service.directory / "ready.json").read_text())["ports"],
                    service.ports,
                )
                with self.assertRaises(ssl.SSLCertVerificationError):
                    imaplib.IMAP4_SSL(
                        "127.0.0.1",
                        service.ports["imaps"],
                        ssl_context=ssl.create_default_context(),
                        timeout=5,
                    )
            finally:
                service.stop()

    def test_strict_uid_copy_scoped_expunge_and_uidvalidity_reset(self):
        runs = run_directory()
        service = MailServices(Path(tempfile.mkdtemp(prefix="protocol-", dir=runs)) / "services")
        try:
            service.start()
            ctx = ssl.create_default_context(cafile=str(service.directory / "ca.crt"))
            data = b"From: alpha@example.test\r\nTo: recipient@example.test\r\nSubject: Exact copy\r\nMessage-ID: <copy@example.test>\r\n\r\nContent\r\n"
            target = mailbox.quote('Archive "quoted" & \u65e5\u672c\u8a9e')
            with imaplib.IMAP4("127.0.0.1", service.ports["imap"], timeout=5) as mail:
                self.assertIn("LOGINDISABLED", mail.capabilities)
                mail.starttls(ssl_context=ctx)
                mail.login("alpha@example.test", service.passwords["alpha@example.test"])
                capability = mail.capability()[1][0].split()
                self.assertIn(b"UIDPLUS", capability)
                self.assertIn(b"MOVE", capability)
                self.assertEqual(mail.create(target)[0], "OK")
                self.assertEqual(mail.append("INBOX", None, None, data)[0], "OK")
                self.assertEqual(
                    mail.append("INBOX", None, None, data.replace(b"Content", b"Unrelated"))[0],
                    "OK",
                )
                mail.select("INBOX")
                self.assertGreater(mailbox.validity(mail), 0)
                mail.uid("store", "2", "+FLAGS.SILENT", "(\\Deleted)")
                self.assertEqual(mail.uid("copy", "1", target)[0], "OK")
                _, copyuid = mail.response("COPYUID")
                self.assertRegex(copyuid[0], rb"^[0-9]+ 1 [0-9]+$")
                mail.uid("store", "1", "+FLAGS.SILENT", "(\\Deleted)")
                self.assertEqual(mail.uid("expunge", "1")[0], "OK")
                self.assertEqual(mail.uid("search", None, "ALL")[1], [b"2"])
                self.assertIn("\\Deleted", mailbox.fetch(mail, 2)["flags"])
                mail.select(target)
                old_validity = mailbox.validity(mail)
                observed = mailbox.fetch(mail, 1)
                import base64

                self.assertEqual(base64.b64decode(observed["raw_base64"]), data)
                mail.unselect()
                self.assertEqual(mail.delete(target)[0], "OK")
                self.assertEqual(mail.create(target)[0], "OK")
                mail.select(target)
                self.assertNotEqual(mailbox.validity(mail), old_validity)
                self.assertEqual(mail.uid("search", None, "ALL")[1], [b""])
                (service.directory / "protocol-evidence.json").write_text(
                    json.dumps(
                        {
                            "capabilities": [c.decode() for c in capability],
                            "copied": observed,
                            "unrelated_deleted_uid_survived": 2,
                        }
                    )
                )
                # Dovecot must reject malformed commands instead of accepting an
                # accidental empty UID set as a successful quiet delta.
                with self.assertRaises(imaplib.IMAP4.error):
                    mail.uid("fetch", "", "(FLAGS)")
            with smtplib.SMTP("127.0.0.1", service.ports["smtp"], timeout=5) as smtp:
                smtp.starttls(context=ctx)
                with self.assertRaises(smtplib.SMTPAuthenticationError):
                    smtp.login("alpha@example.test", service.passwords["beta@example.test"])
                smtp.login("alpha@example.test", service.passwords["alpha@example.test"])
                for _ in range(2):
                    smtp.sendmail("alpha@example.test", ["recipient@example.test"], data)
            self.assertEqual(service.api("/api/v1/messages")["total"], 2)
        finally:
            service.stop()


if __name__ == "__main__":
    unittest.main()
