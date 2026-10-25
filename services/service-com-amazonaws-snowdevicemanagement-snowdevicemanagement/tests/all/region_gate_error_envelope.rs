// SPDX-License-Identifier: CC0-1.0

//! The WIRE ENVELOPE of the account/region device precondition, end to end over HTTP.
//!
//! The rule under test: a request signed for a Region this account has activated no
//! Snow Device Management device in is refused by every operation the model declares
//! `AccessDeniedException` on (`smithy_query(query="digest")`: CancelTask, CreateTask,
//! DescribeDevice, DescribeDeviceEc2Instances, DescribeExecution, DescribeTask,
//! ListDeviceResources, ListDevices, ListExecutions, ListTasks), and a restJson1
//! client resolves that refusal from the HTTP status and the `x-amzn-errortype`
//! header: HTTP 403 (`AccessDeniedException` is `errorKind` client, `@httpError 403` —
//! `smithy_query(query="shape",shape="AccessDeniedException")`) with that header naming
//! the shape, the same code in the body's `__type`, and ONE message for the whole
//! class. The refusal is decided before any id is resolved, so the id-addressed
//! operations never answer the per-resource `ResourceNotFoundException` (404) there and
//! the collection operations never answer a page (200): the envelope a gated caller
//! reads carries no success representation at all. Where the Region DOES hold an
//! inventory the operation's own answer stands, header included — which is what shows
//! the 403 is the precondition speaking and not a blanket refusal.
//!
//! `tests/all/device_region_gate.rs` pins the refusal itself (status, body code,
//! message class, per-Region scope, the tagging operations' exemption); what this suite
//! adds is the header a client branches on, over the two inventory states that make a
//! Region device-free — an account that has activated nothing at all, and an account
//! whose devices are all in another Region — and the matching envelope of the answers a
//! served Region gives.
//!
//! Rows differ along what the rule generalises over: the operation (all ten), the
//! account's inventory state, the signed Region, and the shape of the ids addressed.
//!
//! Nothing here asserts a value the emulator invented: the inventory is put in state
//! through the one activation seam (the API models no device-creation operation) and
//! every answer is read back off the wire.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The wire error code the precondition refuses with, and the status the model gives
/// it.
const GATE_CODE: &str = "AccessDeniedException";
const GATE_STATUS: u16 = 403;

/// The per-resource failure the id-addressed operations answer in a Region that meets
/// the precondition, with its modelled status (`@httpError 404`).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: u16 = 404;

/// The Region whose inventory makes the control rows: the one the account activates
/// devices in.
const STOCKED_REGION: &str = "eu-west-2";

/// Regions of the same account that hold no device of their own. Three of them, so the
/// envelope is shown to be a property of what the Region holds rather than of one
/// Region name.
const DEVICE_FREE_REGIONS: &[&str] = &["us-east-1", "ap-northeast-1", "sa-east-1"];

/// How many devices the activation seam puts in [`STOCKED_REGION`].
const ACTIVATED_DEVICES: u64 = 2;

/// What the emulator answered: the parts a restJson1 client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` header, reduced to the shape's name (restJson1 allows a
    /// shape id or a URI-suffixed form), or the empty string when absent.
    error_type: String,
    body: Value,
}

impl Answer {
    /// The wire error code the BODY carries (`__type`), or the empty string.
    fn body_code(&self) -> &str {
        self.body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
    }

    /// The `message` member of an error body, or the empty string.
    fn message(&self) -> &str {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
    }
}

/// One emulator with `count` devices activated in `region` — `count = 0` is an account
/// that has activated none anywhere. Answers the router and the activated ids.
fn emulator_with(region: &str, count: u64) -> (MultiServiceRouter, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let activated = service_state(&*engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(region, count, SystemTime::now());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, activated)
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
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
    Answer {
        status,
        error_type,
        body,
    }
}

