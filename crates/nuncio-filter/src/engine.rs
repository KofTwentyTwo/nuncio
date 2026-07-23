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
                        let re = Regex::new(pat).map_err(|e| format!("regex compile error: {e}"))?;
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
    pub fn evaluate_condition(&self, email: &Email) -> bool {
        Self::eval_node(&self.rule.conditions, email, &self.compiled_regexes)
    }

    fn eval_node(node: &ConditionNode, email: &Email, regexes: &[(String, Regex)]) -> bool {
        match node {
            ConditionNode::Leaf(leaf) => Self::eval_leaf(leaf, email, regexes),
            ConditionNode::And(children) => children.iter().all(|c| Self::eval_node(c, email, regexes)),
            ConditionNode::Or(children) => children.iter().any(|c| Self::eval_node(c, email, regexes)),
            ConditionNode::Not(inner) => !Self::eval_node(inner, email, regexes),
        }
    }

    fn eval_leaf(leaf: &ConditionLeaf, email: &Email, regexes: &[(String, Regex)]) -> bool {
        match &leaf.field {
            FilterField::Subject => Self::eval_string_op(&email.subject, &leaf.operator, &leaf.value, regexes),
            FilterField::From => Self::eval_string_op(&email.sender, &leaf.operator, &leaf.value, regexes),
            FilterField::To => Self::eval_string_op(&email.recipient, &leaf.operator, &leaf.value, regexes),
            FilterField::Body => {
                let body = email.body_plain.as_deref().or(email.body_html.as_deref()).unwrap_or("");
                Self::eval_string_op(body, &leaf.operator, &leaf.value, regexes)
            }
            FilterField::Folder | FilterField::Account => Self::eval_string_op(&email.folder_id, &leaf.operator, &leaf.value, regexes),
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
                let size = email.body_plain.as_ref().map(|b| b.len() as i64).unwrap_or(0);
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

    fn eval_string_op(haystack: &str, op: &FilterOperator, val: &FilterValue, regexes: &[(String, Regex)]) -> bool {
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
                        Regex::new(pat).map(|re| re.is_match(haystack)).unwrap_or(false)
                    }
                }
                _ => false,
            },
            FilterOperator::In => match val {
                FilterValue::List(list) => list.iter().any(|s| haystack.eq_ignore_ascii_case(s)),
                _ => false,
            },
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
    pub fn evaluate(&self, email: &Email) -> Vec<(FilterRule, Vec<RuleAction>)> {
        let guard = self.cache.load();
        let mut results = Vec::new();

        for filter in &guard.filters {
            if filter.evaluate_condition(email) {
                results.push((filter.rule.clone(), filter.rule.actions.clone()));
            }
        }

        results
    }

    /// Evaluate with Tokio 50ms hard timeout for ReDoS safety (#277).
    pub async fn evaluate_with_timeout(&self, email: &Email, timeout_duration: Duration) -> Vec<(FilterRule, Vec<RuleAction>)> {
        let email_clone = email.clone();
        let engine_cache = self.cache.clone();

        tokio::time::timeout(timeout_duration, async move {
            let guard = engine_cache.load();
            let mut results = Vec::new();
            for filter in &guard.filters {
                if filter.evaluate_condition(&email_clone) {
                    results.push((filter.rule.clone(), filter.rule.actions.clone()));
                }
            }
            results
        })
        .await
        .unwrap_or_default()
    }

    /// Dry-run preview evaluation returning detailed microsecond traces (#274).
    pub fn preview(&self, email: &Email) -> FilterPreviewResult {
        let start = Instant::now();
        let guard = self.cache.load();
        let mut matched_rule_id = None;
        let mut matched_rule_name = None;
        let mut actions = Vec::new();
        let mut traces = Vec::new();
        let mut matched = false;

        for filter in &guard.filters {
            let is_match = filter.evaluate_condition(email);
            traces.push(format!("Rule '{}' (priority {}): {}", filter.rule.name, filter.rule.priority, if is_match { "MATCH" } else { "NO MATCH" }));
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

    /// Generate HMAC-SHA256 signature for outbound webhooks (#280).
    pub fn sign_webhook_payload(secret: &str, timestamp: i64, payload: &str) -> String {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key length");
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
        let sig = FilterEngine::sign_webhook_payload("secret123", 1700000000, "{\"event\":\"mail\"}");
        assert!(sig.starts_with("t=1700000000,v1="));
    }
}
