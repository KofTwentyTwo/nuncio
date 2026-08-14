//! Raw IMAP command layer: the responses `async-imap`'s typed API discards.
//!
//! The engine needs five things the typed client cannot express, and they all
//! decide whether a sync saw everything, whether a mutation actually happened,
//! or whether two engines can agree on what a message *is*:
//!
//! | Needed | Why the typed API cannot give it |
//! | --- | --- |
//! | `ENABLE` | not implemented at all, and RFC 7162 3.2.3 makes a server reply `BAD` to a QRESYNC `SELECT` without it |
//! | `SELECT (QRESYNC …)` | no such method, and `select()`'s parser drops `VANISHED` |
//! | `COPYUID` | `uid_copy`/`uid_mv` return `Result<()>`; the tagged response *code* is discarded |
//! | `[MODIFIED …]` | same discard, and `imap-proto` has no `ResponseCode` variant for it at all |
//! | `EMAILID` | `imap-proto` cannot *parse* it, and failing to parse kills the connection -- see [`uid_fetch_email_ids`] |
//!
//! ## Why this is hand-rolled rather than a fork
//!
//! `Session::run_command` and `Session::read_response` are public, and
//! `imap-proto` already parses `Response::Vanished` and
//! `ResponseCode::CopyUid`. So the only thing missing is a collector that
//! reads responses until the tagged one and keeps what it saw -- roughly a
//! hundred lines, against maintaining a fork of the client for the same
//! result. The one genuine gap is `[MODIFIED …]`, handled below.
//!
//! ## The `MODIFIED` gap
//!
//! `imap-proto` 0.16.7 has no `ResponseCode::Modified`. Its `resp_text` parser
//! is `opt(resp_text_code)` followed by free text, so an unrecognised code is
//! not an error: `opt` yields `None` and the whole `[MODIFIED 7,9] …` is
//! carried through as the response's `information` string. That is where this
//! module recovers it from. Parsing a response code out of a human-readable
//! field is not how this should work forever -- upstreaming a `Modified`
//! variant is the real fix -- but it is deterministic, it is covered by tests
//! against a server that emits the real syntax, and the alternative is having
//! no conflict detection at all.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};

use async_imap::Session;
// Reached through `async-imap`'s re-export rather than a direct dependency,
// so the parser types can never version-skew against the client using them.
use async_imap::imap_proto::{Response, ResponseCode, Status, UidSetMember};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::parser::MailError;

/// Bounded wait for any single response line.
///
/// A server that accepts a command and then stops answering must surface as a
/// stall, not as a sync that never returns. Mirrors the per-item FETCH timeout
/// the typed path already applies -- an unbounded read here would reintroduce
/// exactly the hang that bounded it.
const RESPONSE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// A `COPYUID` response code (RFC 4315 section 3): the destination mailbox's
/// UIDVALIDITY, the source UIDs, and the UIDs they became.
///
/// Its presence is the only positive proof that a `COPY`/`MOVE` actually moved
/// something. Its **absence proves nothing**: RFC 6851 section 4.3 makes it
/// only `SHOULD` for `MOVE`, and RFC 4315 section 3 permits omitting it for a
/// `UIDNOTSTICKY` destination or one the client cannot `SELECT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyUid {
    /// UIDVALIDITY of the destination mailbox.
    pub uid_validity: u32,
    /// Source UIDs, expanded from the wire's set syntax.
    pub source_uids: Vec<u32>,
    /// Destination UIDs, positionally aligned with `source_uids`.
    pub destination_uids: Vec<u32>,
}

/// Everything one command's responses carried, tagged and untagged.
#[derive(Debug, Default, Clone)]
pub struct RawExchange {
    /// `true` when the tagged response was `OK`.
    pub ok: bool,
    /// The tagged response's human text, which is also where an unparsed
    /// response code such as `[MODIFIED …]` ends up.
    pub information: Option<String>,
    /// `COPYUID`, from wherever it arrived. RFC 6851 section 4.3 advises
    /// servers to send it in an **untagged** `OK` for `MOVE`, so a collector
    /// that only inspected the tagged response would miss it on every
    /// conforming server.
    pub copy_uid: Option<CopyUid>,
    /// UIDs reported by `VANISHED` (RFC 7162), whether or not `(EARLIER)`.
    pub vanished_uids: Vec<u32>,
    /// Sequence numbers reported by untagged `EXPUNGE`. Servers that have
    /// QRESYNC enabled send `VANISHED` instead (RFC 6851 section 4.4), so a
    /// delete proved by counting these alone reports failure on success.
    pub expunged_seqs: Vec<u32>,
    /// UIDs named by untagged `FETCH` responses.
    pub fetched_uids: Vec<u32>,
    /// `HIGHESTMODSEQ`, from an untagged `OK` during `SELECT`.
    pub highest_modseq: Option<u64>,
    /// UIDVALIDITY, from an untagged `OK` during `SELECT`.
    pub uid_validity: Option<u32>,
    /// UIDNEXT, from an untagged `OK` during `SELECT`.
    pub uid_next: Option<u32>,
}

