// SPDX-License-Identifier: CC0-1.0

//! How this service answers an `Arn` that names no notification rule of its own.
//!
//! The RULE: every operation that addresses a notification rule by ARN runs the
//! resource-based authorization check FIRST, before it looks anything up — so a value
//! outside the notification-rule ARN form (`NotificationRuleArn`'s
//! `^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$`)
//! identifies no resource of this service, nothing can authorize the caller for it, and
//! the answer is HTTP 403 `AccessDeniedException` over fixed text either side of the ARN
//! exactly as submitted. That answer is deliberately distinct from the one a WELL-FORMED
//! ARN naming no rule gets, which is each operation's own absence answer.
//!
//! Each suite asserts that rule over a TABLE: the rows differ in WHICH field of the ARN
//! form the value breaks (the resource type, the service namespace, the partition, the
//! region, the account, the resource prefix, trailing whitespace inside the id, no ARN at
//! all) crossed with every operation that carries the member, rather than pinning the one
//! malformed value an issue happened to quote.
//!
//! The harness (`Emulator`, `error_code`, `message`, `string`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use serde_json::{json, Value};

use crate::notification_rule_lifecycle::{error_code, message, string, Emulator};

/// The region every call in this suite is signed for.
const REGION: &str = "us-east-1";

/// A target address used wherever an operation needs one.
const TARGET: &str = "arn:aws:sns:us-east-1:123456789012:authz-topic";

/// A well-formed `CreateNotificationRule` body.
fn create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
        "DetailType": "BASIC",
        "EventTypeIds": ["codecommit-repository-comments-on-commits"],
        "Targets": [{ "TargetType": "SNS", "TargetAddress": TARGET }],
    })
}

/// The 403 text: fixed either side of the ARN exactly as submitted.
fn not_authorized(arn: &str) -> String {
    format!(
        "You don't have sufficient permissions to perform operations on the following \
         resource: {arn}. For more information, see the notifications feature \
         documentation in the user guide for the Developer Tools console."
    )
}

/// Every operation that names a notification rule by ARN, with a body that carries that
/// ARN and is otherwise well formed, so the ARN is the only thing under test.
const RULE_ARN_OPERATIONS: &[(&str, &str)] = &[
    ("DescribeNotificationRule", "/describeNotificationRule"),
    ("DeleteNotificationRule", "/deleteNotificationRule"),
    ("UpdateNotificationRule", "/updateNotificationRule"),
    ("Subscribe", "/subscribe"),
    ("Unsubscribe", "/unsubscribe"),
    ("TagResource", "/tagResource"),
];

/// The request each of those operations sends for `arn`.
fn request_for(path: &str, arn: &str) -> Value {
    match path {
        "/updateNotificationRule" => json!({ "Arn": arn, "Status": "DISABLED" }),
        "/subscribe" => json!({
            "Arn": arn,
            "Target": { "TargetType": "SNS", "TargetAddress": TARGET },
        }),
        "/unsubscribe" => json!({ "Arn": arn, "TargetAddress": TARGET }),
        "/tagResource" => json!({ "Arn": arn, "Tags": { "team": "dev" } }),
        _ => json!({ "Arn": arn }),
    }
}

/// The malformed forms, each breaking ONE field of the ARN form. `id` is the rule id of a
/// rule that really exists, so nothing in a row depends on the resource being absent.
fn malformed_forms(id: &str) -> Vec<(String, String)> {
    vec![
        (
            "the resource-type segment is not notificationrule".to_string(),
            format!("arn:aws:codestar-notifications:{REGION}:000000000000:wrongsegment/{id}"),
        ),
        (
            "another service's namespace".to_string(),
            format!("arn:aws:codepipeline:{REGION}:000000000000:notificationrule/{id}"),
        ),
        (
            "a partition that is not an aws one".to_string(),
            format!("arn:notaws:codestar-notifications:{REGION}:000000000000:notificationrule/{id}"),
        ),
        (
            "an empty region".to_string(),
            format!("arn:aws:codestar-notifications::000000000000:notificationrule/{id}"),
        ),
        (
            "an account that is not twelve digits".to_string(),
            format!("arn:aws:codestar-notifications:{REGION}:12345:notificationrule/{id}"),
        ),
        (
            "an account that is not digits at all".to_string(),
            format!("arn:aws:codestar-notifications:{REGION}:00000000000a:notificationrule/{id}"),
        ),
        (
            "no resource prefix at all".to_string(),
            format!("arn:aws:codestar-notifications:{REGION}:000000000000:{id}"),
        ),
        (
            "an id that ends on whitespace".to_string(),
            format!("arn:aws:codestar-notifications:{REGION}:000000000000:notificationrule/{id} "),
        ),
        (
            "not an ARN at all".to_string(),
            "not-an-arn".to_string(),
        ),
        (
            "too few ARN fields".to_string(),
            "arn:aws:codestar-notifications:notificationrule/x".to_string(),
        ),
    ]
}

