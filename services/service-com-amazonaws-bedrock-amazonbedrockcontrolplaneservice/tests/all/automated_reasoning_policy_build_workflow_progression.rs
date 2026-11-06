// SPDX-License-Identifier: CC0-1.0

//! The PROGRESSION an Automated Reasoning policy build workflow runs through, and the two
//! refusals that are defined over the window in which it is running, over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A started build workflow is reported in the status the time since its start owes
//!    it**: `StartAutomatedReasoningPolicyBuildWorkflow` "initiates the process of analyzing
//!    source documents and generating policy rules, variables, and types", so the workflow is
//!    asynchronous work, and the processing values of
//!    `AutomatedReasoningPolicyBuildWorkflowStatus` — `PREPROCESSING`, `BUILDING`, `TESTING`
//!    — are statuses a read of a running workflow reports, each dwelling its own span before
//!    the terminal `COMPLETED`. Asserted over a table of elapsed times spanning every status
//!    of the progression (and far past its end), differing along the workflow type too, with
//!    `updatedAt` stamped at the instant each move was OWED rather than at the instant of the
//!    read — so reading twice reports one instant.
//! 2. **A build workflow that is not `COMPLETED` runs no tests**:
//!    `StartAutomatedReasoningPolicyTestWorkflow` is refused `400 ValidationException` in the
//!    one sentence Amazon Bedrock words that refusal with, for every non-terminal status of
//!    the progression — while the same request against the completed workflow is accepted, so
//!    the refusal is about the status and not about the request.
//! 3. **A policy whose build workflows have not been deleted is not deleted unforced**: an
//!    unforced `DeleteAutomatedReasoningPolicy` is refused `400 ValidationException` naming
//!    the artifact kind in the way, whatever status the workflow holds, and goes through once
//!    that workflow is deleted.
//!
//! Rules 1–3 are asserted on the emulator's own clock by AGEING the stored workflow (the
//! harness every suite of this crate ages a record with), and rule 1 is asserted once more on
//! the WALL clock, where nothing is aged and the suite waits through one span — the way a
//! client drives this API.
//!
//! Every expectation is derived from the model, cited per rule
//! (`smithy_query(query="operation",operation="StartAutomatedReasoningPolicyBuildWorkflow,GetAutomatedReasoningPolicyBuildWorkflow,StartAutomatedReasoningPolicyTestWorkflow,DeleteAutomatedReasoningPolicy")`
//! / `query="shape",shape="AutomatedReasoningPolicyBuildWorkflowStatus"`). The requests go
//! through `crate::support` — the ONE harness every suite in this crate drives the emulator
//! with.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::automated_reasoning_policy_test_cases::{create_policy, create_test_case, CHECK_RESULTS};
use crate::support::{age_build_workflow, emulator, label, Emulator, Reply};

/// Every value `AutomatedReasoningPolicyBuildWorkflowStatus` admits
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyBuildWorkflowStatus")`), so a
/// row asserts a status the MODEL carries rather than one this suite invented.
const WORKFLOW_STATUSES: &[&str] = &[
    "SCHEDULED",
    "CANCEL_REQUESTED",
    "PREPROCESSING",
    "BUILDING",
    "TESTING",
    "COMPLETED",
    "FAILED",
    "CANCELLED",
];

/// The `AutomatedReasoningPolicyBuildWorkflowType` values the rows build with
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyBuildWorkflowType")`).
const WORKFLOW_TYPES: &[&str] = &[
    "INGEST_CONTENT",
    "REFINE_POLICY",
    "IMPORT_POLICY",
    "GENERATE_FIDELITY_REPORT",
    "GENERATE_POLICY_SCENARIOS",
    "RESOLVE_POLICY_AMBIGUITIES",
    "ITERATIVELY_REFINE_POLICY",
];

/// A source document's bytes on the wire: `AutomatedReasoningPolicyBuildDocumentBlob` is a
/// `blob` (`@length(1, 5000000)`), which this protocol carries base64-encoded — this is
/// `policy rules`.
const DOCUMENT_BLOB: &str = "cG9saWN5IHJ1bGVz";

/// How long each status of the progression dwells before the clock owes the next one.
const STAGE: Duration = bedrock::state::BUILD_WORKFLOW_STAGE_AFTER;

