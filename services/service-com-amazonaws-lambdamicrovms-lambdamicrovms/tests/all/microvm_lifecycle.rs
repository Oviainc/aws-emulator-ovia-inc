// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MicroVM image -> MicroVM lifecycle slice, driven over the
//! wire through the same router the emulator boots.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for; each minted ARN must carry it.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical emulator account id (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// Comfortably longer than every clock-owed span in `crate::state` (each is 500 ms), so
/// by the time this has passed the transition is due even on a loaded machine. The
/// margin is deliberately several times the span: these suites assert that a transition
/// HAS become due, so only an over-long wait is safe.
pub(crate) const PAST_THE_SPAN: Duration = Duration::from_millis(2_500);

/// Comfortably longer than the 1-second `maximumDurationInSeconds` the expiry suite asks
/// for, for the same reason as [`PAST_THE_SPAN`].
const PAST_THE_MAXIMUM_DURATION: Duration = Duration::from_millis(3_000);

/// The base image every create builds on: the AWS-MANAGED Amazon Linux 2023 MicroVM
/// base image this service publishes (`state::MANAGED_BASE_IMAGE_IDS`), in the ARN form
/// an AWS-owned resource of this service carries (`aws` in the account field).
const BASE_IMAGE_ARN: &str = "arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1";

/// A build role ARN: `RoleArn`, pattern
/// `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const BUILD_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/microvm-build";

/// An execution role ARN, same `RoleArn` shape as the build role.
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/microvm-exec";

/// Every `CreateMicrovmImage` input member that is also an output member. The response
/// is the stored record, so each one the request carried must come back unchanged.
const ECHOED_IMAGE_MEMBERS: &[&str] = &[
    "name",
    "baseImageArn",
    "baseImageVersion",
    "buildRoleArn",
    "description",
    "codeArtifact",
    "logging",
    "egressNetworkConnectors",
    "cpuConfigurations",
    "resources",
    "additionalOsCapabilities",
    "hooks",
    "environmentVariables",
    "tags",
];

/// A fresh emulator holding only this service, over one shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator as [`emulator`], with the shared engine kept as well — so a suite
/// that needs the store the handlers reach (the MicroVM endpoint suite) gets the one
/// engine rather than building a second harness.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambdamicrovms_lambdamicrovms::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One request against the emulator; the SigV4 credential scope is what routes it to
/// this service's `lambda` signing name and names the region its ARNs carry.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(router, REGION, method, uri, body).await
}

/// The same request as [`call`], signed for `region` — so a suite can assert which
/// region the ARNs of an answer carry.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://lambda.{region}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20260101/{region}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// A `CreateMicrovmImage` body: the four required members, plus whatever `extra` adds.
pub(crate) fn image_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "name": name,
        "baseImageArn": BASE_IMAGE_ARN,
        "buildRoleArn": BUILD_ROLE_ARN,
        "codeArtifact": {"uri": "s3://artifacts/app.zip"},
    });
    if let Value::Object(members) = extra {
        let map = body.as_object_mut().expect("the body is an object");
        for (key, value) in members {
            map.insert(key, value);
        }
    }
    body
}

/// The ARN `CreateMicrovmImage` must mint for `name`.
pub(crate) fn expected_image_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:microvm-image:{name}")
}

/// The ARN this service mints for a MicroVM — the other form `MicrovmIdentifier`
/// accepts ("The ARN or ID of the MicroVm").
pub(crate) fn expected_microvm_arn(microvm_id: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:microvm:{microvm_id}")
}

