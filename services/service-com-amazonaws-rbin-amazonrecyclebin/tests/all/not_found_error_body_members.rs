// SPDX-License-Identifier: CC0-1.0

//! RULE: an error body carries only the members the service populates for the condition
//! it is reporting — an OPTIONAL member it does not populate is absent from the wire
//! body, and a value its model enum declares is not reason enough to send it. For this
//! service that member is `ResourceNotFoundException$Reason`: the 404 for a retention
//! rule the request addressed and this Region does not hold carries `Message` and NO
//! `Reason`.
//!
//! The table spans the class the rule generalises over, so no single request is pinned:
//!
//! 1. `an_absent_rule_is_reported_with_message_and_no_reason` drives all EIGHT
//!    rule-addressed operations — the five addressed by `Identifier` and the three
//!    addressed by `ResourceArn` — over identifiers that differ in form (lowercase,
//!    uppercase, mixed, all digits) and in WHY they name no rule (never minted, or
//!    created and then deleted), plus well-formed rule ARNs whose Region, account or
//!    rule id names no rule this Region holds. Every 404 body must hold exactly the
//!    wire error code and `Message`.
//! 2. `the_reason_members_this_service_does_populate_are_still_sent` is the contrast that
//!    keeps rule 1 narrow: the three other modelled errors still report their own fault
//!    class, so the omission belongs to the one member the service leaves unset and is
//!    not a blanket drop of optional error members.
//!
//! Every case drives the service through the real router, so the wire protocol, the
//! route table, the input-validation gate and the handlers are exercised exactly as a
//! signed SDK request would exercise them.

use serde_json::{json, Value};

use service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_TAGS_PER_RULE;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_response_bodies::{created_rule, error_code, message, with_arn_segment};
use crate::resource_tags::{keys, list_tags, pairs, tag_resource, untag_resource};
use crate::retention_rule_lifecycle::{
    UNLOCK_DELAY_MIN, create_rule, delete_rule, get_rule, lock_configuration, retention, router,
    update_rule,
};
use crate::rule_lock_transitions::{lock_rule, unlock_rule};

/// The members a `ResourceNotFoundException` body of this service may carry: the wire
/// error code the protocol adds, and the one modelled member the service populates.
/// `Reason` is modelled but NOT sent on this path.
/// Source: smithy_query(query="shape", shape="ResourceNotFoundException") — members
/// `{Message, Reason}`, neither required.
const NOT_FOUND_BODY_MEMBERS: [&str; 2] = ["__type", "Message"];

/// Every modelled member name of the error, so a body is checked for the ABSENCE of the
/// one that must not be sent by name rather than by counting keys alone.
const WITHHELD_MEMBER: &str = "Reason";

/// The responses the eight rule-addressed operations give for a rule addressed by
/// `identifier`, keyed by operation. The three ARN-addressed operations address the same
/// absent rule through `arn`, and every row carries the rule's IDENTIFIER: that is what
/// the refusal states, whichever of the two the request named the rule by.
async fn rule_addressed_refusals(
    router: &MultiServiceRouter,
    identifier: &str,
    arn: &str,
) -> Vec<(&'static str, String, (u16, Value))> {
    vec![
        (
            "GetRule",
            identifier.to_string(),
            get_rule(router, identifier).await,
        ),
        (
            "UpdateRule",
            identifier.to_string(),
            update_rule(router, identifier, json!({ "Description": "anything" })).await,
        ),
        (
            "DeleteRule",
            identifier.to_string(),
            delete_rule(router, identifier).await,
        ),
        (
            "LockRule",
            identifier.to_string(),
            lock_rule(
                router,
                identifier,
                json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
            )
            .await,
        ),
        (
            "UnlockRule",
            identifier.to_string(),
            unlock_rule(router, identifier).await,
        ),
        (
            "ListTagsForResource",
            identifier.to_string(),
            list_tags(router, arn).await,
        ),
        (
            "TagResource",
            identifier.to_string(),
            tag_resource(router, arn, &pairs(&[("team", "platform")])).await,
        ),
        (
            "UntagResource",
            identifier.to_string(),
            untag_resource(router, arn, &keys(&["team"])).await,
        ),
    ]
}

/// The one body assertion of rule 1: status, error code, the message composed from the
/// rule's own identifier, and NO member beyond the ones the service populates.
fn assert_not_found_body(case: &str, identifier: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {body:?}"
    );
    let expected = format!("Recycle Bin Rule '{identifier}' does not exist.");
    assert_eq!(
        message(body),
        expected,
        "case {case:?}: expected Message {expected:?}, got {:?}",
        message(body)
    );
    assert_eq!(
        body.get(WITHHELD_MEMBER),
        None,
        "case {case:?}: expected no {WITHHELD_MEMBER} member on the not-found body, got {:?}",
        body.get(WITHHELD_MEMBER)
    );
    let mut present: Vec<&str> = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object body, got {body:?}"))
        .keys()
        .map(String::as_str)
        .collect();
    present.sort_unstable();
    let mut expected_members: Vec<&str> = NOT_FOUND_BODY_MEMBERS.to_vec();
    expected_members.sort_unstable();
    assert_eq!(
        present, expected_members,
        "case {case:?}: expected exactly the members {expected_members:?} on the not-found \
         body, got {present:?} ({body:?})"
    );
}

