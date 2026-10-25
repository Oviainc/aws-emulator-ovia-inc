// SPDX-License-Identifier: CC0-1.0

//! What this service's failures SAY, over the wire.
//!
//! The status and the error code of every modelled failure are asserted by the suites
//! beside this one; this suite asserts the `Message` member, which is part of the
//! contract too — a client surfaces it, parses an identifier out of it, or matches its
//! wording to classify a failure.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * a boundary constraint refusal states the member path and the constraint the
//!   framework reported, withholds the value of a member the model marks `@sensitive`,
//!   quotes it for every other member, and advertises the `@pattern` expression this
//!   service publishes for the member;
//! * the name-already-taken refusal is the service's FIXED text: it interpolates neither
//!   the submitted name nor the rule that holds it;
//! * an absent notification rule is reported from the SUBMITTED ARN, in the form the
//!   calling operation states — the reporting path by the rule ID the ARN carries plus
//!   the sentence telling the caller to check it, the mutating paths as
//!   `A notification rule with the Id <identifier> was not found.` — so the message is
//!   per-operation rather than one string for every call.
//!
//! The harness (`Emulator`, `error_code`, `string`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use serde_json::{json, Value};

use crate::notification_rule_lifecycle::{error_code, string, Emulator};

/// The one text this service states for a rule-name conflict, whichever operation
/// refuses: it states the name-uniqueness rule and interpolates nothing.
const TAKEN_NAME_TEXT: &str = "The notification rule was not created because another \
                              notification rule already exists with the same name. \
                              Notification rule names must be unique within an AWS \
                              account. This helps prevent exceeding the maximum allowed \
                              number of notification rules.";

/// The `Message` member of an error body. restJson1 renders it under the modelled member
/// name; a client that lowercases it is served by the second lookup.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A well-formed `CreateNotificationRule` body, which each row then breaks in ONE place.
fn create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
        "DetailType": "BASIC",
        "EventTypeIds": ["codecommit-repository-comments-on-commits"],
        "Targets": [],
    })
}

/// An ARN of this service's own form, for a rule nothing created.
fn absent_rule_arn(region: &str, id: &str) -> String {
    format!("arn:aws:codestar-notifications:{region}:000000000000:notificationrule/{id}")
}

