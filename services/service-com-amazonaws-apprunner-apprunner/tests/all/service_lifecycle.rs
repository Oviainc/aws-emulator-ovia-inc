// SPDX-License-Identifier: CC0-1.0

//! The App Runner *service* resource lifecycle, end to end over the wire.
//!
//! Every case here drives the real `awsJson1_0` surface through the
//! `MultiServiceRouter` (`POST /` with `X-Amz-Target: AppRunner.<Operation>`),
//! so what is asserted is what a client sees: status code, error code, and the
//! JSON body computed from the persisted record.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_apprunner_apprunner::state;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test requests are signed for. Deliberately NOT the emulator's
/// default, so every ARN and service URL asserted below proves the region came
/// from the request's SigV4 scope rather than from a constant.
const REGION: &str = "eu-west-2";

/// The canonical account id every emulator ARN carries.
const ACCOUNT: &str = "000000000000";

/// How long a test waits for an asynchronous App Runner operation to fall due.
/// Derived from the emulator's own span so the two cannot drift apart.
fn settle_wait() -> Duration {
    state::OPERATION_SETTLE_DELAY + Duration::from_millis(250)
}

/// Wait past the due instant of an operation started just now. A blocking sleep:
/// nothing else is in flight in these tests, and the emulator never sleeps for a
/// due instant itself.
fn wait_for_settle() {
    std::thread::sleep(settle_wait());
}

/// Move a service's pending transition's due instant, so a state-machine case is
/// asserted deterministically instead of racing the 500 ms wall clock.
///
/// This writes only the instant the emulator itself stores; the transition is
/// still materialised by `settle_due` on the next request, exactly as it is in
/// production.
fn move_due(engine: &Arc<engine::InMemoryEngine>, service_arn: &str, due_at: SystemTime) {
    use service_com_amazonaws_apprunner_apprunner::state::ComAmazonawsApprunnerApprunnerState;

    let store = engine.state::<ComAmazonawsApprunnerApprunnerState>();
    let mut store = store.lock();
    let pending = store
        .services
        .get_mut(service_arn)
        .and_then(|record| record.pending.as_mut())
        .unwrap_or_else(|| panic!("{service_arn} has no operation in flight"));
    pending.due_at = due_at;
    store.expect_due(due_at);
}

/// Hold a service's operation in flight for the rest of the test, so a call that
/// must see the transient state cannot race the clock.
fn hold_in_flight(engine: &Arc<engine::InMemoryEngine>, service_arn: &str) {
    move_due(
        engine,
        service_arn,
        SystemTime::now() + Duration::from_secs(3600),
    );
}

/// Make a service's operation due now, so the next request materialises it.
fn make_due_now(engine: &Arc<engine::InMemoryEngine>, service_arn: &str) {
    move_due(
        engine,
        service_arn,
        SystemTime::now() - Duration::from_millis(1),
    );
}

/// A router with just this service mounted, over a fresh engine, so each test
/// gets its own store.
fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// As [`router`], but also hands back the engine, so a test can assert what the
/// store holds as well as what the wire returns.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise handler behaviour, not signature verification.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apprunner_apprunner::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Call one operation and return `(status, body)`, signed for `REGION`.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// As [`call`], signed for `region` — the Region the handler scopes the request
/// to.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AppRunner.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/apprunner/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or_else(|error| {
            panic!(
                "operation {operation}: body is not JSON ({error}): {}",
                String::from_utf8_lossy(bytes)
            )
        })
    };
    (status, parsed)
}

/// The error code (`__type`) of a failed call.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .rsplit('#')
        .next()
        .unwrap_or("")
        .to_string()
}

/// A minimal valid `SourceConfiguration`: a public ECR image.
fn ecr_public_source() -> Value {
    json!({
        "ImageRepository": {
            "ImageIdentifier": "public.ecr.aws/aws-containers/hello-app-runner:latest",
            "ImageRepositoryType": "ECR_PUBLIC"
        }
    })
}

/// Create a service and return its successful response body.
async fn create_ok(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(
        router,
        "CreateService",
        json!({ "ServiceName": name, "SourceConfiguration": ecr_public_source() }),
    )
    .await;
    assert_eq!(status, 200, "create {name}: expected 200, got {body:?}");
    body
}

fn service_of(body: &Value) -> &Value {
    body.get("Service")
        .unwrap_or_else(|| panic!("response carries no Service: {body:?}"))
}

fn str_at(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member {member} is absent or not a string in {value:?}"))
        .to_string()
}

