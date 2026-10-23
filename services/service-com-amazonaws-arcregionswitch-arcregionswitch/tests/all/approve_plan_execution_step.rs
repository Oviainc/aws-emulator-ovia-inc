// SPDX-License-Identifier: CC0-1.0

//! `ApprovePlanExecutionStep` when there is nothing to decide.
//!
//! The rule under test: a manual-approval decision applies to the execution its plan is
//! ONGOING. An execution that has reached a state it never leaves is past deciding, so a
//! plan that is ongoing none has nothing to decide and the call is the modelled
//! `ResourceNotFoundException` (HTTP 404) — whichever way that execution ended, and
//! whichever decision the caller sends. Its message is built from the request: it names
//! the plan the request named, the way the ongoing-execution refusals of
//! `StartPlanExecution` and `DeletePlan` do. A request that names no stored plan at all is
//! the same not-found message every plan lookup in this service answers with.
//!
//! The cases where the plan IS ongoing an execution — the step it waits on, a step it has
//! not reached, a step already decided — live in `execution_progression.rs` beside the
//! progression they belong to.
//!
//! Every case drives the emulator through the same `MultiServiceRouter::dispatch` the
//! router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{self, EXECUTION_START_SPAN};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The canonical account id every ARN a generated emulator mints carries
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// How long a case waits for a transition the clock owes: the span itself plus enough
/// slack that a loaded test machine does not read the state before it is due.
const SETTLE_WAIT: Duration = Duration::from_millis(250);

