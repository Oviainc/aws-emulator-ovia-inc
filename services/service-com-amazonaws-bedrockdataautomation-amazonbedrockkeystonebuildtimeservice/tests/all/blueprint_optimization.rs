// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the blueprint-optimization job
//! (`InvokeBlueprintOptimizationAsync` mints one, `GetBlueprintOptimizationStatus`
//! reports it — the `create`/`read` pair of the modelled
//! `BlueprintOptimizationJobResource`).
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation, the
//! handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The suites are TABLE-driven: each asserts a rule over a set of inputs that differ
//! along what the rule generalises over (sample count, output-object form, stage
//! selection, encryption, tag count, signing region, invocation-ARN form), not over one
//! recorded request.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`). `can_handle` resolves the route key from it, and the handlers read
/// the region out of its credential scope when minting an ARN.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{region}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator under test, with a handle on the shared engine so a case can assert what
/// the store HOLDS and not only what it answers.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request in `region` and return its status and decoded JSON body
/// (`Value::Null` for an empty body).
async fn call_in_region(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization(region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// The region every case but the ARN-region rows signs for.
const REGION: &str = "us-east-1";

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, method, uri, body, REGION).await
}

/// `CreateBlueprint`, per its modelled `@http` `{method: PUT, uri: /blueprints/}`.
async fn create_blueprint(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    stage: &str,
) -> String {
    let body = json!({
        "blueprintName": name,
        "type": "DOCUMENT",
        "schema": r#"{"class":"invoice"}"#,
        "blueprintStage": stage,
    });
    let (status, payload) = call(router, "PUT", "/blueprints/", &body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
    );
    let blueprint = payload
        .get("blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: CreateBlueprint returned no blueprint"));
    string_at(&blueprint, "blueprintArn", case)
}

/// `InvokeBlueprintOptimizationAsync`, per its modelled `@http`
/// `{method: POST, uri: /invokeBlueprintOptimizationAsync, code: 200}`.
async fn invoke_optimization(
    router: &MultiServiceRouter,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        "POST",
        "/invokeBlueprintOptimizationAsync",
        body,
        region,
    )
    .await
}

/// `GetBlueprintOptimizationStatus`, per its modelled `@http`
/// `{method: POST, uri: /getBlueprintOptimizationStatus/{invocationArn}}`.
async fn get_status(router: &MultiServiceRouter, invocation_arn: &str) -> (u16, Value) {
    let uri = format!(
        "/getBlueprintOptimizationStatus/{}",
        encode_label(invocation_arn)
    );
    call(router, "POST", &uri, &json!({})).await
}

/// Structural check of a minted invocation ARN against the modelled
/// `BlueprintOptimizationInvocationArn` pattern
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:blueprint-optimization-invocation/[a-zA-Z0-9-_]+$`
/// (source: `smithy_query(operation="GetBlueprintOptimizationStatus")`), plus its
/// 128-character bound.
fn assert_well_formed_invocation_arn(arn: &str, expected_region: &str, case: &str) {
    assert!(
        arn.len() <= 128,
        "case {case:?}: invocationArn is {} characters, over the modeled 128: {arn}",
        arn.len()
    );
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: invocationArn has {} colon-separated fields, expected 6: {arn}",
        fields.len()
    );
    assert_eq!(fields[0], "arn", "case {case:?}: ARN prefix, got {arn}");
    assert_eq!(fields[1], "aws", "case {case:?}: ARN partition, got {arn}");
    assert_eq!(fields[2], "bedrock", "case {case:?}: ARN service, got {arn}");
    assert_eq!(
        fields[3], expected_region,
        "case {case:?}: ARN region, got {arn}"
    );
    assert!(
        fields[4].len() == 12 && fields[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: ARN account must be 12 digits, got {arn}"
    );
    let id = fields[5]
        .strip_prefix(state::OPTIMIZATION_INVOCATION_RESOURCE_PREFIX)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ARN resource must be {}<id>, got {arn}",
                state::OPTIMIZATION_INVOCATION_RESOURCE_PREFIX
            )
        });
    assert!(
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "case {case:?}: ARN resource id must match [a-zA-Z0-9-_]+, got {arn}"
    );
}

/// A data-automation profile ARN inside the modelled `DataAutomationProfileArn` pattern
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-profile/[a-zA-Z0-9-_.]+$`.
fn profile_arn(region: &str) -> String {
    format!("arn:aws:bedrock:{region}:000000000000:data-automation-profile/us.data-automation-v1")
}

