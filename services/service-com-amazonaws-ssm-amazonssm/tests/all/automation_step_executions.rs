// SPDX-License-Identifier: CC0-1.0

//! `DescribeAutomationStepExecutions` end to end: the step executions of the automation
//! `StartAutomationExecution` persisted, reported with what the walk has done with each of
//! them, narrowed by the request's filters and served a page at a time.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol").
//!
//! An automation is `Pending` until the service picks it up, so the cases that look past
//! that instant move the clock through the service's own state ([`pick_up`]) rather than
//! sleeping for it.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — the filter key, the values one filter carries, how many filters narrow
//! together, the status the walk gave a step, the page size, the order the page is read in —
//! rather than the single report the recorded unit happened to make.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::ComAmazonawsSsmAmazonssmState;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so a report served here proves the Region came from the request's
/// SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to pin that a step execution is reachable only through the Region its
/// automation started in.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// A well-formed `AutomationExecutionId` (the modelled length is exactly 36) no execution
/// was ever minted under.
const UNKNOWN_EXECUTION_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A token shaped like one of this service's minted ids that names no step execution.
const UNKNOWN_TOKEN: &str = "99999999-8888-7777-6666-555555555555";

/// Every `StepExecutionFilterKey` the model declares, from
/// `smithy_query(query="shape",shape="StepExecutionFilterKey")`. The filter table below
/// must exercise every one of them.
const FILTER_KEYS: [&str; 9] = [
    "StartTimeBefore",
    "StartTimeAfter",
    "StepExecutionStatus",
    "StepExecutionId",
    "StepName",
    "Action",
    "ParentStepExecutionId",
    "ParentStepIteration",
    "ParentStepIteratorValue",
];

/// The bounds the modelled `MaxResults` of this report carries (range 1..50), from
/// `smithy_query(query="operation",operation="DescribeAutomationStepExecutions")`.
const MIN_MAX_RESULTS: i64 = 1;

/// See [`MIN_MAX_RESULTS`].
const MAX_MAX_RESULTS: i64 = 50;

/// A router with just this service mounted, and the engine it was mounted on — the handle
/// a case reads the store through and moves the clock with.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Call `operation` signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call `operation` signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The refusal message of a failure body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// A string member of a response body.
fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or("<unset>").to_string()
}

/// Create an Automation runbook of `name` with `content`, asserting it succeeded.
async fn runbook(router: &MultiServiceRouter, name: &str, content: &Value) {
    let (status, created) = call(
        router,
        "CreateDocument",
        &json!({
            "Name": name,
            "DocumentType": "Automation",
            "Content": content.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the runbook {name} must be created first, got {status} with {created:?}"
    );
}

/// Start an automation in `region`, asserting it succeeded, and answering the id it minted.
async fn started_in_region(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
) -> String {
    let (status, response) = call_in_region(router, region, "StartAutomationExecution", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from StartAutomationExecution, got {status} with \
         {response:?}"
    );
    text(&response, "AutomationExecutionId")
}

/// Start an automation in [`REGION`].
async fn started(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    started_in_region(router, REGION, case, body).await
}

/// An instant well in the past, which is what the clock is moved to below.
fn long_ago() -> SystemTime {
    SystemTime::now() - Duration::from_secs(3_600)
}

/// Let the clock reach the pickup every execution of `region` is waiting for.
///
/// The service owes a started automation ONE transition, a span after the start
/// (`automation::PICKUP_DELAY`); a case that asserts what the walk reported moves the clock
/// to it through the store rather than sleeping for it.
fn pick_up(engine: &Arc<engine::InMemoryEngine>, region: &str) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    for ((held, _), execution) in store.automation_executions.iter_mut() {
        if held == region && execution.runnable_at.is_some() {
            execution.runnable_at = Some(long_ago());
        }
    }
}

/// The instant the walk started the step `step_name` of `execution_id`, read out of the
/// store so a bound can be set relative to it exactly.
fn step_started_at(
    engine: &Arc<engine::InMemoryEngine>,
    execution_id: &str,
    step_name: &str,
) -> SystemTime {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let store = state.lock();
    store
        .automation_execution(REGION, execution_id)
        .and_then(|execution| {
            execution
                .steps
                .iter()
                .find(|step| step.definition.name == step_name)
                .and_then(|step| step.started_at)
        })
        .unwrap_or_else(|| panic!("the step {step_name} must have started"))
}

/// `at` in the ISO 8601 spelling a timestamp filter value carries.
fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant renders")
}

