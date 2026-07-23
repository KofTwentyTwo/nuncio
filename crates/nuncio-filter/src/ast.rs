//! AST and Domain Entities for NSQL (Nuncio SQL) Email Filter Rules.

use serde::{Deserialize, Serialize};

/// Target fields supported by NSQL email filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterField {
    /// Email subject line.
    Subject,
    /// Sender email address (`From`).
    From,
    /// Recipient email address (`To`).
    To,
    /// Email body (plain text or HTML).
    Body,
    /// Specific MIME header value (e.g. `X-Spam-Score`).
    Header(String),
    /// Attachment presence indicator (`true`/`false`).
    HasAttachment,
    /// Message size in bytes.
    Size,
    /// Current folder location (e.g. `INBOX`).
    Folder,
    /// Arrival timestamp.
    Date,
    /// Target Account identifier or email address.
    Account,
}

impl FilterField {
    /// Parse a field string into a `FilterField`.
    pub fn parse_str(s: &str) -> Option<Self> {
        let s_lower = s.to_lowercase();
        match s_lower.as_str() {
            "subject" => Some(Self::Subject),
            "from" | "sender" => Some(Self::From),
            "to" | "recipient" => Some(Self::To),
            "body" => Some(Self::Body),
            "has_attachment" | "has_attachments" | "attachment" => Some(Self::HasAttachment),
            "size" => Some(Self::Size),
            "folder" => Some(Self::Folder),
            "date" | "received_at" => Some(Self::Date),
            "account" | "account_id" => Some(Self::Account),
            _ if s_lower.starts_with("header[") && s_lower.ends_with(']') => {
                let key = &s[7..s.len() - 1];
                Some(Self::Header(key.trim_matches('\'').trim_matches('"').to_string()))
            }
            _ => None,
        }
    }

    /// Render field name as NSQL string.
    pub fn to_nsql(&self) -> String {
        match self {
            Self::Subject => "subject".to_string(),
            Self::From => "from".to_string(),
            Self::To => "to".to_string(),
            Self::Body => "body".to_string(),
            Self::Header(name) => format!("header['{name}']"),
            Self::HasAttachment => "has_attachment".to_string(),
            Self::Size => "size".to_string(),
            Self::Folder => "folder".to_string(),
            Self::Date => "date".to_string(),
            Self::Account => "account".to_string(),
        }
    }
}

/// Comparison and matching operators supported by NSQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterOperator {
    /// Exact match (`=`).
    Equals,
    /// Inequality (`!=`).
    NotEquals,
    /// Substring inclusion (`CONTAINS`).
    Contains,
    /// Substring exclusion (`NOT CONTAINS`).
    NotContains,
    /// Regular expression pattern match (`MATCHES`).
    Matches,
    /// Greater than (`>`).
    GreaterThan,
    /// Less than (`<`).
    LessThan,
    /// Greater than or equal (`>=`).
    GreaterThanOrEqual,
    /// Less than or equal (`<=`).
    LessThanOrEqual,
    /// In set (`IN`).
    In,
}

impl FilterOperator {
    /// Render operator as NSQL string keyword or symbol.
    pub fn to_nsql(&self) -> &'static str {
        match self {
            Self::Equals => "=",
            Self::NotEquals => "!=",
            Self::Contains => "CONTAINS",
            Self::NotContains => "NOT CONTAINS",
            Self::Matches => "MATCHES",
            Self::GreaterThan => ">",
            Self::LessThan => "<",
            Self::GreaterThanOrEqual => ">=",
            Self::LessThanOrEqual => "<=",
            Self::In => "IN",
        }
    }
}

/// Filter condition leaf value variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterValue {
    /// String literal value.
    String(String),
    /// Numeric integer value.
    Number(i64),
    /// Boolean flag value.
    Boolean(bool),
    /// Set/list of string values for `IN` operator.
    List(Vec<String>),
}

impl FilterValue {
    /// Render value literal as NSQL code representation.
    pub fn to_nsql(&self) -> String {
        match self {
            Self::String(s) => format!("'{}'", s.replace('\'', "\\'")),
            Self::Number(n) => n.to_string(),
            Self::Boolean(b) => if *b { "true".to_string() } else { "false".to_string() },
            Self::List(list) => {
                let items: Vec<String> = list.iter().map(|s| format!("'{}'", s.replace('\'', "\\'"))).collect();
                format!("({})", items.join(", "))
            }
        }
    }
}

/// Leaf condition comparing a field against a value using an operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionLeaf {
    /// Field targeted by condition.
    pub field: FilterField,
    /// Operator applied.
    pub operator: FilterOperator,
    /// Literal target value.
    pub value: FilterValue,
}

/// Recursive condition tree node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionNode {
    /// Terminal condition leaf.
    Leaf(ConditionLeaf),
    /// Logical conjunction (`AND`).
    And(Vec<ConditionNode>),
    /// Logical disjunction (`OR`).
    Or(Vec<ConditionNode>),
    /// Logical negation (`NOT`).
    Not(Box<ConditionNode>),
}

impl ConditionNode {
    /// Calculate maximum AST depth to prevent stack overflows.
    pub fn depth(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::And(children) | Self::Or(children) => {
                1 + children.iter().map(|c| c.depth()).max().unwrap_or(0)
            }
            Self::Not(inner) => 1 + inner.depth(),
        }
    }

    /// Render condition tree as NSQL WHERE clause expression.
    pub fn to_nsql(&self) -> String {
        match self {
            Self::Leaf(leaf) => format!("{} {} {}", leaf.field.to_nsql(), leaf.operator.to_nsql(), leaf.value.to_nsql()),
            Self::And(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| format!("({})", n.to_nsql())).collect();
                parts.join(" AND ")
            }
            Self::Or(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| format!("({})", n.to_nsql())).collect();
                parts.join(" OR ")
            }
            Self::Not(node) => format!("NOT ({})", node.to_nsql()),
        }
    }
}

