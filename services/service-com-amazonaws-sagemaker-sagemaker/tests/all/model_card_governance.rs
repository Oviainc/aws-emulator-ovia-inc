// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's model-card governance resources:
//! `CreateModelCard` and the `DescribeModelCard` that reads it back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via
//! the `Emulator` harness `inference_deployment_lifecycle` owns, so what these tests
//! exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call. The rows
//! differ along the axes each rule generalises over: the name's shape, every member of
//! `ModelCardStatus`, the content's size and script (ASCII, CJK, emoji, RTL, combining
//! marks) up to the modelled 100000-character bound, the presence of the optional
//! members, the signed region, and name-vs-ARN for the reader.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Every member of `ModelCardStatus`
/// (smithy_query query="shape" shape="ModelCardStatus": `Draft | PendingReview |
/// Approved | Archived`). A card can be created in any of them.
pub(crate) const MODEL_CARD_STATUSES: [&str; 4] =
    ["Draft", "PendingReview", "Approved", "Archived"];

/// `ModelCardContent`'s modelled maximum length (smithy_query query="operation"
/// operation="CreateModelCard": `Content` is `len 0..100000`).
pub(crate) const CONTENT_MAX_LEN: usize = 100_000;

/// A `CreateModelCard` body naming `name`, in `status`, carrying `content`.
pub(crate) fn create_body(name: &str, status: &str, content: &str) -> Value {
    json!({
        "ModelCardName": name,
        "ModelCardStatus": status,
        "Content": content,
    })
}

/// A model card content document whose `model_description` is `description`.
pub(crate) fn content_with_description(description: &str) -> String {
    json!({
        "model_card_schema_version": "0.0.1",
        "model_overview": { "model_description": description },
    })
    .to_string()
}

/// `DescribeModelCard` for `name`, at `version` when one is given — "If a version is
/// not provided, then the latest version of the model card is described"
/// (smithy_query query="operation" operation="DescribeModelCard": `ModelCardVersion`).
pub(crate) fn describe_body(name: &str, version: Option<i32>) -> Value {
    match version {
        Some(version) => json!({ "ModelCardName": name, "ModelCardVersion": version }),
        None => json!({ "ModelCardName": name }),
    }
}

pub(crate) fn number_field(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric member {member} in {body}"))
}

