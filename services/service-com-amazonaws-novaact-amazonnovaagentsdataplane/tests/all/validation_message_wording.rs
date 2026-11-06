// SPDX-License-Identifier: CC0-1.0

//! How this service words a request its modelled input constraints refuse.
//!
//! The rule under test: a request that breaks a modelled input constraint is answered
//! HTTP 400 `ValidationException` whose message is the counted envelope
//! (`1 validation error detected: ` / `N validation errors detected: `) over one clause
//! per offending value, and a clause stating a `@pattern` requirement names the MEMBER PATH
//! and the modelled expression WITHOUT its `^` / `$` anchors, and no value at all — the
//! requirement is the expression every value of that member must match rather than the one
//! value that missed it. An enumerated member's value SET is stated the same way, listing
//! the values the model declares and no submitted value. Every other constraint class keeps
//! the clause the framework writes, value and all.
//!
//! The table varies what that rule generalises over rather than the one recorded request:
//! which operation refuses, where the member is bound (a body member, a URI label, a
//! query parameter), which constraint class it broke (`@pattern`, `@length` on each side,
//! `@range` on each side, an enum value set, an absent `@required` member), how many
//! violations one request raises at once — including a member that breaks its bound AND its
//! expression, so one envelope carries a clause of each kind — whether the member is
//! `@sensitive`, and the form of the submitted value. The recorded case —
//! `CreateWorkflowDefinition` with a `name` containing spaces and punctuation — is one row
//! of it.
//!
//! Every modelled expression quoted below is the one `smithy_query(query="shape", …)`
//! reports for that member's target shape, with the anchors stripped.

use serde_json::{json, Value};

use crate::workflow_lifecycle::{text, Emulator};

/// `WorkflowDefinitionName`: `len 1..40`, `pattern ^[a-zA-Z0-9_-]{1,40}$`
/// (`smithy_query(query="shape",shape="WorkflowDefinitionName")`).
const DEFINITION_NAME_PATTERN: &str = "[a-zA-Z0-9_-]{1,40}";
const DEFINITION_NAME_MAX: usize = 40;

/// `UuidString`: `pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="shape",shape="UuidString")`).
const UUID_PATTERN: &str = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";

/// `ClientToken`: `len 33..256`, `pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`
/// (`smithy_query(query="shape",shape="ClientToken")`).
const CLIENT_TOKEN_PATTERN: &str = "[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}";

/// `NextToken`: `len 1..2048`, `pattern ^\S*$`
/// (`smithy_query(query="shape",shape="NextToken")`).
const NEXT_TOKEN_PATTERN: &str = r"\S*";

/// `CloudWatchLogGroupName`: `len 1..512`, `pattern ^[a-zA-Z0-9_/.-]+$`
/// (`smithy_query(query="shape",shape="CloudWatchLogGroupName")`).
const LOG_GROUP_PATTERN: &str = "[a-zA-Z0-9_/.-]+";

/// `MaxResults`: `range 1..100` (`smithy_query(query="shape",shape="MaxResults")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// `SortOrder`'s value set, in the order the model declares it
/// (`smithy_query(query="shape",shape="SortOrder")`).
const SORT_ORDER_VALUES: [&str; 2] = ["Ascending", "Descending"];

/// One refused request and the clauses its message must carry, in order.
struct Case {
    label: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The value the request submitted at the member whose requirement states none, where
    /// the row has one: no clause may echo it.
    withheld: Option<String>,
    clauses: Vec<String>,
}

/// The clause for a value that broke `requirement` at `path` without echoing it — the
/// `@pattern` class.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause for `value`, which broke `requirement` at `path` — every class but
/// `@pattern`, where the submitted value is quoted as the framework renders it.
fn value_clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause for a `@required` member the request never sent.
fn absent_clause(path: &str) -> String {
    format!("Value null at '{path}' failed to satisfy constraint: Member must not be null")
}

/// The whole message: the counted envelope over `clauses`.
fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        ),
    }
}

/// A `clientToken` inside `ClientToken`'s length bounds whose body is `body`, padded with
/// characters the pattern admits so only `body` decides whether it is accepted.
fn padded_token(body: &str) -> String {
    let mut token = format!("tok{body}");
    while token.len() < 33 {
        token.push('0');
    }
    token
}

