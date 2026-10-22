// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the framework refused before any handler ran.
//!
//! RULE under test: a modeled-constraint violation is answered with HTTP 400 and the
//! error code `ValidationException` (smithy_query `query="service_info"`:
//! `common_errors` carries `ValidationException`, `errorKind` client, `@httpError 400`),
//! and its message is the AWS constraint envelope
//! (`N validation error(s) detected: <clause>[; <clause>]`) over one clause per
//! violation, where each clause
//!
//! 1. names the member path and the violated constraint and NOTHING of the submitted
//!    value — `Value at '<path>' failed to satisfy constraint: <requirement>` — except
//!    for a member that was ABSENT, which is reported as the `Value null` it was,
//!    because that is a fact of the request's shape rather than a value the caller
//!    sent; and
//! 2. quotes a `@pattern` as the expression a value has to satisfy, with the `^` / `$`
//!    anchors the model writes to make the match total stripped off, so a client that
//!    recompiles the quoted expression gets the regex AWS quoted.
//!
//! The tables below span what the rule generalises over: the constraint class (pattern,
//! length, range, absence), the member and its shape, the operation, the submitted
//! value's content and size, and the number of violations in one request — not the one
//! recorded request. Every expected pattern expression is the model's own, read with
//! `smithy_query` and named at its constant.

use serde_json::{json, Value};

use crate::bill_scenario_lifecycle::{error_code, Emulator};

/// `WorkloadEstimateName` / `BillScenarioName` / `BillEstimateName` declare
/// `@pattern "^[a-zA-Z0-9-]+$"` (smithy_query `shape="WorkloadEstimateName"`); this is
/// that expression as a clause has to quote it — without the anchors.
const NAME_PATTERN: &str = "[a-zA-Z0-9-]+";

/// `ResourceId`: `@pattern`
/// `"^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$"`, unanchored.
const RESOURCE_ID_PATTERN: &str =
    "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}";

/// `Arn`: `@pattern`
/// `"^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$"`,
/// unanchored.
const ARN_PATTERN: &str =
    "arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+";

/// `ClientToken`: `@pattern "^[\u0021-\u007E]+$"`, unanchored. The model spells the
/// bounds as the literal six characters `\u0021`, so this is a raw string.
const CLIENT_TOKEN_PATTERN: &str = r"[\u0021-\u007E]+";

/// `ResourceTagValue` (the `Tags` map's value): `@pattern "^[\w\s:+=@/-]*$"`, unanchored.
const TAG_VALUE_PATTERN: &str = r"[\w\s:+=@/-]*";

/// A well-formed `ResourceId` (the shape is `len 36..36` of lowercase hex), so a case
/// that is about ANOTHER member is not also a violation of this one.
const VALID_RESOURCE_ID: &str = "0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0";

/// A well-formed 36-character `ResourceId` EXCEPT for its last character, which is
/// outside `[a-f0-9]`: a pattern violation with no length violation beside it.
const PATTERN_ONLY_RESOURCE_ID: &str = "0f9e8d7c-6b5a-4938-2716-05f4e3d2c1bz";

/// An ARN long enough for `Arn`'s `@length min 20` whose resource segment carries
/// characters `[-a-z0-9/:_]` excludes: a pattern violation with no length violation.
const PATTERN_ONLY_ARN: &str =
    "arn:aws:bcm-pricing-calculator:us-east-1:000000000000:bill-scenario/NOT Allowed";

