// SPDX-License-Identifier: CC0-1.0

//! The account/region device precondition this service's device and task surface is
//! served behind, end to end over HTTP.
//!
//! The rule under test: a request signed for a Region this account has activated no
//! Snow Device Management device in is REFUSED — HTTP 403 with the wire error code
//! `AccessDeniedException` — by every operation the model declares that error on
//! (`smithy_query(query="digest")`: ListDevices, ListTasks, DescribeDevice,
//! DescribeTask, DescribeDeviceEc2Instances, ListDeviceResources, ListExecutions,
//! DescribeExecution, CreateTask, CancelTask), and that refusal is decided BEFORE any
//! id is resolved and before any field of the request is validated, so one identical
//! answer covers every request shape. The tagging operations, which the model
//! declares no `AccessDeniedException` on at all (only `ResourceNotFoundException`
//! and the service-common errors), are NOT gated and still reach their own
//! per-resource existence check. The precondition is read from state: activating a
//! device in that Region lifts it, and a Region holding an inventory answers its own
//! records.
//!
//! The message is asserted against its RULE rather than against a recorded string: it
//! is ONE message for the whole class — identical across every operation, every
//! request shape and every repetition — and it speaks of device availability in the
//! Region, never of a principal or of IAM authorization.
//!
//! Rows differ along what the rule generalises over: the operation (all ten), the
//! Region, the kind of id (managed device, task, the execution pair), the shape of the
//! id (both modelled `@length(1..64)` bounds, the shapes this store mints, unrelated
//! shapes, and ids that really exist in another Region), the filters and page sizes a
//! request can carry, the number of instance ids asked for (none, one malformed, one
//! well formed), and the number of calls (one, repeated, eight at once).
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

/// Regions this account has activated no device in. Three of them, so the refusal is
/// shown to be a property of what the Region holds rather than of one Region name.
const GATED_REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2"];

/// The one Region this account HAS devices in, so the very same rows can be asked of
/// a Region that meets the precondition and of Regions that do not.
const STOCKED_REGION: &str = "us-west-2";

/// How many devices the activation seam puts in [`STOCKED_REGION`].
const ACTIVATED_DEVICES: u64 = 2;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// This service's ARN namespace, as its `@aws.api#service` trait gives it.
const ARN_SERVICE: &str = "snow-device-management";

/// The wire error code the precondition refuses with: `AccessDeniedException`,
/// `errorKind` client, `@httpError 403`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
const GATE_CODE: &str = "AccessDeniedException";
const GATE_STATUS: u16 = 403;

/// Words a refusal about IAM authorization carries and a refusal about device
/// availability in a Region must not: the two are different classes, and a client
/// branching on the gate must not be told it is a permissions problem.
const AUTHORIZATION_WORDS: &[&str] = &["not authorized", "User:", "iam", "policy", "permission"];

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

/// One emulator with [`ACTIVATED_DEVICES`] devices activated in [`STOCKED_REGION`]
/// and none anywhere else, answering the router, the shared engine (so a later
/// activation can reach the same state) and the ids of the activated devices.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let activated = service_state(&*engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(STOCKED_REGION, ACTIVATED_DEVICES, SystemTime::now());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine, activated)
}

/// Activate `count` devices in `region` on the engine the router already serves.
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

/// Percent-encode one URI path segment the way an SDK does: an ARN carries `:` and
/// `/`, and `/tags/{resourceArn}` is a single (non-greedy) label.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        })
        .collect()
}