/// Create an image and wait out its asynchronous build, so it has an active version.
pub(crate) async fn built_image(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, json!({}))),
    )
    .await;
    assert_eq!(status, 201, "create image {name}: got {body}");
    tokio::time::sleep(PAST_THE_SPAN).await;
    body["imageArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Run a MicroVM from `image_identifier` and return the whole response body.
pub(crate) async fn run_microvm(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/2025-09-09/microvms", Some(body)).await
}

/// RULE: `CreateMicrovmImage` mints and persists an image whose ARN is
/// `arn:aws:lambda:<region>:<account>:microvm-image:<name>`, and the response is that
/// stored record — so every input member that is also an output member comes back
/// unchanged, and a later read by name OR by ARN returns the same record.
#[tokio::test]
async fn created_images_are_persisted_and_read_back_by_name_or_arn() {
    let router = emulator();

    // `ImageName` is len 1..64, pattern ^[a-zA-Z0-9-_]+$ (from the model), so the rows
    // span the name's published bounds and punctuation as well as which optional
    // members the request carries and which arm each union takes.
    let longest = format!("max-length-name-{}", "x".repeat(48));
    let rows: Vec<(&str, String, Value)> = vec![
        ("required members only", "a".to_string(), json!({})),
        (
            "name at the 64-character maximum",
            longest.clone(),
            json!({}),
        ),
        (
            "underscores, hyphens and digits",
            "img_0-1_2".to_string(),
            json!({"description": "a described image"}),
        ),
        (
            "every optional member, cloudWatch logging",
            "fully-specified".to_string(),
            json!({
                "baseImageVersion": "7",
                "description": "fully specified",
                "logging": {"cloudWatch": {"logGroup": "/aws/microvm/demo"}},
                "egressNetworkConnectors": ["connector-egress"],
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 2048}],
                "additionalOsCapabilities": ["ALL"],
                "hooks": {"port": 8080},
                "environmentVariables": {"STAGE": "prod", "TIER": "web"},
                "tags": {"team": "platform", "cost-centre": "1234"},
            }),
        ),
        (
            "logging explicitly disabled",
            "logging-off".to_string(),
            json!({"logging": {"disabled": {}}}),
        ),
    ];

    // Phase 1: create every row. One asynchronous build span covers them all, so the
    // suite waits it out ONCE rather than once per row.
    let mut created_rows: Vec<(&str, String, Value)> = Vec::new();
    for (case, name, extra) in rows {
        let request = image_body(&name, extra);
        let (status, created) =
            call(&router, "POST", "/2025-09-09/microvm-images", Some(request.clone())).await;

        assert_eq!(status, 201, "case {case:?}: expected 201, got {status} {created}");
        assert_eq!(
            created["imageArn"].as_str(),
            Some(expected_image_arn(&name).as_str()),
            "case {case:?}: expected the minted image ARN, got {:?}",
            created["imageArn"]
        );
        assert_eq!(
            created["state"].as_str(),
            Some("CREATING"),
            "case {case:?}: a build is asynchronous, so the image starts CREATING, got {:?}",
            created["state"]
        );
        assert_eq!(
            created["imageVersion"].as_str(),
            Some("1.0"),
            "case {case:?}: expected the first image version, got {:?}",
            created["imageVersion"]
        );
        assert!(
            created["latestActiveImageVersion"].is_null(),
            "case {case:?}: no version is active until the build completes, got {:?}",
            created["latestActiveImageVersion"]
        );
        assert!(
            created["createdAt"].is_number(),
            "case {case:?}: expected a createdAt timestamp, got {:?}",
            created["createdAt"]
        );
        for member in ECHOED_IMAGE_MEMBERS {
            if let Some(sent) = request.get(*member) {
                assert_eq!(
                    created.get(*member),
                    Some(sent),
                    "case {case:?}: member {member} must come back from the stored record, got {:?}",
                    created.get(*member)
                );
            }
        }
        created_rows.push((case, name, created));
    }

    tokio::time::sleep(PAST_THE_SPAN).await;

    // Phase 2: every stored image reads back under either form
    // `MicrovmImageIdentifier` documents: "The ARN or ID of the MicroVmImage".
    for (case, name, created) in &created_rows {
        let by_name = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvm-images/{name}"),
            None,
        )
        .await;
        let arn = expected_image_arn(name);
        let by_arn = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvm-images/{}", as_uri_value(&arn)),
            None,
        )
        .await;

        assert_eq!(by_name.0, 200, "case {case:?}: get by name, got {:?}", by_name);
        assert_eq!(by_arn.0, 200, "case {case:?}: get by ARN, got {:?}", by_arn);
        assert_eq!(
            by_name.1, by_arn.1,
            "case {case:?}: the ARN and the name must resolve to the same record"
        );
        assert_eq!(
            by_name.1["state"].as_str(),
            Some("CREATED"),
            "case {case:?}: the build is due, so the image is CREATED, got {:?}",
            by_name.1["state"]
        );
        assert_eq!(
            by_name.1["latestActiveImageVersion"].as_str(),
            Some("1.0"),
            "case {case:?}: the completed build becomes the active version, got {:?}",
            by_name.1["latestActiveImageVersion"]
        );
        assert_eq!(
            by_name.1["createdAt"], created["createdAt"],
            "case {case:?}: createdAt is stable across reads"
        );
        assert_eq!(
            by_name.1.get("tags"),
            Some(&created.get("tags").cloned().unwrap_or_else(|| json!({}))),
            "case {case:?}: the image's tags are persisted, got {:?}",
            by_name.1.get("tags")
        );
    }
}

