// SPDX-License-Identifier: CC0-1.0

//! `DeleteCustomModel` end-to-end, over the same `MultiServiceRouter` the emulator boots
//! (`crate::support`): what a delete removes, what it leaves behind and how it refuses.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A delete removes exactly the custom model its `modelIdentifier` names**, whichever
//!    of the two spellings that member admits named it ("Name of the model to delete", and
//!    the member's pattern admits the model's ARN too,
//!    `smithy_query(query="operation",operation="DeleteCustomModel")`): the model is gone
//!    from `GetCustomModel` and from `ListCustomModels`, every other stored model is
//!    untouched, and the operation answers 200 with an empty body (its output shape has no
//!    members).
//! 2. **An identifier that resolves to no custom model of this account and Region is
//!    refused as the spelling it IS** — including the identifier of a model this suite has
//!    already deleted, which is what makes two deletes racing for one model answer exactly
//!    one success: an ARN that names no model is the missing ARN `ResourceNotFoundException`
//!    reports (404, "The specified resource Amazon Resource Name (ARN) was not found",
//!    `smithy_query(query="shape",shape="ResourceNotFoundException")`) carrying this
//!    resource's not-found sentence, while a bare NAME that resolves to nothing is no ARN to
//!    report missing but an identifier the service could not resolve — 400
//!    `ValidationException` with Amazon Bedrock's one sentence for that class, exactly as
//!    `GetCustomModel` refuses the same two spellings.
//! 3. **A delete leaves the model's NAME free**, so a later create may mint a new model
//!    under it — a different model, with its own ARN.
//! 4. **A model a custom model deployment is still serving is not removed**: that
//!    deployment carries "the custom model this deployment serves"
//!    (`smithy_query(query="operation",operation="CreateCustomModelDeployment")`), so the
//!    delete is refused with 400 `ConflictException` naming the model and the deployment,
//!    the model stays readable, and a deployment serving ANOTHER model refuses nothing.
//!
//! The models the rows delete span how a custom model comes into being — the two data
//! sources `CreateCustomModel` admits, and the output model a completed
//! `CreateModelCustomizationJob` brings into being, which carries a base-model segment in
//! its ARN and a `jobArn` where an imported model carries neither — and the `modelStatus`
//! it holds when the delete arrives (`Creating`, and `Active` once the create's span has
//! passed).

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values this service and the model fix for these rows.
// ---------------------------------------------------------------------------------------

/// The one sentence a request addressing a custom model ARN this account does not hold is
/// refused with, as the service states it.
const NOT_FOUND: &str = bedrock::state::CUSTOM_MODEL_NOT_FOUND_MESSAGE;

/// The one sentence Amazon Bedrock states for a model identifier it cannot resolve to a
/// model at all, as the service states it. Fixed text: it quotes no part of the request.
const INVALID_IDENTIFIER: &str = bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE;

/// The base-model segment a `CustomModelArn` carries for a model created from artifacts.
const IMPORTED_BASE: &str = bedrock::state::CUSTOM_MODEL_IMPORTED_BASE;

/// How long after a create a custom model's `Creating` status is owed its move to `Active`.
const ACTIVE_AFTER: Duration = bedrock::state::CUSTOM_MODEL_ACTIVE_AFTER;

/// How long after its create a model customization job is owed its completion — the moment
/// the custom model it customizes comes into being.
const COMPLETE_AFTER: Duration = bedrock::state::MODEL_CUSTOMIZATION_JOB_COMPLETE_AFTER;

/// `ModelStatus` is `[Active, Creating, Failed]`
/// (`smithy_query(query="shape",shape="ModelStatus")`).
const CREATING: &str = "Creating";
const ACTIVE: &str = "Active";

/// A `RoleArn`: `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/BedrockCustomModelImport";

/// `CustomizationType` the base models of the published catalog are offered under, as the
/// catalog spells it (`bedrock::state::FoundationModel::customizations_supported`).
const FINE_TUNING: &str = "FINE_TUNING";