/// One sample pair, as the modelled `BlueprintOptimizationSample` carries it: the asset
/// and the ground truth for it, each an `S3Object`.
fn sample(index: usize) -> Value {
    json!({
        "assetS3Object": { "s3Uri": format!("s3://bda-samples/assets/invoice-{index}.pdf") },
        "groundTruthS3Object": { "s3Uri": format!("s3://bda-samples/truth/invoice-{index}.json") },
    })
}

/// One submission the suites drive: the rows differ along everything the
/// submit-then-report rule generalises over.
struct Submission {
    case: &'static str,
    /// The stage the blueprint under optimization is created at.
    blueprint_stage: &'static str,
    /// The stage the submission SELECTS, or `None` when it selects none.
    selected_stage: Option<&'static str>,
    samples: Vec<Value>,
    output_configuration: Value,
    encryption_configuration: Option<Value>,
    tags: Vec<Value>,
    region: &'static str,
}

impl Submission {
    fn body(&self, blueprint_arn: &str) -> Value {
        let mut blueprint = json!({ "blueprintArn": blueprint_arn });
        if let Some(stage) = self.selected_stage {
            blueprint["stage"] = json!(stage);
        }
        let mut body = json!({
            "blueprint": blueprint,
            "samples": self.samples,
            "outputConfiguration": self.output_configuration,
            "dataAutomationProfileArn": profile_arn(self.region),
        });
        if let Some(encryption) = &self.encryption_configuration {
            body["encryptionConfiguration"] = encryption.clone();
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        body
    }
}

/// The submissions every suite here runs: one sample or several, a stage selected or
/// not, an output object with and without a key and a version, encryption named or not,
/// no tags and several, and two signing regions.
fn submissions() -> Vec<Submission> {
    vec![
        Submission {
            case: "one sample, no stage selected, bucket-root output",
            blueprint_stage: "DEVELOPMENT",
            selected_stage: None,
            samples: vec![sample(0)],
            output_configuration: json!({ "s3Object": { "s3Uri": "s3://bda-output" } }),
            encryption_configuration: None,
            tags: Vec::new(),
            region: REGION,
        },
        Submission {
            case: "two samples, DEVELOPMENT selected, nested output key",
            blueprint_stage: "DEVELOPMENT",
            selected_stage: Some("DEVELOPMENT"),
            samples: vec![sample(0), sample(1)],
            output_configuration: json!({
                "s3Object": { "s3Uri": "s3://bda-output/optimization/run-1/" },
            }),
            encryption_configuration: None,
            tags: vec![json!({ "key": "team", "value": "docs" })],
            region: REGION,
        },
        Submission {
            case: "three samples, LIVE selected, versioned output object, encrypted",
            blueprint_stage: "LIVE",
            selected_stage: Some("LIVE"),
            samples: vec![sample(0), sample(1), sample(2)],
            output_configuration: json!({
                "s3Object": { "s3Uri": "s3://bda-output/optimized.json", "version": "3" },
            }),
            encryption_configuration: Some(json!({
                "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abc-123",
                "kmsEncryptionContext": { "purpose": "optimization" },
            })),
            tags: vec![
                json!({ "key": "team", "value": "docs" }),
                json!({ "key": "cost-center", "value": "4711" }),
            ],
            region: REGION,
        },
        Submission {
            case: "another signing region",
            blueprint_stage: "LIVE",
            selected_stage: None,
            samples: vec![sample(0)],
            output_configuration: json!({
                "s3Object": { "s3Uri": "s3://bda-output-west/optimized.json" },
            }),
            encryption_configuration: None,
            tags: Vec::new(),
            region: "us-west-2",
        },
    ]
}

/// A submission is PERSISTED under the ARN it answers, and the read reports that stored
/// job: its clock-owed status and the output configuration the submission carried.
#[tokio::test]
async fn a_submission_is_persisted_and_reported_under_the_arn_it_answers() {
    for (index, spec) in submissions().into_iter().enumerate() {
        let case = spec.case;
        let (router, engine) = emulator();
        let blueprint_arn = create_blueprint(
            &router,
            case,
            &format!("optimize-bp-{index}"),
            spec.blueprint_stage,
        )
        .await;

        let (status, answered) =
            invoke_optimization(&router, &spec.body(&blueprint_arn), spec.region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the modeled InvokeBlueprintOptimizationAsync 200, \
             got {status} with {answered:?}"
        );
        let invocation_arn = string_at(&answered, "invocationArn", case);
        assert_well_formed_invocation_arn(&invocation_arn, spec.region, case);

        // PERSISTED: the store holds the submission under the ARN that was answered.
        {
            let store = engine.state::<ServiceState>();
            let held = store.lock();
            let job = held
                .optimization_jobs
                .get(&invocation_arn)
                .unwrap_or_else(|| panic!("case {case:?}: no job stored at {invocation_arn}"));
            assert_eq!(
                job.blueprint_arn, blueprint_arn,
                "case {case:?}: the stored job references the wrong blueprint"
            );
            assert_eq!(
                job.blueprint_stage.as_deref(),
                spec.selected_stage,
                "case {case:?}: the stored job kept the wrong selected stage"
            );
            assert_eq!(
                job.samples.len(),
                spec.samples.len(),
                "case {case:?}: the stored job kept {} samples, expected {}",
                job.samples.len(),
                spec.samples.len()
            );
            assert_eq!(
                job.data_automation_profile_arn,
                profile_arn(spec.region),
                "case {case:?}: the stored job kept the wrong dataAutomationProfileArn"
            );
            assert_eq!(
                job.tags.len(),
                spec.tags.len(),
                "case {case:?}: the stored job kept {} tags, expected {}",
                job.tags.len(),
                spec.tags.len()
            );
            let expected_key = spec
                .encryption_configuration
                .as_ref()
                .map(|config| string_at(config, "kmsKeyId", case));
            assert_eq!(
                job.kms_key_id, expected_key,
                "case {case:?}: the stored job kept the wrong kmsKeyId"
            );
        }

        // ROUND TRIP: the read answers about the stored job.
        let (status, reported) = get_status(&router, &invocation_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBlueprintOptimizationStatus 200, \
             got {status} with {reported:?}"
        );
        // The submission starts the job, so the status read straight after it is the
        // first, transient one: the transition away from it is owed at a non-zero span,
        // so no caller can miss it.
        assert_eq!(
            string_at(&reported, "status", case),
            state::OPTIMIZATION_CREATED,
            "case {case:?}: a just-submitted job is {} until the clock moves it: \
             {reported:?}",
            state::OPTIMIZATION_CREATED
        );
        assert_eq!(
            reported.get("outputConfiguration"),
            Some(&spec.output_configuration),
            "case {case:?}: the read must report the submitted outputConfiguration: \
             {reported:?}"
        );
        // A job that failed nothing reports no error.
        for member in ["errorType", "errorMessage"] {
            assert_eq!(
                reported.get(member),
                None,
                "case {case:?}: a job carrying no error must omit {member}: {reported:?}"
            );
        }
    }
}

/// Every submission gets its OWN job: the operation returns the identity of the job it
/// accepted, and no value in the request names an earlier one, so identical submissions
/// are distinct jobs — each resolvable through the read.
#[tokio::test]
async fn every_submission_mints_its_own_invocation_arn() {
    let (router, engine) = emulator();
    let blueprint_arn = create_blueprint(&router, "unique arns", "optimize-bp-unique", "LIVE").await;
    let spec = Submission {
        case: "unique arns",
        blueprint_stage: "LIVE",
        selected_stage: Some("LIVE"),
        samples: vec![sample(0)],
        output_configuration: json!({ "s3Object": { "s3Uri": "s3://bda-output/run.json" } }),
        encryption_configuration: None,
        tags: Vec::new(),
        region: REGION,
    };
    let body = spec.body(&blueprint_arn);

    let mut seen: Vec<String> = Vec::new();
    for attempt in 1..=4 {
        let case = format!("submission #{attempt}");
        let (status, answered) = invoke_optimization(&router, &body, REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answered:?}"
        );
        let arn = string_at(&answered, "invocationArn", &case);
        assert!(
            !seen.contains(&arn),
            "case {case:?}: {arn} was already handed out for an earlier submission"
        );
        let (status, reported) = get_status(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the ARN just minted must resolve, got {status} with {reported:?}"
        );
        seen.push(arn);
    }

