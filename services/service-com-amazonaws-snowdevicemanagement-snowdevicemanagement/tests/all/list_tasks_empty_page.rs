// SPDX-License-Identifier: CC0-1.0

//! What `ListTasks` answers when no task stored in the signed Region matches the
//! request, end to end over HTTP.
//!
//! The rule under test: `ListTasks` (`GET /tasks`, `@readonly`,
//! `@paginated(items: tasks)` — `smithy_query(query="operation",operation="ListTasks")`)
//! is a projection of the tasks created in the signed Region, so a Region that meets
//! this service's device/region precondition is ANSWERED — HTTP 200 with a `tasks`
//! list, EMPTY and carrying no `nextToken` when nothing stored there matches —
//! whatever filter, page size or resume token the request carries, and a `nextToken`
//! comes back only when a page was actually cut short. The 403
//! `AccessDeniedException` the service refuses other Regions with reports DEVICE
//! availability in the Region, never an empty task list: it is decided by whether the
//! Region holds a Snow-Device-Management-activated device, so activating one there —
//! without creating a single task — turns every one of those same requests into the
//! empty page above.
//!
//! That distinction is the point of this suite: "this Region manages no device" and
//! "this Region has no task to report" are two different facts with two different
//! answers, and neither may be served as the other. The device/region refusal itself,
//! across all ten gated operations, is pinned in `device_region_gate.rs`; the full
//! listing, filtering and paging of a Region that HAS tasks in
//! `account_device_gate.rs`.
//!
//! Rows differ along what the rule generalises over: the Region, how many devices it
//! holds, the `TaskState` filter (every value the model admits), the page size (both
//! modelled bounds and a value between them), and the resume token (absent, and one
//! that resumes past every stored task).
//!
//! Nothing here asserts a value the emulator invented: the inventory is put in state
//! through the one activation seam (the API models no device-creation operation), the
//! tasks are created over the wire, and every answer is read back off the wire.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    encode_token, service_state,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Regions that hold devices, with how many each holds: the answer is a property of
/// what the Region holds, not of one Region name or one inventory size.
const STOCKED_REGIONS: &[(&str, u64)] = &[("us-west-2", 1), ("eu-central-1", 2), ("ap-south-1", 5)];

/// `TaskState`'s values — the filter `ListTasks` admits
/// (`smithy_query(query="shape",shape="TaskState")`).
const TASK_STATES: &[&str] = &["IN_PROGRESS", "CANCELED", "COMPLETED"];

/// The bounds `ListTasksInput$maxResults` is modelled with (`@range(min: 1, max: 100)`,
/// `smithy_query(query="shape",shape="MaxResults")`), spelled out so a wrong bound in
/// the implementation cannot make this pass.
const MAX_RESULTS_FLOOR: i32 = 1;
const MAX_RESULTS_CEILING: i32 = 100;

/// The wire error code and status the device/region precondition refuses with:
/// `AccessDeniedException`, `errorKind` client, `@httpError 403`.
const GATE_CODE: &str = "AccessDeniedException";
const GATE_STATUS: u16 = 403;

/// Words a refusal about IAM authorization carries and a refusal about device
/// availability in a Region must not.
const AUTHORIZATION_WORDS: &[&str] = &["not authorized", "User:", "iam", "policy", "permission"];

/// How this error class is worded: one sentence about device availability in the
/// signed Region, all fixed text — it has no variable part to fill from the request,
/// which is why the identical message covers every request shape.
const GATE_MESSAGE: &str = "No devices found in this region.";

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

/// One request shape a `ListTasks` caller can send.
struct Shape {
    case: String,
    uri: String,
}

