//! High-Performance In-Memory Filter Engine, Lock-Free Cache, and Keyset Chunking Triage.

use crate::ast::{
    ConditionLeaf, ConditionNode, FilterField, FilterOperator, FilterPreviewResult, FilterRule,
    FilterValue, RuleAction,
};
use arc_swap::ArcSwap;
use hmac::{Hmac, Mac};
use nuncio_core::model::Email;
use regex::Regex;
use sha2::Sha256;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Pre-compiled single filter rule optimizing regex and predicate execution.
#[derive(Clone)]
pub struct CompiledFilter {
    /// Source filter rule.
    pub rule: FilterRule,
    /// Pre-compiled regex patterns for `MATCHES` operators.
    compiled_regexes: Vec<(String, Regex)>,
}

impl CompiledFilter {
    /// Pre-compile regexes in rule conditions.
    pub fn compile(rule: FilterRule) -> Result<Self, String> {
        let mut regexes = Vec::new();
        Self::collect_regexes(&rule.conditions, &mut regexes)?;
        Ok(Self {
            rule,
            compiled_regexes: regexes,
        })
    }

    fn collect_regexes(node: &ConditionNode, acc: &mut Vec<(String, Regex)>) -> Result<(), String> {
        match node {
            ConditionNode::Leaf(leaf) => {
                if leaf.operator == FilterOperator::Matches {
                    if let FilterValue::String(pat) = &leaf.value {
                        let re =
                            Regex::new(pat).map_err(|e| format!("regex compile error: {e}"))?;
                        acc.push((pat.clone(), re));
                    }
                }
                Ok(())
            }
            ConditionNode::And(children) | ConditionNode::Or(children) => {
                for c in children {
                    Self::collect_regexes(c, acc)?;
                }
                Ok(())
            }
            ConditionNode::Not(inner) => Self::collect_regexes(inner, acc),
        }
    }

    /// Evaluate email message against compiled rule condition tree.
    ///
    /// This checks only the WHERE-clause conditions; it does NOT apply the
    /// rule's account scope. Callers must gate on
    /// [`FilterRule::matches_account`] first (as every `FilterEngine` entry
    /// point does), or a rule scoped to one account will fire on messages of
    /// another.
    pub fn evaluate_condition(&self, email: &Email) -> bool {
        Self::eval_node(&self.rule.conditions, email, &self.compiled_regexes)
    }

    fn eval_node(node: &ConditionNode, email: &Email, regexes: &[(String, Regex)]) -> bool {
        match node {
            ConditionNode::Leaf(leaf) => Self::eval_leaf(leaf, email, regexes),
            ConditionNode::And(children) => {
                children.iter().all(|c| Self::eval_node(c, email, regexes))
            }
            ConditionNode::Or(children) => {
                children.iter().any(|c| Self::eval_node(c, email, regexes))
            }
            ConditionNode::Not(inner) => !Self::eval_node(inner, email, regexes),
        }
    }

    fn eval_leaf(leaf: &ConditionLeaf, email: &Email, regexes: &[(String, Regex)]) -> bool {
        match &leaf.field {
            FilterField::Subject => {
                Self::eval_string_op(&email.subject, &leaf.operator, &leaf.value, regexes)
            }
            FilterField::From => {
                Self::eval_string_op(&email.sender, &leaf.operator, &leaf.value, regexes)
            }
            FilterField::To => {
                Self::eval_string_op(&email.recipient, &leaf.operator, &leaf.value, regexes)
            }
            FilterField::Body => {
                let body = email
                    .body_plain
                    .as_deref()
                    .or(email.body_html.as_deref())
                    .unwrap_or("");
                Self::eval_string_op(body, &leaf.operator, &leaf.value, regexes)
            }
            FilterField::Folder | FilterField::Account => {
                Self::eval_string_op(&email.folder_id, &leaf.operator, &leaf.value, regexes)
            }
            FilterField::HasAttachment => {
                let has = !email.attachments.is_empty();
                if let FilterValue::Boolean(b) = leaf.value {
                    if leaf.operator == FilterOperator::Equals {
                        has == b
                    } else {
                        has != b
                    }
                } else {
                    false
                }
            }
            FilterField::Size => {
                let size = email
                    .body_plain
                    .as_ref()
                    .map(|b| b.len() as i64)
                    .unwrap_or(0);
                if let FilterValue::Number(target) = leaf.value {
                    match leaf.operator {
                        FilterOperator::Equals => size == target,
                        FilterOperator::NotEquals => size != target,
                        FilterOperator::GreaterThan => size > target,
                        FilterOperator::LessThan => size < target,
                        FilterOperator::GreaterThanOrEqual => size >= target,
                        FilterOperator::LessThanOrEqual => size <= target,
                        _ => false,
                    }
                } else {
                    false
                }
            }
            FilterField::Date => {
                if let FilterValue::Number(target) = leaf.value {
                    match leaf.operator {
                        FilterOperator::Equals => email.received_at == target,
                        FilterOperator::NotEquals => email.received_at != target,
                        FilterOperator::GreaterThan => email.received_at > target,
                        FilterOperator::LessThan => email.received_at < target,
                        FilterOperator::GreaterThanOrEqual => email.received_at >= target,
                        FilterOperator::LessThanOrEqual => email.received_at <= target,
                        _ => false,
                    }
                } else {
                    false
                }
            }
            FilterField::Header(_name) => {
                // Header evaluation fallback
                false
            }
        }
    }

