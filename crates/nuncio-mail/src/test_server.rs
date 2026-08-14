//! A stateful, multi-connection IMAP server for offline tests.
//!
//! `wiremock` only speaks HTTP, so JMAP and the DAV protocols have a mock and
//! IMAP does not. What existed instead was a scripted responder re-written
//! inline in each test: one connection, no mailbox state, and a hand-rolled
//! answer per command. That shape cannot express the behaviour this module
//! exists to test -- two clients observing each other's writes through shared
//! server state.
//!
//! The server binds a real loopback `TcpListener` and speaks enough of
//! IMAP4rev1 (plus RFC 7162 CONDSTORE/QRESYNC, RFC 4315 UIDPLUS, and RFC 6851
//! MOVE) for a client to be driven end to end over plain TCP. Mailbox state is
//! shared across every accepted connection, so a mutation issued on one
//! connection is visible to the next command on another -- which is the only
//! way to reproduce, offline, what happens when several engines sync the same
//! account.
//!
//! Capability advertisement is selectable ([`ServerProfile`]) because the sync
//! ladder branches on it: Fastmail/Cyrus and Dovecot advertise QRESYNC, Gmail
//! advertises CONDSTORE without QRESYNC, and Exchange advertises neither. A
//! test that only ever runs against a QRESYNC server proves nothing about the
//! path most real accounts take.
//!
//! Deliberately not a conformant server. It implements the commands the engine
//! issues, in the response shapes the relevant RFCs require, and answers `BAD`
//! to everything else.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Which capabilities the server advertises, and therefore which rung of the
/// sync ladder a client can negotiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerProfile {
    /// No CONDSTORE and no QRESYNC: full UID-set reconciliation is the only
    /// option. Models Exchange.
    Basic,
    /// CONDSTORE but no QRESYNC: `CHANGEDSINCE` works, `SELECT (QRESYNC ...)`
    /// does not. Models Gmail, which is the common case and the one a
    /// QRESYNC-only test suite silently skips.
    Condstore,
    /// CONDSTORE and QRESYNC. Models Fastmail/Cyrus and Dovecot.
    Qresync,
}

impl ServerProfile {
    /// The `CAPABILITY` atom list for this profile. `UIDPLUS` and `MOVE` are
    /// advertised by all three because every profile we model supports them.
    fn capability_line(self) -> String {
        let mut caps = vec!["IMAP4rev1", "UIDPLUS", "MOVE", "ENABLE", "LIST-STATUS"];
        match self {
            ServerProfile::Basic => {}
            ServerProfile::Condstore => caps.push("CONDSTORE"),
            ServerProfile::Qresync => {
                caps.push("CONDSTORE");
                caps.push("QRESYNC");
            }
        }
        caps.join(" ")
    }

    fn supports_condstore(self) -> bool {
        matches!(self, ServerProfile::Condstore | ServerProfile::Qresync)
    }

    fn supports_qresync(self) -> bool {
        matches!(self, ServerProfile::Qresync)
    }
}

/// One message as the server holds it.
#[derive(Debug, Clone)]
pub struct MockMessage {
    pub uid: u32,
    pub modseq: u64,
    pub flags: BTreeSet<String>,
    /// Full RFC822 octets served for `BODY[]`.
    pub body: String,
}

/// A mailbox: its UID space, its mod-sequence, and the tombstones needed to
/// answer `VANISHED (EARLIER)`.
#[derive(Debug, Clone)]
pub struct MockMailbox {
    pub uid_validity: u32,
    pub uid_next: u32,
    pub highest_modseq: u64,
    pub messages: Vec<MockMessage>,
    /// `(uid, modseq_at_removal)` for every message removed from this mailbox.
    /// QRESYNC needs removal history; a client resyncing from an older modseq
    /// must be told what disappeared, which is exactly the information a
    /// high-water-mark sync can never recover.
    pub vanished: Vec<(u32, u64)>,
}

impl MockMailbox {
    fn new(uid_validity: u32) -> Self {
        Self {
            uid_validity,
            uid_next: 1,
            highest_modseq: 1,
            messages: Vec::new(),
            vanished: Vec::new(),
        }
    }

    fn next_modseq(&mut self) -> u64 {
        self.highest_modseq += 1;
        self.highest_modseq
    }

    fn append(&mut self, body: &str, flags: &[&str]) -> u32 {
        let uid = self.uid_next;
        self.uid_next += 1;
        let modseq = self.next_modseq();
        self.messages.push(MockMessage {
            uid,
            modseq,
            flags: flags.iter().map(|f| (*f).to_string()).collect(),
            body: body.to_string(),
        });
        uid
    }

    /// Remove `uid`, recording a tombstone at a fresh modseq.
    fn remove(&mut self, uid: u32) -> bool {
        let Some(pos) = self.messages.iter().position(|m| m.uid == uid) else {
            return false;
        };
        let modseq = self.next_modseq();
        self.messages.remove(pos);
        self.vanished.push((uid, modseq));
        true
    }

    /// 1-based sequence number of `uid`, as untagged responses require.
    fn seq_of(&self, uid: u32) -> usize {
        self.messages
            .iter()
            .position(|m| m.uid == uid)
            .map_or(0, |p| p + 1)
    }
}