/// RULE: the image build is asynchronous — a read taken straight after the create still
/// observes `CREATING` with no active version, and only a later read observes `CREATED`.
#[tokio::test]
async fn an_image_build_is_asynchronous_and_observable_in_both_states() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body("async-build", json!({}))),
    )
    .await;
    assert_eq!(status, 201, "create: got {created}");

    let (status, pending) = call(
        &router,
        "GET",
        "/2025-09-09/microvm-images/async-build",
        None,
    )
    .await;
    assert_eq!(status, 200, "read during the build: got {pending}");
    assert_eq!(
        pending["state"].as_str(),
        Some("CREATING"),
        "a read taken straight after the create still sees the transient state, got {:?}",
        pending["state"]
    );
    assert!(
        pending["latestActiveImageVersion"].is_null(),
        "no version is active mid-build, got {:?}",
        pending["latestActiveImageVersion"]
    );

    tokio::time::sleep(PAST_THE_SPAN).await;

    let (status, done) = call(
        &router,
        "GET",
        "/2025-09-09/microvm-images/async-build",
        None,
    )
    .await;
    assert_eq!(status, 200, "read after the build: got {done}");
    assert_eq!(
        done["state"].as_str(),
        Some("CREATED"),
        "the build the clock made due is materialised, got {:?}",
        done["state"]
    );
    assert_eq!(
        done["latestActiveImageVersion"].as_str(),
        Some("1.0"),
        "the completed build becomes the active version, got {:?}",
        done["latestActiveImageVersion"]
    );
}

/// RULE: an image `name` is unique within the account — a second create of the same
/// name is refused 400 `ValidationException` naming the taken name — while a repeat of
/// the SAME `clientToken` returns the already-created image instead of minting a second
/// one.
#[tokio::test]
async fn duplicate_names_conflict_and_a_client_token_replays() {
    let router = emulator();

    // Rows differ in whether the first create carried a token, and in whether the
    // second request repeats that token, uses another, or carries none.
    let (status, first) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body("dup", json!({"clientToken": "token-one"}))),
    )
    .await;
    assert_eq!(status, 201, "first create: got {first}");

    let replays: Vec<(&str, Value, u16)> = vec![
        (
            "the same client token replays",
            image_body("dup", json!({"clientToken": "token-one"})),
            201,
        ),
        (
            "the same token with different members still replays",
            image_body(
                "dup",
                json!({"clientToken": "token-one", "description": "changed"}),
            ),
            201,
        ),
        (
            "a different token on the same name conflicts",
            image_body("dup", json!({"clientToken": "token-two"})),
            400,
        ),
        (
            "no token on the same name conflicts",
            image_body("dup", json!({})),
            400,
        ),
    ];

    for (case, body, expected) in replays {
        let (status, response) = call(&router, "POST", "/2025-09-09/microvm-images", Some(body)).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {response}"
        );
        if expected == 201 {
            assert_eq!(
                response["imageArn"], first["imageArn"],
                "case {case:?}: a replay must return the first image, got {:?}",
                response["imageArn"]
            );
            assert_eq!(
                response["createdAt"], first["createdAt"],
                "case {case:?}: a replay performs no further action, got {:?}",
                response["createdAt"]
            );
        } else {
            assert_eq!(
                response["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {response}"
            );
            assert_eq!(
                response["message"].as_str(),
                Some("A MicroVM image with the name 'dup' already exists in this account"),
                "case {case:?}: the refusal must name the taken name, got {:?}",
                response["message"]
            );
        }
    }

    // Exactly one image exists under that name, and it kept the first description.
    let (_, stored) = call(&router, "GET", "/2025-09-09/microvm-images/dup", None).await;
    assert_eq!(
        stored["imageArn"], first["imageArn"],
        "the store still holds exactly the first image, got {stored}"
    );
}

