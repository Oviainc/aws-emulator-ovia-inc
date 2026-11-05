// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `UpdateModelCard`, read back through
//! `DescribeModelCard`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via
//! the `Emulator` harness `inference_deployment_lifecycle` owns, so what these tests
//! exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call. The rows
//! differ along the axes each rule generalises over: every `ModelCardStatus` a card can
//! be created in paired with every status it can be moved to, the content's size and
//! script, the number of successive updates, whether the optional `SecurityConfig` is
//! there, the signed region, and name-vs-ARN for the member that names the card.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::model_card_governance::{
    content_with_description, create_body, describe_body, number_field, CONTENT_MAX_LEN,
    MODEL_CARD_STATUSES,
};

/// An `UpdateModelCard` body moving `name` to `status`.
fn status_update(name: &str, status: &str) -> Value {
    json!({ "ModelCardName": name, "ModelCardStatus": status })
}

/// An `UpdateModelCard` body replacing `name`'s content with `content`.
fn content_update(name: &str, content: &str) -> Value {
    json!({ "ModelCardName": name, "Content": content })
}

/// RULE: an `UpdateModelCard` that carries a `ModelCardStatus` moves the card's LATEST
/// version to that approval status in place — it adds no version — records the call on
/// that version (`LastModifiedTime`, `LastModifiedBy`), leaves the content exactly as it
/// was, and answers with the card's stored ARN.
#[tokio::test]
async fn a_status_update_moves_the_latest_version_in_place_and_adds_no_version() {
    // Rows: every status a card can be created in, paired with every status
    // `UpdateModelCard` can move it to — including the self-transition.
    for created_in in MODEL_CARD_STATUSES {
        for moved_to in MODEL_CARD_STATUSES {
            let case = format!("created_in={created_in} moved_to={moved_to}");
            let emulator = Emulator::new();
            let name = "governed-card";
            let content = content_with_description("content the status update must not touch");
            emulator
                .ok("CreateModelCard", &create_body(name, created_in, &content))
                .await;
            let created = emulator.ok("DescribeModelCard", &describe_body(name, None)).await;
            let creation_time = number_field(&created, "CreationTime");

            let (status_code, updated) = emulator.call("UpdateModelCard", &status_update(name, moved_to)).await;
            assert_eq!(
                status_code, 200,
                "case {case}: expected UpdateModelCard to succeed, got {status_code} with {updated}"
            );
            let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/{name}");
            assert_eq!(
                str_field(&updated, "ModelCardArn"),
                arn,
                "case {case}: expected the stored ModelCardArn {arn}, got {updated:?}"
            );

            let described = emulator.ok("DescribeModelCard", &describe_body(name, None)).await;
            assert_eq!(
                str_field(&described, "ModelCardStatus"),
                moved_to,
                "case {case}: expected the status to be {moved_to}, got {described:?}"
            );
            assert_eq!(
                number_field(&described, "ModelCardVersion"),
                1.0,
                "case {case}: expected a status update to add no version, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "Content"),
                content,
                "case {case}: expected the content to survive a status update, got {described:?}"
            );
            assert_eq!(
                number_field(&described, "CreationTime"),
                creation_time,
                "case {case}: expected the creation time to be untouched, got {described:?}"
            );
            assert!(
                number_field(&described, "LastModifiedTime") >= creation_time,
                "case {case}: expected LastModifiedTime at or after CreationTime, got \
                 {described:?}"
            );
            // The update was modified by an identity, so one is reported — the IAM
            // identity the updating request signed with.
            assert_eq!(
                described
                    .get("LastModifiedBy")
                    .and_then(|identity| identity.get("IamIdentity"))
                    .and_then(|identity| identity.get("Arn"))
                    .and_then(Value::as_str),
                Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
                "case {case}: expected LastModifiedBy to name the updater, got {described:?}"
            );

            // The card has exactly one version: the one past it is not stored.
            let (missing_status, missing) = emulator
                .call("DescribeModelCard", &describe_body(name, Some(2)))
                .await;
            assert_eq!(
                missing_status, 400,
                "case {case}: expected no version 2 after a status update, got \
                 {missing_status} with {missing}"
            );
            assert_eq!(
                error_code(&missing),
                "ResourceNotFound",
                "case {case}: expected ResourceNotFound for version 2, got {missing:?}"
            );
        }
    }
}

