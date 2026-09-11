"""Independent local Dovecot/Mailpit lifecycle; no production Nuncio imports."""

import argparse
import base64
import imaplib
import json
import os
import secrets
import shutil
import smtplib
import ssl
import subprocess
import time
import urllib.request
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent


def run_directory():
    path = Path(
        os.environ.get("NUNCIO_MAIL_TEST_RUNS", HERE.parents[1] / "test-results/imap-services/runs")
    )
    path.mkdir(parents=True, exist_ok=True)
    return path


class MailServices:
    def __init__(self, directory: Path):
        self.directory = directory.resolve()
        self.project = "nuncio-mail-" + uuid.uuid4().hex[:16]
        self.passwords = {}
        self.ports = {}
        self.api_password = ""
        self.started = False
        self.commands = []
        self.docker_endpoint = None

    def command(self, args, *, timeout=45):
        if args[:1] == ["docker"] and args[1:2] != ["context"]:
            if self.docker_endpoint is None:
                raise ValueError("local Docker endpoint was not validated")
            args = ["docker", "--host", self.docker_endpoint, *args[1:]]
        env = dict(os.environ, NUNCIO_MAIL_TEST_DIR=str(self.directory))
        result = subprocess.run(
            args,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            check=False,
        )
        self.commands.append({"command": args, "exit_status": result.returncode})
        if self.directory.is_dir():
            (self.directory / "commands.json").write_text(json.dumps(self.commands, indent=2))
        if result.returncode:
            raise RuntimeError(result.stdout.decode(errors="replace"))
        return result.stdout

    def compose(self, *args):
        return self.command(
            [
                "docker",
                "compose",
                "--env-file",
                "/dev/null",
                "--project-name",
                self.project,
                "-f",
                str(HERE / "compose.yaml"),
                *args,
            ]
        )

    def prepare(self):
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=False)
        for user in ("alpha@example.test", "beta@example.test"):
            self.passwords[user] = secrets.token_hex(24)
        self.api_password = secrets.token_hex(24)
        # A private ancestor protects host access; bind-mounted individual files
        # are readable by the non-root server UID inside the containers.
        for name, content in {
            "dovecot-users": "".join(f"{u}:{{PLAIN}}{p}\n" for u, p in self.passwords.items()),
            "smtp-users": "".join(f"{u}:{p}\n" for u, p in self.passwords.items()),
            "api-user": f"observer:{self.api_password}\n",
        }.items():
            (self.directory / name).write_text(content)
            (self.directory / name).chmod(0o444)
        (self.directory / "credentials.json").write_text(json.dumps(self.passwords))
        (self.directory / "credentials.json").chmod(0o600)
        shutil.copyfile(HERE / "dovecot.conf", self.directory / "dovecot.conf")

        def openssl(*args):
            return self.command(["openssl", *args])

        openssl(
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "7",
            "-subj",
            "/CN=Nuncio disposable test CA",
            "-keyout",
            str(self.directory / "ca.key"),
            "-out",
            str(self.directory / "ca.crt"),
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        )
        openssl(
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=localhost",
            "-keyout",
            str(self.directory / "server.key"),
            "-out",
            str(self.directory / "server.csr"),
        )
        extension = self.directory / "server.ext"
        extension.write_text(
            "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n"
        )
        openssl(
            "x509",
            "-req",
            "-in",
            str(self.directory / "server.csr"),
            "-CA",
            str(self.directory / "ca.crt"),
            "-CAkey",
            str(self.directory / "ca.key"),
            "-CAcreateserial",
            "-days",
            "7",
            "-extfile",
            str(extension),
            "-out",
            str(self.directory / "server.crt"),
        )
        (self.directory / "ca.key").chmod(0o600)
        (self.directory / "server.key").chmod(0o444)
        (self.directory / "project.json").write_text(
            json.dumps({"project": self.project, "docker_endpoint": self.docker_endpoint})
        )

    def start(self):
        self.require_local_docker()
        self.prepare()
        return self.launch()

    def require_local_docker(self):
        if os.environ.get("DOCKER_CONTEXT") or not os.environ.get("DOCKER_HOST"):
            endpoint = json.loads(
                self.command(
                    ["docker", "context", "inspect", "--format", "{{json .Endpoints.docker.Host}}"],
                    timeout=5,
                )
            )
        else:
            endpoint = os.environ["DOCKER_HOST"]
        if not isinstance(endpoint, str) or not endpoint.startswith("unix:///"):
            raise ValueError("test services require a local Docker Unix socket")
        self.docker_endpoint = endpoint

    def reset(self):
        self.stop()
        return self.launch()

    def launch(self):
        self.started = True
        try:
            self.compose("up", "-d", "--pull", "never", "--no-build")
            for key, name, port in [
                ("imaps", "dovecot", 31993),
                ("imap", "dovecot", 31143),
                ("smtp", "mailpit", 1025),
                ("api", "mailpit", 8025),
                ("smtps", "mailpit_tls", 1025),
                ("api_tls", "mailpit_tls", 8025),
            ]:
                address = self.compose("port", name, str(port)).decode().strip()
                host, number = address.rsplit(":", 1)
                if host != "127.0.0.1":
                    raise RuntimeError("test service must bind numeric loopback: " + repr(address))
                self.ports[key] = int(number)
            context = ssl.create_default_context(cafile=str(self.directory / "ca.crt"))
            deadline = time.monotonic() + 20
            while True:
                try:
                    with imaplib.IMAP4_SSL(
                        "127.0.0.1", self.ports["imaps"], ssl_context=context, timeout=2
                    ) as mail:
                        mail.login("alpha@example.test", self.passwords["alpha@example.test"])
                    with smtplib.SMTP("127.0.0.1", self.ports["smtp"], timeout=2) as smtp:
                        smtp.starttls(context=context)
                        smtp.login("alpha@example.test", self.passwords["alpha@example.test"])
                    with smtplib.SMTP_SSL(
                        "127.0.0.1", self.ports["smtps"], context=context, timeout=2
                    ) as smtp:
                        smtp.login("alpha@example.test", self.passwords["alpha@example.test"])
                    self.api("/api/v1/messages")
                    self.api("/api/v1/messages", tls=True)
                    break
                except (OSError, imaplib.IMAP4.error, smtplib.SMTPException):
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.1)
            ready = {
                "schema_version": 1,
                "project": self.project,
                "ports": self.ports,
                "ca_file": str(self.directory / "ca.crt"),
                "credentials_file": str(self.directory / "credentials.json"),
            }
            temporary = self.directory / "ready.tmp"
            temporary.write_text(json.dumps(ready, indent=2))
            temporary.replace(self.directory / "ready.json")
            return ready
        except BaseException:
            try:
                identifiers = self.compose("ps", "-a", "-q").decode().split()
                if identifiers:
                    (self.directory / "ports.json").write_bytes(
                        self.command(
                            [
                                "docker",
                                "inspect",
                                "--format",
                                "{{json .NetworkSettings}} {{json .HostConfig.PortBindings}} {{json .State}}",
                                *identifiers,
                            ]
                        )
                    )
                (self.directory / "startup.log").write_bytes(self.compose("logs", "--no-color"))
            except Exception:
                (self.directory / "diagnostic-error.txt").write_text(
                    "Could not collect startup diagnostics.\n"
                )
            finally:
                self.stop()
            raise

    def api(self, path, *, raw=False, tls=False):
        if not path.startswith("/api/"):
            raise ValueError("local API path required")
        port = self.ports["api_tls" if tls else "api"]
        request = urllib.request.Request(f"http://127.0.0.1:{port}{path}")
        token = base64.b64encode(("observer:" + self.api_password).encode()).decode()
        request.add_header("Authorization", "Basic " + token)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(request, timeout=5) as response:
            content = response.read(70 * 1024 * 1024 + 1)
            if len(content) > 70 * 1024 * 1024:
                raise RuntimeError("capture exceeds limit")
            return content if raw else json.loads(content)

    def stop(self):
        if self.started:
            try:
                self.compose("down", "--volumes", "--remove-orphans", "--timeout", "5")
            finally:
                self.started = False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["start", "stop"])
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    service = MailServices(args.directory)
    if args.command == "start":
        service.start()
        print(service.directory / "ready.json")
    else:
        identity = json.loads((service.directory / "project.json").read_text())
        saved = identity["project"]
        if (
            not saved.startswith("nuncio-mail-")
            or len(saved) != 28
            or not all(c in "0123456789abcdef" for c in saved[12:])
        ):
            raise ValueError("invalid test composition identity")
        service.project = saved
        service.commands = json.loads((service.directory / "commands.json").read_text())
        endpoint = identity.get("docker_endpoint")
        if endpoint is None:
            service.require_local_docker()
        elif isinstance(endpoint, str) and endpoint.startswith("unix:///"):
            service.docker_endpoint = endpoint
        else:
            raise ValueError("invalid saved local Docker endpoint")
        service.started = True
        service.stop()


if __name__ == "__main__":
    main()