/// RULE: `RunMicrovm` resolves the image, then mints and persists a MicroVM whose
/// stored record is what both `RunMicrovm` and `GetMicrovm` report; it starts `PENDING`
/// and the clock moves it to `RUNNING`.
#[tokio::test]
async fn running_a_microvm_persists_it_and_get_returns_the_stored_record() {
    let router = emulator();
    let image_arn = built_image(&router, "runnable").await;

    // Rows differ along the identifier form, whether a version is named explicitly,
    // which optional members are present, and the `maximumDurationInSeconds` supplied.
    let rows: Vec<(&str, Value, i64)> = vec![
        (
            "image by name, every default",
            json!({"imageIdentifier": "runnable"}),
            28_800,
        ),
        (
            "image by ARN",
            json!({"imageIdentifier": image_arn.clone()}),
            28_800,
        ),
        (
            "explicit image version",
            json!({"imageIdentifier": "runnable", "imageVersion": "1.0"}),
            28_800,
        ),
        (
            "every optional member",
            json!({
                "imageIdentifier": "runnable",
                "executionRoleArn": EXECUTION_ROLE_ARN,
                "idlePolicy": {
                    "maxIdleDurationSeconds": 120,
                    "suspendedDurationSeconds": 0,
                    "autoResumeEnabled": true,
                },
                "logging": {"disabled": {}},
                "ingressNetworkConnectors": ["ingress-a", "ingress-b"],
                "egressNetworkConnectors": ["egress-a"],
                "runHookPayload": "{}",
                "maximumDurationInSeconds": 600,
            }),
            600,
        ),
    ];

    for (case, body, expected_duration) in rows {
        let (status, run) = run_microvm(&router, body.clone()).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {run}");

        let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();
        assert!(
            microvm_id.starts_with("mvm-"),
            "case {case:?}: expected a minted microvmId, got {:?}",
            run["microvmId"]
        );
        assert_eq!(
            run["state"].as_str(),
            Some("PENDING"),
            "case {case:?}: a MicroVM starts PENDING, got {:?}",
            run["state"]
        );
        let endpoint = run["endpoint"].as_str().unwrap_or_default();
        // The endpoint is the address this emulator publishes the MicroVM on: its own
        // HTTPS listener, with the MicroVM named in the path. What that address answers
        // is pinned in `microvm_endpoint_data_plane`.
        assert!(
            endpoint.starts_with("https://")
                && endpoint.ends_with(&format!("/microvms/{microvm_id}")),
            "case {case:?}: expected an HTTPS endpoint naming this MicroVM, got {:?}",
            run["endpoint"]
        );
        assert_eq!(
            run["imageArn"].as_str(),
            Some(image_arn.as_str()),
            "case {case:?}: the identifier resolves to the stored image ARN, got {:?}",
            run["imageArn"]
        );
        assert_eq!(
            run["imageVersion"].as_str(),
            Some("1.0"),
            "case {case:?}: expected the image's active version, got {:?}",
            run["imageVersion"]
        );
        assert_eq!(
            run["maximumDurationInSeconds"].as_i64(),
            Some(expected_duration),
            "case {case:?}: expected maximumDurationInSeconds {expected_duration}, got {:?}",
            run["maximumDurationInSeconds"]
        );
        assert!(
            run["startedAt"].is_number(),
            "case {case:?}: expected a startedAt timestamp, got {:?}",
            run["startedAt"]
        );
        assert!(
            run["terminatedAt"].is_null(),
            "case {case:?}: a running MicroVM has no terminatedAt, got {:?}",
            run["terminatedAt"]
        );
        for member in [
            "executionRoleArn",
            "idlePolicy",
            "ingressNetworkConnectors",
            "egressNetworkConnectors",
        ] {
            if let Some(sent) = body.get(member) {
                assert_eq!(
                    run.get(member),
                    Some(sent),
                    "case {case:?}: member {member} must come back from the stored record, got {:?}",
                    run.get(member)
                );
            }
        }

        // The reader sees exactly what the create persisted.
        let (status, got) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms/{microvm_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: get: got {status} {got}");
        assert_eq!(
            got, run,
            "case {case:?}: GetMicrovm must return the record RunMicrovm stored"
        );

        tokio::time::sleep(PAST_THE_SPAN).await;
        let (_, running) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms/{microvm_id}"),
            None,
        )
        .await;
        assert_eq!(
            running["state"].as_str(),
            Some("RUNNING"),
            "case {case:?}: provisioning the clock made due is materialised, got {:?}",
            running["state"]
        );
        assert_eq!(
            running["startedAt"], run["startedAt"],
            "case {case:?}: startedAt is stable across reads"
        );
    }
}

/// RULE: a repeat of the same `RunMicrovm` `clientToken` returns the MicroVM that token
/// already minted rather than starting a second one.
#[tokio::test]
async fn a_run_microvm_client_token_replay_returns_the_same_microvm() {
    let router = emulator();
    built_image(&router, "replayable").await;

    let body = json!({"imageIdentifier": "replayable", "clientToken": "run-token"});
    let (first_status, first) = run_microvm(&router, body.clone()).await;
    let (second_status, second) = run_microvm(&router, body).await;

    assert_eq!(first_status, 200, "first run: got {first}");
    assert_eq!(second_status, 200, "replayed run: got {second}");
    assert_eq!(
        first["microvmId"], second["microvmId"],
        "a clientToken replay must return the same microvmId, got {:?} then {:?}",
        first["microvmId"], second["microvmId"]
    );

    let (_, listed) = call(&router, "GET", "/2025-09-09/microvms", None).await;
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(1),
        "a replay must not start a second MicroVM, got {listed}"
    );
}