/// RULE: `CreateModelCard` persists the card under its `ModelCardName` as version
/// [`state::FIRST_MODEL_CARD_VERSION`], and `DescribeModelCard` replays exactly what was
/// stored — the same `Content` byte for byte, the same `ModelCardStatus`, the same
/// optional `SecurityConfig` — under the `ModelCardArn` the create returned.
#[tokio::test]
async fn created_model_cards_round_trip_through_describe() {
    // The rows differ along what the rule generalises over: the name's shape (one
    // character, digits, hyphens, the modelled 63-character maximum), every status
    // member, the content's script and size, and whether the optional members are there.
    let names = [
        "c",
        "card1",
        "demo-model-card-with-hyphens",
        // `ModelCardName` is an `EntityName`, `len 1..63`.
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let contents: Vec<String> = vec![
        // An empty JSON object: the smallest document that is still model card content.
        "{}".to_string(),
        content_with_description("a plain ASCII description"),
        // Non-ASCII scripts, an emoji with a zero-width joiner, an RTL string and a
        // combining mark: `Content` is a string, so every one must survive untouched.
        content_with_description("日本語のモデル説明 — 모델 설명"),
        content_with_description("família 👩‍👩‍👧‍👦 café résumé"),
        content_with_description("الوصف النموذجي بالعربية"),
        content_with_description("e\u{0301}galite\u{0301} \u{0301}\u{0301}"),
        // A nested document with arrays, so structure as well as scalars round trips.
        json!({
            "model_overview": { "model_id": "m-1", "model_name": "n-1" },
            "evaluation_details": [
                { "name": "eval-1", "metric_groups": [{ "name": "g", "metric_data": [] }] },
            ],
            "additional_information": { "custom_details": { "k": [1, 2, 3], "b": true } },
        })
        .to_string(),
        // Up to the modelled bound: a document whose serialized length is exactly
        // `CONTENT_MAX_LEN`.
        {
            let envelope = content_with_description("").len();
            content_with_description(&"x".repeat(CONTENT_MAX_LEN - envelope))
        },
    ];
    let security_configs = [
        None,
        Some(json!({ "KmsKeyId": "alias/sagemaker-model-card" })),
        Some(json!({ "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab" })),
    ];
    // `Tags` is `len 0..50`; `DescribeModelCard` models no `Tags` member (they are read
    // through `ListTags`), so these rows assert only that each shape is accepted.
    let tag_sets = [
        None,
        Some(json!([])),
        Some(json!([{ "Key": "team", "Value": "ml" }])),
        Some(Value::Array(
            (0..50)
                .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
                .collect::<Vec<Value>>(),
        )),
    ];

    for (index, content) in contents.iter().enumerate() {
        let name = names[index % names.len()];
        let status = MODEL_CARD_STATUSES[index % MODEL_CARD_STATUSES.len()];
        let security_config = &security_configs[index % security_configs.len()];
        let tags = &tag_sets[index % tag_sets.len()];
        let case = format!(
            "name={name} status={status} content_len={} security_config={} tags={}",
            content.len(),
            security_config.is_some(),
            tags.is_some(),
        );

        let emulator = Emulator::new();
        let mut body = create_body(name, status, content);
        if let Some(config) = security_config {
            body["SecurityConfig"] = config.clone();
        }
        if let Some(tag_list) = tags {
            body["Tags"] = tag_list.clone();
        }

        let (status_code, created) = emulator.call("CreateModelCard", &body).await;
        assert_eq!(
            status_code, 200,
            "case {case}: expected CreateModelCard to succeed, got {status_code} with {created}"
        );
        let expected_arn = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/{name}");
        assert_eq!(
            str_field(&created, "ModelCardArn"),
            expected_arn,
            "case {case}: expected ModelCardArn {expected_arn}, got {created:?}"
        );

        let described = emulator
            .ok("DescribeModelCard", &json!({ "ModelCardName": name }))
            .await;
        assert_eq!(
            str_field(&described, "ModelCardArn"),
            expected_arn,
            "case {case}: expected the described ARN to be the created one, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelCardName"),
            name,
            "case {case}: expected ModelCardName {name}, got {described:?}"
        );
        assert_eq!(
            number_field(&described, "ModelCardVersion"),
            1.0,
            "case {case}: expected a new card to be version 1, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "Content"),
            *content,
            "case {case}: expected Content to round trip unchanged, got {:?}",
            described.get("Content")
        );
        assert_eq!(
            str_field(&described, "ModelCardStatus"),
            status,
            "case {case}: expected ModelCardStatus {status}, got {described:?}"
        );
        assert_eq!(
            described.get("SecurityConfig").cloned(),
            security_config.clone(),
            "case {case}: expected SecurityConfig to round trip, got {described:?}"
        );
        assert!(
            number_field(&described, "CreationTime") > 0.0,
            "case {case}: expected a CreationTime after the epoch, got {described:?}"
        );
        assert!(
            described.get("CreatedBy").is_some_and(Value::is_object),
            "case {case}: expected the required CreatedBy member, got {described:?}"
        );
        // `CreatedBy` names the IAM identity the create signed with, replayed from what
        // the create stored rather than rebuilt per read.
        let created_by = described
            .get("CreatedBy")
            .unwrap_or_else(|| panic!("case {case}: CreatedBy, got {described:?}"));
        assert_eq!(
            created_by
                .get("IamIdentity")
                .and_then(|identity| identity.get("Arn"))
                .and_then(Value::as_str),
            Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
            "case {case}: CreatedBy.IamIdentity.Arn, got {created_by:?}"
        );
        // Nothing has modified the card, so there is no identity to report for a
        // modification — and the member is how the card reports its last modifier, so it
        // is present as the EMPTY `UserContext` rather than omitted (the rule the
        // `read_responses_report_what_a_resource_holds` suite pins).
        assert_eq!(
            described.get("LastModifiedBy"),
            Some(&json!({})),
            "case {case}: expected an empty LastModifiedBy on an unmodified card, got \
             {described:?}"
        );
        assert!(
            number_field(&described, "LastModifiedTime") > 0.0,
            "case {case}: expected a LastModifiedTime after the epoch, got {described:?}"
        );
        // Nobody asked for this card to be deleted, so it is in none of
        // `ModelCardProcessingStatus`'s deletion states.
        assert_eq!(
            described.get("ModelCardProcessingStatus"),
            None,
            "case {case}: expected no ModelCardProcessingStatus, got {described:?}"
        );
    }
}