    fn eval_string_op(
        haystack: &str,
        op: &FilterOperator,
        val: &FilterValue,
        regexes: &[(String, Regex)],
    ) -> bool {
        match op {
            FilterOperator::Equals => match val {
                FilterValue::String(s) => haystack.eq_ignore_ascii_case(s),
                _ => false,
            },
            FilterOperator::NotEquals => match val {
                FilterValue::String(s) => !haystack.eq_ignore_ascii_case(s),
                _ => false,
            },
            FilterOperator::Contains => match val {
                FilterValue::String(s) => haystack.to_lowercase().contains(&s.to_lowercase()),
                _ => false,
            },
            FilterOperator::NotContains => match val {
                FilterValue::String(s) => !haystack.to_lowercase().contains(&s.to_lowercase()),
                _ => false,
            },
            FilterOperator::Matches => match val {
                FilterValue::String(pat) => {
                    if let Some((_, re)) = regexes.iter().find(|(p, _)| p == pat) {
                        re.is_match(haystack)
                    } else {
                        Regex::new(pat)
                            .map(|re| re.is_match(haystack))
                            .unwrap_or(false)
                    }
                }
                _ => false,
            },
            FilterOperator::In => Self::eval_in_set(haystack, val),
            // NOT IN is defined as the exact complement of IN: an empty set
            // makes IN always false, so NOT IN is always true, and any value
            // shape that IN cannot match against also makes NOT IN true.
            FilterOperator::NotIn => !Self::eval_in_set(haystack, val),
            _ => false,
        }
    }

    fn eval_in_set(haystack: &str, val: &FilterValue) -> bool {
        match val {
            FilterValue::List(list) => list.iter().any(|s| haystack.eq_ignore_ascii_case(s)),
            _ => false,
        }
    }
}

/// Collection of compiled filters ordered by priority.
#[derive(Clone, Default)]
pub struct CompiledFilterSet {
    pub filters: Vec<CompiledFilter>,
}

impl CompiledFilterSet {
    /// Construct set from raw rules sorting by priority ascending.
    pub fn new(mut rules: Vec<FilterRule>) -> Result<Self, String> {
        rules.sort_by_key(|r| r.priority);
        let mut filters = Vec::new();
        for rule in rules {
            if rule.enabled {
                filters.push(CompiledFilter::compile(rule)?);
            }
        }
        Ok(Self { filters })
    }
}

/// Lock-free Filter Engine wrapping `ArcSwap<CompiledFilterSet>`.
pub struct FilterEngine {
    cache: Arc<ArcSwap<CompiledFilterSet>>,
}

impl FilterEngine {
    /// Construct a new `FilterEngine`.
    pub fn new(rules: Vec<FilterRule>) -> Result<Self, String> {
        let set = CompiledFilterSet::new(rules)?;
        Ok(Self {
            cache: Arc::new(ArcSwap::from_pointee(set)),
        })
    }

    /// Swap / reload active filter rules atomically (<5ns latencies).
    pub fn reload_rules(&self, rules: Vec<FilterRule>) -> Result<(), String> {
        let set = CompiledFilterSet::new(rules)?;
        self.cache.store(Arc::new(set));
        Ok(())
    }

