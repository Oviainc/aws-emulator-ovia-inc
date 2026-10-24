// SPDX-License-Identifier: CC0-1.0

//! What every id-addressed operation of this service answers for an id that names
//! nothing, end to end over HTTP.
//!
//! The rule under test: an operation addressed to a resource by id answers the
//! modelled `ResourceNotFoundException` (HTTP 404) for an id that names no such
//! resource in the signed Region, never the permission refusal —
//! `AccessDeniedException` (HTTP 403) is for a caller the action is not permitted
//! for, which is not what an id naming nothing is. The same ids are asked of a
//! Region that holds an inventory and of one that holds none, so the answer is shown
//! to come from the id rather than from how much the Region holds.
//!
//! Rows differ along what the rule generalises over: the operation (every one the
//! model gives a `ResourceNotFoundException` and an id to resolve), the kind of id
//! (managed device, task, the execution pair), and the SHAPE of the unknown id —
//! both modelled `@length(1..64)` bounds, an id in the shape the store mints, ids in
//! other shapes, and an id that exists but in ANOTHER Region, which is what makes
//! the lookup regional rather than global. The last test pins the other side of
//! that: a task's executions are read out of the Region the task was created in, so
//! one Region's answers never carry another's records.
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

/// How many devices are activated in [`SIGNED_REGION`], so the Region holds an
/// inventory for an id to be looked up against.
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
        "case {case:?}: an id that names nothing is the modelled \
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
async fn an_execution_is_read_out_of_the_region_its_task_was_created_in() {
    // Two Regions of the same account, each with a task over its OWN device: a
    // request names a task id, and what it gets back must come from the partition
    // it signed for — never another Region's execution records.
    let (router, managed, elsewhere) = emulator_with(SIGNED_REGION, ACTIVATED_DEVICES);
    let here = create_task(&router, &managed[0]).await;
    let there = {
        let answer = call(
            &router,
            OTHER_REGION,
            "POST",
            "/task",
            Some(json!({ "targets": [&elsewhere[0]], "command": { "reboot": {} } })),
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

    // Rows: the Region, its own task, the one device that task targets, and the
    // device of the OTHER Region, which none of its answers may name.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (SIGNED_REGION, &here, &managed[0], &elsewhere[0]),
        (OTHER_REGION, &there, &elsewhere[0], &managed[0]),
    ];

    for (region, task, device, foreign_device) in rows {
        let case = format!("ListExecutions of {task} in {region}");
        let answer = call(
            &router,
            region,
            "GET",
            &format!("/executions?taskId={task}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a task of this Region lists its executions, got {} with {}",
            answer.status, answer.body
        );
        let listed = answer.body["executions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected executions, got {}", answer.body))
            .clone();
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: the task targets exactly one device, got {}",
            answer.body
        );
        for execution in &listed {
            assert_eq!(
                execution["managedDeviceId"].as_str(),
                Some(device),
                "case {case:?}: a page may only hold this Region's executions, got {execution}"
            );
            assert_eq!(
                execution["taskId"].as_str(),
                Some(task),
                "case {case:?}: a page may only hold this task's executions, got {execution}"
            );
        }

        // The pair that exists resolves; the same task against the other Region's
        // device does not, so neither half of the key crosses the partition.
        let case = format!("DescribeExecution {task}/{device} in {region}");
        let answer = call(
            &router,
            region,
            "POST",
            &format!("/task/{task}/execution/{device}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the pair this Region holds must describe, got {} with {}",
            answer.status, answer.body
        );
        let case = format!("DescribeExecution {task}/{foreign_device} in {region}");
        let answer = call(
            &router,
            region,
            "POST",
            &format!("/task/{task}/execution/{foreign_device}"),
            None,
        )
        .await;
        assert_not_found(&answer, &case);
    }

    // And each Region's task is unknown in the other, so its executions are
    // unreachable from there whatever that Region holds.
    for (region, foreign_task) in [(SIGNED_REGION, &there), (OTHER_REGION, &here)] {
        let case = format!("ListExecutions of {foreign_task} in {region}");
        let answer = call(
            &router,
            region,
            "GET",
            &format!("/executions?taskId={foreign_task}"),
            None,
        )
        .await;
        assert_not_found(&answer, &case);
    }
}

#[tokio::test]
async fn an_id_is_resolved_against_the_signed_region_whatever_that_region_holds() {
    // The SAME calls with the SAME unknown ids, in a Region this account has
    // activated no device in: an id names nothing there either, so the answer is the
    // one above. Nothing turns a sparse Region into a permission problem.
    let (router, managed, _) = emulator_with(SIGNED_REGION, ACTIVATED_DEVICES);
    let task = create_task(&router, &managed[0]).await;
    let empty_region = "ap-northeast-1";

    let mut rows: Vec<Request> = device_addressed("smd-ffffffffffffffff", &task);
    rows.extend(task_addressed("st-ffffffffffffffff", &managed[0]));
    // The ids that DO exist in the signed Region name nothing in the empty one: the
    // lookup is regional.
    rows.extend(device_addressed(&managed[0], &task));

    for request in rows {
        let case = format!("{} in {empty_region}", request.operation);
        let answer = call(
            &router,
            empty_region,
            request.method,
            &request.uri,
            request.body,
        )
        .await;
        assert_not_found(&answer, &case);
    }
}