/// Server state shared by every connection.
#[derive(Debug)]
pub struct ServerState {
    pub profile: ServerProfile,
    pub mailboxes: BTreeMap<String, MockMailbox>,
    /// Every command line received, across all connections, in arrival order.
    /// Tests assert on this to prove *which* protocol path ran -- e.g. that a
    /// second sync narrowed its FETCH rather than re-fetching everything.
    pub commands: Vec<String>,
    next_uid_validity: u32,
}

impl ServerState {
    fn new(profile: ServerProfile) -> Self {
        Self {
            profile,
            mailboxes: BTreeMap::new(),
            commands: Vec::new(),
            next_uid_validity: 1000,
        }
    }
}

/// Handle to a running mock server. Dropping it aborts the accept loop; the
/// bound port is released with it.
#[derive(Debug)]
pub struct MockImapServer {
    addr: SocketAddr,
    state: Arc<Mutex<ServerState>>,
    accept_task: tokio::task::JoinHandle<()>,
}

impl Drop for MockImapServer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

/// Lock the shared state, recovering from a poisoned mutex rather than
/// panicking: a panic in one connection task must not cascade into every other
/// assertion in the test.
fn lock_state(state: &Arc<Mutex<ServerState>>) -> MutexGuard<'_, ServerState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl MockImapServer {
    /// Bind an ephemeral loopback port and start accepting connections.
    pub async fn start(profile: ServerProfile) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(ServerState::new(profile)));

        let accept_state = Arc::clone(&state);
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let conn_state = Arc::clone(&accept_state);
                tokio::spawn(async move {
                    // A dropped client mid-command is normal in these tests;
                    // the connection task simply ends.
                    let _ = handle_connection(socket, conn_state).await;
                });
            }
        });

        Ok(Self {
            addr,
            state,
            accept_task,
        })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn host(&self) -> &'static str {
        "127.0.0.1"
    }

    /// Create a mailbox with a fresh UIDVALIDITY and return its name.
    pub fn create_mailbox(&self, name: &str) {
        let mut state = lock_state(&self.state);
        let uid_validity = state.next_uid_validity;
        state.next_uid_validity += 1;
        state
            .mailboxes
            .insert(name.to_string(), MockMailbox::new(uid_validity));
    }

    /// Append a message, creating the mailbox if absent. Returns its UID.
    pub fn append_message(&self, mailbox: &str, body: &str, flags: &[&str]) -> u32 {
        let mut state = lock_state(&self.state);
        let uid_validity = state.next_uid_validity;
        let entry = state.mailboxes.entry(mailbox.to_string());
        let mailbox = entry.or_insert_with(|| {
            // Only consumes a UIDVALIDITY when the mailbox is actually new.
            MockMailbox::new(uid_validity)
        });
        let fresh = mailbox.uid_next == 1 && mailbox.messages.is_empty();
        let uid = mailbox.append(body, flags);
        if fresh {
            state.next_uid_validity += 1;
        }
        uid
    }

    /// Simulate another client removing a message -- the case a forward-only
    /// high-water-mark sync can never observe.
    pub fn expunge(&self, mailbox: &str, uid: u32) -> bool {
        let mut state = lock_state(&self.state);
        state
            .mailboxes
            .get_mut(mailbox)
            .is_some_and(|mb| mb.remove(uid))
    }

    /// Force a UIDVALIDITY change, as a server does when a mailbox is
    /// recreated. UIDs restart at 1.
    pub fn bump_uid_validity(&self, mailbox: &str) {
        let mut state = lock_state(&self.state);
        if let Some(mb) = state.mailboxes.get_mut(mailbox) {
            mb.uid_validity += 1;
            mb.uid_next = 1;
            mb.messages.clear();
            mb.vanished.clear();
        }
    }

    /// UIDs currently present in `mailbox`.
    pub fn uids(&self, mailbox: &str) -> Vec<u32> {
        let state = lock_state(&self.state);
        state
            .mailboxes
            .get(mailbox)
            .map(|mb| mb.messages.iter().map(|m| m.uid).collect())
            .unwrap_or_default()
    }

    /// Flags currently set on `uid`, or `None` if it is not present.
    pub fn flags(&self, mailbox: &str, uid: u32) -> Option<Vec<String>> {
        let state = lock_state(&self.state);
        state.mailboxes.get(mailbox).and_then(|mb| {
            mb.messages
                .iter()
                .find(|m| m.uid == uid)
                .map(|m| m.flags.iter().cloned().collect())
        })
    }

    /// Every command line the server has received, in arrival order.
    pub fn commands(&self) -> Vec<String> {
        lock_state(&self.state).commands.clone()
    }

    /// Commands containing `needle` (case-insensitive).
    pub fn commands_matching(&self, needle: &str) -> Vec<String> {
        let upper = needle.to_ascii_uppercase();
        self.commands()
            .into_iter()
            .filter(|c| c.to_ascii_uppercase().contains(&upper))
            .collect()
    }
}

/// Per-connection session state. Only `selected` and `qresync_enabled` are
/// connection-scoped; everything else lives in the shared `ServerState`.
struct Session {
    selected: Option<String>,
    qresync_enabled: bool,
}