/// The `message` an awsJson1_0 error body carries. (`error_code`, and the wire harness
/// itself, come from `bill_scenario_lifecycle`: one copy per test target, so no suite
/// drives a different front door.)
fn error_message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One clause of the envelope: the subject (`Value`, or `Value null` for an absent
/// member), the member path, and the requirement.
fn clause(subject: &str, path: &str, requirement: &str) -> String {
    format!("{subject} at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement a `@pattern` violation states.
fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The whole message: the AWS envelope over `clauses`, joined the way AWS joins them.
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [single] => format!("1 validation error detected: {single}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// One row: a request the model refuses, and the whole message it must be refused with.
struct Case {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expected: String,
}

/// RULE (1) and (2) over the constraint classes, members and operations this service
/// declares: status 400, code `ValidationException`, and the exact envelope.
#[tokio::test]
async fn a_constraint_violation_is_stated_without_the_submitted_value() {
    let over_long_name = "a".repeat(65);
    let over_long_name_with_space = format!("{} x", "a".repeat(64));
    let over_long_token = "x".repeat(2049);

    let cases = vec![
        // --- @pattern, the class the recorded instance belongs to ----------------
        Case {
            case: "pattern on a required name",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": "bad name!" }),
            expected: envelope(&[clause(
                "Value",
                "name",
                &pattern_requirement(NAME_PATTERN),
            )]),
        },
        Case {
            case: "pattern, empty value",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": "" }),
            expected: envelope(&[clause(
                "Value",
                "name",
                &pattern_requirement(NAME_PATTERN),
            )]),
        },
        Case {
            case: "pattern, non-ASCII value",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": "estimación" }),
            expected: envelope(&[clause(
                "Value",
                "name",
                &pattern_requirement(NAME_PATTERN),
            )]),
        },
        Case {
            case: "pattern, value carrying a quote",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": "it's bad" }),
            expected: envelope(&[clause(
                "Value",
                "name",
                &pattern_requirement(NAME_PATTERN),
            )]),
        },
        Case {
            case: "pattern on the same name member of another operation",
            operation: "CreateBillScenario",
            request: json!({ "name": "bad name!" }),
            expected: envelope(&[clause(
                "Value",
                "name",
                &pattern_requirement(NAME_PATTERN),
            )]),
        },
        Case {
            case: "pattern on a name beside a well-formed sibling member",
            operation: "CreateBillEstimate",
            request: json!({ "name": "bad name!", "billScenarioId": VALID_RESOURCE_ID }),
            expected: envelope(&[clause(
                "Value",
                "name",
                &pattern_requirement(NAME_PATTERN),
            )]),
        },
        Case {
            case: "pattern on an identifier, a different expression",
            operation: "GetWorkloadEstimate",
            request: json!({ "identifier": PATTERN_ONLY_RESOURCE_ID }),
            expected: envelope(&[clause(
                "Value",
                "identifier",
                &pattern_requirement(RESOURCE_ID_PATTERN),
            )]),
        },
        Case {
            case: "pattern on an identifier of a mutating operation",
            operation: "DeleteBillScenario",
            request: json!({ "identifier": PATTERN_ONLY_RESOURCE_ID }),
            expected: envelope(&[clause(
                "Value",
                "identifier",
                &pattern_requirement(RESOURCE_ID_PATTERN),
            )]),
        },
        Case {
            case: "pattern on an ARN, an expression carrying ':' and '/'",
            operation: "ListTagsForResource",
            request: json!({ "arn": PATTERN_ONLY_ARN }),
            expected: envelope(&[clause(
                "Value",
                "arn",
                &pattern_requirement(ARN_PATTERN),
            )]),
        },
        Case {
            case: "pattern on an idempotency token, an expression carrying backslashes",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": "fine-name", "clientToken": "has a space" }),
            expected: envelope(&[clause(
                "Value",
                "clientToken",
                &pattern_requirement(CLIENT_TOKEN_PATTERN),
            )]),
        },
        // --- @length -------------------------------------------------------------
        Case {
            case: "length maximum, one character past it",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": over_long_name }),
            expected: envelope(&[clause(
                "Value",
                "name",
                "Member must have length less than or equal to 64",
            )]),
        },
        Case {
            case: "length minimum",
            operation: "GetWorkloadEstimate",
            request: json!({ "identifier": "0f9e8d7c" }),
            expected: envelope(&[
                clause(
                    "Value",
                    "identifier",
                    "Member must have length greater than or equal to 36",
                ),
                clause(
                    "Value",
                    "identifier",
                    &pattern_requirement(RESOURCE_ID_PATTERN),
                ),
            ]),
        },
        Case {
            case: "length maximum on a 2 KiB member, far past the elision cap",
            operation: "ListBillScenarios",
            request: json!({ "nextToken": over_long_token }),
            expected: envelope(&[clause(
                "Value",
                "nextToken",
                "Member must have length less than or equal to 2048",
            )]),
        },
        // --- @range --------------------------------------------------------------
        Case {
            case: "range minimum, one below the published bound",
            operation: "ListBillScenarios",
            request: json!({ "maxResults": 0 }),
            expected: envelope(&[clause(
                "Value",
                "maxResults",
                "Member must have value greater than or equal to 1",
            )]),
        },
        Case {
            case: "range maximum, one above the published bound",
            operation: "ListBillScenarios",
            request: json!({ "maxResults": 26 }),
            expected: envelope(&[clause(
                "Value",
                "maxResults",
                "Member must have value less than or equal to 25",
            )]),
        },
        Case {
            case: "range maximum on another paginated operation",
            operation: "ListWorkloadEstimates",
            request: json!({ "maxResults": 1000 }),
            expected: envelope(&[clause(
                "Value",
                "maxResults",
                "Member must have value less than or equal to 25",
            )]),
        },
        // --- absence: the ONE clause that still names a subject other than `Value`
        Case {
            case: "a required member absent from the request",
            operation: "CreateWorkloadEstimate",
            request: json!({}),
            expected: envelope(&[clause("Value null", "name", "Member must not be null")]),
        },
        Case {
            case: "a required identifier absent from the request",
            operation: "GetWorkloadEstimate",
            request: json!({}),
            expected: envelope(&[clause(
                "Value null",
                "identifier",
                "Member must not be null",
            )]),
        },
        // --- several violations of one member, counted and joined ----------------
        Case {
            case: "one member over its length maximum AND outside its pattern",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": over_long_name_with_space }),
            expected: envelope(&[
                clause(
                    "Value",
                    "name",
                    "Member must have length less than or equal to 64",
                ),
                clause("Value", "name", &pattern_requirement(NAME_PATTERN)),
            ]),
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let (status, body) = emulator.call(case.operation, case.request.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?} ({}): expected 400, got {status} with {body}",
            case.case, case.operation
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?} ({}): expected ValidationException, got {body}",
            case.case,
            case.operation
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, case.expected,
            "case {:?} ({}): expected message {:?}, got {actual:?}",
            case.case, case.operation, case.expected
        );
    }
}

/// RULE (1), as an INVARIANT over the submitted value: whatever a caller sends, the
/// refusal text does not carry it back — not the value, not a prefix of it, and not the
/// framework's `... [<n> bytes]` elision marker, which is wire text AWS never emits.
#[tokio::test]
async fn no_submitted_value_reaches_the_refusal_text() {
    // Each value violates `WorkloadEstimateName`'s `@pattern`/`@length`, and each is a
    // different hazard: punctuation, a quote, a newline, a non-ASCII grapheme, a value
    // far over the framework's 256-byte echo cap, and one that itself looks like the
    // elision marker.
    let long = "z".repeat(4096);
    let values = [
        "bad name!",
        "it's bad",
        "two\nlines",
        "estimación",
        "a b",
        long.as_str(),
        "zzz ... [7 bytes]",
    ];

    let emulator = Emulator::new();
    for value in values {
        let (status, body) = emulator
            .call("CreateWorkloadEstimate", json!({ "name": value }))
            .await;
        let message = error_message(&body);
        assert_eq!(
            status, 400,
            "case {value:?}: expected 400, got {status} with {body}"
        );
        assert!(
            !message.contains(value),
            "case {value:?}: the submitted value is echoed in {message:?}"
        );
        // A prefix long enough to be recognisable as the caller's input, which is what
        // an elided echo would carry.
        let prefix: String = value.chars().take(8).collect();
        assert!(
            !message.contains(&prefix),
            "case {value:?}: a prefix of the submitted value reaches {message:?}"
        );
        assert!(
            !message.contains("... ["),
            "case {value:?}: the framework's elision marker reaches {message:?}"
        );
        // The message stays the size of the rule it states, not the size of the request.
        assert!(
            message.len() < 512,
            "case {value:?}: the message is request-sized ({} bytes): {message:?}",
            message.len()
        );
    }
}

/// RULE (2), as an invariant over every `@pattern` this service's members declare: the
/// expression a clause quotes carries no `^` prefix and no `$` suffix, while the rest of
/// the expression — including the characters a regex needs — survives intact.
#[tokio::test]
async fn a_quoted_pattern_carries_no_anchors() {
    let cases = [
        (
            "CreateWorkloadEstimate",
            json!({ "name": "bad name!" }),
            NAME_PATTERN,
        ),
        (
            "CreateBillScenario",
            json!({ "name": "bad name!" }),
            NAME_PATTERN,
        ),
        (
            "GetWorkloadEstimate",
            json!({ "identifier": PATTERN_ONLY_RESOURCE_ID }),
            RESOURCE_ID_PATTERN,
        ),
        (
            "ListTagsForResource",
            json!({ "arn": PATTERN_ONLY_ARN }),
            ARN_PATTERN,
        ),
        (
            "CreateWorkloadEstimate",
            json!({ "name": "fine-name", "clientToken": "has a space" }),
            CLIENT_TOKEN_PATTERN,
        ),
        (
            "TagResource",
            json!({
                "arn": format!(
                    "arn:aws:bcm-pricing-calculator:us-east-1:000000000000:workload-estimate/{VALID_RESOURCE_ID}"
                ),
                "tags": { "team": "finops!" },
            }),
            TAG_VALUE_PATTERN,
        ),
    ];

    let emulator = Emulator::new();
    for (operation, request, expression) in cases {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {operation} {request}: expected 400, got {status} with {body}"
        );
        let message = error_message(&body);
        let marker = "Member must satisfy regular expression pattern: ";
        let quoted = message
            .rsplit(marker)
            .next()
            .unwrap_or_default()
            .trim_end_matches(';')
            .to_string();
        assert_eq!(
            quoted, expression,
            "case {operation} {request}: expected the pattern {expression:?}, got {quoted:?} \
             in {message:?}"
        );
        assert!(
            !quoted.starts_with('^') && !quoted.ends_with('$'),
            "case {operation} {request}: the quoted pattern keeps its anchors: {quoted:?}"
        );
    }
}

/// The other half of a bound: a value AT a published bound is not refused at all. Only
/// the refusal is asserted away — a scaffolded operation still answers 501 for a request
/// it accepts, and that is not a validation failure.
#[tokio::test]
async fn a_value_at_a_published_bound_is_not_refused() {
    // `MaxResults` is `@range 1..25`; `WorkloadEstimateName` is `@length ..64` over
    // `[a-zA-Z0-9-]`; `ClientToken` is `@length 1..64` over printable ASCII.
    let longest_name = "a".repeat(64);
    let longest_token = "t".repeat(64);
    let cases = [
        ("ListBillScenarios", json!({ "maxResults": 1 })),
        ("ListBillScenarios", json!({ "maxResults": 25 })),
        ("ListWorkloadEstimates", json!({ "maxResults": 25 })),
        ("CreateWorkloadEstimate", json!({ "name": "a" })),
        ("CreateWorkloadEstimate", json!({ "name": longest_name })),
        (
            "CreateWorkloadEstimate",
            json!({ "name": "Mixed-Case-123", "clientToken": longest_token }),
        ),
        (
            "CreateWorkloadEstimate",
            json!({ "name": "fine", "clientToken": "!" }),
        ),
        (
            "GetWorkloadEstimate",
            json!({ "identifier": VALID_RESOURCE_ID }),
        ),
    ];

    let emulator = Emulator::new();
    for (operation, request) in cases {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_ne!(
            error_code(&body),
            "ValidationException",
            "case {operation} {request}: a value at its published bound was refused \
             with {status} and {body}"
        );
    }
}
