// SPDX-License-Identifier: CC0-1.0

//! Resolving the control-plane resources a submission REFERENCES, end to end over the
//! wire.
//!
//! The rule under test: an invocation names what to extract by reference — a
//! data-automation project (`dataAutomationConfiguration.dataAutomationProjectArn`) or
//! inline `blueprints` — and both resources belong to the data-automation BUILD-TIME
//! service, which this emulator serves on the same engine. The job's work IS that
//! project's output configuration (or that blueprint's schema), so the reference is
//! resolved against the owning service's store before a job is minted, and a reference
//! that resolves to nothing is refused with the operation's own modelled
//! `ValidationException` (HTTP 400) — the only client-fault error either invoke operation
//! declares — with nothing persisted.
//!
//! The rows span what the rule generalises over, not one request: which member carries the
//! reference, how many references a submission carries, whether the resource was never
//! created or was created and then deleted, whether the account owns it or AWS does
//! (`…:aws:…`, the `SERVICE` half of the build-time service's `ResourceOwner` enum, which
//! is a catalog no account store holds and so cannot be refused), the account and Region
//! fields of the ARN, and which of the two invoke operations is asked.
//!
//! Every project and blueprint a row references is created through the build-time
//! service's own `CreateDataAutomationProject` / `CreateBlueprint` and released through
//! its `DeleteDataAutomationProject` / `DeleteBlueprint`, over the same `restJson1` wire
//! an SDK uses — so the two services are asserted to agree about ONE set of resources.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice as build_time;
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

use bda::state::{BLUEPRINT_NOT_FOUND_MESSAGE, PROJECT_NOT_FOUND_MESSAGE};

/// The signing name both services share (`OPERATIONS.md`).
const SIGNING_NAME: &str = "bedrock";

/// The Region every request below signs for.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (`smithy_query` shape `DataAutomationProfileArn`).
/// No emulated service owns a data-automation profile, so this member is not resolved
/// against any store — the row that submits an unknown-looking profile ARN is accepted
/// for exactly that reason.
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// What a row expects the submission to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// HTTP 200 with an `invocationArn`, and a job the reader can resolve.
    Accepted,
    /// HTTP 400 `ValidationException` carrying this message, and nothing persisted.
    Refused(&'static str),
}

/// One row of the resolution table.
struct ReferenceCase {
    case: String,
    /// `dataAutomationConfiguration`, or `Value::Null` to leave the member absent.
    configuration: Value,
    /// `blueprints`, or `Value::Null` to leave the member absent.
    blueprints: Value,
    expected: Expectation,
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', ':'])
            .next()
            .unwrap_or_default()
    }

    /// The error message the refusal carries, or the empty string.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// The SigV4 `Authorization` header both services route on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The emulator under test: the runtime service AND the build-time service that owns the
/// referenced resources, over ONE shared engine, exactly as the production router mounts
/// them.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(build_time::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Call one runtime operation the way an SDK would (`awsJson1_1`, `POST /` plus
/// `X-Amz-Target`).
async fn call_runtime(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Call one build-time operation the way an SDK would (`restJson1`, a bound method and
/// URI).
async fn call_build_time(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Value,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Percent-encode an `@httpLabel` value the way an AWS SDK encodes a non-greedy one:
/// encoding every non-alphanumeric byte is a superset of what a label needs and covers
/// the `:` and `/` of an ARN, so the whole ARN travels inside ONE path segment. The
/// encoding itself is the `percent-encoding` crate's.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Create one data-automation project through the owning service and return its minted
/// `projectArn`. `stage` is `projectStage`, or `None` to leave the member absent.
async fn create_project(
    router: &MultiServiceRouter,
    name: &str,
    stage: Option<&str>,
    standard_output: Value,
) -> String {
    let mut body = json!({
        "projectName": name,
        "standardOutputConfiguration": standard_output,
    });
    if let Some(stage) = stage {
        body["projectStage"] = json!(stage);
    }
    let answer = call_build_time(router, "PUT", "/data-automation-projects/", body).await;
    assert_eq!(
        answer.status, 201,
        "CreateDataAutomationProject({name:?}) must answer its modelled 201, got {} / {}",
        answer.status, answer.body
    );
    answer.body["projectArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDataAutomationProject({name:?}) returned no projectArn"))
        .to_string()
}

/// Create one blueprint through the owning service and return its minted `blueprintArn`.
async fn create_blueprint(
    router: &MultiServiceRouter,
    name: &str,
    kind: &str,
    schema: &str,
) -> String {
    let body = json!({"blueprintName": name, "type": kind, "schema": schema});
    let answer = call_build_time(router, "PUT", "/blueprints/", body).await;
    assert_eq!(
        answer.status, 201,
        "CreateBlueprint({name:?}) must answer its modelled 201, got {} / {}",
        answer.status, answer.body
    );
    answer.body["blueprint"]["blueprintArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateBlueprint({name:?}) returned no blueprintArn"))
        .to_string()
}

/// Release one project through the owning service.
async fn delete_project(router: &MultiServiceRouter, arn: &str) {
    let uri = format!("/data-automation-projects/{}/", encode_label(arn));
    let answer = call_build_time(router, "DELETE", &uri, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "DeleteDataAutomationProject({arn:?}) must answer 200, got {} / {}",
        answer.status, answer.body
    );
}

/// Release one blueprint through the owning service.
async fn delete_blueprint(router: &MultiServiceRouter, arn: &str) {
    let uri = format!("/blueprints/{}/", encode_label(arn));
    let answer = call_build_time(router, "DELETE", &uri, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "DeleteBlueprint({arn:?}) must answer 200, got {} / {}",
        answer.status, answer.body
    );
}

/// A submission body carrying `configuration` and `blueprints` as the row gave them.
fn submission(case: &str, configuration: &Value, blueprints: &Value) -> Value {
    let mut body = json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
        "outputConfiguration": {"s3Uri": format!("s3://out-bucket/{case}/")},
        "dataAutomationProfileArn": PROFILE_ARN,
    });
    if !configuration.is_null() {
        body["dataAutomationConfiguration"] = configuration.clone();
    }
    if !blueprints.is_null() {
        body["blueprints"] = blueprints.clone();
    }
    body
}

