// SPDX-License-Identifier: CC0-1.0

//! What this service answers on an emulator exactly as BOOTED — a shared engine on
//! which nothing has activated a device, which is the state a live router starts in —
//! and how the device inventory a caller reads becomes non-empty.
//!
//! The rules under test.
//!
//! 1. An account/Region holding no Snow Device Management device fails this service's
//!    account/region precondition, so every operation the model declares
//!    `AccessDeniedException` on refuses the call there: HTTP 403, wire error code
//!    `AccessDeniedException` (`errorKind` client, `@httpError 403` —
//!    `smithy_query(query="shape",shape="AccessDeniedException")`), and the ONE message
//!    this service words that class with, carrying no part of a success payload. A
//!    collection operation answers that refusal INSTEAD of a page, so `ListDevices` on
//!    such an account/Region has no 200 representation at all. The refusal is decided
//!    before an id is resolved and before a filter is validated, so no filter, page
//!    size or id shape a request can carry changes it.
//! 2. Nothing lifts that precondition on its own. Snow Device Management models no
//!    device-creation operation — a device becomes manageable by being ORDERED, which
//!    is another service's API — so a store nothing has activated a device on stays
//!    empty however long the emulator runs and however often it is asked, and a refused
//!    call writes nothing.
//! 3. The precondition and the inventory are both READ FROM STATE: once this account
//!    holds devices in the signed Region, the same operations serve it and
//!    `ListDevices` reports exactly those device records — the ids, ARNs, ordering job
//!    and tags each one carries — while a Region still holding none keeps answering the
//!    refusal.
//!
//! Rows differ along what the rules generalise over: the operation (all ten the model
//! declares the refusal on), the signed Region (three, plus a caller that signs for
//! none and is resolved to the Region this service defaults to), the shape of the ids
//! addressed, the filters and page sizes a request carries, how many times a caller
//! asks, and how many devices the account holds in the signed Region (none, one, two,
//! three).
//!
//! Where the refusal's status, code and wording come from: the recorded behaviour of
//! this service for an account/Region holding no device — the wire fixture
//! `snow-device-management-list-devices-and-tasks-baseline` (GET /managed-devices and
//! GET /tasks) and the behavioral checks `account-device-gate-blocks-all-reads`,
//! `list-devices-param-validation` and `service-gate-error-stable-not-flaky`, as the
//! resolved reports under `issues/things-that-now-work/` transcribe them. The model's
//! doc text for the error shape is generic prose about access ("You don't have
//! sufficient access to perform this action") and does not word this class; the
//! recording does, which is why the message below is pinned as the literal the wire
//! carries.
//!
//! Nothing here asserts a value the emulator invented: the inventory is put in state
//! through the one activation seam and every answer is read back off the wire.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The wire error code the precondition refuses with, and the status the model gives
/// it.
const GATE_CODE: &str = "AccessDeniedException";
const GATE_STATUS: u16 = 403;

/// The one message this service words that refusal with, spelled out here so a change
/// to the wording cannot pass unnoticed. It states device availability in the Region —
/// the fact the refusal is about — and nothing about a principal or a permission.
///
/// This is the wording for the whole class, not a value fitted to one request: every
/// gated operation, in every Region, for every request shape, answers this exact text,
/// which is what the rows below assert. It is the recorded wording of this refusal (see
/// the fixtures named in this suite's header), so it is pinned as a wire golden here,
/// where the sibling gate suites assert the message's RULE instead.
const GATE_MESSAGE: &str = "No devices found in this region.";

/// Regions a caller signs for. Three of them, so the answer is a property of what the
/// Region holds rather than of one Region name.
const BOOT_REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-northeast-2"];

/// The Region a request carrying no SigV4 credential scope is resolved to, which is the
/// one this service's store defaults ARNs and records to.
const DEFAULT_REGION: &str = "us-east-1";