/// The table: every row is a request the modelled constraints refuse.
fn cases() -> Vec<Case> {
    let over_long_name = "n".repeat(DEFINITION_NAME_MAX + 1);
    let spaced_token = padded_token("with a space");
    let pattern_requirement =
        |pattern: &str| format!("Member must satisfy regular expression pattern: {pattern}");
    let min_length = |min: usize| format!("Member must have length greater than or equal to {min}");
    let max_length = |max: usize| format!("Member must have length less than or equal to {max}");

    vec![
        // The recorded row: a body member whose value is outside its pattern.
        Case {
            label: "a body member outside its pattern",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "name": "bad name with spaces!" })),
            withheld: Some("bad name with spaces!".to_string()),
            clauses: vec![clause(
                "name",
                &pattern_requirement(DEFINITION_NAME_PATTERN),
            )],
        },
        // A different value of the same class: punctuation only, no spaces.
        Case {
            label: "a body member outside its pattern, punctuation only",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "name": "name.with.dots" })),
            withheld: Some("name.with.dots".to_string()),
            clauses: vec![clause(
                "name",
                &pattern_requirement(DEFINITION_NAME_PATTERN),
            )],
        },
        // One request, two violations of one member: a name one character past the length
        // maximum is outside the pattern's repetition bound too, so the envelope counts
        // both — the bound quoting the value the caller sent, the expression quoting none.
        Case {
            label: "a body member one character past its length maximum",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "name": over_long_name })),
            withheld: None,
            clauses: vec![
                value_clause(&over_long_name, "name", &max_length(DEFINITION_NAME_MAX)),
                clause("name", &pattern_requirement(DEFINITION_NAME_PATTERN)),
            ],
        },
        Case {
            label: "a body member below its length minimum",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "name": "" })),
            withheld: None,
            clauses: vec![
                value_clause("", "name", &min_length(1)),
                clause("name", &pattern_requirement(DEFINITION_NAME_PATTERN)),
            ],
        },
        // A @sensitive member states no value whichever class refused it: the framework
        // withholds the content of a member the model marks sensitive.
        Case {
            label: "a sensitive body member below its length minimum",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "name": "ok-name", "description": "" })),
            withheld: None,
            clauses: vec![clause("description", &min_length(1))],
        },
        Case {
            label: "an idempotency token outside its pattern",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "name": "ok-name", "clientToken": spaced_token.clone() })),
            withheld: Some(spaced_token.clone()),
            clauses: vec![clause(
                "clientToken",
                &pattern_requirement(CLIENT_TOKEN_PATTERN),
            )],
        },
        Case {
            label: "an idempotency token below its length minimum",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "name": "ok-name", "clientToken": "short" })),
            withheld: None,
            clauses: vec![value_clause("short", "clientToken", &min_length(33))],
        },
        Case {
            label: "a required body member absent",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({})),
            withheld: None,
            clauses: vec![absent_clause("name")],
        },
        // A URI label, validated the same way a body member is.
        Case {
            label: "a URI label outside its pattern",
            method: "GET",
            path: "/workflow-definitions/ok-name/workflow-runs/not-a-uuid".to_string(),
            body: None,
            withheld: Some("not-a-uuid".to_string()),
            clauses: vec![clause("workflowRunId", &pattern_requirement(UUID_PATTERN))],
        },
        Case {
            label: "a URI label naming a definition outside its pattern",
            method: "GET",
            path: "/workflow-definitions/not.a.name".to_string(),
            body: None,
            withheld: Some("not.a.name".to_string()),
            clauses: vec![clause(
                "workflowDefinitionName",
                &pattern_requirement(DEFINITION_NAME_PATTERN),
            )],
        },
        // A query parameter, on each side of its modelled range: another class, so the
        // framework's clause — value and all — stands.
        Case {
            label: "a query parameter one below its range minimum",
            method: "POST",
            path: format!("/workflow-definitions?maxResults={}", MAX_RESULTS_MIN - 1),
            body: Some(json!({})),
            withheld: None,
            clauses: vec![value_clause(
                &(MAX_RESULTS_MIN - 1).to_string(),
                "maxResults",
                &format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
            )],
        },
        Case {
            label: "a query parameter one above its range maximum",
            method: "POST",
            path: format!("/workflow-definitions?maxResults={}", MAX_RESULTS_MAX + 1),
            body: Some(json!({})),
            withheld: None,
            clauses: vec![value_clause(
                &(MAX_RESULTS_MAX + 1).to_string(),
                "maxResults",
                &format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
            )],
        },
        // A pattern whose expression carries a backslash escape: stripping the anchors
        // must leave the escape alone.
        Case {
            label: "a query parameter outside its pattern",
            method: "POST",
            path: "/workflow-definitions?nextToken=a%20token".to_string(),
            body: Some(json!({})),
            withheld: Some("a token".to_string()),
            clauses: vec![clause(
                "nextToken",
                &pattern_requirement(NEXT_TOKEN_PATTERN),
            )],
        },
        // An enumerated member: the requirement is the value set the model declares, and
        // the clause names no submitted value — the same half of the rule a `@pattern`
        // clause follows, because the set, like the expression, is what every value of the
        // member must satisfy rather than the one value that did not.
        Case {
            label: "a body member outside its enum value set",
            method: "POST",
            path: "/workflow-definitions".to_string(),
            body: Some(json!({ "sortOrder": "Sideways" })),
            withheld: Some("Sideways".to_string()),
            clauses: vec![clause(
                "sortOrder",
                &format!(
                    "Member must satisfy enum value set: [{}]",
                    SORT_ORDER_VALUES.join(", ")
                ),
            )],
        },
        // Another operation, another member, the same wording.
        Case {
            label: "another operation's optional member outside its pattern",
            method: "PUT",
            path: "/workflow-definitions/ok-name/workflow-runs".to_string(),
            body: Some(json!({
                "modelId": "nova-act-v1",
                "clientInfo": { "compatibilityVersion": 1 },
                "logGroupName": "bad log group!",
            })),
            withheld: Some("bad log group!".to_string()),
            clauses: vec![clause(
                "logGroupName",
                &pattern_requirement(LOG_GROUP_PATTERN),
            )],
        },
        // A URI label and a body member broken by one request: one envelope, both
        // clauses, in the order the walk reaches the members.
        Case {
            label: "a URI label and a body member broken at once",
            method: "PUT",
            path: "/workflow-definitions/not.a.name/workflow-runs".to_string(),
            body: Some(json!({
                "modelId": "nova-act-v1",
                "clientInfo": { "compatibilityVersion": 1 },
                "logGroupName": "bad log group!",
            })),
            withheld: Some("not.a.name".to_string()),
            clauses: vec![
                clause(
                    "workflowDefinitionName",
                    &pattern_requirement(DEFINITION_NAME_PATTERN),
                ),
                clause(
                    "logGroupName",
                    &pattern_requirement(LOG_GROUP_PATTERN),
                ),
            ],
        },
    ]
}

