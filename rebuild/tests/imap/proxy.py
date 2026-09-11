"""Protocol-aware fault control in front of independent local mail servers.

Only framing and acknowledgement boundaries are handled here. Dovecot/Mailpit
own command validation, mailbox state, accepted deliveries, and resulting bytes.
"""

import asyncio
import hashlib
import re
import ssl
from collections import Counter
from dataclasses import dataclass, field

MAX_LITERAL = 70 * 1024 * 1024


@dataclass
class Fault:
    protocol: str
    command: str
    phase: str
    action: str
    entered: asyncio.Event = field(default_factory=asyncio.Event)
    released: asyncio.Event = field(default_factory=asyncio.Event)

    def validate(self):
        if self.protocol not in ("imap", "smtp") or self.phase not in ("before", "after"):
            raise ValueError("unsupported fault boundary")
        if self.action not in ("disconnect", "withhold", "reject", "truncate"):
            raise ValueError("unsupported fault action")
        if self.action == "reject" and self.phase != "before":
            raise ValueError("a rejection cannot replace an accepted acknowledgement")
        if self.action == "truncate" and (
            self.protocol != "imap" or self.command != "UID FETCH" or self.phase != "after"
        ):
            raise ValueError("truncate applies to the first FETCH response literal")
        commands = {
            "imap": {
                "CAPABILITY",
                "LOGIN",
                "AUTHENTICATE",
                "LIST",
                "UID SEARCH",
                "UID FETCH",
                "UID COPY",
                "UID MOVE",
                "UID STORE",
                "UID EXPUNGE",
                "APPEND",
                "SELECT",
                "EXAMINE",
            },
            "smtp": {"DATA", "AUTH", "MAIL", "RCPT"},
        }
        if self.command not in commands[self.protocol]:
            raise ValueError("unsupported fault command")