impl RawExchange {
    /// UIDs the server refused to update because they changed since the
    /// client's mod-sequence (RFC 7162 section 3.1.3).
    ///
    /// `Some(vec![])` cannot occur: a `MODIFIED` code always names at least
    /// one UID, so `None` means "no conflict reported" and `Some` means "these
    /// specific UIDs conflicted".
    pub fn modified_uids(&self) -> Option<Vec<u32>> {
        self.information
            .as_deref()
            .and_then(parse_modified_response_code)
    }
}

/// Recover `[MODIFIED <uid-set>]` from a tagged response's text.
///
/// See the module docs for why this is a string scan: `imap-proto` has no
/// `ResponseCode` variant for it, and its `opt(resp_text_code)` leaves the
/// unrecognised code in the free-text field rather than failing.
fn parse_modified_response_code(information: &str) -> Option<Vec<u32>> {
    let start = information.find("[MODIFIED ")?;
    let rest = &information[start + "[MODIFIED ".len()..];
    let end = rest.find(']')?;
    let uids = expand_uid_set_text(&rest[..end]);
    if uids.is_empty() {
        None
    } else {
        Some(uids)
    }
}

/// Expand an IMAP `uid-set` in its wire text form (`7,9`, `1:4`, `1:3,7`).
fn expand_uid_set_text(spec: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((lo, hi)) = part.split_once(':') {
            if let (Ok(lo), Ok(hi)) = (lo.trim().parse::<u32>(), hi.trim().parse::<u32>()) {
                let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
                out.extend(lo..=hi);
            }
        } else if let Ok(uid) = part.parse::<u32>() {
            out.push(uid);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Flatten `imap-proto`'s uid-set representation into concrete UIDs.
fn expand_uid_set_members(members: &[UidSetMember]) -> Vec<u32> {
    let mut out = Vec::new();
    for member in members {
        match member {
            UidSetMember::Uid(uid) => out.push(*uid),
            UidSetMember::UidRange(range) => out.extend(range.clone()),
        }
    }
    out
}

/// Issue `command` and read responses until its tagged completion, keeping
/// everything that arrived on the way.
///
/// This is the whole point of the module. `run_command_and_check_ok` reads to
/// the same place and throws away the untagged responses *and* the tagged
/// response code, which between them carry every fact about whether the
/// command did anything.
pub async fn run_collected<S>(
    session: &mut Session<S>,
    command: &str,
) -> Result<RawExchange, MailError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    let tag = session
        .run_command(command)
        .await
        .map_err(|e| MailError::ImapError(format!("failed to issue '{command}': {e}")))?;

    let mut exchange = RawExchange::default();

    loop {
        let response = tokio::time::timeout(RESPONSE_READ_TIMEOUT, session.read_response())
            .await
            .map_err(|_| {
                MailError::FetchStalled(format!(
                    "no response to '{command}' within {RESPONSE_READ_TIMEOUT:?}"
                ))
            })?
            .map_err(|e| MailError::ImapError(format!("reading response to '{command}': {e}")))?
            .ok_or_else(|| {
                MailError::ImapError(format!("connection closed before '{command}' completed"))
            })?;

        let mut done = false;
        match response.parsed() {
            Response::Done {
                tag: response_tag,
                status,
                code,
                information,
            } => {
                // Only our own tag ends the exchange; another command's
                // completion interleaved here would otherwise truncate ours.
                if response_tag == &tag {
                    exchange.ok = matches!(status, Status::Ok);
                    exchange.information = information.as_ref().map(|i| i.to_string());
                    absorb_code(&mut exchange, code.as_ref());
                    done = true;
                }
            }
            Response::Data { code, .. } => absorb_code(&mut exchange, code.as_ref()),
            Response::Vanished { uids, .. } => {
                for range in uids {
                    exchange.vanished_uids.extend(range.clone());
                }
            }
            Response::Expunge(seq) => exchange.expunged_seqs.push(*seq),
            Response::Fetch(_, attributes) => {
                for attribute in attributes {
                    if let async_imap::imap_proto::AttributeValue::Uid(uid) = attribute {
                        exchange.fetched_uids.push(*uid);
                    }
                }
            }
            _ => {}
        }

        if done {
            break;
        }
    }

    Ok(exchange)
}

/// Fold a response code into the exchange, from a tagged or untagged response.
fn absorb_code(exchange: &mut RawExchange, code: Option<&ResponseCode<'_>>) {
    match code {
        Some(ResponseCode::CopyUid(uid_validity, source, destination)) => {
            exchange.copy_uid = Some(CopyUid {
                uid_validity: *uid_validity,
                source_uids: expand_uid_set_members(source),
                destination_uids: expand_uid_set_members(destination),
            });
        }
        Some(ResponseCode::HighestModSeq(modseq)) => exchange.highest_modseq = Some(*modseq),
        Some(ResponseCode::UidValidity(validity)) => exchange.uid_validity = Some(*validity),
        Some(ResponseCode::UidNext(uid_next)) => exchange.uid_next = Some(*uid_next),
        _ => {}
    }
}

/// Issue `ENABLE` for `capabilities` (RFC 5161).
///
/// Returns whether the server accepted the command. It deliberately does not
/// try to confirm *which* capabilities were enabled: the untagged `* ENABLED`
/// line has no `imap-proto` representation, and confirming it is unnecessary
/// -- RFC 7162 section 3.2.3 requires a server to answer `BAD` to a QRESYNC
/// `SELECT` that was never enabled, so a QRESYNC `SELECT` that succeeds is
/// itself the proof.
pub async fn enable<S>(session: &mut Session<S>, capabilities: &[&str]) -> Result<bool, MailError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    if capabilities.is_empty() {
        return Ok(true);
    }
    let exchange = run_collected(session, &format!("ENABLE {}", capabilities.join(" "))).await?;
    Ok(exchange.ok)
}