/// The `ListTasks` request shapes every row of this suite is asked in: no filter,
/// each `TaskState`, both modelled `maxResults` bounds and one between them, and a
/// resume token that names a key past every task id this store mints — on its own and
/// combined with a page size.
fn request_shapes() -> Vec<Shape> {
    // A resume key lexically after every task id the store mints, encoded the way this
    // service encodes one, so the token is a well-formed `NextToken` that resumes past
    // the end of the listing.
    let past_the_end = encode_token("zzzzzzzzzzzzzzzz");
    let mut shapes = vec![Shape {
        case: "no filter".to_string(),
        uri: "/tasks".to_string(),
    }];
    for state in TASK_STATES {
        shapes.push(Shape {
            case: format!("state={state}"),
            uri: format!("/tasks?state={state}"),
        });
    }
    shapes.push(Shape {
        case: format!("maxResults={MAX_RESULTS_FLOOR} (the modelled floor)"),
        uri: format!("/tasks?maxResults={MAX_RESULTS_FLOOR}"),
    });
    shapes.push(Shape {
        case: format!("maxResults={MAX_RESULTS_CEILING} (the modelled ceiling)"),
        uri: format!("/tasks?maxResults={MAX_RESULTS_CEILING}"),
    });
    shapes.push(Shape {
        case: "maxResults=7 with a state filter".to_string(),
        uri: "/tasks?maxResults=7&state=IN_PROGRESS".to_string(),
    });
    shapes.push(Shape {
        case: "nextToken resuming past every stored task".to_string(),
        uri: format!("/tasks?nextToken={past_the_end}"),
    });
    shapes.push(Shape {
        case: "nextToken past every stored task, with a page size".to_string(),
        uri: format!("/tasks?nextToken={past_the_end}&maxResults={MAX_RESULTS_FLOOR}"),
    });
    shapes
}

/// One emulator over a fresh engine, answering the router and the engine so a later
/// activation reaches the same state.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Activate Snow Device Management on `count` devices in `region` through the one
/// activation seam, answering the ids it minted.
fn activate_in(engine: &Arc<engine::InMemoryEngine>, region: &str, count: u64) -> Vec<String> {
    service_state(&**engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(region, count, SystemTime::now())
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

/// Assert `answer` is the empty page: HTTP 200, a `tasks` list that is present and
/// empty, and NO `nextToken` — there is no further page to ask for.
fn assert_empty_page(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["tasks"],
        json!([]),
        "case {case:?}: expected an empty tasks list, got {}",
        answer.body
    );
    assert!(
        answer.body["nextToken"].is_null(),
        "case {case:?}: an answer with nothing left to report carries no nextToken, got {}",
        answer.body
    );
}

/// Assert `answer` is the device/region refusal, and answer its message so the caller
/// can check every row carries the same one.
fn assert_gated(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, GATE_STATUS,
        "case {case:?}: expected HTTP {GATE_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        GATE_CODE,
        "case {case:?}: expected {GATE_CODE}, got {}",
        answer.body
    );
    let message = answer.message();
    assert_eq!(
        message, GATE_MESSAGE,
        "case {case:?}: expected the device/region refusal's wording, got {message:?}"
    );
    let lowered = message.to_lowercase();
    assert!(
        lowered.contains("device") && lowered.contains("region"),
        "case {case:?}: the refusal states that the Region holds no device, got {message:?}"
    );
    for word in AUTHORIZATION_WORDS {
        assert!(
            !lowered.contains(&word.to_lowercase()),
            "case {case:?}: the refusal is about device availability, not about the caller's \
             authorization, so it must not carry {word:?}, got {message:?}"
        );
    }
    message.to_string()
}

#[tokio::test]
async fn a_region_with_devices_and_no_task_answers_every_request_shape_with_an_empty_page() {
    // Rows: three Regions holding one, two and five devices, each asked in every
    // request shape. No task has been created anywhere, so nothing matches — and
    // nothing matching is a 200 with an empty list, not a refusal.
    for (region, devices) in STOCKED_REGIONS {
        let (router, engine) = emulator();
        let activated = activate_in(&engine, region, *devices);
        assert_eq!(
            activated.len(),
            usize::try_from(*devices).expect("a device count fits in usize"),
            "case {region:?}: the activation seam put {devices} devices in the Region, got \
             {activated:?}"
        );
        for shape in request_shapes() {
            let case = format!("{} devices in {region}, {}", devices, shape.case);
            let answer = call(&router, region, "GET", &shape.uri, None).await;
            assert_empty_page(&answer, &case);
        }
    }
}

