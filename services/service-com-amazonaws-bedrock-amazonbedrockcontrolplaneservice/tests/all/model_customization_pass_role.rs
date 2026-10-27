// SPDX-License-Identifier: CC0-1.0

//! The `iam:PassRole` decision `CreateModelCustomizationJob` is settled by, end to end over
//! the same `MultiServiceRouter` the emulator boots (`crate::support`).
//!
//! The rules under test, each asserted over a SET of requests rather than over one:
//!
//! 1. **A role passed for a customization cannot be passed**, so the create is refused with
//!    the modelled `AccessDeniedException` (`@httpError(403)`) carrying the one sentence the
//!    service states that denial with — for every value the member can carry, because the
//!    principal that would take the role on is Amazon Bedrock's own and so is a principal of
//!    no caller's account. The rows differ along everything the request can differ along:
//!    the role's account, path, partition and length, both `baseModelIdentifier` spellings
//!    (including one the catalog does not publish, which only a LATER step could report on),
//!    every `customizationType`, the tag counts on both tagged resources (including more
//!    than one resource may carry), the name lengths at both modelled bounds, an
//!    `@idempotencyToken` and the signing region.
//! 2. **The decision stands in front of everything the request names**: it is taken before
//!    `baseModelIdentifier` is resolved and before the per-resource tag bound is applied, so
//!    a request that would have been refused by either still draws the 403 — the status
//!    class, the error code and the retryability a client branches on are the
//!    authorization's.
//! 3. **A denied pass leaves nothing behind**: `GetModelCustomizationJob` answers 404 for
//!    the `jobName` it carried, `ListModelCustomizationJobs(nameContains=<jobName>)` reports
//!    no job, `ListCustomModels(nameContains=<customModelName>)` reports no model, and a
//!    repeat of the very same request — `clientRequestToken` included — draws the identical
//!    refusal rather than replaying a mint.
//! 4. **A value outside a member's published FORM is still refused for that form**, 400
//!    `ValidationException`, ahead of the authorization decision: the request has to be a
//!    request before there is a role in it to pass.
//!
//! Every expectation is derived from the operation's own model
//! (`smithy_query(query="operation",operation="CreateModelCustomizationJob,ListModelCustomizationJobs,ListCustomModels")`)
//! or from the published catalog this service serves through `ListFoundationModels`.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// The sentence Amazon Bedrock states a denied pass-role with, spelled here rather than
/// read off the service so this suite pins the wording a client sees.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// The published foundation model catalog this service serves (`state::FOUNDATION_MODELS`),
/// so the base models these rows carry follow the catalog rather than a pinned spelling.
const CATALOG: &[service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FoundationModel] =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS;

/// The most tags one resource of this service may carry, as `TooManyTagsException`
/// documents it ("50 tags per resource").
const MAX_TAGS: usize =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_TAGS_PER_RESOURCE;

/// `CustomizationType` values (`smithy_query(query="shape",shape="CustomizationType")`).
const CUSTOMIZATION_TYPES: &[&str] = &[
    "CONTINUED_PRE_TRAINING",
    "DISTILLATION",
    "FINE_TUNING",
    "IMPORTED",
    "REINFORCEMENT_FINE_TUNING",
];

/// A base model the catalog publishes as supporting `FINE_TUNING`: the identifier a caller
/// that read `ListFoundationModels` would submit.
fn fine_tuning_model() -> &'static str {
    CATALOG
        .iter()
        .find(|model| model.customizations_supported.contains(&"FINE_TUNING"))
        .map(|model| model.model_id)
        .expect("the catalog must publish a base model supporting FINE_TUNING")
}

/// The `FoundationModelArn` spelling of a base model id.
fn foundation_model_arn(model_id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}::foundation-model/{model_id}")
}

/// `n` distinct tags, each key and value inside `TagKey`/`TagValue`'s own pattern
/// (`^[a-zA-Z0-9\s._:/=+@-]*$`).
fn tags(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "key": format!("key-{index}"), "value": format!("value-{index}") }))
            .collect(),
    )
}

/// `character` repeated `count` times, for a member driven to one of its `@length` bounds.
fn repeat(character: char, count: usize) -> String {
    std::iter::repeat_n(character, count).collect()
}

/// A `CreateModelCustomizationJob` body carrying the six `@required` members plus whatever
/// `extra` adds.
fn create_body(
    job_name: &str,
    model_name: &str,
    base_model: &str,
    role: &str,
    extra: Value,
) -> Value {
    let mut body = json!({
        "jobName": job_name,
        "customModelName": model_name,
        "baseModelIdentifier": base_model,
        "roleArn": role,
        "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
        "outputDataConfig": { "s3Uri": "s3://customization-output/" },
    });
    let members = body.as_object_mut().expect("an object");
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        members.insert(member, value);
    }
    body
}

async fn create(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/model-customization-jobs", Some(body))
        .await
}

async fn get_job(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/model-customization-jobs/{}", label(identifier)),
        None,
    )
    .await
}

async fn list_jobs(emu: &Emulator, region: &str, name_contains: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/model-customization-jobs?nameContains={name_contains}"),
        None,
    )
    .await
}