/// `SELECT <mailbox> (QRESYNC (<uidvalidity> <modseq>))` (RFC 7162 section 3.2.5).
///
/// The returned exchange carries the `VANISHED` set and the changed `FETCH`
/// UIDs the server volunteered, which together are the entire reason to use
/// QRESYNC: they are how a client learns what disappeared while it was away.
/// A plain `SELECT` can never report that.
///
/// `known_uids` is optional; omitting it makes the server behave as if the
/// client knew `1:<UIDNEXT-1>` (section 3.2.5.1), which is correct but can
/// make the `VANISHED` set much larger than it needs to be.
pub async fn select_qresync<S>(
    session: &mut Session<S>,
    mailbox: &str,
    uid_validity: u32,
    modseq: u64,
    known_uids: Option<&str>,
) -> Result<RawExchange, MailError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    let command = match known_uids {
        Some(uids) => format!("SELECT {mailbox} (QRESYNC ({uid_validity} {modseq} {uids}))"),
        None => format!("SELECT {mailbox} (QRESYNC ({uid_validity} {modseq}))"),
    };
    let exchange = run_collected(session, &command).await?;
    if !exchange.ok {
        return Err(MailError::ImapError(format!(
            "QRESYNC SELECT of '{mailbox}' was refused: {}",
            exchange.information.as_deref().unwrap_or("no detail")
        )));
    }
    Ok(exchange)
}

