// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused at the request boundary.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk and this service's
//! `override_refusal` all run exactly as they do in the emulator binary.
//!
//! The rule under test is the REQUIREMENT a `@pattern` violation states: the
//! expression a value has to satisfy, without the outer `^`/`$` a model writes to
//! turn Smithy's unanchored find into a whole-string match. The rows span every
//! shape of modelled pattern this service declares — a bare repetition count, a
//! character class, a fixed-width class, an alternation whose own `$`/`^` sit
//! INSIDE it — across six operations and both body and path-bound members, so the
//! rule is read off the pattern rather than off one member. The rest of the
//! message (the count, the clause, the echoed value, the `; ` separator and every
//! other requirement) is the framework's, and the last two tests pin that too.

use serde_json::{json, Value};

use crate::application_lifecycle::{Emulator, Reply};

/// The modelled error code and status a constraint failure is answered with
/// (`smithy_query(query="shape", shape="ValidationException")`: `@httpError 400`).
///
/// `pub(crate)` because `tests/all/pattern_violation_messages.rs` pins the SAME rule on
/// the application-lookup operations: the two suites read one set of constants so they
/// cannot drift on the expression each expects.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

// The patterns the model declares on the members exercised below, written out as
// AWS reports them: the modelled expression with its OUTER anchors removed. Source:
// `smithy_query(query="operation", operation="ListApplications,ListComponents,
// GetApplication,GetOperation,RegisterApplication,PutResourcePermission",
// model_path="services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/com-amazonaws-ssmsap-ssmsap.json")`.

/// `NextToken`: modelled `^.{16,2048}$`.
const NEXT_TOKEN_PATTERN: &str = ".{16,2048}";

/// `ApplicationId`: modelled `^[\w\d\.-]+$`.
pub(crate) const APPLICATION_ID_PATTERN: &str = r"[\w\d\.-]+";

/// `ComponentId`: modelled `^[\w\d-]+$`.
pub(crate) const COMPONENT_ID_PATTERN: &str = r"[\w\d-]+";

/// `DatabaseId`: modelled `[\w\d]+$` — a shape that spells only the END anchor, which is
/// an outer one all the same.
pub(crate) const DATABASE_ID_PATTERN: &str = r"[\w\d]+";

/// `OperationId`: modelled `^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$`.
const OPERATION_ID_PATTERN: &str =
    r"[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?";

/// `SAPInstanceNumber`: modelled `^[0-9]{2}$`.
const SAP_INSTANCE_NUMBER_PATTERN: &str = r"[0-9]{2}";

/// `SID`: modelled `^[A-Z][A-Z0-9]{2}$`.
const SID_PATTERN: &str = r"[A-Z][A-Z0-9]{2}";

/// `Arn` / `SsmSapArn`: modelled
/// `^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$`. Only the OUTER anchors go — the `$`
/// and `^` either side of the alternation are part of the expression.
const ARN_PATTERN: &str = r"arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+";

/// An ARN that satisfies [`ARN_PATTERN`], so a row failing one ARN member is not
/// also failing another.
const VALID_ARN: &str = "arn:aws:ssm-sap:us-east-1:123456789012:application/HDB-TEST";