/// One request: the operation it names and how a client sends it.
struct Request {
    operation: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation the model declares `AccessDeniedException` on, addressed to `device`
/// and `task`. The ten of them together are the gated surface.
fn gated_requests(device: &str, task: &str) -> Vec<Request> {
    vec![
        Request {
            operation: "ListDevices".to_string(),
            method: "GET",
            uri: "/managed-devices".to_string(),
            body: None,
        },
        Request {
            operation: "ListTasks".to_string(),
            method: "GET",
            uri: "/tasks".to_string(),
            body: None,
        },
        Request {
            operation: format!("DescribeDevice {device:?}"),
            method: "POST",
            uri: format!("/managed-device/{device}/describe"),
            body: None,
        },
        Request {
            operation: format!("DescribeDeviceEc2Instances {device:?}"),
            method: "POST",
            uri: format!("/managed-device/{device}/resources/ec2/describe"),
            body: Some(json!({ "instanceIds": ["i-0123456789abcdef0"] })),
        },
        Request {
            operation: format!("ListDeviceResources {device:?}"),
            method: "GET",
            uri: format!("/managed-device/{device}/resources"),
            body: None,
        },
        Request {
            operation: format!("DescribeTask {task:?}"),
            method: "POST",
            uri: format!("/task/{task}"),
            body: None,
        },
        Request {
            operation: format!("CancelTask {task:?}"),
            method: "POST",
            uri: format!("/task/{task}/cancel"),
            body: None,
        },
        Request {
            operation: format!("ListExecutions of {task:?}"),
            method: "GET",
            uri: format!("/executions?taskId={task}"),
            body: None,
        },
        Request {
            operation: format!("DescribeExecution {task:?}/{device:?}"),
            method: "POST",
            uri: format!("/task/{task}/execution/{device}"),
            body: None,
        },
        Request {
            operation: format!("CreateTask targeting {device:?}"),
            method: "POST",
            uri: "/task".to_string(),
            body: Some(json!({ "targets": [device], "command": { "unlock": {} } })),
        },
    ]
}

/// Device and task ids in the shapes a client can send: the `@length(min: 1, max: 64)`
/// floor and ceiling both `ManagedDeviceId` and `TaskId` are modelled with
/// (`smithy_query(query="shape",shape="ManagedDeviceId,TaskId")`), and the hyphenated
/// shapes a caller copies out of a job.
fn id_pairs() -> Vec<(String, String)> {
    vec![
        ("d".to_string(), "t".to_string()),
        ("a".repeat(64), "b".repeat(64)),
        (
            "probe-mdevice-0a1b2c3d".to_string(),
            "probe-task-4e5f6a7b".to_string(),
        ),
    ]
}

/// Assert `answer` is the precondition's refusal as a restJson1 client reads it, and
/// hand its message back so a caller can check every refusal carried the same one.
fn assert_gate_envelope(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, GATE_STATUS,
        "case {case:?}: expected HTTP {GATE_STATUS}, got {answer:?}"
    );
    assert_eq!(
        answer.error_type, GATE_CODE,
        "case {case:?}: expected the x-amzn-errortype header to name {GATE_CODE}, got {answer:?}"
    );
    assert_eq!(
        answer.body_code(),
        GATE_CODE,
        "case {case:?}: the body's __type must carry the same code as the header, got {answer:?}"
    );
    let message = answer.message();
    assert!(
        !message.is_empty(),
        "case {case:?}: the refusal carries a message, got {answer:?}"
    );
    // A gated caller gets no success representation: none of the success members of the
    // operations above may ride along with the refusal.
    for member in [
        "devices",
        "tasks",
        "executions",
        "resources",
        "taskId",
        "managedDeviceId",
        "instances",
    ] {
        assert!(
            answer.body[member].is_null(),
            "case {case:?}: a refusal carries no part of a success payload, found {member:?} in \
             {answer:?}"
        );
    }
    message.to_string()
}

#[tokio::test]
async fn the_gate_refusal_carries_the_same_wire_envelope_on_every_gated_operation() {
    // Two inventory states make a signed Region device-free: an account that has
    // activated nothing anywhere, and an account whose devices are all in another
    // Region. Neither has a device in the signed Region, so both answer the
    // precondition — the refusal is a property of the Region, not of the account total.
    let (bare, _) = emulator_with(STOCKED_REGION, 0);
    let (stocked, activated) = emulator_with(STOCKED_REGION, ACTIVATED_DEVICES);
    let owned_device = activated
        .first()
        .expect("the activation seam put devices in the stocked Region")
        .clone();

    let mut pairs = id_pairs();
    // An id that really EXISTS, in the stocked Region: the gate outranks id resolution,
    // so the signed Region's emptiness answers even then.
    pairs.push((owned_device.clone(), "probe-task-ffffffff".to_string()));

    let mut messages: BTreeSet<String> = BTreeSet::new();
    let mut envelopes: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for (inventory, router) in [
        ("no device activated anywhere", &bare),
        ("devices activated in another Region only", &stocked),
    ] {
        for region in DEVICE_FREE_REGIONS {
            for (device, task) in &pairs {
                for request in gated_requests(device, task) {
                    let case = format!("{} in {region} ({inventory})", request.operation);
                    let answer =
                        call(router, region, request.method, &request.uri, request.body).await;
                    messages.insert(assert_gate_envelope(&answer, &case));
                    envelopes.insert((
                        answer.status,
                        answer.error_type.clone(),
                        answer.body_code().to_string(),
                    ));
                }
            }
        }
    }

    assert_eq!(
        envelopes,
        BTreeSet::from([(
            GATE_STATUS,
            GATE_CODE.to_string(),
            GATE_CODE.to_string()
        )]),
        "case \"one envelope\": every gated operation answers a device-free Region the identical \
         status, header and body code, got {envelopes:?}"
    );
    assert_eq!(
        messages.len(),
        1,
        "case \"one message\": the refusal is ONE message for the whole class, never fitted to \
         the request, got {messages:?}"
    );

    // Nothing the refusals touched was created: the Region that meets the precondition
    // still holds no task, so a refused CreateTask minted none.
    let tasks = call(&stocked, STOCKED_REGION, "GET", "/tasks", None).await;
    assert_eq!(
        tasks.body["tasks"],
        json!([]),
        "case \"a refused CreateTask mints nothing\": got {tasks:?}"
    );
}