/// One request: the operation it names and how to send it.
struct Request {
    operation: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation the model declares `AccessDeniedException` on, addressed to
/// `device` and `task` and carrying the filters and page sizes the model admits.
///
/// The ten of them together are the gated surface; the rows within each span what a
/// request can carry, so the refusal is shown not to depend on any of it.
fn gated_requests(device: &str, task: &str) -> Vec<Request> {
    vec![
        Request {
            operation: "ListDevices".to_string(),
            method: "GET",
            uri: "/managed-devices".to_string(),
            body: None,
        },
        Request {
            operation: "ListDevices (paged)".to_string(),
            method: "GET",
            uri: "/managed-devices?maxResults=1".to_string(),
            body: None,
        },
        Request {
            // `jobId` is modelled `@length(min: 1, max: 64)` and carries no pattern,
            // so this value reaches the handler: the gate, not a filter check, is
            // what answers it.
            operation: "ListDevices (unusable jobId filter)".to_string(),
            method: "GET",
            uri: "/managed-devices?jobId=not%20a%20valid%20job%20id%21%21&maxResults=1".to_string(),
            body: None,
        },
        Request {
            operation: "ListDevices (plausible jobId filter)".to_string(),
            method: "GET",
            uri: "/managed-devices?jobId=JID00000000-0000-4000-8000-000000000000".to_string(),
            body: None,
        },
        Request {
            operation: "ListDevices (nextToken this service never issued)".to_string(),
            method: "GET",
            uri: "/managed-devices?nextToken=zz".to_string(),
            body: None,
        },
        Request {
            operation: "ListTasks".to_string(),
            method: "GET",
            uri: "/tasks".to_string(),
            body: None,
        },
        Request {
            operation: "ListTasks (state filter)".to_string(),
            method: "GET",
            uri: "/tasks?state=IN_PROGRESS&maxResults=100".to_string(),
            body: None,
        },
        Request {
            operation: "ListTasks (nextToken this service never issued)".to_string(),
            method: "GET",
            uri: "/tasks?nextToken=zz".to_string(),
            body: None,
        },
        Request {
            operation: format!("DescribeDevice {device:?}"),
            method: "POST",
            uri: format!("/managed-device/{device}/describe"),
            body: None,
        },
        Request {
            operation: format!("ListDeviceResources {device:?}"),
            method: "GET",
            uri: format!("/managed-device/{device}/resources?type=AWS%3A%3AEC2%3A%3AInstance"),
            body: None,
        },
        Request {
            operation: format!("DescribeDeviceEc2Instances {device:?} (malformed instance id)"),
            method: "POST",
            uri: format!("/managed-device/{device}/resources/ec2/describe"),
            body: Some(json!({ "instanceIds": ["not-an-instance-id"] })),
        },
        Request {
            operation: format!("DescribeDeviceEc2Instances {device:?} (well-formed instance id)"),
            method: "POST",
            uri: format!("/managed-device/{device}/resources/ec2/describe"),
            body: Some(json!({ "instanceIds": ["i-0123456789abcdef0"] })),
        },
        Request {
            operation: format!("DescribeDeviceEc2Instances {device:?} (no instance id)"),
            method: "POST",
            uri: format!("/managed-device/{device}/resources/ec2/describe"),
            body: Some(json!({ "instanceIds": [] })),
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
            uri: format!("/executions?taskId={task}&state=QUEUED"),
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
        Request {
            operation: format!("CreateTask targeting {device:?} with tags and a clientToken"),
            method: "POST",
            uri: "/task".to_string(),
            body: Some(json!({
                "targets": [device],
                "command": { "reboot": {} },
                "description": "a gated create",
                "tags": { "owner": "fleet" },
                "clientToken": "11111111-1111-4111-8111-111111111111"
            })),
        },
        Request {
            // A tag map this service refuses everywhere else (the `aws:` prefix is
            // reserved): the gate outranks that field check too.
            operation: format!("CreateTask targeting {device:?} with a reserved tag key"),
            method: "POST",
            uri: "/task".to_string(),
            body: Some(json!({
                "targets": [device],
                "command": { "unlock": {} },
                "tags": { "aws:owner": "fleet" }
            })),
        },
    ]
}

/// Device and task ids in the shapes a client can send: the modelled
/// `@length(min: 1, max: 64)` floor and ceiling (`ManagedDeviceId` and `TaskId` are
/// both bounded that way — `smithy_query(query="shape",shape="ManagedDeviceId,TaskId")`),
/// the shapes this store mints, and two unrelated shapes.
fn id_pairs() -> Vec<(String, String)> {
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

/// Assert `answer` is the device/region precondition's refusal, and hand its message
/// back so a caller can check every refusal carried the same one.
fn assert_gated(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, GATE_STATUS,
        "case {case:?}: a Region this account has activated no device in is refused, \
         expected HTTP {GATE_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        GATE_CODE,
        "case {case:?}: expected the wire error code {GATE_CODE}, got {}",
        answer.body
    );
    let message = answer.message();
    assert!(
        !message.is_empty(),
        "case {case:?}: the refusal carries a message, got {}",
        answer.body
    );
    // The message states device availability in the Region, so it names both; and it
    // is not an authorization refusal, so it names no principal and no policy.
    let lowered = message.to_lowercase();
    assert!(
        lowered.contains("device") && lowered.contains("region"),
        "case {case:?}: the refusal is about device availability in the Region, so its \
         message names both, got {message:?}"
    );
    for word in AUTHORIZATION_WORDS {
        assert!(
            !lowered.contains(&word.to_lowercase()),
            "case {case:?}: the refusal is not an IAM authorization refusal, so its \
             message must not carry {word:?}, got {message:?}"
        );
    }
    message.to_string()
}

#[tokio::test]
async fn every_gated_operation_refuses_a_region_with_no_activated_device() {
    let (router, _, activated) = emulator();

    // Every gated operation, in every device-free Region, over every shape of id a
    // client can send — plus the ids that really EXIST in the stocked Region, which is
    // what shows the gate outranks id resolution.
    let mut pairs = id_pairs();
    pairs.push((
        activated
            .first()
            .expect("the activation seam put devices in the stocked Region")
            .clone(),
        "st-0000000000000001".to_string(),
    ));

    let mut messages: BTreeSet<String> = BTreeSet::new();
    let mut statuses: BTreeSet<u16> = BTreeSet::new();
    for region in GATED_REGIONS {
        for (device, task) in &pairs {
            for request in gated_requests(device, task) {
                let case = format!("{} in {region}", request.operation);
                let answer =
                    call(&router, region, request.method, &request.uri, request.body).await;
                messages.insert(assert_gated(&answer, &case));
                statuses.insert(answer.status);
            }
        }
    }

    assert_eq!(
        statuses,
        BTreeSet::from([GATE_STATUS]),
        "case \"one outcome\": every gated operation answers a device-free Region the \
         same status, got {statuses:?}"
    );
    assert_eq!(
        messages.len(),
        1,
        "case \"one message\": the refusal is ONE message for the whole class, never \
         fitted to the request, got {messages:?}"
    );
}

#[tokio::test]
async fn the_gate_outranks_the_answers_a_served_region_gives_the_same_requests() {
    // The control: the very rows the gate swallows get their OWN answers in a Region
    // that meets the precondition, so the 403 above is the precondition speaking and
    // not a blanket refusal.
    let (router, _, activated) = emulator();
    let device = activated
        .first()
        .expect("the activation seam put devices in the stocked Region")
        .clone();

    /// A control row: the request, and the status plus wire code it must answer in a
    /// Region that holds an inventory.
    struct Control {
        case: &'static str,
        method: &'static str,
        uri: String,
        body: Option<Value>,
        status: u16,
        code: &'static str,
    }
    let controls = vec![
        Control {
            case: "ListDevices with an unusable jobId filter is served, not refused",
            method: "GET",
            uri: "/managed-devices?jobId=not%20a%20valid%20job%20id%21%21&maxResults=1".to_string(),
            body: None,
            status: 200,
            code: "",
        },
        Control {
            case: "ListDevices with a nextToken this service never issued is a 400",
            method: "GET",
            uri: "/managed-devices?nextToken=zz".to_string(),
            body: None,
            status: 400,
            code: "ValidationException",
        },
        Control {
            case: "DescribeDevice of an id naming nothing here is a 404",
            method: "POST",
            uri: "/managed-device/smd-ffffffffffffffff/describe".to_string(),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Control {
            case: "DescribeDeviceEc2Instances of a managed device with no instance id is served",
            method: "POST",
            uri: format!("/managed-device/{device}/resources/ec2/describe"),
            body: Some(json!({ "instanceIds": [] })),
            status: 200,
            code: "",
        },
        Control {
            case: "CreateTask with a reserved tag key is a 400",
            method: "POST",
            uri: "/task".to_string(),
            body: Some(json!({
                "targets": [&device],
                "command": { "unlock": {} },
                "tags": { "aws:owner": "fleet" }
            })),
            status: 400,
            code: "ValidationException",
        },
        Control {
            case: "CancelTask of a task naming nothing here is a 404",
            method: "POST",
            uri: "/task/st-ffffffffffffffff/cancel".to_string(),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Control {
            case: "ListTasks of a Region that created none is an empty page",
            method: "GET",
            uri: "/tasks".to_string(),
            body: None,
            status: 200,
            code: "",
        },
    ];

    for control in controls {
        let served = call(
            &router,
            STOCKED_REGION,
            control.method,
            &control.uri,
            control.body.clone(),
        )
        .await;
        assert_eq!(
            served.status, control.status,
            "case {:?}: expected HTTP {} in {STOCKED_REGION}, got {} with {}",
            control.case, control.status, served.status, served.body
        );
        assert_eq!(
            served.error_code(),
            control.code,
            "case {:?}: expected the wire error code {:?} in {STOCKED_REGION}, got {}",
            control.case,
            control.code,
            served.body
        );

        // The same request in a device-free Region: the precondition, whatever the
        // served answer above was.
        let gated_case = format!("{} \u{2014} gated Region", control.case);
        let gated = call(
            &router,
            GATED_REGIONS[0],
            control.method,
            &control.uri,
            control.body,
        )
        .await;
        assert_gated(&gated, &gated_case);
    }
}

#[tokio::test]
async fn the_refusal_is_the_same_on_repeated_and_on_concurrent_calls() {
    let (router, _, _) = emulator();
    let region = GATED_REGIONS[1];
    let task = "sim2real-no-such-task";

    // Repeated reads: the gate is a property of state, so nothing resolves it in the
    // background and nothing flaps between a success and a refusal.
    let mut seen: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for attempt in 0..4 {
        let case = format!("ListDevices sample {attempt} in {region}");
        let answer = call(&router, region, "GET", "/managed-devices?maxResults=1", None).await;
        assert_gated(&answer, &case);
        seen.insert((
            answer.status,
            answer.error_code().to_string(),
            answer.message().to_string(),
        ));

        let case = format!("CancelTask sample {attempt} in {region}");
        let answer = call(&router, region, "POST", &format!("/task/{task}/cancel"), None).await;
        assert_gated(&answer, &case);
        seen.insert((
            answer.status,
            answer.error_code().to_string(),
            answer.message().to_string(),
        ));
    }
    assert_eq!(
        seen.len(),
        1,
        "case \"repeated samples\": every sample answers the identical status, code and \
         message, got {seen:?}"
    );

    // Eight CreateTask calls at once against the same target that names no device:
    // every one of them is refused the same way, with no racing success and no second
    // code.
    let create = |label: &'static str| {
        let router = &router;
        async move {
            let answer = call(
                router,
                region,
                "POST",
                "/task",
                Some(json!({
                    "targets": ["mdev-no-such-device"],
                    "command": { "unlock": {} },
                    "description": label
                })),
            )
            .await;
            (label, answer)
        }
    };
    let racers = tokio::join!(
        create("a"),
        create("b"),
        create("c"),
        create("d"),
        create("e"),
        create("f"),
        create("g"),
        create("h"),
    );
    let racers = [
        racers.0, racers.1, racers.2, racers.3, racers.4, racers.5, racers.6, racers.7,
    ];
    let mut concurrent: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for (label, answer) in &racers {
        let case = format!("concurrent CreateTask {label} in {region}");
        assert_gated(answer, &case);
        concurrent.insert((
            answer.status,
            answer.error_code().to_string(),
            answer.message().to_string(),
        ));
    }
    assert_eq!(
        concurrent.len(),
        1,
        "case \"eight at once\": every concurrent call answers the identical status, code \
         and message, got {concurrent:?}"
    );
    assert_eq!(
        concurrent, seen,
        "case \"concurrent matches repeated\": the refusal does not depend on how many \
         callers asked at once"
    );

    // None of those refused calls created anything: the Region that DOES meet the
    // precondition still holds no task.
    let tasks = call(&router, STOCKED_REGION, "GET", "/tasks", None).await;
    assert_eq!(
        tasks.body["tasks"],
        json!([]),
        "case \"nothing was created\": a refused CreateTask mints no task, got {}",
        tasks.body
    );
}

#[tokio::test]
async fn the_tagging_operations_are_not_gated_and_still_resolve_their_resource() {
    // The model declares no `AccessDeniedException` on `TagResource`,
    // `UntagResource` or `ListTagsForResource`, so the precondition does not apply to
    // them: in a device-free Region they still reach their own existence check.
    let (router, _, activated) = emulator();
    let device = activated
        .first()
        .expect("the activation seam put devices in the stocked Region")
        .clone();
    let region = GATED_REGIONS[2];

    let unknown: Vec<(&str, String)> = vec![
        (
            "a managed-device ARN naming no device",
            format!("arn:aws:{ARN_SERVICE}:{region}:{ACCOUNT}:managed-device/smd-ffffffffffffffff"),
        ),
        (
            "a task ARN naming no task",
            format!("arn:aws:{ARN_SERVICE}:{region}:{ACCOUNT}:task/st-ffffffffffffffff"),
        ),
    ];
    for (case, resource_arn) in &unknown {
        let label = encode_label(resource_arn);
        let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("ListTagsForResource", "GET", format!("/tags/{label}"), None),
            (
                "TagResource",
                "POST",
                format!("/tags/{label}"),
                Some(json!({ "tags": { "owner": "fleet" } })),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("/tags/{label}?tagKeys=owner"),
                None,
            ),
        ];
        for (operation, method, uri, body) in rows {
            let case = format!("{operation} on {case} in {region}");
            let answer = call(&router, region, method, &uri, body).await;
            assert_ne!(
                answer.status, GATE_STATUS,
                "case {case:?}: the tagging operations are not gated, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.status, 404,
                "case {case:?}: an ARN of this account naming no record is not found, got \
                 {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {case:?}: expected the tagging family's NotFoundException, got {}",
                answer.body
            );
        }
    }

    // And a tagging call in that same device-free Region still SUCCEEDS against a
    // record this account really holds: the ARN carries everything the operation
    // addresses, so the signed Region does not gate it.
    let label = encode_label(&format!(
        "arn:aws:{ARN_SERVICE}:{STOCKED_REGION}:{ACCOUNT}:managed-device/{device}"
    ));
    let tagged = call(
        &router,
        region,
        "POST",
        &format!("/tags/{label}"),
        Some(json!({ "tags": { "owner": "fleet" } })),
    )
    .await;
    assert_eq!(
        tagged.status, 200,
        "case \"TagResource on a device this account holds\": expected HTTP 200, got {} \
         with {}",
        tagged.status, tagged.body
    );
    let read = call(&router, region, "GET", &format!("/tags/{label}"), None).await;
    assert_eq!(
        read.body["tags"],
        json!({ "owner": "fleet" }),
        "case \"ListTagsForResource reads the write back\": got {}",
        read.body
    );
}

#[tokio::test]
async fn activating_a_device_lifts_the_gate_for_exactly_that_region() {
    // The precondition is computed from persisted state, not fixed: the same Region
    // that refused every call serves them once it holds a device, and the Regions that
    // still hold none keep refusing.
    let (router, engine, _) = emulator();
    let region = GATED_REGIONS[0];
    let still_gated = GATED_REGIONS[1];

    let before = call(&router, region, "GET", "/managed-devices", None).await;
    assert_gated(&before, "ListDevices before activation");

    let activated = activate_in(&engine, region, 1);
    let device = activated
        .first()
        .expect("one device was activated")
        .clone();

    let listed = call(&router, region, "GET", "/managed-devices", None).await;
    assert_eq!(
        listed.status, 200,
        "case \"ListDevices after activation\": a Region that now holds a device is \
         served, got {} with {}",
        listed.status, listed.body
    );
    let ids: Vec<&str> = listed.body["devices"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case \"ListDevices after activation\": expected devices, got {}",
                listed.body
            )
        })
        .iter()
        .map(|entry| entry["managedDeviceId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec![device.as_str()],
        "case \"ListDevices after activation\": the page reports the activated device, \
         got {}",
        listed.body
    );

    // The round trip: the device the list named describes, and its tasks can be
    // created and read back — the whole surface is open now.
    let described = call(
        &router,
        region,
        "POST",
        &format!("/managed-device/{device}/describe"),
        None,
    )
    .await;
    assert_eq!(
        described.body["managedDeviceId"].as_str(),
        Some(device.as_str()),
        "case \"DescribeDevice after activation\": got {}",
        described.body
    );
    let created = call(
        &router,
        region,
        "POST",
        "/task",
        Some(json!({ "targets": [&device], "command": { "unlock": {} } })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateTask after activation\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    let task = created.body["taskId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"CreateTask after activation\": expected a taskId, got {}",
                created.body
            )
        })
        .to_string();
    let read_back = call(&router, region, "POST", &format!("/task/{task}"), None).await;
    assert_eq!(
        read_back.body["taskId"].as_str(),
        Some(task.as_str()),
        "case \"DescribeTask after activation\": the created task reads back, got {}",
        read_back.body
    );

    // An id naming nothing in the now-served Region is the modelled not-found, not the
    // precondition: the two refusals are distinct and the gate no longer applies here.
    let unknown = call(
        &router,
        region,
        "POST",
        "/managed-device/smd-ffffffffffffffff/describe",
        None,
    )
    .await;
    assert_eq!(
        unknown.status, 404,
        "case \"an unknown id in the now-served Region\": expected HTTP 404, got {} with \
         {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case \"an unknown id in the now-served Region\": expected \
         ResourceNotFoundException, got {}",
        unknown.body
    );

    // And a Region that still holds no device is still refused: the precondition is
    // per Region, not per account.
    for request in gated_requests("smd-ffffffffffffffff", "st-ffffffffffffffff") {
        let case = format!("{} in {still_gated} after the other Region opened", request.operation);
        let answer = call(
            &router,
            still_gated,
            request.method,
            &request.uri,
            request.body,
        )
        .await;
        assert_gated(&answer, &case);
    }
}