/// This one service on a fresh shared engine, plus the engine itself — so a case about an
/// approval window measured in minutes can ask the store to apply what it would owe at a
/// later instant instead of sleeping for it.
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

    /// Wait for the transition the clock owes now, the way a client would.
    async fn wait_for_the_clock(&self) {
        tokio::time::sleep(EXECUTION_START_SPAN + SETTLE_WAIT).await;
    }

    /// Apply the transitions the clock will owe `ahead` from now — the same `settle` every
    /// handler runs first, at a later instant, so a case about a window measured in
    /// minutes does not sleep through it.
    fn settle_ahead(&self, ahead: Duration) {
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

/// The `message` of an error response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// One step carrying a manual-approval execution block, with the window its configuration
/// gives the approval.
///
/// Members from `smithy_query(query="shape", shape="ExecutionApprovalConfiguration",
/// model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `approvalRole` is required and `timeoutMinutes` is `@range(min: 1)` with `@default: 60`.
fn approval_step(name: &str, timeout_minutes: u32) -> Value {
    json!({
        "name": name,
        "executionBlockType": "ManualApproval",
        "executionBlockConfiguration": {
            "executionApprovalConfig": {
                "approvalRole": format!("arn:aws:iam::{ACCOUNT}:role/Approver"),
                "timeoutMinutes": timeout_minutes,
            }
        }
    })
}

/// One step carrying a Lambda execution block: it invokes another service's function, so
/// this emulator cannot perform it and the clock fails the execution when it gets there.
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

/// Create a plan whose `activate` workflow runs `steps`, and return its ARN.
async fn plan_with_steps(emulator: &Emulator, case: &str, name: &str, steps: Vec<Value>) -> String {
    let request = json!({
        "name": name,
        "regions": ["us-east-1", "us-west-2"],
        "recoveryApproach": "activePassive",
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
        "workflows": [{
            "workflowTargetAction": "activate",
            "steps": steps,
        }],
    });
    let (status, created) = emulator.call("CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start an `activate` execution of `plan_arn` in `us-west-2` and return its id.
async fn start_execution(emulator: &Emulator, case: &str, plan_arn: &str) -> String {
    let (status, started) = emulator
        .call(
            "StartPlanExecution",
            &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPlanExecution expected 200, got {status} with {started}"
    );
    started["executionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Read one execution back through `GetPlanExecution`.
async fn get_execution(
    emulator: &Emulator,
    case: &str,
    plan_arn: &str,
    execution_id: &str,
) -> Value {
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
    got
}

/// Decide one step of one execution.
async fn decide(
    emulator: &Emulator,
    plan_arn: &str,
    execution_id: &str,
    step_name: &str,
    approval: &str,
) -> (u16, Value) {
    emulator
        .call(
            "ApprovePlanExecutionStep",
            &json!({
                "planArn": plan_arn,
                "executionId": execution_id,
                "stepName": step_name,
                "approval": approval,
            }),
        )
        .await
}

/// How an execution reaches a state it never leaves, so the plan is ongoing none.
#[derive(Clone, Copy)]
enum Ending {
    /// Its only step was approved, so the workflow ran out of steps.
    ApprovedToCompletion,
    /// Its only step was declined.
    Declined,
    /// The clock reached a step this emulator cannot perform.
    FailedAtAnUnsupportedStep,
    /// Nobody decided its approval inside the window the step configures.
    ApprovalWindowClosed,
}

/// Once a plan is ongoing no execution, a decision has nothing to apply to: it is the
/// modelled 404, whichever way the execution ended and whichever decision is sent, and it
/// changes nothing.
#[tokio::test]
async fn a_decision_is_refused_once_the_plan_is_ongoing_no_execution() {
    // The rows differ along what the rule generalises over: every way an execution of
    // this emulator reaches a state it never leaves.
    let cases = [
        ("approved to completion", Ending::ApprovedToCompletion),
        ("declined", Ending::Declined),
        (
            "failed at a step this emulator cannot perform",
            Ending::FailedAtAnUnsupportedStep,
        ),
        ("its approval window closed", Ending::ApprovalWindowClosed),
    ];

    for (case, ending) in cases {
        let emulator = Emulator::new();
        let steps = match ending {
            // The approval is the LAST step, so deciding it ends the workflow.
            Ending::ApprovedToCompletion | Ending::Declined => vec![approval_step("Approve", 60)],
            // The step the clock cannot perform comes first, so the execution ends
            // without the approval behind it ever being reached.
            Ending::FailedAtAnUnsupportedStep => {
                vec![lambda_step("ScaleUp"), approval_step("Approve", 60)]
            }
            // The shortest window the member's range allows.
            Ending::ApprovalWindowClosed => vec![approval_step("Approve", 1)],
        };
        let plan_arn = plan_with_steps(&emulator, case, "ending", steps).await;
        let execution_id = start_execution(&emulator, case, &plan_arn).await;

        // Let the clock reach the first step, then end the execution the row's way.
        emulator.wait_for_the_clock().await;
        match ending {
            Ending::ApprovedToCompletion => {
                let (status, body) =
                    decide(&emulator, &plan_arn, &execution_id, "Approve", "approve").await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: approving the waiting step expected 200, got {status} with \
                     {body}"
                );
                // The clock then finds no step left and completes the execution.
                emulator.wait_for_the_clock().await;
            }
            Ending::Declined => {
                let (status, body) =
                    decide(&emulator, &plan_arn, &execution_id, "Approve", "decline").await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: declining the waiting step expected 200, got {status} with \
                     {body}"
                );
            }
            // The clock already ended it when it reached the unsupported step.
            Ending::FailedAtAnUnsupportedStep => {}
            Ending::ApprovalWindowClosed => emulator.settle_ahead(Duration::from_secs(120)),
        }

        // The execution is in a state it never leaves, and the plan is ongoing none.
        let ended = get_execution(&emulator, case, &plan_arn, &execution_id).await;
        let state = ended["executionState"].as_str().unwrap_or_default();
        assert!(
            [
                "completed",
                "canceled",
                "failed",
                "planExecutionTimedOut",
                "completedWithExceptions",
                "completedMonitoringApplicationHealth",
            ]
            .contains(&state),
            "case {case:?}: expected an execution state it never leaves, got {state:?}"
        );
        let (status, page) = emulator.call("ListPlans", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPlans expected 200, got {status} with {page}"
        );
        assert!(
            page["plans"][0]["activePlanExecution"].is_null(),
            "case {case:?}: expected the plan to be ongoing no execution, got {}",
            page["plans"][0]["activePlanExecution"]
        );

        // So any decision, on any step of it, is the modelled not-found — and the message
        // names the plan the request named.
        for approval in ["approve", "decline"] {
            for step_name in ["Approve", "ScaleUp", "NoSuchStep"] {
                let (status, body) =
                    decide(&emulator, &plan_arn, &execution_id, step_name, approval).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: {approval} on {step_name} expected 404, got {status} with \
                     {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: {approval} on {step_name} expected \
                     ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!("There is no active execution ongoing for plan {plan_arn}"),
                    "case {case:?}: {approval} on {step_name} expected the \
                     no-ongoing-execution message, got {body}"
                );
            }
        }

        // And the refusals changed nothing about the execution that ended.
        let after = get_execution(&emulator, case, &plan_arn, &execution_id).await;
        assert_eq!(
            after, ended,
            "case {case:?}: a refused decision changed the ended execution: expected {ended}, \
             got {after}"
        );
    }
}

/// A decision that names an ARN this emulator holds no plan for is the same
/// request-derived not-found message every plan lookup in this service answers with.
#[tokio::test]
async fn a_decision_for_an_arn_no_plan_is_stored_under_is_the_plan_not_found_message() {
    let emulator = Emulator::new();
    let case = "a stored plan the cases must not name";
    let kept = plan_with_steps(&emulator, case, "kept", vec![approval_step("Approve", 60)]).await;
    let deleted =
        plan_with_steps(&emulator, case, "doomed", vec![approval_step("Approve", 60)]).await;
    let (status, body) = emulator.call("DeletePlan", &json!({"arn": deleted})).await;
    assert_eq!(status, 200, "DeletePlan expected 200, got {status} with {body}");

    // The rows differ along why the ARN names no plan.
    let cases = [
        (
            "an ARN no plan was ever created under",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa"),
        ),
        ("a plan that was deleted", deleted.clone()),
        (
            "a stored plan's name with another suffix",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/kept:zzzzzz"),
        ),
    ];
    for (why, arn) in cases {
        assert_ne!(arn, kept, "case {why:?}: the case must not name the stored plan");
        let (status, body) =
            decide(&emulator, &arn, "us-west-2/0123456789abcdef", "Approve", "approve").await;
        assert_eq!(
            status, 404,
            "case {why:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {why:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Plan not found for ARN: {arn}"),
            "case {why:?}: expected the request-derived not-found message, got {body}"
        );
    }
}

/// While a plan IS ongoing an execution, a decision that names another execution id is the
/// modelled not-found for that id — and the waiting execution is untouched, so the step it
/// waits on can still be decided.
#[tokio::test]
async fn a_decision_naming_an_execution_the_plan_is_not_ongoing_is_not_found() {
    let emulator = Emulator::new();
    let case = "another execution id";
    let plan_arn =
        plan_with_steps(&emulator, case, "ongoing", vec![approval_step("Approve", 60)]).await;
    let execution_id = start_execution(&emulator, case, &plan_arn).await;
    emulator.wait_for_the_clock().await;

    // `RecoveryExecutionId`'s own form, `<region>/<16 hex digits>`, so the id is
    // well-formed and simply names no execution of this plan.
    let other = "us-west-2/0123456789abcdef";
    assert_ne!(
        other, execution_id,
        "case {case:?}: the case must not name the ongoing execution"
    );
    let (status, body) = decide(&emulator, &plan_arn, other, "Approve", "approve").await;
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
        error_message(&body),
        format!("No execution {other} found for plan {plan_arn}"),
        "case {case:?}: expected the request-derived message, got {body}"
    );

    let waiting = get_execution(&emulator, case, &plan_arn, &execution_id).await;
    assert_eq!(
        waiting["executionState"],
        json!("pendingManualApproval"),
        "case {case:?}: expected the ongoing execution still waiting, got {}",
        waiting["executionState"]
    );
    let (status, decided) = decide(&emulator, &plan_arn, &execution_id, "Approve", "approve").await;
    assert_eq!(
        status, 200,
        "case {case:?}: the ongoing execution's step expected to be decidable, got {status} with \
         {decided}"
    );
}
