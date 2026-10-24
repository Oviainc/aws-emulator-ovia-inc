// SPDX-License-Identifier: CC0-1.0

//! What every id-addressed operation of this service answers for an id that names
//! nothing, end to end over HTTP.
//!
//! The rule under test: once the account-level device gate has passed — the account
//! has at least one device with Snow Device Management activated in the signed
//! Region — an operation addressed to a resource by id answers the modelled
//! `ResourceNotFoundException` (HTTP 404) for an id that names no such resource in
//! that Region, never the permission refusal: `AccessDeniedException` (HTTP 403) is
//! the ACCOUNT-level precondition only. The complement is asserted too, so the
//! boundary between the two is pinned from both sides: in a Region with no activated
//! device the very same calls answer the 403 gate.
//!
//! Rows differ along what the rule generalises over: the operation (every one the
//! model gives a `ResourceNotFoundException` and an id to resolve), the kind of id
//! (managed device, task, the execution pair), and the SHAPE of the unknown id —
//! both modelled `@length(1..64)` bounds, an id in the shape the store mints, ids in
//! other shapes, and an id that exists but in ANOTHER Region, which is what makes
//! the lookup regional rather than global.
//!
//! Nothing here asserts a value the emulator invented: the inventory is put in state
//! through the one activation seam (the API models no device-creation operation), the
//! task is created over the wire, and every answer is read back off the wire.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the inventory is
/// activated in.
const SIGNED_REGION: &str = "us-west-2";

/// Another Region of the same account, which also holds an activated device: a
/// record of this Region is addressed from [`SIGNED_REGION`] to show the lookup is
/// regional.
const OTHER_REGION: &str = "eu-central-1";

/// How many devices are activated in [`SIGNED_REGION`], so the account-level gate
/// does not fire there.
const ACTIVATED_DEVICES: u64 = 2;

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success. restJson1
    /// renders `__type` as a shape id on some services, so the code is its last
    /// segment.
    fn error_code(&self) -> &str {
        self.body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
    }

    /// The `message` member of an error body, or the empty string.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// One emulator with `count` devices activated in `region` and one in
/// [`OTHER_REGION`], answering the router plus the ids activated in each.
fn emulator_with(region: &str, count: u64) -> (MultiServiceRouter, Vec<String>, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = service_state(&*engine).expect("the in-memory engine carries this service's state");
    let here = state
        .lock()
        .activate_devices(region, count, SystemTime::now());
    let elsewhere = state
        .lock()
        .activate_devices(OTHER_REGION, 1, SystemTime::now());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, here, elsewhere)
}

/// Dispatch one request signed for `region`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!("snow-device-management.{region}.amazonaws.com"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/\
                 snow-device-management/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One request: the operation it names and how to send it.
struct Request {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every implemented operation that resolves a MANAGED DEVICE id, addressed to
/// `device`. `task` is a task this account really created, so `DescribeExecution`
/// fails on the device half of its pair rather than on the task half.
fn device_addressed(device: &str, task: &str) -> Vec<Request> {
    vec![
        Request {
            operation: "DescribeDevice",
            method: "POST",
            uri: format!("/managed-device/{device}/describe"),
            body: None,
        },
        Request {
            operation: "DescribeDeviceEc2Instances",
            method: "POST",
            uri: format!("/managed-device/{device}/resources/ec2/describe"),
            body: Some(json!({ "instanceIds": ["i-0123456789abcdef0"] })),
        },
        Request {
            operation: "DescribeDeviceEc2Instances (no instance id requested)",
            method: "POST",
            uri: format!("/managed-device/{device}/resources/ec2/describe"),
            body: Some(json!({ "instanceIds": [] })),
        },
        Request {
            operation: "DescribeExecution",
            method: "POST",
            uri: format!("/task/{task}/execution/{device}"),
            body: None,
        },
        Request {
            operation: "CreateTask",
            method: "POST",
            uri: "/task".to_string(),
            body: Some(json!({ "targets": [device], "command": { "unlock": {} } })),
        },
    ]
}

/// Every implemented operation that resolves a TASK id, addressed to `task`.
/// `device` is a device this account really manages, so `DescribeExecution` fails on
/// the task half of its pair.
fn task_addressed(task: &str, device: &str) -> Vec<Request> {
    vec![
        Request {
            operation: "DescribeTask",
            method: "POST",
            uri: format!("/task/{task}"),
            body: None,
        },
        Request {
            operation: "CancelTask",
            method: "POST",
            uri: format!("/task/{task}/cancel"),
            body: None,
        },
        Request {
            operation: "ListExecutions",
            method: "GET",
            uri: format!("/executions?taskId={task}"),
            body: None,
        },
        Request {
            operation: "DescribeExecution",
            method: "POST",
            uri: format!("/task/{task}/execution/{device}"),
            body: None,
        },
    ]
}

/// Ids in the shapes a client can send that name no record: the modelled
/// `@length(min: 1, max: 64)` floor and ceiling (`ManagedDeviceId` and `TaskId` are
/// both bounded that way — `smithy_query(query="shape",shape="ManagedDeviceId")`),
/// the shapes this store mints, and two unrelated shapes.
fn unknown_id_shapes(prefix: &str) -> Vec<String> {
    vec![
        "x".to_string(),
        "f".repeat(64),
        format!("{prefix}-ffffffffffffffff"),
        "does-not-exist".to_string(),
        "0".repeat(32),
    ]
}

/// Create one task against `device` and answer its id.
async fn create_task(router: &MultiServiceRouter, device: &str) -> String {
    let answer = call(
        router,
        SIGNED_REGION,
        "POST",
        "/task",
        Some(json!({ "targets": [device], "command": { "unlock": {} } })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"fixture\": CreateTask against a managed device must succeed, got {} with {}",
        answer.status, answer.body
    );
    answer.body["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"fixture\": expected a taskId, got {}", answer.body))
        .to_string()
}

/// Assert `answer` is the modelled not-found: HTTP 404 `ResourceNotFoundException`
/// with a message, and NOT the account-level permission refusal.
fn assert_not_found(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: an id that names nothing past the account gate is the modelled \
         ResourceNotFoundException, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {}",
        answer.body
    );
    assert!(
        !answer.message().is_empty(),
        "case {case:?}: the refusal carries a message, got {}",
        answer.body
    );
}