/// RULE: `maximumDurationInSeconds` is accepted across its published range (1..28800)
/// and refused outside it with a 400 `ValidationException`.
#[tokio::test]
async fn maximum_duration_is_enforced_at_its_published_bounds() {
    let router = emulator();
    built_image(&router, "bounded").await;

    // The range comes from `RunMicrovmRequest$maximumDurationInSeconds`: both
    // published bounds, one value inside, and one past each bound.
    let rows: Vec<(&str, Value, u16)> = vec![
        ("the published minimum", json!(1), 200),
        ("inside the range", json!(3_600), 200),
        ("the published maximum", json!(28_800), 200),
        ("one below the minimum", json!(0), 400),
        ("one past the maximum", json!(28_801), 400),
        ("negative", json!(-1), 400),
    ];

    for (case, duration, expected) in rows {
        let (status, response) = run_microvm(
            &router,
            json!({"imageIdentifier": "bounded", "maximumDurationInSeconds": duration}),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected} for {duration}, got {status} {response}"
        );
        if expected == 200 {
            assert_eq!(
                response["maximumDurationInSeconds"], duration,
                "case {case:?}: the accepted duration is stored, got {:?}",
                response["maximumDurationInSeconds"]
            );
        }
    }
}

/// RULE: `maximumDurationInSeconds` is "the maximum duration in seconds that the MicroVM
/// can exist before being terminated by the platform" — once it has elapsed, the stored
/// MicroVM is `TERMINATED` with a reason saying so.
#[tokio::test]
async fn a_microvm_that_outlives_its_maximum_duration_is_terminated_by_the_platform() {
    let router = emulator();
    built_image(&router, "short-lived").await;

    let (status, run) = run_microvm(
        &router,
        json!({"imageIdentifier": "short-lived", "maximumDurationInSeconds": 1}),
    )
    .await;
    assert_eq!(status, 200, "run: got {run}");
    let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();

    tokio::time::sleep(PAST_THE_MAXIMUM_DURATION).await;

    let (status, expired) = call(
        &router,
        "GET",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "read after the maximum duration: got {expired}");
    assert_eq!(
        expired["state"].as_str(),
        Some("TERMINATED"),
        "the platform terminates a MicroVM past its maximum duration, got {:?}",
        expired["state"]
    );
    assert_eq!(
        expired["stateReason"].as_str(),
        Some("Platform.MaximumDurationExceeded"),
        "expected the platform's termination reason, got {:?}",
        expired["stateReason"]
    );
    assert!(
        expired["terminatedAt"].is_number(),
        "a terminated MicroVM carries terminatedAt, got {:?}",
        expired["terminatedAt"]
    );
}

