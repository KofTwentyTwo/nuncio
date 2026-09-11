# Local Google test service

`{"op":"remove_calendar","account":"alpha@example.test","id":"team-alpha@example.test"}` removes a calendar from that account's remote list and invalidates its event cursors. Other accounts retain their calendars. Removal is a mock control, not a production Calendar API call.

Optional Gmail fields can be omitted for a named fixture using stdin `{"op":"omit_message_fields","account":"alpha@example.test","id":"m-003","fields":["raw","internalDate"]}`. Empty fields restores normal responses. Allowed omissions are threadId, historyId, internalDate, labelIds, payload, raw, and sizeEstimate; identity cannot be omitted by this control. HTTP conformance verifies account isolation and restoration. Rust harnesses can also stop the actual listener while retaining its independent request/effect snapshots for offline-read assertions.

Run from the isolated worktree root:

```sh
cargo run --manifest-path rebuild/Cargo.toml -p nuncio-test-support --bin mock-google -- --ready-file /tmp/nuncio-google-ready.json
```

Choose a fresh readiness path. The service binds an ephemeral IPv4 loopback port, writes that path using create-new with mode 0600 on Unix, then emits the same ready JSON on stdout. It never contacts Google. It is unshipped test support, with no production engine/proto dependency.

The two accounts are `alpha@example.test` and `beta@example.test`. Each has an owned primary calendar and a secondary team calendar. The synthetic OAuth registration is `nuncio-test-client`; an optional client secret must equal `synthetic-client-secret`. Use the announced base URL plus `/o/oauth2/v2/auth`, `/token`, and `/v1/userinfo`. Userinfo requires openid/email permissions and returns fixed independent subjects ending 001/002. Token responses use Google's canonical userinfo.email spelling for the email scope. Mail and Calendar API paths use that same base URL. Consent defaults to alpha; `login_hint` selects either account. Generate PKCE and state normally. Do not use real client IDs or credentials.

Send one JSON command per line on stdin. Each receives a JSON result or explicit error on stdout. Controls are not HTTP endpoints and are not given to provider adapters.

```json
{"op":"snapshot"}
{"op":"page_cap","size":1}
{"op":"page_overlap","enabled":true}
{"op":"reverse","enabled":true}
{"op":"advance","seconds":3601}
{"op":"rotate_refresh_tokens","enabled":true}
{"op":"revoke","account":"alpha@example.test"}
{"op":"deny_consent","deny":true}
{"op":"deny_scope","scope":"https://www.googleapis.com/auth/calendar"}
{"op":"expire_cursor","account":"alpha@example.test","scope":"gmail_history"}
{"op":"expire_cursor","account":"alpha@example.test","scope":{"calendar":"primary"}}
{"op":"inject","fault":{"method":"POST","path":"/gmail/v1/users/me/messages/send","account":"alpha@example.test","call":null,"phase":"after","action":{"kind":"disconnect"}}}
{"op":"reset"}
{"op":"shutdown"}
```

Other commands are add_message, change_labels, delete_message, put_event, delete_event, and release. Their typed JSON fields are defined in `src/bin/mock-google.rs`. Message seeding uses an array of raw byte integers. In-process harnesses use `GoogleControl` directly, including wait_for_barrier/release_barrier. Snapshots contain remote records, accepted send bytes, request counts, and notification recipients. They do not expose OAuth grants.

Faults are one-shot, selected by exact method/path, optional authenticated account, and optional absolute call ordinal. A null ordinal means the next matching call. Before faults prevent acceptance; after faults run only after a successful route mutation/response. Actions are status (with optional Retry-After), malformed_json, truncated_body, disconnect, delay, and withhold. Withholding names an out-of-band barrier; shutdown cancels it. Truncation flushes a JSON prefix before failing the body stream. Delays are simulated response behavior, not readiness sleeps.

