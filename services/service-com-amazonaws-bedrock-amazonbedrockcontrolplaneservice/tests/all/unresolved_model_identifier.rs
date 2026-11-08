// SPDX-License-Identifier: CC0-1.0

//! What every member that IDENTIFIES a model answers when the value it carries resolves to
//! no model, driven end to end over the same `MultiServiceRouter` the emulator boots
//! (`crate::support`).
//!
//! # The rule
//!
//! A model-identifying member admits several spellings of one model — the ARN of a resource
//! this service holds, a bare foundation model id, a bare model name — and an unresolvable
//! value is NOT the same failure in each. Which it is comes off the SPELLING the caller
//! used, never off what the store happens to hold:
//!
//! * an ARN of one of the resource types the operation resolves that member against names a
//!   resource to report missing: 404 `ResourceNotFoundException` ("The specified resource
//!   Amazon Resource Name (ARN) was not found"), carrying that resource type's own
//!   not-found sentence;
//! * every other spelling — a bare model id, a bare model name, an ARN of a resource type
//!   that operation does not resolve the member against — is a value this service cannot
//!   read as the model the operation addresses, which is an INPUT fault: 400
//!   `ValidationException` carrying the one FIXED sentence Amazon Bedrock states for that
//!   class ([`INVALID_MODEL_IDENTIFIER`]).
//!
//! Neither sentence quotes the caller's value except where the resource it reports absent
//! IS that ARN, and none of them is emulator prose about the account or the region.
//!
//! `CreateModelCustomizationJob` adds one reading in front of its own resolution: a
//! customization runs on a TRAINING VARIANT of a published model, which Amazon Bedrock
//! publishes under the model id with its version qualifiers, so a bare
//! `<provider>.<model>` id carrying no qualifier identifies no variant to customize and is
//! refused with the same fixed sentence — before the `iam:PassRole` step, which is what the
//! corpus records for it.
//!
//! # What the table generalises over
//!
//! One row per independent input along the axes the rule spans: which operation carries the
//! member (a URI label and a body member; a read, a delete and three creates), which family
//! that operation resolves it against (the published catalogue, this account's custom
//! models, its imported models, its inference profiles), which spelling the value takes
//! (every ARN resource type the member's own `@pattern` admits, a bare model id, a bare
//! model name), and whether the spelled resource type is one that operation resolves at all.
//! Every expectation comes from the model
//! (`smithy_query(query="operation",operation="GetCustomModel,DeleteCustomModel,GetImportedModel,CreateProvisionedModelThroughput,CreateModelInvocationJob,CreateModelCustomizationJob")`)
//! and from the sentences `crate::state` states for each class.

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{emulator, foundation_model_arn, label, Reply, ACCOUNT, REGION};

/// The one sentence Amazon Bedrock states for a value it cannot read as the model an
/// operation addresses — written out here rather than read off the crate, so this suite pins
/// the wording a client branches on.
const INVALID_MODEL_IDENTIFIER: &str = "The provided model identifier is invalid.";

/// The not-found sentence of a `custom-model`.
const CUSTOM_MODEL_NOT_FOUND: &str = "Custom model not found.";

/// The not-found sentence of the other resource types these rows name, as the type spelled
/// in words opening the sentence.
const IMPORTED_MODEL_NOT_FOUND: &str = "Imported model not found";
const INFERENCE_PROFILE_NOT_FOUND: &str = "Application inference profile not found";

/// A base model the published catalogue carries whose id is version-qualified: the spelling
/// `CreateModelCustomizationJob` identifies a customizable variant by.
fn qualified_catalogue_model() -> &'static str {
    bedrock::state::FOUNDATION_MODELS
        .iter()
        .find(|model| model.model_id.contains(':'))
        .map(|model| model.model_id)
        .expect("the catalogue must publish a version-qualified model id")
}

/// A base model the published catalogue carries whose id carries NO version qualifier: the
/// on-demand spelling of a published model.
fn unqualified_catalogue_model() -> &'static str {
    bedrock::state::FOUNDATION_MODELS
        .iter()
        .find(|model| !model.model_id.contains(':'))
        .map(|model| model.model_id)
        .expect("the catalogue must publish an unqualified model id")
}