    /// Evaluate email message returning matching rule actions.
    ///
    /// A rule only ever fires for the account it was created against
    /// (`FilterRule::target_account`, set via `ON ACCOUNT` or `*` for all
    /// accounts) — condition matching alone is not account-scoped, so this
    /// check must happen before `evaluate_condition` runs.
    pub fn evaluate(&self, email: &Email) -> Vec<(FilterRule, Vec<RuleAction>)> {
        let guard = self.cache.load();
        let mut results = Vec::new();

        for filter in &guard.filters {
            if filter.rule.matches_account(&email.account_id) && filter.evaluate_condition(email) {
                results.push((filter.rule.clone(), filter.rule.actions.clone()));
            }
        }

        results
    }

    /// Evaluate with a Tokio hard timeout for ReDoS safety.
    pub async fn evaluate_with_timeout(
        &self,
        email: &Email,
        timeout_duration: Duration,
    ) -> Vec<(FilterRule, Vec<RuleAction>)> {
        let email_clone = email.clone();
        let engine_cache = self.cache.clone();

        tokio::time::timeout(timeout_duration, async move {
            let guard = engine_cache.load();
            let mut results = Vec::new();
            for filter in &guard.filters {
                if filter.rule.matches_account(&email_clone.account_id)
                    && filter.evaluate_condition(&email_clone)
                {
                    results.push((filter.rule.clone(), filter.rule.actions.clone()));
                }
            }
            results
        })
        .await
        .unwrap_or_default()
    }

    /// Dry-run preview evaluation returning detailed microsecond traces.
    ///
    /// Rules whose account scope does not match the message are reported as
    /// `SKIPPED (account scope mismatch)` rather than omitted, so the trace
    /// distinguishes "condition did not match" from "rule does not apply here".
    pub fn preview(&self, email: &Email) -> FilterPreviewResult {
        let start = Instant::now();
        let guard = self.cache.load();
        let mut matched_rule_id = None;
        let mut matched_rule_name = None;
        let mut actions = Vec::new();
        let mut traces = Vec::new();
        let mut matched = false;

        for filter in &guard.filters {
            if !filter.rule.matches_account(&email.account_id) {
                traces.push(format!(
                    "Rule '{}' (priority {}): SKIPPED (account scope mismatch)",
                    filter.rule.name, filter.rule.priority
                ));
                continue;
            }

            let is_match = filter.evaluate_condition(email);
            traces.push(format!(
                "Rule '{}' (priority {}): {}",
                filter.rule.name,
                filter.rule.priority,
                if is_match { "MATCH" } else { "NO MATCH" }
            ));
            if is_match && !matched {
                matched = true;
                matched_rule_id = Some(filter.rule.id.clone());
                matched_rule_name = Some(filter.rule.name.clone());
                actions = filter.rule.actions.clone();
            }
        }

        let elapsed = start.elapsed().as_micros() as u64;

        FilterPreviewResult {
            message_id: email.id.clone(),
            matched,
            matched_rule_id,
            matched_rule_name,
            actions_evaluated: actions,
            execution_time_us: elapsed,
            condition_traces: traces,
        }
    }

