// SPDX-License-Identifier: CC0-1.0

//! The account-level device gate, and the front-door refusal of a `CreateTask` that
//! names no target, end to end over HTTP.
//!
//! The rule under test: in a Region where this account has activated Snow Device
//! Management on no Snow Family device, EVERY operation the model declares
//! `AccessDeniedException` on is refused with that error (HTTP 403, a message naming
//! device/Region availability) BEFORE the request's ids are resolved — so a gated
//! caller never learns whether a device or a task exists — and the refusal is a
//! precondition, so repeated and concurrent calls answer it identically. A Region that
//! HAS an activated device answers the same calls from state: the per-resource
//! `ResourceNotFoundException` for an id it does not hold, the resource for one it
//! does.
//!
//! Rows differ along what the rule generalises over: the operation, the Region, the id
//! form, the repeat, and — for the `targets` refusal — the size of the list.
//!
//! Nothing here asserts a value the emulator invented: the device inventory is put in
//! state through the one activation seam (the API models no device-creation operation),
//! and every answer is read back over the wire.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region this suite activates devices in.
const ACTIVE_REGION: &str = "ap-southeast-2";

/// Regions this account has activated no device in. Three of them, so the gate is
/// shown to be a property of the Region the request is signed for rather than of one
/// particular Region name.
const GATED_REGIONS: &[&str] = &["us-east-1", "eu-west-1", "us-west-2"];

/// `TaskState`'s values, from
/// `smithy_query(query="shape",shape="TaskState")` — the filter `ListTasks` admits.
const TASK_STATES: &[&str] = &["IN_PROGRESS", "CANCELED", "COMPLETED"];

/// The bound `CreateTaskInput$targets` is modelled with (`@length(min: 1, max: 10)`),
/// spelled out here so a wrong bound in the implementation cannot make this pass.
const TARGETS_FLOOR: usize = 1;
const TARGETS_CEILING: usize = 10;

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let code = self.body["__type"].as_str().unwrap_or_default();
        // restJson1 renders `__type` as a shape id on some services; the code is its
        // last segment.
        code.rsplit('#').next().unwrap_or_default()
    }

    /// The `message` member of an error body, or the empty string.
    fn message(&self) -> &str {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
    }
}

/// One emulator, with Snow Device Management activated on `count` devices in
/// `region` — `count = 0` is an account that has ordered nothing, which is the gated
/// state. Answers the router and the ids of the devices it activated.
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
        .header("host", format!("snow-device-management.{region}.amazonaws.com"))
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

/// One request per gated operation, shaped so that nothing but the gate can refuse it
/// first: every id is well formed and every body satisfies the model's constraints.
///
/// The set is the implemented operations the model declares `AccessDeniedException`
/// on (`smithy_query(query="digest")`): ListDevices, ListTasks, DescribeDevice,
/// DescribeTask, DescribeExecution, DescribeDeviceEc2Instances, CreateTask,
/// CancelTask and ListExecutions, plus the three list operations carrying a
/// `nextToken` this service never issued. ListDeviceResources answers the gate too
/// (it is still scaffolded past it), and the tag operations do not declare the
/// error at all.
fn gated_requests(device_id: &str, task_id: &str) -> Vec<(String, &'static str, String, Option<Value>)> {
    vec![
        (
            "ListDevices".to_string(),
            "GET",
            "/managed-devices".to_string(),
            None,
        ),
        (
            "ListDevices?maxResults=1".to_string(),
            "GET",
            "/managed-devices?maxResults=1".to_string(),
            None,
        ),
        ("ListTasks".to_string(), "GET", "/tasks".to_string(), None),
        (
            "ListTasks?state=IN_PROGRESS".to_string(),
            "GET",
            "/tasks?state=IN_PROGRESS".to_string(),
            None,
        ),
        (
            "DescribeDevice".to_string(),
            "POST",
            format!("/managed-device/{device_id}/describe"),
            None,
        ),
        (
            "DescribeTask".to_string(),
            "POST",
            format!("/task/{task_id}"),
            None,
        ),
        (
            "CreateTask".to_string(),
            "POST",
            "/task".to_string(),
            Some(json!({ "targets": [device_id], "command": { "unlock": {} } })),
        ),
        (
            "CancelTask".to_string(),
            "POST",
            format!("/task/{task_id}/cancel"),
            None,
        ),
        (
            "ListExecutions".to_string(),
            "GET",
            format!("/executions?taskId={task_id}"),
            None,
        ),
        // A `nextToken` this service never issued is request-carried state like an
        // id, so the gate answers it too rather than judging the token.
        (
            "ListDevices?nextToken=<unissued>".to_string(),
            "GET",
            "/managed-devices?nextToken=zz".to_string(),
            None,
        ),
        (
            "ListTasks?nextToken=<unissued>".to_string(),
            "GET",
            "/tasks?nextToken=zz".to_string(),
            None,
        ),
        (
            "ListExecutions?nextToken=<unissued>".to_string(),
            "GET",
            format!("/executions?taskId={task_id}&nextToken=zz"),
            None,
        ),
        (
            "DescribeExecution".to_string(),
            "POST",
            format!("/task/{task_id}/execution/{device_id}"),
            None,
        ),
        (
            "ListDeviceResources".to_string(),
            "GET",
            format!("/managed-device/{device_id}/resources"),
            None,
        ),
        (
            "DescribeDeviceEc2Instances".to_string(),
            "POST",
            format!("/managed-device/{device_id}/resources/ec2/describe"),
            Some(json!({ "instanceIds": ["i-0123456789abcdef0"] })),
        ),
    ]
}