    let store = engine.state::<ServiceState>();
    let held = store.lock();
    assert_eq!(
        held.optimization_jobs.len(),
        seen.len(),
        "the store must hold one job per submission, got {}",
        held.optimization_jobs.len()
    );
}

/// An `invocationArn` that is well formed but names no job this store holds is refused
/// with the modelled `ValidationException` (HTTP 400), with one fixed message that
/// carries nothing from the request — whatever region, account or id the ARN names, and
/// whether or not other jobs exist.
#[tokio::test]
async fn an_invocation_arn_naming_no_job_is_refused_as_a_validation_failure() {
    const NOT_FOUND: &str = "The requested operation is not recognized by the service.";

    let (router, _engine) = emulator();
    let blueprint_arn = create_blueprint(&router, "unknown arns", "optimize-bp-unknown", "LIVE")
        .await;
    let spec = Submission {
        case: "unknown arns",
        blueprint_stage: "LIVE",
        selected_stage: None,
        samples: vec![sample(0)],
        output_configuration: json!({ "s3Object": { "s3Uri": "s3://bda-output/run.json" } }),
        encryption_configuration: None,
        tags: Vec::new(),
        region: REGION,
    };
    // One job DOES exist, so a refusal below is about the ARN that was asked for and not
    // about an empty store.
    let (status, answered) = invoke_optimization(&router, &spec.body(&blueprint_arn), REGION).await;
    assert_eq!(status, 200, "the reference job must be accepted: {answered:?}");
    let minted = string_at(&answered, "invocationArn", "reference job");
    let (status, reported) = get_status(&router, &minted).await;
    assert_eq!(
        status, 200,
        "the reference job must read back, got {status} with {reported:?}"
    );

    // The minted ARN with its last character swapped: one character away from a job that
    // exists is still a job that does not.
    let mut neighbour = minted.clone();
    neighbour.pop();
    neighbour.push('f');
    let neighbour = if neighbour == minted {
        format!("{minted}0")
    } else {
        neighbour
    };

    let prefix = state::OPTIMIZATION_INVOCATION_RESOURCE_PREFIX;
    let unknown = [
        (
            "another region",
            format!("arn:aws:bedrock:eu-west-1:000000000000:{prefix}0000ffff-0000000000000001"),
        ),
        (
            "another account",
            format!("arn:aws:bedrock:us-east-1:123456789012:{prefix}0000ffff-0000000000000001"),
        ),
        (
            "a uuid-shaped id",
            format!(
                "arn:aws:bedrock:us-east-1:000000000000:{prefix}\
                 9f8c1f1e-7c24-4a4e-bb0e-3f3b1a2d6c57"
            ),
        ),
        (
            "a one-character id",
            format!("arn:aws:bedrock:us-east-1:000000000000:{prefix}a"),
        ),
        (
            "an id carrying the pattern's underscore",
            format!("arn:aws:bedrock:us-east-1:000000000000:{prefix}job_0001"),
        ),
        (
            "an id of the form this service mints, never minted",
            format!("arn:aws:bedrock:us-east-1:000000000000:{prefix}0000beef-00000000000000ff"),
        ),
        ("one character off a minted ARN", neighbour),
    ];

    for (case, arn) in unknown {
        let (status, payload) = get_status(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an invocationArn naming no job is a 400, \
             got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: wrong error code in {payload:?}"
        );
        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some(NOT_FOUND),
            "case {case:?}: wrong refusal message in {payload:?}"
        );
    }
}