/// RULE: a model card's ARN is `arn:aws:sagemaker:<signed region>:<account>:model-card/
/// <name>` (the emulator's default region when the request names none), and the reader
/// accepts the card's name or that ARN — `DescribeModelCard`'s `ModelCardName` is a
/// `ModelCardNameOrArn`, so both forms resolve to the one stored card.
#[tokio::test]
async fn model_card_arns_carry_the_signed_region_and_name_the_card_the_reader_accepts() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        for name in ["c", "card-in-region"] {
            let case = format!("region={region} name={name}");
            let emulator = Emulator::new();
            let content = content_with_description("regional card");

            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateModelCard",
                    &create_body(name, "Draft", &content),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected CreateModelCard to succeed, got {status} with {created}"
            );
            let arn = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-card/{name}");
            assert_eq!(
                str_field(&created, "ModelCardArn"),
                arn,
                "case {case}: expected ModelCardArn {arn}, got {created:?}"
            );

            // Both forms of `ModelCardNameOrArn` name the same card.
            for reference in [name.to_string(), arn.clone()] {
                let (read_status, described) = emulator
                    .call_in(
                        region,
                        "DescribeModelCard",
                        &json!({ "ModelCardName": reference }),
                    )
                    .await;
                assert_eq!(
                    read_status, 200,
                    "case {case} reference={reference}: expected DescribeModelCard to \
                     succeed, got {read_status} with {described}"
                );
                assert_eq!(
                    str_field(&described, "Content"),
                    content,
                    "case {case} reference={reference}: expected the stored content, got \
                     {described:?}"
                );
            }
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call_unsigned(
            "CreateModelCard",
            &create_body("unsigned-card", "Draft", "{}"),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected CreateModelCard to succeed, got {status} with {created}"
    );
    assert_eq!(
        str_field(&created, "ModelCardArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/unsigned-card"),
        "case unsigned: expected the default-region ARN, got {created:?}"
    );
}

