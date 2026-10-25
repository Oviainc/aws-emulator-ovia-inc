// SPDX-License-Identifier: CC0-1.0

//! The front-door refusal of a `CreateTask` that names no target device, and what
//! `ListTasks` reports, end to end over HTTP.
//!
//! The rules under test. (1) A `CreateTask` that names NO target — no `targets`
//! member at all, or an empty list — has no device for the call to be authorized
//! against, so the front door denies it (`UnauthorizedException` / "Unauthorized")
//! before any constraint check; a list that is too LONG is a request-shape problem
//! and keeps `ValidationException`. (2) `ListTasks` is a projection of the tasks
//! created in the signed Region: every one of them once, filtered by `TaskState`
//! when the request names one, paged at `maxResults`, and nothing from any other
//! Region.
//!
//! What a caller signed for a Region this account has activated NO device in is
//! answered is a different rule, pinned in `device_region_gate.rs`; the last
//! assertion of the first test here pins only that the front-door denial comes even
//! earlier than that.
//!
//! Rows differ along what the rules generalise over: the size of the `targets` list
//! (absent, empty, both modelled bounds, past the maximum), the command and target
//! count of each created task, the `TaskState` filter, the page size, and the
//! Region.
//!
//! Nothing here asserts a value the emulator invented: the device inventory is put in
//! state through the one activation seam (the API models no device-creation operation),
//! and every answer is read back over the wire.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region this suite activates devices in.
const ACTIVE_REGION: &str = "ap-southeast-2";

/// Other Regions of the same account, which hold no device of their own. Three of
/// them, so the answer is shown to be a property of the Region the request is signed
/// for rather than of one particular Region name.
const OTHER_REGIONS: &[&str] = &["us-east-1", "eu-west-1", "us-west-2"];

/// One further Region of the same account that DOES hold a device of its own, and no
/// task: it is what shows `ListTasks` to be scoped to the Region the request is
/// signed for, since it meets the device/region precondition and still reports none
/// of the tasks created in [`ACTIVE_REGION`].
const PEER_REGION: &str = "eu-central-1";

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
/// `region` — `count = 0` is an account that has ordered nothing. Answers the router
/// and the ids of the devices it activated.
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

/// One emulator with `count` devices in `region` and one device in [`PEER_REGION`],
/// answering the router and the ids activated in `region`.
fn emulator_with_peer(region: &str, count: u64) -> (MultiServiceRouter, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = service_state(&*engine).expect("the in-memory engine carries this service's state");
    let activated = state
        .lock()
        .activate_devices(region, count, SystemTime::now());
    state
        .lock()
        .activate_devices(PEER_REGION, 1, SystemTime::now());
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

    // The denial is the front door's, ahead of anything this service's handler does —
    // including the device/region precondition the handler checks first: a Region this
    // account manages no device in still answers the front-door denial for a
    // `CreateTask` that names no target.
    let (bare, _) = emulator_with(ACTIVE_REGION, 0);
    let answer = call(
        &bare,
        ACTIVE_REGION,
        "POST",
        "/task",
        Some(json!({ "targets": [], "command": { "unlock": {} } })),
    )
    .await;
    assert_eq!(
        answer.error_code(),
        "UnauthorizedException",
        "case \"no targets without an inventory\": the front door denies the call before \
         the handler runs, got {}",
        answer.body
    );
}

#[tokio::test]
async fn list_tasks_reports_the_tasks_created_in_the_region_and_filters_them_by_state() {
    let (router, activated) = emulator_with_peer(ACTIVE_REGION, 3);

    // Nothing has been created yet, so the list is empty.
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
    // account that meets the device/region precondition reports none of them, and
    // reports that as an empty list.
    let case = format!("ListTasks in {PEER_REGION}");
    let answer = call(&router, PEER_REGION, "GET", "/tasks", None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["tasks"],
        json!([]),
        "case {case:?}: a Region no task was created in reports none, got {}",
        answer.body
    );

    // And a Region of the same account that has activated no device at all does not
    // get a listing to read at all: the device/region precondition refuses it.
    for region in OTHER_REGIONS {
        let case = format!("ListTasks in {region}");
        let answer = call(&router, region, "GET", "/tasks", None).await;
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case {case:?}: a Region holding no device is refused, got {} with {}",
            answer.status,
            answer.body
        );
    }
}