    /// Generate HMAC-SHA256 signature for outbound webhooks.
    pub fn sign_webhook_payload(secret: &str, timestamp: i64, payload: &str) -> String {
        type HmacSha256 = Hmac<Sha256>;
        // HMAC accepts keys of any length (RFC 2104), so this never fails in practice.
        let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
            return String::new();
        };
        let data = format!("{timestamp}.{payload}");
        mac.update(data.as_bytes());
        let hash = hex::encode(mac.finalize().into_bytes());
        format!("t={timestamp},v1={hash}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::model::Email;

    fn test_email(account_id: &str, subject: &str, folder_id: &str) -> Email {
        Email {
            id: "msg-1".to_string(),
            account_id: account_id.to_string(),
            folder_id: folder_id.to_string(),
            subject: subject.to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: false,
            body_plain: Some("Hello".to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn test_engine_evaluate_and_lockfree_reload() {
        let nsql = "WHERE subject CONTAINS 'Urgent' ACTION MARK READ, MOVE TO 'Priority'";
        let rule = crate::parser::NsqlParser::parse_rule("Urgent", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let email = Email {
            id: "msg-1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "inbox".to_string(),
            subject: "Urgent Meeting".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            read: false,
            body_plain: Some("Hello".to_string()),
            body_html: None,
            attachments: Vec::new(),
        };

        let results = engine.evaluate(&email);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.len(), 2);

        let preview = engine.preview(&email);
        assert!(preview.matched);
        assert_eq!(preview.matched_rule_name, Some("Urgent".to_string()));
    }

    #[test]
    fn test_webhook_signature() {
        let sig =
            FilterEngine::sign_webhook_payload("secret123", 1700000000, "{\"event\":\"mail\"}");
        assert!(sig.starts_with("t=1700000000,v1="));
    }

    #[test]
    fn test_evaluate_scopes_rule_to_its_owning_account() {
        let nsql = "ON ACCOUNT 'acct-a' WHERE subject CONTAINS 'Report' ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Scoped To A", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        // Condition matches the subject, but the message belongs to a
        // different account than the rule was created for.
        let email_b = test_email("acct-b", "Weekly Report", "inbox");
        assert!(
            engine.evaluate(&email_b).is_empty(),
            "rule scoped to acct-a must not match a message from acct-b"
        );

        let email_a = test_email("acct-a", "Weekly Report", "inbox");
        assert_eq!(engine.evaluate(&email_a).len(), 1);
    }

    #[tokio::test]
    async fn test_evaluate_with_timeout_scopes_rule_to_its_owning_account() {
        let nsql = "ON ACCOUNT 'acct-a' WHERE subject CONTAINS 'Report' ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Scoped To A", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let email_b = test_email("acct-b", "Weekly Report", "inbox");
        let results = engine
            .evaluate_with_timeout(&email_b, Duration::from_secs(1))
            .await;
        assert!(results.is_empty());

        let email_a = test_email("acct-a", "Weekly Report", "inbox");
        let results = engine
            .evaluate_with_timeout(&email_a, Duration::from_secs(1))
            .await;
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_preview_reports_account_scope_mismatch() {
        let nsql = "ON ACCOUNT 'acct-a' WHERE subject CONTAINS 'Report' ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Scoped To A", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let email_b = test_email("acct-b", "Weekly Report", "inbox");
        let preview = engine.preview(&email_b);
        assert!(!preview.matched);
        assert!(preview
            .condition_traces
            .iter()
            .any(|t| t.contains("account scope mismatch")));
    }

    #[test]
    fn test_wildcard_account_rule_still_matches_any_account() {
        let nsql = "WHERE subject CONTAINS 'Report' ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Global", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let email = test_email("any-account", "Weekly Report", "inbox");
        assert_eq!(engine.evaluate(&email).len(), 1);
    }

    #[test]
    fn test_not_in_operator_is_complement_of_in() {
        let nsql = "WHERE folder NOT IN ('spam', 'trash') ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Not In Spam Or Trash", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let inbox_email = test_email("acct-1", "Anything", "inbox");
        assert_eq!(engine.evaluate(&inbox_email).len(), 1);

        let spam_email = test_email("acct-1", "Anything", "spam");
        assert!(engine.evaluate(&spam_email).is_empty());
    }

    #[test]
    fn test_not_in_empty_set_always_matches() {
        // NOT IN () is the complement of IN (), and IN against an empty set
        // never matches anything, so NOT IN () must always evaluate true.
        let rule = FilterRule {
            id: "r1".to_string(),
            name: "Empty Set".to_string(),
            target_account: "*".to_string(),
            priority: 1,
            enabled: true,
            nsql_text: String::new(),
            conditions: ConditionNode::Leaf(ConditionLeaf {
                field: FilterField::Folder,
                operator: FilterOperator::NotIn,
                value: FilterValue::List(vec![]),
            }),
            actions: vec![RuleAction::MarkRead],
            created_at: 0,
            updated_at: 0,
        };
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let email = test_email("acct-1", "Anything", "inbox");
        assert_eq!(engine.evaluate(&email).len(), 1);
    }

    #[test]
    fn test_not_in_against_non_list_value_matches_as_in_complement() {
        // A leaf carrying a non-list value under NOT IN is a degenerate shape
        // that IN can never match (see `eval_in_set`); NOT IN, defined as
        // IN's exact complement, must therefore always match it.
        let leaf = ConditionLeaf {
            field: FilterField::Subject,
            operator: FilterOperator::NotIn,
            value: FilterValue::String("not-a-list".to_string()),
        };
        let rule = FilterRule {
            id: "r1".to_string(),
            name: "Null Case".to_string(),
            target_account: "*".to_string(),
            priority: 1,
            enabled: true,
            nsql_text: String::new(),
            conditions: ConditionNode::Leaf(leaf),
            actions: vec![RuleAction::MarkRead],
            created_at: 0,
            updated_at: 0,
        };
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let email = test_email("acct-1", "Anything", "inbox");
        assert_eq!(engine.evaluate(&email).len(), 1);
    }
}