/// `UID STORE <set> (UNCHANGEDSINCE <modseq>) <op> (<flag>)` (RFC 7162 section 3.1.3).
///
/// The compare-and-swap that turns a lost update into a detected one. Read the
/// result with [`RawExchange::modified_uids`]: `None` means every targeted UID
/// was updated, `Some(uids)` means those were left untouched because another
/// client changed them first.
///
/// `.SILENT` is appended because the untagged `FETCH` responses a non-silent
/// store produces are not the proof of effect they look like -- RFC 3501
/// section 6.4.6 lets a server suppress them, and a no-op store legitimately
/// produces none. `MODIFIED` is the signal that matters.
pub async fn uid_store_unchanged_since<S>(
    session: &mut Session<S>,
    uid_set: &str,
    modseq: u64,
    op: &str,
    flag: &str,
) -> Result<RawExchange, MailError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    let command = format!("UID STORE {uid_set} (UNCHANGEDSINCE {modseq}) {op}.SILENT ({flag})");
    let exchange = run_collected(session, &command).await?;
    // A tagged NO/BAD is a genuine failure; a tagged OK carrying MODIFIED is a
    // conflict, which is a successful command with a reportable outcome.
    if !exchange.ok && exchange.modified_uids().is_none() {
        return Err(MailError::ImapError(format!(
            "conditional UID STORE on '{uid_set}' failed: {}",
            exchange.information.as_deref().unwrap_or("no detail")
        )));
    }
    Ok(exchange)
}

/// `UID MOVE <set> <destination>` (RFC 6851), collecting the `COPYUID` proof.
///
/// Check [`RawExchange::copy_uid`]: present means the move genuinely happened
/// and names the destination UIDs; **absent means unknown, not failed**. A UID
/// set matching nothing is a successful no-op per RFC 3501 section 6.4.8, and
/// a conforming server may also omit `COPYUID` for a `UIDNOTSTICKY`
/// destination.
pub async fn uid_move_collected<S>(
    session: &mut Session<S>,
    uid_set: &str,
    destination: &str,
) -> Result<RawExchange, MailError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    let exchange = run_collected(session, &format!("UID MOVE {uid_set} {destination}")).await?;
    if !exchange.ok {
        return Err(MailError::ImapError(format!(
            "UID MOVE of '{uid_set}' to '{destination}' failed: {}",
            exchange.information.as_deref().unwrap_or("no detail")
        )));
    }
    Ok(exchange)
}

/// `UID EXPUNGE <set>` (RFC 4315), collecting both removal reports.
///
/// A server with QRESYNC enabled answers with `VANISHED` rather than
/// `EXPUNGE` (RFC 6851 section 4.4), so both are gathered and callers should
/// treat either as evidence. Counting only `EXPUNGE` reports failure on every
/// successful delete against a QRESYNC server.
pub async fn uid_expunge_collected<S>(
    session: &mut Session<S>,
    uid_set: &str,
) -> Result<RawExchange, MailError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    let exchange = run_collected(session, &format!("UID EXPUNGE {uid_set}")).await?;
    if !exchange.ok {
        return Err(MailError::ImapError(format!(
            "UID EXPUNGE of '{uid_set}' failed: {}",
            exchange.information.as_deref().unwrap_or("no detail")
        )));
    }
    Ok(exchange)
}

/// Upper bound on the bytes one raw exchange will accumulate before giving up.
///
/// The raw reader has no framing beyond "a line beginning with our tag", so a
/// server that answers with an endless stream and never completes the command
/// would otherwise grow this buffer without limit. One `EMAILID` line is a few
/// dozen bytes and a batch is capped at a few hundred messages, so this is
/// orders of magnitude above any legitimate reply.
const MAX_RAW_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Tag prefix for commands this module writes straight to the transport.
///
/// `async-imap` allocates its own tags from an `IdGenerator` that emits
/// `format!("A{:04}", n % 10_000)` -- always the letter `A` followed by four
/// decimal digits. A tag starting with `NX` therefore cannot collide with one
/// the typed client will ever issue, no matter how many commands a session
/// runs or how often the generator wraps. Colliding would be worse than
/// untidy: two commands sharing a tag makes each one's completion look like
/// the other's, and a reader would stop at the wrong response.
const RAW_TAG_PREFIX: &str = "NX";

/// Sequence behind [`RAW_TAG_PREFIX`]. Process-wide rather than per-session
/// because it costs nothing and makes a tag unique in a packet capture across
/// every connection, which is the only thing anyone reads these tags for.
static RAW_TAG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_raw_tag() -> String {
    let n = RAW_TAG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{RAW_TAG_PREFIX}{n}")
}