/// Report the step executions of `execution_id`, with whatever else `body` carries.
async fn describe(
    router: &MultiServiceRouter,
    execution_id: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = body.clone();
    request["AutomationExecutionId"] = json!(execution_id);
    call(router, "DescribeAutomationStepExecutions", &request).await
}

/// Report the step executions of `execution_id`, asserting the report succeeded.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    execution_id: &str,
    body: &Value,
) -> Value {
    let (status, response) = describe(router, execution_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAutomationStepExecutions, got {status} \
         with {response:?}"
    );
    response
}

/// The step names a report carries, in the order it carried them.
fn names(report: &Value) -> Vec<String> {
    report["StepExecutions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| text(step, "StepName"))
        .collect()
}

/// A runbook that branches on one parameter, so one path runs and the others do not: four
/// steps of two actions, which is what the filter table narrows.
fn branching_runbook() -> Value {
    json!({
        "schemaVersion": "0.3",
        "description": "Takes one path.",
        "parameters": {
            "Path": {"type": "String", "default": "left"},
        },
        "mainSteps": [
            {
                "name": "decide",
                "action": "aws:branch",
                "inputs": {
                    "Choices": [
                        {"NextStep": "left", "Variable": "{{ Path }}", "StringEquals": "left"},
                        {"NextStep": "right", "Variable": "{{ Path }}", "StringEquals": "right"},
                    ],
                    "Default": "neither",
                },
            },
            {"name": "left", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
            {"name": "right", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
            {"name": "neither", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
        ],
    })
}

/// A runbook of one step named `only`, whose action and inputs the case picks.
fn one_step_runbook(action: &str, inputs: Value) -> Value {
    json!({
        "schemaVersion": "0.3",
        "mainSteps": [{"name": "only", "action": action, "inputs": inputs}],
    })
}

/// Start the branching runbook and let the service pick it up, so the walk has run the
/// `decide` and `left` steps and left `right` and `neither` untouched.
async fn walked(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>) -> String {
    runbook(router, "Branching", &branching_runbook()).await;
    let id = started(router, "a branching start", &json!({"DocumentName": "Branching"})).await;
    pick_up(engine, REGION);
    id
}

/// Every step the runbook declares is reported with what the walk has done with it — the
/// step's own declaration, its minted id, and the window and status the walk gave it — and
/// the report agrees with what `GetAutomationExecution` reports for the same execution.
#[tokio::test]
async fn every_step_of_the_walk_is_reported_with_what_the_walk_did() {
    let (router, engine) = service();
    runbook(&router, "Branching", &branching_runbook()).await;
    let id = started(&router, "a branching start", &json!({"DocumentName": "Branching"})).await;

    // Nothing has picked the execution up yet, so no step has run: every step is reported
    // `Pending`, with neither a window nor resolved inputs.
    let pending = reported(&router, "before the pickup", &id, &json!({})).await;
    assert_eq!(
        names(&pending),
        vec!["decide", "left", "right", "neither"],
        "case \"before the pickup\": expected every step the runbook declares, got {pending}"
    );
    for step in pending["StepExecutions"].as_array().unwrap_or(&Vec::new()) {
        let name = text(step, "StepName");
        assert_eq!(
            text(step, "StepStatus"),
            "Pending",
            "case \"before the pickup\": expected the step {name} to be Pending, got {step}"
        );
        assert!(
            step["ExecutionStartTime"].is_null() && step["ExecutionEndTime"].is_null(),
            "case \"before the pickup\": expected no window for the unstarted step {name}, got \
             {step}"
        );
        assert_eq!(
            text(step, "StepExecutionId").chars().count(),
            36,
            "case \"before the pickup\": expected a minted 36-character step execution id for \
             {name}, got {step}"
        );
    }

    pick_up(&engine, REGION);
    let walked = reported(&router, "after the pickup", &id, &json!({})).await;
    let statuses: Vec<(String, String)> = walked["StepExecutions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| (text(step, "StepName"), text(step, "StepStatus")))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("decide".to_string(), "Success".to_string()),
            ("left".to_string(), "Success".to_string()),
            ("right".to_string(), "Pending".to_string()),
            ("neither".to_string(), "Pending".to_string()),
        ],
        "case \"after the pickup\": expected the one path the branch chose to have run, got \
         {walked}"
    );
    let decide = walked["StepExecutions"][0].clone();
    assert_eq!(
        (
            text(&decide, "Action"),
            text(&decide, "OnFailure"),
            decide["MaxAttempts"].as_i64(),
            decide["ExecutionStartTime"].is_null(),
            decide["ExecutionEndTime"].is_null(),
        ),
        (
            "aws:branch".to_string(),
            "Abort".to_string(),
            Some(1),
            false,
            false
        ),
        "case \"after the pickup\": expected the step's own declaration and the window the walk \
         gave it, got {decide}"
    );

    // The same step executions a `GetAutomationExecution` of this execution reports: one
    // walk, read through two operations.
    let (status, got) = call(
        &router,
        "GetAutomationExecution",
        &json!({"AutomationExecutionId": &id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"round trip\": expected HTTP 200 from GetAutomationExecution, got {status} with \
         {got:?}"
    );
    let through_get: Vec<(String, String, String)> = got["AutomationExecution"]["StepExecutions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| {
            (
                text(step, "StepName"),
                text(step, "StepStatus"),
                text(step, "StepExecutionId"),
            )
        })
        .collect();
    let through_describe: Vec<(String, String, String)> = walked["StepExecutions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|step| {
            (
                text(step, "StepName"),
                text(step, "StepStatus"),
                text(step, "StepExecutionId"),
            )
        })
        .collect();
    assert_eq!(
        through_describe, through_get,
        "case \"round trip\": expected both readers of the one walk to agree, got {walked} and \
         {got}"
    );
    assert!(
        walked["NextToken"].is_null(),
        "case \"after the pickup\": expected no token when a report holds every step, got {walked}"
    );
}

/// A filter selects the step executions that meet it: the key's own test, over any of the
/// values the filter carries, and every filter of the request narrowing together.
#[tokio::test]
async fn a_filter_selects_the_step_executions_that_meet_it() {
    let (router, engine) = service();
    let id = walked(&router, &engine).await;

    let report = reported(&router, "unfiltered", &id, &json!({})).await;
    let left_id = text(&report["StepExecutions"][1], "StepExecutionId");
    let decide_id = text(&report["StepExecutions"][0], "StepExecutionId");
    // The instant the walk started a step, so each timestamp bound is set a second to
    // either side of a real start rather than at a value a recording happened to hold.
    let started_at = step_started_at(&engine, &id, "decide");
    let a_second_before = iso8601(started_at - Duration::from_secs(1));
    let a_second_after = iso8601(started_at + Duration::from_secs(1));

    // One row per filter key the model declares, plus a row per way a filter carries more
    // than one value and a row per pair of filters narrowing together.
    let rows: Vec<(String, Value, Vec<&str>)> = vec![
        (
            "one step by name".to_string(),
            json!([{"Key": "StepName", "Values": ["left"]}]),
            vec!["left"],
        ),
        (
            "two names, either of which selects its step".to_string(),
            json!([{"Key": "StepName", "Values": ["neither", "left"]}]),
            vec!["left", "neither"],
        ),
        (
            "a name no step of the runbook carries".to_string(),
            json!([{"Key": "StepName", "Values": ["nowhere"]}]),
            vec![],
        ),
        (
            "the action the branch step runs".to_string(),
            json!([{"Key": "Action", "Values": ["aws:branch"]}]),
            vec!["decide"],
        ),
        (
            "the action the other three steps run".to_string(),
            json!([{"Key": "Action", "Values": ["aws:sleep"]}]),
            vec!["left", "right", "neither"],
        ),
        (
            "the status of the steps that ran".to_string(),
            json!([{"Key": "StepExecutionStatus", "Values": ["Success"]}]),
            vec!["decide", "left"],
        ),
        (
            "the status of the steps that did not".to_string(),
            json!([{"Key": "StepExecutionStatus", "Values": ["Pending"]}]),
            vec!["right", "neither"],
        ),
        (
            "two statuses, which together are every step".to_string(),
            json!([{"Key": "StepExecutionStatus", "Values": ["Pending", "Success"]}]),
            vec!["decide", "left", "right", "neither"],
        ),
        (
            "a status no step of this walk carries".to_string(),
            json!([{"Key": "StepExecutionStatus", "Values": ["Cancelled"]}]),
            vec![],
        ),
        (
            "one step by its minted id".to_string(),
            json!([{"Key": "StepExecutionId", "Values": [left_id.clone()]}]),
            vec!["left"],
        ),
        (
            "an id no step execution carries".to_string(),
            json!([{"Key": "StepExecutionId", "Values": [UNKNOWN_TOKEN]}]),
            vec![],
        ),
        (
            "started after an instant a second before the walk".to_string(),
            json!([{"Key": "StartTimeAfter", "Values": [a_second_before.clone()]}]),
            vec!["decide", "left"],
        ),
        (
            "started after an instant a second into the walk".to_string(),
            json!([{"Key": "StartTimeAfter", "Values": [a_second_after.clone()]}]),
            vec![],
        ),
        (
            "started before an instant a second into the walk".to_string(),
            json!([{"Key": "StartTimeBefore", "Values": [a_second_after.clone()]}]),
            vec!["decide", "left"],
        ),
        (
            "started before an instant a second before the walk".to_string(),
            json!([{"Key": "StartTimeBefore", "Values": [a_second_before.clone()]}]),
            vec![],
        ),
        (
            "started before an instant long past the walk".to_string(),
            json!([{"Key": "StartTimeBefore", "Values": ["2999-01-01T00:00:00Z"]}]),
            vec!["decide", "left"],
        ),
        (
            "started after an instant long before the walk".to_string(),
            json!([{"Key": "StartTimeAfter", "Values": ["2000-01-01T00:00:00Z"]}]),
            vec!["decide", "left"],
        ),
        // No action of this emulator runs a step inside another step, so no step execution
        // carries the parent details these three keys filter.
        (
            "the parent step of a step nothing ran inside".to_string(),
            json!([{"Key": "ParentStepExecutionId", "Values": [decide_id.clone()]}]),
            vec![],
        ),
        (
            "an iteration of a parent step".to_string(),
            json!([{"Key": "ParentStepIteration", "Values": ["1"]}]),
            vec![],
        ),
        (
            "an iterator value of a parent step".to_string(),
            json!([{"Key": "ParentStepIteratorValue", "Values": ["left"]}]),
            vec![],
        ),
        (
            "an action and a status, which narrow together".to_string(),
            json!([
                {"Key": "Action", "Values": ["aws:sleep"]},
                {"Key": "StepExecutionStatus", "Values": ["Success"]},
            ]),
            vec!["left"],
        ),
        (
            "an action and a name, which narrow together".to_string(),
            json!([
                {"Key": "Action", "Values": ["aws:sleep"]},
                {"Key": "StepName", "Values": ["right"]},
            ]),
            vec!["right"],
        ),
        (
            "two filters nothing meets at once".to_string(),
            json!([
                {"Key": "Action", "Values": ["aws:branch"]},
                {"Key": "StepExecutionStatus", "Values": ["Pending"]},
            ]),
            vec![],
        ),
    ];

    let mut keys_covered: BTreeSet<String> = BTreeSet::new();
    for (case, filters, expected) in &rows {
        for filter in filters.as_array().cloned().unwrap_or_default() {
            keys_covered.insert(text(&filter, "Key"));
        }
        let report = reported(&router, case, &id, &json!({"Filters": filters})).await;
        let expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            names(&report),
            expected,
            "case {case:?}: expected the steps {expected:?} to meet {filters}, got {report}"
        );
    }

    let modelled: BTreeSet<String> = FILTER_KEYS.iter().map(|key| key.to_string()).collect();
    assert_eq!(
        keys_covered, modelled,
        "the table must exercise every modelled filter key: expected {modelled:?}, got \
         {keys_covered:?}"
    );
}

/// The status a filter selects by is the status the WALK gave the step, whatever the step's
/// action did.
#[tokio::test]
async fn the_status_filter_selects_by_the_status_the_walk_gave_the_step() {
    let (router, engine) = service();

    // One row per outcome a step of this emulator's walk reaches.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a wait that is over",
            "aws:sleep",
            json!({"Duration": "PT0S"}),
            "Success",
        ),
        (
            "a wait that is not over",
            "aws:sleep",
            json!({"Duration": "PT1H"}),
            "InProgress",
        ),
        ("a pause", "aws:pause", json!({}), "Waiting"),
        (
            "an action nothing here runs",
            "aws:executeScript",
            json!({"Runtime": "python3.11", "Handler": "main"}),
            "Failed",
        ),
    ];

    for (case, action, inputs, expected) in rows {
        let name = format!("Stepping-{}", case.replace(' ', "-"));
        runbook(&router, &name, &one_step_runbook(action, inputs)).await;
        let id = started(&router, case, &json!({"DocumentName": name})).await;
        pick_up(&engine, REGION);

        let report = reported(&router, case, &id, &json!({})).await;
        assert_eq!(
            text(&report["StepExecutions"][0], "StepStatus"),
            expected,
            "case {case:?}: expected the walk to leave the step {expected}, got {report}"
        );
        let selected = reported(
            &router,
            case,
            &id,
            &json!({"Filters": [{"Key": "StepExecutionStatus", "Values": [expected]}]}),
        )
        .await;
        assert_eq!(
            names(&selected),
            vec!["only"],
            "case {case:?}: expected a {expected} filter to select the step, got {selected}"
        );
        let other = if expected == "Success" {
            "Failed"
        } else {
            "Success"
        };
        let rejected = reported(
            &router,
            case,
            &id,
            &json!({"Filters": [{"Key": "StepExecutionStatus", "Values": [other]}]}),
        )
        .await;
        assert_eq!(
            names(&rejected),
            Vec::<String>::new(),
            "case {case:?}: expected a {other} filter to select no step, got {rejected}"
        );
    }
}