/// RULE 1: whichever rule-addressed operation reports it, and however the request
/// addressed the rule, a 404 for a rule this Region does not hold carries `Message` and
/// no `Reason`.
#[tokio::test]
async fn an_absent_rule_is_reported_with_message_and_no_reason() {
    let router = router();

    // A rule that exists, so no refusal below comes from an empty store, and its ARN as
    // the template every absent rule's ARN is built from.
    let (held, held_arn) = created_rule(
        &router,
        "a rule this Region holds",
        json!({ "ResourceType": "EBS_SNAPSHOT", "RetentionPeriod": retention(7) }),
    )
    .await;
    // One created and then deleted: the second way a rule can be absent.
    let (deleted, _) = created_rule(
        &router,
        "a rule that is then deleted",
        json!({ "ResourceType": "EBS_VOLUME", "RetentionPeriod": retention(6) }),
    )
    .await;
    let (status, _) = delete_rule(&router, &deleted).await;
    assert_eq!(status, 204, "setup: DeleteRule expected HTTP 204, got {status}");

    let rows = [
        ("never minted, all lowercase", "zzzzzzzzzzz".to_string()),
        ("never minted, all uppercase", "ZZZZZZZZZZZ".to_string()),
        ("never minted, mixed case", "0a1B2c3D4e5".to_string()),
        ("never minted, all digits", "99999999999".to_string()),
        ("created and then deleted", deleted.clone()),
    ];

    for (why, identifier) in &rows {
        // The same absent rule, addressed the way a tag operation addresses it.
        let arn = held_arn.replace(&held, identifier);
        for (operation, identifier_named, (status, body)) in
            rule_addressed_refusals(&router, identifier, &arn).await
        {
            let case = format!("{operation} on an identifier {why} ({identifier})");
            assert_not_found_body(&case, &identifier_named, status, &body);
        }
    }

    // Absence an ARN can express and an identifier cannot: a well-formed rule ARN whose
    // Region or account names no rule this Region and account hold. The rule the ARN
    // names is the held one, so its identifier is what the refusal states — no other
    // segment of the ARN reaches the message.
    let other_region = with_arn_segment(&held_arn, 3, "us-gov-east-1");
    let other_account = with_arn_segment(&held_arn, 4, "111122223333");
    for (why, absent_arn) in [
        ("another Region", other_region.as_str()),
        ("another account", other_account.as_str()),
    ] {
        for (operation, (status, body)) in [
            ("ListTagsForResource", list_tags(&router, absent_arn).await),
            (
                "TagResource",
                tag_resource(&router, absent_arn, &pairs(&[("team", "platform")])).await,
            ),
            (
                "UntagResource",
                untag_resource(&router, absent_arn, &keys(&["team"])).await,
            ),
        ] {
            let case = format!("{operation} on an ARN naming {why} ({absent_arn})");
            assert_not_found_body(&case, &held, status, &body);
        }
    }

    // The rule that DOES exist still reads back, so none of the refusals above touched it.
    let (status, fetched) = get_rule(&router, &held).await;
    assert_eq!(
        status, 200,
        "case \"the held rule after every refusal\": expected HTTP 200, got {status} / {fetched:?}"
    );
}

/// One row of the contrast table: the refusal a request answered with, and the status,
/// error code and reason it must report.
struct PopulatedReasonRow {
    case: &'static str,
    expected_status: u16,
    expected_code: &'static str,
    expected_reason: &'static str,
    status: u16,
    body: Value,
}

/// RULE 1's bound: the reason members this service DOES populate are still sent, so the
/// omission above is the one member the service leaves unset rather than every optional
/// member of an error body.
#[tokio::test]
async fn the_reason_members_this_service_does_populate_are_still_sent() {
    let router = router();

    // A tag-level rule, which cannot be locked: the conflict condition.
    let (tag_level, _) = created_rule(
        &router,
        "a tag-level rule",
        json!({
            "ResourceType": "EC2_IMAGE",
            "RetentionPeriod": retention(10),
            "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
        }),
    )
    .await;

    let over_quota: Vec<Value> = (0..=MAX_TAGS_PER_RULE)
        .map(|nth| json!({ "Key": format!("k{nth}"), "Value": format!("v{nth}") }))
        .collect();

    let mut rows: Vec<PopulatedReasonRow> = Vec::new();

    let (status, body) = lock_rule(
        &router,
        &tag_level,
        json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
    )
    .await;
    rows.push(PopulatedReasonRow {
        case: "LockRule on a rule whose own state forbids it",
        expected_status: 409,
        expected_code: "ConflictException",
        expected_reason: "INVALID_RULE_STATE",
        status,
        body,
    });

    let (status, body) =
        update_rule(&router, &tag_level, json!({ "Description": "tab\tseparated" })).await;
    rows.push(PopulatedReasonRow {
        case: "UpdateRule with a description outside its pattern",
        expected_status: 400,
        expected_code: "ValidationException",
        expected_reason: "INVALID_PARAMETER_VALUE",
        status,
        body,
    });

    let (status, body) = create_rule(
        &router,
        json!({
            "ResourceType": "EBS_VOLUME",
            "RetentionPeriod": retention(7),
            "Tags": over_quota,
        }),
    )
    .await;
    rows.push(PopulatedReasonRow {
        case: "CreateRule with one tag past the tags-per-rule quota",
        expected_status: 402,
        expected_code: "ServiceQuotaExceededException",
        expected_reason: "SERVICE_QUOTA_EXCEEDED",
        status,
        body,
    });

    for PopulatedReasonRow {
        case,
        expected_status,
        expected_code,
        expected_reason,
        status,
        body,
    } in &rows
    {
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} / {body:?}"
        );
        assert_eq!(
            &error_code(body),
            expected_code,
            "case {case:?}: expected the modelled {expected_code}, got {body:?}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some(*expected_reason),
            "case {case:?}: expected Reason {expected_reason:?}, got {:?}",
            body.get("Reason")
        );
    }
}
