// SPDX-License-Identifier: CC0-1.0

//! The `iam:PassRole` decision `CreateModelCustomizationJob` is settled by, end to end over
//! the same `MultiServiceRouter` the emulator boots (`crate::support`).
//!
//! The rules under test, each asserted over a SET of requests rather than over one:
//!
//! 1. **A caller may pass a role of its OWN account**, so such a create is ADMITTED: 201
//!    with the `@required` `jobArn`, a job a read then reports `InProgress` under the role
//!    the request named. The rows differ along everything the role value can differ along
//!    while still naming a role of this account — a bare name, a name behind an IAM path,
//!    the empty account field the member's pattern admits, the value at the published
//!    `@length` bound — and along the rest of the request: both `baseModelIdentifier`
//!    spellings, the customization asked for by name or left to the operation's default, the
//!    tag counts a resource may carry, an `@idempotencyToken` and the signing region.
//!    The pass is REFUSED only on what this account's IAM ANSWERED about the role, and this
//!    suite mounts the one service, so there is no IAM here to answer: these rows pin the
//!    grounding half of the decision. The half IAM answers — whether the account holds that
//!    role, and whether the trust policy it carries grants Amazon Bedrock the assume this
//!    pass would make — is pinned in
//!    `core/router/tests/all/bedrock_model_customization_pass_role.rs`, where this service
//!    and IAM are mounted over one engine.
//! 2. **A role of ANOTHER account cannot be passed**, so the create is refused with the
//!    modelled `AccessDeniedException` (`@httpError(403)`) carrying the one sentence the
//!    service states that denial with — over rows differing in the other account's digits,
//!    the path the role sits behind, the partition and the length.
//! 3. **The decision stands in front of everything the request RESOLVES, but behind the
//!    FORM of the model identifier it carries**: a `baseModelIdentifier` that identifies no
//!    customizable base model variant (a bare id with no version qualifier) draws that
//!    reading's own 400 under either role, while a body a step behind the pass refuses (a
//!    `custom-model` ARN this account does not hold, a customization the base model is not
//!    published for, more tags than a resource may carry) draws the 403 when the role is
//!    another account's and that later step's own refusal when the role is this account's —
//!    so an admitting decision does not swallow what follows it.
//! 4. **A denied pass leaves nothing behind**: `GetModelCustomizationJob` answers 404 for the
//!    `jobName` it carried, `ListModelCustomizationJobs(nameContains=<jobName>)` reports no
//!    job, `ListCustomModels(nameContains=<customModelName>)` reports no model, and a repeat
//!    of the very same request — `clientRequestToken` included — draws the identical refusal
//!    rather than replaying a mint.
//! 5. **A value outside a member's published FORM is still refused for that form**, 400
//!    `ValidationException`, ahead of the authorization decision: the request has to be a
//!    request before there is a role in it to pass.
//!
//! Every expectation is derived from the operation's own model
//! (`smithy_query(query="operation",operation="CreateModelCustomizationJob,GetModelCustomizationJob,ListModelCustomizationJobs,ListCustomModels")`)
//! or from the published catalog this service serves through `ListFoundationModels`.

use serde_json::{json, Value};

use crate::support::{emulator, label, wall_clock_seconds, Emulator, Reply, ACCOUNT, REGION};

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

/// `ModelCustomizationJobStatus` — the status a job that has just been created carries
/// (`smithy_query(query="shape",shape="ModelCustomizationJobStatus")`).
const IN_PROGRESS: &str =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_CUSTOMIZATION_JOB_STATUS_IN_PROGRESS;

/// `CustomizationType` values (`smithy_query(query="shape",shape="CustomizationType")`).
const CUSTOMIZATION_TYPES: &[&str] = &[
    "CONTINUED_PRE_TRAINING",
    "DISTILLATION",
    "FINE_TUNING",
    "IMPORTED",
    "REINFORCEMENT_FINE_TUNING",
];

/// The customization a create that names none runs: "Creates a fine-tuning job to customize
/// a base model" (`smithy_query(query="operation",operation="CreateModelCustomizationJob")`).
const DEFAULT_CUSTOMIZATION: &str = "FINE_TUNING";