/// A report is served a page at a time, every page size reports the same ordered set, and
/// `ReverseOrder` reads that order backwards.
#[tokio::test]
async fn a_report_pages_through_the_step_executions_in_order() {
    let (router, engine) = service();
    let id = walked(&router, &engine).await;

    let whole = names(&reported(&router, "the whole report", &id, &json!({})).await);
    assert_eq!(
        whole,
        vec!["decide", "left", "right", "neither"],
        "case \"the whole report\": expected the order the walk started the steps in, with the \
         unstarted steps after them, got {whole:?}"
    );

    // One row per page size: each of the sizes that splits this set, the modelled bounds,
    // and the request that names no size at all.
    for page_size in [Some(1_i64), Some(2), Some(3), Some(4), Some(MAX_MAX_RESULTS), None] {
        let case = format!("maxResults={page_size:?}");
        let mut walked_names: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["MaxResults"] = json!(size);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let report = reported(&router, &case, &id, &body).await;
            let page = names(&report);
            if let Some(size) = page_size {
                assert!(
                    i64::try_from(page.len()).unwrap_or(i64::MAX) <= size,
                    "case {case:?}: a page must hold at most {size} step executions, got {page:?}"
                );
            }
            walked_names.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages, got \
                 {walked_names:?}"
            );
            match report["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked_names, whole,
            "case {case:?}: expected paging to report every step once in the one order, got \
             {walked_names:?}"
        );
    }

    // "Indicates whether to list step executions in reverse order by start time": the same
    // set, read backwards.
    let mut backwards = whole.clone();
    backwards.reverse();
    for (case, requested) in [
        ("ReverseOrder true", json!({"ReverseOrder": true})),
        ("ReverseOrder false", json!({"ReverseOrder": false})),
        ("ReverseOrder unset", json!({})),
    ] {
        let report = reported(&router, case, &id, &requested).await;
        let expected = if case == "ReverseOrder true" {
            &backwards
        } else {
            &whole
        };
        assert_eq!(
            &names(&report),
            expected,
            "case {case:?}: expected the order {expected:?}, got {report}"
        );
    }

    // A page of a reversed report resumes in that same reversed order.
    let first = reported(
        &router,
        "a reversed page",
        &id,
        &json!({"ReverseOrder": true, "MaxResults": 2}),
    )
    .await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"a reversed page\": a further page was due: {first}"))
        .to_string();
    let second = reported(
        &router,
        "a reversed page",
        &id,
        &json!({"ReverseOrder": true, "MaxResults": 2, "NextToken": token}),
    )
    .await;
    let mut both = names(&first);
    both.extend(names(&second));
    assert_eq!(
        both, backwards,
        "case \"a reversed page\": expected the reversed order across pages, got {both:?}"
    );
}