/// A `custom-model` ARN of this account naming a model nothing created. The resource is
/// `custom-model/<base model>/<id>`, as `CustomModelArn` spells it.
fn missing_custom_model_arn(base_model: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{base_model}/aaaaaaaaaaaa")
}

/// An `imported-model` ARN of this account naming a model no import created.
fn missing_imported_model_arn() -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:imported-model/bbbbbbbbbbbb")
}

/// An `application-inference-profile` ARN of this account naming a profile nothing created.
fn missing_inference_profile_arn() -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:application-inference-profile/cccccccccccc")
}

// ---------------------------------------------------------------------------------------
// The requests each surface makes of one identifier.
// ---------------------------------------------------------------------------------------

/// One request addressing `identifier`: the method, the path and the body a surface carries
/// it in.
struct Request {
    method: &'static str,
    path: String,
    body: Option<Value>,
}

fn get_custom_model(identifier: &str) -> Request {
    Request {
        method: "GET",
        path: format!("/custom-models/{}", label(identifier)),
        body: None,
    }
}

fn delete_custom_model(identifier: &str) -> Request {
    Request {
        method: "DELETE",
        path: format!("/custom-models/{}", label(identifier)),
        body: None,
    }
}

fn get_imported_model(identifier: &str) -> Request {
    Request {
        method: "GET",
        path: format!("/imported-models/{}", label(identifier)),
        body: None,
    }
}

fn buy_provisioned_throughput(identifier: &str) -> Request {
    Request {
        method: "POST",
        path: "/provisioned-model-throughput".to_string(),
        body: Some(json!({
            "provisionedModelName": "pt-unresolved",
            "modelUnits": 1,
            "modelId": identifier,
            "commitmentDuration": "OneMonth",
        })),
    }
}

fn submit_batch_job(identifier: &str) -> Request {
    Request {
        method: "POST",
        path: "/model-invocation-job".to_string(),
        body: Some(json!({
            "jobName": "batch-unresolved",
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/bedrock-batch"),
            "modelId": identifier,
            "inputDataConfig": {
                "s3InputDataConfig": { "s3Uri": "s3://batch-in/prompts.jsonl" }
            },
            "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": "s3://batch-out/" } },
        })),
    }
}

fn start_customization_job(identifier: &str) -> Request {
    Request {
        method: "POST",
        path: "/model-customization-jobs".to_string(),
        body: Some(json!({
            "jobName": "customization-unresolved",
            "customModelName": "CustomizationUnresolved",
            "baseModelIdentifier": identifier,
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
            "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
            "outputDataConfig": { "s3Uri": "s3://customization-output/" },
        })),
    }
}

/// One row: the surface, the value it carries, and the refusal the rule demands.
struct Case {
    case: &'static str,
    request: fn(&str) -> Request,
    identifier: String,
    status: u16,
    code: &'static str,
    message: String,
}

