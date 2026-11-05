// SPDX-License-Identifier: CC0-1.0

//! The caller-supplied regular expressions a WAFv2 save carries: compiling one into
//! something that matches, finding the one a save must be refused for, and the
//! refusal it draws.
//!
//! Every expression here comes from a request, so [`compile_regex`] is the ONE place
//! that compiles one and it bounds every compile with [`REGEX_SIZE_LIMIT`] (the
//! compiled form) and [`REGEX_DFA_SIZE_LIMIT`] (the match cache); the one extra
//! judgement it makes runs the compiled expression against ONE fixed, empty input.
//! So the work one expression can ask for is bounded by those two constants and by
//! the modelled length of the member that carried it, and the number of expressions
//! one request carries is bounded by that member's own modelled list length and —
//! for a rule tree — by the depth bound [`validate_statement_nesting`] applies
//! before any walk.

use super::*;
use crate::ops::types::Regex;

/// Emulator resource guard, NOT an AWS quota: the largest compiled form this
/// emulator builds for one caller-supplied regular expression.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Emulator resource guard, NOT an AWS quota: the largest cache the matcher may use
/// while matching one value against one caller-supplied regular expression.
const REGEX_DFA_SIZE_LIMIT: usize = 1 << 20;

/// `pattern` compiled into something that MATCHES, or `None` when WAF will not
/// inspect requests with it.
///
/// The ONE place a caller-supplied expression becomes something that matches, so the
/// engine that decides whether a stored pattern matches a request is the engine that
/// decides, when the request that carries it is saved, whether it is an expression
/// WAF can run at all: no expression can be accepted by one and rejected by the
/// other. Compiled with the maintained crate — the syntax this service documents for
/// a `RegexString` is that crate's — under a compiled-size and a match-cache bound,
/// because the pattern comes from the request.
///
/// Two ways text is not something that matches, because the modelled constraints on
/// a `RegexString` bound its length only (`Regex$RegexString` is `@length(1..512)`
/// with `@pattern(".*")`, `smithy_query(query="shape",shape="Regex")`), so every byte
/// sequence of legal length reaches the service and what WAF can do with it is the
/// service's own judgement:
///
/// * the engine cannot parse it; or
/// * its match can consume NOTHING. An expression that matches the EMPTY string
///   matches at the start of every value whatever that value holds, so it states no
///   criterion a request can fail — `.*`, `a*`, `(x)?` and `^$` alike — and WAF
///   refuses it as text that is not in the form this parameter takes, rather than
///   storing an inspection criterion that inspects nothing.
pub fn compile_regex(pattern: &str) -> Option<::regex::bytes::Regex> {
    let compiled = ::regex::bytes::RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .ok()?;
    (!compiled.is_match(b"")).then_some(compiled)
}

/// A `RegexString` of `expressions` this service cannot compile into something that
/// matches, if any (the first such the scan reaches). See [`compile_regex`].
pub fn unparseable_regex(expressions: &[Regex]) -> Option<&str> {
    expressions
        .iter()
        .filter_map(|expression| expression.regex_string.as_deref())
        .find(|pattern| compile_regex(pattern).is_none())
}

/// A regular expression `rules` carries that this service cannot compile into
/// something that matches, if any (the first such the walk reaches): a
/// `RegexMatchStatement$RegexString` at any nesting depth, or one of the
/// `ClientSideAction$ExemptUriRegularExpressions` a managed rule group's
/// configuration carries.
///
/// Every expression a save would store is judged by [`compile_regex`], whichever
/// member carries it: a rule's own expression is as unusable as a pattern set's when
/// it does not parse or its match can consume nothing.
///
/// Walked with an explicit worklist, and only after [`validate_statement_nesting`]
/// has bounded the DEPTH of the same caller-supplied tree: the tree comes off the
/// wire.
pub fn unparseable_rule_regex(rules: &[Rule]) -> Option<&str> {
    let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some(regex_match) = statement.regex_match_statement.as_ref() {
            if compile_regex(&regex_match.regex_string).is_none() {
                return Some(&regex_match.regex_string);
            }
        }
        if let Some(managed) = statement.managed_rule_group_statement.as_ref() {
            for config in managed.managed_rule_group_configs.iter().flatten() {
                let exempt = config
                    .aws_managed_rules_anti_d_do_s_rule_set
                    .as_ref()
                    .and_then(|anti_ddos| {
                        anti_ddos
                            .client_side_action_config
                            .challenge
                            .exempt_uri_regular_expressions
                            .as_deref()
                    })
                    .unwrap_or_default();
                if let Some(pattern) = unparseable_regex(exempt) {
                    return Some(pattern);
                }
            }
        }
        pending.extend(nested_statements(statement));
    }
    None
}

/// [`invalid_parameter_exception`] for a regular expression WAF will not inspect
/// requests with ([`compile_regex`]): `Field` the settings the expression was found
/// in, `Parameter` the expression as the request wrote it and `Reason`
/// [`FORMAT_NOT_VALID_REASON`].
pub fn invalid_regex_exception(field: &str, pattern: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception(field, pattern, FORMAT_NOT_VALID_REASON)
}