/// `CreateService` mints a record, and `DescribeService` returns that same
/// record: the ids, the ARN, the URL, the timestamps and the stored
/// configuration all come out of the store, and the status moves from
/// `OPERATION_IN_PROGRESS` to `RUNNING` when the clock owes it.
#[tokio::test]
async fn create_persists_a_service_that_describe_reads_back() {
    let router = router();
    let created = create_ok(&router, "my-web-svc").await;
    let service = service_of(&created);

    let service_id = str_at(service, "ServiceId");
    let service_arn = str_at(service, "ServiceArn");
    let operation_id = str_at(&created, "OperationId");

    // `ServiceId`: the model gives it length(32,32); App Runner answers with the
    // 32-hex-digit form.
    assert_eq!(
        service_id.len(),
        32,
        "ServiceId must be 32 characters, got {service_id:?}"
    );
    assert!(
        service_id.chars().all(|c| c.is_ascii_hexdigit()),
        "ServiceId must be hex digits, got {service_id:?}"
    );
    // `OperationId` targets the model's `UUID` shape: 36 chars, hyphenated,
    // version 4 and the 8/9/a/b variant.
    assert_eq!(
        operation_id.len(),
        36,
        "OperationId must be 36 characters, got {operation_id:?}"
    );
    let fields: Vec<&str> = operation_id.split('-').collect();
    assert_eq!(
        fields.iter().map(|f| f.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "OperationId must be a hyphenated UUID, got {operation_id:?}"
    );
    assert!(
        fields[2].starts_with('4'),
        "OperationId must be a version-4 UUID, got {operation_id:?}"
    );

    assert_eq!(
        service_arn,
        format!("arn:aws:apprunner:{REGION}:{ACCOUNT}:service/my-web-svc/{service_id}"),
        "ServiceArn is built from the request's region, the account and the minted id"
    );
    assert_eq!(
        str_at(service, "ServiceUrl"),
        format!("{service_id}.{REGION}.awsapprunner.com"),
        "ServiceUrl is the service's subdomain in the request's region"
    );
    assert_eq!(
        str_at(service, "Status"),
        "OPERATION_IN_PROGRESS",
        "a create is asynchronous, so the service is not ready yet: {service:?}"
    );

    // The SAME record, read back by ARN.
    let (status, described) =
        call(&router, "DescribeService", json!({ "ServiceArn": service_arn })).await;
    assert_eq!(status, 200, "describe: expected 200, got {described:?}");
    let read = service_of(&described);
    for member in [
        "ServiceId",
        "ServiceArn",
        "ServiceUrl",
        "ServiceName",
        "CreatedAt",
        "SourceConfiguration",
        "AutoScalingConfigurationSummary",
    ] {
        assert_eq!(
            read.get(member),
            service.get(member),
            "member {member}: describe must return the record create persisted"
        );
    }

    // Defaults the control plane applied and stored, from each member's own doc.
    assert_eq!(
        read.get("InstanceConfiguration"),
        Some(&json!({ "Cpu": "1 vCPU", "Memory": "2 GB" })),
        "InstanceConfiguration defaults (Cpu 1 vCPU, Memory 2 GB) in {read:?}"
    );
    assert_eq!(
        read.get("HealthCheckConfiguration"),
        Some(&json!({
            "Protocol": "TCP",
            "Path": "/",
            "Interval": 5,
            "Timeout": 2,
            "HealthyThreshold": 1,
            "UnhealthyThreshold": 5
        })),
        "HealthCheckConfiguration defaults in {read:?}"
    );
    assert_eq!(
        read.get("NetworkConfiguration"),
        Some(&json!({
            "EgressConfiguration": { "EgressType": "DEFAULT" },
            "IngressConfiguration": { "IsPubliclyAccessible": false },
            "IpAddressType": "IPV4"
        })),
        "NetworkConfiguration defaults in {read:?}"
    );
    let summary = read
        .get("AutoScalingConfigurationSummary")
        .expect("AutoScalingConfigurationSummary is required");
    assert_eq!(
        str_at(summary, "AutoScalingConfigurationArn"),
        format!(
            "arn:aws:apprunner:{REGION}:{ACCOUNT}:autoscalingconfiguration/DefaultConfiguration/1"
        ),
        "with no ARN supplied the service is associated with the default configuration"
    );

    // The clock owes the service `RUNNING`, and the operation `SUCCEEDED`.
    wait_for_settle();
    let (_, settled) =
        call(&router, "DescribeService", json!({ "ServiceArn": service_arn })).await;
    let settled_service = service_of(&settled);
    assert_eq!(
        str_at(settled_service, "Status"),
        "RUNNING",
        "after the operation falls due the service is running: {settled_service:?}"
    );

    let (status, operations) =
        call(&router, "ListOperations", json!({ "ServiceArn": service_arn })).await;
    assert_eq!(status, 200, "list operations: got {operations:?}");
    let log = operations
        .get("OperationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no OperationSummaryList in {operations:?}"));
    assert_eq!(log.len(), 1, "one operation was recorded: {log:?}");
    assert_eq!(
        str_at(&log[0], "Id"),
        operation_id,
        "the OperationId create returned must appear in the log: {log:?}"
    );
    assert_eq!(str_at(&log[0], "Type"), "CREATE_SERVICE", "{log:?}");
    assert_eq!(str_at(&log[0], "Status"), "SUCCEEDED", "{log:?}");
    assert_eq!(str_at(&log[0], "TargetArn"), service_arn, "{log:?}");
    assert!(
        log[0].get("EndedAt").is_some(),
        "a finished operation has an EndedAt: {log:?}"
    );
}

