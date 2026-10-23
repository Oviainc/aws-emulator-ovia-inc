// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `DeleteModelCard`, checked against the readers that see
//! the card before and after it (`DescribeModelCard`) and against the `CreateModelCard`
//! that may reuse the freed name.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via
//! the `Emulator` harness `inference_deployment_lifecycle` owns, so what these tests
//! exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call. The rows
//! differ along the axes each rule generalises over: the name's shape and length, every
//! member of `ModelCardStatus`, the content's size and script, how many versions the
//! card has gained before it is deleted, the signed region, how many cards the store
//! holds, and which identifier the refusal is asked about.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::model_card_governance::{
    content_with_description, create_body, describe_body, number_field, CONTENT_MAX_LEN,
    MODEL_CARD_STATUSES,
};

/// `DeleteModelCard`'s only input member (smithy_query query="operation"
/// operation="DeleteModelCard": `ModelCardName` is an `EntityName`, `len 1..63`, and the
/// operation names no version).
fn delete_body(name: &str) -> Value {
    json!({ "ModelCardName": name })
}

/// Whether `body` reports no members at all — `DeleteModelCard`'s output is the modelled
/// `Unit`, so a success carries either an empty document or no body.
fn is_empty_output(body: &Value) -> bool {
    body.as_object().is_none_or(serde_json::Map::is_empty)
}

/// RULE: `DeleteModelCard` releases the model card its `ModelCardName` names, with every
/// version of it — the operation names no version — so each reader of that card or of any
/// version it held is afterwards refused with the modelled `ResourceNotFound`, the name is
/// free for a later create, and the delete itself answers an empty (`Unit`) output.
#[tokio::test]
async fn deleting_a_model_card_releases_every_version_and_frees_the_name() {
    // The rows differ along what the rule generalises over: the name's shape (one
    // character, digits, hyphens, the modelled 63-character maximum), every status
    // member, the content's script and size up to the modelled bound, and how many
    // versions the card has gained before the delete.
    let long_name = "a234567890123456789012345678901234567890123456789012345678901b";
    // A document whose serialized length is exactly `ModelCardContent`'s modelled
    // maximum, so the delete is exercised on the largest card that can be stored.
    let max_content = {
        let envelope = content_with_description("").len();
        content_with_description(&"x".repeat(CONTENT_MAX_LEN - envelope))
    };
    let rows: Vec<(&str, String, usize)> = vec![
        ("c", "{}".to_string(), 0),
        (
            "card1",
            r#"{"model_overview":{"model_id":"m-1"}}"#.to_string(),
            1,
        ),
        ("demo-model-card-with-hyphens", "{}".to_string(), 3),
        (
            long_name,
            r#"{"model_overview":{"model_name":"日本語"}}"#.to_string(),
            0,
        ),
        (
            "emoji-card",
            r#"{"business_details":{"business_problem":"👩‍👩‍👧‍👦"}}"#.to_string(),
            2,
        ),
        (
            "rtl-card",
            r#"{"model_overview":{"model_description":"الوصف"}}"#.to_string(),
            1,
        ),
        ("largest-card", max_content, 1),
    ];

    for (index, (name, content, extra_versions)) in rows.iter().enumerate() {
        let status = MODEL_CARD_STATUSES[index % MODEL_CARD_STATUSES.len()];
        let case = format!(
            "name={name} status={status} content_len={} extra_versions={extra_versions}",
            content.len()
        );
        let emulator = Emulator::new();

        emulator
            .ok("CreateModelCard", &create_body(name, status, content))
            .await;
        // Each content update appends the next version, so the card has
        // `1 + extra_versions` of them when the delete lands.
        for version in 0..*extra_versions {
            emulator
                .ok(
                    "UpdateModelCard",
                    &json!({
                        "ModelCardName": name,
                        "Content": content_with_description(&format!("revision {version}")),
                    }),
                )
                .await;
        }
        let held = u64::try_from(*extra_versions).expect("a small row count") + 1;
        let described = emulator.ok("DescribeModelCard", &describe_body(name, None)).await;
        assert_eq!(
            number_field(&described, "ModelCardVersion"),
            held as f64,
            "case {case}: expected the card to hold {held} version(s) before the delete, \
             got {described:?}"
        );

        let (delete_status, deleted) = emulator.call("DeleteModelCard", &delete_body(name)).await;
        assert_eq!(
            delete_status, 200,
            "case {case}: expected DeleteModelCard to succeed, got {delete_status} with \
             {deleted}"
        );
        assert!(
            is_empty_output(&deleted),
            "case {case}: expected an empty DeleteModelCard output, got {deleted}"
        );

        // The card is gone, and so is every version it held: the operation names no
        // version, so the delete releases the card itself.
        for version in std::iter::once(None).chain(
            (1..=i32::try_from(held).expect("a small version count")).map(Some),
        ) {
            let (read_status, missing) = emulator
                .call("DescribeModelCard", &describe_body(name, version))
                .await;
            assert_eq!(
                read_status, 400,
                "case {case} version={version:?}: expected HTTP 400 after the delete, got \
                 {read_status} with {missing}"
            );
            assert_eq!(
                error_code(&missing),
                "ResourceNotFound",
                "case {case} version={version:?}: expected ResourceNotFound, got {missing:?}"
            );
        }

        // The name is free again: a create under it is accepted and starts a fresh
        // version history rather than resuming the deleted card's.
        let replacement = content_with_description("the card created after the delete");
        let (recreate_status, recreated) = emulator
            .call(
                "CreateModelCard",
                &create_body(name, "Draft", &replacement),
            )
            .await;
        assert_eq!(
            recreate_status, 200,
            "case {case}: expected the freed name to be creatable, got {recreate_status} \
             with {recreated}"
        );
        assert_eq!(
            str_field(&recreated, "ModelCardArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/{name}"),
            "case {case}: expected the card's ARN, got {recreated:?}"
        );
        let after = emulator.ok("DescribeModelCard", &describe_body(name, None)).await;
        assert_eq!(
            number_field(&after, "ModelCardVersion"),
            1.0,
            "case {case}: expected the recreated card to start at version 1, got {after:?}"
        );
        assert_eq!(
            str_field(&after, "Content"),
            replacement,
            "case {case}: expected the recreated card's own content, got {after:?}"
        );
    }
}