/// The first base model the account's published catalog offers for `FINE_TUNING`, so a row
/// follows the catalog rather than a model id it may add or drop.
fn fine_tuning_base_model() -> &'static str {
    bedrock::state::FOUNDATION_MODELS
        .iter()
        .find(|model| model.customizations_supported.contains(&FINE_TUNING))
        .map(|model| model.model_id)
        .expect("the published catalog must offer a base model for FINE_TUNING")
}

// ---------------------------------------------------------------------------------------
// Requests.
// ---------------------------------------------------------------------------------------

/// A `CreateCustomModel` body naming the artifacts in Amazon S3, the form that also needs a
/// `roleArn`.
fn s3_body(name: &str) -> Value {
    json!({
        "modelName": name,
        "modelSourceConfig": { "s3DataSource": { "s3Uri": "s3://bedrock-model-artifacts/exports/" } },
        "roleArn": ROLE_ARN,
    })
}

/// A `CreateCustomModel` body naming the artifacts by model package ARN, which needs no
/// role.
fn package_body(name: &str) -> Value {
    json!({
        "modelName": name,
        "customModelDataSource": {
            "modelPackageArnDataSource": {
                "modelPackageArn":
                    "arn:aws:sagemaker:us-west-2:000000000000:model-package/nova-export-1",
            },
        },
    })
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        "/custom-models/create-custom-model",
        Some(body),
    )
    .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call("GET", &format!("/custom-models/{}", label(identifier)), None)
        .await
}

async fn delete(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/custom-models/{}", label(identifier)),
        None,
    )
    .await
}

async fn list(emu: &Emulator) -> Reply {
    emu.call("GET", "/custom-models", None).await
}

/// Create one custom model from artifacts and hand back its ARN, failing the case by name
/// when the create is refused.
async fn mint(emu: &Emulator, case: &str, body: Value) -> String {
    let reply = create_in_region(emu, REGION, body).await;
    assert_eq!(
        reply.status, 202,
        "case {case:?}: CreateCustomModel must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("modelArn")
}

/// Submit a model customization job over a published base model and hand back the
/// `jobArn` it minted.
async fn submit_customization(emu: &Emulator, case: &str, job_name: &str, model_name: &str) -> String {
    let reply = emu
        .call(
            "POST",
            "/model-customization-jobs",
            Some(json!({
                "jobName": job_name,
                "customModelName": model_name,
                "roleArn": ROLE_ARN,
                "baseModelIdentifier": fine_tuning_base_model(),
                "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
                "outputDataConfig": { "s3Uri": "s3://customization-output/" },
            })),
        )
        .await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: CreateModelCustomizationJob must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("jobArn")
}

/// Create a custom model deployment over `model_arn` and hand back its ARN.
async fn deploy(emu: &Emulator, case: &str, name: &str, model_arn: &str) -> String {
    let reply = emu
        .call(
            "POST",
            "/model-customization/custom-model-deployments",
            Some(json!({ "modelDeploymentName": name, "modelArn": model_arn })),
        )
        .await;
    assert_eq!(
        reply.status, 202,
        "case {case:?}: CreateCustomModelDeployment must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("customModelDeploymentArn")
}

// ---------------------------------------------------------------------------------------
// The store the handlers share: aging a submitted job instead of sleeping through its run.
// ---------------------------------------------------------------------------------------

// The shift of a submitted job's instants is `crate::support`'s — the ONE copy this crate's
// suites share, so the model a COMPLETED job produces is reached without sleeping through
// the whole run.
use crate::support::age_model_customization_job;

// ---------------------------------------------------------------------------------------
// Response helpers.
// ---------------------------------------------------------------------------------------

/// The HTTP status and the wire error code a reply carries, so a row pins both at once.
type Verdict = (u16, Option<String>);

/// The [`Verdict`] a reply carries.
fn verdict(reply: &Reply) -> Verdict {
    (reply.status, reply.error.clone())
}

/// The ARNs a listing reported, in the order it reported them.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("modelSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: modelSummaries must be a present array, got {:?}",
                reply.body
            )
        })
        .iter()
        .map(|row| {
            row.get("modelArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary must carry modelArn: {row:?}"))
                .to_string()
        })
        .collect()
}

