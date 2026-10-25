// SPDX-License-Identifier: CC0-1.0

//! The Snow Device Management device-inventory and task lifecycle, end to end over HTTP.
//!
//! Every case here goes through the router, so it exercises routing, deserialization,
//! the handler, the persisted store and serialization — the same path a client takes.
//! Nothing reaches into the state struct, so an answer that is not computed from
//! persisted state cannot pass.
//!
//! The rule under test is the one this control plane states: every operation's answer
//! is a projection of the stored device / task / execution records, and a create or a
//! cancel is visible to every later report of those records. Each case is a TABLE whose
//! rows differ along what the rule generalises over — the device, the job, the page
//! size, the command, the target count, the state filter — never a single recorded
//! request.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    service_state, EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. Deliberately NOT the emulator's
/// default, so an ARN carrying it proves the region came from the SigV4 scope.
pub(crate) const SIGNED_REGION: &str = "ap-southeast-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/ap-southeast-2/snow-device-management/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `UnlockState`'s values, from `smithy_query(query="shape",shape="UnlockState")`.
const UNLOCK_STATES: &[&str] = &["UNLOCKED", "LOCKED", "UNLOCKING"];

/// `TaskState`'s values, from `smithy_query(query="shape",shape="TaskState")`.
const TASK_STATES: &[&str] = &["IN_PROGRESS", "CANCELED", "COMPLETED"];

/// `ExecutionState`'s values, from `smithy_query(query="shape",shape="ExecutionState")`.
const EXECUTION_STATES: &[&str] = &[
    "QUEUED",
    "IN_PROGRESS",
    "CANCELED",
    "FAILED",
    "SUCCEEDED",
    "REJECTED",
    "TIMED_OUT",
];

/// `PhysicalConnectorType`'s values, from
/// `smithy_query(query="shape",shape="PhysicalConnectorType")`.
const PHYSICAL_CONNECTOR_TYPES: &[&str] = &["QSFP", "RJ45", "RJ45_2", "SFP_PLUS", "WIFI"];

/// `IpAddressAssignment`'s values, from
/// `smithy_query(query="shape",shape="IpAddressAssignment")`.
const IP_ADDRESS_ASSIGNMENTS: &[&str] = &["DHCP", "STATIC"];

/// The bounds `maxResults` is modelled with on `ListDevices` and `ListExecutions`
/// (`@range(min: 1, max: 100)`), spelled out here rather than imported so a wrong bound
/// in the implementation cannot make this test pass.
const MAX_RESULTS_FLOOR: i64 = 1;
const MAX_RESULTS_CEILING: i64 = 100;

/// How many devices this suite activates Snow Device Management on. Two jobs' worth,
/// so a `jobId` filter has both a multi-device and a single-device answer to compute.
const ACTIVATED_DEVICES: u64 = 3;

/// One emulator: a router carrying only this service, over one shared engine, with
/// Snow Device Management activated on [`ACTIVATED_DEVICES`] ordered devices in
/// [`SIGNED_REGION`].
///
/// The API models NO device-creation operation — a device becomes manageable by being
/// ORDERED, which is another service's API — so the activation goes through the one
/// state seam that puts a device in the inventory. Without it this account's inventory
/// is empty, which is what `empty_inventory_refusals.rs` pins.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    service_state(&*engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(SIGNED_REGION, ACTIVATED_DEVICES, SystemTime::now());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "snow-device-management.ap-southeast-2.amazonaws.com")
        .header("authorization", SIGNED_SCOPE);
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

async fn list_devices(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/managed-devices{query}"), None).await
}

pub(crate) async fn describe_device(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/managed-device/{id}/describe"),
        None,
    )
    .await
}

pub(crate) async fn create_task(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/task", Some(request)).await
}

pub(crate) async fn describe_task(router: &MultiServiceRouter, task_id: &str) -> Answer {
    call(router, "POST", &format!("/task/{task_id}"), None).await
}

/// The `/tags/{resourceArn}` URI for one resource ARN.
///
/// `resourceArn` is bound as an `@httpLabel`, and a non-greedy label is ONE path
/// segment, so the `/` inside an ARN's resource field is percent-encoded or it would
/// split the segment.
pub(crate) fn tags_uri(resource_arn: &str) -> String {
    format!("/tags/{}", resource_arn.replace('/', "%2F"))
}

/// `ListTagsForResource` for one resource ARN.
pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &tags_uri(resource_arn), None).await
}

/// `TagResource` writing `tags` onto one resource ARN.
pub(crate) async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: Value,
) -> Answer {
    call(
        router,
        "POST",
        &tags_uri(resource_arn),
        Some(json!({ "tags": tags })),
    )
    .await
}

pub(crate) async fn cancel_task(router: &MultiServiceRouter, task_id: &str) -> Answer {
    call(router, "POST", &format!("/task/{task_id}/cancel"), None).await
}

pub(crate) async fn list_executions(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/executions{query}"), None).await
}