#[tokio::test]
async fn a_region_holding_an_inventory_answers_its_own_envelope_instead() {
    // The control: in the Region that meets the precondition the same operations answer
    // their OWN envelopes — the per-resource 404 for an id naming nothing, and a 200
    // with no error header at all for what the store can serve.
    let (router, activated) = emulator_with(STOCKED_REGION, ACTIVATED_DEVICES);
    let device = activated
        .first()
        .expect("the activation seam put devices in the stocked Region")
        .clone();

    // An id naming nothing in a served Region is the modelled not-found, header and
    // all — never the precondition.
    for (unknown_device, unknown_task) in id_pairs() {
        for request in gated_requests(&unknown_device, &unknown_task) {
            let case = format!("{} in {STOCKED_REGION}", request.operation);
            let answer = call(
                &router,
                STOCKED_REGION,
                request.method,
                &request.uri,
                request.body,
            )
            .await;
            assert_ne!(
                answer.status, GATE_STATUS,
                "case {case:?}: a Region holding an inventory is not gated, got {answer:?}"
            );
            if answer.status == NOT_FOUND_STATUS {
                assert_eq!(
                    answer.error_type, NOT_FOUND_CODE,
                    "case {case:?}: expected the x-amzn-errortype header to name \
                     {NOT_FOUND_CODE}, got {answer:?}"
                );
                assert_eq!(
                    answer.body_code(),
                    NOT_FOUND_CODE,
                    "case {case:?}: the body's __type must carry the same code as the header, \
                     got {answer:?}"
                );
            } else {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the collection operations serve this Region, got {answer:?}"
                );
                assert!(
                    answer.error_type.is_empty(),
                    "case {case:?}: a served answer sets no x-amzn-errortype, got {answer:?}"
                );
            }
        }
    }

    // And the round trip through the served Region: the device the inventory holds
    // describes, a task created against it reads back, and its execution is reported.
    let described = call(
        &router,
        STOCKED_REGION,
        "POST",
        &format!("/managed-device/{device}/describe"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case \"DescribeDevice of an activated device\": expected HTTP 200, got {described:?}"
    );
    assert_eq!(
        described.body["managedDeviceId"].as_str(),
        Some(device.as_str()),
        "case \"DescribeDevice of an activated device\": got {described:?}"
    );

    let created = call(
        &router,
        STOCKED_REGION,
        "POST",
        "/task",
        Some(json!({ "targets": [&device], "command": { "reboot": {} } })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateTask in a served Region\": expected HTTP 200, got {created:?}"
    );
    let task = created.body["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"CreateTask in a served Region\": got {created:?}"))
        .to_string();

    let read_back = call(
        &router,
        STOCKED_REGION,
        "POST",
        &format!("/task/{task}"),
        None,
    )
    .await;
    assert_eq!(
        read_back.body["taskId"].as_str(),
        Some(task.as_str()),
        "case \"DescribeTask of the created task\": the create reads back, got {read_back:?}"
    );
    let execution = call(
        &router,
        STOCKED_REGION,
        "POST",
        &format!("/task/{task}/execution/{device}"),
        None,
    )
    .await;
    assert_eq!(
        execution.body["managedDeviceId"].as_str(),
        Some(device.as_str()),
        "case \"DescribeExecution of the created task\": got {execution:?}"
    );

    // The same task id, asked of a device-free Region of the same account: the
    // precondition, not the answer above — a record that exists is still not disclosed
    // there.
    let gated = call(
        &router,
        DEVICE_FREE_REGIONS[0],
        "POST",
        &format!("/task/{task}"),
        None,
    )
    .await;
    assert_gate_envelope(
        &gated,
        "DescribeTask of an existing task from a device-free Region",
    );
}