/// RULE: a refusal the framework raises before any handler runs is stated as
/// `N validation error(s) detected: ` over one clause per violation; a clause names the
/// member path and the constraint, quotes the submitted value only for a member the model
/// does not mark `@sensitive`, reports an absent member as `null`, and advertises the
/// `@pattern` expression this service publishes for the member.
///
/// The rows differ along what the rule generalises over: the constraint class (`@pattern`,
/// `@length` at both bounds, an absent `@required` member), whether the member is
/// `@sensitive`, how deeply the member is nested (top level, and inside a structure), and
/// which operation carries it. The expressions and bounds are the model's own
/// (`smithy_query(query="operation",operation="CreateNotificationRule,Subscribe,Unsubscribe")`).
#[tokio::test]
async fn a_boundary_refusal_withholds_a_sensitive_members_value_and_advertises_the_pattern() {
    let long_name = "n".repeat(65);
    let long_address = format!("arn:aws:sns:us-east-1:123456789012:{}", "t".repeat(300));
    let arn = absent_rule_arn("us-east-1", "11111111-1111-4111-8111-111111111111");

    let mut absent_name = create_body("placeholder");
    absent_name
        .as_object_mut()
        .expect("the body is an object")
        .remove("Name");

    // (case, path, request, the whole message expected)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "the sensitive Name outside its pattern",
            "/createNotificationRule",
            create_body("bad name!"),
            r"1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9\-_ ]+$".to_string(),
        ),
        (
            "the sensitive Name over its length maximum",
            "/createNotificationRule",
            create_body(&long_name),
            "1 validation error detected: Value at 'name' failed to satisfy constraint: Member \
             must have length less than or equal to 64"
                .to_string(),
        ),
        (
            "the sensitive Name absent",
            "/createNotificationRule",
            absent_name,
            "1 validation error detected: Value null at 'name' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
        (
            "a non-sensitive member outside its pattern",
            "/createNotificationRule",
            json!({
                "Name": "resource-case",
                "Resource": "not-an-arn",
                "DetailType": "BASIC",
                "EventTypeIds": ["codecommit-repository-comments-on-commits"],
                "Targets": [],
            }),
            r"1 validation error detected: Value 'not-an-arn' at 'resource' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[^:\s]*:[^:\s]*:[^:\s]*:[0-9]{12}:[^\s]+$".to_string(),
        ),
        (
            "a non-sensitive token outside its pattern",
            "/createNotificationRule",
            json!({
                "Name": "token-case",
                "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                "DetailType": "BASIC",
                "EventTypeIds": ["codecommit-repository-comments-on-commits"],
                "Targets": [],
                "ClientRequestToken": "bad token",
            }),
            r"1 validation error detected: Value 'bad token' at 'clientRequestToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w:/-]+$".to_string(),
        ),
        (
            "a sensitive member nested in a structure, under its length minimum",
            "/subscribe",
            json!({
                "Arn": arn,
                "Target": { "TargetType": "SNS", "TargetAddress": "" },
            }),
            "1 validation error detected: Value at 'target.targetAddress' failed to satisfy \
             constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "a top-level sensitive member over its length maximum",
            "/unsubscribe",
            json!({ "Arn": arn, "TargetAddress": long_address }),
            "1 validation error detected: Value at 'targetAddress' failed to satisfy constraint: \
             Member must have length less than or equal to 320"
                .to_string(),
        ),
    ];

    let emulator = Emulator::new();

    for (case, path, request, expected) in rows {
        let (status, body) = emulator.post(path, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a constraint refusal is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: got {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: the name-already-taken refusal is the service's fixed text — it names neither the
/// submitted name (`NotificationRuleName` is `@sensitive`) nor the rule that holds it.
///
/// The rows differ along the name's own form (plain, spaced, underscored, hyphenated, and
/// the modelled 64-character maximum), so nothing about the message can be keyed to the
/// value: every row states the same sentence.
#[tokio::test]
async fn the_taken_name_refusal_is_a_constant_that_echoes_neither_the_name_nor_the_rule() {
    const EXPECTED: &str = TAKEN_NAME_TEXT;

    let max_length_name = "N".repeat(64);
    let names = [
        "taken",
        "Taken Rule With Spaces",
        "taken_rule_1",
        "taken-rule-2",
        max_length_name.as_str(),
    ];

    let emulator = Emulator::new();

    for name in names {
        let first = emulator
            .ok(name, "/createNotificationRule", &create_body(name))
            .await;
        let arn = string(&first, "Arn");

        // Same name, everything else different, so only the name can be the conflict.
        let mut second = create_body(name);
        second["Resource"] = json!("arn:aws:codebuild:us-east-1:123456789012:project/other");
        second["DetailType"] = json!("FULL");
        let (status, body) = emulator.post("/createNotificationRule", &second).await;

        assert_eq!(
            status, 409,
            "case {name:?}: a taken name is a 409, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceAlreadyExistsException",
            "case {name:?}: got {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, EXPECTED,
            "case {name:?}: expected the service's fixed text, got {stated:?}"
        );
        assert!(
            !stated.contains(name),
            "case {name:?}: the submitted name must not be echoed, got {stated:?}"
        );
        assert!(
            !stated.contains(&arn) && !stated.contains("notificationrule/"),
            "case {name:?}: the conflicting rule must not be named, got {stated:?}"
        );
    }
}

/// RULE: a rename onto a name another rule in the region holds is refused with the SAME
/// text the create path states for the class, and echoes neither the submitted name nor
/// the other rule.
///
/// `UpdateNotificationRule` models no `ResourceAlreadyExistsException`, so only the SHAPE
/// differs: the refusal is this service's `ValidationException` (HTTP 400).
#[tokio::test]
async fn a_rename_onto_a_taken_name_echoes_neither_the_name_nor_the_other_rule() {
    const EXPECTED: &str = TAKEN_NAME_TEXT;

    let emulator = Emulator::new();

    // Rows differ in the name being taken, so the refusal cannot be keyed to the value.
    for (index, taken) in ["held", "Held Rule", "held_rule-3"].iter().enumerate() {
        let holder = emulator
            .ok(taken, "/createNotificationRule", &create_body(taken))
            .await;
        let holder_arn = string(&holder, "Arn");
        let mover = emulator
            .ok(
                taken,
                "/createNotificationRule",
                &create_body(&format!("mover-{index}")),
            )
            .await;
        let mover_arn = string(&mover, "Arn");

        let (status, body) = emulator
            .post(
                "/updateNotificationRule",
                &json!({ "Arn": mover_arn, "Name": taken }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {taken:?}: a rename onto a taken name is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {taken:?}: got {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, EXPECTED,
            "case {taken:?}: expected the service's fixed text, got {stated:?}"
        );
        assert!(
            !stated.contains(taken) && !stated.contains(&holder_arn),
            "case {taken:?}: neither the name nor the holding rule may be echoed, got {stated:?}"
        );
    }
}

/// RULE: an absent notification rule is reported from the SUBMITTED ARN, in the form the
/// calling operation states — the reporting path names the rule ID the ARN carries and
/// tells the caller to check it; the mutating paths state
/// `A notification rule with the Id <identifier> was not found.`, over the identifier each
/// of them names the rule by.
///
/// The rows differ along the region the ARN is signed for and the form of the rule id it
/// carries (a UUID, a short opaque id, a long one, and a hyphen-free one), so nothing in
/// the message can be keyed to one recorded ARN.
#[tokio::test]
async fn an_absent_rule_is_reported_from_the_submitted_arn_per_operation() {
    // (case, region, rule id)
    let rows: [(&str, &str, &str); 4] = [
        ("a uuid id", "us-east-1", "11111111-1111-4111-8111-111111111111"),
        ("a short opaque id", "us-east-1", "r1"),
        ("a long id", "eu-west-1", "a-notification-rule-id-of-some-length"),
        ("an id without hyphens", "us-east-1", "abcdef0123456789"),
    ];

    let emulator = Emulator::new();

    for (case, region, id) in rows {
        let arn = absent_rule_arn(region, id);

        let reporting_form = format!(
            "A notification rule with the following ID cannot be found: {id}. Verify that the \
             notification rule ID is correct, and then try again."
        );
        let by_id = format!("A notification rule with the Id {id} was not found.");
        let by_arn = format!("A notification rule with the Id {arn} was not found.");

        // (operation, path, request, expected status, expected code, expected message)
        let calls: [(&str, &str, Value, u16, &str, &str); 4] = [
            (
                "DescribeNotificationRule",
                "/describeNotificationRule",
                json!({ "Arn": arn }),
                404,
                "ResourceNotFoundException",
                reporting_form.as_str(),
            ),
            (
                "UpdateNotificationRule",
                "/updateNotificationRule",
                json!({ "Arn": arn, "Status": "DISABLED" }),
                404,
                "ResourceNotFoundException",
                by_id.as_str(),
            ),
            (
                "Subscribe",
                "/subscribe",
                json!({
                    "Arn": arn,
                    "Target": {
                        "TargetType": "SNS",
                        "TargetAddress": "arn:aws:sns:us-east-1:123456789012:topic",
                    },
                }),
                404,
                "ResourceNotFoundException",
                by_arn.as_str(),
            ),
            (
                // Models no not-found error, so the absence is stated in the one client
                // error it does model — with the same per-operation identifier form.
                "Unsubscribe",
                "/unsubscribe",
                json!({
                    "Arn": arn,
                    "TargetAddress": "arn:aws:sns:us-east-1:123456789012:topic",
                }),
                400,
                "ValidationException",
                by_id.as_str(),
            ),
        ];

        for (operation, path, request, expected_status, expected_code, expected_message) in calls {
            let (status, body) = emulator.post_in(region, path, &request).await;
            assert_eq!(
                status, expected_status,
                "case {case:?} on {operation}: expected {expected_status}, got {status} with \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} on {operation}: got {body}"
            );
            let stated = message(&body);
            assert_eq!(
                stated, expected_message,
                "case {case:?} on {operation}: expected {expected_message:?}, got {stated:?}"
            );
        }

        // The reporting form names the rule by ID alone, and the subscribe form by the
        // whole ARN: the message is per-operation, not one string for every call.
        assert!(
            !reporting_form.contains("arn:aws"),
            "case {case:?}: the reporting form names the ID, not the ARN, got \
             {reporting_form:?}"
        );
        assert_ne!(
            by_id, by_arn,
            "case {case:?}: the two mutating forms name different identifiers"
        );
    }
}

/// RULE: a rule that EXISTS is served, so none of the above fires on a present rule —
/// the not-found text is reached only by an ARN this region does not hold, and a rule
/// created in one region is absent from another.
#[tokio::test]
async fn a_present_rule_is_served_and_only_another_regions_endpoint_reports_it_absent() {
    let emulator = Emulator::new();

    for (case, region, other) in [
        ("created in us-east-1", "us-east-1", "eu-west-1"),
        ("created in ap-south-1", "ap-south-1", "us-west-2"),
    ] {
        let (status, created) = emulator
            .post_in(region, "/createNotificationRule", &create_body("present"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status} with {created}"
        );
        let arn = string(&created, "Arn");

        let (status, described) = emulator
            .post_in(region, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rule's own region serves it, got {status} with {described}"
        );
        assert_eq!(
            message(&described),
            String::new(),
            "case {case:?}: a served read carries no message, got {described}"
        );

        let (status, body) = emulator
            .post_in(other, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: another region does not hold it, got {status} with {body}"
        );
        let id = arn.rsplit('/').next().unwrap_or_default();
        assert_eq!(
            message(&body),
            format!(
                "A notification rule with the following ID cannot be found: {id}. Verify that \
                 the notification rule ID is correct, and then try again."
            ),
            "case {case:?}: expected the reporting form over the ARN's own id, got {body}"
        );
    }
}
