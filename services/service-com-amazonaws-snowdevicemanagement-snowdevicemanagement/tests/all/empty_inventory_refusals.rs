// SPDX-License-Identifier: CC0-1.0

//! What this service answers a caller whose account manages no device in the signed
//! Region, end to end over HTTP.
//!
//! The rule under test: every answer is decided by what the request names against
//! the records this account holds in the signed Region — an id that names no such
//! record is the modelled `ResourceNotFoundException` (HTTP 404), a reporting
//! operation over a collection that holds nothing is HTTP 200 with an empty page,
//! and a `nextToken` this service never issued is the modelled
//! `ValidationException` (HTTP 400). `AccessDeniedException` (HTTP 403) is the
//! answer for a caller the action is not permitted for, so an empty inventory is
//! never answered with it: the same unknown id is answered the same way whether the
//! Region holds other devices or none at all.
//!
//! Rows differ along what the rule generalises over: the operation (every one the
//! model declares `AccessDeniedException` on, so none of them can be substituting
//! it), the Region, the kind of id (managed device, task, the execution pair), the
//! shape of the unknown id (both modelled `@length(1..64)` bounds, the shapes this
//! store mints, unrelated shapes, and an id that exists in ANOTHER Region), and —
//! for the reporting operations — the filters and the page size.
//!
//! Nothing here asserts a value the emulator invented: the device inventory is put
//! in state through the one activation seam (the API models no device-creation
//! operation), the task is created over the wire, and every answer is read back off
//! the wire.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Regions this account has activated no device in. Three of them, so an answer is
/// shown to be a property of the records the Region holds rather than of one
/// particular Region name.
const EMPTY_REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2"];

/// The one Region this account HAS devices in, so the same rows can be asked of a
/// Region that holds records and of Regions that hold none.
const STOCKED_REGION: &str = "us-west-2";

/// How many devices the activation seam puts in [`STOCKED_REGION`].
const ACTIVATED_DEVICES: u64 = 2;

/// The member each reporting operation carries its page in, with the request that
/// reads it. The filters and page sizes come from the modelled members
/// (`smithy_query(query="operation",operation="ListDevices,ListTasks")`:
/// `jobId`, `state`, `maxResults`).
const COLLECTION_REQUESTS: &[(&str, &str, &str)] = &[
    ("ListDevices", "/managed-devices", "devices"),
    ("ListDevices?maxResults=1", "/managed-devices?maxResults=1", "devices"),
    (
        "ListDevices?jobId",
        "/managed-devices?jobId=JID00000000-0000-4000-8000-000000000000",
        "devices",
    ),
    ("ListTasks", "/tasks", "tasks"),
    ("ListTasks?maxResults=100", "/tasks?maxResults=100", "tasks"),
    ("ListTasks?state=IN_PROGRESS", "/tasks?state=IN_PROGRESS", "tasks"),
    ("ListTasks?state=COMPLETED", "/tasks?state=COMPLETED", "tasks"),
];

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
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
    }
}