#[tokio::test]
async fn a_pattern_requirement_is_stated_over_the_expression_alone() {
    let emulator = Emulator::new("us-east-1");
    // A definition every row could have resolved, so a row that is refused is refused by
    // its input and not by a missing parent.
    let (status, body) = emulator
        .call("PUT", "/workflow-definitions", Some(json!({ "name": "ok-name" })))
        .await;
    assert_eq!(
        status, 201,
        "the fixture definition must be created, got {status} {body}"
    );

    for case in cases() {
        let label = case.label;
        let (status, answer) = emulator
            .call(case.method, &case.path, case.body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"], "ValidationException",
            "case {label:?}: expected ValidationException, got {answer}"
        );

        let message = text(&answer, "message");
        let expected = envelope(&case.clauses);
        assert_eq!(
            message, expected,
            "case {label:?}: expected message {expected:?}, got {message:?}"
        );

        // The two deviations the rule forbids, asserted on their own so a regression says
        // which half broke: no submitted value in a clause that states none, and no anchors
        // on the expression it quotes.
        if let Some(withheld) = &case.withheld {
            assert!(
                !message.contains(withheld.as_str()),
                "case {label:?}: a clause stating a requirement over every value must not \
                 echo the submitted value {withheld:?}, got {message:?}"
            );
        }
        assert!(
            !message.contains('^') && !message.contains('$'),
            "case {label:?}: a quoted expression carries no ^ or $ anchor, got {message:?}"
        );
    }
}

/// The same requests, once their values are inside the constraints: the wording above is
/// how a refusal reads, not a refusal of everything.
#[tokio::test]
async fn a_value_inside_every_constraint_is_not_refused() {
    let emulator = Emulator::new("eu-west-1");

    // (label, method, path, body, the status it must be answered with)
    let accepted: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "a name at the length maximum",
            "PUT",
            "/workflow-definitions".to_string(),
            Some(json!({ "name": "n".repeat(DEFINITION_NAME_MAX) })),
            201,
        ),
        (
            "a single-character name with a token and a description",
            "PUT",
            "/workflow-definitions".to_string(),
            Some(json!({
                "name": "n",
                "description": "a described definition",
                "clientToken": padded_token("accepted"),
            })),
            201,
        ),
        (
            "a page size at each published bound",
            "POST",
            format!("/workflow-definitions?maxResults={MAX_RESULTS_MIN}"),
            Some(json!({ "sortOrder": SORT_ORDER_VALUES[0] })),
            200,
        ),
        (
            "a page size at the published maximum",
            "POST",
            format!("/workflow-definitions?maxResults={MAX_RESULTS_MAX}"),
            Some(json!({ "sortOrder": SORT_ORDER_VALUES[1] })),
            200,
        ),
    ];

    for (label, method, path, body, expected_status) in accepted {
        let (status, answer) = emulator.call(method, &path, body).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {status} {answer}"
        );
    }
}