/// The blueprint a submission references has to be one this account holds, at the stage
/// the submission selects: a reference that resolves to nothing earns the modelled
/// `ResourceNotFoundException` (HTTP 404) and mints no job. A public, AWS-owned blueprint
/// ARN is accepted: this store holds no catalog of those, so its absence here says
/// nothing about them.
#[tokio::test]
async fn a_submission_referencing_no_blueprint_of_this_account_is_refused() {
    const NOT_FOUND: &str = "Blueprint Arn not found.";

    let (router, engine) = emulator();
    let development = create_blueprint(&router, "stages", "optimize-bp-dev", "DEVELOPMENT").await;
    let live = create_blueprint(&router, "stages", "optimize-bp-live", "LIVE").await;

    // (case, blueprintArn, selected stage, accepted?)
    let cases: Vec<(&str, String, Option<&str>, bool)> = vec![
        ("a blueprint at its own stage", live.clone(), Some("LIVE"), true),
        (
            "a blueprint with no stage selected",
            development.clone(),
            None,
            true,
        ),
        (
            "a DEVELOPMENT blueprint, selected at its own stage",
            development.clone(),
            Some("DEVELOPMENT"),
            true,
        ),
        (
            "a public, AWS-owned blueprint this store holds no catalog of",
            "arn:aws:bedrock:us-east-1:aws:blueprint/bedrock-data-automation-public-invoice"
                .to_string(),
            None,
            true,
        ),
        (
            "a blueprint ARN nothing created",
            "arn:aws:bedrock:us-east-1:000000000000:blueprint/0000beef-000000000001".to_string(),
            None,
            false,
        ),
        (
            "a blueprint selected at the stage it is NOT at",
            live.clone(),
            Some("DEVELOPMENT"),
            false,
        ),
        (
            "a DEVELOPMENT blueprint selected as LIVE",
            development.clone(),
            Some("LIVE"),
            false,
        ),
    ];

    let mut accepted = 0usize;
    for (case, blueprint_arn, selected_stage, is_accepted) in cases {
        let mut blueprint = json!({ "blueprintArn": blueprint_arn });
        if let Some(stage) = selected_stage {
            blueprint["stage"] = json!(stage);
        }
        let body = json!({
            "blueprint": blueprint,
            "samples": [sample(0)],
            "outputConfiguration": { "s3Object": { "s3Uri": "s3://bda-output/run.json" } },
            "dataAutomationProfileArn": profile_arn(REGION),
        });
        let (status, payload) = invoke_optimization(&router, &body, REGION).await;
        if is_accepted {
            accepted += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the submission to be accepted, \
                 got {status} with {payload:?}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: an unresolvable blueprint reference is a 404, \
                 got {status} with {payload:?}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: wrong error code in {payload:?}"
            );
            assert_eq!(
                payload.get("message").and_then(Value::as_str),
                Some(NOT_FOUND),
                "case {case:?}: wrong refusal message in {payload:?}"
            );
        }
    }

    // A refused submission left nothing behind: only the accepted ones are stored.
    let store = engine.state::<ServiceState>();
    let held = store.lock();
    assert_eq!(
        held.optimization_jobs.len(),
        accepted,
        "the store must hold exactly the accepted submissions, got {}",
        held.optimization_jobs.len()
    );
}