pub(crate) async fn describe_execution(
    router: &MultiServiceRouter,
    task_id: &str,
    managed_device_id: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/task/{task_id}/execution/{managed_device_id}"),
        None,
    )
    .await
}

/// Assert a call succeeded and hand back its body.
pub(crate) fn ok(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body.clone()
}

/// Assert a call was refused with HTTP 404 `ResourceNotFoundException`.
pub(crate) fn assert_not_found(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {} with {}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().contains("ResourceNotFoundException"),
        "case {case:?}: expected a ResourceNotFoundException, got {}",
        answer.body
    );
}

/// Assert a call was refused with HTTP 400 `ValidationException`.
pub(crate) fn assert_validation(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {}",
        answer.body
    );
}

/// A required string member of a JSON object.
pub(crate) fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The `devices` array of a `ListDevices` answer.
fn devices_of(body: &Value, case: &str) -> Vec<Value> {
    body["devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a devices array, got {body}"))
        .clone()
}

/// The `executions` array of a `ListExecutions` answer.
pub(crate) fn executions_of(body: &Value, case: &str) -> Vec<Value> {
    body["executions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an executions array, got {body}"))
        .clone()
}

/// Every activated device, in the order `ListDevices` serves them.
pub(crate) async fn whole_inventory(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let answer = list_devices(router, "").await;
    let body = ok(&answer, case);
    let devices = devices_of(&body, case);
    assert!(
        devices.len() >= 2,
        "case {case:?}: this suite activates a device inventory, got {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: a page holding the whole inventory must carry no nextToken, got {body}"
    );
    devices
}

/// The `managedDeviceId`s of a device list, in order.
pub(crate) fn ids_of(devices: &[Value], case: &str) -> Vec<String> {
    devices
        .iter()
        .map(|device| string(device, "managedDeviceId", case))
        .collect()
}

/// A `NextToken` must satisfy the modelled `@length(1..1024)` and
/// `@pattern("[a-zA-Z0-9+/=]*")`, or no client can send it back.
fn assert_modelled_token(token: &str, case: &str) {
    assert!(
        (1..=1024).contains(&token.len())
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')),
        "case {case:?}: nextToken {token:?} is outside NextToken's length 1..1024 / \
         pattern [a-zA-Z0-9+/=]*"
    );
}

#[tokio::test]
async fn list_devices_reports_the_activated_inventory_with_stable_region_scoped_arns() {
    let router = emulator();
    let first = whole_inventory(&router, "first ListDevices").await;

    let mut seen = BTreeSet::new();
    for device in &first {
        let case = format!("device {device}");
        let id = string(device, "managedDeviceId", &case);
        let arn = string(device, "managedDeviceArn", &case);
        // Every emulator ARN is `arn:aws:<namespace>:<region>:000000000000:<resource>`,
        // and this service's ARN namespace is `snow-device-management`.
        let expected =
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:managed-device/{id}");
        assert_eq!(
            arn, expected,
            "case {case:?}: expected the device ARN to name the signed region and the \
             emulator account, got {arn:?}"
        );
        assert!(
            (1..=64).contains(&id.len()),
            "case {case:?}: managedDeviceId {id:?} is outside the modelled length 1..64"
        );
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: managedDeviceId {id:?} appears twice in one page"
        );
        assert!(
            !string(device, "associatedWithJob", &case).is_empty(),
            "case {case:?}: every device reports the job it was ordered by"
        );
    }

    let ids = ids_of(&first, "first ListDevices");
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "case \"ordering\": ListDevices must serve a stable total order its pagination \
         token can index into, got {ids:?}"
    );

    // A read must not change what a later caller sees.
    let second = whole_inventory(&router, "second ListDevices").await;
    assert_eq!(
        first, second,
        "case \"repeat\": ListDevices is a report, so two calls must answer identically"
    );
}