/// RULE: `TerminateMicrovm` moves the stored MicroVM out of `RUNNING` and records
/// `terminatedAt`, so a later read and a listing both observe it; and being
/// `@idempotent`, a repeat succeeds and changes nothing.
#[tokio::test]
async fn terminate_is_observable_and_idempotent() {
    let router = emulator();
    built_image(&router, "terminable").await;

    let (_, run) = run_microvm(&router, json!({"imageIdentifier": "terminable"})).await;
    let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();
    tokio::time::sleep(PAST_THE_SPAN).await;

    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "terminate: got {status} {body}");

    let (status, ending) = call(
        &router,
        "GET",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "read after terminate: got {ending}");
    assert_eq!(
        ending["state"].as_str(),
        Some("TERMINATING"),
        "terminate moves the MicroVM out of RUNNING at once, got {:?}",
        ending["state"]
    );
    assert!(
        ending["terminatedAt"].is_null(),
        "terminatedAt is stamped when the MicroVM REACHES TERMINATED, got {:?}",
        ending["terminatedAt"]
    );
    assert_eq!(
        ending["stateReason"].as_str(),
        Some("Client.UserInitiatedTermination"),
        "expected the caller-initiated termination reason, got {:?}",
        ending["stateReason"]
    );

    // `@idempotent`: terminating again succeeds and leaves the record alone.
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "second terminate: got {status} {body}");
    let (_, after) = call(
        &router,
        "GET",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(
        after["state"], ending["state"],
        "a repeated terminate must not move the state on, got {:?}",
        after["state"]
    );
    assert_eq!(
        after["stateReason"], ending["stateReason"],
        "a repeated terminate must not rewrite stateReason, got {:?}",
        after["stateReason"]
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    let (_, terminated) = call(
        &router,
        "GET",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(
        terminated["state"].as_str(),
        Some("TERMINATED"),
        "the TERMINATING -> TERMINATED move the clock made due is materialised, got {:?}",
        terminated["state"]
    );
    assert!(
        terminated["terminatedAt"].is_number(),
        "a MicroVM that reached TERMINATED carries terminatedAt, got {:?}",
        terminated["terminatedAt"]
    );

    // A third terminate, now of an already-terminated MicroVM, still succeeds.
    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "terminating an already-terminated MicroVM must succeed");
    let (_, again) = call(
        &router,
        "GET",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(
        again["terminatedAt"], terminated["terminatedAt"],
        "terminating an already-terminated MicroVM must not move terminatedAt, got {:?}",
        again["terminatedAt"]
    );

    let (_, listed) = call(&router, "GET", "/2025-09-09/microvms", None).await;
    let states: Vec<&str> = listed["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["state"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        states,
        vec!["TERMINATED"],
        "the listing reports the terminated MicroVM's new state, got {listed}"
    );
}

/// RULE: `ListMicrovms` serves the store, filtered by `imageIdentifier` /
/// `imageVersion`, in pages of at most `maxResults` with a `nextToken` while more
/// remain — and the token it returns is the one it accepts.
#[tokio::test]
async fn list_microvms_filters_and_pages_over_the_store() {
    let router = emulator();
    let first_arn = built_image(&router, "image-one").await;
    let second_arn = built_image(&router, "image-two").await;

    let mut on_first: Vec<String> = Vec::new();
    for _ in 0..3 {
        let (_, run) = run_microvm(&router, json!({"imageIdentifier": "image-one"})).await;
        on_first.push(run["microvmId"].as_str().unwrap_or_default().to_string());
    }
    let (_, other) = run_microvm(&router, json!({"imageIdentifier": "image-two"})).await;
    let on_second = other["microvmId"].as_str().unwrap_or_default().to_string();

    // Rows span the filter forms and the expected membership of each.
    let filters: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter", String::new(), {
            let mut all = on_first.clone();
            all.push(on_second.clone());
            all.sort();
            all
        }),
        (
            "filter by image name",
            "?imageIdentifier=image-one".to_string(),
            on_first.clone(),
        ),
        (
            "filter by image ARN",
            format!("?imageIdentifier={}", as_uri_value(&first_arn)),
            on_first.clone(),
        ),
        (
            "filter by the other image's ARN",
            format!("?imageIdentifier={}", as_uri_value(&second_arn)),
            vec![on_second.clone()],
        ),
        (
            "filter by image and version",
            "?imageIdentifier=image-one&imageVersion=1.0".to_string(),
            on_first.clone(),
        ),
        (
            "filter by a version nothing runs",
            "?imageIdentifier=image-one&imageVersion=9".to_string(),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in filters {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms{query}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let ids: Vec<String> = body["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["microvmId"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a page that exhausted the matches returns no nextToken, got {:?}",
            body["nextToken"]
        );
    }

    // `maxResults` caps the page, and following the `nextToken` walks the rest.
    for page_size in [1_usize, 2, 3] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(next) => format!("?imageIdentifier=image-one&maxResults={page_size}&nextToken={next}"),
                None => format!("?imageIdentifier=image-one&maxResults={page_size}"),
            };
            let (status, body) =
                call(&router, "GET", &format!("/2025-09-09/microvms{query}"), None).await;
            assert_eq!(status, 200, "page size {page_size}: got {status} {body}");
            let items = body["items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "page size {page_size}: a page must not exceed maxResults, got {} items",
                items.len()
            );
            for item in &items {
                seen.push(item["microvmId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(pages <= 5, "page size {page_size}: pagination did not terminate");
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, on_first,
            "page size {page_size}: paging must visit every match exactly once, got {seen:?}"
        );
    }

    // A token this service did not issue is refused, not silently ignored.
    let (status, body) = call(
        &router,
        "GET",
        "/2025-09-09/microvms?nextToken=not-a-token",
        None,
    )
    .await;
    assert_eq!(status, 400, "a malformed nextToken must be refused, got {status} {body}");
    assert_eq!(
        body["__type"].as_str(),
        Some("ValidationException"),
        "expected ValidationException, got {body}"
    );

    // `maxResults` carries its own `@range(min: 1, max: 50)`
    // (`smithy_query(query="operation",operation="ListMicrovms")` reports
    // `maxResults` as `{"target": "Integer", "httpQuery": "maxResults",
    // "range": {"min": 1, "max": 50}}`), so each published bound is served and a
    // value past either bound is refused.
    for (case, max_results, expected) in [
        ("the published minimum", "1", 200),
        ("the published maximum", "50", 200),
        ("one below the minimum", "0", 400),
        ("one past the maximum", "51", 400),
        ("negative", "-1", 400),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms?maxResults={max_results}"),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: maxResults={max_results} expected {expected}, got {status} {body}"
        );
    }
}

