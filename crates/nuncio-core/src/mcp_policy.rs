//! MCP Agent Access Control & RBAC Policy Engine.
//!
//! Provides granular data domain filtering (emails, accounts, calendars, contacts, telemetry, filter rules)
//! and capabilities authorization per connected AI agent.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Supported data domain categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    /// Email messages and attachments.
    Mail,
    /// Calendar events and schedules.
    Calendar,
    /// Address book contacts.
    Contacts,
    /// NSQL filter and automation rules.
    FilterRules,
    /// Real-time daemon telemetry & logs.
    Telemetry,
}

/// Action capability permission flags for an AI agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPermissions {
    /// Read email messages and list mailboxes.
    pub read_mail: bool,
    /// Send email messages (subject to HITL prompt validation).
    pub send_mail: bool,
    /// Read calendar events.
    pub read_calendar: bool,
    /// Create, edit, or delete calendar events.
    pub write_calendar: bool,
    /// Read address book contacts.
    pub read_contacts: bool,
    /// Manage (create/edit/delete) NSQL filter rules.
    pub manage_filters: bool,
    /// Inspect real-time daemon metrics and telemetry logs.
    pub read_telemetry: bool,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            read_mail: true,
            send_mail: false, // Default to read-only for safety
            read_calendar: true,
            write_calendar: false,
            read_contacts: true,
            manage_filters: false,
            read_telemetry: true,
        }
    }
}

impl AgentPermissions {
    /// Full administrative capability set.
    pub fn full_access() -> Self {
        Self {
            read_mail: true,
            send_mail: true,
            read_calendar: true,
            write_calendar: true,
            read_contacts: true,
            manage_filters: true,
            read_telemetry: true,
        }
    }

    /// Read-only capability set.
    pub fn read_only() -> Self {
        Self {
            read_mail: true,
            send_mail: false,
            read_calendar: true,
            write_calendar: false,
            read_contacts: true,
            manage_filters: false,
            read_telemetry: true,
        }
    }
}

/// Scoped policy governing an AI Agent's data visibility and permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAgentPolicy {
    /// Unique agent identifier (e.g. "claude-desktop", "antigravity-agent", "support-bot").
    pub agent_id: String,
    /// User-friendly policy description.
    pub name: String,
    /// Allowed account ID patterns (e.g. ["acct-work", "*@kof22.com"], or ["*"] for all).
    pub allowed_accounts: Vec<String>,
    /// Allowed folder patterns (e.g. ["inbox", "sent"], or ["*"] for all).
    pub allowed_folders: Vec<String>,
    /// Enabled data domain categories.
    pub allowed_data_types: HashSet<DataType>,
    /// Capability permissions.
    pub permissions: AgentPermissions,
    /// Auto-redact sensitive regex patterns (e.g. SSNs, credit cards) before LLM response output.
    pub redact_sensitive_patterns: bool,
}

impl Default for McpAgentPolicy {
    fn default() -> Self {
        let mut data_types = HashSet::new();
        data_types.insert(DataType::Mail);
        data_types.insert(DataType::Calendar);
        data_types.insert(DataType::Contacts);
        data_types.insert(DataType::Telemetry);

        Self {
            agent_id: "default-agent".to_string(),
            name: "Default MCP Agent Policy".to_string(),
            allowed_accounts: vec!["*".to_string()],
            allowed_folders: vec!["*".to_string()],
            allowed_data_types: data_types,
            permissions: AgentPermissions::default(),
            redact_sensitive_patterns: true,
        }
    }
}

impl McpAgentPolicy {
    /// Unrestricted full access policy.
    pub fn unrestricted(agent_id: &str) -> Self {
        let mut data_types = HashSet::new();
        data_types.insert(DataType::Mail);
        data_types.insert(DataType::Calendar);
        data_types.insert(DataType::Contacts);
        data_types.insert(DataType::FilterRules);
        data_types.insert(DataType::Telemetry);

        Self {
            agent_id: agent_id.to_string(),
            name: format!("Unrestricted Policy for {}", agent_id),
            allowed_accounts: vec!["*".to_string()],
            allowed_folders: vec!["*".to_string()],
            allowed_data_types: data_types,
            permissions: AgentPermissions::full_access(),
            redact_sensitive_patterns: false,
        }
    }