#[tokio::test]
async fn list_devices_narrows_the_inventory_to_one_job_id() {
    let router = emulator();
    let inventory = whole_inventory(&router, "inventory").await;

    // The table rows are the jobs the inventory itself reports, plus a job id no
    // device carries — built from the answer rather than written down, so the case
    // set spans whatever inventory the emulator seeds.
    let mut by_job: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for device in &inventory {
        let case = format!("device {device}");
        by_job
            .entry(string(device, "associatedWithJob", &case))
            .or_default()
            .push(string(device, "managedDeviceId", &case));
    }
    assert!(
        by_job.len() >= 2,
        "case \"fixture\": the activated inventory must span at least two jobs for a \
         jobId filter to be observable, got {by_job:?}"
    );
    by_job.insert("JID00000000-0000-4000-8000-ffffffffffff".to_string(), vec![]);

    for (job_id, expected) in &by_job {
        let case = format!("jobId={job_id}");
        let answer = list_devices(&router, &format!("?jobId={job_id}")).await;
        let body = ok(&answer, &case);
        let actual = ids_of(&devices_of(&body, &case), &case);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected exactly the devices that job ordered, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn list_devices_pages_at_every_page_size_and_resumes_where_it_stopped() {
    let router = emulator();
    let all = ids_of(&whole_inventory(&router, "inventory").await, "inventory");

    // One row per page size from a single device up to the modelled ceiling: the
    // concatenation of the pages must be the whole list, once, in order.
    let mut sizes: Vec<i64> = (MAX_RESULTS_FLOOR..=all.len() as i64).collect();
    sizes.push(MAX_RESULTS_CEILING);
    for size in sizes {
        let case = format!("maxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match token.as_deref() {
                None => format!("?maxResults={size}"),
                Some(token) => format!("?maxResults={size}&nextToken={token}"),
            };
            let answer = list_devices(&router, &query).await;
            let body = ok(&answer, &case);
            let page = ids_of(&devices_of(&body, &case), &case);
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page must hold at most maxResults devices, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging did not terminate; walked {walked:?}"
            );
            match body["nextToken"].as_str() {
                None => break,
                Some(next) => {
                    assert_modelled_token(next, &case);
                    token = Some(next.to_string());
                }
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must concatenate to the whole inventory, in order"
        );
    }
}

#[tokio::test]
async fn list_devices_honours_the_modelled_max_results_range() {
    let router = emulator();
    // The published bounds, and one past each: `@range(min: 1, max: 100)`.
    let rows: &[(i64, bool)] = &[
        (MAX_RESULTS_FLOOR - 1, false),
        (MAX_RESULTS_FLOOR, true),
        (MAX_RESULTS_CEILING, true),
        (MAX_RESULTS_CEILING + 1, false),
    ];
    for (max_results, admitted) in rows {
        let case = format!("maxResults={max_results}");
        let answer = list_devices(&router, &format!("?maxResults={max_results}")).await;
        if *admitted {
            let body = ok(&answer, &case);
            assert!(
                !devices_of(&body, &case).is_empty(),
                "case {case:?}: a page inside the modelled range must serve devices"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected a value outside @range(1..100) to be refused, \
                 got {} with {}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_list_operations_refuse_a_next_token_they_never_issued() {
    let router = emulator();
    let task_id = queue_task(&router, &["unlock"], 1, None).await.0;

    // Each row satisfies `NextToken`'s own pattern, so it reaches the handler, and
    // none of them is a token this service ever handed out.
    let rows: &[&str] = &["zz", "abc", "ffff", "ZZZZZZZZ", "===="];
    for token in rows {
        let case = format!("ListDevices nextToken={token}");
        assert_validation(
            &list_devices(&router, &format!("?nextToken={token}")).await,
            &case,
        );
        let case = format!("ListExecutions nextToken={token}");
        assert_validation(
            &list_executions(&router, &format!("?taskId={task_id}&nextToken={token}")).await,
            &case,
        );
    }
}

#[tokio::test]
async fn describe_device_reports_the_stored_record_for_every_activated_device() {
    let router = emulator();
    let inventory = whole_inventory(&router, "inventory").await;

    for summary in &inventory {
        let id = string(summary, "managedDeviceId", "inventory");
        let case = format!("managedDeviceId={id}");
        let body = ok(&describe_device(&router, &id).await, &case);

        // The identity members must be the ones ListDevices returned — one record,
        // two projections of it.
        for member in ["managedDeviceId", "managedDeviceArn", "associatedWithJob"] {
            assert_eq!(
                body[member], summary[member],
                "case {case:?}: DescribeDevice and ListDevices must project the same \
                 stored {member}, got {} and {}",
                body[member], summary[member]
            );
        }
        assert_eq!(
            body["tags"], summary["tags"],
            "case {case:?}: both projections report the same stored tags, got {} and {}",
            body["tags"], summary["tags"]
        );

        let device_state = string(&body, "deviceState", &case);
        assert!(
            UNLOCK_STATES.contains(&device_state.as_str()),
            "case {case:?}: deviceState {device_state:?} is not one of UnlockState's \
             values {UNLOCK_STATES:?}"
        );
        assert!(
            !string(&body, "deviceType", &case).is_empty(),
            "case {case:?}: every device reports the Snow Family device type it is"
        );
        assert!(
            !string(&body["software"], "installedVersion", &case).is_empty(),
            "case {case:?}: software must report the version installed on the device, \
             got {}",
            body["software"]
        );

        let capacities = body["deviceCapacities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected deviceCapacities, got {body}"));
        assert!(
            (1..=100).contains(&capacities.len()),
            "case {case:?}: deviceCapacities must be a non-empty list inside the \
             modelled length 0..100, got {capacities:?}"
        );
        for capacity in capacities {
            let total = capacity["total"].as_i64().unwrap_or(-1);
            let used = capacity["used"].as_i64().unwrap_or(-1);
            let available = capacity["available"].as_i64().unwrap_or(-1);
            assert!(
                total >= 0 && used >= 0 && available >= 0 && used + available == total,
                "case {case:?}: a capacity's used and available must account for its \
                 total, got {capacity}"
            );
        }

        let interfaces = body["physicalNetworkInterfaces"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: expected physicalNetworkInterfaces, got {body}")
        });
        assert!(
            !interfaces.is_empty(),
            "case {case:?}: a managed device reports at least one physical interface"
        );
        for interface in interfaces {
            let connector = string(interface, "physicalConnectorType", &case);
            assert!(
                PHYSICAL_CONNECTOR_TYPES.contains(&connector.as_str()),
                "case {case:?}: physicalConnectorType {connector:?} is not one of \
                 {PHYSICAL_CONNECTOR_TYPES:?}"
            );
            let assignment = string(interface, "ipAddressAssignment", &case);
            assert!(
                IP_ADDRESS_ASSIGNMENTS.contains(&assignment.as_str()),
                "case {case:?}: ipAddressAssignment {assignment:?} is not one of \
                 {IP_ADDRESS_ASSIGNMENTS:?}"
            );
        }

        for member in ["lastReachedOutAt", "lastUpdatedAt"] {
            let instant = body[member].as_f64().unwrap_or(-1.0);
            assert!(
                instant > 0.0,
                "case {case:?}: {member} must be the stored instant, got {}",
                body[member]
            );
        }
    }
}

#[tokio::test]
async fn describe_device_404s_an_id_that_names_no_device() {
    let router = emulator();
    let known = ids_of(&whole_inventory(&router, "inventory").await, "inventory");

    // Rows differ in form: a plain word, a well-shaped but unused id, the wrong case
    // of a real id, and an id belonging to another resource kind.
    let rows: Vec<String> = vec![
        "does-not-exist".to_string(),
        "smd-ffffffffffffffff".to_string(),
        known[0].to_uppercase(),
        "st-0000000000000001".to_string(),
    ];
    for id in rows {
        if known.contains(&id) {
            continue;
        }
        let case = format!("managedDeviceId={id}");
        assert_not_found(&describe_device(&router, &id).await, &case);
    }
}

/// Create a task over the first `target_count` devices with `command`, returning its
/// id, ARN and the targets it was given (in the order they were submitted).
pub(crate) async fn queue_task(
    router: &MultiServiceRouter,
    commands: &[&str],
    target_count: usize,
    description: Option<&str>,
) -> (String, String, Vec<String>) {
    let case = format!("{commands:?} over {target_count} targets");
    let inventory = whole_inventory(router, &case).await;
    // Submitted in reverse inventory order, so an answer that re-derives the targets
    // from the device map instead of reading the stored record is visible.
    let targets: Vec<String> = ids_of(&inventory, &case)
        .into_iter()
        .rev()
        .take(target_count)
        .collect();
    let mut request = json!({
        "targets": targets,
        "command": { commands[0]: {} },
    });
    if let Some(description) = description {
        request["description"] = json!(description);
    }
    let body = ok(&create_task(router, request).await, &case);
    let task_id = string(&body, "taskId", &case);
    let task_arn = string(&body, "taskArn", &case);
    (task_id, task_arn, targets)
}

#[tokio::test]
async fn create_task_persists_what_describe_task_and_list_executions_report() {
    // Rows differ in command, in how many devices the task targets, in whether a
    // description was given and in whether tags were.
    struct Row {
        command: &'static str,
        target_count: usize,
        description: Option<&'static str>,
        tags: Option<Value>,
    }
    let rows = [
        Row {
            command: "unlock",
            target_count: 1,
            description: Some("bootstrap test"),
            tags: None,
        },
        Row {
            command: "reboot",
            target_count: 2,
            description: None,
            tags: Some(json!({ "owner": "ops", "env": "test" })),
        },
        Row {
            command: "unlock",
            target_count: 3,
            description: Some("Reboot every device in job 1."),
            tags: Some(json!({ "purpose": "bootstrap" })),
        },
    ];

    for row in &rows {
        let router = emulator();
        let case = format!(
            "{} over {} targets, description {:?}",
            row.command, row.target_count, row.description
        );
        let inventory = whole_inventory(&router, &case).await;
        let targets: Vec<String> = ids_of(&inventory, &case)
            .into_iter()
            .rev()
            .take(row.target_count)
            .collect();
        assert_eq!(
            targets.len(),
            row.target_count,
            "case {case:?}: the activated inventory must hold enough devices for this row"
        );

        let mut request = json!({ "targets": targets, "command": { row.command: {} } });
        if let Some(description) = row.description {
            request["description"] = json!(description);
        }
        if let Some(tags) = &row.tags {
            request["tags"] = tags.clone();
        }
        let created = ok(&create_task(&router, request).await, &case);
        let task_id = string(&created, "taskId", &case);
        let task_arn = string(&created, "taskArn", &case);
        assert!(
            (1..=64).contains(&task_id.len()),
            "case {case:?}: taskId {task_id:?} is outside the modelled length 1..64"
        );
        assert_eq!(
            task_arn,
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:task/{task_id}"),
            "case {case:?}: expected the task ARN to name the signed region, the \
             emulator account and the minted task id"
        );

        let described = ok(&describe_task(&router, &task_id).await, &case);
        assert_eq!(
            described["taskId"].as_str(),
            Some(task_id.as_str()),
            "case {case:?}: DescribeTask must report the id CreateTask minted, got {described}"
        );
        assert_eq!(
            described["taskArn"].as_str(),
            Some(task_arn.as_str()),
            "case {case:?}: DescribeTask must report the ARN CreateTask minted, got {described}"
        );
        assert_eq!(
            described["targets"],
            json!(targets),
            "case {case:?}: DescribeTask must echo the stored targets, in the order they \
             were submitted, got {}",
            described["targets"]
        );
        assert_eq!(
            described["state"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: a task is running from the instant it is created, got {described}"
        );
        assert_eq!(
            described["description"].as_str(),
            row.description,
            "case {case:?}: DescribeTask must report the stored description, got {}",
            described["description"]
        );
        assert_eq!(
            described["tags"],
            row.tags.clone().unwrap_or_else(|| json!({})),
            "case {case:?}: DescribeTask must report the stored tags, got {}",
            described["tags"]
        );
        let created_at = described["createdAt"].as_f64().unwrap_or(-1.0);
        assert!(
            created_at > 0.0,
            "case {case:?}: createdAt must be the instant the task was created, got {described}"
        );
        assert!(
            described["completedAt"].is_null(),
            "case {case:?}: a running task has not completed, got {described}"
        );
        let task_state = string(&described, "state", &case);
        assert!(
            TASK_STATES.contains(&task_state.as_str()),
            "case {case:?}: state {task_state:?} is not one of TaskState's values \
             {TASK_STATES:?}"
        );

        // One execution per target device, each addressable by the pair the model
        // keys an execution on.
        let listed = ok(
            &list_executions(&router, &format!("?taskId={task_id}")).await,
            &case,
        );
        let executions = executions_of(&listed, &case);
        assert_eq!(
            executions.len(),
            targets.len(),
            "case {case:?}: CreateTask must persist one execution per target, got {listed}"
        );
        let mut execution_ids = BTreeSet::new();
        let mut covered = BTreeSet::new();
        for execution in &executions {
            assert_eq!(
                execution["taskId"].as_str(),
                Some(task_id.as_str()),
                "case {case:?}: every execution belongs to the task that created it, \
                 got {execution}"
            );
            let execution_id = string(execution, "executionId", &case);
            assert!(
                (1..=64).contains(&execution_id.len()),
                "case {case:?}: executionId {execution_id:?} is outside the modelled \
                 length 1..64"
            );
            assert!(
                execution_ids.insert(execution_id.clone()),
                "case {case:?}: executionId {execution_id:?} was minted twice"
            );
            let device = string(execution, "managedDeviceId", &case);
            assert!(
                targets.contains(&device),
                "case {case:?}: an execution names a device the task did not target, \
                 got {execution}"
            );
            covered.insert(device);
            assert_eq!(
                execution["state"].as_str(),
                Some("QUEUED"),
                "case {case:?}: the command has not reached the device the instant the \
                 create returns, got {execution}"
            );
        }
        assert_eq!(
            covered.len(),
            targets.len(),
            "case {case:?}: every target must have its own execution, got {listed}"
        );
    }
}

#[tokio::test]
async fn create_task_replays_a_repeated_client_token_instead_of_minting_a_second_task() {
    let router = emulator();
    let inventory = whole_inventory(&router, "inventory").await;
    let target = ids_of(&inventory, "inventory")[0].clone();

    // Rows span the `IdempotencyToken` length bound 1..64 and both commands.
    let rows: &[(&str, &str)] = &[
        ("tok-1", "unlock"),
        ("a", "reboot"),
        (
            "0123456789012345678901234567890123456789012345678901234567890123",
            "unlock",
        ),
    ];
    let mut minted = BTreeSet::new();
    for (token, command) in rows {
        let case = format!("clientToken={token}, command={command}");
        let request = json!({
            "targets": [target.clone()],
            "command": { *command: {} },
            "clientToken": token,
        });
        let first = ok(&create_task(&router, request.clone()).await, &case);
        let task_id = string(&first, "taskId", &case);
        assert!(
            minted.insert(task_id.clone()),
            "case {case:?}: a fresh clientToken must mint a task of its own, got \
             {task_id:?} again"
        );

        let replay = ok(&create_task(&router, request).await, &case);
        assert_eq!(
            replay, first,
            "case {case:?}: a repeated clientToken must report the task it already \
             created, got {replay}"
        );

        // And no second task hides behind it: the replay created no extra execution.
        let listed = ok(
            &list_executions(&router, &format!("?taskId={task_id}")).await,
            &case,
        );
        assert_eq!(
            executions_of(&listed, &case).len(),
            1,
            "case {case:?}: the replay must not add an execution, got {listed}"
        );
    }
}

#[tokio::test]
async fn create_task_404s_a_target_that_names_no_device() {
    let router = emulator();
    let known = ids_of(&whole_inventory(&router, "inventory").await, "inventory");
    let unknown = "smd-ffffffffffffffff";

    // Rows differ in where the unknown target sits and how many there are.
    let rows: Vec<Vec<String>> = vec![
        vec![unknown.to_string()],
        vec![unknown.to_string(), known[0].clone()],
        vec![known[0].clone(), unknown.to_string()],
        vec![unknown.to_string(), "does-not-exist".to_string()],
    ];
    for targets in rows {
        let case = format!("targets={targets:?}");
        let answer = create_task(
            &router,
            json!({ "targets": targets, "command": { "unlock": {} } }),
        )
        .await;
        assert_not_found(&answer, &case);
    }

    // And nothing was persisted for the refused calls: a task over only known
    // devices still gets the first id the store mints.
    let created = ok(
        &create_task(
            &router,
            json!({ "targets": [known[0].clone()], "command": { "unlock": {} } }),
        )
        .await,
        "after the refusals",
    );
    let task_id = string(&created, "taskId", "after the refusals");
    let listed = ok(
        &list_executions(&router, &format!("?taskId={task_id}")).await,
        "after the refusals",
    );
    assert_eq!(
        executions_of(&listed, "after the refusals").len(),
        1,
        "case \"after the refusals\": a refused CreateTask must persist nothing, got {listed}"
    );
}

#[tokio::test]
async fn cancel_task_moves_the_task_and_its_pending_executions_to_canceled() {
    // Rows differ in command and in how many executions have to be cancelled.
    let rows: &[(&str, usize)] = &[("unlock", 1), ("reboot", 2), ("unlock", 3)];
    for (command, target_count) in rows {
        let router = emulator();
        let case = format!("{command} over {target_count} targets");
        let (task_id, _, targets) = queue_task(&router, &[command], *target_count, None).await;
        let before = ok(&describe_task(&router, &task_id).await, &case);
        let created_at = before["createdAt"].as_f64().unwrap_or(-1.0);

        // Far short of the span the clock owes the first execution, so the cancel
        // lands while every execution is still pending.
        tokio::time::sleep(Duration::from_millis(60)).await;

        let cancelled = ok(&cancel_task(&router, &task_id).await, &case);
        assert_eq!(
            cancelled["taskId"].as_str(),
            Some(task_id.as_str()),
            "case {case:?}: CancelTask reports the task it cancelled, got {cancelled}"
        );

        let after = ok(&describe_task(&router, &task_id).await, &case);
        assert_eq!(
            after["state"].as_str(),
            Some("CANCELED"),
            "case {case:?}: a cancelled task must report CANCELED afterwards, got {after}"
        );
        let last_updated_at = after["lastUpdatedAt"].as_f64().unwrap_or(-1.0);
        assert!(
            last_updated_at > created_at,
            "case {case:?}: the cancel must advance lastUpdatedAt past createdAt \
             ({created_at}), got {last_updated_at}"
        );
        assert_eq!(
            after["targets"],
            json!(targets),
            "case {case:?}: cancelling must not disturb the stored targets, got {after}"
        );

        let listed = ok(
            &list_executions(&router, &format!("?taskId={task_id}")).await,
            &case,
        );
        let executions = executions_of(&listed, &case);
        assert_eq!(
            executions.len(),
            targets.len(),
            "case {case:?}: cancelling must not drop an execution, got {listed}"
        );
        for execution in &executions {
            assert_eq!(
                execution["state"].as_str(),
                Some("CANCELED"),
                "case {case:?}: a pending execution of a cancelled task is cancelled, \
                 got {execution}"
            );
        }

        // Cancelling again changes nothing: there is nothing left to stop.
        ok(&cancel_task(&router, &task_id).await, &case);
        let again = ok(&describe_task(&router, &task_id).await, &case);
        assert_eq!(
            again["state"].as_str(),
            Some("CANCELED"),
            "case {case:?}: a second cancel must leave the task CANCELED, got {again}"
        );
        assert_eq!(
            again["lastUpdatedAt"], after["lastUpdatedAt"],
            "case {case:?}: a second cancel has nothing to stop, so it must not move \
             lastUpdatedAt, got {again}"
        );
    }
}

#[tokio::test]
async fn the_task_operations_404_a_task_id_that_names_no_task() {
    let router = emulator();
    // Rows differ in form: a plain word, a well-shaped but unused id, and an id
    // belonging to another resource kind.
    let rows: &[&str] = &[
        "does-not-exist",
        "st-ffffffffffffffff",
        "smd-0000000000000000",
    ];
    for task_id in rows {
        assert_not_found(
            &describe_task(&router, task_id).await,
            &format!("DescribeTask taskId={task_id}"),
        );
        assert_not_found(
            &cancel_task(&router, task_id).await,
            &format!("CancelTask taskId={task_id}"),
        );
        assert_not_found(
            &list_executions(&router, &format!("?taskId={task_id}")).await,
            &format!("ListExecutions taskId={task_id}"),
        );
    }
}

#[tokio::test]
async fn describe_execution_reports_the_stored_execution_for_every_target() {
    // Rows differ in how many devices the task targets and in the command, so the
    // execution the pair addresses is not one shape of record.
    let rows: &[(&str, usize)] = &[("unlock", 1), ("reboot", 3)];
    for (command, target_count) in rows {
        let router = emulator();
        let case = format!("{command} over {target_count} targets");
        let (task_id, _, targets) = queue_task(&router, &[command], *target_count, None).await;

        let listed = ok(
            &list_executions(&router, &format!("?taskId={task_id}")).await,
            &case,
        );
        let summaries = executions_of(&listed, &case);
        assert_eq!(
            summaries.len(),
            targets.len(),
            "case {case:?}: every target has an execution, got {listed}"
        );

        // DescribeExecution and ListExecutions are two projections of ONE record.
        for summary in &summaries {
            let device = string(summary, "managedDeviceId", &case);
            let case = format!("{case}, device {device}");
            let described = ok(&describe_execution(&router, &task_id, &device).await, &case);
            for member in ["taskId", "executionId", "managedDeviceId", "state"] {
                assert_eq!(
                    described[member], summary[member],
                    "case {case:?}: DescribeExecution and ListExecutions must project the \
                     same stored {member}, got {} and {}",
                    described[member], summary[member]
                );
            }
            let state = string(&described, "state", &case);
            assert!(
                EXECUTION_STATES.contains(&state.as_str()),
                "case {case:?}: state {state:?} is not one of ExecutionState's values \
                 {EXECUTION_STATES:?}"
            );
            assert!(
                described["startedAt"].is_null(),
                "case {case:?}: a queued execution has not started, so it reports no \
                 startedAt, got {described}"
            );
            assert!(
                described["lastUpdatedAt"].as_f64().unwrap_or(-1.0) > 0.0,
                "case {case:?}: lastUpdatedAt must be the stored instant, got {described}"
            );
        }

        // The pair is what addresses an execution: a device the task did not target,
        // and a task that does not exist, both answer the modelled 404.
        let untargeted: Vec<String> = ids_of(&whole_inventory(&router, &case).await, &case)
            .into_iter()
            .filter(|id| !targets.contains(id))
            .collect();
        for device in untargeted {
            assert_not_found(
                &describe_execution(&router, &task_id, &device).await,
                &format!("{case}, untargeted device {device}"),
            );
        }
        assert_not_found(
            &describe_execution(&router, "st-ffffffffffffffff", &targets[0]).await,
            &format!("{case}, unknown task"),
        );

        // And the clock carries the record this operation reports: past the dispatch
        // span the execution is running and reports the instant it started.
        tokio::time::sleep(EXECUTION_START_SPAN + Duration::from_millis(150)).await;
        let running = ok(
            &describe_execution(&router, &task_id, &targets[0]).await,
            &case,
        );
        assert_eq!(
            running["state"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: past the dispatch span the execution is running, got {running}"
        );
        assert!(
            running["startedAt"].as_f64().unwrap_or(-1.0) > 0.0,
            "case {case:?}: a running execution reports the instant it started, got {running}"
        );
    }
}

#[tokio::test]
async fn list_executions_narrows_a_task_to_one_execution_state() {
    let router = emulator();
    let (task_id, _, targets) = queue_task(&router, &["unlock"], 3, None).await;

    // One row per value of `ExecutionState`: a freshly created task's executions are
    // all QUEUED, so every other state selects nothing.
    for state in EXECUTION_STATES {
        let case = format!("taskId={task_id}, state={state}");
        let body = ok(
            &list_executions(&router, &format!("?taskId={task_id}&state={state}")).await,
            &case,
        );
        let expected = if *state == "QUEUED" { targets.len() } else { 0 };
        let executions = executions_of(&body, &case);
        assert_eq!(
            executions.len(),
            expected,
            "case {case:?}: expected {expected} executions in state {state}, got {body}"
        );
        for execution in &executions {
            assert_eq!(
                execution["state"].as_str(),
                Some(*state),
                "case {case:?}: a filtered page must hold only that state, got {execution}"
            );
        }
    }
}

#[tokio::test]
async fn list_executions_pages_at_every_page_size() {
    let router = emulator();
    let (task_id, _, targets) = queue_task(&router, &["reboot"], 3, None).await;
    let whole = ok(
        &list_executions(&router, &format!("?taskId={task_id}")).await,
        "whole task",
    );
    let all: Vec<String> = executions_of(&whole, "whole task")
        .iter()
        .map(|execution| string(execution, "managedDeviceId", "whole task"))
        .collect();
    assert_eq!(
        all.len(),
        targets.len(),
        "case \"whole task\": every target has an execution, got {whole}"
    );

    for size in MAX_RESULTS_FLOOR..=all.len() as i64 {
        let case = format!("taskId={task_id}, maxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match token.as_deref() {
                None => format!("?taskId={task_id}&maxResults={size}"),
                Some(token) => format!("?taskId={task_id}&maxResults={size}&nextToken={token}"),
            };
            let body = ok(&list_executions(&router, &query).await, &case);
            let page = executions_of(&body, &case);
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page must hold at most maxResults executions, got {body}"
            );
            walked.extend(
                page.iter()
                    .map(|execution| string(execution, "managedDeviceId", &case)),
            );
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging did not terminate; walked {walked:?}"
            );
            match body["nextToken"].as_str() {
                None => break,
                Some(next) => {
                    assert_modelled_token(next, &case);
                    token = Some(next.to_string());
                }
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must concatenate to the task's whole execution \
             list, in order"
        );
    }
}

#[tokio::test]
async fn the_clock_carries_an_execution_to_succeeded_and_applies_the_command() {
    // Rows differ in command, which is what decides the effect on the device:
    // `Command$unlock` "Unlocks the device", `Command$reboot` "Reboots the device".
    let rows: &[(&str, &str)] = &[("unlock", "UNLOCKED"), ("reboot", "LOCKED")];
    for (command, device_state_after) in rows {
        let router = emulator();
        let case = format!("{command} settles");
        let (task_id, _, targets) = queue_task(&router, &[command], 1, None).await;
        let device = targets[0].clone();
        let before = ok(&describe_device(&router, &device).await, &case);
        assert_eq!(
            before["deviceState"].as_str(),
            Some("LOCKED"),
            "case {case:?}: an activated device is locked until a task unlocks it, got {before}"
        );

        // The span is never zero, so the caller right after the create still sees the
        // queued execution — asserted by the create case above. Past the first span
        // the clock owes the execution IN_PROGRESS.
        tokio::time::sleep(EXECUTION_START_SPAN + Duration::from_millis(150)).await;
        let listed = ok(
            &list_executions(&router, &format!("?taskId={task_id}")).await,
            &case,
        );
        let executions = executions_of(&listed, &case);
        assert_eq!(
            executions[0]["state"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: past the dispatch span the execution is running, got {listed}"
        );
        // `ExecutionSummary` carries no `startedAt` (only `DescribeExecution` reports
        // it, which `describe_execution_reports_the_stored_execution_for_every_target`
        // pins), so the running execution's own instant is not observable here; what
        // this page states is its state.
        let running = ok(&describe_task(&router, &task_id).await, &case);
        assert_eq!(
            running["state"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: the task is still running while its execution is, got {running}"
        );

        // Past the second span the execution has finished and the task with it.
        tokio::time::sleep(EXECUTION_FINISH_SPAN + Duration::from_millis(250)).await;
        let listed = ok(
            &list_executions(&router, &format!("?taskId={task_id}")).await,
            &case,
        );
        let executions = executions_of(&listed, &case);
        assert_eq!(
            executions[0]["state"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: past both spans the execution has finished, got {listed}"
        );

        let finished = ok(&describe_task(&router, &task_id).await, &case);
        assert_eq!(
            finished["state"].as_str(),
            Some("COMPLETED"),
            "case {case:?}: a task whose every execution finished is completed, got \
             {finished}"
        );
        let completed_at = finished["completedAt"].as_f64().unwrap_or(-1.0);
        let created_at = finished["createdAt"].as_f64().unwrap_or(-1.0);
        assert!(
            completed_at >= created_at && created_at > 0.0,
            "case {case:?}: completedAt must be the instant the task finished, got \
             {finished}"
        );

        // And the command reached the device: this is the whole point of the task.
        let after = ok(&describe_device(&router, &device).await, &case);
        assert_eq!(
            after["deviceState"].as_str(),
            Some(*device_state_after),
            "case {case:?}: expected the device to be {device_state_after} once the \
             {command} finished, got {after}"
        );
        let reached_out_before = before["lastReachedOutAt"].as_f64().unwrap_or(-1.0);
        let reached_out_after = after["lastReachedOutAt"].as_f64().unwrap_or(-1.0);
        assert!(
            reached_out_after > reached_out_before,
            "case {case:?}: a device that answered a command checked in, so \
             lastReachedOutAt must advance past {reached_out_before}, got \
             {reached_out_after}"
        );

        // A cancel after the fact has nothing to stop: the task keeps the state the
        // clock already gave it.
        ok(&cancel_task(&router, &task_id).await, &case);
        let still = ok(&describe_task(&router, &task_id).await, &case);
        assert_eq!(
            still["state"].as_str(),
            Some("COMPLETED"),
            "case {case:?}: a completed task cannot be cancelled back, got {still}"
        );
    }
}
