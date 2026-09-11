use super::MailError;
use crate::domain::imap::MailboxName;

// imap-proto validates the response grammar but retains quoted escape bytes in
// its Cow<str>. Inspect the original representation so literal backslashes are
// never confused with quoted escapes.
pub(super) fn mailbox(raw: &[u8]) -> Result<MailboxName, MailError> {
    // ResponseData owns a buffer block, which may contain another response or
    // spare initialized bytes. Only the first parsed response belongs here.
    let (remaining, _) =
        async_imap::imap_proto::parser::parse_response(raw).map_err(|_| MailError::Protocol)?;
    let raw = &raw[..raw.len() - remaining.len()];
    let end = raw
        .iter()
        .position(|b| *b == b')')
        .ok_or(MailError::Protocol)?;
    let rest = raw
        .get(end + 1..)
        .and_then(|r| r.strip_prefix(b" "))
        .ok_or(MailError::Protocol)?;
    let rest = if let Some(rest) = rest.strip_prefix(b"NIL") {
        rest
    } else {
        string(rest)?.1
    };
    let rest = rest.strip_prefix(b" ").ok_or(MailError::Protocol)?;
    let (name, rest) = if matches!(rest.first(), Some(b'"' | b'{')) {
        string(rest)?
    } else {
        let end = rest
            .iter()
            .position(|b| *b == b'\r')
            .ok_or(MailError::Protocol)?;
        (rest[..end].to_vec(), &rest[end..])
    };
    if rest != b"\r\n" {
        return Err(MailError::Protocol);
    }
    MailboxName::from_wire(std::str::from_utf8(&name).map_err(|_| MailError::Protocol)?)
        .map_err(|_| MailError::Protocol)
}
fn string(raw: &[u8]) -> Result<(Vec<u8>, &[u8]), MailError> {
    if raw.first() == Some(&b'"') {
        let mut result = Vec::new();
        let mut i = 1;
        while let Some(&byte) = raw.get(i) {
            i += 1;
            match byte {
                b'"' => return Ok((result, &raw[i..])),
                b'\\' => {
                    let next = *raw.get(i).ok_or(MailError::Protocol)?;
                    if !matches!(next, b'\\' | b'"') {
                        return Err(MailError::Protocol);
                    }
                    result.push(next);
                    i += 1;
                }
                b'\r' | b'\n' | 0 => return Err(MailError::Protocol),
                byte => result.push(byte),
            }
        }
        return Err(MailError::Protocol);
    }
    let raw = raw.strip_prefix(b"{").ok_or(MailError::Protocol)?;
    let end = raw
        .iter()
        .position(|b| *b == b'}')
        .ok_or(MailError::Protocol)?;
    let size = std::str::from_utf8(&raw[..end])
        .map_err(|_| MailError::Protocol)?
        .parse::<usize>()
        .map_err(|_| MailError::Protocol)?;
    if size > 8192 {
        return Err(MailError::Protocol);
    }
    let raw = raw[end..]
        .strip_prefix(b"}\r\n")
        .ok_or(MailError::Protocol)?;
    let (value, rest) = raw.split_at_checked(size).ok_or(MailError::Protocol)?;
    Ok((value.to_vec(), rest))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_escapes_and_literal_backslashes_have_distinct_meanings() {
        let quoted = b"* LIST (\\HasNoChildren) \"/\" \"a\\\"b\\\\c\"\r\n";
        let literal = b"* LIST (\\HasNoChildren) \"/\" {5}\r\na\"b\\c\r\n";
        for wire in [quoted.as_slice(), literal.as_slice()] {
            assert!(async_imap::imap_proto::parser::parse_response(wire).is_ok());
            assert_eq!(
                mailbox(wire).map(|n| n.as_str().to_owned()).ok(),
                Some("a\"b\\c".into())
            );
        }
        let mut buffered = quoted.to_vec();
        buffered.extend_from_slice(b"tag OK complete\r\n\0\0");
        assert_eq!(
            mailbox(&buffered).map(|n| n.as_str().to_owned()).ok(),
            Some("a\"b\\c".into())
        );
        let literal = b"* LIST () NIL {4}\r\na\\\\b\r\n";
        assert_eq!(
            mailbox(literal).map(|n| n.as_str().to_owned()).ok(),
            Some("a\\\\b".into())
        );
    }
}