/// The `message` of a failure body: the framework's pre-dispatch envelope writes the
/// member as `message`, a handler-raised modelled `ValidationException` as `Message`.
///
/// `pub(crate)` for the same reason as the constants above: one reading of a refusal's
/// message for every suite of this target.
pub(crate) fn message(reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| reply.body.get("Message").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

/// A minimal `RegisterApplication` body that satisfies every required member, so a
/// row's own bad member is the only violation.
fn registration(extra: (&str, Value)) -> Value {
    let mut body = json!({
        "ApplicationId": "HDB-TEST",
        "ApplicationType": "HANA",
        "Instances": ["i-0123456789abcdef0"],
    });
    body.as_object_mut()
        .expect("the body is a JSON object")
        .insert(extra.0.to_string(), extra.1);
    body
}

/// One refused request: a label, the operation's URI path, the body, the input
/// position the walk names, the value it echoes and the pattern it must report.
struct PatternCase {
    label: &'static str,
    path: &'static str,
    body: Value,
    member: &'static str,
    submitted: String,
    pattern: &'static str,
}

/// The rows: one per shape of modelled pattern, over six operations.
fn pattern_cases() -> Vec<PatternCase> {
    // 15 characters: one short of `NextToken`'s 16, so the value is inside no other
    // constraint's reach.
    let short_token = "x".repeat(15);
    vec![
        PatternCase {
            label: "a repetition count (ListApplications NextToken)",
            path: "/list-applications",
            body: json!({ "NextToken": "short" }),
            member: "nextToken",
            submitted: "short".to_string(),
            pattern: NEXT_TOKEN_PATTERN,
        },
        PatternCase {
            label: "the same pattern on another operation (ListComponents NextToken)",
            path: "/list-components",
            body: json!({ "NextToken": short_token.clone() }),
            member: "nextToken",
            submitted: short_token,
            pattern: NEXT_TOKEN_PATTERN,
        },
        PatternCase {
            label: "a character class (GetApplication ApplicationId)",
            path: "/get-application",
            body: json!({ "ApplicationId": "has space" }),
            member: "applicationId",
            submitted: "has space".to_string(),
            pattern: APPLICATION_ID_PATTERN,
        },
        PatternCase {
            label: "a grouped fixed-width class (GetOperation OperationId)",
            path: "/get-operation",
            body: json!({ "OperationId": "not-a-uuid" }),
            member: "operationId",
            submitted: "not-a-uuid".to_string(),
            pattern: OPERATION_ID_PATTERN,
        },
        PatternCase {
            label: "a two-digit class (RegisterApplication SapInstanceNumber)",
            path: "/register-application",
            body: registration(("SapInstanceNumber", json!("7"))),
            member: "sapInstanceNumber",
            submitted: "7".to_string(),
            pattern: SAP_INSTANCE_NUMBER_PATTERN,
        },
        PatternCase {
            label: "an upper-case class (RegisterApplication Sid)",
            path: "/register-application",
            body: registration(("Sid", json!("abc"))),
            member: "sid",
            submitted: "abc".to_string(),
            pattern: SID_PATTERN,
        },
        PatternCase {
            label: "an alternation carrying its own anchors (PutResourcePermission ResourceArn)",
            path: "/put-resource-permission",
            body: json!({
                "ActionType": "RESTORE",
                "SourceResourceArn": VALID_ARN,
                "ResourceArn": "not-an-arn",
            }),
            member: "resourceArn",
            submitted: "not-an-arn".to_string(),
            pattern: ARN_PATTERN,
        },
        PatternCase {
            label: "a path-bound member (TagResource resourceArn)",
            path: "/tags/not-an-arn",
            body: json!({ "tags": { "env": "dev" } }),
            member: "resourceArn",
            submitted: "not-an-arn".to_string(),
            pattern: ARN_PATTERN,
        },
    ]
}

/// RULE: a `@pattern` violation is refused as `ValidationException` (HTTP 400) whose
/// message reports the modelled expression WITHOUT the outer `^`/`$` anchors — the
/// anchoring is how the model spells a whole-string match, not part of the
/// expression the service advertises, so a client that re-validates with the regex
/// it read out of the message applies the one AWS reported.
#[tokio::test]
async fn a_pattern_violation_reports_the_expression_without_the_models_outer_anchors() {
    let emulator = Emulator::new();

    for case in pattern_cases() {
        let PatternCase {
            label,
            path,
            body,
            member,
            submitted,
            pattern,
        } = case;
        let reply = emulator.send(path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {label:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
            reply.code()
        );

        let expected = format!(
            "1 validation error detected: Value '{submitted}' at '{member}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {pattern}"
        );
        let actual = message(&reply);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
        // And the anchored spelling is gone, not merely followed by the stripped
        // one: a message carrying `^…$` is the divergence this rule exists for.
        assert!(
            !actual.contains(&format!("pattern: ^{pattern}$")),
            "case {label:?}: the message still reports the model's anchors, got {actual:?}"
        );
    }
}

/// RULE: only the `@pattern` requirement is reworded — the count wording, the clause,
/// the echoed value, the absent-member subject and every other constraint's
/// requirement stay what the framework (and AWS) states.
#[tokio::test]
async fn every_other_half_of_the_envelope_is_pinned_as_a_literal() {
    let emulator = Emulator::new();
    // A valid-charset id one character past `ApplicationId`'s maximum of 60, so the
    // length bound is the only thing it fails.
    let long_id = "a".repeat(61);

    // (label, path, body, the whole expected message)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "an absent required member",
            "/get-operation",
            json!({}),
            "1 validation error detected: Value null at 'operationId' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a @range minimum",
            "/list-applications",
            json!({ "MaxResults": 0 }),
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "a @range maximum",
            "/list-applications",
            json!({ "MaxResults": 51 }),
            "1 validation error detected: Value '51' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 50"
                .to_string(),
        ),
        (
            "a @length maximum",
            "/get-application",
            json!({ "ApplicationId": long_id.clone() }),
            format!(
                "1 validation error detected: Value '{long_id}' at 'applicationId' failed to \
                 satisfy constraint: Member must have length less than or equal to 60"
            ),
        ),
    ];

    for (label, path, body, expected) in rows {
        let reply = emulator.send(path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {label:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
            reply.code()
        );
        let actual = message(&reply);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a request violating SEVERAL constraints is one refusal carrying one clause
/// per violation — the plural count, and the clauses joined with `; ` — each clause
/// stating its own requirement.
#[tokio::test]
async fn several_violations_are_one_envelope_with_one_clause_each() {
    let emulator = Emulator::new();

    let mut body = registration(("Sid", json!("abc")));
    body.as_object_mut()
        .expect("the body is a JSON object")
        .insert("SapInstanceNumber".to_string(), json!("7"));
    let reply = emulator.send("/register-application", body).await;
    assert_eq!(
        reply.status, 400,
        "expected 400, got {}: {:?}",
        reply.status, reply.body
    );
    let actual = message(&reply);
    assert!(
        actual.starts_with("2 validation errors detected: "),
        "expected the plural envelope over two clauses, got {actual:?}"
    );
    for (member, submitted, pattern) in [
        ("sapInstanceNumber", "7", SAP_INSTANCE_NUMBER_PATTERN),
        ("sid", "abc", SID_PATTERN),
    ] {
        let clause = format!(
            "Value '{submitted}' at '{member}' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: {pattern}"
        );
        assert!(
            actual.contains(&clause),
            "case {member:?}: expected the clause {clause:?}, got {actual:?}"
        );
    }
    assert_eq!(
        actual.matches("; ").count(),
        1,
        "expected the two clauses joined with a single `; `, got {actual:?}"
    );
}

/// RULE: the rewording touches only a request the framework refused at the BOUNDARY —
/// a request the model accepts is served, and a refusal a HANDLER raises keeps its own
/// message.
#[tokio::test]
async fn a_request_the_model_accepts_is_still_served() {
    let emulator = Emulator::new();

    // Rows: no member at all, and a page size at each published bound of
    // `MaxResults` (range 1..50).
    let rows: Vec<(&str, Value)> = vec![
        ("no members", json!({})),
        ("the smallest page", json!({ "MaxResults": 1 })),
        ("the largest page", json!({ "MaxResults": 50 })),
    ];

    for (label, body) in rows {
        let reply = emulator.send("/list-applications", body).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200, got {}: {:?}",
            reply.status, reply.body
        );
    }

    // A token at `NextToken`'s 16-character minimum satisfies the modelled pattern,
    // so the boundary passes it to the handler, whose own refusal — a token this
    // account never minted — is reported in its own words rather than wrapped in a
    // constraint envelope.
    let reply = emulator
        .send(
            "/list-applications",
            json!({ "NextToken": "t".repeat(16) }),
        )
        .await;
    assert_eq!(
        reply.status, 400,
        "expected the handler to refuse an unminted token with 400, got {}: {:?}",
        reply.status, reply.body
    );
    let actual = message(&reply);
    assert!(
        !actual.contains("validation error"),
        "expected the handler's own message for a token the boundary accepted, got {actual:?}"
    );
}