/// RULE: an identifier naming no stored resource answers 404 `ResourceNotFoundException`
/// — never a 501, and never a hollow 200.
#[tokio::test]
async fn unknown_identifiers_answer_resource_not_found() {
    let router = emulator();
    built_image(&router, "present").await;

    // An image this account created and then deleted: the identifier names a record the
    // account holds, so its lookup reports that record's absence.
    let gone_arn = built_image(&router, "gone").await;
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(&gone_arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "delete the gone image: got {status} {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    // Rows span every operation in the slice that resolves an identifier, and both
    // identifier forms. An IMAGE identifier that is neither an id this account holds
    // nor a well-formed ARN is a REFUSAL rather than a not-found, and one this account
    // holds no record of is the authorization refusal (see
    // `image_request_validation::an_image_lookup_separates_a_malformed_identifier_from_an_absent_one`),
    // so a missing image is addressed here by the ARN of a record this account owns.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "get a deleted image by ARN",
            "GET",
            format!("/2025-09-09/microvm-images/{}", as_uri_value(&gone_arn)),
        ),
        (
            "get an absent MicroVM",
            "GET",
            "/2025-09-09/microvms/mvm-00000000deadbeef".to_string(),
        ),
        (
            "terminate an absent MicroVM",
            "DELETE",
            "/2025-09-09/microvms/mvm-00000000deadbeef".to_string(),
        ),
    ];

    for (case, method, uri) in rows {
        let (status, body) = call(&router, method, &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // Running from an absent image, or a version of a present image that is not its
    // active one, is the same 404.
    for (case, body) in [
        ("absent image", json!({"imageIdentifier": "absent"})),
        (
            "absent version of a present image",
            json!({"imageIdentifier": "present", "imageVersion": "9"}),
        ),
    ] {
        let (status, response) = run_microvm(&router, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {response}"
        );
        assert_eq!(
            response["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
    }
}

/// RULE: a MicroVM runs a BUILT image version. With no `imageVersion` named, the
/// precondition is the resolution of the image's ACTIVE version, so an image whose build
/// has not finished has none and the refusal is the 404 that says so; naming a version
/// that EXISTS but is not active is instead a conflict with that version's current state
/// (409). Once the build is due, both succeed.
#[tokio::test]
async fn running_from_an_unbuilt_image_is_refused_until_a_version_is_active() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body("still-building", json!({}))),
    )
    .await;
    assert_eq!(status, 201, "create: got {created}");

    let (status, response) = run_microvm(&router, json!({"imageIdentifier": "still-building"})).await;
    assert_eq!(
        status, 404,
        "a CREATING image has no active version to resolve, got {status} {response}"
    );
    assert_eq!(
        response["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {response}"
    );
    assert_eq!(
        response["message"].as_str(),
        Some(
            format!(
                "No active version found for MicroVM image {}",
                expected_image_arn("still-building")
            )
            .as_str()
        ),
        "the 404 must name the active-version miss, got {response}"
    );

    let (status, response) = run_microvm(
        &router,
        json!({"imageIdentifier": "still-building", "imageVersion": "1.0"}),
    )
    .await;
    assert_eq!(
        status, 409,
        "a version that exists but is not active conflicts, got {status} {response}"
    );
    assert_eq!(
        response["__type"].as_str(),
        Some("ConflictException"),
        "expected ConflictException, got {response}"
    );

    // Once the build is due, the same requests succeed.
    tokio::time::sleep(PAST_THE_SPAN).await;
    for (case, body) in [
        ("the active version", json!({"imageIdentifier": "still-building"})),
        (
            "that version named explicitly",
            json!({"imageIdentifier": "still-building", "imageVersion": "1.0"}),
        ),
    ] {
        let (status, response) = run_microvm(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: succeeds once the build completed, got {status} {response}"
        );
    }
}

/// RULE: the store is BOUNDED — the account's image and concurrent-MicroVM quotas are
/// refused with the modelled `ServiceQuotaExceededException` (402) rather than letting a
/// caller grow the store without limit.
#[tokio::test]
async fn the_account_quotas_are_enforced_with_the_modelled_402() {
    use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
        MAX_ACTIVE_MICROVMS, MAX_MICROVM_IMAGES,
    };

    let router = emulator();
    for index in 0..MAX_MICROVM_IMAGES {
        let (status, body) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&format!("img-{index}"), json!({}))),
        )
        .await;
        assert_eq!(status, 201, "image {index}: got {status} {body}");
    }
    let (status, body) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body("one-image-too-many", json!({}))),
    )
    .await;
    assert_eq!(
        status, 402,
        "one image past the quota must be refused, got {status} {body}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some("ServiceQuotaExceededException"),
        "expected ServiceQuotaExceededException, got {body}"
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    for index in 0..MAX_ACTIVE_MICROVMS {
        let (status, body) = run_microvm(&router, json!({"imageIdentifier": "img-0"})).await;
        assert_eq!(status, 200, "microvm {index}: got {status} {body}");
    }
    let (status, body) = run_microvm(&router, json!({"imageIdentifier": "img-0"})).await;
    assert_eq!(
        status, 402,
        "one MicroVM past the quota must be refused, got {status} {body}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some("ServiceQuotaExceededException"),
        "expected ServiceQuotaExceededException, got {body}"
    );
}