/// RULE: an `UpdateModelCard` that carries `Content` APPENDS the next version of the
/// card — the versions before it stay readable with the content they were recorded with
/// — and the new version carries forward the status and `SecurityConfig` the previous
/// one held. A reader that names no version reads the newest.
#[tokio::test]
async fn a_content_update_appends_the_next_version_and_leaves_the_history_readable() {
    // Rows differ along what the rule generalises over: the content's size and script,
    // how many successive updates are applied, the status the card is in, whether the
    // optional `SecurityConfig` is there, and whether the update names the card by name
    // or by ARN.
    let updates: Vec<Vec<String>> = vec![
        // One update, the smallest document that is still model card content.
        vec!["{}".to_string()],
        // Three successive updates: versions 2, 3 and 4.
        vec![
            content_with_description("second revision"),
            content_with_description("third revision"),
            content_with_description("fourth revision"),
        ],
        // Non-ASCII scripts, an emoji with a zero-width joiner and a combining mark: the
        // appended content must survive byte for byte.
        vec![
            content_with_description("日本語のモデル説明 — 모델 설명"),
            content_with_description("família 👩‍👩‍👧‍👦 café"),
            content_with_description("e\u{0301}galite\u{0301}"),
        ],
        // A nested document, so structure as well as scalars round trips.
        vec![json!({
            "model_overview": { "model_id": "m-2", "model_name": "n-2" },
            "evaluation_details": [{ "name": "eval-2", "metric_groups": [] }],
        })
        .to_string()],
        // Up to the modelled bound: a document whose serialized length is exactly
        // `ModelCardContent`'s maximum.
        vec![{
            let envelope = content_with_description("").len();
            content_with_description(&"y".repeat(CONTENT_MAX_LEN - envelope))
        }],
    ];
    let security_configs = [
        None,
        Some(json!({ "KmsKeyId": "alias/sagemaker-model-card" })),
        Some(json!({ "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab" })),
    ];

    for (index, revisions) in updates.iter().enumerate() {
        let status = MODEL_CARD_STATUSES[index % MODEL_CARD_STATUSES.len()];
        let security_config = &security_configs[index % security_configs.len()];
        let by_arn = index % 2 == 0;
        let name = "versioned-card";
        let case = format!(
            "revisions={} status={status} security_config={} by_arn={by_arn}",
            revisions.len(),
            security_config.is_some(),
        );

        let emulator = Emulator::new();
        let first_content = content_with_description("the content the create recorded");
        let mut body = create_body(name, status, &first_content);
        if let Some(config) = security_config {
            body["SecurityConfig"] = config.clone();
        }
        emulator.ok("CreateModelCard", &body).await;

        let arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/{name}");
        let reference = if by_arn { arn.clone() } else { name.to_string() };
        // Every version the card should hold, in order: the create's, then one per
        // update.
        let mut expected: Vec<String> = vec![first_content];
        for revision in revisions {
            let updated = emulator
                .ok("UpdateModelCard", &content_update(&reference, revision))
                .await;
            assert_eq!(
                str_field(&updated, "ModelCardArn"),
                arn,
                "case {case}: expected the stored ModelCardArn {arn}, got {updated:?}"
            );
            expected.push(revision.clone());
        }

        for (offset, content) in expected.iter().enumerate() {
            let version = i32::try_from(offset).expect("a small version number") + 1;
            let described = emulator
                .ok("DescribeModelCard", &describe_body(&reference, Some(version)))
                .await;
            assert_eq!(
                number_field(&described, "ModelCardVersion"),
                f64::from(version),
                "case {case}: expected version {version}, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "Content"),
                *content,
                "case {case} version={version}: expected the content recorded for that \
                 version, got {:?}",
                described.get("Content")
            );
            // The status and the security configuration belong to the card's review
            // state, so every appended version carries the ones in force.
            assert_eq!(
                str_field(&described, "ModelCardStatus"),
                status,
                "case {case} version={version}: expected the status to carry forward, got \
                 {described:?}"
            );
            assert_eq!(
                described.get("SecurityConfig").cloned(),
                security_config.clone(),
                "case {case} version={version}: expected SecurityConfig to carry forward, \
                 got {described:?}"
            );
        }

        // A reader that names no version reads the newest one.
        let latest = emulator
            .ok("DescribeModelCard", &describe_body(&reference, None))
            .await;
        let last_version = i32::try_from(expected.len()).expect("a small version count");
        assert_eq!(
            number_field(&latest, "ModelCardVersion"),
            f64::from(last_version),
            "case {case}: expected the latest version to be {last_version}, got {latest:?}"
        );
        assert_eq!(
            str_field(&latest, "Content"),
            *expected.last().expect("at least the created version"),
            "case {case}: expected the newest content, got {:?}",
            latest.get("Content")
        );

        // And one past the newest is not stored: the updates appended exactly one
        // version each.
        let (missing_status, missing) = emulator
            .call(
                "DescribeModelCard",
                &describe_body(&reference, Some(last_version + 1)),
            )
            .await;
        assert_eq!(
            missing_status, 400,
            "case {case}: expected no version {}, got {missing_status} with {missing}",
            last_version + 1
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound past the newest version, got {missing:?}"
        );
    }
}

