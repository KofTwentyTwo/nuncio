# NSQL Filter Language & Webhook Specification

> **Status (2026-07-26):** describes the *intended* language. The parser,
> validator, and match engine are genuinely implemented and wired; however
> **actions do not yet execute**, `ON ACCOUNT` scoping is ignored, header
> conditions and non-ASCII rules have known bugs, and webhooks are unwired. Treat
> this as the design target, not current behavior. Corrections are tracked in
> [`ROADMAP.md`](ROADMAP.md) (Phase 3) and [`BACKLOG.md`](BACKLOG.md); reality is
> the 2026-07-26 assessment.

Nuncio SQL Filter Language (NSQL) is a declarative, high-throughput email routing and automation language powered by `sqlparser-rs`.

---

## 1. NSQL Compiler Pipeline & Validation Flowchart

![NSQL Compiler Pipeline](assets/nsql_pipeline.svg)

---

## 2. NSQL Syntax Grammar

```sql
/* Complete NSQL Statement Syntax */
[ON ACCOUNT '<target_account>']
WHEN <field> <operator> <value> [AND|OR <condition>]
THEN <action_1> [, <action_2> ...]
[WITH STOP PROCESSING];
```

### Supported Fields & Operators
- **Fields**: `subject`, `from`, `to`, `body`, `header['X-Spam-Score']`, `has_attachment`, `size`, `folder`, `date`, `account`.
- **Operators**: `=`, `!=`, `CONTAINS`, `NOT CONTAINS`, `MATCHES` (DFA Regex), `>`, `<`, `>=`, `<=`, `IN ('a', 'b')`.
- **Actions**: `MOVE TO 'folder'`, `MARK READ`, `MARK UNREAD`, `FLAG`, `UNFLAG`, `DELETE`, `FORWARD TO 'email'`, `CALL WEBHOOK 'url'`.

---

## 3. Cryptographic Webhook Dispatch Flow

![IPC Frame Sequence](assets/ipc_sequence.svg)