fn cases() -> Vec<Case> {
    let qualified = qualified_catalogue_model();
    let unqualified = unqualified_catalogue_model();
    let missing_foundation_model = foundation_model_arn(REGION, "acme.not-a-model-v1");
    vec![
        // ---- the custom model family: it resolves `custom-model` ARNs alone -------------
        Case {
            case: "GetCustomModel by a name nothing holds",
            request: get_custom_model,
            identifier: "NoSuchCustomModel".to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "GetCustomModel by a custom-model ARN nothing minted",
            request: get_custom_model,
            identifier: missing_custom_model_arn(qualified),
            status: 404,
            code: "ResourceNotFoundException",
            message: CUSTOM_MODEL_NOT_FOUND.to_string(),
        },
        Case {
            // `ModelIdentifier` admits a `foundation-model` ARN too, but this operation
            // resolves the member against the CUSTOM models alone, so such a value names no
            // resource it could report missing.
            case: "GetCustomModel by a foundation-model ARN",
            request: get_custom_model,
            identifier: foundation_model_arn(REGION, qualified),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "DeleteCustomModel by a name nothing holds",
            request: delete_custom_model,
            identifier: "NoSuchCustomModel".to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "DeleteCustomModel by a custom-model ARN nothing minted",
            request: delete_custom_model,
            identifier: missing_custom_model_arn(qualified),
            status: 404,
            code: "ResourceNotFoundException",
            message: CUSTOM_MODEL_NOT_FOUND.to_string(),
        },
        // ---- the imported model family: it resolves `imported-model` ARNs ---------------
        Case {
            case: "GetImportedModel by a name no import created",
            request: get_imported_model,
            identifier: "NoSuchImportedModel".to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "GetImportedModel by an imported-model ARN nothing minted",
            request: get_imported_model,
            identifier: missing_imported_model_arn(),
            status: 404,
            code: "ResourceNotFoundException",
            message: IMPORTED_MODEL_NOT_FOUND.to_string(),
        },
        // ---- a purchase: it resolves the catalogue and this account's custom models -----
        Case {
            case: "CreateProvisionedModelThroughput by a bare id no row carries",
            request: buy_provisioned_throughput,
            identifier: "acme.not-a-model-v1".to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "CreateProvisionedModelThroughput by a foundation-model ARN no row carries",
            request: buy_provisioned_throughput,
            identifier: missing_foundation_model.clone(),
            status: 404,
            code: "ResourceNotFoundException",
            message: bedrock::state::foundation_model_not_found(&missing_foundation_model),
        },
        Case {
            case: "CreateProvisionedModelThroughput by a custom-model ARN nothing minted",
            request: buy_provisioned_throughput,
            identifier: missing_custom_model_arn(qualified),
            status: 404,
            code: "ResourceNotFoundException",
            message: CUSTOM_MODEL_NOT_FOUND.to_string(),
        },
        // ---- a batch submission: it resolves those two and the inference profiles -------
        Case {
            case: "CreateModelInvocationJob by a bare id no row carries",
            request: submit_batch_job,
            identifier: "acme.not-a-model-v1".to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "CreateModelInvocationJob by a foundation-model ARN no row carries",
            request: submit_batch_job,
            identifier: missing_foundation_model.clone(),
            status: 404,
            code: "ResourceNotFoundException",
            message: bedrock::state::foundation_model_not_found(&missing_foundation_model),
        },
        Case {
            case: "CreateModelInvocationJob by an inference-profile ARN nothing minted",
            request: submit_batch_job,
            identifier: missing_inference_profile_arn(),
            status: 404,
            code: "ResourceNotFoundException",
            message: INFERENCE_PROFILE_NOT_FOUND.to_string(),
        },
        // ---- a customization: the FORM of the identifier is read first ------------------
        Case {
            case: "CreateModelCustomizationJob by an unqualified published model id",
            request: start_customization_job,
            identifier: unqualified.to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "CreateModelCustomizationJob by a bare id with no qualifier at all",
            request: start_customization_job,
            identifier: "acme.not-a-model-v1".to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "CreateModelCustomizationJob by a qualified id no row carries",
            request: start_customization_job,
            identifier: "acme.not-a-model-v1:0:4k".to_string(),
            status: 400,
            code: "ValidationException",
            message: INVALID_MODEL_IDENTIFIER.to_string(),
        },
        Case {
            case: "CreateModelCustomizationJob by a custom-model ARN nothing minted",
            request: start_customization_job,
            identifier: missing_custom_model_arn(qualified),
            status: 404,
            code: "ResourceNotFoundException",
            message: CUSTOM_MODEL_NOT_FOUND.to_string(),
        },
    ]
}

/// The `message` a refusal carries, or the empty string when it carries none.
fn message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn an_identifier_that_resolves_to_no_model_is_refused_as_the_spelling_it_is() {
    let emu = emulator();
    for case in cases() {
        let request = (case.request)(&case.identifier);
        let reply = emu
            .call(request.method, &request.path, request.body.clone())
            .await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (case.status, Some(case.code)),
            "case {:?} ({}): expected {} {}, got {} {:?}",
            case.case,
            case.identifier,
            case.status,
            case.code,
            reply.status,
            reply.body
        );
        assert_eq!(
            message(&reply),
            case.message,
            "case {:?} ({}): expected the sentence {:?}, got {:?}",
            case.case,
            case.identifier,
            case.message,
            message(&reply)
        );
    }
}