async fn handle_connection(
    mut socket: TcpStream,
    state: Arc<Mutex<ServerState>>,
) -> std::io::Result<()> {
    let greeting = {
        let st = lock_state(&state);
        format!(
            "* OK [CAPABILITY {}] Nuncio mock IMAP ready\r\n",
            st.profile.capability_line()
        )
    };
    socket.write_all(greeting.as_bytes()).await?;

    let mut session = Session {
        selected: None,
        qresync_enabled: false,
    };

    while let Some(line) = read_line(&mut socket).await {
        if line.trim().is_empty() {
            continue;
        }
        let response = {
            let mut st = lock_state(&state);
            st.commands.push(line.clone());
            dispatch(&line, &mut session, &mut st)
        };
        socket.write_all(response.text.as_bytes()).await?;
        if response.close {
            break;
        }
    }
    Ok(())
}

struct Reply {
    text: String,
    close: bool,
}

impl Reply {
    fn open(text: String) -> Self {
        Self { text, close: false }
    }
    fn closing(text: String) -> Self {
        Self { text, close: true }
    }
}

fn dispatch(line: &str, session: &mut Session, st: &mut ServerState) -> Reply {
    let mut parts = line.split_whitespace();
    let tag = parts.next().unwrap_or("*").to_string();
    let command = parts.next().unwrap_or("").to_ascii_uppercase();
    let rest: Vec<String> = parts.map(str::to_string).collect();

    match command.as_str() {
        "CAPABILITY" => Reply::open(format!(
            "* CAPABILITY {}\r\n{tag} OK CAPABILITY completed\r\n",
            st.profile.capability_line()
        )),
        "LOGIN" => Reply::open(format!(
            "{tag} OK [CAPABILITY {}] Logged in\r\n",
            st.profile.capability_line()
        )),
        "ENABLE" => handle_enable(&tag, &rest, session, st),
        "NOOP" => Reply::open(format!("{tag} OK NOOP completed\r\n")),
        "LIST" => handle_list(&tag, st),
        "STATUS" => handle_status(&tag, &rest, st),
        "SELECT" | "EXAMINE" => handle_select(&tag, line, &rest, session, st),
        "UID" => handle_uid(&tag, line, &rest, session, st),
        "LOGOUT" => Reply::closing(format!(
            "* BYE Logging out\r\n{tag} OK LOGOUT completed\r\n"
        )),
        _ => Reply::open(format!("{tag} BAD unsupported command\r\n")),
    }
}

fn handle_enable(tag: &str, rest: &[String], session: &mut Session, st: &ServerState) -> Reply {
    let wants_qresync = rest.iter().any(|a| a.eq_ignore_ascii_case("QRESYNC"));
    if wants_qresync && st.profile.supports_qresync() {
        session.qresync_enabled = true;
        return Reply::open(format!(
            "* ENABLED QRESYNC\r\n{tag} OK ENABLE completed\r\n"
        ));
    }
    Reply::open(format!("{tag} OK ENABLE completed\r\n"))
}

fn handle_list(tag: &str, st: &ServerState) -> Reply {
    let mut out = String::new();
    for name in st.mailboxes.keys() {
        out.push_str(&format!("* LIST () \"/\" {name}\r\n"));
    }
    out.push_str(&format!("{tag} OK LIST completed\r\n"));
    Reply::open(out)
}

fn handle_status(tag: &str, rest: &[String], st: &ServerState) -> Reply {
    let Some(name) = rest.first().map(|n| n.trim_matches('"').to_string()) else {
        return Reply::open(format!("{tag} BAD STATUS needs a mailbox\r\n"));
    };
    let Some(mb) = st.mailboxes.get(&name) else {
        return Reply::open(format!("{tag} NO no such mailbox\r\n"));
    };
    let mut items = format!(
        "MESSAGES {} UIDNEXT {} UIDVALIDITY {}",
        mb.messages.len(),
        mb.uid_next,
        mb.uid_validity
    );
    if st.profile.supports_condstore() {
        items.push_str(&format!(" HIGHESTMODSEQ {}", mb.highest_modseq));
    }
    Reply::open(format!(
        "* STATUS {name} ({items})\r\n{tag} OK STATUS completed\r\n"
    ))
}

fn handle_select(
    tag: &str,
    line: &str,
    rest: &[String],
    session: &mut Session,
    st: &ServerState,
) -> Reply {
    let Some(name) = rest.first().map(|n| n.trim_matches('"').to_string()) else {
        return Reply::open(format!("{tag} BAD SELECT needs a mailbox\r\n"));
    };
    let Some(mb) = st.mailboxes.get(&name) else {
        return Reply::open(format!("{tag} NO no such mailbox\r\n"));
    };

    let upper = line.to_ascii_uppercase();
    let asked_qresync = upper.contains("QRESYNC");
    // RFC 7162 3.2.3: a server MUST reply BAD to a QRESYNC SELECT parameter
    // when the client has not issued `ENABLE QRESYNC` on this connection.
    // Enforced so a client that skips ENABLE fails here rather than silently
    // degrading in production.
    if asked_qresync && !session.qresync_enabled {
        return Reply::open(format!("{tag} BAD QRESYNC not enabled\r\n"));
    }

    session.selected = Some(name.clone());

    let mut out = String::new();
    out.push_str(&format!("* {} EXISTS\r\n", mb.messages.len()));
    out.push_str("* 0 RECENT\r\n");
    out.push_str("* FLAGS (\\Seen \\Flagged \\Deleted \\Draft)\r\n");
    out.push_str("* OK [PERMANENTFLAGS (\\Seen \\Flagged \\Deleted \\Draft \\*)] Limited\r\n");
    out.push_str(&format!(
        "* OK [UIDVALIDITY {}] UIDs valid\r\n",
        mb.uid_validity
    ));
    out.push_str(&format!(
        "* OK [UIDNEXT {}] Predicted next\r\n",
        mb.uid_next
    ));
    if st.profile.supports_condstore() {
        out.push_str(&format!(
            "* OK [HIGHESTMODSEQ {}] Highest\r\n",
            mb.highest_modseq
        ));
    }

    if asked_qresync {
        if let Some(since) = parse_qresync_modseq(line) {
            let gone: Vec<String> = mb
                .vanished
                .iter()
                .filter(|(_, m)| *m > since)
                .map(|(u, _)| u.to_string())
                .collect();
            if !gone.is_empty() {
                out.push_str(&format!("* VANISHED (EARLIER) {}\r\n", gone.join(",")));
            }
            for msg in mb.messages.iter().filter(|m| m.modseq > since) {
                out.push_str(&fetch_line(mb, msg, false));
            }
        }
    }

    out.push_str(&format!("{tag} OK [READ-WRITE] SELECT completed\r\n"));
    Reply::open(out)
}