/// One emulator with [`ACTIVATED_DEVICES`] devices activated in
/// [`STOCKED_REGION`] and none anywhere else, answering the router and the ids of
/// the activated devices.
fn emulator() -> (MultiServiceRouter, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let activated = service_state(&*engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(STOCKED_REGION, ACTIVATED_DEVICES, SystemTime::now());
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
    operation: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every implemented operation that resolves an id, addressed to a device id and a
/// task id that name no record of the Region the request is signed for.
///
/// The set is the id-addressed operations the model declares both
/// `ResourceNotFoundException` and `AccessDeniedException` on
/// (`smithy_query(query="digest")`), so none of them can be answering the one in
/// place of the other.
fn addressed_requests(device: &str, task: &str) -> Vec<Request> {
    vec![
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
            body: Some(json!({ "instanceIds": ["s.i-00000000000000000"] })),
        },
        Request {
            operation: format!("CreateTask targeting {device:?}"),
            method: "POST",
            uri: "/task".to_string(),
            body: Some(json!({ "targets": [device], "command": { "unlock": {} } })),
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
    ]
}

/// Device and task ids in the shapes a client can send that name no record: the
/// modelled `@length(min: 1, max: 64)` floor and ceiling (`ManagedDeviceId`,
/// `TaskId` and `ExecutionId` are all bounded that way —
/// `smithy_query(query="shape",shape="ManagedDeviceId,TaskId")`), the shapes this
/// store mints, and two unrelated shapes.
fn unknown_id_pairs() -> Vec<(String, String)> {
    vec![
        ("x".to_string(), "y".to_string()),
        ("f".repeat(64), "e".repeat(64)),
        (
            "smd-ffffffffffffffff".to_string(),
            "st-ffffffffffffffff".to_string(),
        ),
        ("does-not-exist".to_string(), "no-such-task".to_string()),
        ("0".repeat(32), "9".repeat(32)),
    ]
}

/// Assert `answer` is the modelled not-found: HTTP 404 `ResourceNotFoundException`
/// carrying a message, and never the permission refusal.
fn assert_not_found(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: an id that names no record of this Region is the modelled \
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

/// Assert `answer` is not a permission refusal, whatever else it is.
fn assert_not_denied(answer: &Answer, case: &str) {
    assert_ne!(
        answer.status, 403,
        "case {case:?}: an account that manages no device is not a caller without \
         permission, got {} with {}",
        answer.status, answer.body
    );
    assert_ne!(
        answer.error_code(),
        "AccessDeniedException",
        "case {case:?}: expected no permission refusal, got {}",
        answer.body
    );
}

#[tokio::test]
async fn an_id_naming_no_record_is_not_found_in_a_region_with_no_devices() {
    let (router, _) = emulator();

    let mut statuses: BTreeSet<u16> = BTreeSet::new();
    for region in EMPTY_REGIONS {
        for (device, task) in unknown_id_pairs() {
            for request in addressed_requests(&device, &task) {
                let case = format!("{} in {region}", request.operation);
                let answer = call(
                    &router,
                    region,
                    request.method,
                    &request.uri,
                    request.body,
                )
                .await;
                assert_not_found(&answer, &case);
                assert_not_denied(&answer, &case);
                statuses.insert(answer.status);
            }
        }
    }
    assert_eq!(
        statuses,
        BTreeSet::from([404]),
        "case \"one outcome\": every id-addressed operation answers an unknown id the \
         same way, got {statuses:?}"
    );
}

#[tokio::test]
async fn the_answer_to_an_unknown_id_does_not_depend_on_what_else_the_region_holds() {
    let (router, activated) = emulator();
    let managed = activated
        .first()
        .expect("the activation seam put devices in the stocked Region")
        .clone();

    // The Region that HAS devices answers its own records, which is what makes the
    // 404 below a statement about the id rather than a blanket refusal.
    let described = call(
        &router,
        STOCKED_REGION,
        "POST",
        &format!("/managed-device/{managed}/describe"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case \"a managed device\": a device this account manages here must describe, \
         got {} with {}",
        described.status, described.body
    );
    assert_eq!(
        described.body["managedDeviceId"].as_str(),
        Some(managed.as_str()),
        "case \"a managed device\": DescribeDevice must report the id it was addressed \
         to, got {}",
        described.body
    );

    // A task really created in the stocked Region: known there, and nowhere else.
    let created = call(
        &router,
        STOCKED_REGION,
        "POST",
        "/task",
        Some(json!({ "targets": [&managed], "command": { "reboot": {} } })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"fixture\": CreateTask against a managed device must succeed, got {} \
         with {}",
        created.status, created.body
    );
    let task = created.body["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"fixture\": expected a taskId, got {}", created.body))
        .to_string();

    // The same unknown ids asked of the stocked Region: the answer is the 404 the
    // device-free Regions gave, so what else the Region holds changes nothing.
    for (device, unknown_task) in unknown_id_pairs() {
        for request in addressed_requests(&device, &unknown_task) {
            let case = format!("{} in {STOCKED_REGION}", request.operation);
            let answer = call(
                &router,
                STOCKED_REGION,
                request.method,
                &request.uri,
                request.body,
            )
            .await;
            assert_not_found(&answer, &case);
            assert_not_denied(&answer, &case);
        }
    }

    // And the records that DO exist in the stocked Region are unknown in every
    // device-free Region of the same account: the lookup is regional, and a Region
    // that holds nothing refuses by id, not by permission.
    for region in EMPTY_REGIONS {
        for request in addressed_requests(&managed, &task) {
            let case = format!("{} in {region}", request.operation);
            let answer = call(
                &router,
                region,
                request.method,
                &request.uri,
                request.body,
            )
            .await;
            assert_not_found(&answer, &case);
            assert_not_denied(&answer, &case);
        }
    }
}

#[tokio::test]
async fn a_collection_that_holds_nothing_is_an_empty_page_not_a_refusal() {
    let (router, _) = emulator();

    for region in EMPTY_REGIONS {
        for (operation, uri, member) in COLLECTION_REQUESTS {
            let case = format!("{operation} in {region}");
            let answer = call(&router, region, "GET", uri, None).await;
            assert_not_denied(&answer, &case);
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a collection that holds nothing is served, not refused, \
                 got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body[*member],
                json!([]),
                "case {case:?}: expected an empty {member} page, got {}",
                answer.body
            );
            assert!(
                answer.body["nextToken"].is_null(),
                "case {case:?}: an empty page has no next page, got {}",
                answer.body
            );
        }
    }

    // The stocked Region serves the same operations from ITS records: the devices it
    // holds, and no task until one is created. The two Regions differ only in what
    // they hold.
    let devices = call(&router, STOCKED_REGION, "GET", "/managed-devices", None).await;
    assert_eq!(
        devices.body["devices"]
            .as_array()
            .unwrap_or_else(|| panic!(
                "case \"stocked ListDevices\": expected devices, got {}",
                devices.body
            ))
            .len(),
        usize::try_from(ACTIVATED_DEVICES).expect("the activated count fits in usize"),
        "case \"stocked ListDevices\": expected the activated inventory, got {}",
        devices.body
    );
    let tasks = call(&router, STOCKED_REGION, "GET", "/tasks", None).await;
    assert_eq!(
        tasks.body["tasks"],
        json!([]),
        "case \"stocked ListTasks\": no task was created yet, got {}",
        tasks.body
    );
}

#[tokio::test]
async fn an_unissued_pagination_token_is_judged_on_its_own() {
    let (router, _) = emulator();

    // A `nextToken` this service never issued is a request-shape problem wherever it
    // is sent: the refusal names the member, and it is the same in a Region that
    // holds records and in one that holds none.
    let rows: &[(&str, &str)] = &[
        ("ListDevices", "/managed-devices?nextToken=zz"),
        ("ListTasks", "/tasks?nextToken=zz"),
    ];
    let mut regions: Vec<&str> = EMPTY_REGIONS.to_vec();
    regions.push(STOCKED_REGION);

    for region in regions {
        for (operation, uri) in rows {
            let case = format!("{operation} with an unissued nextToken in {region}");
            let answer = call(&router, region, "GET", uri, None).await;
            assert_not_denied(&answer, &case);
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a token this service never issued is a request-shape \
                 problem, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert!(
                answer.message().contains("nextToken"),
                "case {case:?}: the message is built from the request, so it names the \
                 member, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn no_operation_of_this_service_answers_a_permission_refusal_for_an_empty_inventory() {
    let (router, _) = emulator();

    // Every operation the model declares `AccessDeniedException` on, including the
    // one still scaffolded (ListDeviceResources), asked of a Region this account
    // manages no device in: whatever each answers, none of them answers 403.
    for region in EMPTY_REGIONS {
        let mut requests = addressed_requests("smd-ffffffffffffffff", "st-ffffffffffffffff");
        requests.push(Request {
            operation: "ListDeviceResources".to_string(),
            method: "GET",
            uri: "/managed-device/smd-ffffffffffffffff/resources".to_string(),
            body: None,
        });
        for (operation, uri, _) in COLLECTION_REQUESTS {
            requests.push(Request {
                operation: (*operation).to_string(),
                method: "GET",
                uri: (*uri).to_string(),
                body: None,
            });
        }
        for request in requests {
            let case = format!("{} in {region}", request.operation);
            let answer = call(
                &router,
                region,
                request.method,
                &request.uri,
                request.body,
            )
            .await;
            assert_not_denied(&answer, &case);
        }
    }
}
