//! NSQL Parser and Dialect implementation using `sqlparser-rs`.

use crate::ast::{
    ConditionLeaf, ConditionNode, FilterField, FilterOperator, FilterRule, FilterValue, RuleAction,
};
use sqlparser::ast::{BinaryOperator, Expr, Statement, UnaryOperator, Value as SqlValue};
use sqlparser::dialect::Dialect;
use sqlparser::parser::Parser;
use thiserror::Error;

/// Parser errors emitted by NSQL compiler.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Syntax error parsing SQL structure.
    #[error("NSQL syntax error: {0}")]
    Syntax(String),
    /// Invalid field or operator.
    #[error("invalid filter expression: {0}")]
    InvalidExpression(String),
    /// Invalid rule action.
    #[error("invalid rule action: {0}")]
    InvalidAction(String),
    /// AST depth limit exceeded (MAX_AST_DEPTH = 10).
    #[error("AST recursion depth limit exceeded (max 10)")]
    MaxDepthExceeded,
}

/// Custom SQL Dialect for Nuncio SQL (NSQL).
#[derive(Debug, Default)]
pub struct NuncioSqlDialect;

impl Dialect for NuncioSqlDialect {
    fn is_identifier_start(&self, ch: char) -> bool {
        ch.is_alphabetic() || ch == '_' || ch == '$'
    }

    fn is_identifier_part(&self, ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == '-' || ch == '*' || ch == '%'
    }
}

/// Compiler parser producing `FilterRule` AST from NSQL source code text.
pub struct NsqlParser;

impl NsqlParser {
    /// Maximum permitted AST recursion depth (Requirement #276).
    pub const MAX_AST_DEPTH: usize = 10;

    /// Parse complete NSQL statement into a `FilterRule`.
    pub fn parse_rule(name: impl Into<String>, priority: i32, nsql: &str) -> Result<FilterRule, ParseError> {
        let name_str = name.into();
        let (target_account, body_nsql) = Self::extract_on_account(nsql);
        let (where_str, actions) = Self::split_where_and_actions(&body_nsql)?;
        let conditions = Self::parse_conditions(&where_str)?;

        if conditions.depth() > Self::MAX_AST_DEPTH {
            return Err(ParseError::MaxDepthExceeded);
        }

        let now = chrono::Utc::now().timestamp();
        Ok(FilterRule {
            id: format!("rule-{}", uuid_or_simple_hash(&name_str, nsql)),
            name: name_str,
            target_account,
            priority,
            enabled: true,
            nsql_text: nsql.to_string(),
            conditions,
            actions,
            created_at: now,
            updated_at: now,
        })
    }

    /// Extract `ON ACCOUNT '<target>'` clause if present.
    fn extract_on_account(nsql: &str) -> (String, String) {
        let nsql_trim = nsql.trim();

        if let Some(idx) = find_keyword_outside_quotes(nsql_trim, "ON ACCOUNT") {
            let prefix = nsql_trim[..idx].trim();
            let after = nsql_trim[idx + 10..].trim();

            let (target, rest) = if after.starts_with('\'') {
                let end_quote = after[1..].find('\'').map(|i| i + 1).unwrap_or(after.len());
                let t = &after[1..end_quote];
                let r = if end_quote + 1 < after.len() { &after[end_quote + 1..] } else { "" };
                (t, r)
            } else if after.starts_with('"') {
                let end_quote = after[1..].find('"').map(|i| i + 1).unwrap_or(after.len());
                let t = &after[1..end_quote];
                let r = if end_quote + 1 < after.len() { &after[end_quote + 1..] } else { "" };
                (t, r)
            } else {
                let mut parts = after.split_whitespace();
                let t = parts.next().unwrap_or("*");
                let r = after[t.len()..].trim();
                (t, r)
            };

            let combined = if prefix.is_empty() {
                rest.trim().to_string()
            } else {
                format!("{prefix} {}", rest.trim()).trim().to_string()
            };

            let target_account = if target == "?" || target == "*" || target == "%" {
                "*".to_string()
            } else {
                target.to_string()
            };

            return (target_account, combined);
        }

        ("*".to_string(), nsql_trim.to_string())
    }