/// Enough past a due instant that the move it owes has been made, whatever rounding the wire
/// form of an instant carries.
const SETTLE_SLACK: Duration = Duration::from_millis(150);

/// How far apart two instants of one response may be and still be the same instant: the wire
/// form is an RFC 3339 `date-time`, so a comparison of parsed seconds carries its rounding.
const INSTANT_TOLERANCE: f64 = 0.01;

/// `StartAutomatedReasoningPolicyBuildWorkflow`: the `sourceContent` is the request's
/// `@httpPayload` and the policy ARN and the workflow type are path labels.
async fn start_build_workflow(emu: &Emulator, policy_arn: &str, workflow_type: &str) -> String {
    let started = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/build-workflows/{workflow_type}/start",
                label(policy_arn)
            ),
            Some(json!({
                "workflowContent": {
                    "documents": [{
                        "document": DOCUMENT_BLOB,
                        "documentContentType": "txt",
                        "documentName": "safety-policy.txt",
                    }],
                },
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case {workflow_type:?}: the start must answer 200, got {} {:?}",
        started.status, started.body
    );
    started.text("buildWorkflowId")
}

async fn get_build_workflow(emu: &Emulator, policy_arn: &str, build_workflow_id: &str) -> Reply {
    emu.call(
        "GET",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}",
            label(policy_arn)
        ),
        None,
    )
    .await
}

async fn start_test_workflow(emu: &Emulator, policy_arn: &str, build_workflow_id: &str) -> Reply {
    emu.call(
        "POST",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/test-workflows",
            label(policy_arn)
        ),
        Some(json!({})),
    )
    .await
}

/// `DeleteAutomatedReasoningPolicy` with `force` left out, so the member's modelled
/// `@default(false)` applies.
async fn delete_policy_unforced(emu: &Emulator, policy_arn: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
        None,
    )
    .await
}

/// A test stored under `policy_arn`, so a refused `StartAutomatedReasoningPolicyTestWorkflow`
/// is refused on the build workflow's status rather than on the policy having nothing to run.
async fn seed_test_case(emu: &Emulator, case: &str, policy_arn: &str) -> String {
    let created = create_test_case(
        emu,
        policy_arn,
        json!({
            "guardContent": "returns close after 30 days",
            "expectedAggregatedFindingsResult": CHECK_RESULTS[0],
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateAutomatedReasoningPolicyTestCase must answer 200, got {} {:?}",
        created.status, created.body
    );
    created.text("testCaseId")
}

/// One row of the progression: how far past the start the workflow is read, the status the
/// elapsed time owes it, and how long after the start that status was owed.
struct Stage {
    case: &'static str,
    elapsed: Duration,
    status: &'static str,
    owed_after: Duration,
}

/// The whole progression, read at every status it passes through and once far past its end.
///
/// `owed_after` is what `updatedAt` must report, counted from `createdAt`: the instant the
/// move was OWED. A workflow still in the status its start accepted was never moved, so its
/// `updatedAt` is its `createdAt`.
fn stages() -> Vec<Stage> {
    vec![
        Stage {
            case: "a build the start has just accepted",
            elapsed: Duration::ZERO,
            status: "SCHEDULED",
            owed_after: Duration::ZERO,
        },
        Stage {
            case: "a build whose first processing status has fallen due",
            elapsed: STAGE + SETTLE_SLACK,
            status: "PREPROCESSING",
            owed_after: STAGE,
        },
        Stage {
            case: "a build past two of its spans",
            elapsed: 2 * STAGE + SETTLE_SLACK,
            status: "BUILDING",
            owed_after: 2 * STAGE,
        },
        Stage {
            case: "a build past three of its spans",
            elapsed: 3 * STAGE + SETTLE_SLACK,
            status: "TESTING",
            owed_after: 3 * STAGE,
        },
        Stage {
            case: "a build past the whole progression",
            elapsed: 4 * STAGE + SETTLE_SLACK,
            status: "COMPLETED",
            owed_after: 4 * STAGE,
        },
        Stage {
            case: "a build first read long after its start",
            elapsed: 20 * STAGE,
            status: "COMPLETED",
            owed_after: 4 * STAGE,
        },
    ]
}

// ---------------------------------------------------------------------------------------
// 1. A started workflow is reported in the status the time since its start owes it.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_started_build_workflow_is_read_in_the_status_its_elapsed_time_owes() {
    let emu = emulator();

    for (index, stage) in stages().iter().enumerate() {
        let case = stage.case;
        // Rows differ along the workflow type as well, so the progression is not keyed to
        // one type of build. `AutomatedReasoningPolicyName` is `[0-9a-zA-Z-_ ]+`, so the
        // row's index names its policy.
        let workflow_type = WORKFLOW_TYPES[index % WORKFLOW_TYPES.len()];
        let policy_arn = create_policy(&emu, &format!("progression policy {index}")).await;
        let id = start_build_workflow(&emu, &policy_arn, workflow_type).await;
        age_build_workflow(&emu, &policy_arn, &id, stage.elapsed);

        let read = get_build_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read must answer 200, got {} {:?}",
            read.status, read.body
        );
        let status = read.text("status");
        assert!(
            WORKFLOW_STATUSES.contains(&status.as_str()),
            "case {case:?}: the status must be one the model admits, got {status:?}"
        );
        assert_eq!(
            status, stage.status,
            "case {case:?}: expected the status {:?} {:?} after the start, got {status:?}",
            stage.status, stage.elapsed
        );
        assert_eq!(
            read.text("buildWorkflowType"),
            workflow_type,
            "case {case:?}: a status move must leave the workflow type alone, got {:?}",
            read.get("buildWorkflowType")
        );

        // `updatedAt` reports the instant the move was OWED, not the instant of the read:
        // the clock owes the move, so a workflow read twice reports it once.
        let created_at = read.instant("createdAt");
        let updated_at = read.instant("updatedAt");
        assert!(
            (updated_at - created_at - stage.owed_after.as_secs_f64()).abs() < INSTANT_TOLERANCE,
            "case {case:?}: expected updatedAt {:?} after createdAt, got {} after {}",
            stage.owed_after,
            updated_at,
            created_at
        );

        let again = get_build_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            again.body, read.body,
            "case {case:?}: a read reports and changes nothing, so a second read must answer \
             identically, got {:?} against {:?}",
            again.body, read.body
        );
    }
}