#[tokio::test]
async fn an_unknown_device_id_is_not_found_on_every_operation_that_resolves_one() {
    let (router, managed, elsewhere) = emulator_with(SIGNED_REGION, ACTIVATED_DEVICES);
    let task = create_task(&router, &managed[0]).await;

    // The rows: the shape of the unknown device id, plus a device that exists in
    // another Region of the same account — the lookup is regional, so that one is
    // unknown here too.
    let mut ids = unknown_id_shapes("smd");
    ids.push(elsewhere[0].clone());
    let mut statuses: BTreeSet<u16> = BTreeSet::new();

    for device in &ids {
        assert!(
            !managed.contains(device),
            "case \"fixture\": {device:?} must not be in this Region's inventory"
        );
        for request in device_addressed(device, &task) {
            let case = format!("{} with managedDeviceId {device:?}", request.operation);
            let answer = call(
                &router,
                SIGNED_REGION,
                request.method,
                &request.uri,
                request.body,
            )
            .await;
            assert_not_found(&answer, &case);
            statuses.insert(answer.status);
        }
    }

    assert_eq!(
        statuses,
        BTreeSet::from([404]),
        "case \"one outcome\": every operation answers an unknown device id the same \
         way, got {statuses:?}"
    );

    // The refusals changed nothing a later caller sees: the managed devices still
    // describe, and the task still has exactly the executions it was created with.
    for device in &managed {
        let case = format!("managed device {device} after the refusals");
        let answer = call(
            &router,
            SIGNED_REGION,
            "POST",
            &format!("/managed-device/{device}/describe"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a refused call must not change what a later caller sees, got \
             {} with {}",
            answer.status, answer.body
        );
    }
    let case = "the created task after the refusals";
    let answer = call(
        &router,
        SIGNED_REGION,
        "GET",
        &format!("/executions?taskId={task}"),
        None,
    )
    .await;
    let executions = answer.body["executions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected executions, got {}", answer.body))
        .len();
    assert_eq!(
        executions, 1,
        "case {case:?}: the task still targets exactly the one device it was created \
         for, got {}",
        answer.body
    );
}

#[tokio::test]
async fn an_unknown_task_id_is_not_found_on_every_operation_that_resolves_one() {
    let (router, managed, elsewhere) = emulator_with(SIGNED_REGION, ACTIVATED_DEVICES);
    // A task created in another Region: known to the account, unknown here.
    let elsewhere_device = elsewhere[0].clone();
    let other_region_task = {
        let answer = call(
            &router,
            OTHER_REGION,
            "POST",
            "/task",
            Some(json!({ "targets": [elsewhere_device], "command": { "reboot": {} } })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case \"fixture\": CreateTask in {OTHER_REGION} must succeed, got {} with {}",
            answer.status, answer.body
        );
        answer.body["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case \"fixture\": expected a taskId, got {}", answer.body))
            .to_string()
    };

    let mut ids = unknown_id_shapes("st");
    ids.push(other_region_task);

    for task in &ids {
        for request in task_addressed(task, &managed[0]) {
            let case = format!("{} with taskId {task:?}", request.operation);
            let answer = call(
                &router,
                SIGNED_REGION,
                request.method,
                &request.uri,
                request.body,
            )
            .await;
            assert_not_found(&answer, &case);
        }
    }
}

#[tokio::test]
async fn the_permission_refusal_is_the_account_level_precondition_only() {
    // The SAME calls with the SAME unknown ids, in a Region this account has
    // activated no device in: there the account-level gate answers 403 before any id
    // is resolved. This is the complement of the two tests above, so the boundary
    // between the 403 and the 404 is pinned from both sides.
    let (router, managed, _) = emulator_with(SIGNED_REGION, ACTIVATED_DEVICES);
    let task = create_task(&router, &managed[0]).await;
    let gated_region = "ap-northeast-1";

    let mut rows: Vec<Request> = device_addressed("smd-ffffffffffffffff", &task);
    rows.extend(task_addressed("st-ffffffffffffffff", &managed[0]));
    // The ids that DO exist in the signed Region are refused in the gated one too:
    // the gate is a property of the Region, not of the id.
    rows.extend(device_addressed(&managed[0], &task));

    for request in rows {
        let case = format!("{} in {gated_region}", request.operation);
        let answer = call(
            &router,
            gated_region,
            request.method,
            &request.uri,
            request.body,
        )
        .await;
        assert_eq!(
            answer.status, 403,
            "case {case:?}: a Region with no activated device is refused by the \
             account-level gate, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {}",
            answer.body
        );
        let message = answer.message().to_lowercase();
        assert!(
            message.contains("device") && message.contains("region"),
            "case {case:?}: the gate states the account-level precondition, got {:?}",
            answer.message()
        );
    }
}
