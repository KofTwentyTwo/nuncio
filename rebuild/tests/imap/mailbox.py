"""Independent test-side IMAP mailbox encoding and observations (RFC 3501)."""

import base64
import re


def encode(name: str) -> str:
    chunks = []
    pending = []

    def flush():
        if pending:
            data = "".join(pending).encode("utf-16-be")
            chunks.append("&" + base64.b64encode(data).decode().rstrip("=").replace("/", ",") + "-")
            pending.clear()

    for char in name:
        if " " <= char <= "~":
            flush()
            chunks.append("&-" if char == "&" else char)
        else:
            pending.append(char)
    flush()
    return "".join(chunks)


def quote(name: str) -> str:
    if (
        not isinstance(name, str)
        or not name
        or len(name.encode("utf-8")) > 2048
        or any(ord(c) < 32 or ord(c) == 127 for c in name)
    ):
        raise ValueError("invalid fixture mailbox")
    return '"' + encode(name).replace("\\", "\\\\").replace('"', '\\"') + '"'


def validity(mail) -> int:
    _, values = mail.response("UIDVALIDITY")
    if len(values) != 1 or not values[0].isdigit():
        raise ValueError("missing independent UIDVALIDITY")
    return int(values[0])


def fetch(mail, uid: int):
    status, values = mail.uid("fetch", str(uid), "(UID FLAGS RFC822.SIZE BODY.PEEK[])")
    if status != "OK":
        raise ValueError("independent FETCH failed")
    literals = [v for v in values if isinstance(v, tuple)]
    if len(literals) != 1:
        raise ValueError("independent FETCH missing/duplicate object")
    header, raw = literals[0]
    found = re.search(rb"\bUID ([0-9]+)\b", header)
    size = re.search(rb"\bRFC822.SIZE ([0-9]+)\b", header)
    flags = re.search(rb"\bFLAGS \(([^)]*)\)", header)
    if not found or int(found[1]) != uid or not size or int(size[1]) != len(raw) or flags is None:
        raise ValueError("invalid independent FETCH metadata")
    return {
        "uid": uid,
        "flags": flags[1].decode("ascii").split(),
        "raw_base64": base64.b64encode(raw).decode("ascii"),
    }