/// Actions executed when a filter rule matches an email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleAction {
    /// Move email to target folder.
    MoveTo(String),
    /// Copy email to target folder.
    CopyTo(String),
    /// Mark email as read.
    MarkRead,
    /// Mark email as unread.
    MarkUnread,
    /// Add starred/flagged state.
    Flag,
    /// Remove starred/flagged state.
    Unflag,
    /// Move email to trash / mark deleted.
    Delete,
    /// Forward copy of email to another recipient address.
    ForwardTo(String),
    /// Send POST request to external HTTP webhook endpoint.
    CallWebhook(String),
}

impl RuleAction {
    /// Render rule action as NSQL string clause.
    pub fn to_nsql(&self) -> String {
        match self {
            Self::MoveTo(folder) => format!("MOVE TO '{folder}'"),
            Self::CopyTo(folder) => format!("COPY TO '{folder}'"),
            Self::MarkRead => "MARK READ".to_string(),
            Self::MarkUnread => "MARK UNREAD".to_string(),
            Self::Flag => "FLAG".to_string(),
            Self::Unflag => "UNFLAG".to_string(),
            Self::Delete => "DELETE".to_string(),
            Self::ForwardTo(email) => format!("FORWARD TO '{email}'"),
            Self::CallWebhook(url) => format!("CALL WEBHOOK '{url}'"),
        }
    }
}

/// Domain entity representing a complete NSQL Filter Rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterRule {
    /// Rule unique identifier.
    pub id: String,
    /// Rule display name.
    pub name: String,
    /// Target Account pattern ("*" for all accounts, "user@nuncio.mx", or wildcard "%@kof22.com%").
    pub target_account: String,
    /// Rule evaluation order (0 = highest).
    pub priority: i32,
    /// Active flag.
    pub enabled: bool,
    /// Full NSQL source code text.
    pub nsql_text: String,
    /// Parsed condition tree.
    pub conditions: ConditionNode,
    /// Actions executed on match.
    pub actions: Vec<RuleAction>,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

impl FilterRule {
    /// Check whether this rule matches a given target account.
    pub fn matches_account(&self, account_id: &str) -> bool {
        if self.target_account == "*" || self.target_account == "%" || self.target_account.is_empty() {
            return true;
        }
        if self.target_account.eq_ignore_ascii_case(account_id) {
            return true;
        }
        if self.target_account.contains('*') || self.target_account.contains('%') {
            let pattern = self
                .target_account
                .replace('*', ".*")
                .replace('%', ".*");
            if let Ok(re) = regex::Regex::new(&format!("(?i)^{pattern}$")) {
                return re.is_match(account_id);
            }
        }
        false
    }
}

/// Execution log entry recorded when a rule matches a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterExecutionLog {
    /// Log entry primary key.
    pub id: i64,
    /// Rule ID executed.
    pub rule_id: String,
    /// Email message ID evaluated.
    pub message_id: String,
    /// Action description string.
    pub action_taken: String,
    /// Timestamp of execution.
    pub matched_at: i64,
    /// Cryptographic hash of preceding log record (hash-chain ledger).
    pub prev_hash: String,
    /// HMAC-SHA256 hash of this record.
    pub hash: String,
}

/// Outbox item for remote IMAP/JMAP mutations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRemoteMutation {
    /// Primary key UUID.
    pub id: String,
    /// Source filter rule ID.
    pub rule_id: String,
    /// Target email message ID.
    pub message_id: String,
    /// Type of mutation (`MOVE`, `FLAG`, `MARK_READ`, `DELETE`, etc.).
    pub mutation_type: String,
    /// JSON payload with parameters (e.g. folder name, target address).
    pub payload: String,
    /// Status (`pending`, `completed`, `failed`).
    pub status: String,
    /// Number of retry attempts.
    pub retry_count: i32,
    /// Record creation timestamp.
    pub created_at: i64,
}

/// Dry-run preview evaluation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterPreviewResult {
    /// Evaluated email message ID.
    pub message_id: String,
    /// Whether any filter rule matched.
    pub matched: bool,
    /// ID of matching rule if any.
    pub matched_rule_id: Option<String>,
    /// Name of matching rule if any.
    pub matched_rule_name: Option<String>,
    /// Actions that would be executed.
    pub actions_evaluated: Vec<RuleAction>,
    /// Evaluation duration in microseconds.
    pub execution_time_us: u64,
    /// Human-readable step-by-step match traces.
    pub condition_traces: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_rule_account_wildcard_matching() {
        let rule = FilterRule {
            id: "r1".to_string(),
            name: "Global Rule".to_string(),
            target_account: "*".to_string(),
            priority: 1,
            enabled: true,
            nsql_text: "".to_string(),
            conditions: ConditionNode::Leaf(ConditionLeaf {
                field: FilterField::Subject,
                operator: FilterOperator::Equals,
                value: FilterValue::String("x".to_string()),
            }),
            actions: vec![],
            created_at: 0,
            updated_at: 0,
        };

        assert!(rule.matches_account("james.maes@kof22.com"));
        assert!(rule.matches_account("work@nuncio.mx"));

        let domain_rule = FilterRule {
            target_account: "%@kof22.com".to_string(),
            ..rule.clone()
        };

        assert!(domain_rule.matches_account("alice@kof22.com"));
        assert!(!domain_rule.matches_account("bob@nuncio.mx"));
    }
}