async fn list_custom_models(emu: &Emulator, region: &str, name_contains: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/custom-models?nameContains={name_contains}"),
        None,
    )
    .await
}

/// The rows a listing reports under `member`, with an omitted list read as no rows.
fn listing_rows(reply: &Reply, member: &str) -> Vec<Value> {
    match reply.get(member) {
        Value::Null => Vec::new(),
        Value::Array(rows) => rows,
        other => panic!("member {member} must be a list, got {other}"),
    }
}

/// The `message` a refusal states.
fn refusal_message(reply: &Reply) -> String {
    reply.get("message").as_str().unwrap_or_default().to_string()
}

/// One request the pass-role decision answers: its label, the region it is signed for, and
/// its body.
struct Row {
    case: &'static str,
    region: &'static str,
    body: Value,
}

/// Every request whose `roleArn` and whose other members are inside their published forms,
/// so the pass-role decision is what answers each one.
fn passable_form_rows() -> Vec<Row> {
    let model = fine_tuning_model();
    let mut rows = vec![
        Row {
            case: "a role of the account this emulator serves",
            region: REGION,
            body: create_body(
                "pass-role-own-account",
                "PassRoleOwnAccount",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({}),
            ),
        },
        Row {
            case: "a role of another account",
            region: REGION,
            body: create_body(
                "pass-role-other-account",
                "PassRoleOtherAccount",
                model,
                "arn:aws:iam::210987654321:role/bedrock-customization",
                json!({}),
            ),
        },
        Row {
            // The member's pattern admits an EMPTY account field
            // (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
            case: "a role ARN carrying no account at all",
            region: REGION,
            body: create_body(
                "pass-role-no-account",
                "PassRoleNoAccount",
                model,
                "arn:aws:iam:::role/bedrock-customization",
                json!({}),
            ),
        },
        Row {
            case: "a role behind a path, in another partition",
            region: REGION,
            body: create_body(
                "pass-role-path",
                "PassRolePath",
                model,
                "arn:aws-us-gov:iam::210987654321:role/service-role/BedrockCustomization",
                json!({}),
            ),
        },
        Row {
            // `RoleArn` is `@length(..2048)`, and `arn:aws:iam::000000000000:role/` is 31
            // characters of it.
            case: "a role ARN at the published length bound",
            region: REGION,
            body: create_body(
                "pass-role-longest",
                "PassRoleLongest",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/{}", repeat('r', 2017)),
                json!({}),
            ),
        },
        Row {
            case: "the base model named by its foundation-model ARN",
            region: REGION,
            body: create_body(
                "pass-role-model-arn",
                "PassRoleModelArn",
                &foundation_model_arn(model),
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({}),
            ),
        },
        Row {
            // A base model no catalog row carries: only a step AFTER the authorization
            // could report it absent, so this row pins the ordering.
            case: "a base model the catalog does not publish",
            region: REGION,
            body: create_body(
                "pass-role-unknown-model",
                "PassRoleUnknownModel",
                "acme.not-a-model-v1",
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({}),
            ),
        },
        Row {
            // A custom-model ARN of this account, which holds no custom model either.
            case: "a base model named by a custom-model ARN this account does not hold",
            region: REGION,
            body: create_body(
                "pass-role-custom-model",
                "PassRoleCustomModel",
                &format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{model}/abcdef012345"),
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({}),
            ),
        },
        Row {
            // More tags than one resource may carry: the tag bound could only answer after
            // the authorization, so this row pins that ordering too.
            case: "more tags than either tagged resource may carry",
            region: REGION,
            body: create_body(
                "pass-role-too-many-tags",
                "PassRoleTooManyTags",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({ "jobTags": tags(MAX_TAGS + 1), "customModelTags": tags(MAX_TAGS + 1) }),
            ),
        },
        Row {
            case: "the tag counts a resource may carry, and an idempotency token",
            region: REGION,
            body: create_body(
                "pass-role-tagged",
                "PassRoleTagged",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({
                    "jobTags": tags(MAX_TAGS),
                    "customModelTags": tags(1),
                    "clientRequestToken": "a-token-for-a-denied-create",
                    "hyperParameters": { "epochCount": "2" },
                    "validationDataConfig": {
                        "validators": [{ "s3Uri": "s3://customization-data/validate.jsonl" }],
                    },
                    "customModelKmsKeyId": "11111111-2222-3333-4444-555555555555",
                    "vpcConfig": {
                        "subnetIds": ["subnet-0a1b2c3d"],
                        "securityGroupIds": ["sg-0123456789abcdef0"],
                    },
                }),
            ),
        },
        Row {
            case: "names at the minimum modelled length",
            region: REGION,
            body: create_body(
                "a",
                "b",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({}),
            ),
        },
        Row {
            case: "names at the maximum modelled length",
            region: REGION,
            body: create_body(
                &format!("j{}", repeat('x', 62)),
                &format!("m{}", repeat('y', 62)),
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({}),
            ),
        },
        Row {
            case: "another signing region",
            region: "eu-central-1",
            body: create_body(
                "pass-role-elsewhere",
                "PassRoleElsewhere",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({}),
            ),
        },
    ];
    // One row per modelled `customizationType`, asked for by name: the decision is taken
    // before the catalog is consulted about which of them the base model is published for.
    for (index, customization) in CUSTOMIZATION_TYPES.iter().enumerate() {
        rows.push(Row {
            case: "a customization asked for by name",
            region: REGION,
            body: create_body(
                &format!("pass-role-customization-{index}"),
                &format!("PassRoleCustomization{index}"),
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
                json!({ "customizationType": customization }),
            ),
        });
    }
    rows
}