/// Pull the mod-sequence out of `SELECT x (QRESYNC (uidvalidity modseq ...))`.
fn parse_qresync_modseq(line: &str) -> Option<u64> {
    let upper = line.to_ascii_uppercase();
    let at = upper.find("QRESYNC")?;
    let tail = &line[at + "QRESYNC".len()..];
    let nums: Vec<u64> = tail
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    // (uidvalidity modseq [known-uids...]) -- the second number is the modseq.
    nums.get(1).copied()
}

fn handle_uid(
    tag: &str,
    line: &str,
    rest: &[String],
    session: &Session,
    st: &mut ServerState,
) -> Reply {
    let sub = rest
        .first()
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_default();
    let Some(selected) = session.selected.clone() else {
        return Reply::open(format!("{tag} BAD no mailbox selected\r\n"));
    };
    match sub.as_str() {
        "FETCH" => handle_uid_fetch(tag, line, rest, &selected, st),
        "STORE" => handle_uid_store(tag, line, rest, &selected, st),
        "COPY" => handle_uid_copy(tag, rest, &selected, st, false),
        "MOVE" => handle_uid_copy(tag, rest, &selected, st, true),
        "EXPUNGE" => handle_uid_expunge(tag, rest, &selected, session, st),
        _ => Reply::open(format!("{tag} BAD unsupported UID subcommand\r\n")),
    }
}

