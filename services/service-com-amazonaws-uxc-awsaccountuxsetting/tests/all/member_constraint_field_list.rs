// SPDX-License-Identifier: CC0-1.0

//! What a member-constraint refusal reports STRUCTURALLY, end to end over the
//! wire (the shared harness is `crate::wire`).
//!
//! The rule: when this service refuses a request because an input member's value
//! falls outside a constraint the model declares for that member, the modelled
//! `ValidationException` identifies every refused member structurally as well as
//! in prose — one `fieldList` entry per violated constraint, carrying that
//! member's own `path` and the same clause the prose `message` states for it —
//! beside the HTTP 400 and the `ValidationException` error code.
//!
//! `ValidationException.fieldList` is "The list of fields that are invalid" and
//! `ValidationExceptionField` requires both `path` and `message`
//! (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionField")`).
//! A client that inspects `fieldList` to report which parameter was wrong has to
//! find it there, not parse the sentence.
//!
//! The members asserted over are the two `ListServices` binds with `@httpQuery` —
//! `maxResults` (`@range(1, 1000)`) and `nextToken` (`@length(32, 512)`,
//! `@pattern ^[-A-Za-z0-9+/_]+=*$`), per
//! `smithy_query(query="operation", operation="ListServices")` — which are this
//! service's whole URI-bound input surface. The rows span which member is
//! refused, which KIND of constraint it broke (`@range`, `@length`, `@pattern`),
//! the value's size and form, and how MANY violations one request carries (one,
//! one member breaking two constraints, and both members at once).

use crate::wire::{build_router, call, list_services_uri, update_customizations};
use serde_json::{json, Value};

/// The error code this service's model declares for an input that fails its
/// constraints (`smithy_query(query="operation", operation="ListServices")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The marker every requirement clause states the broken constraint after.
const CONSTRAINT_CLAUSE: &str = "failed to satisfy constraint:";

/// `maxResults` values outside the modelled `@range(1, 1000)`: each published
/// bound crossed by one, and far past it at both ends.
const OUT_OF_RANGE_MAX_RESULTS: [i64; 7] = [0, -1, -7, -1000, 1001, 5000, 1_000_000];

/// The modelled `@length` bounds of `NextToken`, so a row can be shown to
/// exercise the constraint it means to.
const NEXT_TOKEN_LENGTH: std::ops::RangeInclusive<usize> = 32..=512;

/// `nextToken` values outside `@length(32, 512)` and INSIDE the member's
/// charset, so length is the only constraint they break: each published bound
/// crossed by one, and far past it at both ends.
fn out_of_length_tokens() -> Vec<String> {
    [1_usize, 2, 31, 513, 600, 1024]
        .iter()
        .map(|length| "A".repeat(*length))
        .collect()
}

/// `nextToken` values inside `@length(32, 512)` whose characters fall outside
/// `@pattern ^[-A-Za-z0-9+/_]+=*$`, so the pattern is the only constraint they
/// break: a run of one illegal character, and a single intrusion into an
/// otherwise legal token at the head, the middle and the tail, at three sizes.
fn out_of_pattern_tokens() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for character in ['!', ' ', '.', '%'] {
        for length in [32_usize, 97, 512] {
            rows.push((
                format!("{length} {character:?} characters"),
                character.to_string().repeat(length),
            ));
            let legal = "A".repeat(length - 1);
            for (position, at) in [("head", 0), ("middle", length / 2), ("tail", length - 1)] {
                let mut value = legal.clone();
                value.insert(at, character);
                rows.push((
                    format!("one {character:?} at the {position} of {length}"),
                    value,
                ));
            }
        }
    }
    rows
}

/// One row: the request, and the member path each expected `fieldList` entry
/// names (one path per violated constraint, so a member breaking two appears
/// twice).
struct Case {
    case: String,
    max_results: Option<i64>,
    next_token: Option<String>,
    paths: Vec<&'static str>,
}

/// Every row the rule is asserted over: one member out of `@range`, one out of
/// `@length`, one out of `@pattern`, one member breaking two constraints at
/// once, and both members refused in a single request.
fn cases() -> Vec<Case> {
    let mut rows: Vec<Case> = Vec::new();

    for max_results in OUT_OF_RANGE_MAX_RESULTS {
        rows.push(Case {
            case: format!("maxResults={max_results} outside @range(1, 1000)"),
            max_results: Some(max_results),
            next_token: None,
            paths: vec!["maxResults"],
        });
    }

    for token in out_of_length_tokens() {
        rows.push(Case {
            case: format!("nextToken of {} characters outside @length", token.len()),
            max_results: None,
            next_token: Some(token),
            paths: vec!["nextToken"],
        });
    }

    for (name, token) in out_of_pattern_tokens() {
        assert!(
            NEXT_TOKEN_LENGTH.contains(&token.len()),
            "case {name:?}: a pattern row must satisfy @length(32, 512) so only the \
             pattern is broken, got {} characters",
            token.len()
        );
        rows.push(Case {
            case: format!("nextToken with {name} outside @pattern"),
            max_results: None,
            next_token: Some(token),
            paths: vec!["nextToken"],
        });
    }

    // One member, two constraints: too short for `@length` AND outside
    // `@pattern`, so the refusal reports that member twice — once per clause.
    for token in ["!", "!!", "a b"] {
        rows.push(Case {
            case: format!("nextToken {token:?} outside both @length and @pattern"),
            max_results: None,
            next_token: Some(token.to_string()),
            paths: vec!["nextToken", "nextToken"],
        });
    }

    // Both members refused in one request, at each end of each bound.
    for (max_results, token) in [
        (0_i64, "!".repeat(40)),
        (1001, "A".repeat(31)),
        (-5, "A".repeat(513)),
    ] {
        rows.push(Case {
            case: format!(
                "maxResults={max_results} and a {}-character nextToken together",
                token.len()
            ),
            max_results: Some(max_results),
            next_token: Some(token),
            paths: vec!["maxResults", "nextToken"],
        });
    }

    rows
}