#[tokio::test]
async fn a_filter_that_matches_no_stored_task_is_an_empty_page_not_a_refusal() {
    // A Region that really does hold tasks: a filter selecting none of them is still
    // the empty page, and a `nextToken` comes back only when a page was cut short.
    let (region, devices) = STOCKED_REGIONS[2];
    let (router, engine) = emulator();
    let activated = activate_in(&engine, region, devices);

    // Rows differ in command and target count, so the listing is not keyed to one
    // shape of task.
    let rows: &[(&str, usize)] = &[("unlock", 1), ("reboot", 2), ("unlock", 3)];
    let mut created: Vec<String> = Vec::new();
    for (command, target_count) in rows {
        let case = format!("{command} over {target_count} targets");
        let targets: Vec<String> = activated.iter().take(*target_count).cloned().collect();
        let answer = call(
            &router,
            region,
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
    created.sort();

    // A task reaches `CANCELED` only through `CancelTask`, so nothing the clock owes
    // can put a task in that state behind this assertion's back: the filter matches
    // nothing, and that is an empty page with no further page to ask for.
    for uri in [
        "/tasks?state=CANCELED",
        "/tasks?state=CANCELED&maxResults=1",
        &format!("/tasks?nextToken={}", encode_token("zzzzzzzzzzzzzzzz")),
    ] {
        let case = format!("{} stored tasks, {uri}", created.len());
        let answer = call(&router, region, "GET", uri, None).await;
        assert_empty_page(&answer, &case);
    }

    // The control: the same Region, unfiltered, reports the stored tasks — so the
    // empty pages above are the filter and the token speaking, not an unreachable
    // listing. The default page holds all three, so there is still no `nextToken`.
    let listed = call(&router, region, "GET", "/tasks", None).await;
    let reported: Vec<String> = listed.body["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"unfiltered\": expected tasks, got {}", listed.body))
        .iter()
        .map(|task| task["taskId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, created,
        "case \"unfiltered\": ListTasks reports every task created in the Region, got {}",
        listed.body
    );
    assert!(
        listed.body["nextToken"].is_null(),
        "case \"unfiltered\": a page that reported every task carries no nextToken, got {}",
        listed.body
    );

    // And a page that WAS cut short does carry one: `nextToken` marks a continuation,
    // never an empty answer.
    let cut = call(
        &router,
        region,
        "GET",
        &format!("/tasks?maxResults={MAX_RESULTS_FLOOR}"),
        None,
    )
    .await;
    assert_eq!(
        cut.body["tasks"].as_array().map(Vec::len),
        Some(usize::try_from(MAX_RESULTS_FLOOR).expect("a page size fits in usize")),
        "case \"maxResults at the floor\": expected one task on the page, got {}",
        cut.body
    );
    assert!(
        cut.body["nextToken"].is_string(),
        "case \"maxResults at the floor\": a page cut short carries a nextToken, got {}",
        cut.body
    );
}

#[tokio::test]
async fn activating_a_device_turns_the_regions_refusal_into_the_empty_page() {
    // The refusal is about devices, not about tasks: the same Region, with the same
    // (empty) task store, is refused while it holds no device and answers the empty
    // page once it holds one — with no task created in between.
    let (stocked, devices) = STOCKED_REGIONS[0];
    let gated = "eu-west-1";
    let (router, engine) = emulator();
    activate_in(&engine, stocked, devices);

    let mut messages: Vec<String> = Vec::new();
    for shape in request_shapes() {
        let case = format!("{} before activation, {}", gated, shape.case);
        let answer = call(&router, gated, "GET", &shape.uri, None).await;
        messages.push(assert_gated(&answer, &case));
    }
    messages.dedup();
    assert_eq!(
        messages.len(),
        1,
        "case \"one message\": the refusal is decided before the request's filters are \
         read, so every shape gets the identical message, got {messages:?}"
    );

    activate_in(&engine, gated, 1);

    for shape in request_shapes() {
        let case = format!("{} after activation, {}", gated, shape.case);
        let answer = call(&router, gated, "GET", &shape.uri, None).await;
        assert_empty_page(&answer, &case);
    }

    // A third Region of the same account, which still holds no device, is still
    // refused: the precondition is per Region and is read from state.
    let answer = call(&router, "us-east-1", "GET", "/tasks", None).await;
    assert_gated(&answer, "us-east-1 still holds no device");
}