/// A report of an execution this endpoint does not hold, of a filter value its key cannot
/// carry, of a token it did not issue or of a page size outside the modelled range is
/// refused with the modelled failure for it.
#[tokio::test]
async fn a_report_refuses_what_it_cannot_serve() {
    let (router, engine) = service();
    let id = walked(&router, &engine).await;

    // An execution id no execution of this endpoint carries.
    let (status, refused) = describe(&router, UNKNOWN_EXECUTION_ID, &json!({})).await;
    assert_eq!(
        (status, error_code(&refused), error_message(&refused)),
        (
            400,
            "AutomationExecutionNotFoundException".to_string(),
            format!("Automation execution {UNKNOWN_EXECUTION_ID} does not exist.")
        ),
        "case \"an unknown execution\": expected the modelled not-found refusal, got {status} \
         with {refused:?}"
    );

    // The same execution, read through another Region's endpoint: an automation runs at the
    // endpoint its start was sent to, so no other Region holds its steps.
    let (status, elsewhere) = call_in_region(
        &router,
        OTHER_REGION,
        "DescribeAutomationStepExecutions",
        &json!({"AutomationExecutionId": &id}),
    )
    .await;
    assert_eq!(
        (status, error_code(&elsewhere)),
        (400, "AutomationExecutionNotFoundException".to_string()),
        "case \"another Region\": expected the execution to be reachable only in the Region it \
         started in, got {status} with {elsewhere:?}"
    );

    // One row per filter value its key cannot carry.
    let bad_values = [
        ("a start bound that is no timestamp", "StartTimeBefore", "yesterday"),
        ("an end bound that is no timestamp", "StartTimeAfter", "1 July 2024"),
        ("a status no step can hold", "StepExecutionStatus", "Finished"),
        ("an iteration that is no number", "ParentStepIteration", "first"),
    ];
    for (case, key, value) in bad_values {
        let (status, refused) = describe(
            &router,
            &id,
            &json!({"Filters": [{"Key": key, "Values": [value]}]}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "InvalidFilterValue".to_string()),
            "case {case:?}: expected the modelled filter-value refusal, got {status} with \
             {refused:?}"
        );
        assert!(
            error_message(&refused).contains(value),
            "case {case:?}: expected the refusal to name the value it could not serve, got {}",
            error_message(&refused)
        );
    }

    // A filter value the key CAN carry, at each published bound of the status vocabulary, is
    // served rather than refused.
    for value in ["Pending", "Success", "Waiting", "TimedOut"] {
        let case = format!("the status {value}");
        let (status, served) = describe(
            &router,
            &id,
            &json!({"Filters": [{"Key": "StepExecutionStatus", "Values": [value]}]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a modelled status value to be served, got {status} with \
             {served:?}"
        );
    }

    // One row per token this report did not issue: one that is not shaped like a minted id
    // at all, and one that is but names no step of the result.
    for (case, token) in [
        ("a token of no shape this service mints", "page-2"),
        ("a token shaped like an id that names no step", UNKNOWN_TOKEN),
    ] {
        let (status, refused) = describe(&router, &id, &json!({"NextToken": token})).await;
        assert_eq!(
            (status, error_code(&refused), error_message(&refused)),
            (
                400,
                "InvalidNextToken".to_string(),
                "The NextToken value is not valid".to_string()
            ),
            "case {case:?}: expected the modelled token refusal, got {status} with {refused:?}"
        );
    }

    // The modelled `MaxResults` range: each published bound is served, and one past either
    // bound is refused.
    for max_results in [MIN_MAX_RESULTS, MAX_MAX_RESULTS] {
        let case = format!("maxResults={max_results}");
        let (status, served) = describe(&router, &id, &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a page size inside the modelled range to be served, got \
             {status} with {served:?}"
        );
    }
    for max_results in [MIN_MAX_RESULTS - 1, MAX_MAX_RESULTS + 1] {
        let case = format!("maxResults={max_results}");
        let (status, refused) = describe(&router, &id, &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a page size outside the modelled range to be refused, got \
             {status} with {refused:?}"
        );
    }
}