    /// Split NSQL text into WHERE clause part and ACTION clause part.
    fn split_where_and_actions(nsql: &str) -> Result<(String, Vec<RuleAction>), ParseError> {
        let nsql_trim = nsql.trim();
        let action_idx = find_keyword_outside_quotes(nsql_trim, "ACTION");

        let (where_part, action_part) = match action_idx {
            Some(idx) => (&nsql_trim[..idx], &nsql_trim[idx + 6..]),
            None => (nsql_trim, ""),
        };

        let actions = if action_part.trim().is_empty() {
            Vec::new()
        } else {
            Self::parse_actions(action_part)?
        };

        Ok((where_part.trim().to_string(), actions))
    }

    /// Parse WHERE condition string into `ConditionNode`.
    pub fn parse_conditions(where_str: &str) -> Result<ConditionNode, ParseError> {
        let trimmed = where_str.trim();
        if trimmed.is_empty() {
            return Err(ParseError::Syntax("empty filter condition".to_string()));
        }

        let preprocessed = preprocess_nsql_where(trimmed);

        let full_sql = if preprocessed.to_uppercase().starts_with("SELECT") {
            preprocessed.clone()
        } else if preprocessed.to_uppercase().starts_with("WHERE") {
            format!("SELECT * FROM emails {preprocessed}")
        } else {
            format!("SELECT * FROM emails WHERE {preprocessed}")
        };

        let dialect = NuncioSqlDialect;
        let statements = Parser::parse_sql(&dialect, &full_sql)
            .map_err(|e| ParseError::Syntax(e.to_string()))?;

        if statements.is_empty() {
            return Err(ParseError::Syntax("no statements parsed".to_string()));
        }

        match &statements[0] {
            Statement::Query(query) => {
                if let sqlparser::ast::SetExpr::Select(select) = query.body.as_ref() {
                    if let Some(selection) = &select.selection {
                        Self::convert_expr(selection, 1)
                    } else {
                        Err(ParseError::Syntax("missing WHERE selection clause".to_string()))
                    }
                } else {
                    Err(ParseError::Syntax("expected SELECT query statement".to_string()))
                }
            }
            _ => Err(ParseError::Syntax("expected SELECT statement".to_string())),
        }
    }