class MailProxy:
    def __init__(self, services, *, hidden_capabilities=()):
        if any(
            cap not in ("MOVE", "UIDPLUS", "CONDSTORE", "QRESYNC") for cap in hidden_capabilities
        ):
            raise ValueError("unsupported capability profile")
        self.services = services
        self.hidden = set(hidden_capabilities)
        if "CONDSTORE" in self.hidden:
            self.hidden.add("QRESYNC")
        self.ports = {}
        self.listeners = []
        self.tasks = set()
        self.accepting = True
        self.faults = []
        self.requests = Counter()
        self.accepted = Counter()
        self.smtp_deliveries = []
        self.server_tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.server_tls.minimum_version = ssl.TLSVersion.TLSv1_2
        self.server_tls.load_cert_chain(
            services.directory / "server.crt", services.directory / "server.key"
        )
        self.client_tls = ssl.create_default_context(cafile=str(services.directory / "ca.crt"))

    def inject(self, fault):
        fault.validate()
        if len(self.faults) >= 100:
            raise ValueError("fault queue full")
        self.faults.append(fault)

    def snapshot(self):
        return {
            "requests": dict(self.requests),
            "accepted": dict(self.accepted),
            "active_connections": len(self.tasks),
            "pending_faults": len(self.faults),
            "smtp_deliveries": list(self.smtp_deliveries),
        }

    async def start(self):
        for name, protocol, tls in [
            ("imaps", "imap", True),
            ("imap", "imap", False),
            ("smtp", "smtp", False),
            ("smtps", "smtp", True),
        ]:

            async def handler(reader, writer, protocol=protocol, tls=tls):
                await self._connection(reader, writer, protocol, tls)

            server = await asyncio.start_server(
                handler, "127.0.0.1", 0, ssl=self.server_tls if tls else None, limit=65536
            )
            self.listeners.append(server)
            self.ports[name] = server.sockets[0].getsockname()[1]

    async def stop(self):
        for listener in self.listeners:
            listener.close()
        await asyncio.gather(*(listener.wait_closed() for listener in self.listeners))
        await self.drop_connections()
        self.listeners.clear()

    async def drop_connections(self):
        for task in list(self.tasks):
            task.cancel()
        await asyncio.gather(*list(self.tasks), return_exceptions=True)

    def _take(self, protocol, command, phase):
        for i, fault in enumerate(self.faults):
            if (fault.protocol, fault.command, fault.phase) == (protocol, command, phase):
                return self.faults.pop(i)
        return None

    async def _fault(self, fault, writer, tag=b""):
        if fault is None:
            return True
        fault.entered.set()
        if fault.action == "withhold":
            await fault.released.wait()
            return True
        if fault.action == "reject":
            await self._write(
                writer,
                tag + b" NO injected rejection\r\n" if tag else b"451 injected rejection\r\n",
            )
        return False

    async def _connection(self, reader, writer, protocol, tls):
        task = asyncio.current_task()
        if not self.accepting or len(self.tasks) >= 64:
            writer.close()
            return
        self.tasks.add(task)
        backend = None
        try:
            port = self.services.ports[protocol + "s" if tls else protocol]
            remote, backend = await asyncio.open_connection(
                "127.0.0.1",
                port,
                ssl=self.client_tls if tls else None,
                server_hostname="127.0.0.1" if tls else None,
                limit=65536,
            )
            if protocol == "imap":
                await asyncio.wait_for(self._imap(reader, writer, remote, backend), 60)
            else:
                await asyncio.wait_for(self._smtp(reader, writer, remote, backend), 60)
        except (OSError, ValueError, EOFError, asyncio.IncompleteReadError, TimeoutError):
            pass
        finally:
            for stream in (writer, backend):
                if stream is not None:
                    stream.close()
                    try:
                        await asyncio.wait_for(stream.wait_closed(), 1)
                    except (OSError, TimeoutError):
                        pass
            self.tasks.discard(task)

    async def _line(self, reader):
        line = await reader.readline()
        if not line or not line.endswith(b"\r\n"):
            raise EOFError("incomplete protocol line")
        return line

    async def _write(self, writer, data):
        writer.write(data)
        await writer.drain()

    def _capabilities(self, line):
        if not self.hidden:
            return line
        if b"CAPABILITY" in line:
            for cap in self.hidden:
                line = re.sub(rb" " + cap.encode() + rb"(?=[ \]\r])", b"", line)
        if "UIDPLUS" in self.hidden:
            line = re.sub(rb"\[(?:APPENDUID|COPYUID) [^\]]+\] ?", b"", line)
        if "CONDSTORE" in self.hidden and re.match(rb"^\* OK \[(?:HIGHESTMODSEQ|NOMODSEQ)\b", line):
            return b""
        return line

    def _profile_command(self, line, command):
        if command == "ENABLE":
            # RFC5161 ignores unknown extensions. Forward distinct unknown
            # names so Dovecot still validates ENABLE syntax and auth state.
            tokens = line.rstrip(b"\r\n").split()
            for i in range(2, len(tokens)):
                if tokens[i].decode("ascii").upper() in self.hidden:
                    tokens[i] = b"NUNCIO-UNSUPPORTED-" + tokens[i]
            return b" ".join(tokens) + b"\r\n"
        arguments = re.sub(rb'"(?:[^"\\]|\\.)*"', b'""', line.upper())
        if "CONDSTORE" in self.hidden:
            expressions = {
                "SELECT": rb"\(CONDSTORE\b",
                "EXAMINE": rb"\(CONDSTORE\b",
                "UID FETCH": rb"\b(?:MODSEQ|CHANGEDSINCE)\b",
                "FETCH": rb"\b(?:MODSEQ|CHANGEDSINCE)\b",
                "UID STORE": rb"\bUNCHANGEDSINCE\b",
                "STORE": rb"\bUNCHANGEDSINCE\b",
                "UID SEARCH": rb"\bMODSEQ\b",
                "SEARCH": rb"\bMODSEQ\b",
                "STATUS": rb"\bHIGHESTMODSEQ\b",
            }
            if command in expressions and re.search(expressions[command], arguments):
                return None
        if "QRESYNC" in self.hidden:
            if command in ("SELECT", "EXAMINE") and re.search(rb"\(QRESYNC\b", arguments):
                return None
            if command in ("UID FETCH", "FETCH") and re.search(rb"\bVANISHED\b", arguments):
                return None
        return line

    async def _imap_response(self, reader, writer, tag, *, continuation=False, fault=None):
        size = 0
        while True:
            line = await self._line(reader)
            size += len(line)
            if size > MAX_LITERAL:
                raise ValueError("response exceeds test bound")
            if line.startswith(tag + b" "):
                return line
            if line.startswith(b"+") and continuation:
                return line
            await self._write(writer, self._capabilities(line))
            literal = re.search(rb"\{([0-9]+)\}\r\n$", line)
            if literal:
                length = int(literal[1])
                if length > MAX_LITERAL - size:
                    raise ValueError("response literal exceeds test bound")
                size += length
                if fault is not None and fault.action == "truncate":
                    fault.entered.set()
                    prefix = await reader.readexactly(max(1, length // 2))
                    await self._write(writer, prefix)
                    raise EOFError("injected truncated literal")
                while length:
                    chunk = await reader.readexactly(min(65536, length))
                    await self._write(writer, chunk)
                    length -= len(chunk)

    async def _imap(self, reader, writer, remote, backend):
        await self._write(writer, self._capabilities(await self._line(remote)))
        while True:
            line = await self._line(reader)
            pieces = line.split(maxsplit=3)
            if len(pieces) < 2:
                raise ValueError("missing command tag")
            tag, verb = pieces[0], pieces[1].upper()
            command = verb.decode("ascii")
            if verb == b"UID" and len(pieces) >= 3:
                command += " " + pieces[2].decode("ascii").upper()
            if command not in {
                "CAPABILITY",
                "LOGIN",
                "AUTHENTICATE",
                "STARTTLS",
                "ENABLE",
                "ID",
                "NAMESPACE",
                "LIST",
                "LSUB",
                "SELECT",
                "EXAMINE",
                "CLOSE",
                "UNSELECT",
                "FETCH",
                "STORE",
                "SEARCH",
                "SORT",
                "THREAD",
                "STATUS",
                "APPEND",
                "COPY",
                "MOVE",
                "EXPUNGE",
                "UID FETCH",
                "UID STORE",
                "UID SEARCH",
                "UID SORT",
                "UID THREAD",
                "UID COPY",
                "UID MOVE",
                "UID EXPUNGE",
                "CREATE",
                "DELETE",
                "RENAME",
                "SUBSCRIBE",
                "UNSUBSCRIBE",
                "GETMETADATA",
                "SETMETADATA",
                "GETQUOTA",
                "SETQUOTA",
                "GETQUOTAROOT",
                "NOOP",
                "IDLE",
                "LOGOUT",
            }:
                command = "UNKNOWN"
            key = "imap " + command
            self.requests[key] += 1
            enable_names = line.split()[2:] if command == "ENABLE" else []
            empty_enable = bool(enable_names) and all(
                name.decode("ascii").upper() in self.hidden for name in enable_names
            )
            if (
                command == "UID MOVE"
                and "MOVE" in self.hidden
                or command == "UID EXPUNGE"
                and "UIDPLUS" in self.hidden
            ):
                await self._write(writer, tag + b" BAD unsupported capability\r\n")
                continue
            line = self._profile_command(line, command)
            if line is None:
                await self._write(writer, tag + b" BAD unsupported extension syntax\r\n")
                continue
            fault = self._take("imap", command, "before")
            if not await self._fault(fault, writer, tag):
                if fault.action == "reject":
                    continue
                return
            await self._write(backend, line)
            literal = re.search(rb"\{([0-9]+)(\+)?\}\r\n$", line)
            while literal:
                length = int(literal[1])
                if length > MAX_LITERAL:
                    raise ValueError("request literal exceeds test bound")
                if not literal[2]:
                    response = await self._imap_response(remote, writer, tag, continuation=True)
                    await self._write(writer, response)
                    if not response.startswith(b"+"):
                        break
                while length:
                    chunk = await reader.readexactly(min(65536, length))
                    await self._write(backend, chunk)
                    length -= len(chunk)
                line = await self._line(reader)
                await self._write(backend, line)
                literal = re.search(rb"\{([0-9]+)(\+)?\}\r\n$", line)
            else:
                fault = self._take("imap", command, "after")
                response = await self._imap_dialogue(
                    reader, writer, remote, backend, tag, verb, fault
                )
                success = response.startswith(tag + b" OK")
                if success:
                    self.accepted[key] += 1
                    if fault is not None and fault.action == "truncate":
                        # Metadata-only FETCH completed without a literal. Keep
                        # the fault for an actual body response.
                        self.faults.insert(0, fault)
                        fault = None
                    if not await self._fault(fault, writer, tag):
                        return
                elif fault is not None:
                    # An after-accept boundary cannot hide a server rejection.
                    self.faults.insert(0, fault)
                if success and empty_enable:
                    # Dovecot2.4.5 omits ENABLED when all names are unknown.
                    # This capability profile emits RFC5161's required empty
                    # response only after the server accepts the command.
                    await self._write(writer, b"* ENABLED\r\n")
                await self._write(writer, self._capabilities(response))
                if verb == b"STARTTLS" and success:
                    await backend.start_tls(self.client_tls, server_hostname="127.0.0.1")
                    await writer.start_tls(self.server_tls)
                if verb == b"LOGOUT":
                    return

    async def _imap_dialogue(self, reader, writer, remote, backend, tag, verb, fault):
        if verb == b"AUTHENTICATE":
            while True:
                reply = await self._imap_response(remote, writer, tag, continuation=True)
                if not reply.startswith(b"+"):
                    return reply
                await self._write(writer, reply)
                await self._write(backend, await self._line(reader))
        if verb == b"IDLE":
            reply = await self._imap_response(remote, writer, tag, continuation=True)
            if not reply.startswith(b"+"):
                return reply
            await self._write(writer, reply)
            response = asyncio.create_task(self._imap_response(remote, writer, tag))
            try:
                done = await self._line(reader)
                if done.upper() != b"DONE\r\n":
                    raise ValueError("IDLE requires DONE")
                await self._write(backend, done)
                return await response
            finally:
                if not response.done():
                    response.cancel()
                await asyncio.gather(response, return_exceptions=True)
        return await self._imap_response(remote, writer, tag, fault=fault)

    async def _smtp_response(self, reader):
        data = bytearray()
        while True:
            line = await self._line(reader)
            if not re.match(rb"^[0-9]{3}[ -]", line):
                raise ValueError("invalid SMTP response")
            data.extend(line)
            if len(data) > 65536:
                raise ValueError("SMTP response too large")
            if line[3:4] == b" ":
                return bytes(data), int(line[:3])

    async def _smtp(self, reader, writer, remote, backend):
        greeting, _ = await self._smtp_response(remote)
        await self._write(writer, greeting)
        sender = None
        recipients = []
        while True:
            line = await self._line(reader)
            command = line.split(maxsplit=1)[0].decode("ascii").upper()
            if command not in {
                "HELO",
                "EHLO",
                "STARTTLS",
                "AUTH",
                "MAIL",
                "RCPT",
                "DATA",
                "RSET",
                "NOOP",
                "QUIT",
                "HELP",
                "VRFY",
                "EXPN",
            }:
                command = "UNKNOWN"
            key = "smtp " + command
            self.requests[key] += 1
            if command == "DATA" and len(self.smtp_deliveries) >= 1000:
                await self._write(writer, b"451 test observation capacity exhausted\r\n")
                continue
            fault = self._take("smtp", command, "before")
            if not await self._fault(fault, writer):
                if fault.action == "reject":
                    continue
                return
            await self._write(backend, line)
            response, code = await self._smtp_response(remote)
            wire_hash = None
            wire_size = 0
            if command == "AUTH":
                while code == 334:
                    await self._write(writer, response)
                    await self._write(backend, await self._line(reader))
                    response, code = await self._smtp_response(remote)
            if command == "DATA" and code == 354:
                await self._write(writer, response)
                size = 0
                wire_hash = hashlib.sha256()
                while True:
                    data = await self._line(reader)
                    size += len(data)
                    if size > MAX_LITERAL:
                        raise ValueError("SMTP data too large")
                    await self._write(backend, data)
                    if data == b".\r\n":
                        break
                    original = data[1:] if data.startswith(b".") else data
                    wire_hash.update(original)
                    wire_size += len(original)
                response, code = await self._smtp_response(remote)
            if 200 <= code < 300:
                if command == "MAIL":
                    sender = self._smtp_path(line, b"MAIL FROM:")
                    recipients = []
                elif command == "RCPT":
                    if len(recipients) >= 100:
                        raise ValueError("SMTP recipient observation limit")
                    recipients.append(self._smtp_path(line, b"RCPT TO:"))
                elif command in ("RSET", "STARTTLS"):
                    sender, recipients = None, []
                elif command == "DATA" and wire_hash is not None:
                    if sender is None or not recipients:
                        raise ValueError("accepted DATA without observed envelope")
                    self.smtp_deliveries.append(
                        {
                            "sender": sender,
                            "recipients": recipients,
                            "wire_sha256": wire_hash.hexdigest(),
                            "wire_size": wire_size,
                        }
                    )
                self.accepted[key] += 1
                if not await self._fault(self._take("smtp", command, "after"), writer):
                    return
            await self._write(writer, response)
            if wire_hash is not None:
                sender, recipients = None, []
            if command == "STARTTLS" and code == 220:
                await backend.start_tls(self.client_tls, server_hostname="127.0.0.1")
                await writer.start_tls(self.server_tls)
            if command == "QUIT":
                return

    def _smtp_path(self, line, prefix):
        if not line.upper().startswith(prefix):
            raise ValueError("invalid accepted SMTP path prefix")
        path = line[len(prefix) :].lstrip()
        if not path.startswith(b"<"):
            raise ValueError("invalid accepted SMTP path")
        quoted = escaped = False
        for offset, byte in enumerate(path[1:], 1):
            if offset > 513:
                raise ValueError("SMTP path observation limit")
            if escaped:
                escaped = False
            elif byte == 92 and quoted:
                escaped = True
            elif byte == 34:
                quoted = not quoted
            elif byte == 62 and not quoted:
                return path[1:offset].decode("utf-8")
        raise ValueError("incomplete accepted SMTP path")