/// A job settles through the statuses the clock owes it, and reporting one writes
/// nothing: two reads at the same stage agree, and the transitions come due whether or
/// not anybody called in between.
#[tokio::test]
async fn a_job_settles_through_the_statuses_the_clock_owes_it() {
    let (router, _engine) = emulator();
    let blueprint_arn = create_blueprint(&router, "settling", "optimize-bp-settling", "LIVE").await;
    let output = json!({ "s3Object": { "s3Uri": "s3://bda-output/settling.json" } });
    let body = json!({
        "blueprint": { "blueprintArn": blueprint_arn },
        "samples": [sample(0), sample(1)],
        "outputConfiguration": output,
        "dataAutomationProfileArn": profile_arn(REGION),
    });
    let (status, answered) = invoke_optimization(&router, &body, REGION).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answered:?}");
    let invocation_arn = string_at(&answered, "invocationArn", "settling");

    // Each leg of the lifecycle: the status before the first transition is due, then
    // after each one. A test may wait for a due instant; a handler may not. `tokio`'s
    // timer is not in this workspace's feature set, so the current-thread test runtime
    // parks instead.
    let legs = [
        ("before the first transition is due", state::OPTIMIZATION_CREATED),
        ("after the first transition is due", state::OPTIMIZATION_IN_PROGRESS),
        ("after the second transition is due", state::OPTIMIZATION_SUCCESS),
    ];
    for (leg, (case, expected)) in legs.into_iter().enumerate() {
        if leg > 0 {
            std::thread::sleep(state::OPTIMIZATION_STAGE_SPAN + Duration::from_millis(250));
        }
        // Two reads in a row agree, because reporting the status writes nothing.
        for attempt in 1..=2 {
            let (status, reported) = get_status(&router, &invocation_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} read #{attempt}: expected 200, got {status} with {reported:?}"
            );
            assert_eq!(
                string_at(&reported, "status", case),
                expected,
                "case {case:?} read #{attempt}: expected status {expected}, got {reported:?}"
            );
            assert_eq!(
                reported.get("outputConfiguration"),
                Some(&output),
                "case {case:?} read #{attempt}: the read must keep reporting the \
                 submitted outputConfiguration: {reported:?}"
            );
        }
    }
}
