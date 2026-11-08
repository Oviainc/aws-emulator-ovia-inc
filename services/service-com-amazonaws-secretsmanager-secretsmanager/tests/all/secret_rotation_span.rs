// SPDX-License-Identifier: CC0-1.0

//! WHEN a rotation's steps run, as a client watching the rotation function's own side effect
//! sees it: Secrets Manager, Lambda and Amazon S3 on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in.
//!
//! THE RULE: `RotateSecret` "configures and STARTS the asynchronous process of rotating the
//! secret" (this service's Smithy model) — so the four steps the rotation function serves are
//! driven straight through, each one following the last, and what a client waits for is the
//! function's own work rather than a span this service adds ahead of it or between two steps.
//! The step that produces a side effect a client can watch has therefore run inside as many
//! invocations of the function as its place in the order, and the whole rotation inside four
//! of them — which is what makes a client that acts on an early step's side effect (here the
//! backup copy the function writes to Amazon S3) find the rest of the rotation already done
//! rather than still waiting to start.
//!
//! That is the half of the rotation contract a span added per step breaks without failing
//! anything: every step still runs and the secret still ends up rotated, but each one lands
//! later than the work it does, so a client that saw the function's first side effect reads a
//! secret whose `AWSCURRENT` has not moved yet.
//!
//! The steps differ along WHERE in the order each one sits — the function writes its backup
//! copy in every one of them, under that step's own key, so ONE rotation shows when each ran
//! and the rule is asserted from every point a client can first notice the rotation. The span
//! one step is allowed is MEASURED on the function itself ([`one_invocation`]), so what is
//! asserted is the rotation's span in units of the work it does rather than a figure a host
//! has to meet.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::secret_client_request_token::fresh_token;
use crate::secret_rotation_driver::package;

/// The Region every request in this suite signs for.
const REGION: &str = "us-west-2";

/// The service principal Secrets Manager invokes a rotation function as.
const SERVICE_PRINCIPAL: &str = "secretsmanager.amazonaws.com";

/// The IAM action an invocation of a function performs.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A well-formed execution role ARN for the rotation function.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/rotation-exec";

/// The handler entry point the deployment package below declares.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The bucket the rotation function writes its backup copy to.
const BUCKET: &str = "rotation-backups";

/// The value the secret holds before the rotation, so the rotated value is asserted on the
/// material rather than on the version id alone.
const INITIAL_VALUE: &str = "the-value-before-the-rotation";

/// What the rotation function appends to the value it rotates.
const ROTATED_SUFFIX: &str = "-rotated";

/// How long a client watching for the function's side effect waits between looks, and the
/// longest it waits at all. The interval is the one a client polls at; the rule is about
/// what it finds once the side effect is there.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);
const SIDE_EFFECT_WAIT: Duration = Duration::from_secs(30);

/// What ONE step of a rotation costs beyond the invocation it is: the slack a scheduled task
/// and a client's own look interval add to the function's own run.
///
/// The span of a step itself is MEASURED per row ([`one_invocation`]) rather than written
/// down, because an invocation of a function costs whatever this host costs it; what the rule
/// bounds is the rotation's span in units of that work, so a span this service adds ahead of
/// a step, or between two of them, is what the bands below catch.
const STEP_SLACK: Duration = Duration::from_secs(1);

/// How many steps a rotation drives the function through: "Secrets Manager calls the Lambda
/// rotation function four times, each time with a different value for the Step parameter"
/// (Secrets Manager User Guide, *How rotation works*), which is the length of this service's
/// own `ROTATION_STEPS`.
const ROTATION_STEPS: usize =
    service_com_amazonaws_secretsmanager_secretsmanager::state::rotation::ROTATION_STEPS.len();

/// A `Step` value that is none of the rotation's own, so the function reads the secret and
/// returns without writing anything: what one invocation of it costs, measured.
const MEASURING_STEP: &str = "measureSecret";