/// The Region the ENDPOINT of an unsigned request names below. Deliberately not
/// [`DEFAULT_REGION`], so a Region resolved from the credential scope's absence is
/// distinguishable from one read off the host header.
const UNSCOPED_HOST_REGION: &str = "ap-southeast-1";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// This service's ARN namespace, as its `@aws.api#service` trait gives it.
const ARN_SERVICE: &str = "snow-device-management";

/// How long the repeated-sample rows let the emulator run between calls. Long enough
/// for any owed clock work to come due, since the point is that NOTHING comes due.
const SAMPLE_GAP: Duration = Duration::from_millis(60);

/// What the emulator answered: the status, the `x-amzn-errortype` header a restJson1
/// client branches on, and the JSON body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The wire error code the body carries (`__type`), or the empty string on a
    /// success. restJson1 renders `__type` as a shape id on some services, so the code
    /// is its last segment.
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

/// An emulator exactly as booted: one shared engine, this service mounted on it, and
/// nothing activated anywhere. Answers the router and the engine, so a later activation
/// reaches the same store the handlers read.
fn booted_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Activate Snow Device Management on `count` devices in `region` through the one seam
/// that puts a device in the inventory, answering the ids it minted.
fn activate_in(engine: &Arc<engine::InMemoryEngine>, region: &str, count: u64) -> Vec<String> {
    service_state(&**engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(region, count, SystemTime::now())
}

/// How many devices this account holds, over every Region, as the store holds them.
fn devices_held(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .devices
        .len()
}

/// Dispatch one request. `region` is the Region it is signed for; `None` signs for no
/// Region at all, which is the unscoped caller this service resolves to its default.
async fn call(
    router: &MultiServiceRouter,
    region: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let host = region.unwrap_or(UNSCOPED_HOST_REGION);
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("snow-device-management.{host}.amazonaws.com"));
    if let Some(region) = region {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/\
                 snow-device-management/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
    }
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

/// Every operation the model declares `AccessDeniedException` on
/// (`smithy_query(query="digest")`: ListDevices, ListTasks, DescribeDevice,
/// DescribeDeviceEc2Instances, ListDeviceResources, DescribeTask, CancelTask,
/// ListExecutions, DescribeExecution, CreateTask), addressed to `device` and `task`,
/// plus the filters and page sizes a collection request can carry.
fn gated_requests(device: &str, task: &str) -> Vec<Request> {
    vec![
        Request {
            operation: "ListDevices".to_string(),
            method: "GET",
            uri: "/managed-devices".to_string(),
            body: None,
        },
        Request {
            operation: "ListDevices (smallest page)".to_string(),
            method: "GET",
            uri: "/managed-devices?maxResults=1".to_string(),
            body: None,
        },
        Request {
            operation: "ListDevices (jobId filter naming no job)".to_string(),
            method: "GET",
            uri: "/managed-devices?jobId=JID00000000-0000-4000-8000-000000000000".to_string(),
            body: None,
        },
        Request {
            operation: "ListTasks".to_string(),
            method: "GET",
            uri: "/tasks?maxResults=100".to_string(),
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

/// Device and task ids in the shapes a client can send: the modelled
/// `@length(min: 1, max: 64)` floor and ceiling (`ManagedDeviceId` and `TaskId` are
/// both bounded that way — `smithy_query(query="shape",shape="ManagedDeviceId,TaskId")`)
/// and the shapes this store mints.
fn id_pairs() -> Vec<(String, String)> {
    vec![
        ("q".to_string(), "r".to_string()),
        ("c".repeat(64), "d".repeat(64)),
        (
            "smd-00000000000000ff".to_string(),
            "st-00000000000000ff".to_string(),
        ),
    ]
}

/// The members a success of any gated operation would carry: a refusal carries none of
/// them, so a caller never reads a page (or a created task) beside the 403.
const SUCCESS_MEMBERS: &[&str] = &[
    "devices",
    "tasks",
    "executions",
    "resources",
    "instances",
    "taskId",
    "managedDeviceId",
    "nextToken",
];

/// Assert `answer` is this service's account/region precondition, exactly as a
/// restJson1 client reads it.
fn assert_precondition(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, GATE_STATUS,
        "case {case:?}: expected HTTP {GATE_STATUS}, got {answer:?}"
    );
    assert_eq!(
        answer.error_type, GATE_CODE,
        "case {case:?}: expected the x-amzn-errortype header to name {GATE_CODE}, got {answer:?}"
    );
    assert_eq!(
        answer.error_code(),
        GATE_CODE,
        "case {case:?}: the body's __type must carry the same code as the header, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        GATE_MESSAGE,
        "case {case:?}: expected the message {GATE_MESSAGE:?}, got {:?}",
        answer.message()
    );
    for member in SUCCESS_MEMBERS {
        assert!(
            answer.body[*member].is_null(),
            "case {case:?}: a refusal carries no part of a success payload, found {member:?} in \
             {answer:?}"
        );
    }
}

/// The ARN a managed device of this account carries in `region`.
fn expected_device_arn(region: &str, managed_device_id: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}:{region}:{ACCOUNT}:managed-device/{managed_device_id}")
}

#[tokio::test]
async fn a_booted_emulator_answers_the_precondition_on_every_gated_operation() {
    // Nothing has activated a device on this engine, which is the state a live router
    // starts in: every operation the model declares the refusal on answers it, in every
    // Region, over every id shape, filter and page size a request can carry — and the
    // collection operations answer it instead of a page.
    let (router, engine) = booted_emulator();

    let mut answers: BTreeSet<(u16, String, String, String)> = BTreeSet::new();
    for region in BOOT_REGIONS {
        for (device, task) in id_pairs() {
            for request in gated_requests(&device, &task) {
                let case = format!("{} in {region}", request.operation);
                let answer = call(
                    &router,
                    Some(region),
                    request.method,
                    &request.uri,
                    request.body,
                )
                .await;
                assert_precondition(&answer, &case);
                answers.insert((
                    answer.status,
                    answer.error_type.clone(),
                    answer.error_code().to_string(),
                    answer.message().to_string(),
                ));
            }
        }
    }

    // A caller that signs for no Region at all is resolved to a Region of this account
    // too, and that Region holds no device either. Which Region the resolution picks is
    // pinned on its own below.
    for request in gated_requests("smd-00000000000000ff", "st-00000000000000ff") {
        let case = format!("{} signed for no Region", request.operation);
        let answer = call(&router, None, request.method, &request.uri, request.body).await;
        assert_precondition(&answer, &case);
        answers.insert((
            answer.status,
            answer.error_type.clone(),
            answer.error_code().to_string(),
            answer.message().to_string(),
        ));
    }

    assert_eq!(
        answers,
        BTreeSet::from([(
            GATE_STATUS,
            GATE_CODE.to_string(),
            GATE_CODE.to_string(),
            GATE_MESSAGE.to_string(),
        )]),
        "case \"one refusal\": every gated operation answers an account/Region holding no \
         device the identical status, header, code and message, got {answers:?}"
    );

    // And none of those calls wrote anything: a refused read activates no device and a
    // refused CreateTask mints no task.
    assert_eq!(
        devices_held(&engine),
        0,
        "case \"a refused call writes nothing\": the store still holds no device, got {}",
        devices_held(&engine)
    );
}

#[tokio::test]
async fn no_device_appears_while_the_emulator_runs() {
    // The precondition is a fact about state, and no clock work is owed on an account
    // that has ordered nothing: repeated samples, spread over time, answer the identical
    // refusal and the inventory stays empty — nothing provisions a device in the
    // background, because the API models no way for one to be created from a request.
    let (router, engine) = booted_emulator();
    let region = BOOT_REGIONS[2];

    let mut samples: BTreeSet<(u16, String, String)> = BTreeSet::new();
    for attempt in 0..4 {
        for request in [
            ("ListDevices", "GET", "/managed-devices".to_string(), None),
            (
                "ListDevices (smallest page)",
                "GET",
                "/managed-devices?maxResults=1".to_string(),
                None,
            ),
            (
                "CreateTask",
                "POST",
                "/task".to_string(),
                Some(json!({ "targets": ["smd-00000000000000ff"], "command": { "reboot": {} } })),
            ),
        ] {
            let (operation, method, uri, body) = request;
            let case = format!("{operation} sample {attempt} in {region}");
            let answer = call(&router, Some(region), method, &uri, body).await;
            assert_precondition(&answer, &case);
            samples.insert((
                answer.status,
                answer.error_code().to_string(),
                answer.message().to_string(),
            ));
        }
        assert_eq!(
            devices_held(&engine),
            0,
            "case \"sample {attempt}\": the inventory is still empty, got {}",
            devices_held(&engine)
        );
        tokio::time::sleep(SAMPLE_GAP).await;
    }

    assert_eq!(
        samples,
        BTreeSet::from([(
            GATE_STATUS,
            GATE_CODE.to_string(),
            GATE_MESSAGE.to_string()
        )]),
        "case \"repeated samples\": every sample answers the identical refusal, got {samples:?}"
    );
}

#[tokio::test]
async fn a_caller_that_signs_for_no_region_is_resolved_to_the_default_region() {
    // Which Region an unsigned caller is answered from is decided by the credential
    // scope's ABSENCE, not by the endpoint it addressed: the account holds its device in
    // the default Region only, and the unsigned request — whose host names a different
    // Region — is served that inventory, while the same request signed for the Region
    // the host names answers the precondition.
    let (router, engine) = booted_emulator();
    assert_ne!(
        UNSCOPED_HOST_REGION, DEFAULT_REGION,
        "case \"the row is meaningful\": the endpoint must name a Region other than the \
         default one, got {UNSCOPED_HOST_REGION}"
    );
    let activated = activate_in(&engine, DEFAULT_REGION, 1);

    let unsigned = call(&router, None, "GET", "/managed-devices", None).await;
    assert_eq!(
        unsigned.status, 200,
        "case \"unsigned ListDevices\": the default Region holds a device, got {unsigned:?}"
    );
    let reported: Vec<String> = unsigned.body["devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"unsigned ListDevices\": expected devices, got {unsigned:?}"))
        .iter()
        .map(|summary| {
            summary["managedDeviceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        reported, activated,
        "case \"unsigned ListDevices\": the answer comes from the default Region's inventory, \
         not from the Region the endpoint names, got {unsigned:?}"
    );
    assert_eq!(
        unsigned.body["devices"][0]["managedDeviceArn"].as_str(),
        Some(expected_device_arn(DEFAULT_REGION, &activated[0]).as_str()),
        "case \"unsigned ListDevices\": the device ARN names the resolved Region, got \
         {unsigned:?}"
    );

    let signed = call(
        &router,
        Some(UNSCOPED_HOST_REGION),
        "GET",
        "/managed-devices",
        None,
    )
    .await;
    assert_precondition(
        &signed,
        "ListDevices signed for the Region the unsigned endpoint named",
    );
}

#[tokio::test]
async fn the_devices_the_account_holds_are_what_list_devices_reports() {
    // The same handler that refuses an account/Region holding no device serves one that
    // holds devices, and reports exactly the records state holds: rows differ in the
    // Region and in how many devices it holds.
    let rows: &[(&str, u64)] = &[("us-west-2", 1), ("eu-central-1", 2), ("ap-southeast-2", 3)];

    for (region, count) in rows {
        let case = format!("{count} devices in {region}");
        let (router, engine) = booted_emulator();

        // Before the account holds anything there, the refusal; one activation later the
        // same request is served from state.
        let before = call(&router, Some(region), "GET", "/managed-devices", None).await;
        assert_precondition(&before, &format!("{case}: before the activation"));

        let mut activated = activate_in(&engine, region, *count);
        activated.sort();

        let listed = call(&router, Some(region), "GET", "/managed-devices", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: an account/Region holding devices is served, got {listed:?}"
        );
        assert!(
            listed.error_type.is_empty(),
            "case {case:?}: a served answer sets no x-amzn-errortype, got {listed:?}"
        );
        let page = listed.body["devices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a devices list, got {listed:?}"))
            .clone();
        let reported: Vec<String> = page
            .iter()
            .map(|summary| {
                summary["managedDeviceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, activated,
            "case {case:?}: the page reports every device the Region holds, once, in id order, \
             got {listed:?}"
        );

        // Every summary is the device record read back: the ARN this account's device
        // carries in the signed Region, and the ordering job and tags DescribeDevice
        // reports for the same id.
        for summary in &page {
            let id = summary["managedDeviceId"].as_str().unwrap_or_default();
            let row = format!("{case}: device {id}");
            assert_eq!(
                summary["managedDeviceArn"].as_str(),
                Some(expected_device_arn(region, id).as_str()),
                "case {row:?}: the summary carries this account's device ARN in the signed \
                 Region, got {summary}"
            );
            let described = call(
                &router,
                Some(region),
                "POST",
                &format!("/managed-device/{id}/describe"),
                None,
            )
            .await;
            assert_eq!(
                described.status, 200,
                "case {row:?}: the device the page named describes, got {described:?}"
            );
            for member in ["managedDeviceId", "managedDeviceArn", "associatedWithJob"] {
                assert_eq!(
                    summary[member], described.body[member],
                    "case {row:?}: ListDevices and DescribeDevice project the same stored \
                     {member}, got {} and {}",
                    summary[member], described.body[member]
                );
            }
            assert_eq!(
                summary["tags"],
                json!({}),
                "case {row:?}: a device nothing has tagged carries no tag, got {summary}"
            );
        }

        // The `jobId` filter selects by the stored ordering job, so each job the page
        // names selects exactly the devices carrying it — one job can ship more than one
        // device.
        let jobs: BTreeSet<String> = page
            .iter()
            .map(|summary| {
                summary["associatedWithJob"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        for job in &jobs {
            let row = format!("{case}: jobId={job}");
            let expected: Vec<String> = page
                .iter()
                .filter(|summary| summary["associatedWithJob"].as_str() == Some(job.as_str()))
                .map(|summary| {
                    summary["managedDeviceId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let filtered = call(
                &router,
                Some(region),
                "GET",
                &format!("/managed-devices?jobId={job}"),
                None,
            )
            .await;
            let got: Vec<String> = filtered.body["devices"]
                .as_array()
                .unwrap_or_else(|| panic!("case {row:?}: expected a devices list, got {filtered:?}"))
                .iter()
                .map(|summary| {
                    summary["managedDeviceId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                got, expected,
                "case {row:?}: the filter selects the devices that stored job shipped, got \
                 {filtered:?}"
            );
        }

        // A job no device of this Region was ordered by selects none of them — and that
        // IS served, because the Region holds an inventory: an empty page is what an
        // account/Region holding devices answers a filter matching none of them, which
        // is a different outcome from the precondition above.
        let unmatched = call(
            &router,
            Some(region),
            "GET",
            "/managed-devices?jobId=JID99999999-0000-4000-8000-999999999999",
            None,
        )
        .await;
        assert_eq!(
            unmatched.status, 200,
            "case {case:?}: a filter matching no device is served, got {unmatched:?}"
        );
        assert_eq!(
            unmatched.body["devices"],
            json!([]),
            "case {case:?}: a filter matching no device selects none, got {unmatched:?}"
        );

        // And the Regions of the same account that still hold no device keep answering
        // the precondition: it is per Region, not per account.
        for other in BOOT_REGIONS {
            let answer = call(&router, Some(other), "GET", "/managed-devices", None).await;
            assert_precondition(&answer, &format!("{case}: ListDevices in {other}"));
        }
    }
}
