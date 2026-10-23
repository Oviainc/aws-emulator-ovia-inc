// SPDX-License-Identifier: CC0-1.0

//! How a refusal QUOTES the `@pattern` a value failed.
//!
//! The rule: when the service refuses a value for violating a modelled
//! `@pattern`, the requirement clause states the expression the value had to
//! satisfy WITHOUT the whole-value anchors the model writes around it — the `^`
//! and `$` are how Smithy turns a find into a whole-string match, not part of the
//! expression the service advertises — while the status (400), the error shape
//! (`ValidationException`), the envelope, the member path and every other kind of
//! requirement stay exactly as the framework states them.
//!
//! The input this service binds a `@pattern` to is `ListServices.nextToken`
//! (`NextToken` `@length(32, 512)`, `@pattern ^[-A-Za-z0-9+/_]+=*$` —
//! `smithy_query(query="operation", operation="ListServices")`), bound `@httpQuery`,
//! so it is refused by the framework's boundary walk and worded through
//! `Service::override_refusal`. Each row below satisfies the member's `@length` and
//! violates only its `@pattern`, so the refusal carries exactly one clause; the
//! control rows at the end violate a `@length` or a `@range` instead and must keep
//! the framework's own requirement wording untouched.

use crate::wire::{build_router, call, list_services_uri};
use serde_json::Value;

/// The `NextToken` `@pattern` as the model writes it
/// (`smithy_query(query="shape", shape="NextToken")`).
const MODEL_PATTERN: &str = r"^[-A-Za-z0-9+/_]+=*$";

/// The modelled `@length` bounds of `NextToken`, so a row can be checked to
/// exercise the pattern rather than the length.
const NEXT_TOKEN_LENGTH: std::ops::RangeInclusive<usize> = 32..=512;

/// The marker a requirement clause states a pattern after.
const PATTERN_CLAUSE: &str = "Member must satisfy regular expression pattern: ";

/// The expression a pattern refusal must quote: [`MODEL_PATTERN`] with the
/// whole-value anchor pair dropped. Derived here rather than written out, so this
/// states the RULE and not one expected string.
fn advertised_pattern() -> String {
    MODEL_PATTERN
        .strip_prefix('^')
        .and_then(|body| body.strip_suffix('$'))
        .expect("the modelled pattern is a whole-value pattern")
        .to_string()
}