/// RULE: a `DeleteModelCard` naming a card the store does not hold is refused with the
/// modelled `ResourceNotFound` (HTTP 400) and quotes the identifier the caller passed —
/// never a 200 for a delete that removed nothing — and the refusal releases nothing, so
/// the cards the store does hold survive it.
#[tokio::test]
async fn deleting_a_card_that_is_not_stored_is_refused_and_releases_nothing() {
    let emulator = Emulator::new();
    let content = content_with_description("the card the refusals must not touch");
    emulator
        .ok("CreateModelCard", &create_body("stored-card", "Approved", &content))
        .await;

    // Rows: a name never created, a name differing from the stored one only in case
    // (`EntityName` is case-sensitive), the shortest and the longest name the model
    // allows, and the stored card's name AFTER it has been deleted — a second delete of
    // one card is the same refusal. The third field says whether the row creates and
    // deletes its card first, so the control flow does not depend on the row's label.
    let cases: [(&str, String, bool); 5] = [
        ("unknown name", "no-such-card".to_string(), false),
        (
            "name differing only in case",
            "Stored-Card".to_string(),
            false,
        ),
        ("shortest allowed name", "z".to_string(), false),
        (
            "longest allowed name",
            "z234567890123456789012345678901234567890123456789012345678901y".to_string(),
            false,
        ),
        (
            "a second delete of one card",
            "deleted-twice".to_string(),
            true,
        ),
    ];

    for (case, name, delete_it_first) in cases {
        if delete_it_first {
            emulator
                .ok("CreateModelCard", &create_body(&name, "Draft", "{}"))
                .await;
            let (first, body) = emulator.call("DeleteModelCard", &delete_body(&name)).await;
            assert_eq!(
                first, 200,
                "case {case}: expected the first delete to succeed, got {first} with {body}"
            );
        }

        let (status, refused) = emulator.call("DeleteModelCard", &delete_body(&name)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound, got {refused:?}"
        );
        assert!(
            message(&refused).contains(&name),
            "case {case}: expected the refusal to name {name}, got {:?}",
            message(&refused)
        );

        // The refusal released nothing: the stored card reads back exactly as created.
        let described = emulator
            .ok("DescribeModelCard", &describe_body("stored-card", None))
            .await;
        assert_eq!(
            str_field(&described, "Content"),
            content,
            "case {case}: expected the stored card to survive the refusal, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelCardStatus"),
            "Approved",
            "case {case}: expected the stored card's status to survive, got {described:?}"
        );
    }
}

/// RULE: a delete releases ONLY the card its `ModelCardName` names: every other card the
/// store holds — including one created under another signed region, since the store keys
/// a card by its name alone — is still readable afterwards, with its own content, status
/// and ARN.
#[tokio::test]
async fn a_delete_releases_only_the_card_it_names() {
    // Rows: each card in the set is, in turn, the one deleted, and the cards differ along
    // the signed region they were created in — so the surviving cards include cards from
    // the deleted one's region and from other regions.
    let cards = [
        ("alpha", "us-east-1", "Draft"),
        ("beta-card", "eu-west-1", "Archived"),
        ("g", "ap-southeast-2", "PendingReview"),
        ("delta-model-card", "us-east-1", "Approved"),
    ];

    for (target, target_region, _) in cards {
        let case = format!("target={target} region={target_region}");
        let emulator = Emulator::new();
        for (name, region, status) in cards {
            let (created_status, created) = emulator
                .call_in(
                    region,
                    "CreateModelCard",
                    &create_body(name, status, &content_with_description(name)),
                )
                .await;
            assert_eq!(
                created_status, 200,
                "case {case}: expected CreateModelCard for {name} to succeed, got \
                 {created_status} with {created}"
            );
        }

        let (status, deleted) = emulator
            .call_in(target_region, "DeleteModelCard", &delete_body(target))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected DeleteModelCard to succeed, got {status} with {deleted}"
        );

        for (name, region, card_status) in cards {
            let (read_status, body) = emulator
                .call_in(region, "DescribeModelCard", &describe_body(name, None))
                .await;
            if name == target {
                assert_eq!(
                    read_status, 400,
                    "case {case}: expected the deleted card to be gone, got {read_status} \
                     with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFound",
                    "case {case}: expected ResourceNotFound for the deleted card, got {body:?}"
                );
                continue;
            }
            assert_eq!(
                read_status, 200,
                "case {case}: expected {name} to survive the delete, got {read_status} with \
                 {body}"
            );
            assert_eq!(
                str_field(&body, "Content"),
                content_with_description(name),
                "case {case}: expected {name}'s own content, got {body:?}"
            );
            assert_eq!(
                str_field(&body, "ModelCardStatus"),
                card_status,
                "case {case}: expected {name}'s own status, got {body:?}"
            );
            assert_eq!(
                str_field(&body, "ModelCardArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-card/{name}"),
                "case {case}: expected {name}'s own ARN, got {body:?}"
            );
        }
    }
}