/// The `fieldList` entries of an error body, as `(path, message)` pairs — with
/// the shape the model requires of each entry checked on the way through.
fn field_list(case: &str, body: &Value) -> Vec<(String, String)> {
    let entries = body
        .get("fieldList")
        .unwrap_or_else(|| {
            panic!("case {case:?}: a member-constraint refusal must carry fieldList, got {body}")
        })
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: fieldList must be an array, got {body}"));
    entries
        .iter()
        .map(|entry| {
            let object = entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: a fieldList entry must be an object, got {entry}"));
            assert!(
                object.keys().all(|key| key == "path" || key == "message"),
                "case {case:?}: a fieldList entry carries only the modelled members, got {entry}"
            );
            let path = object
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a fieldList entry requires path, got {entry}"));
            let message = object
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a fieldList entry requires message, got {entry}"));
            assert!(
                !path.is_empty() && !message.is_empty(),
                "case {case:?}: neither member of a fieldList entry may be empty, got {entry}"
            );
            (path.to_string(), message.to_string())
        })
        .collect()
}

/// The count the envelope states it detected (`N validation error(s)
/// detected: …`), which must be the number of entries reported.
fn stated_count(case: &str, message: &str) -> usize {
    message
        .split(' ')
        .next()
        .and_then(|head| head.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: no violation count in {message:?}"))
}

/// The rule, over the table: every refused member is reported in `fieldList`,
/// with its own path and its own clause.
#[tokio::test]
async fn a_member_constraint_refusal_reports_every_refused_member_in_the_field_list() {
    let router = build_router();

    for Case {
        case,
        max_results,
        next_token,
        paths,
    } in cases()
    {
        let case = case.as_str();
        let uri = list_services_uri(max_results, next_token.as_deref());
        let (status, error_type, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({body})"
        );
        assert_eq!(
            error_type, VALIDATION_EXCEPTION,
            "case {case:?}: expected a {VALIDATION_EXCEPTION}, got {error_type:?} ({body})"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no message in {body}"))
            .to_string();

        let entries = field_list(case, &body);

        let mut reported: Vec<String> = entries.iter().map(|(path, _)| path.clone()).collect();
        reported.sort();
        let mut expected: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected one entry per refused member, got {entries:?} \
             (message {message:?})"
        );

        assert_eq!(
            entries.len(),
            stated_count(case, &message),
            "case {case:?}: the envelope and fieldList must count the same violations, \
             got {entries:?} for {message:?}"
        );

        for (path, clause) in &entries {
            assert!(
                clause.contains(&format!("at '{path}'")),
                "case {case:?}: an entry's message must name its own member {path:?}, \
                 got {clause:?}"
            );
            assert!(
                clause.contains(CONSTRAINT_CLAUSE),
                "case {case:?}: an entry's message must state the broken constraint, \
                 got {clause:?}"
            );
            assert!(
                message.contains(clause.as_str()),
                "case {case:?}: an entry's message must be the clause the prose states \
                 for it, got {clause:?} against {message:?}"
            );
        }
    }
}

/// The other side of the rule: a refusal that breaks no member constraint, and a
/// request that breaks none at all, report no `fieldList` — so the member list is
/// evidence about members and not decoration on every 400.
///
/// The rows are the refusals this service answers for a reason other than a
/// member's own constraint: a request BODY it will not read (refused at the front
/// door, which names no member), and `maxResults` at each published bound, which
/// is refused by nothing.
#[tokio::test]
async fn a_refusal_that_breaks_no_member_constraint_reports_no_field_list() {
    let router = build_router();

    // A body outside its modelled constraints: refused before any member of this
    // operation's input is reported on.
    for body in [
        json!({"accountColor": "chartreuse"}),
        json!({"visibleServices": ["Invalid_Service!"]}),
    ] {
        let (status, _, answered) = update_customizations(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {body:?}: expected HTTP 400, got {status} ({answered})"
        );
        assert!(
            answered.get("fieldList").is_none(),
            "case {body:?}: a body refusal names no input member, got {answered}"
        );
    }

    // Each published bound of the one `@range` is accepted, so the refusals above
    // are the bound and not an off-by-one — and a page carries no field list.
    for max_results in [1_i64, 2, 999, 1000] {
        let uri = list_services_uri(Some(max_results), None);
        let (status, error_type, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case maxResults={max_results}: a value inside @range(1, 1000) must be \
             accepted, got {status} {error_type:?} ({body})"
        );
        assert!(
            body.get("fieldList").is_none(),
            "case maxResults={max_results}: an accepted request reports no fieldList, \
             got {body}"
        );
    }
}