/// The whole message a one-clause refusal of `member` carrying `value` states.
fn expected_message(value: &str, member: &str, requirement: &str) -> String {
    format!("1 validation error detected: Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The rows: a `nextToken` inside `@length(32, 512)` whose characters violate
/// `@pattern`. They differ along which character is illegal (punctuation, a space,
/// a quote, a backslash, a regex metacharacter, an anchor character itself), WHERE
/// it sits (the whole value, the head, the middle, the tail) and the value's SIZE
/// (the modelled minimum, in between, and the longest value the framework still
/// echoes whole).
fn pattern_violating_tokens() -> Vec<(String, String)> {
    let illegal = [
        ("exclamation marks", '!'),
        ("asterisks", '*'),
        ("spaces", ' '),
        ("full stops", '.'),
        ("colons", ':'),
        ("percent signs", '%'),
        ("hash signs", '#'),
        ("ampersands", '&'),
        ("single quotes", '\''),
        ("backslashes", '\\'),
        ("carets", '^'),
        ("dollar signs", '$'),
    ];
    let mut rows: Vec<(String, String)> = Vec::new();
    for (name, character) in illegal {
        for length in [32_usize, 40, 97, 256] {
            rows.push((
                format!("{length} {name}"),
                character.to_string().repeat(length),
            ));
            // The same illegal character as a single intrusion into an otherwise
            // legal token, at the head, the middle and the tail.
            let legal = "A".repeat(length - 1);
            for (position, at) in [("head", 0), ("middle", length / 2), ("tail", length - 1)] {
                let mut value = legal.clone();
                value.insert(at, character);
                rows.push((format!("one of {name} at the {position} of {length}"), value));
            }
        }
    }
    rows
}

/// One row of the control table below: a request that violates a constraint which
/// is NOT a pattern, and the requirement the framework states for it.
struct ControlRow {
    case: &'static str,
    max_results: Option<i64>,
    next_token: Option<String>,
    member: &'static str,
    requirement: &'static str,
}

/// The rule, over that table: every pattern refusal states the advertised
/// expression, with no anchors, inside the framework's own envelope.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_without_the_models_anchors() {
    let router = build_router();
    let advertised = advertised_pattern();
    let requirement = format!("{PATTERN_CLAUSE}{advertised}");

    for (case, token) in pattern_violating_tokens() {
        assert!(
            NEXT_TOKEN_LENGTH.contains(&token.len()),
            "case {case:?}: a row must satisfy @length(32, 512) so only the pattern is \
             violated, got {} characters",
            token.len()
        );

        let uri = list_services_uri(None, Some(&token));
        let (status, error_type, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({body})"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {case:?}: expected a ValidationException, got {error_type:?} ({body})"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no message in {body}"));
        assert_eq!(
            message,
            expected_message(&token, "nextToken", &requirement),
            "case {case:?}: expected the AWS envelope quoting {advertised:?}, got {message:?}"
        );
        // Stated separately from the whole-message comparison because the ECHOED
        // value may itself contain `^` or `$`: it is the requirement clause, and
        // only it, that must carry no anchors.
        let quoted = message
            .rsplit_once(PATTERN_CLAUSE)
            .map(|(_, quoted)| quoted)
            .unwrap_or_else(|| panic!("case {case:?}: no pattern clause in {message:?}"));
        assert_eq!(
            quoted, advertised,
            "case {case:?}: expected the advertised expression {advertised:?}, got {quoted:?}"
        );
    }
}

/// The other side of the rule: a requirement that is NOT a pattern is stated
/// exactly as the framework states it, so nothing but the pattern rendering moved.
///
/// Rows span the other constraint kinds this operation's input carries — the
/// `NextToken` `@length` at and past each published bound, and the `MaxResults`
/// `@range` at and past each published bound
/// (`smithy_query(query="operation", operation="ListServices")`).
#[tokio::test]
async fn a_requirement_that_is_not_a_pattern_keeps_the_frameworks_wording() {
    let router = build_router();

    // The other constraint kinds, each at and past its published bound.
    let rows: Vec<ControlRow> = vec![
        ControlRow {
            case: "nextToken one character below the @length minimum",
            max_results: None,
            next_token: Some("A".repeat(31)),
            member: "nextToken",
            requirement: "Member must have length greater than or equal to 32",
        },
        ControlRow {
            case: "nextToken far below the @length minimum",
            max_results: None,
            next_token: Some("A".to_string()),
            member: "nextToken",
            requirement: "Member must have length greater than or equal to 32",
        },
        ControlRow {
            case: "nextToken one character above the @length maximum",
            max_results: None,
            next_token: Some("A".repeat(513)),
            member: "nextToken",
            requirement: "Member must have length less than or equal to 512",
        },
        ControlRow {
            case: "maxResults one below the @range minimum",
            max_results: Some(0),
            next_token: None,
            member: "maxResults",
            requirement: "Member must have value greater than or equal to 1",
        },
        ControlRow {
            case: "maxResults far below the @range minimum",
            max_results: Some(-7),
            next_token: None,
            member: "maxResults",
            requirement: "Member must have value greater than or equal to 1",
        },
        ControlRow {
            case: "maxResults one above the @range maximum",
            max_results: Some(1001),
            next_token: None,
            member: "maxResults",
            requirement: "Member must have value less than or equal to 1000",
        },
    ];

    for ControlRow {
        case,
        max_results,
        next_token,
        member,
        requirement,
    } in rows
    {
        let uri = list_services_uri(max_results, next_token.as_deref());
        let (status, error_type, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({body})"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {case:?}: expected a ValidationException, got {error_type:?} ({body})"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no message in {body}"));
        assert!(
            message.contains(requirement) && message.contains(&format!("at '{member}'")),
            "case {case:?}: expected the framework's own requirement {requirement:?} at \
             {member:?}, got {message:?}"
        );
        assert!(
            !message.contains(PATTERN_CLAUSE),
            "case {case:?}: a non-pattern requirement must not state a pattern, got {message:?}"
        );
    }
}

/// The value a page request DOES satisfy still pages: the rendering change touches
/// only how a refusal is worded, never which requests are refused.
#[tokio::test]
async fn a_token_inside_the_pattern_is_not_refused() {
    let router = build_router();
    let (_, token) = crate::wire::list_page(&router, Some(1), None).await;
    let issued = token.expect("a first page of one entry must carry a nextToken");

    let uri = list_services_uri(Some(1), Some(&issued));
    let (status, error_type, body) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "an issued token must still resume the listing, got {status} {error_type:?} ({body})"
    );
}