/// RULE: a value outside the notification-rule ARN form is answered 403
/// `AccessDeniedException` with the fixed text either side of the submitted value — by
/// every operation that names a rule by ARN, for every field of the form it breaks.
#[tokio::test]
async fn an_arn_that_names_no_rule_of_this_service_is_not_authorized() {
    let emulator = Emulator::new();

    // A rule that really exists, so a refusal cannot be mistaken for absence.
    let created = emulator
        .ok("seed", "/createNotificationRule", &create_body("authz-seed"))
        .await;
    let arn = string(&created, "Arn");
    let id = arn
        .rsplit_once('/')
        .map(|(_, id)| id.to_string())
        .expect("a rule ARN carries its id");

    for (form, value) in malformed_forms(&id) {
        for (operation, path) in RULE_ARN_OPERATIONS {
            let case = format!("{operation} with {form}");
            let (status, body) = emulator.post(path, &request_for(path, &value)).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?}: got {body}"
            );
            let stated = message(&body);
            let expected = not_authorized(&value);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    // Nothing above touched the rule that exists: it still describes, with the one target
    // it was created with.
    let described = emulator
        .ok("seed", "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        string(&described, "Name"),
        "authz-seed",
        "a refused call must not have changed the rule, got {described}"
    );
    assert_eq!(
        described
            .get("Targets")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "a refused call must not have changed the rule's targets, got {described}"
    );
    assert_eq!(
        string(&described, "Status"),
        "ENABLED",
        "a refused update must not have changed the status, got {described}"
    );
    assert!(
        described
            .get("Tags")
            .and_then(Value::as_object)
            .is_some_and(serde_json::Map::is_empty),
        "a refused tagging must not have tagged the rule, got {described}"
    );
}

/// RULE: a WELL-FORMED rule ARN that names no rule is a different answer — each
/// operation's own, and never the 403 above. This is what the three-way distinction
/// (403 not authorized / absent / served) rests on.
///
/// The rows differ in the form of the rule id the ARN carries, so the answer cannot be
/// keyed to one value.
#[tokio::test]
async fn a_well_formed_arn_no_rule_holds_is_not_answered_as_a_denial() {
    let emulator = Emulator::new();

    // (operation, path, expected status, expected code)
    let expectations: &[(&str, &str, u16, &str)] = &[
        (
            "DescribeNotificationRule",
            "/describeNotificationRule",
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateNotificationRule",
            "/updateNotificationRule",
            404,
            "ResourceNotFoundException",
        ),
        ("Subscribe", "/subscribe", 404, "ResourceNotFoundException"),
        ("TagResource", "/tagResource", 404, "ResourceNotFoundException"),
        // Models no not-found error, so it states the absence in the one client error it
        // does model.
        ("Unsubscribe", "/unsubscribe", 400, "ValidationException"),
    ];

    for id in [
        "11111111-1111-4111-8111-111111111111",
        "r1",
        "a-notification-rule-id-of-some-length",
        "abcdef0123456789",
    ] {
        let arn =
            format!("arn:aws:codestar-notifications:{REGION}:000000000000:notificationrule/{id}");
        for (operation, path, expected_status, expected_code) in expectations {
            let case = format!("{operation} on an absent {id}");
            let (status, body) = emulator.post(path, &request_for(path, &arn)).await;
            assert_eq!(
                status, *expected_status,
                "case {case:?}: expected {expected_status}, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                *expected_code,
                "case {case:?}: got {body}"
            );
            assert_ne!(
                message(&body),
                not_authorized(&arn),
                "case {case:?}: an absent rule is not reported as a denial, got {body}"
            );
        }

        // The idempotent delete answers for the ARN it was given, absent or not.
        let deleted = emulator
            .ok(
                "absent delete",
                "/deleteNotificationRule",
                &json!({ "Arn": arn }),
            )
            .await;
        assert_eq!(
            string(&deleted, "Arn"),
            arn,
            "case {id:?}: a delete of a well-formed absent ARN stays idempotent, got {deleted}"
        );
    }
}

/// RULE: the form check is the ARN's, not the region's — an ARN of this service's own
/// form for ANOTHER region is authorized (it names a rule of this service) and is then
/// reported absent by the endpoint that does not hold it.
#[tokio::test]
async fn a_rule_arn_from_another_region_is_absent_rather_than_denied() {
    let emulator = Emulator::new();

    for (case, home, other) in [
        ("created in us-east-1", "us-east-1", "eu-west-1"),
        ("created in ap-south-1", "ap-south-1", "us-west-2"),
    ] {
        let (status, created) = emulator
            .post_in(home, "/createNotificationRule", &create_body("cross-region"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status} with {created}"
        );
        let arn = string(&created, "Arn");

        let (status, body) = emulator
            .post_in(other, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: another region reports it absent, not denied, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body}"
        );
    }
}