/// A base model the catalog publishes as supporting `FINE_TUNING`: the identifier a caller
/// that read `ListFoundationModels(byCustomizationType=FINE_TUNING)` would submit. Amazon
/// Bedrock publishes a customizable model under its training-qualified id, so this is the
/// id the catalog carries the customization on.
fn fine_tuning_model() -> &'static str {
    CATALOG
        .iter()
        .find(|model| model.customizations_supported.contains(&DEFAULT_CUSTOMIZATION))
        .map(|model| model.model_id)
        .expect("the catalog must publish a base model supporting FINE_TUNING")
}

/// A base model the catalog publishes whose id carries NO version qualifier: the on-demand
/// spelling of a published model, which identifies no customizable variant of it.
fn unqualified_model() -> &'static str {
    CATALOG
        .iter()
        .find(|model| !model.model_id.contains(':'))
        .map(|model| model.model_id)
        .expect("the catalog must publish a model whose id carries no version qualifier")
}

/// A published base model and a modelled `CustomizationType` that model is NOT published
/// for: the pair a later step of the create refuses, taken from the catalog rather than
/// written out.
fn unsupported_pair() -> (&'static str, &'static str) {
    CATALOG
        .iter()
        .filter(|model| !model.customizations_supported.is_empty())
        .find_map(|model| {
            CUSTOMIZATION_TYPES
                .iter()
                .find(|customization| !model.customizations_supported.contains(*customization))
                .map(|customization| (model.model_id, *customization))
        })
        .expect("the catalog must publish a model that does not support every customization")
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

/// A role of the account this emulator serves, named `name`.
fn own_role(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
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

/// The `jobName` and `customModelName` a row carries, which the effect legs read back.
fn names(body: &Value) -> (String, String) {
    let member = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the row must carry {name}"))
            .to_string()
    };
    (member("jobName"), member("customModelName"))
}

/// One request the pass-role decision admits: its label, the region it is signed for, its
/// body, and the customization the admitted job must report.
struct Admitted {
    case: &'static str,
    region: &'static str,
    body: Value,
    customization: &'static str,
}

/// Every request whose `roleArn` names a role of the account this emulator serves, so the
/// pass-role decision ADMITS it and the create goes on to mint a job.
fn admitted_rows() -> Vec<Admitted> {
    let model = fine_tuning_model();
    let mut rows = vec![
        Admitted {
            case: "a role of the account this emulator serves",
            region: REGION,
            body: create_body(
                "pass-role-own-account",
                "PassRoleOwnAccount",
                model,
                &own_role("bedrock-customization"),
                json!({}),
            ),
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            case: "a role of this account behind an IAM path",
            region: REGION,
            body: create_body(
                "pass-role-path",
                "PassRolePath",
                model,
                &own_role("service-role/BedrockCustomizationExecution"),
                json!({}),
            ),
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            // The member's pattern admits an EMPTY account field
            // (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`), and a value that names no
            // account names no OTHER account either.
            case: "a role ARN carrying no account at all",
            region: REGION,
            body: create_body(
                "pass-role-no-account",
                "PassRoleNoAccount",
                model,
                "arn:aws:iam:::role/bedrock-customization",
                json!({}),
            ),
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            // `RoleArn` is `@length(..2048)`, and `arn:aws:iam::000000000000:role/` is 31
            // characters of it.
            case: "a role of this account at the published length bound",
            region: REGION,
            body: create_body(
                "pass-role-longest",
                "PassRoleLongest",
                model,
                &own_role(&repeat('r', 2017)),
                json!({}),
            ),
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            case: "the base model named by its foundation-model ARN",
            region: REGION,
            body: create_body(
                "pass-role-model-arn",
                "PassRoleModelArn",
                &foundation_model_arn(model),
                &own_role("bedrock-customization"),
                json!({}),
            ),
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            case: "the tag counts a resource may carry, and an idempotency token",
            region: REGION,
            body: create_body(
                "pass-role-tagged",
                "PassRoleTagged",
                model,
                &own_role("bedrock-customization"),
                json!({
                    "jobTags": tags(MAX_TAGS),
                    "customModelTags": tags(1),
                    "clientRequestToken": "a-token-for-an-admitted-create",
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
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            case: "names at the minimum modelled length",
            region: REGION,
            body: create_body("a", "b", model, &own_role("bedrock-customization"), json!({})),
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            case: "names at the maximum modelled length",
            region: REGION,
            body: create_body(
                &format!("j{}", repeat('x', 62)),
                &format!("m{}", repeat('y', 62)),
                model,
                &own_role("bedrock-customization"),
                json!({}),
            ),
            customization: DEFAULT_CUSTOMIZATION,
        },
        Admitted {
            case: "another signing region",
            region: "eu-central-1",
            body: create_body(
                "pass-role-elsewhere",
                "PassRoleElsewhere",
                model,
                &own_role("bedrock-customization"),
                json!({}),
            ),
            customization: DEFAULT_CUSTOMIZATION,
        },
    ];
    // One row per customization the base model this row carries IS published for, asked for
    // by name: an admitted pass hands the request on to the catalog, which is what decides
    // the customization, so each supported one must create a job reporting it.
    let supported = CATALOG
        .iter()
        .find(|catalogued| catalogued.model_id == model)
        .expect("the base model comes from the catalog");
    for (index, customization) in supported.customizations_supported.iter().enumerate() {
        rows.push(Admitted {
            case: "a customization the base model is published for, asked for by name",
            region: REGION,
            body: create_body(
                &format!("pass-role-customization-{index}"),
                &format!("PassRoleCustomization{index}"),
                model,
                &own_role("bedrock-customization"),
                json!({ "customizationType": customization }),
            ),
            customization,
        });
    }
    rows
}

/// One request the pass-role decision denies: its label and the `roleArn` that earns the
/// denial.
struct Denied {
    case: &'static str,
    role: String,
}

/// Every `roleArn` the member's published pattern admits that names a role of some OTHER
/// account, which no principal of this account can pass.
fn denied_roles() -> Vec<Denied> {
    vec![
        Denied {
            case: "a role of another account",
            role: "arn:aws:iam::210987654321:role/bedrock-customization".to_string(),
        },
        Denied {
            case: "a role of another account, different digits",
            role: "arn:aws:iam::999999999999:role/bedrock-customization".to_string(),
        },
        Denied {
            case: "a role of another account behind an IAM path",
            role: "arn:aws:iam::210987654321:role/service-role/BedrockCustomization".to_string(),
        },
        Denied {
            case: "a role of another account in another partition",
            role: "arn:aws-us-gov:iam::210987654321:role/BedrockCustomization".to_string(),
        },
        Denied {
            case: "a role of another account at the published length bound",
            role: format!("arn:aws:iam::210987654321:role/{}", repeat('r', 2017)),
        },
    ]
}

#[tokio::test]
async fn a_role_of_this_account_may_be_passed_and_the_create_mints_a_job() {
    let emu = emulator();

    for Admitted {
        case,
        region,
        body,
        customization,
    } in admitted_rows()
    {
        let (job_name, model_name) = names(&body);
        let role = body
            .get("roleArn")
            .and_then(Value::as_str)
            .expect("the row must carry roleArn")
            .to_string();

        let before = wall_clock_seconds();
        let created = create(&emu, region, body.clone()).await;
        let after = wall_clock_seconds();
        assert_eq!(
            created.status, 201,
            "case {case:?}: a role of this account may be passed, so the create answers 201, \
             got {} {:?}",
            created.status, created.body
        );
        let job_arn = created.text("jobArn");
        assert!(
            job_arn.starts_with(&format!(
                "arn:aws:bedrock:{region}:{ACCOUNT}:model-customization-job/"
            )),
            "case {case:?}: the minted jobArn must be a ModelCustomizationJobArn of this \
             account in {region}, got {job_arn:?}"
        );

        // The job the create minted is the job a read finds, under either
        // `ModelCustomizationJobIdentifier` spelling.
        for identifier in [job_arn.as_str(), job_name.as_str()] {
            let read = get_job(&emu, region, identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the minted job must be readable by {identifier:?}, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.text("jobArn"),
                job_arn,
                "case {case:?}: the read must report the ARN the create minted, got {:?}",
                read.get("jobArn")
            );
            assert_eq!(
                read.text("jobName"),
                job_name,
                "case {case:?}: the read must report the jobName the create carried, got {:?}",
                read.get("jobName")
            );
            assert_eq!(
                read.text("roleArn"),
                role,
                "case {case:?}: the read must report the role that was passed, got {:?}",
                read.get("roleArn")
            );
            assert_eq!(
                read.text("outputModelName"),
                model_name,
                "case {case:?}: the read must report the customModelName the create carried, \
                 got {:?}",
                read.get("outputModelName")
            );
            assert_eq!(
                read.text("status"),
                IN_PROGRESS,
                "case {case:?}: a job the create just minted is reported {IN_PROGRESS}, got \
                 {:?}",
                read.get("status")
            );
            assert_eq!(
                read.text("customizationType"),
                customization,
                "case {case:?}: the read must report the customization the job runs, got {:?}",
                read.get("customizationType")
            );
            let created_at = read.instant("creationTime");
            assert!(
                created_at >= before - 1.0 && created_at <= after + 1.0,
                "case {case:?}: creationTime {created_at} must fall inside the window \
                 [{before}, {after}] that bracketed the create"
            );
        }

        // And the listing carries it exactly once, in the region it was created in.
        let listed = list_jobs(&emu, region, &job_name).await;
        let listed_once = listing_rows(&listed, "modelCustomizationJobSummaries")
            .iter()
            .filter(|row| row.get("jobArn").and_then(Value::as_str) == Some(job_arn.as_str()))
            .count();
        assert_eq!(
            listed_once, 1,
            "case {case:?}: the minted job must be listed exactly once under the name it \
             carries, got {:?}",
            listed.body
        );
    }
}

#[tokio::test]
async fn an_idempotent_replay_of_an_admitted_create_replays_the_mint() {
    let case = "a replayed clientRequestToken";
    let emu = emulator();
    let role = own_role("bedrock-customization");
    let body = create_body(
        "pass-role-idempotent",
        "PassRoleIdempotent",
        fine_tuning_model(),
        &role,
        json!({ "clientRequestToken": "pass-role-idempotency-token" }),
    );

    let first = create(&emu, REGION, body.clone()).await;
    assert_eq!(
        first.status, 201,
        "an admitted create answers 201, got {} {:?}",
        first.status, first.body
    );
    let replay = create(&emu, REGION, body).await;
    assert_eq!(
        replay.status, 201,
        "case {case:?}: the replay answers as the first request did, got {} {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("jobArn"),
        first.text("jobArn"),
        "case {case:?}: a replay must report the ARN the first request minted, got {:?}",
        replay.get("jobArn")
    );
    let listed = list_jobs(&emu, REGION, "pass-role-idempotent").await;
    assert_eq!(
        listing_rows(&listed, "modelCustomizationJobSummaries").len(),
        1,
        "case {case:?}: a replay mints no second job, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_role_of_another_account_cannot_be_passed_and_the_refusal_stores_nothing() {
    let emu = emulator();
    let model = fine_tuning_model();

    for (index, Denied { case, role }) in denied_roles().into_iter().enumerate() {
        let job_name = format!("denied-pass-role-{index}");
        let model_name = format!("DeniedPassRole{index}");
        let body = create_body(
            &job_name,
            &model_name,
            model,
            &role,
            json!({ "clientRequestToken": format!("denied-token-{index}") }),
        );

        // The same request twice: the refusal is the decision's, so it is the answer every
        // time, and a replayed `clientRequestToken` has no mint to replay.
        for attempt in ["first", "repeated"] {
            let refused = create(&emu, REGION, body.clone()).await;
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
        let read = get_job(&emu, REGION, &job_name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a denied pass leaves no job to read, got {} {:?}",
            read.status, read.body
        );
        let listed = list_jobs(&emu, REGION, &job_name).await;
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
        let models = list_custom_models(&emu, REGION, &model_name).await;
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
async fn the_model_identifier_is_read_before_the_pass_role_decision() {
    let emu = emulator();
    let model = fine_tuning_model();
    let (unsupported_model, unsupported_customization) = unsupported_pair();
    let foreign_role = "arn:aws:iam::210987654321:role/bedrock-customization";

    // Each row is a body ONE step of the create refuses, with the status and code that step
    // answers and whether that step is taken AHEAD of the pass-role decision. The same body
    // is submitted twice: with a role of another account, and with a role of this account.
    // A step ahead of the pass answers in BOTH legs — the FORM of the identifier a request
    // asks to customize is read before who may ask for it — while a step behind it (every
    // step that RESOLVES what the request named, the catalogue included) answers only once
    // the pass is admitted.
    let rows: Vec<(&str, Value, u16, &str, bool)> = vec![
        (
            "a baseModelIdentifier that identifies no model variant",
            json!({
                "jobName": "order-unknown-model",
                "customModelName": "OrderUnknownModel",
                "baseModelIdentifier": "acme.not-a-model-v1",
                "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
                "outputDataConfig": { "s3Uri": "s3://customization-output/" },
            }),
            400,
            "ValidationException",
            true,
        ),
        (
            "a published model by its unqualified on-demand id",
            json!({
                "jobName": "order-unqualified-model",
                "customModelName": "OrderUnqualifiedModel",
                "baseModelIdentifier": unqualified_model(),
                "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
                "outputDataConfig": { "s3Uri": "s3://customization-output/" },
            }),
            400,
            "ValidationException",
            true,
        ),
        (
            "a custom-model ARN this account does not hold",
            json!({
                "jobName": "order-unknown-custom-model",
                "customModelName": "OrderUnknownCustomModel",
                "baseModelIdentifier":
                    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{model}/abcdef012345"),
                "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
                "outputDataConfig": { "s3Uri": "s3://customization-output/" },
            }),
            404,
            "ResourceNotFoundException",
            false,
        ),
        (
            "a customization the base model is not published for",
            json!({
                "jobName": "order-unsupported-customization",
                "customModelName": "OrderUnsupportedCustomization",
                "baseModelIdentifier": unsupported_model,
                "customizationType": unsupported_customization,
                "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
                "outputDataConfig": { "s3Uri": "s3://customization-output/" },
            }),
            400,
            "ValidationException",
            false,
        ),
        (
            "more tags than either tagged resource may carry",
            json!({
                "jobName": "order-too-many-tags",
                "customModelName": "OrderTooManyTags",
                "baseModelIdentifier": model,
                "jobTags": tags(MAX_TAGS + 1),
                "customModelTags": tags(MAX_TAGS + 1),
                "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
                "outputDataConfig": { "s3Uri": "s3://customization-output/" },
            }),
            400,
            "TooManyTagsException",
            false,
        ),
    ];

    for (case, body, step_status, step_code, ahead_of_the_pass) in rows {
        let with_role = |role: &str| {
            let mut body = body.clone();
            body.as_object_mut()
                .expect("an object")
                .insert("roleArn".to_string(), json!(role));
            body
        };

        let foreign = create(&emu, REGION, with_role(foreign_role)).await;
        let (expected_status, expected_code, expected_message) = if ahead_of_the_pass {
            (step_status, step_code, None)
        } else {
            (403, "AccessDeniedException", Some(PASS_ROLE_REFUSAL))
        };
        assert_eq!(
            (foreign.status, foreign.error.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: with a role of another account the answer is the step that runs \
             first ({expected_status} {expected_code}), got {} {:?}",
            foreign.status,
            foreign.body
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                refusal_message(&foreign),
                expected_message,
                "case {case:?}: expected the pass-role denial {expected_message:?}, got {:?}",
                refusal_message(&foreign)
            );
        }

        let own = create(&emu, REGION, with_role(&own_role("bedrock-customization"))).await;
        assert_eq!(
            (own.status, own.error.as_deref()),
            (step_status, Some(step_code)),
            "case {case:?}: with a role of this account the pass is admitted and the step \
             answers {step_status} {step_code}, got {} {:?}",
            own.status,
            own.body
        );
    }

    // No row of either leg stored a job: a refused create is a refused create whichever
    // step refused it.
    let listed = list_jobs(&emu, REGION, "order-").await;
    assert!(
        listing_rows(&listed, "modelCustomizationJobSummaries").is_empty(),
        "a refused create stores no job, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_member_outside_its_published_form_is_refused_for_that_form() {
    let emu = emulator();
    let model = fine_tuning_model();
    let role = own_role("bedrock-customization");

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
                &own_role(&repeat('r', 2018)),
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

/// RULE (the grounding half of the pass-role decision): a pass is refused only on what this
/// account's IAM ANSWERED about the role. With the one service mounted there is no IAM to
/// ask, so nothing is known about any role of this account and no create is refused for its
/// role — while a role of ANOTHER account needs no IAM answer at all, because `iam:PassRole`
/// is granted by the account that HOLDS the role.
#[tokio::test]
async fn a_pass_is_refused_only_on_what_this_accounts_iam_answered() {
    let emu = emulator();
    let model = fine_tuning_model();

    // The rows differ along which account the `roleArn` names and, inside each, along the
    // forms the member's published pattern admits: a bare name, a name behind an IAM path,
    // the empty account field, and another partition.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a role of this account that no IAM here holds",
            own_role("grounding-role"),
            true,
        ),
        (
            "a role of this account behind an IAM path",
            own_role("service-role/GroundingRole"),
            true,
        ),
        (
            "a role ARN carrying no account at all",
            "arn:aws:iam:::role/grounding-role".to_string(),
            true,
        ),
        (
            "a role of this account in another partition",
            format!("arn:aws-us-gov:iam::{ACCOUNT}:role/grounding-role"),
            true,
        ),
        (
            "a role of another account",
            "arn:aws:iam::210987654321:role/grounding-role".to_string(),
            false,
        ),
        (
            "a role of another account behind an IAM path",
            "arn:aws:iam::210987654321:role/service-role/GroundingRole".to_string(),
            false,
        ),
    ];

    for (index, (case, role, admitted)) in rows.into_iter().enumerate() {
        let job_name = format!("grounding-{index}");
        let model_name = format!("Grounding{index}");
        let reply = create(
            &emu,
            REGION,
            create_body(&job_name, &model_name, model, &role, json!({})),
        )
        .await;

        if admitted {
            assert_eq!(
                reply.status, 201,
                "case {case:?} ({role}): with nothing IAM answered the pass is not refused, \
                 got {} {:?}",
                reply.status, reply.body
            );
            let read = get_job(&emu, REGION, &job_name).await;
            assert_eq!(
                (read.status, read.text("roleArn").as_str()),
                (200, role.as_str()),
                "case {case:?}: the minted job must read back under the role it was passed, \
                 got {} {:?}",
                read.status,
                read.body
            );
        } else {
            assert_eq!(
                (
                    reply.status,
                    reply.error.as_deref(),
                    refusal_message(&reply).as_str()
                ),
                (403, Some("AccessDeniedException"), PASS_ROLE_REFUSAL),
                "case {case:?} ({role}): a role of another account cannot be passed, got {} \
                 {:?}",
                reply.status,
                reply.body
            );
            // The refusal left nothing behind: no job under that name, and no custom model
            // reserved under the one the request carried.
            let read = get_job(&emu, REGION, &job_name).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: a refused create stores no job, got {} {:?}",
                read.status, read.body
            );
            let listed = list_jobs(&emu, REGION, &job_name).await;
            assert!(
                listing_rows(&listed, "modelCustomizationJobSummaries").is_empty(),
                "case {case:?}: a refused create is listed nowhere, got {:?}",
                listed.body
            );
            let models = list_custom_models(&emu, REGION, &model_name).await;
            assert!(
                listing_rows(&models, "modelSummaries").is_empty(),
                "case {case:?}: a refused create reserves no custom model, got {:?}",
                models.body
            );
        }
    }
}