/// `UID FETCH <set> (UID EMAILID)` (RFC 8474 section 5.2), read straight off
/// the transport, returning `uid -> EMAILID`.
///
/// # Why this bypasses the client entirely
///
/// `imap-proto` 0.16.7 cannot parse `EMAILID`: its `msg_att` parser is a closed
/// `alt(...)` of known attributes with no catch-all, so an `EMAILID` item is a
/// hard nom error rather than an unknown-but-tolerated one. `async-imap`'s
/// decoder turns any non-`Incomplete` parse error into a fatal `io::Error` and
/// then latches `read_closed`, after which the stream yields `None` forever --
/// the session is dead, not degraded. So `EMAILID` must never reach
/// `parse_response`, which rules out `Session::uid_fetch` **and**
/// [`run_collected`], since the latter reads through `Session::read_response`
/// and therefore through the same decoder.
///
/// What is left is the transport underneath: `Session` exposes `&mut T`
/// (`AsMut`, and `Connection::get_mut` through `Deref`), so the command is
/// written and its reply scanned here, by hand, and no byte of it is offered
/// to nom.
///
/// # The two conditions this is only correct under
///
/// 1. **The client's read buffer must be empty.** `ImapStream` owns a buffer
///    that can hold bytes already pulled off the socket. Raw reads bypass it,
///    so anything sitting there is invisible to this scanner. Call this only
///    at a quiescent point -- immediately after a previous command's *tagged*
///    completion has been read -- and never interleaved with an in-flight
///    typed command or a live response stream.
/// 2. **The tag must not collide** with the client's own. See
///    [`RAW_TAG_PREFIX`].
///
/// # Degrading rather than failing
///
/// A tagged `NO`/`BAD` yields an empty map, not an error. The object id is an
/// identity *upgrade*: without it a message still syncs under the
/// `Message-ID`+content tier or the surrogate, so a server that advertises
/// `OBJECTID` and then refuses the fetch must cost the sync precision, never
/// the mail. A stalled read is different and does surface, because a server
/// that stopped answering mid-command has left the session unusable.
pub async fn uid_fetch_email_ids<S>(
    session: &mut Session<S>,
    uid_set: &str,
) -> Result<BTreeMap<u32, String>, MailError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static,
{
    if uid_set.is_empty() {
        return Ok(BTreeMap::new());
    }

    let tag = next_raw_tag();
    let command = format!("{tag} UID FETCH {uid_set} (UID EMAILID)\r\n");

    let stream: &mut S = session.as_mut();
    stream
        .write_all(command.as_bytes())
        .await
        .map_err(|e| MailError::ImapError(format!("failed to issue '{command:?}': {e}")))?;
    stream
        .flush()
        .await
        .map_err(|e| MailError::ImapError(format!("failed to flush '{command:?}': {e}")))?;

    let (lines, tagged) = read_until_tagged(stream, &tag).await?;

    if !tagged_is_ok(&tagged) {
        tracing::warn!(
            response = %tagged,
            "server refused UID FETCH (UID EMAILID) despite advertising OBJECTID; \
             this pass identifies its messages by a weaker tier"
        );
        return Ok(BTreeMap::new());
    }

    let mut ids = BTreeMap::new();
    for line in &lines {
        if let Some((uid, email_id)) = parse_email_id_line(line) {
            ids.insert(uid, email_id);
        }
    }
    Ok(ids)
}

/// Read complete CRLF-terminated lines until one begins with `tag`, returning
/// the lines before it and the tagged line itself.
///
/// Line-oriented scanning is sound *for this command only*: an `EMAILID`
/// response carries no literal (`{n}`) section, so no line body can contain an
/// embedded CRLF. It must not be reused for a command whose reply can carry
/// literals -- a body fetch -- without adding literal handling.
async fn read_until_tagged<S>(stream: &mut S, tag: &str) -> Result<(Vec<String>, String), MailError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut scanned = 0usize;
    let mut lines: Vec<String> = Vec::new();
    let tag_prefix = format!("{tag} ");

    loop {
        let read = tokio::time::timeout(RESPONSE_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| {
                MailError::FetchStalled(format!(
                    "no response to raw '{tag}' within {RESPONSE_READ_TIMEOUT:?}"
                ))
            })?
            .map_err(|e| MailError::ImapError(format!("reading response to raw '{tag}': {e}")))?;
        if read == 0 {
            return Err(MailError::ImapError(format!(
                "connection closed before raw '{tag}' completed"
            )));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_RAW_RESPONSE_BYTES {
            return Err(MailError::ImapError(format!(
                "raw '{tag}' response exceeded {MAX_RAW_RESPONSE_BYTES} bytes without completing"
            )));
        }

        // Consume every line that is now complete. `scanned` marks how far the
        // buffer has already been split, so a reply arriving in many chunks is
        // not re-scanned from the start each time.
        while let Some(offset) = buffer[scanned..].iter().position(|b| *b == b'\n') {
            let end = scanned + offset;
            let line = String::from_utf8_lossy(&buffer[scanned..end])
                .trim_end_matches('\r')
                .to_string();
            scanned = end + 1;
            if line.starts_with(&tag_prefix) {
                return Ok((lines, line));
            }
            lines.push(line);
        }
    }
}

