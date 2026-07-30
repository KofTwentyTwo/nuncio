//! 6-Pass `NsqlValidator` type checker and security auditor for NSQL rules.

use crate::ast::*;
use thiserror::Error;

/// Validation error categories emitted across the 6 audit passes.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// Pass 1: Field type mismatch error.
    #[error("Pass 1 (Field Types Error): {0}")]
    FieldTypeMismatch(String),
    /// Pass 2: Folder existence or reference error.
    #[error("Pass 2 (Folder Reference Error): {0}")]
    InvalidFolder(String),
    /// Pass 3: RFC 5322 email address format violation.
    #[error("Pass 3 (RFC 5322 Format Error): {0}")]
    InvalidEmailFormat(String),
    /// Pass 4: ReDoS pattern vulnerability or AST depth exceeded.
    #[error("Pass 4 (ReDoS & AST Depth Error): {0}")]
    ReDoSOrDepth(String),
    #[error("Pass 5 (Contradiction Error): {0}")]
    /// Pass 5: Contradiction or impossible logical condition detected.
    ContradictionDetected(String),
    /// Pass 6: Action security policy violation.
    #[error("Pass 6 (Action Security Error): {0}")]
    ActionSecurityViolation(String),
}

/// Configuration parameters for `NsqlValidator`.
#[derive(Debug, Clone)]
pub struct ValidationOptions {
    /// Optional list of known folder names for Pass 2 verification.
    pub available_folders: Option<Vec<String>>,
    /// Optional domain whitelist for Pass 6 `FORWARD TO` actions.
    pub allowed_forward_domains: Option<Vec<String>>,
    /// When set, reject webhook targets that point at private, loopback,
    /// link-local, or metadata address ranges. The validator checks literal-IP
    /// hosts; the dispatcher additionally checks addresses a hostname resolves
    /// to before connecting.
    pub block_private_webhooks: bool,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            available_folders: None,
            allowed_forward_domains: None,
            block_private_webhooks: true,
        }
    }
}

/// 6-Pass Validator enforcing type safety, ReDoS immunization, and security compliance.
pub struct NsqlValidator;

impl NsqlValidator {
    /// Execute all 6 validation passes sequentially against a `FilterRule`.
    pub fn validate(rule: &FilterRule, opts: &ValidationOptions) -> Result<(), ValidationError> {
        Self::pass1_field_types(&rule.conditions)?;
        Self::pass2_folder_references(
            &rule.conditions,
            &rule.actions,
            opts.available_folders.as_deref(),
        )?;
        Self::pass3_rfc5322(&rule.conditions, &rule.actions)?;
        Self::pass4_redos_and_depth(&rule.conditions)?;
        Self::pass5_contradictions(&rule.conditions)?;
        Self::pass6_action_security(&rule.actions, opts)?;
        Ok(())
    }

    /// Pass 1: Field Types Validation.
    fn pass1_field_types(node: &ConditionNode) -> Result<(), ValidationError> {
        match node {
            ConditionNode::Leaf(leaf) => match &leaf.field {
                FilterField::Size | FilterField::Date => match &leaf.value {
                    FilterValue::Number(_) => Ok(()),
                    _ => Err(ValidationError::FieldTypeMismatch(format!(
                        "field {:?} requires a numeric integer value",
                        leaf.field
                    ))),
                },
                FilterField::HasAttachment => match &leaf.value {
                    FilterValue::Boolean(_) => Ok(()),
                    _ => Err(ValidationError::FieldTypeMismatch(
                        "field 'has_attachment' requires a boolean value".to_string(),
                    )),
                },
                FilterField::Header(_) => Err(ValidationError::FieldTypeMismatch(
                    "header field conditions are not yet supported: message headers are not indexed"
                        .to_string(),
                )),
                FilterField::Subject
                | FilterField::From
                | FilterField::To
                | FilterField::Body
                | FilterField::Folder
                | FilterField::Account => match &leaf.value {
                    FilterValue::String(_) | FilterValue::List(_) => Ok(()),
                    _ => Err(ValidationError::FieldTypeMismatch(format!(
                        "field {:?} requires a string or string list value",
                        leaf.field
                    ))),
                },
            },
            ConditionNode::And(children) | ConditionNode::Or(children) => {
                for c in children {
                    Self::pass1_field_types(c)?;
                }
                Ok(())
            }
            ConditionNode::Not(inner) => Self::pass1_field_types(inner),
        }
    }