    /// Convert `sqlparser::ast::Expr` into `ConditionNode`.
    fn convert_expr(expr: &Expr, depth: usize) -> Result<ConditionNode, ParseError> {
        if depth > Self::MAX_AST_DEPTH {
            return Err(ParseError::MaxDepthExceeded);
        }

        match expr {
            Expr::Nested(inner) => Self::convert_expr(inner, depth),

            Expr::UnaryOp { op: UnaryOperator::Not, expr: inner } => {
                let node = Self::convert_expr(inner, depth + 1)?;
                Ok(ConditionNode::Not(Box::new(node)))
            }

            Expr::BinaryOp { left, op, right } => match op {
                BinaryOperator::And => {
                    let left_node = Self::convert_expr(left, depth + 1)?;
                    let right_node = Self::convert_expr(right, depth + 1)?;
                    Ok(ConditionNode::And(vec![left_node, right_node]))
                }
                BinaryOperator::Or => {
                    let left_node = Self::convert_expr(left, depth + 1)?;
                    let right_node = Self::convert_expr(right, depth + 1)?;
                    Ok(ConditionNode::Or(vec![left_node, right_node]))
                }
                BinaryOperator::Eq => Self::convert_leaf(left, FilterOperator::Equals, right),
                BinaryOperator::NotEq => Self::convert_leaf(left, FilterOperator::NotEquals, right),
                BinaryOperator::Gt => Self::convert_leaf(left, FilterOperator::GreaterThan, right),
                BinaryOperator::Lt => Self::convert_leaf(left, FilterOperator::LessThan, right),
                BinaryOperator::GtEq => Self::convert_leaf(left, FilterOperator::GreaterThanOrEqual, right),
                BinaryOperator::LtEq => Self::convert_leaf(left, FilterOperator::LessThanOrEqual, right),
                BinaryOperator::PGLikeMatch => Self::convert_leaf(left, FilterOperator::Contains, right),
                BinaryOperator::PGILikeMatch => Self::convert_leaf(left, FilterOperator::Contains, right),
                BinaryOperator::PGNotLikeMatch => Self::convert_leaf(left, FilterOperator::NotContains, right),
                BinaryOperator::PGRegexMatch | BinaryOperator::PGRegexIMatch => Self::convert_leaf(left, FilterOperator::Matches, right),
                BinaryOperator::Custom(custom_op) => {
                    let op_upper = custom_op.to_uppercase();
                    match op_upper.as_str() {
                        "CONTAINS" => Self::convert_leaf(left, FilterOperator::Contains, right),
                        "MATCHES" | "REGEX" => Self::convert_leaf(left, FilterOperator::Matches, right),
                        _ => Err(ParseError::InvalidExpression(format!("unsupported operator: {custom_op}"))),
                    }
                }
                _ => Err(ParseError::InvalidExpression(format!("unsupported binary operator: {op:?}"))),
            },

            Expr::Like { expr: left, pattern: right, negated, .. } => {
                let op = if *negated { FilterOperator::NotContains } else { FilterOperator::Contains };
                Self::convert_leaf(left, op, right)
            }

            Expr::ILike { expr: left, pattern: right, negated, .. } => {
                let op = if *negated { FilterOperator::NotContains } else { FilterOperator::Contains };
                Self::convert_leaf(left, op, right)
            }

            Expr::InList { expr: left, list, negated } => {
                let field = Self::extract_field(left)?;
                let mut values = Vec::new();
                for item in list {
                    if let Expr::Value(SqlValue::SingleQuotedString(s)) = item {
                        values.push(s.clone());
                    } else if let Expr::Identifier(ident) = item {
                        values.push(ident.value.clone());
                    } else {
                        return Err(ParseError::InvalidExpression("IN list elements must be strings or identifiers".to_string()));
                    }
                }
                let op = if *negated { FilterOperator::NotEquals } else { FilterOperator::In };
                Ok(ConditionNode::Leaf(ConditionLeaf {
                    field,
                    operator: op,
                    value: FilterValue::List(values),
                }))
            }

            Expr::Identifier(ident) => {
                if let Some(field) = FilterField::parse_str(&ident.value) {
                    Ok(ConditionNode::Leaf(ConditionLeaf {
                        field,
                        operator: FilterOperator::Equals,
                        value: FilterValue::Boolean(true),
                    }))
                } else {
                    Err(ParseError::InvalidExpression(format!("unknown field identifier: {}", ident.value)))
                }
            }

            _ => Self::extract_field(expr)
                .map(|f| ConditionNode::Leaf(ConditionLeaf {
                    field: f,
                    operator: FilterOperator::Equals,
                    value: FilterValue::Boolean(true),
                }))
                .map_err(|_| ParseError::InvalidExpression(format!("unsupported expression structure: {expr:?}"))),
        }
    }

    /// Extract `FilterField` and `FilterValue` for leaf condition.
    fn convert_leaf(left: &Expr, op: FilterOperator, right: &Expr) -> Result<ConditionNode, ParseError> {
        let field = Self::extract_field(left)?;
        let value = Self::extract_value(right)?;
        Ok(ConditionNode::Leaf(ConditionLeaf { field, operator: op, value }))
    }