/// Whether a tagged completion line reports `OK`.
fn tagged_is_ok(tagged: &str) -> bool {
    tagged
        .split_whitespace()
        .nth(1)
        .is_some_and(|status| status.eq_ignore_ascii_case("OK"))
}

/// Pull `(uid, EMAILID)` out of one untagged `FETCH` line.
///
/// Both wire spellings are accepted. RFC 8474 section 5.2 defines the response
/// as `"EMAILID" SP "(" objectid ")"`, but the bare `EMAILID <objectid>` form
/// is what a hand-rolled scanner meets often enough that rejecting it would
/// mean silently dropping to a weaker identity tier against a server that
/// answered the question correctly enough.
///
/// Returns `None` for anything that is not an untagged `FETCH` carrying both a
/// UID and a non-empty object id -- there is nothing to key on without both.
fn parse_email_id_line(line: &str) -> Option<(u32, String)> {
    let upper = line.to_ascii_uppercase();
    if !upper.starts_with("* ") || !upper.contains(" FETCH ") {
        return None;
    }
    // `to_ascii_uppercase` is byte-length preserving, so offsets found in the
    // uppercased copy index the original exactly. Item *names* are
    // case-insensitive (RFC 3501 section 9); an object id is not, so the value
    // is always taken from the original line.
    let uid = find_item_value(&upper, line, "UID ")?.parse::<u32>().ok()?;
    let email_id = find_item_value(&upper, line, "EMAILID ")?;
    let email_id = email_id
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim_matches('"')
        .to_string();
    if email_id.is_empty() {
        return None;
    }
    Some((uid, email_id))
}

