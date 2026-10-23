// SPDX-License-Identifier: CC0-1.0

//! `ListPlanExecutions`: the execution history of ONE plan, over the wire.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//! `ListPlanExecutions` reports the executions `StartPlanExecution` recorded against the
//! plan named by `planArn` — the current one and the finished ones alike — as the
//! `AbbreviatedExecution` projection of each STORED record, most recently started first,
//! selected by `state` when one is given and paged on `maxResults`/`nextToken`; a plan
//! ARN the store holds no plan under answers `ResourceNotFoundException` (HTTP 404), and
//! a `nextToken` this service never issued is refused rather than answered.
//!
//! Every case drives the emulator through the same `MultiServiceRouter::dispatch` the
//! router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The canonical account id every ARN a generated emulator mints carries
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The values the model's `ExecutionState` enum declares, from
/// `smithy_query(query="shape", shape="ExecutionState", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`.
/// The `state` selection is asserted over EVERY one of them, not only the states this
/// fixture's executions reach.
const EXECUTION_STATES: &[&str] = &[
    "inProgress",
    "pausedByFailedStep",
    "pausedByOperator",
    "completed",
    "completedWithExceptions",
    "canceled",
    "planExecutionTimedOut",
    "pendingManualApproval",
    "failed",
    "pending",
    "completedMonitoringApplicationHealth",
];

/// The members of `AbbreviatedExecution`, from
/// `smithy_query(query="shape", shape="AbbreviatedExecution", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`.
/// `GetPlanExecution` reports the same stored record under the same member names, so
/// each one is compared against what that reader answers rather than against a literal.
const ABBREVIATED_EXECUTION_MEMBERS: &[&str] = &[
    "planArn",
    "executionId",
    "version",
    "updatedAt",
    "comment",
    "startTime",
    "endTime",
    "mode",
    "executionState",
    "executionAction",
    "executionRegion",
    "recoveryExecutionId",
    "actualRecoveryTime",
];

/// The `AbbreviatedExecution` members the model marks `@required`, from the same shape
/// query: each has to be present on every reported item.
const REQUIRED_MEMBERS: &[&str] = &[
    "planArn",
    "executionId",
    "startTime",
    "mode",
    "executionState",
    "executionAction",
    "executionRegion",
];

/// This one service on a fresh shared engine, plus the engine itself — so a case can ask
/// the store to apply what the clock will owe instead of sleeping for it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite exercises operation behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Call one operation and return its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("ArcRegionSwitch.{operation}"))
            // The router keys on the signing name in the SigV4 credential scope.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTEST/20260101/us-east-1/arc-region-switch/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Apply the ONE transition the clock owes each started execution, the same `settle`
    /// every handler runs first but at an instant past the span it is owed in — so a
    /// case that needs a finished execution does not sleep for the test machine.
    fn settle_past_the_clock(&self) {
        let ahead = state::EXECUTION_START_SPAN + Duration::from_secs(1);
        state::service_state(&*self.engine)
            .expect("the shared engine holds this service's store")
            .lock()
            .settle(SystemTime::now() + ahead);
    }
}

/// The wire error code of an error response: the `__type` member's shape name.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// One step carrying a Lambda execution block: it invokes another service's function,
/// which this emulator does not run, so the clock fails the execution that reaches it.
///
/// Members from `smithy_query(query="shape", shape="CustomActionLambdaConfiguration",
/// model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `lambdas`, `retryIntervalMinutes` and `regionToRun` are required.
fn lambda_step(name: &str) -> Value {
    json!({
        "name": name,
        "executionBlockType": "CustomActionLambda",
        "executionBlockConfiguration": {
            "customActionLambdaConfig": {
                "timeoutMinutes": 60,
                "retryIntervalMinutes": 1.0,
                "regionToRun": "activatingRegion",
                "lambdas": [{"arn": format!("arn:aws:lambda:us-west-2:{ACCOUNT}:function:switch")}],
            }
        }
    })
}

