# Nuncio SQL Filter Language (NSQL) Complete Specification & Reference Guide

> **Authoritative Wiki Master Specification**  
> Maintained in `wiki/NSQL-Filter-Language-Specification.md` and synchronized with [KofTwentyTwo/nuncio](https://github.com/KofTwentyTwo/nuncio).

---

## Executive Overview

**Nuncio SQL Filter Language (NSQL)** is a declarative, type-safe Domain Specific Language (DSL) for defining server-side email routing, automated labeling, thread triage, webhook notifications, and security policies across the Nuncio ecosystem.

NSQL rules execute **server-side inside the `nunciod` background daemon** during protocol synchronization (IMAP IDLE and JMAP Push) **before** messages are written to local storage. Under Nuncio's **100% Multi-Shell Parity Mandate**, NSQL rules can be authored, visually built, dry-run tested, exported, deleted, and audited with 1:1 functional equivalence across all four presentation interfaces (`nuncio-cli`, `nuncio-tui`, `nuncio-gui`, `nuncio-mcp`).

---

## 1. Formal EBNF Grammar Specification

```ebnf
(* NSQL Formal EBNF Grammar Definition *)

NsqlStatement     ::= CreateRuleStmt | SelectWhereStmt ;

CreateRuleStmt    ::= "CREATE" "RULE" RuleName 
                      ("ON" "ACCOUNT" AccountId)?
                      ("PRIORITY" PriorityValue)?
                      "WHEN" ConditionExpr
                      "THEN" ActionList
                      ("WITH" StopClause)? ";" ;

RuleName          ::= StringLiteral ;
AccountId         ::= StringLiteral ;
PriorityValue     ::= Integer ;

(* Condition Expression Grammar (Boolean Logic) *)
ConditionExpr     ::= Term ( ("AND" | "OR") Term )* ;
Term              ::= ("NOT")? Factor ;
Factor            ::= "(" ConditionExpr ")" | Comparison ;

Comparison        ::= Identifier Operator ValueExpr
                    | Identifier "IS" ("NOT")? "NULL"
                    | Identifier "BETWEEN" ValueExpr "AND" ValueExpr ;

Identifier        ::= "sender" | "from" | "recipient" | "to" | "cc" | "subject"
                    | "body" | "body_plain" | "body_html" | "size" | "size_bytes"
                    | "has_attachment" | "read" | "flagged"
                    | "header" "(" StringLiteral ")" ;

Operator          ::= "=" | "!=" | "<>" | ">" | "<" | ">=" | "<="
                    | "LIKE" | "NOT" "LIKE" | "REGEXP" | "MATCH" | "CONTAINS" ;

ValueExpr         ::= StringLiteral | Integer | BooleanLiteral ;

(* Action Grammar *)
ActionList        ::= ActionStmt ( "," ActionStmt )* ;

ActionStmt        ::= MoveAction
                    | CopyAction
                    | SetAction
                    | AddLabelAction
                    | RemoveLabelAction
                    | ForwardAction
                    | DeleteAction
                    | WebhookAction ;

MoveAction        ::= "MOVE" ("TO")? StringLiteral ;
CopyAction        ::= "COPY" ("TO")? StringLiteral ;
SetAction         ::= "SET" ("read" | "flagged") "=" BooleanLiteral ;
AddLabelAction    ::= "ADD" "LABEL" StringLiteral ;
RemoveLabelAction ::= "REMOVE" "LABEL" StringLiteral ;
ForwardAction     ::= "FORWARD" ("TO")? StringLiteral ;
DeleteAction      ::= "DELETE" ;
WebhookAction     ::= ("CALL" | "TRIGGER") "WEBHOOK" StringLiteral ;

StopClause        ::= "STOP" "PROCESSING" | "CONTINUE" ;

StringLiteral     ::= "'" ( [^'\\] | '\\' . )* "'" | '"' ( [^"\\] | '\\' . )* '"' ;
Integer           ::= [0-9]+ ;
BooleanLiteral    ::= "TRUE" | "FALSE" | "true" | "false" ;
```

---

## 2. Language Syntax & Fields Reference

### A. Statement Structure
An NSQL rule consists of five primary clauses:
```sql
CREATE RULE "<rule_name>"
ON ACCOUNT "<account_id>"
PRIORITY <integer>
WHEN <condition_expression>
THEN <action_list>
WITH <stop_option>;
```

### B. Condition Target Fields

| Field Name | Aliases | Data Type | Description |
| :--- | :--- | :--- | :--- |
| `sender` | `from` | String | Senders MIME address (`From:` header) |
| `recipient` | `to` | String | Primary recipient address (`To:` header) |
| `cc` | — | String | Carbon copy recipients (`Cc:` header) |
| `subject` | — | String | Email subject line |
| `body` | `body_plain` | String | Plaintext or HTML message content |
| `header('Name')` | — | String | Arbitrary custom MIME header (e.g. `header('X-Spam-Flag')`) |
| `size_bytes` | `size` | Integer | Total payload size in bytes |
| `has_attachment` | — | Boolean | Evaluates whether attached files exist |
| `read` | `is_read` | Boolean | Read / unread status flag |
| `flagged` | `is_flagged` | Boolean | Starred / flagged status flag |

### C. Condition Comparison Operators

| Operator | Syntax | Description | Example |
| :--- | :--- | :--- | :--- |
| **Equals** | `=` / `!=` | Exact string or numeric equality | `to = 'vip@kof22.com'` |
| **Pattern Match** | `LIKE` / `NOT LIKE` | Case-insensitive wildcard pattern (`%`) | `sender LIKE '%@nuncio.mx%'` |
| **Regex Match** | `REGEXP` | Regular expression match | `subject REGEXP '(?i)(invoice\|receipt)'` |
| **Numeric Range** | `>`, `<`, `>=`, `<=` | Numeric comparison for size and dates | `size_bytes > 10485760` (10 MB) |
| **Null Test** | `IS NULL` / `IS NOT NULL` | Evaluates header presence | `header('List-Unsubscribe') IS NOT NULL` |

### D. Imperative Mutation Actions

| Action Command | Arguments | Description |
| :--- | :--- | :--- |
| `MOVE TO 'folder'` | Folder ID string | Relocates message to target mailbox folder |
| `COPY TO 'folder'` | Folder ID string | Copies message to additional folder |
| `SET read = TRUE` | Boolean | Marks message as read (`FALSE` for unread) |
| `SET flagged = TRUE` | Boolean | Flags/stars message (`FALSE` to unflag) |
| `ADD LABEL 'label'` | Label string | Applies custom tag or IMAP keyword |
| `REMOVE LABEL 'label'`| Label string | Removes specified tag |
| `FORWARD TO 'email'` | Recipient string | Forwards message to whitelisted external address |
| `DELETE` | — | Relocates message to Trash folder |
| `CALL WEBHOOK 'url'` | Target HTTPS URL | Triggers signed outbound HMAC-SHA256 HTTP POST webhook |

---

## 3. Comprehensive How-To Guide Across All 4 Presentation Shells

### A. POSIX CLI (`nuncio-cli`)

#### 1. Creating a Rule from Inline NSQL String
```bash
nuncio filter create \
  --name "VIP Priority Alerts" \
  --sql "CREATE RULE 'VIP Priority' ON ACCOUNT 'james.maes@kof22.com' PRIORITY 10 WHEN sender LIKE '%@kof22.com%' THEN MOVE TO 'vip-inbox', SET flagged = TRUE WITH STOP PROCESSING;" \
  --json
```

#### 2. Dry-Run Testing an NSQL Rule Against Inbox
```bash
nuncio filter test --sql "WHEN sender LIKE '%@github.com' THEN MOVE TO 'Dev/Github'" --limit 50
```

#### 3. Exporting & Importing NSQL Rule Manifests
```bash
# Export rules to a formatted NSQL script file
nuncio filter export --account-id acct-1 --format sql > production_rules.nsql

# Import rules from NSQL file
nuncio filter import --file production_rules.nsql
```

#### 4. Auditing Rule Execution Logs
```bash
nuncio filter logs --rule-id rule_01J38X4902K --limit 20
```

---

### B. Terminal TUI (`nuncio-tui` in `AppMode::FilterRules`)

1. Open Nuncio TUI (`nuncio-tui`).
2. Press `[a]` or navigate to **Account & Filter Settings**.
3. Press `[n]` to create a new rule.
4. Press `[s]` to toggle between **Visual Form Mode** and the **NSQL Syntax Editor**.
5. Write your NSQL rule syntax. Press `[Ctrl+T]` to execute a instant dry-run preview against local emails.
6. Press `[Ctrl+S]` to save the rule.
7. Use `[J]` / `[K]` to adjust rule priority up or down.
8. Press `[l]` to open the **Cryptographic Execution Audit Log Drawer**.

---

### C. Desktop GUI (`nuncio-gui` `<FilterRulesModal />`)

1. In the left sidebar footer, click **`Accounts & Connectivity`** or open **Filter Rules**.
2. Click **`+ New Rule`**.
3. Toggle between **"Visual Builder"** (drag-and-drop conditions) and **"NSQL Query Editor"** (syntax-highlighted code editor with live syntax checking).
4. Click **`Test Dry-Run Preview`** to view a live split-pane listing all historical inbox messages that match your rule.
5. Click **`Save Rule`**.
6. Switch to the **"Execution Logs"** tab to inspect trigger timestamps, microsecond latency profiling, and action outputs.

---

### D. Native MCP Tool Calls (`nuncio-mcp`)

AI Agents can create, test, and audit rules via MCP stdio tools:

#### 1. `nuncio_filter_create` Tool Call
```json
{
  "name": "nuncio_filter_create",
  "arguments": {
    "account_id": "acct-1",
    "sql": "CREATE RULE 'Auto-Archive Receipts' WHEN subject LIKE '%Receipt%' OR subject LIKE '%Invoice%' THEN MOVE TO 'Finance', SET read = TRUE WITH STOP PROCESSING;"
  }
}
```

#### 2. `nuncio_filter_test` Dry-Run Preview Tool Call
```json
{
  "name": "nuncio_filter_test",
  "arguments": {
    "sql": "WHEN sender LIKE '%@newsletter.com' THEN MOVE TO 'Feed'",
    "limit": 20
  }
}
```

---

## 4. Real-World NSQL Rule Library

### Rule 1: High-Priority VIP Client Routing & Webhook Alert
```sql
CREATE RULE "VIP Client Priority & Auto-Label" 
ON ACCOUNT "james.maes@kof22.com"
PRIORITY 10
WHEN 
    sender LIKE '%@kof22.com%' 
    AND (subject REGEXP '(Urgent|Architecture|Deadline)' OR has_attachment = TRUE)
THEN
    MOVE TO 'vip-inbox',
    SET read = TRUE,
    SET flagged = TRUE,
    ADD LABEL 'High-Priority',
    CALL WEBHOOK 'https://api.kof22.com/webhooks/vip-alert'
WITH STOP PROCESSING;
```

### Rule 2: Auto-Archive Newsletters & Promotional Bulletins
```sql
CREATE RULE "Auto-Archive Newsletters"
ON ACCOUNT "james.maes@kof22.com"
PRIORITY 50
WHEN 
    (body LIKE '%unsubscribe%' OR header('List-Unsubscribe') IS NOT NULL)
    AND sender NOT LIKE '%@kof22.com%'
THEN
    MOVE TO 'Newsletters',
    SET read = TRUE,
    ADD LABEL 'Promotions'
WITH STOP PROCESSING;
```

### Rule 3: Large File Attachment Isolation
```sql
CREATE RULE "Isolate Large File Attachments"
ON ACCOUNT "james.maes@kof22.com"
PRIORITY 20
WHEN 
    has_attachment = TRUE 
    AND size_bytes > 25165824  -- 25 MB
THEN
    MOVE TO 'Large-Files',
    SET flagged = TRUE,
    CALL WEBHOOK 'https://api.kof22.com/webhooks/large-attachment-alert'
WITH CONTINUE;
```

### Rule 4: Immediate Spam Purging
```sql
CREATE RULE "Immediate Spam Purge"
ON ACCOUNT "james.maes@kof22.com"
PRIORITY 1
WHEN 
    header('X-Spam-Flag') = 'YES'
    OR sender LIKE '%@phishing-domain.com'
THEN
    DELETE
WITH STOP PROCESSING;
```

---

## 5. Security & System Architecture Guarantees

1. **SQL Injection Immunized**: NSQL strings are parsed into typed Rust AST nodes (`sqlparser = "0.54"`). Database queries against SQLite use `sqlx::QueryBuilder` positional parameter bindings (`?`). Zero string concatenation.
2. **ReDoS Protected**: Regular expressions are evaluated using linear-time DFA execution via Rust's `regex` crate with a **50ms hard execution limit** (`tokio::time::timeout`).
3. **SSRF Defended**: Webhooks perform pre-flight DNS IP resolution, blacklisting loopback (`127.0.0.1`), cloud metadata (`169.254.169.254`), and private subnets with `max_redirects(0)`.
4. **Cryptographic Hash Chain Ledger**: Audit log entries in `filter_execution_logs` are block-linked using HMAC-SHA256 signatures ($H_n = \text{HMAC}(K, H_{n-1} \parallel \text{data})$) to guarantee zero log tampering.