/// The token following FETCH item `name` in `line`, located via its uppercased
/// twin `upper`.
///
/// The name must sit on a token boundary, so `UID ` never matches the tail of
/// another attribute name. A parenthesised value is returned whole, brackets
/// included, for the caller to strip.
fn find_item_value(upper: &str, line: &str, name: &str) -> Option<String> {
    let mut from = 0usize;
    loop {
        let at = from + upper[from..].find(name)?;
        let boundary = at == 0
            || upper[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c == '(' || c == ' ');
        if boundary {
            let value_start = at + name.len();
            let rest = &line[value_start..];
            let end = if rest.starts_with('(') {
                rest.find(')').map_or(rest.len(), |i| i + 1)
            } else {
                rest.find([' ', ')']).unwrap_or(rest.len())
            };
            let value = rest[..end].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        from = at + name.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_id_is_read_from_both_wire_spellings() {
        // RFC 8474 5.2 parenthesises the id; the bare form appears in the wild.
        // Both must key the same message, or which spelling a server chose
        // would decide whether identity converges.
        assert_eq!(
            parse_email_id_line("* 1 FETCH (UID 7 EMAILID (M00000001))"),
            Some((7, "M00000001".to_string()))
        );
        assert_eq!(
            parse_email_id_line("* 1 FETCH (UID 7 EMAILID M00000001)"),
            Some((7, "M00000001".to_string()))
        );
        // Item names are case-insensitive; the id itself is not.
        assert_eq!(
            parse_email_id_line("* 1 fetch (uid 7 emailid (Mixed-Case_01))"),
            Some((7, "Mixed-Case_01".to_string()))
        );
        // Order is not fixed, and other attributes may sit between them.
        assert_eq!(
            parse_email_id_line("* 3 FETCH (EMAILID (M9) FLAGS (\\Seen) UID 12)"),
            Some((12, "M9".to_string()))
        );
    }

    #[test]
    fn a_line_missing_either_half_yields_nothing() {
        // Half an answer is not a weaker identity, it is no identity: without
        // the UID there is nothing to attach the id to, and without the id
        // there is nothing to attach.
        assert_eq!(
            parse_email_id_line("* 1 FETCH (UID 7 FLAGS (\\Seen))"),
            None
        );
        assert_eq!(parse_email_id_line("* 1 FETCH (EMAILID (M1))"), None);
        assert_eq!(parse_email_id_line("* 1 FETCH (UID 7 EMAILID ())"), None);
        assert_eq!(parse_email_id_line("NX0 OK UID FETCH completed"), None);
        assert_eq!(parse_email_id_line("* 4 EXPUNGE"), None);
    }

    #[test]
    fn an_item_name_is_only_matched_on_a_token_boundary() {
        // RFC 8474 defines THREADID alongside EMAILID, so a line carrying both
        // is ordinary. Every occurrence of `ID ` in it is the tail of a longer
        // attribute name; without the boundary rule a search would read
        // THREADID's value as the item's own and key the message on it.
        let line = "* 1 FETCH (THREADID (T1) EMAILID (M1) UID 7)";
        let upper = line.to_ascii_uppercase();
        assert_eq!(find_item_value(&upper, line, "ID "), None);
        assert_eq!(
            find_item_value(&upper, line, "EMAILID "),
            Some("(M1)".to_string())
        );
        assert_eq!(find_item_value(&upper, line, "UID "), Some("7".to_string()));
        assert_eq!(parse_email_id_line(line), Some((7, "M1".to_string())));
    }

    #[test]
    fn only_a_tagged_ok_counts_as_a_usable_answer() {
        assert!(tagged_is_ok("NX7 OK UID FETCH completed"));
        assert!(tagged_is_ok("NX7 ok uid fetch completed"));
        assert!(!tagged_is_ok("NX7 NO server does not do that"));
        assert!(!tagged_is_ok("NX7 BAD unknown item"));
        assert!(!tagged_is_ok("NX7"));
    }

    #[test]
    fn raw_tags_can_never_collide_with_the_clients_own() {
        // async-imap's IdGenerator emits `A` + four digits and wraps at 10_000,
        // so a shared tag is only impossible if the prefix differs.
        let tag = next_raw_tag();
        assert!(tag.starts_with(RAW_TAG_PREFIX));
        assert_ne!(&tag[..1], "A");
        assert_ne!(next_raw_tag(), tag, "each raw command gets its own tag");
    }

    #[test]
    fn modified_response_code_is_recovered_from_the_tagged_text() {
        // imap-proto has no ResponseCode::Modified, and its opt(resp_text_code)
        // leaves the unrecognised code in the free-text field. This is the
        // contract that behaviour gives us; if a future imap-proto starts
        // parsing MODIFIED, this test is what will notice.
        let exchange = RawExchange {
            information: Some("[MODIFIED 7,9] Conditional STORE failed".to_string()),
            ..RawExchange::default()
        };
        assert_eq!(exchange.modified_uids(), Some(vec![7, 9]));
    }

    #[test]
    fn modified_parses_ranges_and_reports_none_when_absent() {
        assert_eq!(
            parse_modified_response_code("[MODIFIED 1:3,7] failed"),
            Some(vec![1, 2, 3, 7])
        );
        assert_eq!(
            parse_modified_response_code("Conditional STORE failed"),
            None
        );
        assert_eq!(parse_modified_response_code("[MODIFIED ] weird"), None);
        assert_eq!(
            parse_modified_response_code("[MODIFIED 7 unterminated"),
            None
        );
    }

    #[test]
    fn no_modified_means_no_conflict_not_an_empty_conflict() {
        // The distinction matters: `Some(vec![])` would read as "a conflict
        // affecting nothing", which is not a state the protocol can produce.
        let exchange = RawExchange {
            information: Some("STORE completed".to_string()),
            ..RawExchange::default()
        };
        assert_eq!(exchange.modified_uids(), None);
    }

    #[test]
    fn uid_sets_expand_from_both_wire_forms() {
        assert_eq!(expand_uid_set_text("5"), vec![5]);
        assert_eq!(expand_uid_set_text("1:4"), vec![1, 2, 3, 4]);
        assert_eq!(expand_uid_set_text("9,1:3"), vec![1, 2, 3, 9]);
        // Reversed ranges are normalized rather than yielding nothing.
        assert_eq!(expand_uid_set_text("4:2"), vec![2, 3, 4]);

        assert_eq!(
            expand_uid_set_members(&[UidSetMember::Uid(3), UidSetMember::UidRange(7..=9)]),
            vec![3, 7, 8, 9]
        );
    }
}