/// One step carrying a manual-approval execution block — the one block type whose work
/// is inside this service, so the clock really does reach it.
fn approval_step(name: &str) -> Value {
    json!({
        "name": name,
        "executionBlockType": "ManualApproval",
        "executionBlockConfiguration": {
            "executionApprovalConfig": {
                "approvalRole": format!("arn:aws:iam::{ACCOUNT}:role/Approver"),
                "timeoutMinutes": 60,
            }
        }
    })
}

/// Create a plan whose `activate` workflow cannot be performed here and whose
/// `deactivate` workflow is a single manual approval, and return its ARN: the two
/// workflows let one plan record executions that end in DIFFERENT states.
async fn create_plan(emulator: &Emulator, name: &str) -> String {
    let request = json!({
        "name": name,
        "regions": ["us-east-1", "us-west-2"],
        "recoveryApproach": "activePassive",
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
        "workflows": [
            {"workflowTargetAction": "activate", "steps": [lambda_step("scale-up")]},
            {"workflowTargetAction": "deactivate", "steps": [approval_step("confirm-shift")]},
        ],
    });
    let (status, created) = emulator.call("CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One execution the history fixture started, and the state the clock left it in.
struct Started {
    case: &'static str,
    id: String,
    state: &'static str,
}

/// Start one execution of `plan_arn` and return its id.
async fn start(emulator: &Emulator, case: &str, request: &Value) -> String {
    let (status, started) = emulator.call("StartPlanExecution", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPlanExecution expected 200, got {status} with {started}"
    );
    started["executionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A plan carrying a history of three FINISHED executions, oldest first.
///
/// The rows differ along what the rule generalises over: the action, the mode, whether a
/// comment and a `recoveryExecutionId` are set, the workflow each one runs, and the state
/// the clock leaves them in — two of them share a state, so a selection on it has to
/// report both, in order.
async fn plan_with_a_history(emulator: &Emulator) -> (String, Vec<Started>) {
    let plan_arn = create_plan(emulator, "listing").await;
    let mut history: Vec<Started> = Vec::new();

    // 1. `activate` runs the Lambda block, which nothing here performs: the clock fails
    //    the execution, which frees the plan for the next one.
    let case = "an activate execution that fails";
    let id = start(
        emulator,
        case,
        &json!({
            "planArn": plan_arn,
            "targetRegion": "us-west-2",
            "action": "activate",
            "comment": "the first switch",
        }),
    )
    .await;
    emulator.settle_past_the_clock();
    history.push(Started {
        case,
        id,
        state: "failed",
    });

    // 2. `deactivate` stops at its manual approval; approving the only step leaves the
    //    clock no step to reach, so the execution completes.
    let case = "a deactivate execution approved to completion";
    let id = start(
        emulator,
        case,
        &json!({
            "planArn": plan_arn,
            "targetRegion": "us-west-2",
            "action": "deactivate",
            "mode": "ungraceful",
        }),
    )
    .await;
    emulator.settle_past_the_clock();
    let (status, approved) = emulator
        .call(
            "ApprovePlanExecutionStep",
            &json!({
                "planArn": plan_arn,
                "executionId": id,
                "stepName": "confirm-shift",
                "approval": "approve",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ApprovePlanExecutionStep expected 200, got {status} with {approved}"
    );
    emulator.settle_past_the_clock();
    history.push(Started {
        case,
        id: id.clone(),
        state: "completed",
    });

    // 3. `postRecovery` is the clean-up after a recovery: this plan configures no
    //    workflow for it, so there is no block to perform and the execution completes.
    //    It is also the one action `recoveryExecutionId` is required for.
    let case = "a postRecovery execution naming the recovery it follows";
    let recovery_of = id;
    let id = start(
        emulator,
        case,
        &json!({
            "planArn": plan_arn,
            "targetRegion": "us-east-1",
            "action": "postRecovery",
            "recoveryExecutionId": recovery_of,
        }),
    )
    .await;
    emulator.settle_past_the_clock();
    history.push(Started {
        case,
        id,
        state: "completed",
    });

    (plan_arn, history)
}

/// The `items` of a page, which the operation reports as a list even when it is empty.
fn items(case: &str, page: &Value) -> Vec<Value> {
    assert!(
        page["items"].is_array(),
        "case {case:?}: expected items to be a list, got {page}"
    );
    page["items"].as_array().cloned().unwrap_or_default()
}

/// The `executionId` of every item of a page, in the order reported.
fn reported_ids(case: &str, page: &Value) -> Vec<String> {
    items(case, page)
        .iter()
        .map(|item| item["executionId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// List one page of `plan_arn`'s executions, asserting the call succeeded.
async fn list(emulator: &Emulator, case: &str, request: &Value) -> Value {
    let (status, page) = emulator.call("ListPlanExecutions", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPlanExecutions expected 200, got {status} with {page}"
    );
    page
}

/// Every stored execution of the plan is reported, newest first, and each item carries
/// exactly what `GetPlanExecution` reports for that same execution.
#[tokio::test]
async fn list_plan_executions_reports_every_stored_execution_newest_first() {
    let emulator = Emulator::new();
    let (plan_arn, history) = plan_with_a_history(&emulator).await;

    let page = list(
        &emulator,
        "the whole history",
        &json!({"planArn": plan_arn}),
    )
    .await;
    let reported = reported_ids("the whole history", &page);
    let expected: Vec<String> = history.iter().rev().map(|row| row.id.clone()).collect();
    assert_eq!(
        reported, expected,
        "case \"the whole history\": expected the executions newest first as {expected:?}, \
         got {reported:?}"
    );
    assert!(
        page["nextToken"].is_null(),
        "case \"the whole history\": expected no nextToken on a complete page, got {}",
        page["nextToken"]
    );

    // Each item is the stored record, member for member: the reader that reports the
    // same record in full answers the same values.
    for item in items("the whole history", &page) {
        let execution_id = item["executionId"].as_str().unwrap_or_default().to_string();
        let case = format!("item {execution_id}");
        let (status, got) = emulator
            .call(
                "GetPlanExecution",
                &json!({"planArn": plan_arn, "executionId": execution_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlanExecution expected 200, got {status} with {got}"
        );
        for &member in ABBREVIATED_EXECUTION_MEMBERS {
            assert_eq!(
                item[member], got[member],
                "case {case:?}: ListPlanExecutions reported {member} as {}, GetPlanExecution \
                 reports {}",
                item[member], got[member]
            );
        }
        for &member in REQUIRED_MEMBERS {
            assert!(
                !item[member].is_null(),
                "case {case:?}: expected the required member {member} to be present, got {item}"
            );
        }
        assert_eq!(
            item["planArn"],
            json!(plan_arn),
            "case {case:?}: expected planArn {plan_arn:?}, got {}",
            item["planArn"]
        );
    }

    // And the state each item reports is the one the clock left that execution in.
    for row in &history {
        let item = items("the whole history", &page)
            .into_iter()
            .find(|item| item["executionId"].as_str() == Some(row.id.as_str()));
        let item = item.unwrap_or_else(|| {
            panic!(
                "case {:?}: expected execution {} in the reported history, got {page}",
                row.case, row.id
            )
        });
        assert_eq!(
            item["executionState"],
            json!(row.state),
            "case {:?}: expected executionState {:?}, got {}",
            row.case,
            row.state,
            item["executionState"]
        );
    }
}

/// A plan the store holds with no execution recorded against it reports an empty list,
/// not a refusal and not another plan's executions.
#[tokio::test]
async fn a_plan_with_no_executions_reports_an_empty_list() {
    let emulator = Emulator::new();
    let (busy_plan, history) = plan_with_a_history(&emulator).await;
    let idle_plan = create_plan(&emulator, "idle").await;

    let case = "a plan with no executions";
    let page = list(&emulator, case, &json!({"planArn": idle_plan})).await;
    let reported = reported_ids(case, &page);
    assert!(
        reported.is_empty(),
        "case {case:?}: expected no executions, got {reported:?}"
    );
    assert!(
        page["nextToken"].is_null(),
        "case {case:?}: expected no nextToken, got {}",
        page["nextToken"]
    );

    // The other plan's history is untouched by the empty read.
    let case = "the other plan after the empty read";
    let page = list(&emulator, case, &json!({"planArn": busy_plan})).await;
    let reported = reported_ids(case, &page);
    let expected: Vec<String> = history.iter().rev().map(|row| row.id.clone()).collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected {expected:?}, got {reported:?}"
    );
}

/// `state` selects the executions reporting that `ExecutionState`, in the same newest-
/// first order, and selects none when no execution of the plan is in it.
#[tokio::test]
async fn state_selects_only_the_executions_in_it() {
    let emulator = Emulator::new();
    let (plan_arn, history) = plan_with_a_history(&emulator).await;

    for &wanted in EXECUTION_STATES {
        let case = format!("state {wanted}");
        // The expectation comes from the fixture's own outcomes, so a state no execution
        // reached selects nothing — whatever the catalog entry is.
        let expected: Vec<String> = history
            .iter()
            .rev()
            .filter(|row| row.state == wanted)
            .map(|row| row.id.clone())
            .collect();
        let page = list(
            &emulator,
            &case,
            &json!({"planArn": plan_arn, "state": wanted}),
        )
        .await;
        let reported = reported_ids(&case, &page);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        for item in items(&case, &page) {
            assert_eq!(
                item["executionState"],
                json!(wanted),
                "case {case:?}: expected every item in {wanted}, got {}",
                item["executionState"]
            );
        }
    }

    // An absent `state` selects every execution of the plan.
    let case = "no state at all";
    let page = list(&emulator, case, &json!({"planArn": plan_arn})).await;
    let reported = reported_ids(case, &page);
    assert_eq!(
        reported.len(),
        history.len(),
        "case {case:?}: expected all {} executions, got {reported:?}",
        history.len()
    );
}

/// Paging with any `maxResults` the model allows walks every execution exactly once, in
/// the order one unpaged call reports, and ends with no token — with or without a `state`
/// selection.
#[tokio::test]
async fn pagination_walks_every_selected_execution_once() {
    let emulator = Emulator::new();
    let (plan_arn, history) = plan_with_a_history(&emulator).await;
    let newest_first: Vec<String> = history.iter().rev().map(|row| row.id.clone()).collect();

    // `maxResults` is `@range(1..100)`: both published bounds, and the sizes around the
    // length of the list being paged. `None` is the absent member, which takes the
    // model's own `@default` of 100.
    let page_sizes = [Some(1usize), Some(2), Some(3), Some(4), Some(100), None];
    // Paging is asserted over the whole history and over a `state` selection of it, so
    // the token is a position in what was SELECTED, not in the plan's whole log.
    let selections: [Option<&str>; 3] = [None, Some("completed"), Some("failed")];

    for selection in selections {
        let expected: Vec<String> = history
            .iter()
            .rev()
            .filter(|row| selection.is_none_or(|wanted| row.state == wanted))
            .map(|row| row.id.clone())
            .collect();
        for page_size in page_sizes {
            let case = format!("state {selection:?}, maxResults {page_size:?}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut request = json!({"planArn": plan_arn});
                if let Some(size) = page_size {
                    request["maxResults"] = json!(size);
                }
                if let Some(wanted) = selection {
                    request["state"] = json!(wanted);
                }
                if let Some(token) = token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let page = list(&emulator, &case, &request).await;
                let reported = reported_ids(&case, &page);
                if let Some(size) = page_size {
                    assert!(
                        reported.len() <= size,
                        "case {case:?}: a page carried {} executions, more than maxResults \
                         {size}",
                        reported.len()
                    );
                }
                walked.extend(reported);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
                token = page["nextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                walked, expected,
                "case {case:?}: expected paging to walk {expected:?}, got {walked:?}"
            );
            let expected_pages = expected.len().max(1).div_ceil(page_size.unwrap_or(100));
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages, got {pages}"
            );
        }
    }

    // One unpaged call reports the same order the paged walk produced.
    let case = "unpaged";
    let page = list(&emulator, case, &json!({"planArn": plan_arn})).await;
    let reported = reported_ids(case, &page);
    assert_eq!(
        reported, newest_first,
        "case {case:?}: expected {newest_first:?}, got {reported:?}"
    );
}

/// A `maxResults` outside the member's published range is refused by the boundary the
/// model declares, not answered with a clamped page.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    let (plan_arn, _history) = plan_with_a_history(&emulator).await;

    // `ListExecutionsMaxResults` is `@range(min: 1, max: 100)`: one below each published
    // bound and one above it.
    for max_results in [-1i64, 0, 101, 1_000] {
        let case = format!("maxResults {max_results}");
        let (status, body) = emulator
            .call(
                "ListPlanExecutions",
                &json!({"planArn": plan_arn, "maxResults": max_results}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
    }

    // And both published bounds themselves are accepted.
    for max_results in [1i64, 100] {
        let case = format!("maxResults {max_results}");
        let page = list(
            &emulator,
            &case,
            &json!({"planArn": plan_arn, "maxResults": max_results}),
        )
        .await;
        assert!(
            !items(&case, &page).is_empty(),
            "case {case:?}: expected a page of executions, got {page}"
        );
    }
}

/// Every plan ARN the store holds no plan under answers `ResourceNotFoundException`
/// (HTTP 404), with the message this service builds from the ARN the request carried.
#[tokio::test]
async fn an_unknown_plan_arn_is_resource_not_found() {
    let emulator = Emulator::new();
    let (known_plan, _history) = plan_with_a_history(&emulator).await;

    // Each row is well formed by `PlanArn`'s pattern and names no stored plan: a plan
    // never created, and the known plan's name under a different ARN suffix.
    let suffix = known_plan.rsplit(':').next().unwrap_or("aaaaaa").to_string();
    let other_suffix: String = suffix
        .chars()
        .map(|c| if c == 'z' { 'y' } else { 'z' })
        .collect();
    let unknown = [
        (
            "a plan that was never created",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:abc123"),
        ),
        (
            "the known plan's name under another suffix",
            known_plan.replace(&suffix, &other_suffix),
        ),
        (
            "another account's plan",
            "arn:aws-cn:arc-region-switch::210987654321:plan/elsewhere:zzzzzz".to_string(),
        ),
    ];
    for (case, plan_arn) in unknown {
        let (status, body) = emulator
            .call("ListPlanExecutions", &json!({"planArn": plan_arn}))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(state::plan_not_found_message(&plan_arn)),
            "case {case:?}: expected the refusal built from the ARN, got {body}"
        );
    }
}

/// A `nextToken` this service never issued is refused, and the one it DOES issue resumes
/// the walk.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let emulator = Emulator::new();
    let (plan_arn, history) = plan_with_a_history(&emulator).await;
    let total = history.len();

    // The only tokens handed out are positions strictly inside the selected list.
    let cases = [
        ("not a number", "page-two".to_string()),
        ("negative", "-1".to_string()),
        ("exactly at the end of the history", total.to_string()),
        ("one past the end", (total + 1).to_string()),
        ("far past the end", "999".to_string()),
        (
            "a position inside the history but past a narrower selection",
            "1".to_string(),
        ),
    ];
    for (case, next_token) in cases {
        let mut request = json!({"planArn": plan_arn, "nextToken": next_token});
        if case.contains("narrower selection") {
            // `failed` selects one execution, so position 1 is past its end even though
            // the plan's whole history reaches it.
            request["state"] = json!("failed");
        }
        let (status, body) = emulator.call("ListPlanExecutions", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    let case = "the issued token";
    let first = list(
        &emulator,
        case,
        &json!({"planArn": plan_arn, "maxResults": 1}),
    )
    .await;
    let issued = first["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !issued.is_empty(),
        "case {case:?}: expected a nextToken on a partial page, got {first}"
    );
    let second = list(
        &emulator,
        case,
        &json!({"planArn": plan_arn, "maxResults": 1, "nextToken": issued}),
    )
    .await;
    let resumed = reported_ids(case, &second);
    let expected: Vec<String> = history
        .iter()
        .rev()
        .skip(1)
        .take(1)
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(
        resumed, expected,
        "case {case:?}: expected the token to resume at {expected:?}, got {resumed:?}"
    );
}