/// RULE: the two aspects are updated independently — a status update applies to whatever
/// version is latest at the time, and the versions appended before it keep the status
/// they were recorded with, so the card's history records the review state each revision
/// was in.
#[tokio::test]
async fn a_status_update_moves_only_the_version_that_is_latest_when_it_runs() {
    // Rows: each status the card is moved to between two content revisions.
    for moved_to in MODEL_CARD_STATUSES {
        let case = format!("moved_to={moved_to}");
        let emulator = Emulator::new();
        let name = "reviewed-card";
        emulator
            .ok(
                "CreateModelCard",
                &create_body(name, "Draft", &content_with_description("v1")),
            )
            .await;
        // Version 1 is moved to `moved_to`, then a content update appends version 2,
        // which inherits it, and a final status update moves version 2 back to Draft.
        emulator
            .ok("UpdateModelCard", &status_update(name, moved_to))
            .await;
        emulator
            .ok(
                "UpdateModelCard",
                &content_update(name, &content_with_description("v2")),
            )
            .await;
        emulator
            .ok("UpdateModelCard", &status_update(name, "Draft"))
            .await;

        let first = emulator
            .ok("DescribeModelCard", &describe_body(name, Some(1)))
            .await;
        assert_eq!(
            str_field(&first, "ModelCardStatus"),
            moved_to,
            "case {case}: expected version 1 to keep the status it was moved to, got {first:?}"
        );
        assert_eq!(
            str_field(&first, "Content"),
            content_with_description("v1"),
            "case {case}: expected version 1's content to be untouched, got {first:?}"
        );

        let second = emulator
            .ok("DescribeModelCard", &describe_body(name, Some(2)))
            .await;
        assert_eq!(
            str_field(&second, "ModelCardStatus"),
            "Draft",
            "case {case}: expected the last status update to move version 2, got {second:?}"
        );
        assert_eq!(
            str_field(&second, "Content"),
            content_with_description("v2"),
            "case {case}: expected version 2's content, got {second:?}"
        );
    }
}