/// `AutoDeploymentsEnabled`'s documented default, over the whole class of
/// sources it distinguishes: "App Runner sets to `false` for a source image that
/// uses an ECR Public repository or an ECR repository that's in an Amazon Web
/// Services account other than the one that the service is in. App Runner sets
/// to `true` in all other cases (which currently include a source code
/// repository or a source image using a same-account ECR repository)."
/// (`SourceConfiguration$AutoDeploymentsEnabled`, smithy_query).
#[tokio::test]
async fn auto_deployments_default_follows_the_source_kind() {
    let same_account_image = json!({
        "ImageRepository": {
            "ImageIdentifier": format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/web:1"),
            "ImageRepositoryType": "ECR"
        }
    });
    let other_account_image = json!({
        "ImageRepository": {
            "ImageIdentifier": format!("123456789012.dkr.ecr.{REGION}.amazonaws.com/web:1"),
            "ImageRepositoryType": "ECR"
        }
    });
    let code_source = json!({
        "CodeRepository": {
            "RepositoryUrl": "https://github.com/example/app",
            "SourceCodeVersion": { "Type": "BRANCH", "Value": "main" },
            "CodeConfiguration": { "ConfigurationSource": "REPOSITORY" }
        }
    });
    let mut explicitly_disabled = code_source.clone();
    explicitly_disabled["AutoDeploymentsEnabled"] = json!(false);

    let cases = [
        ("ecr-public-src", ecr_public_source(), false),
        ("ecr-same-acct", same_account_image, true),
        ("ecr-other-acct", other_account_image, false),
        ("code-repo-src", code_source, true),
        ("code-repo-off", explicitly_disabled, false),
    ];

    let router = router();
    for (name, source, expected) in cases {
        let (status, body) = call(
            &router,
            "CreateService",
            json!({ "ServiceName": name, "SourceConfiguration": source }),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {body:?}");
        let arn = str_at(service_of(&body), "ServiceArn");
        let (_, described) = call(&router, "DescribeService", json!({ "ServiceArn": arn })).await;
        let actual = service_of(&described)
            .get("SourceConfiguration")
            .and_then(|source| source.get("AutoDeploymentsEnabled"))
            .and_then(Value::as_bool);
        assert_eq!(
            actual,
            Some(expected),
            "case {name:?}: expected AutoDeploymentsEnabled {expected}, got {actual:?}"
        );
    }
}

/// Every request this service refuses answers `InvalidRequestException` with
/// HTTP 400 — the modelled client-input error — whether the rule broken is a
/// modelled constraint on a member or one of the structural rules the shapes'
/// own docs state.
#[tokio::test]
async fn invalid_requests_are_refused_as_invalid_request_exception() {
    let vpc_connector = format!("arn:aws:apprunner:{REGION}:{ACCOUNT}:vpcconnector/vpc-1/1");
    let observability =
        format!("arn:aws:apprunner:{REGION}:{ACCOUNT}:observabilityconfiguration/xray/1");

    let cases: Vec<(&str, Value)> = vec![
        // `ServiceName` length 4..40: one below the published minimum.
        (
            "name-too-short",
            json!({ "ServiceName": "abc", "SourceConfiguration": ecr_public_source() }),
        ),
        // One above the published maximum (41 characters).
        (
            "name-too-long",
            json!({ "ServiceName": "a".repeat(41), "SourceConfiguration": ecr_public_source() }),
        ),
        // `ServiceName` pattern: must start with an alphanumeric.
        (
            "name-bad-pattern",
            json!({ "ServiceName": "-leading-dash", "SourceConfiguration": ecr_public_source() }),
        ),
        // A required member absent.
        (
            "name-absent",
            json!({ "SourceConfiguration": ecr_public_source() }),
        ),
        // "You must provide either this member or ImageRepository (but not both)".
        (
            "source-neither",
            json!({ "ServiceName": "src-neither", "SourceConfiguration": {} }),
        ),
        (
            "source-both",
            json!({
                "ServiceName": "src-both",
                "SourceConfiguration": {
                    "ImageRepository": {
                        "ImageIdentifier": "public.ecr.aws/x/y:1",
                        "ImageRepositoryType": "ECR_PUBLIC"
                    },
                    "CodeRepository": {
                        "RepositoryUrl": "https://github.com/example/app",
                        "SourceCodeVersion": { "Type": "BRANCH", "Value": "main" }
                    }
                }
            }),
        ),
        // "VpcConnectorArn ... Only valid when EgressType = VPC".
        (
            "vpc-egress-without-connector",
            json!({
                "ServiceName": "net-novpc",
                "SourceConfiguration": ecr_public_source(),
                "NetworkConfiguration": { "EgressConfiguration": { "EgressType": "VPC" } }
            }),
        ),
        (
            "connector-without-vpc-egress",
            json!({
                "ServiceName": "net-badvpc",
                "SourceConfiguration": ecr_public_source(),
                "NetworkConfiguration": {
                    "EgressConfiguration": {
                        "EgressType": "DEFAULT",
                        "VpcConnectorArn": vpc_connector
                    }
                }
            }),
        ),
        // "ObservabilityConfigurationArn ... Specified only when
        // ObservabilityEnabled is true".
        (
            "observability-on-without-arn",
            json!({
                "ServiceName": "obs-noarn",
                "SourceConfiguration": ecr_public_source(),
                "ObservabilityConfiguration": { "ObservabilityEnabled": true }
            }),
        ),
        (
            "observability-off-with-arn",
            json!({
                "ServiceName": "obs-offarn",
                "SourceConfiguration": ecr_public_source(),
                "ObservabilityConfiguration": {
                    "ObservabilityEnabled": false,
                    "ObservabilityConfigurationArn": observability
                }
            }),
        ),
        // An `AutoScalingConfigurationArn` that names some other resource kind.
        (
            "autoscaling-arn-wrong-kind",
            json!({
                "ServiceName": "asc-wrong",
                "SourceConfiguration": ecr_public_source(),
                "AutoScalingConfigurationArn": vpc_connector
            }),
        ),
    ];

    let router = router();
    for (name, body) in cases {
        let (status, response) = call(&router, "CreateService", body).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidRequestException",
            "case {name:?}: expected InvalidRequestException, got {response:?}"
        );
    }

    // The published bounds themselves are accepted: 4 and 40 characters.
    for name in ["abcd", &"a".repeat(40)] {
        let (status, body) = call(
            &router,
            "CreateService",
            json!({ "ServiceName": name, "SourceConfiguration": ecr_public_source() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a name at the published bound is valid, got {body:?}"
        );
    }

    // `ServiceName` "must be unique ... in your Amazon Web Services account in
    // the Amazon Web Services Region" (the member's model doc).
    let (status, body) = call(
        &router,
        "CreateService",
        json!({ "ServiceName": "abcd", "SourceConfiguration": ecr_public_source() }),
    )
    .await;
    assert_eq!(status, 400, "a duplicate name is refused, got {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a duplicate name is an InvalidRequestException: {body:?}"
    );
}

/// An operation that names a well-formed ARN no service was created for answers
/// `ResourceNotFoundException` (HTTP 400) — never a fabricated record.
#[tokio::test]
async fn unknown_service_arn_is_not_found() {
    let router = router();
    let unknown = format!(
        "arn:aws:apprunner:{REGION}:{ACCOUNT}:service/no-such-svc/\
         0123456789abcdef0123456789abcdef"
    );

    for operation in ["DescribeService", "ListOperations", "UpdateService", "DeleteService"] {
        let (status, body) = call(&router, operation, json!({ "ServiceArn": unknown })).await;
        assert_eq!(
            status, 400,
            "{operation}: expected HTTP 400 for an unknown ARN, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{operation}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}

/// `UpdateService` merges the supplied members into the stored record, bumps
/// `UpdatedAt`, and logs an `UPDATE_SERVICE` operation; a later
/// `DescribeService` reports the merged record. An update of a service with an
/// operation still in flight is refused with `InvalidStateException`.
#[tokio::test]
async fn update_merges_into_the_stored_record() {
    let (router, engine) = router_with_engine();
    let created = create_ok(&router, "upd-target").await;
    let service_arn = str_at(service_of(&created), "ServiceArn");
    let create_operation = str_at(&created, "OperationId");
    let mut previous_updated_at = service_of(&created)
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .expect("UpdatedAt is required");

    // While the create is in flight the service is not in a state that accepts
    // an update. The create is held in flight explicitly, so the case asserts
    // the transient state rather than racing the due instant.
    hold_in_flight(&engine, &service_arn);
    let (status, body) = call(
        &router,
        "UpdateService",
        json!({ "ServiceArn": service_arn, "InstanceConfiguration": { "Cpu": "2 vCPU" } }),
    )
    .await;
    assert_eq!(status, 400, "update during create: got {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidStateException",
        "an update of a service with an operation in flight is refused: {body:?}"
    );

    make_due_now(&engine, &service_arn);

    // Each row changes a different member; every other member keeps its stored
    // value, which is what makes this a merge rather than a replace.
    let cases: Vec<(&str, Value, &str, Value)> = vec![
        (
            "cpu-only",
            json!({ "InstanceConfiguration": { "Cpu": "2 vCPU" } }),
            "InstanceConfiguration",
            json!({ "Cpu": "2 vCPU", "Memory": "2 GB" }),
        ),
        (
            "memory-only",
            json!({ "InstanceConfiguration": { "Memory": "4 GB" } }),
            "InstanceConfiguration",
            json!({ "Cpu": "2 vCPU", "Memory": "4 GB" }),
        ),
        (
            "health-check-path",
            json!({ "HealthCheckConfiguration": { "Protocol": "HTTP", "Path": "/healthz" } }),
            "HealthCheckConfiguration",
            json!({
                "Protocol": "HTTP",
                "Path": "/healthz",
                "Interval": 5,
                "Timeout": 2,
                "HealthyThreshold": 1,
                "UnhealthyThreshold": 5
            }),
        ),
        (
            "ip-address-type",
            json!({ "NetworkConfiguration": { "IpAddressType": "DUAL_STACK" } }),
            "NetworkConfiguration",
            json!({
                "EgressConfiguration": { "EgressType": "DEFAULT" },
                "IngressConfiguration": { "IsPubliclyAccessible": false },
                "IpAddressType": "DUAL_STACK"
            }),
        ),
    ];

    let mut operation_ids = vec![create_operation];
    for (case, patch, member, expected) in cases {
        let mut request = patch;
        request["ServiceArn"] = json!(service_arn);
        let (status, body) = call(&router, "UpdateService", request).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
        operation_ids.push(str_at(&body, "OperationId"));
        assert_eq!(
            str_at(service_of(&body), "Status"),
            "OPERATION_IN_PROGRESS",
            "case {case:?}: an update is asynchronous: {body:?}"
        );

        // Hold the update in flight before reading the record back. The pending
        // transition is materialised by `settle_due` on the next REQUEST, so on a
        // loaded machine the describe below can fall past the 500 ms due instant,
        // materialise the update and leave nothing for `make_due_now` at the foot
        // of this loop to move — which is a race in the test, not in the emulator.
        // Pinning the due instant far out first makes the read deterministic; the
        // merged members this row asserts do not depend on the transition.
        hold_in_flight(&engine, &service_arn);

        let (_, described) =
            call(&router, "DescribeService", json!({ "ServiceArn": service_arn })).await;
        let read = service_of(&described);
        assert_eq!(
            read.get(member),
            Some(&expected),
            "case {case:?}: describe must report the merged {member}, got {read:?}"
        );
        let updated_at = read
            .get("UpdatedAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: UpdatedAt is required in {read:?}"));
        assert!(
            updated_at > previous_updated_at,
            "case {case:?}: UpdatedAt must advance on every update, was \
             {previous_updated_at}, got {updated_at}"
        );
        previous_updated_at = updated_at;
        // The next row needs a settled service again.
        make_due_now(&engine, &service_arn);
    }

    let (_, operations) =
        call(&router, "ListOperations", json!({ "ServiceArn": service_arn })).await;
    let log = operations
        .get("OperationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no OperationSummaryList in {operations:?}"));
    let logged: Vec<String> = log.iter().map(|entry| str_at(entry, "Id")).collect();
    for id in &operation_ids {
        assert!(
            logged.contains(id),
            "operation {id} must appear in the log, got {logged:?}"
        );
    }
    let types: Vec<String> = log.iter().map(|entry| str_at(entry, "Type")).collect();
    assert_eq!(
        types.iter().filter(|kind| *kind == "UPDATE_SERVICE").count(),
        4,
        "one UPDATE_SERVICE per update: {types:?}"
    );
    assert_eq!(
        types.iter().filter(|kind| *kind == "CREATE_SERVICE").count(),
        1,
        "one CREATE_SERVICE: {types:?}"
    );
}

/// `DeleteService` takes the service out of the live listing once the delete
/// falls due, and the name it held becomes available again.
#[tokio::test]
async fn delete_removes_the_service_once_it_falls_due() {
    let (router, engine) = router_with_engine();
    let created = create_ok(&router, "del-target").await;
    let service_arn = str_at(service_of(&created), "ServiceArn");
    make_due_now(&engine, &service_arn);

    let (status, deleted) =
        call(&router, "DeleteService", json!({ "ServiceArn": service_arn })).await;
    assert_eq!(status, 200, "delete: expected 200, got {deleted:?}");
    let delete_operation = str_at(&deleted, "OperationId");
    assert_eq!(
        str_at(service_of(&deleted), "Status"),
        "OPERATION_IN_PROGRESS",
        "a delete is asynchronous: {deleted:?}"
    );

    // A second delete while the first is in flight is a state error, not a
    // not-found: the service is still there. The delete is held in flight
    // explicitly, so the case cannot race the due instant.
    hold_in_flight(&engine, &service_arn);
    let (status, body) = call(&router, "DeleteService", json!({ "ServiceArn": service_arn })).await;
    assert_eq!(status, 400, "second delete in flight: got {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidStateException",
        "a delete already in flight is refused with InvalidStateException: {body:?}"
    );

    make_due_now(&engine, &service_arn);

    // Gone from the live listing, gone from Describe, and a repeat delete is now
    // a not-found.
    let (_, listed) = call(&router, "ListServices", json!({})).await;
    let summaries = listed
        .get("ServiceSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no ServiceSummaryList in {listed:?}"));
    assert!(
        summaries.is_empty(),
        "a deleted service is not listed, got {summaries:?}"
    );

    for operation in ["DescribeService", "DeleteService", "ListOperations"] {
        let (status, body) = call(&router, operation, json!({ "ServiceArn": service_arn })).await;
        assert_eq!(status, 400, "{operation} after delete: got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{operation} after delete must be a not-found: {body:?}"
        );
    }
    assert!(
        !delete_operation.is_empty(),
        "the delete returned an OperationId"
    );

    // The name is free again.
    let recreated = create_ok(&router, "del-target").await;
    assert_ne!(
        str_at(service_of(&recreated), "ServiceId"),
        str_at(service_of(&created), "ServiceId"),
        "the re-created service is a new resource with a new id"
    );
}

/// `ListServices` pages the persisted services: for ANY page size the model
/// allows (`MaxResults` range 1..20) and any number of services, following
/// `NextToken` enumerates every live service exactly once, in `ServiceName`
/// order, and the last page carries no token.
///
/// The whole small domain is swept rather than sampled: 0..=4 services against
/// every published page size.
#[tokio::test]
async fn list_services_pages_over_every_allowed_page_size() {
    for count in 0..=4_usize {
        let router = router();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let name = format!("svc-{index:04}");
            create_ok(&router, &name).await;
            expected.push(name);
        }
        expected.sort();

        for max_results in 1..=20_i32 {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({ "MaxResults": max_results });
                if let Some(token) = token.as_ref() {
                    request["NextToken"] = json!(token);
                }
                let (status, body) = call(&router, "ListServices", request).await;
                assert_eq!(
                    status, 200,
                    "case {count} services / MaxResults {max_results}: got {body:?}"
                );
                let page = body
                    .get("ServiceSummaryList")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!("no ServiceSummaryList in {body:?}"));
                assert!(
                    page.len() <= max_results as usize,
                    "case {count} services / MaxResults {max_results}: a page must not \
                     exceed MaxResults, got {} entries",
                    page.len()
                );
                for summary in page {
                    seen.push(str_at(summary, "ServiceName"));
                }
                token = body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                pages += 1;
                assert!(
                    pages <= count + 1,
                    "case {count} services / MaxResults {max_results}: pagination did not \
                     terminate after {pages} pages"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, expected,
                "case {count} services / MaxResults {max_results}: expected {expected:?}, \
                 got {seen:?}"
            );
        }

        // No `MaxResults` means "the request retrieves all available results in
        // a single response" (the member's model doc).
        let (status, body) = call(&router, "ListServices", json!({})).await;
        assert_eq!(status, 200, "case {count} services unpaged: got {body:?}");
        let all = body
            .get("ServiceSummaryList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("no ServiceSummaryList in {body:?}"));
        assert_eq!(
            all.len(),
            count,
            "case {count} services unpaged: expected every service in one page, got {all:?}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {count} services unpaged: a single page carries no NextToken, got {body:?}"
        );
    }
}

/// A `NextToken` that names no position in the result set is refused rather than
/// silently answered from the start of the list.
#[tokio::test]
async fn an_unknown_next_token_is_refused() {
    let router = router();
    create_ok(&router, "tok-svc").await;
    let service_arn = {
        let (_, listed) = call(&router, "ListServices", json!({})).await;
        let summaries = listed
            .get("ServiceSummaryList")
            .and_then(Value::as_array)
            .expect("ServiceSummaryList");
        str_at(&summaries[0], "ServiceArn")
    };

    let (status, body) = call(
        &router,
        "ListServices",
        json!({ "MaxResults": 1, "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(status, 400, "bad ListServices token: got {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "bad ListServices token: got {body:?}"
    );

    let (status, body) = call(
        &router,
        "ListOperations",
        json!({ "ServiceArn": service_arn, "MaxResults": 1, "NextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(status, 400, "bad ListOperations token: got {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "bad ListOperations token: got {body:?}"
    );
}

/// The store holds only the services a caller currently has: a create-then-delete
/// loop on the same name leaves nothing behind, so the memory a caller takes is
/// the memory a caller can give back.
#[tokio::test]
async fn a_completed_delete_reclaims_the_record_and_its_log() {
    use service_com_amazonaws_apprunner_apprunner::state::ComAmazonawsApprunnerApprunnerState;

    let (router, engine) = router_with_engine();
    for round in 0..3 {
        let created = create_ok(&router, "churn-svc").await;
        let service_arn = str_at(service_of(&created), "ServiceArn");
        make_due_now(&engine, &service_arn);
        let (status, body) =
            call(&router, "DeleteService", json!({ "ServiceArn": service_arn })).await;
        assert_eq!(status, 200, "round {round}: delete failed with {body:?}");
        make_due_now(&engine, &service_arn);
        // One request after the delete falls due materialises it.
        let (_, listed) = call(&router, "ListServices", json!({})).await;
        assert_eq!(
            listed
                .get("ServiceSummaryList")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "round {round}: nothing is listed after the delete, got {listed:?}"
        );

        let store = engine.state::<ComAmazonawsApprunnerApprunnerState>();
        let store = store.lock();
        assert!(
            store.services.is_empty(),
            "round {round}: the record must be reclaimed, store holds {} service(s)",
            store.services.len()
        );
        assert!(
            store.operations.is_empty(),
            "round {round}: the operation log must be reclaimed, store holds {} log(s)",
            store.operations.len()
        );
        assert!(
            store.name_index.is_empty(),
            "round {round}: the name must be freed, store holds {:?}",
            store.name_index
        );
    }
}

/// One service's operation log is capped: the oldest entries are evicted, so
/// repeated operations on a single service cannot grow the store without bound.
#[test]
fn the_operation_log_is_capped_per_service() {
    use service_com_amazonaws_apprunner_apprunner::state::{
        ComAmazonawsApprunnerApprunnerState, MAX_OPERATIONS_PER_SERVICE,
    };

    let mut store = ComAmazonawsApprunnerApprunnerState::default();
    let arn = format!("arn:aws:apprunner:{REGION}:{ACCOUNT}:service/capped-svc/{}", "0".repeat(32));
    let started = std::time::SystemTime::UNIX_EPOCH;
    let total = MAX_OPERATIONS_PER_SERVICE + 25;
    for index in 0..total {
        store.record_operation(
            &arn,
            state::OP_TYPE_UPDATE_SERVICE,
            format!("operation-{index:04}"),
            started + Duration::from_secs(index as u64),
        );
    }

    let log = store.operations.get(&arn).expect("the log exists");
    assert_eq!(
        log.len(),
        MAX_OPERATIONS_PER_SERVICE,
        "the log is capped at {MAX_OPERATIONS_PER_SERVICE}, got {}",
        log.len()
    );
    assert_eq!(
        log.first().map(|entry| entry.id.as_str()),
        Some(format!("operation-{:04}", total - MAX_OPERATIONS_PER_SERVICE).as_str()),
        "the oldest entries are the ones evicted, log starts at {:?}",
        log.first().map(|entry| entry.id.clone())
    );
    assert_eq!(
        log.last().map(|entry| entry.id.as_str()),
        Some(format!("operation-{:04}", total - 1).as_str()),
        "the most recent entry is kept, log ends at {:?}",
        log.last().map(|entry| entry.id.clone())
    );
}

/// The account's published App Runner service quota is enforced with the
/// modelled `ServiceQuotaExceededException` (HTTP 402): the last service inside
/// the quota is created, the first one past it is refused, and deleting a
/// service frees its slot again.
#[tokio::test]
async fn the_published_service_quota_is_enforced() {
    let (router, engine) = router_with_engine();
    let mut arns = Vec::new();
    for index in 0..state::MAX_SERVICES_PER_ACCOUNT {
        let name = format!("quota-{index:04}");
        let created = create_ok(&router, &name).await;
        arns.push(str_at(service_of(&created), "ServiceArn"));
    }

    let (status, body) = call(
        &router,
        "CreateService",
        json!({ "ServiceName": "quota-over", "SourceConfiguration": ecr_public_source() }),
    )
    .await;
    assert_eq!(
        status, 402,
        "one past the quota of {}: expected HTTP 402, got {status} {body:?}",
        state::MAX_SERVICES_PER_ACCOUNT
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "one past the quota: expected ServiceQuotaExceededException, got {body:?}"
    );

    // The quota counts the services that exist, so a delete gives the slot back.
    make_due_now(&engine, &arns[0]);
    let (status, body) = call(
        &router,
        "DeleteService",
        json!({ "ServiceArn": arns[0].clone() }),
    )
    .await;
    assert_eq!(status, 200, "delete to free a slot: got {body:?}");
    make_due_now(&engine, &arns[0]);
    let (status, body) = call(
        &router,
        "CreateService",
        json!({ "ServiceName": "quota-again", "SourceConfiguration": ecr_public_source() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a freed slot admits a new service, got {status} {body:?}"
    );
}

/// `ServiceName` uniqueness, the service quota and what `ListServices` reports
/// are all scoped to the Region the request is signed for — "unique across all
/// the running App Runner services in your Amazon Web Services account in the
/// Amazon Web Services Region" (`ServiceName`'s model doc).
#[tokio::test]
async fn names_quota_and_listing_are_region_scoped() {
    /// A second Region, so the rule is exercised across Regions rather than in
    /// the one every other case uses.
    const OTHER_REGION: &str = "us-east-2";

    let router = router();
    let request = json!({
        "ServiceName": "twin-svc",
        "SourceConfiguration": ecr_public_source()
    });

    let (status, here) = call_in(&router, REGION, "CreateService", request.clone()).await;
    assert_eq!(status, 200, "create in {REGION}: got {here:?}");
    let (status, there) = call_in(&router, OTHER_REGION, "CreateService", request.clone()).await;
    assert_eq!(
        status, 200,
        "the same name in {OTHER_REGION} is a different service, got {status} {there:?}"
    );

    let here_arn = str_at(service_of(&here), "ServiceArn");
    let there_arn = str_at(service_of(&there), "ServiceArn");
    assert_ne!(
        here_arn, there_arn,
        "two Regions' services are two resources"
    );
    assert!(
        there_arn.contains(OTHER_REGION),
        "the ARN names the Region the request was signed for, got {there_arn:?}"
    );

    // Each Region lists only its own services.
    for (region, expected) in [(REGION, &here_arn), (OTHER_REGION, &there_arn)] {
        let (_, listed) = call_in(&router, region, "ListServices", json!({})).await;
        let arns: Vec<String> = listed
            .get("ServiceSummaryList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("no ServiceSummaryList in {listed:?}"))
            .iter()
            .map(|summary| str_at(summary, "ServiceArn"))
            .collect();
        assert_eq!(
            arns,
            vec![expected.clone()],
            "region {region}: expected only its own service, got {arns:?}"
        );
    }

    // Within one Region the name is still taken.
    let (status, body) = call_in(&router, REGION, "CreateService", request).await;
    assert_eq!(status, 400, "a duplicate within one Region: got {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a duplicate within one Region is refused: {body:?}"
    );

    // A service belongs to the Region its ARN names: every ARN-taking operation
    // refuses it from another Region.
    for operation in [
        "DescribeService",
        "ListOperations",
        "UpdateService",
        "DeleteService",
    ] {
        let (status, body) =
            call_in(&router, OTHER_REGION, operation, json!({ "ServiceArn": here_arn })).await;
        assert_eq!(
            status, 400,
            "{operation} on a {REGION} ARN from {OTHER_REGION}: expected HTTP 400, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{operation} on a cross-Region ARN must be a not-found: {body:?}"
        );
    }
}

/// The whole-store ceiling bounds the record count however many Regions a caller
/// addresses. The per-Region quota alone cannot: the Region comes from the
/// request's own SigV4 credential scope, so varying that one field would hand out
/// a fresh allowance each time.
#[tokio::test]
async fn the_whole_store_ceiling_bounds_every_region() {
    use service_com_amazonaws_apprunner_apprunner::state::{
        ComAmazonawsApprunnerApprunnerState, MAX_SERVICES_PER_ACCOUNT, MAX_SERVICES_TOTAL,
    };

    let (router, engine) = router_with_engine();
    let regions = MAX_SERVICES_TOTAL / MAX_SERVICES_PER_ACCOUNT;
    for region_index in 0..regions {
        let region = format!("ap-test-{region_index:03}");
        for service_index in 0..MAX_SERVICES_PER_ACCOUNT {
            let name = format!("svc-{region_index:03}-{service_index:02}");
            let (status, body) = call_in(
                &router,
                &region,
                "CreateService",
                json!({ "ServiceName": name, "SourceConfiguration": ecr_public_source() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "region {region} service {name}: expected 200, got {status} {body:?}"
            );
        }
    }

    {
        let store = engine.state::<ComAmazonawsApprunnerApprunnerState>();
        let store = store.lock();
        assert_eq!(
            store.services.len(),
            MAX_SERVICES_TOTAL,
            "the store holds exactly the ceiling, got {}",
            store.services.len()
        );
    }

    // A Region that has used none of its own allowance is still refused, because
    // the whole-store ceiling dominates the insert.
    let (status, body) = call_in(
        &router,
        "ap-test-overflow",
        "CreateService",
        json!({ "ServiceName": "over-the-top", "SourceConfiguration": ecr_public_source() }),
    )
    .await;
    assert_eq!(
        status, 402,
        "past the whole-store ceiling: expected HTTP 402, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "past the whole-store ceiling: expected ServiceQuotaExceededException, got {body:?}"
    );
}

/// The status and operation names this service writes are the model's own enum
/// values, not strings invented beside them.
#[test]
fn the_recorded_enum_values_are_the_modelled_ones() {
    use service_com_amazonaws_apprunner_apprunner::ops::types::{
        OPERATION_STATUS_VALUES, OPERATION_TYPE_VALUES, SERVICE_STATUS_VALUES,
    };

    for value in [
        state::SERVICE_OPERATION_IN_PROGRESS,
        state::SERVICE_RUNNING,
        state::SERVICE_DELETED,
    ] {
        assert!(
            SERVICE_STATUS_VALUES.contains(&value),
            "{value:?} is not a modelled ServiceStatus: {SERVICE_STATUS_VALUES:?}"
        );
    }
    for value in [state::OPERATION_IN_PROGRESS, state::OPERATION_SUCCEEDED] {
        assert!(
            OPERATION_STATUS_VALUES.contains(&value),
            "{value:?} is not a modelled OperationStatus: {OPERATION_STATUS_VALUES:?}"
        );
    }
    for value in [
        state::OP_TYPE_CREATE_SERVICE,
        state::OP_TYPE_UPDATE_SERVICE,
        state::OP_TYPE_DELETE_SERVICE,
    ] {
        assert!(
            OPERATION_TYPE_VALUES.contains(&value),
            "{value:?} is not a modelled OperationType: {OPERATION_TYPE_VALUES:?}"
        );
    }
}