/// The refusal an identifier that resolves to no custom model draws, by the spelling it is:
/// the missing-ARN report for an ARN, and the unresolvable-identifier report for a bare
/// name.
fn unresolved(spelling: Spelling) -> (Verdict, &'static str) {
    match spelling {
        Spelling::Arn => (
            (404, Some("ResourceNotFoundException".to_string())),
            NOT_FOUND,
        ),
        Spelling::Name => (
            (400, Some("ValidationException".to_string())),
            INVALID_IDENTIFIER,
        ),
    }
}

/// Assert that `reply` is the refusal an unresolvable identifier of `spelling` draws, status,
/// error code and sentence together.
fn assert_unresolved(case: &str, spelling: Spelling, reply: &Reply) {
    let (expected, message) = unresolved(spelling);
    assert_eq!(
        verdict(reply),
        expected,
        "case {case:?}: expected {expected:?} for a {spelling:?} that resolves to no model, \
         got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.get("message"),
        json!(message),
        "case {case:?}: expected the message {message:?}, got {:?}",
        reply.body
    );
}

// ---------------------------------------------------------------------------------------
// How a row's model comes into being, and which spelling the delete addresses it by.
// ---------------------------------------------------------------------------------------

/// Where the custom model a row deletes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provenance {
    /// `CreateCustomModel` over artifacts in Amazon S3.
    ImportedFromS3,
    /// `CreateCustomModel` over a SageMaker AI model package.
    ImportedFromPackage,
    /// The output model a COMPLETED `CreateModelCustomizationJob` brought into being.
    Customized,
}

/// Which of the two spellings `ModelIdentifier` admits a row deletes the model by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    Arn,
    Name,
}

/// Bring a row's model into being and hand back its ARN. Deleting it is what each row then
/// asserts.
async fn given_model(emu: &Emulator, case: &str, provenance: Provenance, name: &str) -> String {
    match provenance {
        Provenance::ImportedFromS3 => mint(emu, case, s3_body(name)).await,
        Provenance::ImportedFromPackage => mint(emu, case, package_body(name)).await,
        Provenance::Customized => {
            let job = submit_customization(emu, case, &format!("job-{name}"), name).await;
            // The completion the clock owes the job is what registers the model it
            // customized, so the job is aged past its due instant rather than slept
            // through.
            age_model_customization_job(emu, &job, COMPLETE_AFTER + Duration::from_millis(50));
            let read = get(emu, name).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: a completed job's output model must be readable, got {} {:?}",
                read.status, read.body
            );
            read.text("modelArn")
        }
    }
}