/// RULE: `UpdateModelCard` accepts EXACTLY one of `Content` and `ModelCardStatus` — "you
/// cannot update both model card content and model card status in a single call", and a
/// call that carries neither asks for no change — and both refusals are HTTP 400
/// `ValidationException` that persist nothing.
#[tokio::test]
async fn naming_both_aspects_or_neither_is_refused_and_persists_nothing() {
    // Rows: both members together, over every status and several content shapes, and
    // then the body that carries only the card's name.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for status in MODEL_CARD_STATUSES {
        for content in ["{}", r#"{"model_overview":{"model_id":"m"}}"#] {
            cases.push((
                format!("both status={status} content_len={}", content.len()),
                json!({
                    "ModelCardName": "exclusive-card",
                    "ModelCardStatus": status,
                    "Content": content,
                }),
            ));
        }
    }
    cases.push((
        "neither member".to_string(),
        json!({ "ModelCardName": "exclusive-card" }),
    ));

    let stored = content_with_description("the only content this card ever had");
    for (case, body) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateModelCard",
                &create_body("exclusive-card", "Draft", &stored),
            )
            .await;

        let (status_code, refused) = emulator.call("UpdateModelCard", &body).await;
        assert_eq!(
            status_code, 400,
            "case {case}: expected HTTP 400, got {status_code} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused:?}"
        );
        assert!(
            message(&refused).contains("Content") && message(&refused).contains("ModelCardStatus"),
            "case {case}: expected the refusal to name both members, got {:?}",
            message(&refused)
        );

        // Nothing moved: the card is still version 1, Draft, with the stored content.
        let described = emulator
            .ok("DescribeModelCard", &describe_body("exclusive-card", None))
            .await;
        assert_eq!(
            number_field(&described, "ModelCardVersion"),
            1.0,
            "case {case}: expected the refused update to add no version, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelCardStatus"),
            "Draft",
            "case {case}: expected the status to be untouched, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "Content"),
            stored,
            "case {case}: expected the content to be untouched, got {described:?}"
        );
        assert_eq!(
            described.get("LastModifiedBy"),
            Some(&json!({})),
            "case {case}: expected no modifier to be recorded for a refused update, so the \
             member stays the empty UserContext, got {described:?}"
        );
    }
}

/// RULE: an update that names a card the store does not hold is refused with the modelled
/// `ResourceNotFound` (HTTP 400) — in either form of `ModelCardNameOrArn` — and creates
/// nothing.
#[tokio::test]
async fn updating_a_card_that_is_not_stored_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateModelCard", &create_body("stored-card", "Draft", "{}"))
        .await;

    let unknown_arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/no-such-card");
    // Rows: each aspect, by name and by ARN.
    let cases: [(&str, Value); 4] = [
        ("status by name", status_update("no-such-card", "Approved")),
        ("content by name", content_update("no-such-card", "{}")),
        ("status by ARN", status_update(&unknown_arn, "Archived")),
        (
            "content by ARN",
            content_update(&unknown_arn, r#"{"model_overview":{}}"#),
        ),
    ];

    for (case, body) in cases {
        let (status_code, refused) = emulator.call("UpdateModelCard", &body).await;
        assert_eq!(
            status_code, 400,
            "case {case}: expected HTTP 400, got {status_code} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound, got {refused:?}"
        );
        assert!(
            message(&refused).contains("no-such-card"),
            "case {case}: expected the refusal to name the card, got {:?}",
            message(&refused)
        );

        // The refused update created nothing under that name.
        let (read_status, missing) = emulator
            .call("DescribeModelCard", &describe_body("no-such-card", None))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: expected the refused update to persist nothing, got \
             {read_status} with {missing}"
        );
    }

    // The card that does exist is untouched by every refusal above.
    let described = emulator
        .ok("DescribeModelCard", &describe_body("stored-card", None))
        .await;
    assert_eq!(
        number_field(&described, "ModelCardVersion"),
        1.0,
        "case stored card: expected version 1, got {described:?}"
    );
}