/// Secrets Manager, Lambda and Amazon S3 over ONE shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Secrets Manager call (`awsJson1_1`) that must answer 200.
async fn sm_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("authorization", authorization("secretsmanager"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Amazon S3 request, path style, as its status and body text.
async fn s3(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    (
        parts.status.as_u16(),
        String::from_utf8_lossy(&body).to_string(),
    )
}

/// The rotation function this suite runs: the four steps AWS invokes a rotation function
/// with, each through this service's own operations, and the BACKUP copy a client watches a
/// rotation by — written in EVERY step under that step's own key, so one rotation shows when
/// each of the four ran.
const ROTATION_FUNCTION: &str = r#"
import boto3
import os

SUFFIX = "-rotated"


def lambda_handler(event, context):
    arn = event["SecretId"]
    token = event["ClientRequestToken"]
    step = event["Step"]
    client = boto3.client("secretsmanager")
    current = client.get_secret_value(SecretId=arn, VersionStage="AWSCURRENT")
    boto3.client("s3").put_object(
        Bucket=os.environ["BACKUP_BUCKET"],
        Key=os.environ["BACKUP_PREFIX"] + step,
        Body=current["SecretString"].encode("utf-8"),
    )
    if step == "createSecret":
        client.put_secret_value(
            SecretId=arn,
            ClientRequestToken=token,
            SecretString=current["SecretString"] + SUFFIX,
            VersionStages=["AWSPENDING"],
        )
    elif step == "testSecret":
        client.get_secret_value(SecretId=arn, VersionId=token)
    elif step == "finishSecret":
        metadata = client.describe_secret(SecretId=arn)
        previous = None
        for version, stages in metadata["VersionIdsToStages"].items():
            if "AWSCURRENT" in stages:
                previous = version
        client.update_secret_version_stage(
            SecretId=arn,
            VersionStage="AWSCURRENT",
            MoveToVersionId=token,
            RemoveFromVersionId=previous,
        )
    return {"step": step}
"#;

/// Create the rotation function `name`, writing its backup copies under `backup_prefix`, wait
/// out its provisioning window and answer its ARN.
async fn create_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    backup_prefix: &str,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Environment": {"Variables": {
                "BACKUP_BUCKET": BUCKET,
                "BACKUP_PREFIX": backup_prefix,
            }},
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(ROTATION_FUNCTION)),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} with {created}"
    );
    // The grant that admits this service's invocation, which a rotation cannot start without.
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/policy"),
        &json!({
            "StatementId": "rotation-invoke",
            "Action": INVOKE_ACTION,
            "Principal": SERVICE_PRINCIPAL,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from AddPermission, got {status} with {granted}"
    );
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string()
}