// ---------------------------------------------------------------------------------------
// 1 and 2. A delete removes the model its identifier names, and an identifier naming no
//          model — including one already deleted — is refused 404.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_removes_the_model_whichever_spelling_names_it() {
    let emu = emulator();

    // The model every row must leave untouched: a delete removes the model it names and no
    // other.
    let bystander = mint(&emu, "the bystander", s3_body("cm-bystander")).await;

    // (case, provenance, the model's name, the spelling the delete addresses it by, whether
    // the row waits for the model to settle to `Active` first)
    let rows: Vec<(&str, Provenance, &str, Spelling, bool)> = vec![
        (
            "an imported model from Amazon S3, deleted by its ARN while Creating",
            Provenance::ImportedFromS3,
            "cm-del-s3-arn",
            Spelling::Arn,
            false,
        ),
        (
            "an imported model from a model package, deleted by its name while Creating",
            Provenance::ImportedFromPackage,
            "cm_del_pkg_name",
            Spelling::Name,
            false,
        ),
        (
            "an imported model deleted once it has settled to Active",
            Provenance::ImportedFromS3,
            "c",
            Spelling::Arn,
            true,
        ),
        (
            "a customized model deleted by its ARN",
            Provenance::Customized,
            "cm-del-customized-arn",
            Spelling::Arn,
            false,
        ),
        (
            "a customized model deleted by its name",
            Provenance::Customized,
            "cm-del-customized-name",
            Spelling::Name,
            false,
        ),
    ];

    for (case, provenance, name, spelling, settled) in rows {
        let arn = given_model(&emu, case, provenance, name).await;
        if settled {
            // The transition is owed by the CLOCK, so the row waits for it rather than
            // asking for it.
            tokio::time::sleep(ACTIVE_AFTER + Duration::from_millis(150)).await;
        }

        let before = get(&emu, &arn).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the model must be readable before the delete, got {} {:?}",
            before.status, before.body
        );
        let expected_status = if settled || provenance == Provenance::Customized {
            ACTIVE
        } else {
            CREATING
        };
        assert_eq!(
            before.get("modelStatus"),
            json!(expected_status),
            "case {case:?}: expected the model to be {expected_status} when the delete \
             arrives, got {:?}",
            before.body
        );
        assert!(
            listed_arns(case, &list(&emu).await).contains(&arn),
            "case {case:?}: the listing must report {arn} before the delete"
        );

        let identifier = match spelling {
            Spelling::Arn => arn.clone(),
            Spelling::Name => name.to_string(),
        };
        let deleted = delete(&emu, &identifier).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: a delete of {identifier} must answer 200, got {} {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.get("message").is_null(),
            "case {case:?}: DeleteCustomModel's output has no members, got {:?}",
            deleted.body
        );

        // The model is gone from every way of reaching it, and each way is refused as the
        // spelling it carries: the ARN reports the missing ARN, the bare NAME reports an
        // identifier that resolves to nothing.
        assert_unresolved(case, Spelling::Arn, &get(&emu, &arn).await);
        assert_unresolved(case, Spelling::Name, &get(&emu, name).await);
        let listed = listed_arns(case, &list(&emu).await);
        assert!(
            !listed.contains(&arn),
            "case {case:?}: the listing must not report the deleted {arn}, it reported \
             {listed:?}"
        );
        assert!(
            listed.contains(&bystander),
            "case {case:?}: the delete must leave {bystander} alone, the listing reported \
             {listed:?}"
        );

        // A second delete finds nothing left — which is what makes two deletes racing for
        // one model answer exactly one success.
        assert_unresolved(case, spelling, &delete(&emu, &identifier).await);
        assert_unresolved(case, Spelling::Arn, &delete(&emu, &arn).await);
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_custom_model_is_refused_as_the_spelling_it_is() {
    let emu = emulator();

    let live = "cm-present";
    let live_arn = mint(&emu, "the live model", s3_body(live)).await;
    // A model of ANOTHER region: its NAME resolves to nothing in the signing region, the
    // way the listing reports only the signing region's models.
    let elsewhere = create_in_region(&emu, "eu-central-1", s3_body("cm-elsewhere")).await;
    assert_eq!(
        elsewhere.status, 202,
        "the other region's create must be served, got {} {:?}",
        elsewhere.status, elsewhere.body
    );

    // `modelName` is `@length(min: 1, max: 63)`, so the name rows span both bounds beside
    // the everyday spellings.
    let longest_name = "a".repeat(63);
    // (case, the identifier the delete carries, the spelling it is)
    let unknown: Vec<(&str, String, Spelling)> = vec![
        (
            "a name no create ever used",
            "cm-absent".to_string(),
            Spelling::Name,
        ),
        (
            "a prefix of a live name",
            "cm-presen".to_string(),
            Spelling::Name,
        ),
        (
            "the name of a model that lives only in another region",
            "cm-elsewhere".to_string(),
            Spelling::Name,
        ),
        (
            "a shortest-possible name that names nothing",
            "z".to_string(),
            Spelling::Name,
        ),
        (
            "a longest-possible name that names nothing",
            longest_name,
            Spelling::Name,
        ),
        (
            // The member's pattern admits a bare `provider.model` id too, and no such id
            // names a CUSTOM model: it is an identifier this operation cannot resolve.
            "a bare foundation-model id",
            fine_tuning_base_model().to_string(),
            Spelling::Name,
        ),
        (
            "an in-region ARN of this account that names nothing",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{IMPORTED_BASE}/aaaaaaaaaaaa"),
            Spelling::Arn,
        ),
        (
            "an in-region ARN of another account",
            format!(
                "arn:aws:bedrock:{REGION}:123456789012:custom-model/{IMPORTED_BASE}/aaaaaaaaaaaa"
            ),
            Spelling::Arn,
        ),
        (
            "an in-region ARN whose base-model segment names a foundation model",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{}/aaaaaaaaaaaa",
                fine_tuning_base_model()
            ),
            Spelling::Arn,
        ),
    ];
    for (case, identifier, spelling) in unknown {
        assert_unresolved(case, spelling, &delete(&emu, &identifier).await);
        // A refused delete removes nothing.
        assert_eq!(
            listed_arns(case, &list(&emu).await),
            vec![live_arn.clone()],
            "case {case:?}: a refused delete must leave this region's models alone"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. A delete frees the model's name for a later create.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_models_name_is_free_for_a_later_create() {
    let emu = emulator();

    // `modelName` is `@length(min: 1, max: 63)`, so the rows span both bounds beside an
    // everyday name.
    let longest = "a".repeat(63);
    let names: Vec<(&str, &str)> = vec![
        ("a shortest-possible name", "c"),
        ("an everyday name", "cm_reused_name"),
        ("a longest-possible name", longest.as_str()),
    ];
    for (case, name) in names {
        let first = mint(&emu, case, s3_body(name)).await;
        // While the model is live the name names it, so a second create of that name is a
        // conflict.
        let conflicted = create_in_region(&emu, REGION, package_body(name)).await;
        assert_eq!(
            verdict(&conflicted),
            (400, Some("ConflictException".to_string())),
            "case {case:?}: a create of a live name must conflict, got {} {:?}",
            conflicted.status,
            conflicted.body
        );

        assert_eq!(
            delete(&emu, name).await.status,
            200,
            "case {case:?}: the delete of {name} must answer 200"
        );

        let second = mint(&emu, case, package_body(name)).await;
        assert_ne!(
            second, first,
            "case {case:?}: the create after the delete must mint a NEW model, it answered \
             the deleted {first}"
        );
        let read = get(&emu, name).await;
        assert_eq!(
            read.get("modelArn"),
            json!(second),
            "case {case:?}: the name must now read back the model the second create minted, \
             got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A model a deployment is still serving is not removed.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_model_a_deployment_serves_is_refused_as_a_conflict() {
    let emu = emulator();

    let served = mint(&emu, "the served model", s3_body("cm-served")).await;
    let other = mint(&emu, "the unserved model", s3_body("cm-unserved")).await;
    let deployment = deploy(&emu, "the deployment", "cm-served-deployment", &served).await;

    // (case, the identifier the delete carries, the verdict it draws)
    let rows: Vec<(&str, String, Verdict)> = vec![
        (
            "the served model by its ARN",
            served.clone(),
            (400, Some("ConflictException".to_string())),
        ),
        (
            "the served model by its name",
            "cm-served".to_string(),
            (400, Some("ConflictException".to_string())),
        ),
        (
            "a model no deployment serves",
            other.clone(),
            (200, None),
        ),
    ];
    for (case, identifier, expected) in rows {
        let reply = delete(&emu, &identifier).await;
        assert_eq!(
            verdict(&reply),
            expected,
            "case {case:?}: expected {expected:?}, got {} {:?}",
            reply.status,
            reply.body
        );
        if expected.0 == 400 {
            let message = reply.get("message").as_str().unwrap_or_default().to_string();
            // The refusal names the model it would have removed and the deployment holding
            // the association, both read out of the store.
            assert!(
                message.contains(&served) && message.contains(&deployment),
                "case {case:?}: the refusal must name {served} and {deployment}, got \
                 {message:?}"
            );
        }
    }

    // The refused delete performed nothing: the served model is still there, and the one
    // nothing served is gone.
    let read = get(&emu, &served).await;
    assert_eq!(
        read.status, 200,
        "case \"after the conflict\": the served model must still be readable, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        listed_arns("after the conflict", &list(&emu).await),
        vec![served.clone()],
        "case \"after the conflict\": only the served model is left"
    );
}