    /// Extract `FilterField` from SQL expression.
    fn extract_field(expr: &Expr) -> Result<FilterField, ParseError> {
        let expr_str = expr.to_string();
        if expr_str.starts_with("header__") {
            let key = expr_str[8..].replace('_', "-");
            return Ok(FilterField::Header(key));
        }

        if let Some(f) = FilterField::parse_str(&expr_str) {
            return Ok(f);
        }

        match expr {
            Expr::Identifier(ident) => {
                if ident.value.starts_with("header__") {
                    let key = ident.value[8..].replace('_', "-");
                    Ok(FilterField::Header(key))
                } else {
                    FilterField::parse_str(&ident.value)
                        .ok_or_else(|| ParseError::InvalidExpression(format!("unknown field: {}", ident.value)))
                }
            }
            Expr::CompoundIdentifier(idents) => {
                let name = idents.iter().map(|i| i.value.as_str()).collect::<Vec<_>>().join(".");
                FilterField::parse_str(&name)
                    .ok_or_else(|| ParseError::InvalidExpression(format!("unknown field: {name}")))
            }
            _ => {
                let lower = expr_str.to_lowercase();
                if lower.starts_with("header[") && lower.ends_with(']') {
                    let key_str = expr_str[7..expr_str.len() - 1].trim_matches('\'').trim_matches('"').to_string();
                    Ok(FilterField::Header(key_str))
                } else {
                    Err(ParseError::InvalidExpression(format!("expected field identifier, got: {expr_str}")))
                }
            }
        }
    }

    /// Extract `FilterValue` from SQL expression.
    fn extract_value(expr: &Expr) -> Result<FilterValue, ParseError> {
        match expr {
            Expr::Value(SqlValue::SingleQuotedString(s))
            | Expr::Value(SqlValue::DoubleQuotedString(s)) => Ok(FilterValue::String(s.clone())),
            Expr::Value(SqlValue::Number(num, _)) => {
                let parsed = num.parse::<i64>()
                    .map_err(|_| ParseError::InvalidExpression(format!("invalid integer number: {num}")))?;
                Ok(FilterValue::Number(parsed))
            }
            Expr::Value(SqlValue::Boolean(b)) => Ok(FilterValue::Boolean(*b)),
            Expr::Identifier(ident) => {
                let lower = ident.value.to_lowercase();
                if lower == "true" {
                    Ok(FilterValue::Boolean(true))
                } else if lower == "false" {
                    Ok(FilterValue::Boolean(false))
                } else {
                    Ok(FilterValue::String(ident.value.clone()))
                }
            }
            Expr::Tuple(list) => {
                let mut strings = Vec::new();
                for item in list {
                    if let Expr::Value(SqlValue::SingleQuotedString(s)) = item {
                        strings.push(s.clone());
                    } else if let Expr::Identifier(ident) = item {
                        strings.push(ident.value.clone());
                    }
                }
                Ok(FilterValue::List(strings))
            }
            _ => Err(ParseError::InvalidExpression(format!("unsupported value literal: {expr:?}"))),
        }
    }