/// RULE: a `ModelCardName` already in use is refused with the modelled
/// `ConflictException` (HTTP 400) and the card already stored is left exactly as it was —
/// so of N callers racing on one name, exactly one creates it.
#[tokio::test]
async fn a_name_already_in_use_is_refused_with_conflict_and_the_stored_card_is_untouched() {
    // Rows differ along the first card's status and content and along the second
    // request's: the refusal is keyed to the name, not to what the second call carries.
    let cases = [
        ("card-a", "Draft", "{}", "Approved", "{}"),
        (
            "card-b",
            "Approved",
            r#"{"model_overview":{"model_id":"first"}}"#,
            "Archived",
            r#"{"model_overview":{"model_id":"second"}}"#,
        ),
        (
            "c",
            "PendingReview",
            r#"{"model_overview":{"model_name":"日本語"}}"#,
            "Draft",
            "{}",
        ),
    ];

    for (name, first_status, first_content, second_status, second_content) in cases {
        let case = format!("name={name} first={first_status} second={second_status}");
        let emulator = Emulator::new();

        emulator
            .ok(
                "CreateModelCard",
                &create_body(name, first_status, first_content),
            )
            .await;

        let (status, refused) = emulator
            .call(
                "CreateModelCard",
                &create_body(name, second_status, second_content),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a duplicate name, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case}: expected ConflictException, got {refused:?}"
        );
        assert!(
            message(&refused).contains(name),
            "case {case}: expected the refusal to name the card, got {:?}",
            message(&refused)
        );

        let described = emulator
            .ok("DescribeModelCard", &json!({ "ModelCardName": name }))
            .await;
        assert_eq!(
            str_field(&described, "Content"),
            first_content,
            "case {case}: expected the first card's content to survive, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ModelCardStatus"),
            first_status,
            "case {case}: expected the first card's status to survive, got {described:?}"
        );
        assert_eq!(
            number_field(&described, "ModelCardVersion"),
            1.0,
            "case {case}: expected the refused create to add no version, got {described:?}"
        );
    }
}

/// RULE: a reader that names a card, or a version of a card, that the store does not
/// hold is refused with the modelled `ResourceNotFound` (HTTP 400) — never a 200 with
/// empty members.
#[tokio::test]
async fn describing_a_card_or_version_that_is_not_stored_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateModelCard", &create_body("stored-card", "Draft", "{}"))
        .await;

    // Rows: an unknown name, an unknown name in ARN form, and versions either side of
    // the single version the stored card has.
    let cases: [(&str, Value); 5] = [
        ("unknown name", json!({ "ModelCardName": "no-such-card" })),
        (
            "unknown name as ARN",
            json!({
                "ModelCardName":
                    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-card/no-such-card"),
            }),
        ),
        (
            "version above the latest",
            json!({ "ModelCardName": "stored-card", "ModelCardVersion": 2 }),
        ),
        (
            "version zero",
            json!({ "ModelCardName": "stored-card", "ModelCardVersion": 0 }),
        ),
        (
            "negative version",
            json!({ "ModelCardName": "stored-card", "ModelCardVersion": -1 }),
        ),
    ];

    for (case, body) in cases {
        let (status, refused) = emulator.call("DescribeModelCard", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound, got {refused:?}"
        );
    }

    // The version the card does have is still readable: the refusals above changed
    // nothing.
    let described = emulator
        .ok(
            "DescribeModelCard",
            &json!({ "ModelCardName": "stored-card", "ModelCardVersion": 1 }),
        )
        .await;
    assert_eq!(
        number_field(&described, "ModelCardVersion"),
        1.0,
        "case stored version: expected version 1, got {described:?}"
    );
}

