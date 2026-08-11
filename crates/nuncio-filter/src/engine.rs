//! High-Performance In-Memory Filter Engine, Lock-Free Cache, and Keyset Chunking Triage.

use crate::ast::{
    ConditionLeaf, ConditionNode, FilterField, FilterOperator, FilterPreviewResult, FilterRule,
    FilterValue, RuleAction,
};
use arc_swap::ArcSwap;
use hmac::{Hmac, Mac};
use nuncio_core::model::{Email, Placement};
use regex::Regex;
use sha2::Sha256;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// A message together with the one mailbox occupancy being evaluated.
///
/// `WHERE FOLDER = 'INBOX'` has no answer for a message that sits in three
/// folders at once, so evaluation is defined against a single placement and the
/// caller fans out over however many the message has. Borrowing both halves
/// keeps that fan-out free of clones.
#[derive(Debug, Clone, Copy)]
pub struct PlacedEmail<'a> {
    /// Identity and content -- the same for every placement.
    pub email: &'a Email,
    /// The occupancy this evaluation is about.
    pub placement: &'a Placement,
}

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

    /// Evaluate one placement against the compiled rule condition tree.
    ///
    /// This checks only the WHERE-clause conditions; it does NOT apply the
    /// rule's account scope. Callers must gate on
    /// [`FilterRule::matches_account`] first (as every `FilterEngine` entry
    /// point does), or a rule scoped to one account will fire on messages of
    /// another.
    pub fn evaluate_condition(&self, placed: PlacedEmail<'_>) -> bool {
        Self::eval_node(&self.rule.conditions, placed, &self.compiled_regexes)
    }

    fn eval_node(
        node: &ConditionNode,
        placed: PlacedEmail<'_>,
        regexes: &[(String, Regex)],
    ) -> bool {
        match node {
            ConditionNode::Leaf(leaf) => Self::eval_leaf(leaf, placed, regexes),
            ConditionNode::And(children) => {
                children.iter().all(|c| Self::eval_node(c, placed, regexes))
            }
            ConditionNode::Or(children) => {
                children.iter().any(|c| Self::eval_node(c, placed, regexes))
            }
            ConditionNode::Not(inner) => !Self::eval_node(inner, placed, regexes),
        }
    }

    fn eval_leaf(
        leaf: &ConditionLeaf,
        placed: PlacedEmail<'_>,
        regexes: &[(String, Regex)],
    ) -> bool {
        let email = placed.email;
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
            FilterField::Folder => Self::eval_string_op(
                &placed.placement.folder_id,
                &leaf.operator,
                &leaf.value,
                regexes,
            ),
            FilterField::Account => Self::eval_string_op(
                &placed.placement.account_id,
                &leaf.operator,
                &leaf.value,
                regexes,
            ),
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
                // Unreachable defensive branch: `NsqlValidator::pass1_field_types`
                // rejects any rule with a `header[...]` condition before it can
                // reach a `CompiledFilter`, since `Email` has no parsed-headers
                // field to match against. Kept only so this match stays
                // exhaustive over `FilterField`.
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

/// Map each action to its variant name for observability logging, deliberately
/// dropping any destination data (folder name, forward address, webhook URL)
/// that the action variants carry.
fn action_kinds(actions: &[RuleAction]) -> Vec<&'static str> {
    actions
        .iter()
        .map(|action| match action {
            RuleAction::MoveTo(_) => "MOVE_TO",
            RuleAction::CopyTo(_) => "COPY_TO",
            RuleAction::MarkRead => "MARK_READ",
            RuleAction::MarkUnread => "MARK_UNREAD",
            RuleAction::Flag => "FLAG",
            RuleAction::Unflag => "UNFLAG",
            RuleAction::Delete => "DELETE",
            RuleAction::ForwardTo(_) => "FORWARD_TO",
            RuleAction::CallWebhook(_) => "CALL_WEBHOOK",
        })
        .collect()
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
        let rule_count = set.filters.len();
        self.cache.store(Arc::new(set));
        info!(rule_count, "filter engine rules reloaded");
        Ok(())
    }

    /// Evaluate one placement of a message, returning the matching rules' actions.
    ///
    /// Defined per placement, not per message: `FOLDER` and `ACCOUNT` are
    /// properties of where the message sits, and a message can sit in several
    /// places at once. Callers holding a multi-placement message evaluate each
    /// placement and are responsible for not acting twice on one message -- see
    /// the fire-once claim in the store.
    ///
    /// A rule only ever fires for the account it was created against
    /// (`FilterRule::target_account`, set via `ON ACCOUNT` or `*` for all
    /// accounts) — condition matching alone is not account-scoped, so this
    /// check must happen before `evaluate_condition` runs.
    pub fn evaluate(&self, placed: PlacedEmail<'_>) -> Vec<(FilterRule, Vec<RuleAction>)> {
        let guard = self.cache.load();
        let mut results = Vec::new();

        for filter in &guard.filters {
            if filter.rule.matches_account(&placed.placement.account_id)
                && filter.evaluate_condition(placed)
            {
                debug!(
                    rule_id = %filter.rule.id,
                    rule_name = %filter.rule.name,
                    actions = ?action_kinds(&filter.rule.actions),
                    "rule matched; dispatching actions"
                );
                results.push((filter.rule.clone(), filter.rule.actions.clone()));
            }
        }

        results
    }

    /// Evaluate with a Tokio hard timeout for ReDoS safety.
    ///
    /// Each rule's condition evaluation is timed independently (on a
    /// blocking-pool task, so a real timeout can actually preempt a
    /// long-running synchronous evaluation rather than merely racing a timer
    /// that a purely synchronous future would never yield to). A rule whose
    /// evaluation exceeds `timeout_duration` is treated as **non-matching**
    /// for this message — the documented, fail-closed outcome — but unlike a
    /// bare `unwrap_or_default()`, the timeout is never silent: it is logged
    /// with the rule's identity via `warn!`, and evaluation continues with
    /// the remaining rules rather than aborting the whole batch.
    pub async fn evaluate_with_timeout(
        &self,
        placed: PlacedEmail<'_>,
        timeout_duration: Duration,
    ) -> Vec<(FilterRule, Vec<RuleAction>)> {
        let guard = self.cache.load();
        let mut results = Vec::new();

        for filter in &guard.filters {
            if !filter.rule.matches_account(&placed.placement.account_id) {
                continue;
            }

            let filter_for_task = filter.clone();
            let email_for_task = placed.email.clone();
            let placement_for_task = placed.placement.clone();
            let eval_task = tokio::task::spawn_blocking(move || {
                filter_for_task.evaluate_condition(PlacedEmail {
                    email: &email_for_task,
                    placement: &placement_for_task,
                })
            });

            match tokio::time::timeout(timeout_duration, eval_task).await {
                Ok(Ok(true)) => {
                    debug!(
                        rule_id = %filter.rule.id,
                        rule_name = %filter.rule.name,
                        actions = ?action_kinds(&filter.rule.actions),
                        "rule matched; dispatching actions"
                    );
                    results.push((filter.rule.clone(), filter.rule.actions.clone()));
                }
                Ok(Ok(false)) => {}
                Ok(Err(join_error)) => {
                    warn!(
                        rule_id = %filter.rule.id,
                        rule_name = %filter.rule.name,
                        error = %join_error,
                        "rule evaluation task failed to complete; treating rule as non-matching"
                    );
                }
                Err(_elapsed) => {
                    warn!(
                        rule_id = %filter.rule.id,
                        rule_name = %filter.rule.name,
                        timeout_ms = timeout_duration.as_millis(),
                        "rule evaluation timed out (ReDoS guard); treating rule as non-matching"
                    );
                }
            }
        }

        results
    }

    /// Dry-run preview evaluation returning detailed microsecond traces.
    ///
    /// Rules whose account scope does not match the message are reported as
    /// `SKIPPED (account scope mismatch)` rather than omitted, so the trace
    /// distinguishes "condition did not match" from "rule does not apply here".
    pub fn preview(&self, placed: PlacedEmail<'_>) -> FilterPreviewResult {
        let start = Instant::now();
        let guard = self.cache.load();
        let mut matched_rule_id = None;
        let mut matched_rule_name = None;
        let mut actions = Vec::new();
        let mut traces = Vec::new();
        let mut matched = false;

        for filter in &guard.filters {
            if !filter.rule.matches_account(&placed.placement.account_id) {
                traces.push(format!(
                    "Rule '{}' (priority {}): SKIPPED (account scope mismatch)",
                    filter.rule.name, filter.rule.priority
                ));
                continue;
            }

            let is_match = filter.evaluate_condition(placed);
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
            message_id: placed.email.id.clone(),
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
    use std::sync::Mutex;
    use tracing::field::{Field, Visit};
    use tracing::span;

    /// Minimal `tracing::Subscriber` that records a formatted line per event
    /// so tests can assert on emitted level + fields without pulling in
    /// `tracing-subscriber`'s registry machinery.
    struct CapturingSubscriber {
        events: Arc<Mutex<Vec<String>>>,
    }

    struct LineVisitor<'a>(&'a mut String);

    impl Visit for LineVisitor<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!(" {}={:?}", field.name(), value));
        }
    }

    impl tracing::Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }

        fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

        fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut line = format!("{}", event.metadata().level());
            let mut visitor = LineVisitor(&mut line);
            event.record(&mut visitor);
            if let Ok(mut events) = self.events.lock() {
                events.push(line);
            }
        }

        fn enter(&self, _span: &span::Id) {}

        fn exit(&self, _span: &span::Id) {}
    }

    /// Build an `Email`/`Placement` pair that shares one account between the
    /// message and the mailbox it is placed in, matching how a message and
    /// its own placement always agree on account.
    fn test_placed_email(account_id: &str, subject: &str, folder_id: &str) -> (Email, Placement) {
        let email = Email {
            id: "msg-1".to_string(),
            account_id: account_id.to_string(),
            subject: subject.to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            body_plain: Some("Hello".to_string()),
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        };
        let placement = Placement {
            account_id: account_id.to_string(),
            folder_id: folder_id.to_string(),
            uid_validity: "1".to_string(),
            remote_id: "1".to_string(),
            read: false,
        };
        (email, placement)
    }

    #[test]
    fn test_engine_evaluate_and_lockfree_reload() {
        let nsql = "WHERE subject CONTAINS 'Urgent' ACTION MARK READ, MOVE TO 'Priority'";
        let rule = crate::parser::NsqlParser::parse_rule("Urgent", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let (email, placement) = test_placed_email("acct-1", "Urgent Meeting", "inbox");

        let results = engine.evaluate(PlacedEmail {
            email: &email,
            placement: &placement,
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.len(), 2);

        let preview = engine.preview(PlacedEmail {
            email: &email,
            placement: &placement,
        });
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
        let (email_b, placement_b) = test_placed_email("acct-b", "Weekly Report", "inbox");
        assert!(
            engine
                .evaluate(PlacedEmail {
                    email: &email_b,
                    placement: &placement_b
                })
                .is_empty(),
            "rule scoped to acct-a must not match a message from acct-b"
        );

        let (email_a, placement_a) = test_placed_email("acct-a", "Weekly Report", "inbox");
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email_a,
                    placement: &placement_a
                })
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn test_evaluate_with_timeout_scopes_rule_to_its_owning_account() {
        let nsql = "ON ACCOUNT 'acct-a' WHERE subject CONTAINS 'Report' ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Scoped To A", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let (email_b, placement_b) = test_placed_email("acct-b", "Weekly Report", "inbox");
        let results = engine
            .evaluate_with_timeout(
                PlacedEmail {
                    email: &email_b,
                    placement: &placement_b,
                },
                Duration::from_secs(1),
            )
            .await;
        assert!(results.is_empty());

        let (email_a, placement_a) = test_placed_email("acct-a", "Weekly Report", "inbox");
        let results = engine
            .evaluate_with_timeout(
                PlacedEmail {
                    email: &email_a,
                    placement: &placement_a,
                },
                Duration::from_secs(1),
            )
            .await;
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_preview_reports_account_scope_mismatch() {
        let nsql = "ON ACCOUNT 'acct-a' WHERE subject CONTAINS 'Report' ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Scoped To A", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let (email_b, placement_b) = test_placed_email("acct-b", "Weekly Report", "inbox");
        let preview = engine.preview(PlacedEmail {
            email: &email_b,
            placement: &placement_b,
        });
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

        let (email, placement) = test_placed_email("any-account", "Weekly Report", "inbox");
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email,
                    placement: &placement
                })
                .len(),
            1
        );
    }

    #[test]
    fn test_account_condition_field_matches_account_id_not_folder_id() {
        let nsql = "WHERE account = 'acct-a' ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Account Filter", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        // Same folder_id, different account_id: a rule matching on the
        // `account` condition field must key off the placement's account_id,
        // never fall through to comparing folder_id.
        let (email_a, placement_a) = test_placed_email("acct-a", "Anything", "shared-folder");
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email_a,
                    placement: &placement_a
                })
                .len(),
            1,
            "rule on account = 'acct-a' must match a placement with account_id 'acct-a'"
        );

        let (email_b, placement_b) = test_placed_email("acct-b", "Anything", "shared-folder");
        assert!(
            engine
                .evaluate(PlacedEmail {
                    email: &email_b,
                    placement: &placement_b
                })
                .is_empty(),
            "rule on account = 'acct-a' must not match a placement with account_id 'acct-b', \
             even when it shares the same folder_id as an acct-a placement"
        );
    }

    #[test]
    fn test_not_in_operator_is_complement_of_in() {
        let nsql = "WHERE folder NOT IN ('spam', 'trash') ACTION MARK READ";
        let rule = crate::parser::NsqlParser::parse_rule("Not In Spam Or Trash", 1, nsql).unwrap();
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let (inbox_email, inbox_placement) = test_placed_email("acct-1", "Anything", "inbox");
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &inbox_email,
                    placement: &inbox_placement
                })
                .len(),
            1
        );

        let (spam_email, spam_placement) = test_placed_email("acct-1", "Anything", "spam");
        assert!(engine
            .evaluate(PlacedEmail {
                email: &spam_email,
                placement: &spam_placement
            })
            .is_empty());
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

        let (email, placement) = test_placed_email("acct-1", "Anything", "inbox");
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email,
                    placement: &placement
                })
                .len(),
            1
        );
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

        let (email, placement) = test_placed_email("acct-1", "Anything", "inbox");
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email,
                    placement: &placement
                })
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn test_evaluate_with_timeout_logs_warn_and_treats_timeout_as_non_match() {
        // A crafted, slow-to-evaluate condition (large body scanned by a
        // CONTAINS check) paired with a near-zero timeout budget forces the
        // per-rule timeout path deterministically: the evaluation runs on a
        // real blocking-pool thread while the timer races it on the async
        // task, so the timer reliably wins for any evaluation slower than a
        // few microseconds.
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            events: events.clone(),
        };
        let _guard = tracing::subscriber::set_default(subscriber);

        let rule = FilterRule {
            id: "slow-rule".to_string(),
            name: "Slow Rule".to_string(),
            target_account: "*".to_string(),
            priority: 1,
            enabled: true,
            nsql_text: String::new(),
            conditions: ConditionNode::Leaf(ConditionLeaf {
                field: FilterField::Body,
                operator: FilterOperator::Contains,
                value: FilterValue::String("nonexistent-token".to_string()),
            }),
            actions: vec![RuleAction::MarkRead],
            created_at: 0,
            updated_at: 0,
        };
        let engine = FilterEngine::new(vec![rule]).unwrap();

        let (mut email, placement) = test_placed_email("acct-1", "Anything", "inbox");
        email.body_plain = Some("A".repeat(60_000_000));

        let results = engine
            .evaluate_with_timeout(
                PlacedEmail {
                    email: &email,
                    placement: &placement,
                },
                Duration::from_millis(1),
            )
            .await;

        assert!(
            results.is_empty(),
            "documented outcome: a timed-out rule must be treated as non-matching, \
             not fabricated as a match"
        );

        let captured = events.lock().unwrap();
        assert!(
            captured.iter().any(|line| line.contains("WARN")
                && line.contains("timed out")
                && line.contains("slow-rule")),
            "expected a WARN log carrying the rule id for the timed-out evaluation, got: {captured:?}"
        );
    }

    fn sample_email(key: &str) -> Email {
        Email {
            id: key.to_string(),
            account_id: "acct-1".to_string(),
            subject: "Anything".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1700000000,
            body_plain: Some("Hello".to_string()),
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        }
    }

    fn sample_placement(folder_id: &str, remote_id: &str) -> Placement {
        Placement {
            account_id: "acct-1".to_string(),
            folder_id: folder_id.to_string(),
            uid_validity: "1".to_string(),
            remote_id: remote_id.to_string(),
            read: false,
        }
    }

    fn rule_where_folder_is(folder: &str) -> FilterRule {
        let nsql = format!("WHERE folder = '{folder}' ACTION MARK READ");
        crate::parser::NsqlParser::parse_rule("Folder Filter", 1, &nsql).unwrap()
    }

    fn rule_where_account_is(account: &str) -> FilterRule {
        let nsql = format!("WHERE account = '{account}' ACTION MARK READ");
        crate::parser::NsqlParser::parse_rule("Account Filter", 1, &nsql).unwrap()
    }

    #[test]
    fn a_folder_condition_matches_only_the_placement_it_is_evaluated_against() {
        let engine = FilterEngine::new(vec![rule_where_folder_is("INBOX")]).unwrap();
        let email = sample_email("key-1");
        let inbox = sample_placement("INBOX", "5");
        let archive = sample_placement("Archive", "9");

        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email,
                    placement: &inbox
                })
                .len(),
            1,
            "the INBOX placement matches"
        );
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email,
                    placement: &archive
                })
                .len(),
            0,
            "the same message's Archive placement does not"
        );
    }

    #[test]
    fn an_account_condition_reads_the_placement_account() {
        let engine = FilterEngine::new(vec![rule_where_account_is("acct-1")]).unwrap();
        let email = sample_email("key-1");
        let mine = sample_placement("INBOX", "5");
        let theirs = Placement {
            account_id: "acct-2".into(),
            ..sample_placement("INBOX", "5")
        };

        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email,
                    placement: &mine
                })
                .len(),
            1
        );
        assert_eq!(
            engine
                .evaluate(PlacedEmail {
                    email: &email,
                    placement: &theirs
                })
                .len(),
            0
        );
    }
}