    /// Parse ACTION clause tokens.
    pub fn parse_actions(actions_str: &str) -> Result<Vec<RuleAction>, ParseError> {
        let chunks = split_comma_outside_quotes(actions_str);
        let mut actions = Vec::new();

        for chunk in chunks {
            let trimmed = chunk.trim();
            if trimmed.is_empty() {
                continue;
            }
            let upper = trimmed.to_uppercase();

            if upper == "MARK READ" {
                actions.push(RuleAction::MarkRead);
            } else if upper == "MARK UNREAD" {
                actions.push(RuleAction::MarkUnread);
            } else if upper == "FLAG" {
                actions.push(RuleAction::Flag);
            } else if upper == "UNFLAG" {
                actions.push(RuleAction::Unflag);
            } else if upper == "DELETE" {
                actions.push(RuleAction::Delete);
            } else if upper.starts_with("MOVE TO") {
                let target = trimmed[7..].trim().trim_matches('\'').trim_matches('"');
                if target.is_empty() {
                    return Err(ParseError::InvalidAction("MOVE TO missing target folder".to_string()));
                }
                actions.push(RuleAction::MoveTo(target.to_string()));
            } else if upper.starts_with("FORWARD TO") {
                let target = trimmed[10..].trim().trim_matches('\'').trim_matches('"');
                if target.is_empty() {
                    return Err(ParseError::InvalidAction("FORWARD TO missing target email".to_string()));
                }
                actions.push(RuleAction::ForwardTo(target.to_string()));
            } else if upper.starts_with("CALL WEBHOOK") {
                let url = trimmed[12..].trim().trim_matches('\'').trim_matches('"');
                if url.is_empty() {
                    return Err(ParseError::InvalidAction("CALL WEBHOOK missing target URL".to_string()));
                }
                actions.push(RuleAction::CallWebhook(url.to_string()));
            } else {
                return Err(ParseError::InvalidAction(format!("unknown action clause: {trimmed}")));
            }
        }

        Ok(actions)
    }
}

fn preprocess_nsql_where(input: &str) -> String {
    let mut result = input.to_string();

    let re_header = regex::Regex::new(r"(?i)header\s*\[\s*[']([^']+)[']\s*\]").expect("regex");
    result = re_header.replace_all(&result, |caps: &regex::Captures| {
        let key = &caps[1].replace('-', "_");
        format!("header__{key}")
    }).to_string();

    let re_contains = regex::Regex::new(r"(?i)\bCONTAINS\b").expect("regex");
    result = re_contains.replace_all(&result, "LIKE").to_string();

    let re_matches = regex::Regex::new(r"(?i)\bMATCHES\b").expect("regex");
    result = re_matches.replace_all(&result, "~").to_string();

    result
}

fn find_keyword_outside_quotes(text: &str, keyword: &str) -> Option<usize> {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let chars: Vec<char> = text.chars().collect();
    let kw_chars: Vec<char> = keyword.chars().collect();

    for i in 0..chars.len() {
        let c = chars[i];
        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
        } else if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
        } else if !in_single_quote && !in_double_quote {
            if i + kw_chars.len() <= chars.len() {
                let is_match = chars[i..i + kw_chars.len()]
                    .iter()
                    .zip(kw_chars.iter())
                    .all(|(a, b)| a.to_ascii_uppercase() == b.to_ascii_uppercase());
                if is_match {
                    let prev_ok = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == ')';
                    let next_idx = i + kw_chars.len();
                    let next_ok = next_idx == chars.len() || chars[next_idx].is_whitespace();
                    if prev_ok && next_ok {
                        return Some(i);
                    }
                }
            }
        }
    }
    None
}

fn split_comma_outside_quotes(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    for c in text.chars() {
        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(c);
        } else if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(c);
        } else if c == ',' && !in_single_quote && !in_double_quote {
            result.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
    }

    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }

    result
}

fn uuid_or_simple_hash(name: &str, nsql: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(nsql.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_on_account_wildcard() {
        let nsql = "ON ACCOUNT '%@kof22.com' WHERE subject CONTAINS 'Urgent' ACTION MARK READ";
        let rule = NsqlParser::parse_rule("Company Rule", 1, nsql).expect("parse rule");
        assert_eq!(rule.target_account, "%@kof22.com");
        assert!(rule.matches_account("james@kof22.com"));
        assert!(!rule.matches_account("user@nuncio.mx"));
    }

    #[test]
    fn test_parse_simple_nsql() {
        let nsql = "SELECT * FROM emails WHERE subject CONTAINS 'Urgent' ACTION MOVE TO 'priority', MARK READ";
        let rule = NsqlParser::parse_rule("Urgent Rule", 1, nsql).expect("parse rule");
        assert_eq!(rule.name, "Urgent Rule");
        assert_eq!(rule.target_account, "*");
        assert_eq!(rule.actions.len(), 2);
    }
}