Reset restores deterministic seed objects, counters, page configuration, and OAuth state. Token names keep a monotonically increasing generation so pre-reset tokens cannot become valid again. Page tokens are reusable snapshots scoped to account/path/query; they are not synchronization cursors. The default server page cap is two. Overlap uses overlapping pages when the cap is at least two.

Supported release surfaces: OAuth auth/token; Gmail profile, labels, message list/get/attachments, thread get, history, modify, trash/untrash and send; Calendar list, event list/get/instances/insert/patch/delete and freeBusy. MIME, labels, event records, history and delivery effects are independently held in memory. State persists across daemon restarts while the mock process stays running; restarting/resetting the mock deliberately resets its remote world.

The service implements the release's documented subset, not every Google feature. It rejects unknown request fields and unsupported routes/queries. Gmail search currently supports rfc822msgid lookup for reconciliation; no settings, alias discovery, server drafts or permanent-delete endpoint. Calendar canonical requests use an unbounded fixed query and expanded requests use a separate bounded window. Canonical window queries and expanded incremental tokens explicitly return unsupported errors. Recurrence expansion is limited to 10,000 results; omitted mock window bounds are 2025-01-01 through 2030-01-01. These are mock limits, not production Google limits. Notification counters model the requested test invitation effects; they cannot establish actual Google email delivery behavior.

Run conformance:

```sh
cargo test --manifest-path rebuild/Cargo.toml -p nuncio-test-support --test google_mock_contract
```

The suite includes the actual mock subprocess, its readiness file, reset, and shutdown. System and daemon/CLI subprocess E2E are separate gates; see TESTING.md. Refresh rotation invalidates the presented old refresh token and issues a new one before any after-acceptance fault. All tests remain offline from real providers. Synology-compatible local services follow in the approved IMAP/SMTP task.

`omit_calendar_fields` accepts account, id (or primary), and fields from timeZone/summary/primary/accessRole; an empty fields array restores catalog fields. It does not alter the events response timezone. Unsupported omissions fail.

## Calendar permissions

Use stdin `{"op":"set_calendar_role","account":"alpha@example.test","calendar":"team-alpha@example.test","role":"reader"}` to make the secondary calendar read-only. Supported control roles are owner, writer and reader; unsupported roles fail explicitly, and primary ownership cannot be removed. CalendarList and event listings expose the configured role; unauthorized creates, patches and deletes return403 before effects.

For fixture events with organizer.self=false, patches to guestsCanInviteOthers, guestsCanModify or guestsCanSeeOtherGuests return403 forbiddenForNonOrganizer. Attendee-copy content changes are permitted: Google allows local changes that may later be overwritten by organizer updates. The independent RSVP contract checks a selected attendee response while preserving other attendees and unknown private properties. Product RSVP restrictions must be tested separately in the engine; this mock does not invent a blanket organizer-only editing rule. [Calendar sharing](https://developers.google.com/workspace/calendar/api/concepts/sharing), [non-organizer errors](https://developers.google.com/workspace/calendar/api/guides/errors), [attendee-copy behavior](https://developers.google.com/workspace/calendar/api/concepts/inviting-attendees-to-events).


Partial RSVP uses `attendeesOmitted:true` plus exactly one attendee's email/responseStatus/comment. The mock merges that into independent server state, preserving other attendees and untouched provider fields. Invalid or other-participant partial changes fail without event/version/notification effects. Unknown request attendee fields remain rejected. [Google Events partial-response semantics](https://developers.google.com/workspace/calendar/api/v3/reference/events).

FreeBusy conformance independently checks clipped opaque intervals, transparent events, a truly empty calendar versus per-calendar notFound, request limits/unknown fields and unchanged remote state/counters. It supports individual calendars (up to50), not group expansion. Production availability parsing must preserve provider errors and cannot substitute missing results with free time. Recurrence conformance includes whole-series parent edits before a moved/cancelled instance. Inheritance of later master field changes into already stored exceptions remains a compatibility test gap; the existing tests do not establish that behavior.