/// No refusal of this class carries emulator prose about the account or the region: the
/// sentence states the CLASS of the failure, and the only value any of them carries is the
/// ARN a missing-resource reading reports absent.
#[tokio::test]
async fn no_refusal_of_this_class_reports_the_account_or_the_region() {
    let emu = emulator();
    for case in cases() {
        let request = (case.request)(&case.identifier);
        let reply = emu
            .call(request.method, &request.path, request.body.clone())
            .await;
        let stated = message(&reply);
        for forbidden in ["in this account", "answers to", "does not name"] {
            assert!(
                !stated.contains(forbidden),
                "case {:?}: the refusal must not carry {forbidden:?}, got {stated:?}",
                case.case
            );
        }
    }
}

/// The complement of the table: what the published catalogue OFFERS for customization, the
/// customization create accepts. A caller reads
/// `ListFoundationModels(byCustomizationType=FINE_TUNING)` and submits the `modelId` it was
/// handed — every one of them, not a chosen row — and none of those requests may be
/// answered with the invalid-model-identifier sentence, because the id the catalogue
/// publishes the customization on IS the identifier a customization job names.
#[tokio::test]
async fn every_model_the_catalogue_offers_for_customization_is_accepted_by_the_create() {
    let emu = emulator();
    let listed = emu
        .call("GET", "/foundation-models?byCustomizationType=FINE_TUNING", None)
        .await;
    assert_eq!(
        listed.status, 200,
        "setup: the listing must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    let models = listed
        .get("modelSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !models.is_empty(),
        "setup: the catalogue must publish a model offered for FINE_TUNING, got {:?}",
        listed.body
    );

    for (index, model) in models.iter().enumerate() {
        let model_id = model
            .get("modelId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let body = json!({
            "jobName": format!("offered-{index}"),
            "customModelName": format!("Offered{index}"),
            "baseModelIdentifier": model_id,
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/bedrock-customization"),
            "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
            "outputDataConfig": { "s3Uri": "s3://customization-output/" },
        });
        let reply = emu.call("POST", "/model-customization-jobs", Some(body)).await;
        assert_eq!(
            reply.status, 201,
            "case {model_id:?}: the catalogue offers this model for FINE_TUNING, so the \
             create must accept the identifier it published, got {} {:?}",
            reply.status, reply.body
        );
        assert_ne!(
            message(&reply),
            INVALID_MODEL_IDENTIFIER,
            "case {model_id:?}: an identifier the catalogue published for customization is \
             never unreadable, got {:?}",
            reply.body
        );
    }
}
#[tokio::test]
async fn a_refused_identifier_leaves_nothing_behind() {
    let emu = emulator();
    for case in cases() {
        let request = (case.request)(&case.identifier);
        let _ = emu
            .call(request.method, &request.path, request.body.clone())
            .await;
    }

    for (what, path, member) in [
        (
            "the Provisioned Throughputs",
            "/provisioned-model-throughputs?nameContains=pt-unresolved",
            "provisionedModelSummaries",
        ),
        (
            "the batch inference jobs",
            "/model-invocation-jobs?nameContains=batch-unresolved",
            "invocationJobSummaries",
        ),
        (
            "the model customization jobs",
            "/model-customization-jobs?nameContains=customization-unresolved",
            "modelCustomizationJobSummaries",
        ),
        (
            "the custom models",
            "/custom-models?nameContains=CustomizationUnresolved",
            "modelSummaries",
        ),
    ] {
        let listed = emu.call("GET", path, None).await;
        assert_eq!(
            listed.status, 200,
            "case {what}: the listing must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let rows = listed
            .get(member)
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            rows, 0,
            "case {what}: a refused identifier mints nothing, got {:?}",
            listed.body
        );
    }
}