/// The `jobName` and `customModelName` a row carries, which the effect-absent leg reads
/// back.
fn names(body: &Value) -> (String, String) {
    let member = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the row must carry {name}"))
            .to_string()
    };
    (member("jobName"), member("customModelName"))
}

#[tokio::test]
async fn a_customization_role_cannot_be_passed_and_the_refusal_stores_nothing() {
    let emu = emulator();

    for Row { case, region, body } in passable_form_rows() {
        let (job_name, model_name) = names(&body);
        // The same request twice: the refusal is the decision's, so it is the answer every
        // time, and a replayed `clientRequestToken` has no mint to replay.
        for attempt in ["first", "repeated"] {
            let refused = create(&emu, region, body.clone()).await;
            assert_eq!(
                refused.status, 403,
                "case {case:?} ({attempt} attempt): a role that may not be passed is refused \
                 403, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error.as_deref(),
                Some("AccessDeniedException"),
                "case {case:?} ({attempt} attempt): the refusal carries the modelled error \
                 code, got {:?}",
                refused.error
            );
            assert_eq!(
                refusal_message(&refused),
                PASS_ROLE_REFUSAL,
                "case {case:?} ({attempt} attempt): expected the pass-role denial \
                 {PASS_ROLE_REFUSAL:?}, got {:?}",
                refusal_message(&refused)
            );
            assert!(
                refused.get("jobArn").is_null(),
                "case {case:?} ({attempt} attempt): a refusal mints no job ARN, got {:?}",
                refused.body
            );
        }

        // Nothing the refused create named exists afterwards: not the job, and not the
        // model it would have customized.
        let read = get_job(&emu, region, &job_name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a denied pass leaves no job to read, got {} {:?}",
            read.status, read.body
        );
        let listed = list_jobs(&emu, region, &job_name).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListModelCustomizationJobs must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        assert!(
            listing_rows(&listed, "modelCustomizationJobSummaries").is_empty(),
            "case {case:?}: a denied pass leaves no job to list, got {:?}",
            listed.body
        );
        let models = list_custom_models(&emu, region, &model_name).await;
        assert_eq!(
            models.status, 200,
            "case {case:?}: ListCustomModels must answer 200, got {} {:?}",
            models.status, models.body
        );
        assert!(
            listing_rows(&models, "modelSummaries").is_empty(),
            "case {case:?}: a denied pass leaves no custom model to list, got {:?}",
            models.body
        );
    }
}

#[tokio::test]
async fn a_member_outside_its_published_form_is_refused_for_that_form() {
    let emu = emulator();
    let model = fine_tuning_model();
    let role = format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization");

    // Each row drives ONE member outside the form its Smithy model declares. A value of
    // that kind is refused for its form — 400 `ValidationException` — ahead of the
    // pass-role decision, so none of these rows draws the 403 above.
    let cases: Vec<(&str, Value)> = vec![
        (
            "roleArn that is no ARN at all",
            create_body("form-no-arn", "FormNoArn", model, "bedrock-customization", json!({})),
        ),
        (
            "roleArn naming an IAM user rather than a role",
            create_body(
                "form-user-arn",
                "FormUserArn",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:user/not-a-role"),
                json!({}),
            ),
        ),
        (
            "roleArn one past @length max 2048",
            create_body(
                "form-long-arn",
                "FormLongArn",
                model,
                &format!("arn:aws:iam::{ACCOUNT}:role/{}", repeat('r', 2018)),
                json!({}),
            ),
        ),
        (
            "jobName outside its published pattern",
            create_body("-leading-hyphen", "FormJobName", model, &role, json!({})),
        ),
        (
            "customModelName one past @length max 63",
            create_body("form-model-name", &repeat('m', 64), model, &role, json!({})),
        ),
        (
            "baseModelIdentifier one under @length min 1",
            create_body("form-base-model", "FormBaseModel", "", &role, json!({})),
        ),
        (
            "clientRequestToken outside its published pattern",
            create_body(
                "form-token",
                "FormToken",
                model,
                &role,
                json!({ "clientRequestToken": "a token with spaces" }),
            ),
        ),
    ];
    for (case, body) in cases {
        let refused = create(&emu, REGION, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a value outside its published form is ValidationException (400), \
             got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal carries the modelled error code, got {:?}",
            refused.error
        );
    }

    // And no row stored anything either: a refused create is a refused create whichever
    // step refused it.
    let listed = list_jobs(&emu, REGION, "form").await;
    assert!(
        listing_rows(&listed, "modelCustomizationJobSummaries").is_empty(),
        "a refused create stores no job, got {:?}",
        listed.body
    );
}