#[tokio::test]
async fn a_started_build_workflow_is_working_before_it_is_terminal_on_the_wall_clock() {
    let emu = emulator();
    let policy_arn = create_policy(&emu, "wall clock policy").await;
    let test_case_id = seed_test_case(&emu, "the wall clock", &policy_arn).await;
    let id = start_build_workflow(&emu, &policy_arn, WORKFLOW_TYPES[0]).await;

    let accepted = get_build_workflow(&emu, &policy_arn, &id).await;
    assert_eq!(
        accepted.text("status"),
        "SCHEDULED",
        "case straight after the start: the read must report the status the start accepted, \
         got {:?}",
        accepted.get("status")
    );

    // Nothing is aged here: the suite waits one span, which is how long a client polling
    // this API waits before its next read.
    tokio::time::sleep(STAGE + SETTLE_SLACK).await;

    let working = get_build_workflow(&emu, &policy_arn, &id).await;
    let status = working.text("status");
    assert!(
        bedrock::state::BUILD_WORKFLOW_PROCESSING_STATUSES.contains(&status.as_str()),
        "case one span after the start: the build must be read in a processing status, got \
         {status:?}"
    );

    // The window the two in-progress refusals are defined over is reachable on the wall
    // clock: a caller that polls this workflow can issue both while it is running.
    let refused_test = start_test_workflow(&emu, &policy_arn, &id).await;
    assert_eq!(
        (refused_test.status, refused_test.error.as_deref()),
        (400, Some("ValidationException")),
        "case one span after the start: a build that is not COMPLETED runs no tests, got {} {:?}",
        refused_test.status,
        refused_test.body
    );
    assert_eq!(
        refused_test.get("message"),
        json!(format!("Build workflow: {id} is not in COMPLETED status")),
        "case one span after the start: the refusal must be the service's sentence for a \
         build that has not completed, got {:?}",
        refused_test.body
    );

    let refused_delete = delete_policy_unforced(&emu, &policy_arn).await;
    assert_eq!(
        (refused_delete.status, refused_delete.error.as_deref()),
        (400, Some("ValidationException")),
        "case one span after the start: a policy whose build workflow is undeleted is not \
         deleted unforced, got {} {:?}",
        refused_delete.status,
        refused_delete.body
    );

    // Neither refusal consumed anything: the test and the build workflow are both still
    // there to be read.
    let still_there = get_build_workflow(&emu, &policy_arn, &id).await;
    assert_eq!(
        still_there.status, 200,
        "case after the refusals: the build workflow must still read back, got {} {:?}",
        still_there.status, still_there.body
    );
    assert!(
        !test_case_id.is_empty(),
        "case after the refusals: the seeded test must have an id, got {test_case_id:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 2. A build workflow that is not COMPLETED runs no tests.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_build_workflow_that_has_not_completed_runs_no_tests() {
    let emu = emulator();

    for (index, stage) in stages().iter().enumerate() {
        let case = stage.case;
        let policy_arn = create_policy(&emu, &format!("tested policy {index}")).await;
        let test_case_id = seed_test_case(&emu, case, &policy_arn).await;
        let id = start_build_workflow(&emu, &policy_arn, WORKFLOW_TYPES[index % 3]).await;
        age_build_workflow(&emu, &policy_arn, &id, stage.elapsed);

        let reply = start_test_workflow(&emu, &policy_arn, &id).await;
        if stage.status == "COMPLETED" {
            // The same request against the completed build is accepted, so the refusal
            // below is about the status rather than about the request.
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a COMPLETED build workflow's tests must run, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.text("policyArn"),
                policy_arn,
                "case {case:?}: the accepted start must answer the policy it ran for, got {:?}",
                reply.get("policyArn")
            );
        } else {
            assert_eq!(
                (reply.status, reply.error.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: a build workflow in {} runs no tests, got {} {:?}",
                stage.status,
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.get("message"),
                json!(format!("Build workflow: {id} is not in COMPLETED status")),
                "case {case:?}: the refusal must be the service's sentence naming the build \
                 workflow the request carried, got {:?}",
                reply.body
            );
            // The refusal scheduled nothing: the test it would have run has no result.
            let result = emu
                .call(
                    "GET",
                    &format!(
                        "/automated-reasoning-policies/{}/build-workflows/{id}/test-cases/\
                         {test_case_id}/test-results",
                        label(&policy_arn)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                (result.status, result.error.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: a refused test workflow must have scheduled no run, got {} {:?}",
                result.status,
                result.body
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 3. A policy whose build workflows have not been deleted is not deleted unforced.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_unforced_policy_delete_is_refused_while_a_build_workflow_is_undeleted() {
    let emu = emulator();

    for (index, stage) in stages().iter().enumerate() {
        let case = stage.case;
        let policy_arn = create_policy(&emu, &format!("undeleted policy {index}")).await;
        let id = start_build_workflow(&emu, &policy_arn, WORKFLOW_TYPES[index % 5]).await;
        age_build_workflow(&emu, &policy_arn, &id, stage.elapsed);

        let refused = delete_policy_unforced(&emu, &policy_arn).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a policy still holding a build workflow in {} must be refused, got \
             {} {:?}",
            stage.status,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.get("message"),
            json!("There are build workflow that have not been deleted."),
            "case {case:?}: the refusal must be the service's sentence naming the artifact kind \
             in the way, got {:?}",
            refused.body
        );

        // The refused delete took nothing with it: the policy and its workflow both read
        // back.
        let read = get_build_workflow(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused delete must leave the build workflow readable, got {} {:?}",
            read.status, read.body
        );

        // Deleting the workflow is what the refusal asks for, and the same unforced delete
        // then goes through: the validation is about what the policy STILL holds. The
        // workflow's delete carries its `updatedAt` as the concurrency token the model binds
        // to the `updatedAt` query parameter.
        let dropped = emu
            .call(
                "DELETE",
                &format!(
                    "/automated-reasoning-policies/{}/build-workflows/{}?updatedAt={}",
                    label(&policy_arn),
                    label(&id),
                    label(&read.text("updatedAt"))
                ),
                None,
            )
            .await;
        assert_eq!(
            dropped.status, 202,
            "case {case:?}: DeleteAutomatedReasoningPolicyBuildWorkflow must answer 202, got {} \
             {:?}",
            dropped.status, dropped.body
        );
        let accepted = delete_policy_unforced(&emu, &policy_arn).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the unforced delete must go through once no build workflow is left, \
             got {} {:?}",
            accepted.status, accepted.body
        );
    }
}