/// RULE: `IncludedData: MetadataOnly` answers with `Content` sanitized down to the five
/// documented metadata paths it actually carries; `AllData` and an absent `IncludedData`
/// answer with the content as stored, and asking for either changes nothing a later
/// reader sees.
#[tokio::test]
async fn metadata_only_keeps_exactly_the_documented_content_paths() {
    // The five paths `DescribeModelCardResponse$Content` documents as preserved, each
    // paired with the value this test stores under it.
    let full = json!({
        "model_card_schema_version": "0.0.1",
        "model_overview": {
            "model_id": "arn:aws:sagemaker:us-east-1:123456789012:model/m",
            "model_name": "m",
            "model_description": "secret-description",
            "problem_type": "classification",
        },
        "intended_uses": {
            "risk_rating": "Low",
            "intended_uses": "secret-intended-uses",
        },
        "model_package_details": {
            "model_package_group_name": "group-1",
            "model_package_arn": "arn:aws:sagemaker:us-east-1:123456789012:model-package/p/1",
            "model_package_description": "secret-package-description",
        },
        "business_details": { "business_problem": "secret-business-problem" },
        "training_details": { "objective_function": { "function": { "function": "Maximize" } } },
    });
    // Rows differ along how much of the preserved set the document carries: all of it,
    // some of it (a section present but the preserved field absent), and none of it.
    let cases: [(&str, Value, Value); 4] = [
        (
            "every preserved path present",
            full.clone(),
            json!({
                "model_overview": {
                    "model_id": "arn:aws:sagemaker:us-east-1:123456789012:model/m",
                    "model_name": "m",
                },
                "intended_uses": { "risk_rating": "Low" },
                "model_package_details": {
                    "model_package_group_name": "group-1",
                    "model_package_arn":
                        "arn:aws:sagemaker:us-east-1:123456789012:model-package/p/1",
                },
            }),
        ),
        (
            "one preserved field, inside a section that also holds removed ones",
            json!({
                "model_overview": { "model_name": "only-name", "model_description": "secret" },
                "business_details": { "business_problem": "secret" },
            }),
            json!({ "model_overview": { "model_name": "only-name" } }),
        ),
        (
            "a preserved section present but none of its preserved fields",
            json!({ "model_overview": { "model_description": "secret" } }),
            json!({}),
        ),
        ("no preserved path at all", json!({}), json!({})),
    ];

    for (case, stored, expected_metadata) in cases {
        let emulator = Emulator::new();
        let content = stored.to_string();
        emulator
            .ok(
                "CreateModelCard",
                &create_body("sanitized-card", "Draft", &content),
            )
            .await;

        let metadata_only = emulator
            .ok(
                "DescribeModelCard",
                &json!({ "ModelCardName": "sanitized-card", "IncludedData": "MetadataOnly" }),
            )
            .await;
        let sanitized: Value = serde_json::from_str(&str_field(&metadata_only, "Content"))
            .unwrap_or_else(|error| {
                panic!("case {case}: sanitized Content must still be JSON: {error}")
            });
        assert_eq!(
            sanitized, expected_metadata,
            "case {case}: expected the sanitized Content to be {expected_metadata}, got \
             {sanitized}"
        );

        // `AllData` is also the documented default, so both spellings answer with the
        // content as stored — and they do so AFTER the sanitized read, which therefore
        // changed nothing.
        for included_data in [Some("AllData"), None] {
            let mut body = json!({ "ModelCardName": "sanitized-card" });
            if let Some(value) = included_data {
                body["IncludedData"] = json!(value);
            }
            let described = emulator.ok("DescribeModelCard", &body).await;
            assert_eq!(
                str_field(&described, "Content"),
                content,
                "case {case} IncludedData={included_data:?}: expected the stored content, \
                 got {described:?}"
            );
        }
    }
}

/// RULE: `Content` "must be in model card JSON schema and provided as a string", so a
/// value that is not a JSON object is refused with HTTP 400 and nothing is persisted.
#[tokio::test]
async fn content_that_is_not_a_json_object_is_refused_and_persists_nothing() {
    // Rows span the JSON value kinds that are not objects, plus two strings that are not
    // JSON at all.
    let cases = [
        ("empty string", ""),
        ("plain text", "not json at all"),
        ("JSON array", r#"[{"model_overview":{}}]"#),
        ("JSON string", r#""{\"model_overview\":{}}""#),
        ("JSON number", "42"),
        ("JSON boolean", "true"),
        ("JSON null", "null"),
        ("truncated object", r#"{"model_overview":"#),
    ];

    for (case, content) in cases {
        let emulator = Emulator::new();
        let (status, refused) = emulator
            .call(
                "CreateModelCard",
                &create_body("rejected-card", "Draft", content),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
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

        // Nothing was persisted: the reader still refuses the name.
        let (read_status, missing) = emulator
            .call(
                "DescribeModelCard",
                &json!({ "ModelCardName": "rejected-card" }),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: expected the refused create to persist nothing, got \
             {read_status} with {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFound",
            "case {case}: expected ResourceNotFound after a refused create, got {missing:?}"
        );
    }
}