/// RULE: an identifier resolves ONLY to the resource whose stored ARN it is (or whose
/// id it is), so an ARN this service never minted — another partition's, region's,
/// account's or resource type's — resolves to nothing rather than to the local resource
/// whose id its last segment repeats.
#[tokio::test]
async fn identifiers_resolve_by_id_or_by_the_stored_arn_only() {
    let router = emulator();
    built_image(&router, "addressable").await;
    let (_, run) = run_microvm(&router, json!({"imageIdentifier": "addressable"})).await;
    let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();

    // Each row is an identifier form and the status the readers must answer with. The
    // resolving rows come first: a 404 row is also sent to the mutating resolver.
    let rows: Vec<(&str, String, u16)> = vec![
        ("the id", microvm_id.clone(), 200),
        (
            "the ARN this service minted",
            expected_microvm_arn(&microvm_id),
            200,
        ),
        (
            "another partition's ARN with the same id",
            format!("arn:aws-cn:lambda:cn-north-1:{ACCOUNT}:microvm:{microvm_id}"),
            404,
        ),
        (
            "another account's ARN with the same id",
            format!("arn:aws:lambda:{REGION}:999999999999:microvm:{microvm_id}"),
            404,
        ),
        (
            "another region's ARN with the same id",
            format!("arn:aws:lambda:eu-west-1:{ACCOUNT}:microvm:{microvm_id}"),
            404,
        ),
        (
            "another resource type's ARN with the same last segment",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{microvm_id}"),
            404,
        ),
    ];

    for (case, identifier, expected) in rows {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms/{}", as_uri_value(&identifier)),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: GetMicrovm expected {expected}, got {status} {body}"
        );
        if expected == 200 {
            assert_eq!(
                body["microvmId"].as_str(),
                Some(microvm_id.as_str()),
                "case {case:?}: expected the stored MicroVM, got {body}"
            );
            continue;
        }
        // The mutating resolver refuses the same identifier, so no foreign ARN can
        // terminate a local MicroVM.
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/2025-09-09/microvms/{}", as_uri_value(&identifier)),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: TerminateMicrovm expected {expected}, got {status} {body}"
        );
    }

    // And the MicroVM the foreign ARNs named is untouched.
    let (status, after) = call(
        &router,
        "GET",
        &format!("/2025-09-09/microvms/{microvm_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the local MicroVM still reads back, got {after}");
    assert!(
        after["terminatedAt"].is_null(),
        "no foreign ARN may terminate the local MicroVM, got {:?}",
        after["terminatedAt"]
    );
}

/// An ARN (or an id) as a URI path segment or query value.
///
/// Every character an identifier this service mints carries — ASCII letters and digits,
/// `-`, `_`, `.` and `:` — is a legal `pchar` (RFC 3986 §3.3) and a legal query
/// character, so none of them needs escaping and what a test sends is what the server
/// reads back. The assertion keeps that true: a value that WOULD need escaping fails
/// loudly here instead of being sent raw.
pub(crate) fn as_uri_value(value: &str) -> &str {
    assert!(
        value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')),
        "an identifier used in a URI must need no escaping, got {value:?}"
    );
    value
}