/// RULE: `Content` "must be in model card JSON schema and provided as a string", so an
/// update carrying a value that is not a JSON object is refused with HTTP 400
/// `ValidationException` and appends no version.
#[tokio::test]
async fn content_that_is_not_a_json_object_is_refused_and_appends_no_version() {
    // Rows span the JSON value kinds that are not objects, plus strings that are not JSON
    // at all.
    let cases = [
        ("empty string", ""),
        ("plain text", "not json at all"),
        ("JSON array", r#"[{"model_overview":{}}]"#),
        ("JSON string", r#""{\"model_overview\":{}}""#),
        ("JSON number", "42"),
        ("JSON boolean", "false"),
        ("JSON null", "null"),
        ("truncated object", r#"{"model_overview":"#),
    ];

    let stored = content_with_description("the content a refused update must not replace");
    for (case, content) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateModelCard",
                &create_body("checked-card", "Draft", &stored),
            )
            .await;

        let (status_code, refused) = emulator
            .call("UpdateModelCard", &content_update("checked-card", content))
            .await;
        assert_eq!(
            status_code, 400,
            "case {case}: expected HTTP 400, got {status_code} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused:?}"
        );
        assert!(
            message(&refused).contains("Content"),
            "case {case}: expected the refusal to name the member, got {:?}",
            message(&refused)
        );

        let described = emulator
            .ok("DescribeModelCard", &describe_body("checked-card", None))
            .await;
        assert_eq!(
            number_field(&described, "ModelCardVersion"),
            1.0,
            "case {case}: expected the refused update to append no version, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "Content"),
            stored,
            "case {case}: expected the stored content to survive, got {described:?}"
        );
    }
}

/// RULE: a card's ARN belongs to the card, so an update answers with the ARN the create
/// minted for the region it signed for — whatever region the update itself signs for.
#[tokio::test]
async fn an_update_answers_with_the_cards_stored_arn() {
    for created_in in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        for updated_in in ["us-east-1", "eu-west-1"] {
            let case = format!("created_in={created_in} updated_in={updated_in}");
            let emulator = Emulator::new();
            let name = "regional-card";
            let (create_status, created) = emulator
                .call_in(
                    created_in,
                    "CreateModelCard",
                    &create_body(name, "Draft", "{}"),
                )
                .await;
            assert_eq!(
                create_status, 200,
                "case {case}: expected CreateModelCard to succeed, got {create_status} \
                 with {created}"
            );
            let arn = format!("arn:aws:sagemaker:{created_in}:{ACCOUNT}:model-card/{name}");

            let (update_status, updated) = emulator
                .call_in(updated_in, "UpdateModelCard", &status_update(name, "Approved"))
                .await;
            assert_eq!(
                update_status, 200,
                "case {case}: expected UpdateModelCard to succeed, got {update_status} \
                 with {updated}"
            );
            assert_eq!(
                str_field(&updated, "ModelCardArn"),
                arn,
                "case {case}: expected the create's ARN {arn}, got {updated:?}"
            );
        }
    }

    // An unsigned update reaches the same card and reports the same ARN.
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModelCard",
            &create_body("unsigned-updated-card", "Draft", "{}"),
        )
        .await;
    let (status_code, updated) = emulator
        .call_unsigned(
            "UpdateModelCard",
            &content_update("unsigned-updated-card", r#"{"model_overview":{}}"#),
        )
        .await;
    assert_eq!(
        status_code, 200,
        "case unsigned: expected UpdateModelCard to succeed, got {status_code} with {updated}"
    );
    assert_eq!(
        str_field(&updated, "ModelCardArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/unsigned-updated-card"),
        "case unsigned: expected the stored ARN, got {updated:?}"
    );
    let described = emulator
        .ok(
            "DescribeModelCard",
            &describe_body("unsigned-updated-card", None),
        )
        .await;
    assert_eq!(
        number_field(&described, "ModelCardVersion"),
        2.0,
        "case unsigned: expected the content update to append version 2, got {described:?}"
    );
}
