// SPDX-License-Identifier: CC0-1.0

//! This service's own check of a caller-supplied ARN, driven end to end over HTTP
//! through the same `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! For SOME of its resources, Amazon Bedrock reads an ARN a caller submits ITSELF, ahead
//! of the constraints the model declares for the member carrying it, and answers 400
//! `ValidationException` with one of its own two short sentences — never the framework's
//! `N validation error(s) detected: Value '…' at '…' failed to satisfy constraint: Member
//! must satisfy regular expression pattern: …` envelope:
//!
//! * an ARN of one of those resources naming a REGION other than the one the request
//!   addresses is [`ARN_INVALID_FOR_SERVICE_REGION`];
//! * an in-region ARN of one of them whose resource is not a well-formed one of the type
//!   the ARN itself names is `Invalid <resource type> ARN format`.
//!
//! Both sentences are recorded behaviour (`simparity-wire-aws-v1`), not modelled rules:
//! the model declares no per-region ARN check at all, only the anchored `@pattern` each
//! ARN-valued member carries, and nothing in it marks which resources bedrock checks
//! itself. WHICH resources those are is recorded too, in both directions: the same probe
//! generator's ARN, from the same foreign region, is answered with the service's own
//! sentence for a `custom-model`, `model-copy-job`, `model-invocation-job`,
//! `provisioned-model`, `model-customization-job` or `model-import-job`, and with the
//! framework's constraint envelope for an `advanced-prompt-optimization-job` or an
//! `evaluation-job`.
//! `crate::state::SELF_VALIDATED_RESOURCE_TYPES` is that catalog; the rows below drive
//! both sides of it.
//!
//! # What the table below generalises over
//!
//! One row per independent input along the axes the rule spans — which operation and
//! member carries the ARN (a URI label and a body member, members the recorded causes
//! name and members they do not), which resource type the ARN names (every type in the
//! catalog, and types outside it), whether the ARN is well formed for that type, which
//! region each side names (including the request signed FOR the ARN's region, so the
//! region compared is the request's and not a constant), which partition the ARN
//! carries, and the values the rule does NOT reach: a non-ARN identifier, another
//! service's ARN, and the one member documented to name another region on purpose.

use serde_json::{json, Value};

use crate::support::{emulator, label, Reply, ACCOUNT, REGION};

/// The region every request in this suite addresses unless its row says otherwise — the
/// region the ARNs are compared against. The shared harness's own
/// [`REGION`](crate::support::REGION), so a row that addresses "this region" and a row
/// that addresses another cannot drift apart.
const HOME: &str = REGION;

/// What this service answers for an ARN of a resource it validates itself that names
/// another region, written out here rather than read off the crate so the recorded
/// sentence is pinned by this suite independently of the constant that produces it.
const ARN_INVALID_FOR_SERVICE_REGION: &str = "The provided ARN is invalid for the service region";

/// The marker of the framework's constraint envelope: the phrase a client branches on to
/// tell a constraint violation from a service's own sentence.
const CONSTRAINT_ENVELOPE_MARKER: &str = "validation error";