/// A well-formed project ARN, inside the modelled `DataAutomationArn` pattern, that names
/// no project any store holds.
fn unknown_project_arn(account: &str, region: &str, id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{account}:data-automation-project/{id}")
}

/// A well-formed ACCOUNT-owned blueprint ARN (the modelled `BlueprintArn` pattern's
/// `[a-zA-Z0-9-]{12,36}` resource id) that names no blueprint any store holds.
fn unknown_blueprint_arn(account: &str, region: &str, id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{account}:blueprint/{id}")
}

/// One AWS-owned blueprint reference: the `SERVICE`-owned catalog the modelled
/// `BlueprintArn` pattern spells `…:aws:blueprint/bedrock-data-automation-public-…`.
fn public_blueprint(suffix: &str) -> Value {
    json!({"blueprintArn": format!("arn:aws:bedrock:us-west-2:aws:blueprint/bedrock-data-automation-public-{suffix}")})
}

/// How many invocation records this engine retains.
fn invocation_count(engine: &engine::InMemoryEngine) -> usize {
    bda::state::service_state(engine)
        .expect("the runtime store on this engine")
        .lock()
        .invocation_count()
}

#[tokio::test]
async fn a_submission_resolves_every_referenced_project_and_blueprint() {
    let (router, engine) = emulator();

    // ARRANGE through the owning service's own API: two projects (one in each modelled
    // `DataAutomationProjectStage`), two blueprints, and one of each that is created and
    // then released, so a "deleted" row differs from a "never created" row only in how
    // the reference came to be dangling.
    let live_project = create_project(&router, "resolution-live", Some("LIVE"), json!({})).await;
    let development_project = create_project(
        &router,
        "resolution-development",
        Some("DEVELOPMENT"),
        json!({"document": {"outputFormat": {
            "textFormat": {"types": ["MARKDOWN"]},
            "additionalFileFormat": {"state": "DISABLED"},
        }}}),
    )
    .await;
    let document_blueprint =
        create_blueprint(&router, "resolution-document", "DOCUMENT", r#"{"class":"x"}"#).await;
    let video_blueprint =
        create_blueprint(&router, "resolution-video", "VIDEO", r#"{"class":"y"}"#).await;
    let released_project = create_project(&router, "resolution-released", None, json!({})).await;
    delete_project(&router, &released_project).await;
    let released_blueprint =
        create_blueprint(&router, "resolution-released-bp", "IMAGE", r#"{"class":"z"}"#).await;
    delete_blueprint(&router, &released_blueprint).await;

    let cases = vec![
        ReferenceCase {
            case: "a created project, named alone".to_string(),
            configuration: json!({"dataAutomationProjectArn": live_project}),
            blueprints: Value::Null,
            expected: Expectation::Accepted,
        },
        ReferenceCase {
            case: "a created project, with the stage member".to_string(),
            configuration: json!({
                "dataAutomationProjectArn": development_project,
                "stage": "DEVELOPMENT",
            }),
            blueprints: Value::Null,
            expected: Expectation::Accepted,
        },
        ReferenceCase {
            case: "a project nobody created".to_string(),
            configuration: json!({
                "dataAutomationProjectArn":
                    unknown_project_arn("000000000000", REGION, "never-created"),
            }),
            blueprints: Value::Null,
            expected: Expectation::Refused(PROJECT_NOT_FOUND_MESSAGE),
        },
        ReferenceCase {
            case: "a project in another account and Region".to_string(),
            configuration: json!({
                "dataAutomationProjectArn":
                    unknown_project_arn("123456789012", "eu-west-2", "someone_elses"),
            }),
            blueprints: Value::Null,
            expected: Expectation::Refused(PROJECT_NOT_FOUND_MESSAGE),
        },
        ReferenceCase {
            case: "a project that was created and then deleted".to_string(),
            configuration: json!({"dataAutomationProjectArn": released_project}),
            blueprints: Value::Null,
            expected: Expectation::Refused(PROJECT_NOT_FOUND_MESSAGE),
        },
        ReferenceCase {
            case: "an AWS-owned project, which no account store holds".to_string(),
            configuration: json!({
                "dataAutomationProjectArn":
                    unknown_project_arn("aws", REGION, "us-data-automation-catalog"),
            }),
            blueprints: Value::Null,
            expected: Expectation::Accepted,
        },
        ReferenceCase {
            case: "one created blueprint".to_string(),
            configuration: Value::Null,
            blueprints: json!([{"blueprintArn": document_blueprint}]),
            expected: Expectation::Accepted,
        },
        ReferenceCase {
            case: "two created blueprints, one carrying its stage".to_string(),
            configuration: Value::Null,
            blueprints: json!([
                {"blueprintArn": document_blueprint, "stage": "DEVELOPMENT"},
                {"blueprintArn": video_blueprint},
            ]),
            expected: Expectation::Accepted,
        },
        ReferenceCase {
            case: "an account-owned blueprint nobody created".to_string(),
            configuration: Value::Null,
            blueprints: json!([{
                "blueprintArn": unknown_blueprint_arn("000000000000", REGION, "aaaaaaaaaaaa"),
            }]),
            expected: Expectation::Refused(BLUEPRINT_NOT_FOUND_MESSAGE),
        },
        ReferenceCase {
            case: "three blueprints, the last of them unknown".to_string(),
            configuration: Value::Null,
            blueprints: json!([
                {"blueprintArn": document_blueprint},
                {"blueprintArn": video_blueprint},
                {"blueprintArn": unknown_blueprint_arn("123456789012", "us-east-1", "bbbbbbbbbbbbbbbb")},
            ]),
            expected: Expectation::Refused(BLUEPRINT_NOT_FOUND_MESSAGE),
        },
        ReferenceCase {
            case: "a blueprint that was created and then deleted".to_string(),
            configuration: Value::Null,
            blueprints: json!([{"blueprintArn": released_blueprint}]),
            expected: Expectation::Refused(BLUEPRINT_NOT_FOUND_MESSAGE),
        },
        ReferenceCase {
            case: "AWS-owned public blueprints only".to_string(),
            configuration: Value::Null,
            blueprints: json!([public_blueprint("video"), public_blueprint("document")]),
            expected: Expectation::Accepted,
        },
        ReferenceCase {
            case: "an AWS-owned blueprint beside a created one".to_string(),
            configuration: Value::Null,
            blueprints: json!([public_blueprint("audio"), {"blueprintArn": video_blueprint}]),
            expected: Expectation::Accepted,
        },
    ];

    let mut accepted = 0usize;
    for case in &cases {
        let name = case.case.as_str();
        let before = invocation_count(&engine);
        let answer = call_runtime(
            &router,
            "InvokeDataAutomationAsync",
            submission(name, &case.configuration, &case.blueprints),
        )
        .await;

        match case.expected {
            Expectation::Accepted => {
                accepted += 1;
                assert_eq!(
                    answer.status, 200,
                    "case {name:?}: expected 200, got {} / {}",
                    answer.status, answer.body
                );
                let invocation_arn = answer.body["invocationArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: no invocationArn in {}", answer.body))
                    .to_string();
                assert_eq!(
                    invocation_count(&engine),
                    before + 1,
                    "case {name:?}: an accepted submission must persist exactly one job"
                );

                // ROUND TRIP: the job the submission minted is resolvable by the reader.
                let read = call_runtime(
                    &router,
                    "GetDataAutomationStatus",
                    json!({"invocationArn": invocation_arn}),
                )
                .await;
                assert_eq!(
                    read.status, 200,
                    "case {name:?}: GetDataAutomationStatus({invocation_arn:?}) answered {} / {}",
                    read.status, read.body
                );
            }
            Expectation::Refused(message) => {
                assert_eq!(
                    answer.status, 400,
                    "case {name:?}: expected 400, got {} / {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {name:?}: expected ValidationException, got {}",
                    answer.body
                );
                assert_eq!(
                    answer.message(),
                    message,
                    "case {name:?}: wrong refusal message, got {}",
                    answer.body
                );
                assert_eq!(
                    invocation_count(&engine),
                    before,
                    "case {name:?}: a refused submission must persist nothing"
                );
            }
        }
    }

    assert_eq!(
        invocation_count(&engine),
        accepted,
        "exactly the accepted submissions are retained"
    );
}

#[tokio::test]
async fn a_reference_outside_the_modelled_pattern_is_refused_before_resolution() {
    let (router, engine) = emulator();

    // Resolution concerns itself only with ARNs, because a value that is not one is
    // already refused by the modelled `@pattern` on both members — the framework's
    // boundary walk runs before any handler. Pinned here so that invariant is a test and
    // not an assumption.
    let malformed: [(&str, Value, Value); 4] = [
        (
            "project-plain-text",
            json!({"dataAutomationProjectArn": "invoices"}),
            Value::Null,
        ),
        (
            "project-another-vendors-arn",
            json!({"dataAutomationProjectArn": "arn:aws:s3:::out-bucket"}),
            Value::Null,
        ),
        (
            "blueprint-plain-text",
            Value::Null,
            json!([{"blueprintArn": "my-blueprint"}]),
        ),
        (
            "blueprint-member-carrying-an-invocation-arn",
            Value::Null,
            json!([{
                "blueprintArn":
                    format!("arn:aws:bedrock:{REGION}:000000000000:data-automation-invocation/abc"),
            }]),
        ),
    ];

    for (name, configuration, blueprints) in &malformed {
        let answer = call_runtime(
            &router,
            "InvokeDataAutomationAsync",
            submission(name, configuration, blueprints),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {}",
            answer.body
        );
        assert!(
            answer
                .message()
                .contains("Member must satisfy regular expression pattern:"),
            "case {name:?}: the refusal must be the modelled pattern violation, got {}",
            answer.body
        );
        for not_found in [PROJECT_NOT_FOUND_MESSAGE, BLUEPRINT_NOT_FOUND_MESSAGE] {
            assert_ne!(
                answer.message(),
                not_found,
                "case {name:?}: a value that is not an ARN must not reach reference \
                 resolution, got {}",
                answer.body
            );
        }
        assert_eq!(
            invocation_count(&engine),
            0,
            "case {name:?}: a refused submission must persist nothing"
        );
    }
}

#[tokio::test]
async fn the_synchronous_invoke_resolves_the_same_references() {
    let (router, engine) = emulator();
    let project = create_project(&router, "sync-resolution", None, json!({})).await;
    let blueprint =
        create_blueprint(&router, "sync-resolution-bp", "DOCUMENT", r#"{"class":"s"}"#).await;

    // A reference that resolves to nothing is a CLIENT error on this operation too, so it
    // is refused before the 501 this still-unimplemented operation answers for a
    // submission it cannot serve.
    let refusals: Vec<(&str, Value, Value, &str)> = vec![
        (
            "unknown project",
            json!({
                "dataAutomationProjectArn":
                    unknown_project_arn("000000000000", REGION, "sync-never-created"),
            }),
            Value::Null,
            PROJECT_NOT_FOUND_MESSAGE,
        ),
        (
            "unknown blueprint",
            Value::Null,
            json!([{"blueprintArn": unknown_blueprint_arn("000000000000", REGION, "cccccccccccc")}]),
            BLUEPRINT_NOT_FOUND_MESSAGE,
        ),
    ];
    for (name, configuration, blueprints, message) in &refusals {
        let answer = call_runtime(
            &router,
            "InvokeDataAutomation",
            submission(name, configuration, blueprints),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            *message,
            "case {name:?}: wrong refusal message, got {}",
            answer.body
        );
    }

    // A submission whose references DO resolve is not a client error: the synchronous
    // operation is still unimplemented, which is the 501 it has always answered.
    for (name, configuration, blueprints) in [
        (
            "resolvable project",
            json!({"dataAutomationProjectArn": project}),
            Value::Null,
        ),
        (
            "resolvable blueprint",
            Value::Null,
            json!([{"blueprintArn": blueprint}]),
        ),
    ] {
        let answer = call_runtime(
            &router,
            "InvokeDataAutomation",
            submission(name, &configuration, &blueprints),
        )
        .await;
        assert_eq!(
            answer.status, 501,
            "case {name:?}: expected the unimplemented 501, got {} / {}",
            answer.status, answer.body
        );
    }

    assert_eq!(
        invocation_count(&engine),
        0,
        "the synchronous operation mints no asynchronous job"
    );
}