/// Assert `answer` is the account-level device gate: HTTP 403 `AccessDeniedException`
/// whose message states the precondition (devices in this Region) rather than an
/// identity's permissions.
fn assert_gated(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected the account-level device gate to answer HTTP 403, got {} with {}",
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
        "case {case:?}: the gate's message must name device/Region availability, not an \
         identity's permissions, got {:?}",
        answer.message()
    );
}

#[tokio::test]
async fn every_gated_operation_is_refused_in_a_region_with_no_activated_device() {
    // Rows: one per gated operation, in each of several Regions this account has
    // activated no device in.
    for region in GATED_REGIONS {
        let (router, activated) = emulator_with(region, 0);
        assert!(
            activated.is_empty(),
            "case {region:?}: this emulator must start with no activated device"
        );
        for (operation, method, uri, body) in gated_requests("smd-0000000000000000", "st-0000000000000001")
        {
            let case = format!("{operation} in {region}");
            let answer = call(&router, region, method, &uri, body).await;
            assert_gated(&answer, &case);
        }
    }
}

#[tokio::test]
async fn the_gate_is_regional_and_lifts_only_where_a_device_is_activated() {
    let (router, activated) = emulator_with(ACTIVE_REGION, 2);
    let device = activated
        .first()
        .expect("two devices were activated")
        .clone();

    // The Region the devices were activated in answers from state.
    let body = call(&router, ACTIVE_REGION, "GET", "/managed-devices", None).await;
    assert_eq!(
        body.status, 200,
        "case \"active region\": a Region with activated devices must answer ListDevices, got {}",
        body.body
    );
    let listed: BTreeSet<String> = body.body["devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"active region\": expected devices, got {}", body.body))
        .iter()
        .map(|device| {
            device["managedDeviceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed,
        activated.iter().cloned().collect::<BTreeSet<String>>(),
        "case \"active region\": ListDevices must report exactly the activated inventory"
    );

    // Every other Region of the same account is still gated — including for the very
    // device ids that exist in the active one.
    for region in GATED_REGIONS {
        let case = format!("DescribeDevice {device} in {region}");
        let answer = call(
            &router,
            region,
            "POST",
            &format!("/managed-device/{device}/describe"),
            None,
        )
        .await;
        assert_gated(&answer, &case);

        let case = format!("ListDevices in {region}");
        assert_gated(
            &call(&router, region, "GET", "/managed-devices", None).await,
            &case,
        );
    }
}

#[tokio::test]
async fn the_gate_precedes_the_per_resource_lookup() {
    // The same ids, asked of a gated account and of an activated one: the gated
    // account answers 403 for all of them (it discloses nothing about the id), the
    // activated one resolves the id and answers the modelled 404.
    let rows: &[(&str, &str)] = &[
        // (operation, uri) — every id here is well formed and names nothing.
        ("DescribeDevice", "/managed-device/smd-ffffffffffffffff/describe"),
        ("DescribeDevice", "/managed-device/does-not-exist/describe"),
        ("DescribeTask", "/task/st-ffffffffffffffff"),
        ("CancelTask", "/task/st-ffffffffffffffff/cancel"),
        ("ListExecutions", "/executions?taskId=st-ffffffffffffffff"),
    ];

    let (gated, _) = emulator_with(ACTIVE_REGION, 0);
    let (activated, _) = emulator_with(ACTIVE_REGION, 1);
    for (operation, uri) in rows {
        let method = if uri.starts_with("/executions") {
            "GET"
        } else {
            "POST"
        };
        let case = format!("{operation} {uri} on a gated account");
        assert_gated(&call(&gated, ACTIVE_REGION, method, uri, None).await, &case);

        let case = format!("{operation} {uri} on an activated account");
        let answer = call(&activated, ACTIVE_REGION, method, uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: past the gate an unknown id is the modelled \
             ResourceNotFoundException, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
    }

    // CreateTask the same way: the gate precedes resolving its targets.
    let create = json!({ "targets": ["smd-ffffffffffffffff"], "command": { "unlock": {} } });
    assert_gated(
        &call(
            &gated,
            ACTIVE_REGION,
            "POST",
            "/task",
            Some(create.clone()),
        )
        .await,
        "CreateTask on a gated account",
    );
    let answer = call(&activated, ACTIVE_REGION, "POST", "/task", Some(create)).await;
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case \"CreateTask on an activated account\": past the gate an unknown target is \
         the modelled ResourceNotFoundException, got {}",
        answer.body
    );
}

#[tokio::test]
async fn the_gate_is_a_precondition_so_repeated_and_concurrent_calls_answer_it_identically() {
    let (router, _) = emulator_with(ACTIVE_REGION, 0);

    // Repeated over time: a precondition does not come and go.
    let mut samples: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for sample in 0..4 {
        let case = format!("sample {sample}");
        let answer = call(&router, ACTIVE_REGION, "GET", "/managed-devices", None).await;
        assert_gated(&answer, &case);
        samples.insert((
            answer.status,
            answer.error_code().to_string(),
            answer.message().to_string(),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert_eq!(
        samples.len(),
        1,
        "case \"repeat\": the gate must answer every sample identically, got {samples:?}"
    );

    // And concurrently: eight CreateTask calls against the same target.
    let body = json!({ "targets": ["smd-0000000000000000"], "command": { "reboot": {} } });
    let one = || call(&router, ACTIVE_REGION, "POST", "/task", Some(body.clone()));
    let answers = tokio::join!(one(), one(), one(), one(), one(), one(), one(), one());
    let concurrent = [
        answers.0, answers.1, answers.2, answers.3, answers.4, answers.5, answers.6, answers.7,
    ];
    let mut codes: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for (index, answer) in concurrent.iter().enumerate() {
        assert_gated(answer, &format!("concurrent CreateTask {index}"));
        codes.insert((
            answer.status,
            answer.error_code().to_string(),
            answer.message().to_string(),
        ));
    }
    assert_eq!(
        codes.len(),
        1,
        "case \"concurrent\": every concurrent call must answer the same gate, got {codes:?}"
    );
    assert_eq!(
        codes, samples,
        "case \"concurrent vs repeat\": the gate is one answer, not two, got {codes:?} and \
         {samples:?}"
    );
}

#[tokio::test]
async fn create_task_that_names_no_target_is_denied_at_the_front_door() {
    // Rows span the `targets` size class: no member at all, an empty list, the
    // modelled bounds, and one past the maximum. A list that names no device is a
    // front-door denial (`UnauthorizedException` / "Unauthorized"); a list that is too
    // LONG is a request-shape problem and keeps `ValidationException`.
    let (router, activated) = emulator_with(ACTIVE_REGION, 3);
    let device = activated.first().expect("three devices were activated");

    /// What a row expects: the error code and a fragment its message must carry, or
    /// `None` for a call that must succeed.
    struct Row {
        case: &'static str,
        targets: Option<Vec<String>>,
        expected: Option<(&'static str, &'static str)>,
    }
    let rows = vec![
        Row {
            case: "targets absent",
            targets: None,
            expected: Some(("UnauthorizedException", "Unauthorized")),
        },
        Row {
            case: "targets empty",
            targets: Some(vec![]),
            expected: Some(("UnauthorizedException", "Unauthorized")),
        },
        Row {
            case: "targets at the modelled floor",
            targets: Some(vec![device.clone(); TARGETS_FLOOR]),
            expected: None,
        },
        Row {
            case: "targets at the modelled ceiling",
            targets: Some(vec![device.clone(); TARGETS_CEILING]),
            expected: None,
        },
        Row {
            case: "targets one past the modelled ceiling",
            targets: Some(vec![device.clone(); TARGETS_CEILING + 1]),
            // A list that is too LONG is a request-shape problem: the message is
            // built from the request and names the member and its bound.
            expected: Some(("ValidationException", "targets")),
        },
        Row {
            case: "targets names an element that is not a device id",
            targets: Some(vec![String::new()]),
            // The list is NOT empty — it names one (unusable) device id — so this is
            // not the front-door denial: it is resolved as a target like any other
            // and names no device this account manages.
            expected: Some(("ResourceNotFoundException", "managed-device")),
        },
    ];

    for row in &rows {
        let mut request = json!({ "command": { "unlock": {} } });
        if let Some(targets) = &row.targets {
            request["targets"] = json!(targets);
        }
        let answer = call(
            &router,
            ACTIVE_REGION,
            "POST",
            "/task",
            Some(request.clone()),
        )
        .await;
        match row.expected {
            None => assert_eq!(
                answer.status, 200,
                "case {:?}: a targets list inside the modelled bounds must create a task, \
                 got {} with {}",
                row.case, answer.status, answer.body
            ),
            Some((code, fragment)) => {
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {:?}: expected {code}, got {}",
                    row.case,
                    answer.body
                );
                let message = answer.message();
                assert!(
                    message.contains(fragment),
                    "case {:?}: expected a {code} message carrying {fragment:?}, got \
                     {message:?}",
                    row.case
                );
                if code == "UnauthorizedException" {
                    assert_eq!(
                        message, "Unauthorized",
                        "case {:?}: the front-door denial states nothing about the request's \
                         shape, got {message:?}",
                        row.case
                    );
                }
            }
        }
    }

    // The denial precedes the account-level gate too: an account with no activated
    // device still answers it for a CreateTask that names no target.
    let (gated, _) = emulator_with(ACTIVE_REGION, 0);
    let answer = call(
        &gated,
        ACTIVE_REGION,
        "POST",
        "/task",
        Some(json!({ "targets": [], "command": { "unlock": {} } })),
    )
    .await;
    assert_eq!(
        answer.error_code(),
        "UnauthorizedException",
        "case \"no targets on a gated account\": the front door denies the call before the \
         service's own gate, got {}",
        answer.body
    );
}

#[tokio::test]
async fn list_tasks_reports_the_tasks_created_in_the_region_and_filters_them_by_state() {
    let (router, activated) = emulator_with(ACTIVE_REGION, 3);

    // Nothing has been created yet, so the list is empty — the Region has a device, so
    // the gate does not fire.
    let empty = call(&router, ACTIVE_REGION, "GET", "/tasks", None).await;
    assert_eq!(
        empty.status, 200,
        "case \"before any create\": expected HTTP 200, got {} with {}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.body["tasks"],
        json!([]),
        "case \"before any create\": an account that created no task has an empty list, got {}",
        empty.body
    );

    // Rows differ in target count and command, so the listing is not keyed to one
    // shape of task.
    let rows: &[(&str, usize)] = &[("unlock", 1), ("reboot", 2), ("unlock", 3)];
    let mut created: Vec<String> = Vec::new();
    for (command, target_count) in rows {
        let case = format!("{command} over {target_count} targets");
        let targets: Vec<String> = activated.iter().take(*target_count).cloned().collect();
        let answer = call(
            &router,
            ACTIVE_REGION,
            "POST",
            "/task",
            Some(json!({ "targets": targets, "command": { *command: {} } })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateTask to succeed, got {} with {}",
            answer.status, answer.body
        );
        created.push(
            answer.body["taskId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a taskId, got {}", answer.body))
                .to_string(),
        );
    }

    // ListTasks is a projection of those records: every created task, once, with the
    // ARN and state its own DescribeTask reports.
    let listed = call(&router, ACTIVE_REGION, "GET", "/tasks", None).await;
    let tasks = listed.body["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"after the creates\": expected tasks, got {}", listed.body))
        .clone();
    assert_eq!(
        tasks.len(),
        created.len(),
        "case \"after the creates\": ListTasks must report every created task, got {}",
        listed.body
    );
    for task in &tasks {
        let task_id = task["taskId"].as_str().unwrap_or_default().to_string();
        let case = format!("taskId={task_id}");
        assert!(
            created.contains(&task_id),
            "case {case:?}: ListTasks reported a task nothing created, got {task}"
        );
        let described = call(
            &router,
            ACTIVE_REGION,
            "POST",
            &format!("/task/{task_id}"),
            None,
        )
        .await;
        for member in ["taskId", "taskArn", "state", "tags"] {
            assert_eq!(
                task[member], described.body[member],
                "case {case:?}: ListTasks and DescribeTask must project the same stored \
                 {member}, got {} and {}",
                task[member], described.body[member]
            );
        }
    }

    // One row per value of `TaskState`: a freshly created task is IN_PROGRESS, so every
    // other value selects nothing.
    for state in TASK_STATES {
        let case = format!("state={state}");
        let answer = call(
            &router,
            ACTIVE_REGION,
            "GET",
            &format!("/tasks?state={state}"),
            None,
        )
        .await;
        let filtered = answer.body["tasks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected tasks, got {}", answer.body))
            .clone();
        let expected = if *state == "IN_PROGRESS" {
            created.len()
        } else {
            0
        };
        assert_eq!(
            filtered.len(),
            expected,
            "case {case:?}: expected {expected} tasks in state {state}, got {}",
            answer.body
        );
        for task in &filtered {
            assert_eq!(
                task["state"].as_str(),
                Some(*state),
                "case {case:?}: a filtered page must hold only that state, got {task}"
            );
        }
    }

    // A value outside the modelled enum is refused, with the member and the constraint
    // the message is built from.
    let answer = call(
        &router,
        ACTIVE_REGION,
        "GET",
        "/tasks?state=BOGUS_STATE",
        None,
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"state=BOGUS_STATE\": expected HTTP 400, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case \"state=BOGUS_STATE\": expected ValidationException, got {}",
        answer.body
    );
    let message = answer.message();
    assert!(
        message.contains("BOGUS_STATE") && message.contains("state"),
        "case \"state=BOGUS_STATE\": the message is built from the request, so it names the \
         value and the member, got {message:?}"
    );
    for state in TASK_STATES {
        assert!(
            message.contains(state),
            "case \"state=BOGUS_STATE\": the message must list the value set the member \
             admits, missing {state}, got {message:?}"
        );
    }

    // And the pages: every page size from one task up to the whole list, concatenating
    // to the whole list exactly once, in order.
    let all: Vec<String> = tasks
        .iter()
        .map(|task| task["taskId"].as_str().unwrap_or_default().to_string())
        .collect();
    for size in 1..=all.len() {
        let case = format!("maxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match token.as_deref() {
                None => format!("/tasks?maxResults={size}"),
                Some(token) => format!("/tasks?maxResults={size}&nextToken={token}"),
            };
            let answer = call(&router, ACTIVE_REGION, "GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {}",
                answer.status, answer.body
            );
            let page = answer.body["tasks"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected tasks, got {}", answer.body))
                .clone();
            assert!(
                page.len() <= size,
                "case {case:?}: a page must hold at most maxResults tasks, got {}",
                answer.body
            );
            walked.extend(
                page.iter()
                    .map(|task| task["taskId"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging did not terminate; walked {walked:?}"
            );
            match answer.body["nextToken"].as_str() {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must concatenate to the whole task list, in order"
        );
    }

    // The tasks belong to the Region they were created in: another Region of the same
    // account is gated, and one with its own device does not see them.
    for region in GATED_REGIONS {
        let case = format!("ListTasks in {region}");
        assert_gated(&call(&router, region, "GET", "/tasks", None).await, &case);
    }
}