/// Wait until the object `key` is in the bucket — the rotation function's own side effect, as
/// a client watching for it sees it — and answer how long that took, counted from `started`.
async fn backup_written(
    router: &MultiServiceRouter,
    case: &str,
    key: &str,
    started: Instant,
) -> Duration {
    let looks = SIDE_EFFECT_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let (status, _) = s3(router, "HEAD", &format!("/{BUCKET}/{key}"), &[]).await;
        if status == 200 {
            return started.elapsed();
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: the rotation function never wrote its backup object {key:?}");
}

/// The version `AWSCURRENT` names in a `DescribeSecret` answer.
fn current_version(described: &Value) -> Option<String> {
    described["VersionIdsToStages"]
        .as_object()?
        .iter()
        .find(|(_, stages)| {
            stages
                .as_array()
                .is_some_and(|held| held.iter().any(|stage| stage == "AWSCURRENT"))
        })
        .map(|(version, _)| version.clone())
}

/// The version `AWSCURRENT` names, and whether `AWSPENDING` is held by any other version.
async fn staging(router: &MultiServiceRouter, case: &str, secret_id: &str) -> (String, bool) {
    let described = sm_ok(router, case, "DescribeSecret", &json!({"SecretId": secret_id})).await;
    let versions = described["VersionIdsToStages"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut current = String::new();
    let mut pending_elsewhere = false;
    for (version, stages) in &versions {
        let stages: Vec<&str> = stages
            .as_array()
            .map(|held| held.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if stages.contains(&"AWSCURRENT") {
            current = version.clone();
        } else if stages.contains(&"AWSPENDING") {
            pending_elsewhere = true;
        }
    }
    assert!(
        !current.is_empty(),
        "case {case:?}: expected a version to hold AWSCURRENT, got {described}"
    );
    (current, pending_elsewhere)
}

/// One step of the rotation, as the order AWS drives them in places it: the `Step` the
/// function is invoked with, and which of the four it is, counted from one — a step's own side
/// effect cannot be visible before that many steps have run.
const STEPS: [(&str, u32); 4] = [
    ("createSecret", 1),
    ("setSecret", 2),
    ("testSecret", 3),
    ("finishSecret", 4),
];

/// Wait until the rotation has FINISHED — `AWSCURRENT` on `staked` and `LastRotatedDate`
/// stamped, which is what this service writes once the last step has returned — and answer
/// how long that took, counted from `started`.
async fn rotation_settled(
    router: &MultiServiceRouter,
    case: &str,
    secret_id: &str,
    staked: &str,
    started: Instant,
) -> Duration {
    let looks = SIDE_EFFECT_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let described =
            sm_ok(router, case, "DescribeSecret", &json!({"SecretId": secret_id})).await;
        let current = current_version(&described);
        if current.as_deref() == Some(staked) && described["LastRotatedDate"].is_number() {
            return started.elapsed();
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: the rotation never finished on the version {staked:?}")
}

/// What ONE invocation of the rotation function `name` costs on this host, measured by
/// invoking it with a `Step` of none of the rotation's own ([`MEASURING_STEP`]) — so the
/// function reads the secret and returns without writing anything.
async fn one_invocation(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    secret_id: &str,
) -> Duration {
    let started = Instant::now();
    let (status, answered) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/invocations"),
        &json!({
            "Step": MEASURING_STEP,
            "SecretId": secret_id,
            "ClientRequestToken": "none",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the measuring invocation, got {status} with \
         {answered}"
    );
    started.elapsed()
}

/// RULE: the rotation `RotateSecret` starts is driven straight through its four steps — each
/// step has run inside its own place in the order, and the whole rotation inside four steps,
/// leaving `AWSCURRENT` on the version the rotation created, no other version holding
/// `AWSPENDING`, the function's own value stored and `LastRotatedDate` stamped.
#[tokio::test]
async fn a_rotations_steps_run_as_promptly_as_the_work_they_do() {
    let case = "a rotation driven through its four steps";
    let router = router();
    let location = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, created) = s3(&router, "PUT", &format!("/{BUCKET}"), location.as_bytes()).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: expected the bucket to be created, got {status} with {created}"
    );

    let name = "rotation-span".to_string();
    let backup_prefix = format!("backups/{name}/");
    let function_name = "rotation-span-fn".to_string();
    let function = create_function(&router, case, &function_name, &backup_prefix).await;
    sm_ok(
        &router,
        case,
        "CreateSecret",
        &json!({
            "Name": &name,
            "SecretString": INITIAL_VALUE,
            "ClientRequestToken": fresh_token(),
        }),
    )
    .await;
    let (initial_version, _) = staging(&router, case, &name).await;
    // What one step of the rotation COSTS here, so the bands below are stated in units of the
    // work rather than in a figure this host has to meet.
    let step_band = one_invocation(&router, case, &function_name, &name).await + STEP_SLACK;

    let started = Instant::now();
    let rotating = sm_ok(
        &router,
        case,
        "RotateSecret",
        &json!({"SecretId": &name, "RotationLambdaARN": &function}),
    )
    .await;
    let staked = rotating["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a VersionId, got {rotating}"))
        .to_string();

    // WHEN each step ran, as a client watching the function's own side effect sees it: the
    // backup copy that step wrote, which it writes before doing the step's own work.
    for (step, number) in STEPS {
        let waited = backup_written(
            &router,
            case,
            &format!("{backup_prefix}{step}"),
            started,
        )
        .await;
        let band = step_band * number;
        assert!(
            waited <= band,
            "case {case:?}: expected step {number} of the rotation ({step}) to have run inside \
             {band:?} of the call that started it, got {waited:?}"
        );
    }

    // And the rotation finishes: `AWSCURRENT` moves onto the version the rotation created,
    // `AWSPENDING` is no longer held elsewhere, and it is all inside the span the four steps
    // take.
    let settled = rotation_settled(&router, case, &name, &staked, started).await;
    let whole = step_band * u32::try_from(ROTATION_STEPS).unwrap_or(4);
    assert!(
        settled <= whole,
        "case {case:?}: expected the whole rotation to be finished inside {whole:?} of the call \
         that started it, got {settled:?}"
    );
    let (current, pending_elsewhere) = staging(&router, case, &name).await;
    assert_eq!(
        current, staked,
        "case {case:?}: expected AWSCURRENT on the rotated version {staked:?}, got {current:?} \
         against the pre-rotation {initial_version:?}"
    );
    assert!(
        !pending_elsewhere,
        "case {case:?}: expected no other version to hold AWSPENDING once the rotation \
         finished, got the secret with one"
    );

    // The rotated value is the one the function wrote, and the rotation is stamped as
    // completed: the four steps really ran.
    let read = sm_ok(&router, case, "GetSecretValue", &json!({"SecretId": &name})).await;
    assert_eq!(
        read["SecretString"].as_str(),
        Some(format!("{INITIAL_VALUE}{ROTATED_SUFFIX}").as_str()),
        "case {case:?}: expected the rotated value, got {read}"
    );
    let described = sm_ok(&router, case, "DescribeSecret", &json!({"SecretId": &name})).await;
    assert!(
        described["LastRotatedDate"].is_number(),
        "case {case:?}: expected LastRotatedDate to be stamped, got {described}"
    );
}