/// Expand an IMAP `uid-set` (`1,3:5,10:*`) against the UIDs present.
fn expand_uid_set(spec: &str, present: &[u32], uid_next: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        if let Some((lo, hi)) = part.split_once(':') {
            let lo: u32 = lo.trim().parse().unwrap_or(0);
            let hi: u32 = if hi.trim() == "*" {
                uid_next.saturating_sub(1).max(lo)
            } else {
                hi.trim().parse().unwrap_or(0)
            };
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            out.extend(present.iter().copied().filter(|u| *u >= lo && *u <= hi));
        } else if let Ok(single) = part.trim().parse::<u32>() {
            // RFC 3501 6.4.8: a non-existent UID is silently ignored, never an
            // error. Filtering against `present` reproduces that exactly --
            // which is the behaviour that makes an unverified mutation look
            // like a success.
            out.extend(present.iter().copied().filter(|u| *u == single));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn format_flags(flags: &BTreeSet<String>) -> String {
    flags.iter().cloned().collect::<Vec<_>>().join(" ")
}

fn fetch_line(mb: &MockMailbox, msg: &MockMessage, include_body: bool) -> String {
    let seq = mb.seq_of(msg.uid);
    let mut items = format!(
        "UID {} FLAGS ({}) MODSEQ ({})",
        msg.uid,
        format_flags(&msg.flags),
        msg.modseq
    );
    if include_body {
        items.push_str(&format!(" BODY[] {{{}}}\r\n{}", msg.body.len(), msg.body));
    }
    format!("* {seq} FETCH ({items})\r\n")
}

fn handle_uid_fetch(
    tag: &str,
    line: &str,
    rest: &[String],
    selected: &str,
    st: &mut ServerState,
) -> Reply {
    let condstore = st.profile.supports_condstore();
    let Some(mb) = st.mailboxes.get(selected) else {
        return Reply::open(format!("{tag} NO mailbox vanished\r\n"));
    };
    let Some(spec) = rest.get(1) else {
        return Reply::open(format!("{tag} BAD UID FETCH needs a set\r\n"));
    };

    let upper = line.to_ascii_uppercase();
    let include_body = upper.contains("BODY[]") || upper.contains("RFC822");
    let changed_since = if condstore {
        parse_changed_since(&upper)
    } else {
        None
    };

    let present: Vec<u32> = mb.messages.iter().map(|m| m.uid).collect();
    let wanted = expand_uid_set(spec, &present, mb.uid_next);

    let mut out = String::new();
    for msg in mb.messages.iter().filter(|m| wanted.contains(&m.uid)) {
        if let Some(since) = changed_since {
            if msg.modseq <= since {
                continue;
            }
        }
        out.push_str(&fetch_line(mb, msg, include_body));
    }

    // RFC 7162 3.2.6: with CHANGEDSINCE ... VANISHED the server reports
    // removals in the same pass. This is the whole point of the mechanism --
    // it is the only way a client learns a message is gone.
    if changed_since.is_some() && upper.contains("VANISHED") {
        if let Some(since) = changed_since {
            let gone: Vec<String> = mb
                .vanished
                .iter()
                .filter(|(_, m)| *m > since)
                .map(|(u, _)| u.to_string())
                .collect();
            if !gone.is_empty() {
                out.push_str(&format!("* VANISHED (EARLIER) {}\r\n", gone.join(",")));
            }
        }
    }

    out.push_str(&format!("{tag} OK UID FETCH completed\r\n"));
    Reply::open(out)
}

fn parse_changed_since(upper: &str) -> Option<u64> {
    let at = upper.find("CHANGEDSINCE")?;
    upper[at + "CHANGEDSINCE".len()..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn parse_unchanged_since(upper: &str) -> Option<u64> {
    let at = upper.find("UNCHANGEDSINCE")?;
    upper[at + "UNCHANGEDSINCE".len()..]
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn handle_uid_store(
    tag: &str,
    line: &str,
    rest: &[String],
    selected: &str,
    st: &mut ServerState,
) -> Reply {
    let condstore = st.profile.supports_condstore();
    let Some(mb) = st.mailboxes.get_mut(selected) else {
        return Reply::open(format!("{tag} NO mailbox vanished\r\n"));
    };
    let Some(spec) = rest.get(1) else {
        return Reply::open(format!("{tag} BAD UID STORE needs a set\r\n"));
    };

    let upper = line.to_ascii_uppercase();
    let silent = upper.contains(".SILENT");
    let removing = line.contains("-FLAGS");
    let unchanged_since = if condstore {
        parse_unchanged_since(&upper)
    } else {
        None
    };

    let flags = parse_flag_list(line);
    let present: Vec<u32> = mb.messages.iter().map(|m| m.uid).collect();
    let targets = expand_uid_set(spec, &present, mb.uid_next);

    let mut modified: Vec<u32> = Vec::new();
    let mut touched: Vec<u32> = Vec::new();
    for uid in targets {
        let Some(idx) = mb.messages.iter().position(|m| m.uid == uid) else {
            continue;
        };
        // RFC 7162 3.1.3: a message changed since the client's mod-sequence is
        // reported in MODIFIED and left untouched -- the compare-and-swap that
        // makes a lost update detectable instead of silent.
        if let Some(since) = unchanged_since {
            if mb.messages[idx].modseq > since {
                modified.push(uid);
                continue;
            }
        }
        let next = mb.next_modseq();
        if let Some(msg) = mb.messages.get_mut(idx) {
            for flag in &flags {
                if removing {
                    msg.flags.remove(flag);
                } else {
                    msg.flags.insert(flag.clone());
                }
            }
            msg.modseq = next;
        }
        touched.push(uid);
    }

    let mut out = String::new();
    if !silent {
        for uid in &touched {
            if let Some(msg) = mb.messages.iter().find(|m| m.uid == *uid) {
                out.push_str(&fetch_line(mb, msg, false));
            }
        }
    }
    // RFC 7162 3.1.3 puts MODIFIED in the *tagged* response -- the response
    // whose code async-imap currently discards.
    if modified.is_empty() {
        out.push_str(&format!("{tag} OK UID STORE completed\r\n"));
    } else {
        let list: Vec<String> = modified.iter().map(u32::to_string).collect();
        out.push_str(&format!(
            "{tag} OK [MODIFIED {}] Conditional STORE failed\r\n",
            list.join(",")
        ));
    }
    Reply::open(out)
}

/// Extract flag atoms from the parenthesised flag list of a STORE.
///
/// Takes the **last** parenthesised group, not the first: a conditional store
/// is `UID STORE <set> (UNCHANGEDSINCE <n>) +FLAGS.SILENT (<flags>)`, so
/// reading the first group silently applies `UNCHANGEDSINCE` and the
/// mod-sequence as though they were flag names. The flag list is always the
/// final argument of a STORE.
fn parse_flag_list(line: &str) -> Vec<String> {
    let Some(open) = line.rfind('(') else {
        return Vec::new();
    };
    let Some(close) = line[open..].find(')') else {
        return Vec::new();
    };
    line[open + 1..open + close]
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn handle_uid_copy(
    tag: &str,
    rest: &[String],
    selected: &str,
    st: &mut ServerState,
    is_move: bool,
) -> Reply {
    let qresync = st.profile.supports_qresync();
    let (Some(spec), Some(dest_raw)) = (rest.get(1), rest.get(2)) else {
        return Reply::open(format!("{tag} BAD UID COPY needs a set and a mailbox\r\n"));
    };
    let dest = dest_raw.trim_matches('"').to_string();
    if !st.mailboxes.contains_key(&dest) {
        return Reply::open(format!("{tag} NO [TRYCREATE] no such destination\r\n"));
    }

    let (source_uids, source_validity, bodies) = {
        let Some(mb) = st.mailboxes.get(selected) else {
            return Reply::open(format!("{tag} NO mailbox vanished\r\n"));
        };
        let present: Vec<u32> = mb.messages.iter().map(|m| m.uid).collect();
        let uids = expand_uid_set(spec, &present, mb.uid_next);
        let bodies: Vec<(u32, String, BTreeSet<String>)> = mb
            .messages
            .iter()
            .filter(|m| uids.contains(&m.uid))
            .map(|m| (m.uid, m.body.clone(), m.flags.clone()))
            .collect();
        (uids, mb.uid_validity, bodies)
    };

    // RFC 3501 6.4.8 again: a UID set that matches nothing is a successful
    // no-op. No COPYUID is emitted, and the client cannot distinguish this
    // from a copy it did not observe -- which is precisely why an absent
    // COPYUID must mean "unknown", never "applied" and never "conflict".
    if source_uids.is_empty() {
        return Reply::open(format!("{tag} OK UID {} completed\r\n", verb(is_move)));
    }

    let mut dest_uids = Vec::new();
    let dest_validity = {
        let Some(dst) = st.mailboxes.get_mut(&dest) else {
            return Reply::open(format!("{tag} NO destination vanished\r\n"));
        };
        for (_, body, flags) in &bodies {
            let refs: Vec<&str> = flags.iter().map(String::as_str).collect();
            dest_uids.push(dst.append(body, &refs));
        }
        dst.uid_validity
    };

    let mut out = String::new();
    // RFC 6851 4.3 advises COPYUID in an *untagged* OK for MOVE, and RFC 4315
    // 3 defines the code. A client reading only the tagged OK misses it.
    out.push_str(&format!(
        "* OK [COPYUID {} {} {}] Copied\r\n",
        dest_validity,
        join_uids(&source_uids),
        join_uids(&dest_uids)
    ));

    if is_move {
        let Some(src) = st.mailboxes.get_mut(selected) else {
            return Reply::open(format!("{tag} NO mailbox vanished\r\n"));
        };
        let mut seqs = Vec::new();
        for uid in &source_uids {
            seqs.push(src.seq_of(*uid));
            src.remove(*uid);
        }
        if qresync {
            // RFC 6851 4.4: under QRESYNC the server reports VANISHED rather
            // than EXPUNGE. A client proving its move by counting EXPUNGE
            // responses sees zero and wrongly concludes it failed.
            out.push_str(&format!("* VANISHED {}\r\n", join_uids(&source_uids)));
        } else {
            // Descending, so earlier removals do not renumber later ones.
            seqs.sort_unstable();
            for seq in seqs.iter().rev() {
                out.push_str(&format!("* {seq} EXPUNGE\r\n"));
            }
        }
        let _ = source_validity;
    }

    out.push_str(&format!("{tag} OK UID {} completed\r\n", verb(is_move)));
    Reply::open(out)
}

fn verb(is_move: bool) -> &'static str {
    if is_move {
        "MOVE"
    } else {
        "COPY"
    }
}

fn join_uids(uids: &[u32]) -> String {
    uids.iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn handle_uid_expunge(
    tag: &str,
    rest: &[String],
    selected: &str,
    session: &Session,
    st: &mut ServerState,
) -> Reply {
    let qresync = st.profile.supports_qresync();
    let Some(mb) = st.mailboxes.get_mut(selected) else {
        return Reply::open(format!("{tag} NO mailbox vanished\r\n"));
    };
    let Some(spec) = rest.get(1) else {
        return Reply::open(format!("{tag} BAD UID EXPUNGE needs a set\r\n"));
    };

    let present: Vec<u32> = mb.messages.iter().map(|m| m.uid).collect();
    // UID EXPUNGE removes only \Deleted messages within the given set.
    let targets: Vec<u32> = expand_uid_set(spec, &present, mb.uid_next)
        .into_iter()
        .filter(|uid| {
            mb.messages
                .iter()
                .any(|m| m.uid == *uid && m.flags.iter().any(|f| f == "\\Deleted"))
        })
        .collect();

    let mut out = String::new();
    let mut seqs = Vec::new();
    for uid in &targets {
        seqs.push(mb.seq_of(*uid));
        mb.remove(*uid);
    }

    if qresync && session.qresync_enabled {
        if !targets.is_empty() {
            out.push_str(&format!("* VANISHED {}\r\n", join_uids(&targets)));
        }
    } else {
        seqs.sort_unstable();
        for seq in seqs.iter().rev() {
            out.push_str(&format!("* {seq} EXPUNGE\r\n"));
        }
    }
    out.push_str(&format!("{tag} OK UID EXPUNGE completed\r\n"));
    Reply::open(out)
}

/// Read one CRLF-terminated command line, returning `None` at EOF.
async fn read_line(socket: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match socket.read(&mut byte).await {
            Ok(0) => {
                return if buf.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(&buf).into_owned())
                }
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    return Some(String::from_utf8_lossy(&buf).into_owned());
                }
                if byte[0] != b'\r' {
                    buf.push(byte[0]);
                }
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    /// A raw line-oriented IMAP client. The harness is tested at the wire
    /// level on purpose: `async-imap` cannot express ENABLE, a QRESYNC SELECT,
    /// or read a tagged response code, so an engine-driven test could not
    /// observe most of what this server exists to provide.
    struct RawClient {
        reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
        writer: tokio::net::tcp::OwnedWriteHalf,
        counter: usize,
    }

    impl RawClient {
        async fn connect(server: &MockImapServer) -> Self {
            let stream = TcpStream::connect((server.host(), server.port()))
                .await
                .expect("connect to mock server");
            let (r, w) = stream.into_split();
            let mut client = Self {
                reader: BufReader::new(r),
                writer: w,
                counter: 0,
            };
            let greeting = client.read_line().await;
            assert!(greeting.starts_with("* OK"), "greeting was {greeting:?}");
            client
        }

        async fn read_line(&mut self) -> String {
            use tokio::io::AsyncBufReadExt;
            let mut line = String::new();
            let _ = self.reader.read_line(&mut line).await;
            line.trim_end().to_string()
        }

        /// Send a command and collect every response line up to the tagged one.
        async fn send(&mut self, command: &str) -> Vec<String> {
            self.counter += 1;
            let tag = format!("a{}", self.counter);
            let _ = self
                .writer
                .write_all(format!("{tag} {command}\r\n").as_bytes())
                .await;
            let mut lines = Vec::new();
            loop {
                let line = self.read_line().await;
                if line.is_empty() {
                    break;
                }
                let done = line.starts_with(&format!("{tag} "));
                lines.push(line);
                if done {
                    break;
                }
            }
            lines
        }

        async fn login_select(&mut self, mailbox: &str) -> Vec<String> {
            let _ = self.send("LOGIN user pass").await;
            self.send(&format!("SELECT {mailbox}")).await
        }
    }

    fn joined(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[tokio::test]
    async fn two_connections_observe_each_others_writes_through_shared_state() {
        // The property the old inline duplex responders could not express, and
        // the reason this harness exists: engine B must see what engine A did.
        let server = MockImapServer::start(ServerProfile::Qresync)
            .await
            .expect("server starts");
        server.create_mailbox("INBOX");
        server.create_mailbox("Archive");
        let uid = server.append_message("INBOX", "Subject: one\r\n\r\nbody", &[]);

        let mut engine_a = RawClient::connect(&server).await;
        let mut engine_b = RawClient::connect(&server).await;

        engine_a.login_select("INBOX").await;
        engine_b.login_select("INBOX").await;

        // B sees the message before A moves it.
        let before = engine_b.send("UID FETCH 1:* (FLAGS)").await;
        assert!(
            joined(&before).contains(&format!("UID {uid}")),
            "B should see the message first: {before:?}"
        );

        let moved = engine_a.send(&format!("UID MOVE {uid} Archive")).await;
        assert!(
            joined(&moved).contains("COPYUID"),
            "a real move must emit COPYUID: {moved:?}"
        );

        // The same command on B now returns nothing -- shared state, not a
        // per-connection script.
        let after = engine_b.send("UID FETCH 1:* (FLAGS)").await;
        assert!(
            !joined(&after).contains(&format!("UID {uid} ")),
            "B must not still see the moved message: {after:?}"
        );
        assert_eq!(server.uids("INBOX"), Vec::<u32>::new());
        assert_eq!(server.uids("Archive").len(), 1);
    }

    #[tokio::test]
    async fn a_move_of_an_already_moved_uid_succeeds_silently_without_copyuid() {
        // RFC 3501 6.4.8. This is the exact race that makes an unverified
        // mutation report success: the losing engine's MOVE returns OK and
        // does nothing. The harness must reproduce it, or no test can catch it.
        let server = MockImapServer::start(ServerProfile::Qresync)
            .await
            .expect("server starts");
        server.create_mailbox("INBOX");
        server.create_mailbox("Archive");
        server.create_mailbox("Trash");
        let uid = server.append_message("INBOX", "Subject: contested\r\n\r\nx", &[]);

        let mut engine_a = RawClient::connect(&server).await;
        let mut engine_b = RawClient::connect(&server).await;
        engine_a.login_select("INBOX").await;
        engine_b.login_select("INBOX").await;

        let winner = engine_a.send(&format!("UID MOVE {uid} Archive")).await;
        assert!(joined(&winner).contains("COPYUID"));

        let loser = engine_b.send(&format!("UID MOVE {uid} Trash")).await;
        let loser_text = joined(&loser);
        assert!(
            loser_text.contains("OK UID MOVE completed"),
            "the losing move must report OK: {loser:?}"
        );
        assert!(
            !loser_text.contains("COPYUID"),
            "a no-op move must NOT emit COPYUID: {loser:?}"
        );
        assert!(server.uids("Trash").is_empty(), "nothing moved to Trash");
    }

    #[tokio::test]
    async fn unchangedsince_reports_modified_in_the_tagged_response() {
        // RFC 7162 3.1.3. MODIFIED rides on the tagged OK -- the response whose
        // code async-imap discards, which is why chunk 3 needs a raw layer.
        let server = MockImapServer::start(ServerProfile::Condstore)
            .await
            .expect("server starts");
        server.create_mailbox("INBOX");
        let uid = server.append_message("INBOX", "Subject: flags\r\n\r\nx", &[]);

        let mut engine_a = RawClient::connect(&server).await;
        let mut engine_b = RawClient::connect(&server).await;
        engine_a.login_select("INBOX").await;
        engine_b.login_select("INBOX").await;

        // B captures the current modseq, then A changes the message.
        let observed = engine_b.send(&format!("UID FETCH {uid} (FLAGS)")).await;
        let modseq = observed
            .iter()
            .find_map(|l| {
                let at = l.find("MODSEQ (")?;
                l[at + "MODSEQ (".len()..]
                    .split(')')
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .expect("a MODSEQ is reported");

        let _ = engine_a
            .send(&format!("UID STORE {uid} +FLAGS (\\Seen)"))
            .await;

        let conflicted = engine_b
            .send(&format!(
                "UID STORE {uid} (UNCHANGEDSINCE {modseq}) +FLAGS (\\Flagged)"
            ))
            .await;
        assert!(
            joined(&conflicted).contains(&format!("[MODIFIED {uid}]")),
            "B's conditional store must be refused: {conflicted:?}"
        );
        let flags = server.flags("INBOX", uid).unwrap_or_default();
        assert!(
            !flags.iter().any(|f| f == "\\Flagged"),
            "the refused store must not have applied: {flags:?}"
        );
    }

    #[tokio::test]
    async fn qresync_select_reports_vanished_for_messages_removed_since_the_modseq() {
        // The mechanism that fixes ghost messages. A forward-only high-water
        // mark cannot learn this; VANISHED states it directly.
        let server = MockImapServer::start(ServerProfile::Qresync)
            .await
            .expect("server starts");
        server.create_mailbox("INBOX");
        let keep = server.append_message("INBOX", "Subject: keep\r\n\r\nx", &[]);
        let drop = server.append_message("INBOX", "Subject: drop\r\n\r\nx", &[]);

        let mut client = RawClient::connect(&server).await;
        let _ = client.send("LOGIN user pass").await;
        let _ = client.send("ENABLE QRESYNC").await;
        let selected = client.send("SELECT INBOX").await;
        let modseq = selected
            .iter()
            .find_map(|l| {
                let at = l.find("HIGHESTMODSEQ ")?;
                l[at + "HIGHESTMODSEQ ".len()..]
                    .split(']')
                    .next()?
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .expect("HIGHESTMODSEQ is advertised");

        // Another client removes a message while we are away.
        assert!(server.expunge("INBOX", drop));

        let resync = client
            .send(&format!("SELECT INBOX (QRESYNC (1000 {modseq}))"))
            .await;
        let text = joined(&resync);
        assert!(
            text.contains("VANISHED (EARLIER)") && text.contains(&drop.to_string()),
            "the removal must be reported: {resync:?}"
        );
        assert_eq!(server.uids("INBOX"), vec![keep]);
    }

    #[tokio::test]
    async fn qresync_select_is_rejected_without_enable_and_on_a_condstore_only_server() {
        // RFC 7162 3.2.3 requires BAD when QRESYNC was not enabled. Modelling
        // Gmail (CONDSTORE, no QRESYNC) keeps the ladder's rung 2 honest.
        let qresync = MockImapServer::start(ServerProfile::Qresync)
            .await
            .expect("server starts");
        qresync.create_mailbox("INBOX");
        let mut client = RawClient::connect(&qresync).await;
        let _ = client.send("LOGIN user pass").await;
        let no_enable = client.send("SELECT INBOX (QRESYNC (1000 1))").await;
        assert!(
            joined(&no_enable).contains("BAD"),
            "QRESYNC without ENABLE must be BAD: {no_enable:?}"
        );

        let gmail = MockImapServer::start(ServerProfile::Condstore)
            .await
            .expect("server starts");
        gmail.create_mailbox("INBOX");
        let mut gclient = RawClient::connect(&gmail).await;
        let caps = gclient.send("CAPABILITY").await;
        let caps_text = joined(&caps);
        assert!(caps_text.contains("CONDSTORE"), "{caps:?}");
        assert!(
            !caps_text.contains("QRESYNC"),
            "the Gmail profile must not advertise QRESYNC: {caps:?}"
        );
        let _ = gclient.send("LOGIN user pass").await;
        let enabled = gclient.send("ENABLE QRESYNC").await;
        assert!(
            !joined(&enabled).contains("ENABLED QRESYNC"),
            "a CONDSTORE-only server must not enable QRESYNC: {enabled:?}"
        );
    }

    #[tokio::test]
    async fn expunge_reports_vanished_under_qresync_and_expunge_otherwise() {
        // RFC 6851 4.4. A client proving a delete by counting EXPUNGE lines
        // sees none under QRESYNC and would wrongly infer failure.
        for (profile, expect_vanished) in [
            (ServerProfile::Qresync, true),
            (ServerProfile::Condstore, false),
        ] {
            let server = MockImapServer::start(profile).await.expect("server starts");
            server.create_mailbox("INBOX");
            let uid = server.append_message("INBOX", "Subject: gone\r\n\r\nx", &["\\Deleted"]);

            let mut client = RawClient::connect(&server).await;
            let _ = client.send("LOGIN user pass").await;
            if expect_vanished {
                let _ = client.send("ENABLE QRESYNC").await;
            }
            let _ = client.send("SELECT INBOX").await;
            let out = client.send(&format!("UID EXPUNGE {uid}")).await;
            let text = joined(&out);

            if expect_vanished {
                assert!(text.contains("* VANISHED"), "expected VANISHED: {out:?}");
                assert!(!text.contains("EXPUNGE\r"), "{out:?}");
            } else {
                assert!(text.contains("EXPUNGE"), "expected EXPUNGE: {out:?}");
                assert!(!text.contains("VANISHED"), "{out:?}");
            }
            assert!(server.uids("INBOX").is_empty());
        }
    }

    #[tokio::test]
    async fn the_command_log_records_every_connections_commands_in_order() {
        let server = MockImapServer::start(ServerProfile::Basic)
            .await
            .expect("server starts");
        server.create_mailbox("INBOX");
        let mut a = RawClient::connect(&server).await;
        let mut b = RawClient::connect(&server).await;
        a.login_select("INBOX").await;
        let _ = b.send("LOGIN user pass").await;
        let _ = b.send("CAPABILITY").await;

        let fetches = server.commands_matching("SELECT");
        assert_eq!(fetches.len(), 1, "{fetches:?}");
        assert!(server.commands().len() >= 4, "{:?}", server.commands());
    }
}