    /// Pass 2: Folder Existence & Reference Validation.
    fn pass2_folder_references(
        _node: &ConditionNode,
        actions: &[RuleAction],
        available_folders: Option<&[String]>,
    ) -> Result<(), ValidationError> {
        for action in actions {
            if let RuleAction::MoveTo(target) = action {
                if target.trim().is_empty() {
                    return Err(ValidationError::InvalidFolder(
                        "target folder name cannot be empty".to_string(),
                    ));
                }
                if let Some(folders) = available_folders {
                    if !folders.iter().any(|f| f.eq_ignore_ascii_case(target)) {
                        return Err(ValidationError::InvalidFolder(format!(
                            "target folder '{target}' does not exist in available folders"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Pass 3: RFC 5322 Email Format Validation.
    fn pass3_rfc5322(node: &ConditionNode, actions: &[RuleAction]) -> Result<(), ValidationError> {
        for action in actions {
            if let RuleAction::ForwardTo(email) = action {
                if !is_valid_email_format(email) {
                    return Err(ValidationError::InvalidEmailFormat(format!(
                        "FORWARD TO email '{email}' violates RFC 5322 format"
                    )));
                }
            }
        }
        Self::check_condition_emails(node)
    }

    fn check_condition_emails(node: &ConditionNode) -> Result<(), ValidationError> {
        match node {
            ConditionNode::Leaf(leaf) => {
                if (leaf.field == FilterField::From || leaf.field == FilterField::To)
                    && leaf.operator == FilterOperator::Equals
                {
                    if let FilterValue::String(s) = &leaf.value {
                        if !is_valid_email_format(s) {
                            return Err(ValidationError::InvalidEmailFormat(format!(
                                "email equality value '{s}' violates RFC 5322 format"
                            )));
                        }
                    }
                }
                Ok(())
            }
            ConditionNode::And(children) | ConditionNode::Or(children) => {
                for c in children {
                    Self::check_condition_emails(c)?;
                }
                Ok(())
            }
            ConditionNode::Not(inner) => Self::check_condition_emails(inner),
        }
    }

    /// Pass 4: ReDoS Pattern Vulnerability & AST Recursion Depth Check.
    fn pass4_redos_and_depth(node: &ConditionNode) -> Result<(), ValidationError> {
        if node.depth() > 10 {
            return Err(ValidationError::ReDoSOrDepth(
                "AST depth exceeds maximum allowed threshold of 10".to_string(),
            ));
        }
        Self::check_regex_patterns(node)
    }

    fn check_regex_patterns(node: &ConditionNode) -> Result<(), ValidationError> {
        match node {
            ConditionNode::Leaf(leaf) => {
                if leaf.operator == FilterOperator::Matches {
                    if let FilterValue::String(pattern) = &leaf.value {
                        if pattern.contains("+)+")
                            || pattern.contains("*)*")
                            || pattern.contains("+)*")
                            || pattern.contains("*)+")
                        {
                            return Err(ValidationError::ReDoSOrDepth(format!("regex pattern '{pattern}' contains dangerous nested quantifier (ReDoS risk)")));
                        }
                        if regex::Regex::new(pattern).is_err() {
                            return Err(ValidationError::ReDoSOrDepth(format!(
                                "invalid regex pattern '{pattern}'"
                            )));
                        }
                    }
                }
                Ok(())
            }
            ConditionNode::And(children) | ConditionNode::Or(children) => {
                for c in children {
                    Self::check_regex_patterns(c)?;
                }
                Ok(())
            }
            ConditionNode::Not(inner) => Self::check_regex_patterns(inner),
        }
    }

    /// Pass 5: Contradiction & Impossible Condition Detection.
    fn pass5_contradictions(node: &ConditionNode) -> Result<(), ValidationError> {
        if let ConditionNode::And(children) = node {
            let mut size_gt: Option<i64> = None;
            let mut size_lt: Option<i64> = None;

            for c in children {
                if let ConditionNode::Leaf(leaf) = c {
                    if leaf.field == FilterField::Size {
                        if let FilterValue::Number(n) = leaf.value {
                            if leaf.operator == FilterOperator::GreaterThan
                                || leaf.operator == FilterOperator::GreaterThanOrEqual
                            {
                                size_gt = Some(n);
                            } else if leaf.operator == FilterOperator::LessThan
                                || leaf.operator == FilterOperator::LessThanOrEqual
                            {
                                size_lt = Some(n);
                            }
                        }
                    }
                }
            }

            if let (Some(gt), Some(lt)) = (size_gt, size_lt) {
                if gt >= lt {
                    return Err(ValidationError::ContradictionDetected(format!(
                        "size condition contradiction: size > {gt} AND size < {lt}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Pass 6: Action Validity & Security Compliance.
    pub fn pass6_action_security(
        actions: &[RuleAction],
        opts: &ValidationOptions,
    ) -> Result<(), ValidationError> {
        for action in actions {
            match action {
                RuleAction::ForwardTo(email) => {
                    if let Some(allowed_domains) = &opts.allowed_forward_domains {
                        let domain = email.split('@').nth(1).unwrap_or("");
                        if !allowed_domains
                            .iter()
                            .any(|d| d.eq_ignore_ascii_case(domain))
                        {
                            return Err(ValidationError::ActionSecurityViolation(format!(
                                "FORWARD TO domain '{domain}' is not in allowed whitelist"
                            )));
                        }
                    }
                }
                RuleAction::CallWebhook(url) => {
                    if !url.starts_with("http://") && !url.starts_with("https://") {
                        return Err(ValidationError::ActionSecurityViolation(format!(
                            "CALL WEBHOOK URL '{url}' must start with http:// or https://"
                        )));
                    }
                    if opts.block_private_webhooks {
                        // When the host is an IP literal, reject any that lands in a
                        // blocked range. A substring match on the raw URL is not a
                        // real defense: RFC 1918, alternate literal encodings, and
                        // IPv6 all slip past it. Hostnames cannot be resolved here
                        // (this pass is synchronous); the dispatcher performs the
                        // DNS-resolution check before it connects.
                        if let Ok(parsed) = reqwest::Url::parse(url) {
                            if let Some(host) = parsed.host_str() {
                                // `host_str` brackets IPv6 literals (`[::1]`); strip
                                // them so the address parses.
                                let host = host.trim_start_matches('[').trim_end_matches(']');
                                if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                                    if is_blocked_webhook_target(ip) {
                                        return Err(ValidationError::ActionSecurityViolation(format!(
                                            "CALL WEBHOOK URL '{url}' points to a blocked private / metadata address"
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn is_valid_email_format(email: &str) -> bool {
    if email.is_empty() || !email.contains('@') {
        return false;
    }
    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return false;
    }
    !parts[0].is_empty()
        && parts[1].contains('.')
        && !parts[1].starts_with('.')
        && !parts[1].ends_with('.')
}

/// Reports whether `ip` falls in a range that must never be a webhook target.
///
/// This is the core SSRF egress control. It blocks loopback, RFC 1918 / ULA
/// private space, link-local (which subsumes the `169.254.169.254` cloud
/// metadata endpoint), the unspecified address, broadcast, and multicast.
/// It is shared by the synchronous validator (literal IPs in the URL) and the
/// dispatcher (addresses a hostname resolves to) so both apply identical rules.
pub(crate) fn is_blocked_webhook_target(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_blocked_ipv4(v4),
        std::net::IpAddr::V6(v6) => {
            // Unwrap IPv4-mapped addresses (`::ffff:a.b.c.d`) first: an embedded
            // private or loopback IPv4 must be caught by the v4 rules and not
            // slip through as an ordinary v6 address.
            match v6.to_ipv4_mapped() {
                Some(mapped) => is_blocked_ipv4(mapped),
                None => is_blocked_ipv6(v6),
            }
        }
    }
}

fn is_blocked_ipv4(ip: std::net::Ipv4Addr) -> bool {
    // `is_private` implements the exact RFC 1918 blocks (10/8, 172.16/12,
    // 192.168/16), so boundaries such as 172.32.0.0 stay allowed. The leading
    // `0` octet covers 0.0.0.0/8 (this-network, wider than `is_unspecified`).
    ip.octets()[0] == 0
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
}

fn is_blocked_ipv6(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    // `is_unique_local` / `is_unicast_link_local` are not stable on the pinned
    // toolchain, so match the prefixes by hand: fc00::/7 (unique-local) and
    // fe80::/10 (link-local unicast).
    let first = ip.segments()[0];
    (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pass1_field_type_validation() {
        let rule = crate::parser::NsqlParser::parse_rule(
            "Invalid Size",
            1,
            "WHERE size = 'abc' ACTION MARK READ",
        )
        .unwrap();
        let err = NsqlValidator::validate(&rule, &ValidationOptions::default()).unwrap_err();
        assert!(matches!(err, ValidationError::FieldTypeMismatch(_)));
    }

    #[test]
    fn test_pass1_rejects_unsupported_header_field_conditions() {
        let rule = crate::parser::NsqlParser::parse_rule(
            "Header Spam Score",
            1,
            "WHERE header['X-Spam-Score'] = '10' ACTION MARK READ",
        )
        .unwrap();
        let err = NsqlValidator::validate(&rule, &ValidationOptions::default()).unwrap_err();
        assert!(matches!(err, ValidationError::FieldTypeMismatch(_)));
    }

    #[test]
    fn test_pass4_redos_validation() {
        let rule = crate::parser::NsqlParser::parse_rule(
            "ReDoS",
            1,
            "WHERE from MATCHES '(a+)+' ACTION DELETE",
        )
        .unwrap();
        let err = NsqlValidator::validate(&rule, &ValidationOptions::default()).unwrap_err();
        assert!(matches!(err, ValidationError::ReDoSOrDepth(_)));
    }

    #[test]
    fn test_pass5_contradiction_validation() {
        let rule = crate::parser::NsqlParser::parse_rule(
            "Contradiction",
            1,
            "WHERE (size > 100 AND size < 10) ACTION DELETE",
        )
        .unwrap();
        let err = NsqlValidator::validate(&rule, &ValidationOptions::default()).unwrap_err();
        assert!(matches!(err, ValidationError::ContradictionDetected(_)));
    }

    #[test]
    fn test_pass6_action_security_validation() {
        let rule = crate::parser::NsqlParser::parse_rule(
            "Webhook Private",
            1,
            "WHERE subject CONTAINS 'x' ACTION CALL WEBHOOK 'http://127.0.0.1/steal'",
        )
        .unwrap();
        let err = NsqlValidator::validate(&rule, &ValidationOptions::default()).unwrap_err();
        assert!(matches!(err, ValidationError::ActionSecurityViolation(_)));
    }

    #[test]
    fn test_pass6_rejects_literal_ip_bypasses() {
        // Encodings the old case-insensitive substring blocklist let through.
        let blocked_urls = [
            "http://10.1.2.3/hook",
            "http://172.16.0.1/hook",
            "http://192.168.1.1/hook",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/hook",
            "http://[fc00::1]/hook",
            "http://[fe80::1]/hook",
            "http://[::ffff:127.0.0.1]/hook",
        ];
        for url in blocked_urls {
            let rule = crate::parser::NsqlParser::parse_rule(
                "Webhook Bypass",
                1,
                &format!("WHERE subject CONTAINS 'x' ACTION CALL WEBHOOK '{url}'"),
            )
            .unwrap();
            let err = NsqlValidator::validate(&rule, &ValidationOptions::default()).unwrap_err();
            assert!(
                matches!(err, ValidationError::ActionSecurityViolation(_)),
                "expected {url} to be blocked"
            );
        }
    }

    #[test]
    fn test_pass6_allows_public_literal_ip() {
        let rule = crate::parser::NsqlParser::parse_rule(
            "Webhook Public",
            1,
            "WHERE subject CONTAINS 'x' ACTION CALL WEBHOOK 'http://93.184.216.34/hook'",
        )
        .unwrap();
        assert!(NsqlValidator::validate(&rule, &ValidationOptions::default()).is_ok());
    }

    #[test]
    fn test_is_blocked_webhook_target_blocks_private_ranges() {
        let blocked: [std::net::IpAddr; 12] = [
            "127.0.0.1".parse().unwrap(),
            "10.1.2.3".parse().unwrap(),
            "172.16.0.1".parse().unwrap(),
            "172.31.255.255".parse().unwrap(),
            "192.168.1.1".parse().unwrap(),
            "169.254.169.254".parse().unwrap(),
            "169.254.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
            "::1".parse().unwrap(),
            "fc00::1".parse().unwrap(),
            "fe80::1".parse().unwrap(),
            "::ffff:127.0.0.1".parse().unwrap(),
        ];
        for ip in blocked {
            assert!(is_blocked_webhook_target(ip), "{ip} should be blocked");
        }
    }

    #[test]
    fn test_is_blocked_webhook_target_allows_public() {
        // 172.32.0.1 sits just outside 172.16.0.0/12 and proves the range math
        // is exact rather than a naive `172.` prefix check.
        let allowed: [std::net::IpAddr; 3] = [
            "172.32.0.1".parse().unwrap(),
            "93.184.216.34".parse().unwrap(),
            "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap(),
        ];
        for ip in allowed {
            assert!(!is_blocked_webhook_target(ip), "{ip} should be allowed");
        }
    }
}