    /// Check if the agent is allowed to access data from a specific account ID.
    pub fn is_account_allowed(&self, account_id: &str) -> bool {
        if self.allowed_accounts.iter().any(|pattern| pattern == "*") {
            return true;
        }
        for pattern in &self.allowed_accounts {
            if pattern.contains('*') {
                let regex_pattern = format!("(?i)^{}$", regex::escape(pattern).replace("\\*", ".*"));
                if let Ok(re) = regex::Regex::new(&regex_pattern) {
                    if re.is_match(account_id) {
                        return true;
                    }
                }
            } else if pattern.eq_ignore_ascii_case(account_id) {
                return true;
            }
        }
        false
    }

    /// Check if the agent is allowed to access a specific mailbox folder ID.
    pub fn is_folder_allowed(&self, folder_id: &str) -> bool {
        for pattern in &self.allowed_folders {
            if pattern.starts_with('!') {
                let forbidden = &pattern[1..];
                if forbidden.eq_ignore_ascii_case(folder_id) {
                    return false;
                }
            }
        }
        for pattern in &self.allowed_folders {
            if !pattern.starts_with('!') && (pattern == "*" || pattern.eq_ignore_ascii_case(folder_id)) {
                return true;
            }
        }
        false
    }

    /// Check if a data type category is allowed for this agent.
    pub fn is_data_type_allowed(&self, data_type: DataType) -> bool {
        self.allowed_data_types.contains(&data_type)
    }

    /// Apply content redaction (masking credit cards & SSNs if enabled).
    pub fn sanitize_content(&self, text: &str) -> String {
        if !self.redact_sensitive_patterns {
            return text.to_string();
        }

        // Mask credit cards
        let cc_regex = regex::Regex::new(r"\b(?:\d[ -]*?){13,16}\b").unwrap();
        let redacted_cc = cc_regex.replace_all(text, "[REDACTED-CREDIT-CARD]");

        // Mask US SSNs
        let ssn_regex = regex::Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
        let redacted_ssn = ssn_regex.replace_all(&redacted_cc, "[REDACTED-SSN]");

        redacted_ssn.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_blocks_send_mail_and_filter_management() {
        let policy = McpAgentPolicy::default();
        assert!(policy.permissions.read_mail);
        assert!(!policy.permissions.send_mail);
        assert!(!policy.permissions.manage_filters);
        assert!(policy.is_data_type_allowed(DataType::Mail));
        assert!(!policy.is_data_type_allowed(DataType::FilterRules));
    }

    #[test]
    fn account_filtering_wildcards_and_exact() {
        let mut policy = McpAgentPolicy::default();
        policy.allowed_accounts = vec!["acct-work".to_string(), "*@kof22.com".to_string()];

        assert!(policy.is_account_allowed("acct-work"));
        assert!(policy.is_account_allowed("user@kof22.com"));
        assert!(!policy.is_account_allowed("acct-personal"));
    }

    #[test]
    fn folder_filtering_exclusions() {
        let mut policy = McpAgentPolicy::default();
        policy.allowed_folders = vec!["*".to_string(), "!archive".to_string(), "!trash".to_string()];

        assert!(policy.is_folder_allowed("inbox"));
        assert!(policy.is_folder_allowed("sent"));
        assert!(!policy.is_folder_allowed("archive"));
        assert!(!policy.is_folder_allowed("trash"));
    }

    #[test]
    fn sanitize_content_redacts_credit_cards_and_ssns() {
        let policy = McpAgentPolicy::default();
        let raw = "Pay with 4532-1122-3344-5566 and SSN 123-45-6789";
        let clean = policy.sanitize_content(raw);
        assert!(clean.contains("[REDACTED-CREDIT-CARD]"));
        assert!(clean.contains("[REDACTED-SSN]"));
        assert!(!clean.contains("4532-1122-3344-5566"));
        assert!(!clean.contains("123-45-6789"));
    }
}
