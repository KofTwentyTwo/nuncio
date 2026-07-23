//! Lossless round-trip NSQL Code Generator (`rule.to_nsql()`).

use crate::ast::FilterRule;
use crate::parser::NsqlParser;

impl FilterRule {
    /// Render the `FilterRule` AST into lossless NSQL source code text.
    pub fn to_nsql(&self) -> String {
        let where_clause = self.conditions.to_nsql();
        let actions_clause = self
            .actions
            .iter()
            .map(|a| a.to_nsql())
            .collect::<Vec<_>>()
            .join(", ");

        if actions_clause.is_empty() {
            format!("SELECT * FROM emails WHERE {where_clause}")
        } else {
            format!("SELECT * FROM emails WHERE {where_clause} ACTION {actions_clause}")
        }
    }
}

/// Helper verifying algebraic round-trip parsing invariants:
/// `NsqlParser::parse(rule.to_nsql())` produces an equivalent AST.
pub fn verify_roundtrip_invariant(rule: &FilterRule) -> bool {
    let code = rule.to_nsql();
    if let Ok(parsed) = NsqlParser::parse_rule(&rule.name, rule.priority, &code) {
        parsed.conditions == rule.conditions && parsed.actions == rule.actions
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lossless_codegen_roundtrip() {
        let original_nsql = "SELECT * FROM emails WHERE (subject CONTAINS 'Urgent' AND (size > 1024 OR from = 'boss@nuncio.mx')) ACTION MOVE TO 'Priority', MARK READ";
        let rule = NsqlParser::parse_rule("Urgent Rule", 1, original_nsql).expect("parse rule");

        let generated_nsql = rule.to_nsql();
        assert!(generated_nsql.contains("subject CONTAINS 'Urgent'"));
        assert!(generated_nsql.contains("ACTION MOVE TO 'Priority', MARK READ"));

        let re_parsed = NsqlParser::parse_rule("Urgent Rule", 1, &generated_nsql).expect("re-parse generated nsql");
        assert_eq!(rule.conditions, re_parsed.conditions);
        assert_eq!(rule.actions, re_parsed.actions);
    }
}