/// The `$.message` member of `reply`'s error envelope, or the empty string when it
/// carries none.
fn message(reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What one request must be answered with.
#[derive(Debug)]
enum Expect {
    /// 400 `ValidationException` whose `$.message` is EXACTLY this sentence — one of
    /// this service's own two ARN sentences.
    ServiceSentence(String),
    /// 400 `ValidationException` worded as the framework's constraint envelope: this
    /// service's ARN check has nothing to say about the value, so the model's own
    /// refusal stands.
    ConstraintEnvelope,
    /// Every ARN in the request is one this service accepts, so the request reaches the
    /// operation — which answers for the resource the ARN names (a 404 for one nothing
    /// created, a 501 while the operation is scaffolded) instead of refusing the input.
    Accepted,
}

/// One request and the answer the rule demands.
struct Case {
    case: &'static str,
    region: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    expect: Expect,
}

/// A row whose answer is the SENTENCE written out here. Each format sentence is spelled
/// in full rather than recomputed from the resource type, so a row pins the text a client
/// reads instead of restating the production transformation.
fn sentence(text: &str) -> Expect {
    Expect::ServiceSentence(text.to_string())
}

/// The region sentence.
fn region_sentence() -> Expect {
    Expect::ServiceSentence(ARN_INVALID_FOR_SERVICE_REGION.to_string())
}

/// A model-copy-job ARN. `id` is the resource id, which `ModelCopyJobArn` declares as
/// `[a-z0-9]{12}`.
fn copy_job_arn(region: &str, id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:model-copy-job/{id}")
}

/// A custom-model ARN. `CustomModelArn` declares the resource as
/// `custom-model/(imported|<vendor>.<model>)/[a-z0-9]{12}`.
fn custom_model_arn(region: &str, resource: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:custom-model/{resource}")
}

/// A provisioned-model ARN. `ProvisionedModelArn` declares the resource as
/// `provisioned-model/[a-z0-9]{12}`.
fn provisioned_model_arn(region: &str, id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:provisioned-model/{id}")
}

/// A model-invocation-job ARN. `ModelInvocationJobIdentifier` declares the resource as
/// `model-invocation-job/[a-z0-9]{12}`.
fn invocation_job_arn(region: &str, id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:model-invocation-job/{id}")
}

/// A model-customization-job ARN. `ModelCustomizationJobIdentifier` declares the resource
/// as `model-customization-job/<base model>/[a-z0-9]{12}`, the base model carrying the
/// `<provider>.<model>` form and up to two `:` qualifiers.
fn customization_job_arn(region: &str, resource: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:model-customization-job/{resource}")
}

/// A model-import-job ARN. `ModelImportJobIdentifier` declares the resource as
/// `model-import-job/[a-z0-9]{12}`.
fn import_job_arn(region: &str, id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:model-import-job/{id}")
}

/// Every ARN-bearing request this suite drives.
///
/// The ARN shapes and the resource forms each one declares come from the model
/// (`smithy_query(query="operation",operation="GetModelCopyJob,CreateModelCopyJob,\
/// CreateCustomModelDeployment,GetModelInvocationJob,StopModelInvocationJob,\
/// GetProvisionedModelThroughput,UpdateProvisionedModelThroughput,\
/// DeleteProvisionedModelThroughput")`).
fn cases() -> Vec<Case> {
    vec![
        // ---- an ARN naming another region: the region sentence ------------------
        Case {
            case: "a model-copy-job ARN from another region, malformed id (recorded)",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-copy-jobs/{}",
                label(&copy_job_arn("us-east-1", "nonexistent-job-00000"))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            // WELL FORMED for its member, so nothing the model declares refuses it:
            // the region check is this service's own, not the `@pattern`'s.
            case: "a well-formed model-copy-job ARN from another region",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-copy-jobs/{}",
                label(&copy_job_arn("eu-west-1", "abcdef123456"))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            // Another PARTITION as well as another region.
            case: "a model-copy-job ARN in another partition and region",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-copy-jobs/{}",
                label(&format!(
                    "arn:aws-cn:bedrock:cn-north-1:{ACCOUNT}:model-copy-job/abcdef123456"
                ))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            // The same ARN as the row above the previous one, with the request signed
            // FOR that ARN's region: the region compared is the request's own.
            case: "a well-formed model-copy-job ARN read from its own region",
            region: "eu-west-1",
            method: "GET",
            path: format!(
                "/model-copy-jobs/{}",
                label(&copy_job_arn("eu-west-1", "abcdef123456"))
            ),
            body: None,
            expect: Expect::Accepted,
        },
        Case {
            // A BODY member, not a URI label (recorded).
            case: "a custom-model ARN from another region in a request body",
            region: HOME,
            method: "POST",
            path: "/model-customization/custom-model-deployments".to_string(),
            body: Some(json!({
                "modelDeploymentName": "probe-deployment",
                "modelArn": custom_model_arn("us-east-1", "nonexistent-model-00000"),
            })),
            expect: region_sentence(),
        },
        Case {
            case: "a provisioned-model ARN from another region, read (recorded)",
            region: HOME,
            method: "GET",
            path: format!(
                "/provisioned-model-throughput/{}",
                label(&provisioned_model_arn("us-east-1", "nonexistent"))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            case: "a well-formed provisioned-model ARN from another region, updated",
            region: HOME,
            method: "PATCH",
            path: format!(
                "/provisioned-model-throughput/{}",
                label(&provisioned_model_arn("ap-southeast-2", "abcdef123456"))
            ),
            body: Some(json!({})),
            expect: region_sentence(),
        },
        Case {
            case: "a provisioned-model ARN from another region, deleted (recorded)",
            region: HOME,
            method: "DELETE",
            path: format!(
                "/provisioned-model-throughput/{}",
                label(&provisioned_model_arn("us-east-1", "nonexistent"))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            case: "a model-invocation-job ARN from another region, read (recorded)",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-invocation-job/{}",
                label(&invocation_job_arn(
                    "us-east-1",
                    "sim2real-probe-bedrock-nonexistent-inv-job-42cb3346"
                ))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            case: "a well-formed model-invocation-job ARN from another region, stopped",
            region: HOME,
            method: "POST",
            path: format!(
                "/model-invocation-job/{}/stop",
                label(&invocation_job_arn("eu-central-1", "abcdef123456"))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            // A member the recorded causes do not name, carrying a custom-model ARN: the
            // check is keyed on the ARN's resource type, so it reads this one alike.
            case: "a custom-model ARN from another region on a read of the model itself",
            region: HOME,
            method: "GET",
            path: format!(
                "/custom-models/{}",
                label(&custom_model_arn("us-east-1", "anthropic.claude/abcdef123456"))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            case: "a model-customization-job ARN from another region, read (recorded)",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-customization-jobs/{}",
                label(&customization_job_arn(
                    "us-east-1",
                    "amazon.titan-text-lite-v1/abcdef012345"
                ))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            case: "a model-customization-job ARN from another region, stopped (recorded)",
            region: HOME,
            method: "POST",
            path: format!(
                "/model-customization-jobs/{}/stop",
                label(&customization_job_arn(
                    "eu-central-1",
                    "meta.llama3-70b-instruct-v1:0/abcdef012345"
                ))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            case: "a model-import-job ARN from another region, read (recorded)",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-import-jobs/{}",
                label(&import_job_arn("us-east-1", "abcdef012345"))
            ),
            body: None,
            expect: region_sentence(),
        },
        Case {
            // The same ARN read from its OWN region: the region compared is the request's.
            case: "a model-import-job ARN read from its own region",
            region: "us-east-1",
            method: "GET",
            path: format!(
                "/model-import-jobs/{}",
                label(&import_job_arn("us-east-1", "abcdef012345"))
            ),
            body: None,
            expect: Expect::Accepted,
        },
        // ---- an in-region ARN of the wrong form: the format sentence -------------
        Case {
            case: "an in-region custom-model ARN whose id is not one (recorded)",
            region: HOME,
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(json!({
                "sourceModelArn": custom_model_arn(HOME, "nonexistent-model-00000"),
                "targetModelName": "copied-model",
            })),
            expect: sentence("Invalid custom model ARN format"),
        },
        Case {
            case: "an in-region model-copy-job ARN whose id is not one",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-copy-jobs/{}",
                label(&copy_job_arn(HOME, "nonexistent-job-00000"))
            ),
            body: None,
            expect: sentence("Invalid model copy job ARN format"),
        },
        Case {
            case: "an in-region provisioned-model ARN whose id is not one",
            region: HOME,
            method: "GET",
            path: format!(
                "/provisioned-model-throughput/{}",
                label(&provisioned_model_arn(HOME, "nonexistent"))
            ),
            body: None,
            expect: sentence("Invalid provisioned model ARN format"),
        },
        Case {
            case: "an in-region model-invocation-job ARN whose id is not one",
            region: HOME,
            method: "POST",
            path: format!(
                "/model-invocation-job/{}/stop",
                label(&invocation_job_arn(HOME, "not-a-job-id"))
            ),
            body: None,
            expect: sentence("Invalid model invocation job ARN format"),
        },
        // ---- resource types bedrock does NOT check itself -----------------------
        Case {
            // RECORDED the other way round: an evaluation-job ARN from the same foreign
            // region is refused with the framework's constraint envelope
            // (`issues/…-bedrock-validation-error-messages-render-model-regex-with.md`,
            // cause 4), so `evaluation-job` is outside the catalog.
            case: "an evaluation-job ARN from another region",
            region: HOME,
            method: "GET",
            path: format!(
                "/evaluation-jobs/{}",
                label(&format!(
                    "arn:aws:bedrock:us-east-1:{ACCOUNT}:evaluation-job/nonexistent"
                ))
            ),
            body: None,
            expect: Expect::ConstraintEnvelope,
        },
        Case {
            // RECORDED the other way round too (same issue, cause 1).
            case: "an advanced-prompt-optimization-job ARN from another region",
            region: HOME,
            method: "GET",
            path: format!(
                "/advanced-prompt-optimization-jobs/{}",
                label(&format!(
                    "arn:aws:bedrock:us-east-1:{ACCOUNT}:advanced-prompt-optimization-job/\
                     sim2real-probe-bedrock-nonexistent-apo-job-681564ea"
                ))
            ),
            body: None,
            expect: Expect::ConstraintEnvelope,
        },
        Case {
            // No recording covers a foundation-model ARN, and the catalog holds only the
            // resources one does, so the model's own refusal stands for it.
            case: "an in-region foundation-model ARN whose model name is not one",
            region: HOME,
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(json!({
                "sourceModelArn": format!("arn:aws:bedrock:{HOME}::foundation-model/NOTVALID"),
                "targetModelName": "copied-model",
            })),
            expect: Expect::ConstraintEnvelope,
        },
        // ---- values this service's ARN check does not reach ----------------------
        Case {
            // `ProvisionedModelId` admits a bare name beside an ARN; a bare name
            // outside it is not an ARN at all, so the model's own refusal stands.
            case: "an identifier that is not an ARN",
            region: HOME,
            method: "GET",
            path: "/provisioned-model-throughput/not%20a%20name!!".to_string(),
            body: None,
            expect: Expect::ConstraintEnvelope,
        },
        Case {
            // Another service's ARN, from another region and outside its own declared
            // form: this check reads only ARNs in THIS service's namespace.
            case: "a KMS key ARN from another region, outside its declared form",
            region: HOME,
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(json!({
                "sourceModelArn": custom_model_arn(HOME, "anthropic.claude/abcdef123456"),
                "targetModelName": "copied-model",
                "modelKmsKeyId": format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/short"),
            })),
            expect: Expect::ConstraintEnvelope,
        },
        Case {
            // `CreateModelCopyJob` copies a model to the region the request addresses,
            // so its SOURCE model lives in another one by design. The ARN names a base
            // model the published catalog carries, so the handler refuses it for naming
            // no model of this account (404) rather than for its ARN.
            case: "the model-copy source ARN from another region, which is the point",
            region: HOME,
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(json!({
                "sourceModelArn": custom_model_arn(
                    "us-east-1",
                    "anthropic.claude-3-haiku-20240307-v1:0/abcdef123456",
                ),
                "targetModelName": "copied-model",
            })),
            expect: Expect::Accepted,
        },
        Case {
            case: "an in-region, well-formed ARN",
            region: HOME,
            method: "GET",
            path: format!(
                "/model-copy-jobs/{}",
                label(&copy_job_arn(HOME, "abcdef123456"))
            ),
            body: None,
            expect: Expect::Accepted,
        },
    ]
}

#[tokio::test]
async fn an_arn_input_is_refused_in_this_services_own_words() {
    let emulator = emulator();
    for case in cases() {
        let reply = emulator
            .call_in_region(case.region, case.method, &case.path, case.body.clone())
            .await;
        match &case.expect {
            Expect::ServiceSentence(sentence) => {
                assert_eq!(
                    (reply.status, reply.error.as_deref()),
                    (400, Some("ValidationException")),
                    "case {:?}: expected 400 ValidationException, got {} {:?}",
                    case.case,
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    &message(&reply),
                    sentence,
                    "case {:?}: expected the message {sentence:?}, got {:?}",
                    case.case,
                    message(&reply)
                );
            }
            Expect::ConstraintEnvelope => {
                assert_eq!(
                    (reply.status, reply.error.as_deref()),
                    (400, Some("ValidationException")),
                    "case {:?}: expected 400 ValidationException, got {} {:?}",
                    case.case,
                    reply.status,
                    reply.body
                );
                let message = message(&reply);
                assert!(
                    message.contains(CONSTRAINT_ENVELOPE_MARKER),
                    "case {:?}: expected the framework's constraint envelope \
                     (containing {CONSTRAINT_ENVELOPE_MARKER:?}), got {message:?}",
                    case.case
                );
            }
            Expect::Accepted => {
                assert_ne!(
                    reply.status, 400,
                    "case {:?}: expected the request to pass this service's ARN check and \
                     reach the operation, got 400 with {:?}",
                    case.case,
                    message(&reply)
                );
            }
        }
    }
}

/// No request this suite refuses reports the member or the model's expression: that is
/// the shape of a constraint violation, and it is what distinguishes the two envelopes a
/// client branches on.
#[tokio::test]
async fn an_arn_refusal_states_neither_the_member_nor_the_pattern() {
    let emulator = emulator();
    for case in cases() {
        let Expect::ServiceSentence(_) = case.expect else {
            continue;
        };
        let reply = emulator
            .call_in_region(case.region, case.method, &case.path, case.body.clone())
            .await;
        let message = message(&reply);
        for forbidden in [
            CONSTRAINT_ENVELOPE_MARKER,
            "failed to satisfy constraint",
            "regular expression pattern",
            "^arn:",
        ] {
            assert!(
                !message.contains(forbidden),
                "case {:?}: an ARN refusal must not read as a constraint violation, \
                 but its message carries {forbidden:?}: {message:?}",
                case.case
            );
        }
    }
}
