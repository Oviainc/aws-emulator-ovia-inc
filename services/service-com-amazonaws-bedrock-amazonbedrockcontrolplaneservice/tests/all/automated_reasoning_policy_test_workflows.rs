// SPDX-License-Identifier: CC0-1.0

//! The Automated Reasoning policy TEST WORKFLOW end-to-end:
//! `StartAutomatedReasoningPolicyTestWorkflow` schedules the runs of a policy's tests
//! against one of its build workflows and `GetAutomatedReasoningPolicyTestResult` reads one
//! of those runs back, over the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A started test workflow round-trips**: every test the request selected is readable
//!    as a result of the build workflow it ran against, reporting the stored test and a
//!    `testRunStatus` the model admits — for requests spanning a request that names no
//!    test (every test of the policy runs), one that names one, policies holding one, two
//!    and three tests, and tests spanning the `AutomatedReasoningCheckResult` values and
//!    both forms of `queryContent`/`confidenceThreshold`. A test the request did not
//!    select has no result at all.
//! 2. **A run a start has just scheduled has not run yet**: it reports the non-terminal
//!    status until the clock makes its completion due, and the terminal one afterwards —
//!    and a read never moves it, so two reads after the span report ONE instant.
//! 3. **Tests run only against a COMPLETED build workflow**: a start naming a build
//!    workflow in any other status is refused with `ValidationException` carrying the
//!    service's wording for that refusal, while the same start is accepted once that
//!    workflow is `COMPLETED`.
//! 4. **A test workflow is addressed by its policy, its build workflow and its tests**: an
//!    ARN naming no policy of this account and an id nothing started are refused with
//!    `ResourceNotFoundException` (one policy's workflow id names no workflow of another
//!    policy), a `testCaseIds` naming a test the policy does not hold and a policy with no
//!    test at all are refused with `ValidationException`, and a result nothing has run
//!    reads back as nothing.
//! 5. **One test workflow at a time per build workflow**: a second start for a build
//!    workflow whose runs are still running is refused with `ResourceInUseException`,
//!    while the same start is accepted — and re-runs those tests — once they are terminal.
//! 6. **A replayed `clientRequestToken` schedules nothing new**, while a fresh token
//!    re-runs the tests.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="StartAutomatedReasoningPolicyTestWorkflow,GetAutomatedReasoningPolicyTestResult")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{
    age_build_workflow, emulator, label, wall_clock_seconds, Emulator, Reply, ACCOUNT, REGION,
};

/// Every `AutomatedReasoningPolicyTestRunStatus` the model admits
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyTestRunStatus")`).
const TEST_RUN_STATUSES: &[&str] = &[
    "NOT_STARTED",
    "SCHEDULED",
    "IN_PROGRESS",
    "COMPLETED",
    "FAILED",
];

/// The status a run a test workflow has just scheduled carries, before the clock owes it
/// its completion (`AutomatedReasoningPolicyTestRunStatus$SCHEDULED`).
const SCHEDULED: &str = "SCHEDULED";

/// The terminal status the clock owes a scheduled run
/// (`AutomatedReasoningPolicyTestRunStatus$COMPLETED`).
const COMPLETED: &str = "COMPLETED";

/// Every `AutomatedReasoningCheckResult` the model admits, which is what a test's
/// `expectedAggregatedFindingsResult` is drawn from
/// (`smithy_query(query="shape",shape="AutomatedReasoningCheckResult")`).
const CHECK_RESULTS: &[&str] = &[
    "VALID",
    "INVALID",
    "SATISFIABLE",
    "IMPOSSIBLE",
    "TRANSLATION_AMBIGUOUS",
    "TOO_COMPLEX",
    "NO_TRANSLATION",
];

/// The `AutomatedReasoningPolicyBuildWorkflowType` these rows build with: the type that
/// analyses source documents (`AutomatedReasoningPolicyBuildWorkflowType$INGEST_CONTENT`).
const INGEST_CONTENT: &str = "INGEST_CONTENT";

/// A source document's bytes on the wire: `AutomatedReasoningPolicyBuildDocumentBlob` is a
/// `blob` (`@length(1, 5000000)`), which this protocol carries base64-encoded — this is
/// `policy rules`.
const DOCUMENT_BLOB: &str = "cG9saWN5IHJ1bGVz";

/// How far a build workflow a start accepted must be aged to be past the completion the
/// clock owes it: the whole progression the start arms, and a little more, so a workflow
/// aged this much must hold the terminal status.
const PAST_THE_BUILD: Duration =
    bedrock::state::BUILD_WORKFLOW_COMPLETE_AFTER.saturating_add(Duration::from_millis(150));

/// Long enough that a build workflow a cancel took over is past the cancellation the clock
/// owes it: the emulator makes that transition due a fraction of a second after the cancel.
const PAST_THE_CANCELLATION: Duration = Duration::from_millis(900);

/// Long enough that a test run a test workflow scheduled is past the completion the clock
/// owes it, for the same reason [`PAST_THE_CANCELLATION`] is long enough for a cancellation.
const PAST_THE_TEST_RUN: Duration = Duration::from_millis(900);

/// The `ResourceNotFoundException` message the TEST surface answers for a build workflow a
/// request NAMES that nothing holds — the wording Amazon Bedrock answers for that error
/// class, which names neither the policy nor the id the request carried, so it is a FIXED
/// text with no variable part to fill. It is not the sentence an operation ADDRESSED BY a
/// workflow id answers (a read, a cancel, a delete): those are different parts of the
/// service, each with its own recorded wording.
const WORKFLOW_NOT_FOUND_MESSAGE: &str = "Build workflow not found";

/// A `AutomatedReasoningPolicyBuildWorkflowId` nothing in a fresh store has started: the
/// hyphenated UUIDv4 form the member's pattern spells
/// (`^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`).
const UNSTARTED_WORKFLOW_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A `AutomatedReasoningPolicyTestCaseId` nothing in a fresh store has minted, in the
/// `^[0-9A-Z]{12}$` form that member admits.
const UNMINTED_TEST_CASE_ID: &str = "ZZZZZZZZZZZZ";

/// The status the cancel of a build workflow answers with
/// (`CancelAutomatedReasoningPolicyBuildWorkflow` is modelled `@http(code: 202)`).
const CANCEL_ACCEPTED: u16 = 202;

/// One test to create under a policy, spanning the forms
/// `CreateAutomatedReasoningPolicyTestCase`'s input takes.
struct TestSpec {
    guard_content: &'static str,
    query_content: Option<&'static str>,
    expected_result: &'static str,
    confidence_threshold: Option<f64>,
}

/// Create a policy and hand back the `policyArn` it minted.
async fn policy(emu: &Emulator, name: &str) -> String {
    let created = emu
        .call("POST", "/automated-reasoning-policies", Some(json!({ "name": name })))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    created.text("policyArn")
}

/// `CreateAutomatedReasoningPolicyTestCase`: the policy ARN is a path label and the test's
/// content travels in the body.
async fn create_test_case(emu: &Emulator, policy_arn: &str, spec: &TestSpec) -> String {
    let mut body = json!({
        "guardContent": spec.guard_content,
        "expectedAggregatedFindingsResult": spec.expected_result,
    });
    if let Some(query_content) = spec.query_content {
        body["queryContent"] = json!(query_content);
    }
    if let Some(threshold) = spec.confidence_threshold {
        body["confidenceThreshold"] = json!(threshold);
    }
    let created = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/test-cases",
                label(policy_arn)
            ),
            Some(body),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: CreateAutomatedReasoningPolicyTestCase must answer 200, got {} {:?}",
        spec.guard_content, created.status, created.body
    );
    created.text("testCaseId")
}

/// `StartAutomatedReasoningPolicyBuildWorkflow`: the `sourceContent` is the request's
/// `@httpPayload`, and the policy ARN and the workflow type are path labels.
async fn start_build_workflow(emu: &Emulator, policy_arn: &str) -> String {
    let started = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/build-workflows/{INGEST_CONTENT}/start",
                label(policy_arn)
            ),
            Some(json!({
                "workflowContent": {
                    "documents": [{
                        "document": DOCUMENT_BLOB,
                        "documentContentType": "txt",
                        "documentName": "returns.txt",
                    }],
                },
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "StartAutomatedReasoningPolicyBuildWorkflow must answer 200, got {} {:?}",
        started.status, started.body
    );
    started.text("buildWorkflowId")
}

/// A build workflow of `policy_arn` the clock has already completed: the one state a test
/// workflow may run against. The workflow is aged past the progression its start armed
/// rather than slept through.
async fn completed_build_workflow(emu: &Emulator, policy_arn: &str) -> String {
    let id = start_build_workflow(emu, policy_arn).await;
    age_build_workflow(emu, policy_arn, &id, PAST_THE_BUILD);
    id
}

async fn cancel_build_workflow(emu: &Emulator, policy_arn: &str, build_workflow_id: &str) {
    let cancelled = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/cancel",
                label(policy_arn)
            ),
            None,
        )
        .await;
    assert_eq!(
        cancelled.status, CANCEL_ACCEPTED,
        "CancelAutomatedReasoningPolicyBuildWorkflow must answer {CANCEL_ACCEPTED}, got {} {:?}",
        cancelled.status, cancelled.body
    );
}

/// `StartAutomatedReasoningPolicyTestWorkflow`: the policy ARN and the build workflow id are
/// path labels, while `testCaseIds` and `clientRequestToken` travel in the body (neither
/// carries an HTTP binding of its own).
async fn start_test_workflow(
    emu: &Emulator,
    policy_arn: &str,
    build_workflow_id: &str,
    test_case_ids: Option<&[&str]>,
    client_request_token: Option<&str>,
) -> Reply {
    let mut body = json!({});
    if let Some(test_case_ids) = test_case_ids {
        body["testCaseIds"] = json!(test_case_ids);
    }
    if let Some(token) = client_request_token {
        body["clientRequestToken"] = json!(token);
    }
    emu.call(
        "POST",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/test-workflows",
            label(policy_arn)
        ),
        Some(body),
    )
    .await
}

async fn get_test_result(
    emu: &Emulator,
    policy_arn: &str,
    build_workflow_id: &str,
    test_case_id: &str,
) -> Reply {
    emu.call(
        "GET",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/test-cases/\
             {test_case_id}/test-results",
            label(policy_arn)
        ),
        None,
    )
    .await
}

/// The `testResult` member of a read, which the model requires of every success
/// (`GetAutomatedReasoningPolicyTestResultResponse$testResult`).
fn test_result(case: &str, reply: &Reply) -> Value {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: GetAutomatedReasoningPolicyTestResult must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("testResult")
        .as_object()
        .map(|result| Value::Object(result.clone()))
        .unwrap_or_else(|| panic!("case {case:?}: the response carries no testResult: {:?}", reply.body))
}

/// A member of a `testResult` as a string, naming the case when it is absent.
fn text_at(case: &str, result: &Value, path: &[&str]) -> String {
    let mut cursor = result;
    for member in path {
        cursor = cursor
            .get(member)
            .unwrap_or_else(|| panic!("case {case:?}: the testResult has no {member}: {result:?}"));
    }
    cursor
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {path:?} is not a string: {cursor:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------------------
// 1. What a start schedules is what a read of each result reports.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_started_test_workflow_round_trips_through_get_test_result() {
    /// (case, the tests stored under the policy, which of them the request names by index
    /// — `None` names none at all, so every test of the policy runs).
    struct Row {
        case: &'static str,
        tests: Vec<TestSpec>,
        selected: Option<usize>,
    }

    // Rows span a request that selects every test and one that names a single test, over
    // policies holding one, two and three tests, with the tests themselves spanning the
    // check results the enum admits and both forms of the optional members.
    let rows = vec![
        Row {
            case: "one test, none named",
            tests: vec![TestSpec {
                guard_content: "returns close after 30 days",
                query_content: Some("when does a return close?"),
                expected_result: CHECK_RESULTS[0],
                confidence_threshold: Some(0.5),
            }],
            selected: None,
        },
        Row {
            case: "two tests, none named",
            tests: vec![
                TestSpec {
                    guard_content: "returns never close",
                    query_content: None,
                    expected_result: CHECK_RESULTS[1],
                    confidence_threshold: None,
                },
                TestSpec {
                    guard_content: "returns may close",
                    query_content: Some("may a return close?"),
                    expected_result: CHECK_RESULTS[2],
                    confidence_threshold: Some(1.0),
                },
            ],
            selected: None,
        },
        Row {
            case: "three tests, the middle one named",
            tests: vec![
                TestSpec {
                    guard_content: "returns close and stay open",
                    query_content: None,
                    expected_result: CHECK_RESULTS[3],
                    confidence_threshold: Some(0.0),
                },
                TestSpec {
                    guard_content: "returns close, maybe",
                    query_content: Some("ambiguous?"),
                    expected_result: CHECK_RESULTS[4],
                    confidence_threshold: None,
                },
                TestSpec {
                    guard_content: "returns close after thirty times thirty days",
                    query_content: None,
                    expected_result: CHECK_RESULTS[5],
                    confidence_threshold: Some(0.25),
                },
            ],
            selected: Some(1),
        },
        Row {
            case: "two tests, the last one named",
            tests: vec![
                TestSpec {
                    guard_content: "returns are untranslatable",
                    query_content: None,
                    expected_result: CHECK_RESULTS[6],
                    confidence_threshold: None,
                },
                TestSpec {
                    guard_content: "returns close after 7 days",
                    query_content: Some("when?"),
                    expected_result: CHECK_RESULTS[0],
                    confidence_threshold: Some(0.75),
                },
            ],
            selected: Some(1),
        },
    ];

    for row in rows {
        let case = row.case;
        let emu = emulator();
        let policy_arn = policy(&emu, &format!("round trip {}", row.tests.len())).await;
        let mut ids = Vec::new();
        for spec in &row.tests {
            ids.push(create_test_case(&emu, &policy_arn, spec).await);
        }
        let build_workflow_id = completed_build_workflow(&emu, &policy_arn).await;

        let named: Option<Vec<&str>> = row
            .selected
            .map(|index| vec![ids[index].as_str()]);
        let before = wall_clock_seconds();
        let started = start_test_workflow(
            &emu,
            &policy_arn,
            &build_workflow_id,
            named.as_deref(),
            None,
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartAutomatedReasoningPolicyTestWorkflow must answer 200, got {} \
             {:?}",
            started.status, started.body
        );
        assert_eq!(
            started.text("policyArn"),
            policy_arn,
            "case {case:?}: the start must answer the policy it ran the tests of, got {:?}",
            started.get("policyArn")
        );

        // The runs the request selected: the test it named, or every test of the policy.
        let expected: Vec<usize> = match row.selected {
            Some(index) => vec![index],
            None => (0..row.tests.len()).collect(),
        };
        for (index, spec) in row.tests.iter().enumerate() {
            let test_case_id = &ids[index];
            let read = get_test_result(&emu, &policy_arn, &build_workflow_id, test_case_id).await;
            if !expected.contains(&index) {
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: test {test_case_id} was not selected, so it has no result, \
                     got {} {:?}",
                    read.status, read.body
                );
                continue;
            }
            let result = test_result(case, &read);
            assert_eq!(
                text_at(case, &result, &["policyArn"]),
                policy_arn,
                "case {case:?}: the result must name the policy whose test ran, got {result:?}"
            );
            assert_eq!(
                text_at(case, &result, &["testCase", "testCaseId"]),
                *test_case_id,
                "case {case:?}: the result must report the test that ran, got {result:?}"
            );
            assert_eq!(
                text_at(case, &result, &["testCase", "guardContent"]),
                spec.guard_content,
                "case {case:?}: the result must report the stored guardContent, got {result:?}"
            );
            assert_eq!(
                result["testCase"].get("queryContent").and_then(Value::as_str),
                spec.query_content,
                "case {case:?}: expected queryContent {:?}, got {:?}",
                spec.query_content,
                result["testCase"].get("queryContent")
            );
            assert_eq!(
                text_at(case, &result, &["testCase", "expectedAggregatedFindingsResult"]),
                spec.expected_result,
                "case {case:?}: the result must report the expected finding the test carries, \
                 got {result:?}"
            );
            assert_eq!(
                result["testCase"]
                    .get("confidenceThreshold")
                    .and_then(Value::as_f64),
                spec.confidence_threshold,
                "case {case:?}: expected confidenceThreshold {:?}, got {:?}",
                spec.confidence_threshold,
                result["testCase"].get("confidenceThreshold")
            );
            let status = text_at(case, &result, &["testRunStatus"]);
            assert!(
                TEST_RUN_STATUSES.contains(&status.as_str()),
                "case {case:?}: {status:?} is not an AutomatedReasoningPolicyTestRunStatus"
            );
            let updated_at = read_instant(case, &result);
            let after = wall_clock_seconds();
            assert!(
                updated_at >= before - 1.0 && updated_at <= after + 1.0,
                "case {case:?}: the run's updatedAt must fall in the span this request ran \
                 in, got {updated_at} for [{before}, {after}]"
            );
        }
    }
}

/// The `updatedAt` of a `testResult`, which is modelled `@timestampFormat("date-time")`, so
/// the wire form is an RFC 3339 string and anything else is a defect.
fn read_instant(case: &str, result: &Value) -> f64 {
    let raw = text_at(case, result, &["updatedAt"]);
    crate::support::date_time_seconds(case, &raw)
}

// ---------------------------------------------------------------------------------------
// 2. A run a start has just scheduled has not run yet.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_scheduled_test_run_completes_on_the_clock_and_a_read_never_moves_it() {
    let case = "the clock owes a scheduled run its completion";
    let emu = emulator();
    let policy_arn = policy(&emu, "clocked policy").await;
    let test_case_id = create_test_case(
        &emu,
        &policy_arn,
        &TestSpec {
            guard_content: "returns close after 30 days",
            query_content: None,
            expected_result: CHECK_RESULTS[0],
            confidence_threshold: None,
        },
    )
    .await;
    let build_workflow_id = completed_build_workflow(&emu, &policy_arn).await;

    let started = start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, None).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: the start must answer 200, got {} {:?}",
        started.status, started.body
    );

    let before = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await,
    );
    assert_eq!(
        text_at(case, &before, &["testRunStatus"]),
        SCHEDULED,
        "case {case:?}: a run the start has just scheduled has not run yet, got {before:?}"
    );

    tokio::time::sleep(PAST_THE_TEST_RUN).await;

    let settled = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await,
    );
    assert_eq!(
        text_at(case, &settled, &["testRunStatus"]),
        COMPLETED,
        "case {case:?}: the clock owes the run its completion, got {settled:?}"
    );
    let settled_at = read_instant(case, &settled);
    assert!(
        settled_at > read_instant(case, &before),
        "case {case:?}: the completion must move updatedAt past the instant the start \
         recorded, got {settled_at} after {}",
        read_instant(case, &before)
    );

    // A read reports; it must not move anything a later caller sees.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let again = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await,
    );
    assert_eq!(
        text_at(case, &again, &["testRunStatus"]),
        COMPLETED,
        "case {case:?}: a terminal run stays terminal, got {again:?}"
    );
    assert!(
        (read_instant(case, &again) - settled_at).abs() < f64::EPSILON,
        "case {case:?}: two reads after the span must report ONE instant, got {} then {}",
        settled_at,
        read_instant(case, &again)
    );
}

// ---------------------------------------------------------------------------------------
// 3. Tests run only against a COMPLETED build workflow.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn tests_run_only_against_a_completed_build_workflow() {
    /// Which state the build workflow is in when the test workflow is started.
    enum Reached {
        /// The build has only just been accepted, so it is still working.
        Scheduled,
        /// A cancel has taken the build over, so it is winding down.
        CancelRequested,
        /// The clock has made that cancellation due, so the build is terminal but built
        /// nothing.
        Cancelled,
        /// The clock has completed the build: the one state its tests may run against.
        Completed,
    }

    let rows: Vec<(&str, Reached)> = vec![
        ("a build still working", Reached::Scheduled),
        ("a build winding down", Reached::CancelRequested),
        ("a build that was cancelled", Reached::Cancelled),
        ("a build the clock completed", Reached::Completed),
    ];

    for (case, reached) in rows {
        let emu = emulator();
        let policy_arn = policy(&emu, "built policy").await;
        let test_case_id = create_test_case(
            &emu,
            &policy_arn,
            &TestSpec {
                guard_content: "returns close after 30 days",
                query_content: None,
                expected_result: CHECK_RESULTS[0],
                confidence_threshold: None,
            },
        )
        .await;
        let build_workflow_id = start_build_workflow(&emu, &policy_arn).await;
        let accepted = match reached {
            Reached::Scheduled => false,
            Reached::CancelRequested => {
                cancel_build_workflow(&emu, &policy_arn, &build_workflow_id).await;
                false
            }
            Reached::Cancelled => {
                cancel_build_workflow(&emu, &policy_arn, &build_workflow_id).await;
                tokio::time::sleep(PAST_THE_CANCELLATION).await;
                false
            }
            Reached::Completed => {
                age_build_workflow(&emu, &policy_arn, &build_workflow_id, PAST_THE_BUILD);
                true
            }
        };

        let reply = start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, None).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a COMPLETED build workflow's tests must run, got {} {:?}",
                reply.status, reply.body
            );
            let read =
                get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the accepted start must leave a result to read, got {} {:?}",
                read.status, read.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a build workflow that is not COMPLETED must be refused 400, got {} \
             {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            reply.error
        );
        let expected = format!("Build workflow: {build_workflow_id} is not in COMPLETED status");
        assert_eq!(
            reply.text("message"),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            reply.get("message")
        );
        let read = get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused start must schedule nothing, got {} {:?}",
            read.status, read.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A test workflow is addressed by its policy, its build workflow and its tests.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_start_naming_no_stored_policy_build_workflow_or_test_is_refused() {
    let emu = emulator();
    let policy_arn = policy(&emu, "addressed test policy").await;
    let other_arn = policy(&emu, "other test policy").await;
    let test_case_id = create_test_case(
        &emu,
        &policy_arn,
        &TestSpec {
            guard_content: "returns close after 30 days",
            query_content: None,
            expected_result: CHECK_RESULTS[0],
            confidence_threshold: None,
        },
    )
    .await;
    let other_test_case_id = create_test_case(
        &emu,
        &other_arn,
        &TestSpec {
            guard_content: "returns close after 60 days",
            query_content: None,
            expected_result: CHECK_RESULTS[1],
            confidence_threshold: None,
        },
    )
    .await;
    let build_workflow_id = completed_build_workflow(&emu, &policy_arn).await;

    // An ARN one `AutomatedReasoningPolicyArn` admits that nothing minted: the account and
    // region these suites sign for, with a policy id no create produced.
    let unknown_policy_arn = format!(
        "arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/zzzzzzzzzzzz"
    );

    /// (case, the policy the start addresses, the build workflow it names, the tests it
    /// names, the HTTP status and error code it must be refused with, and the message when
    /// the service's wording for that refusal is fixed).
    struct Row {
        case: &'static str,
        policy_arn: String,
        build_workflow_id: String,
        test_case_ids: Option<Vec<String>>,
        status: u16,
        error: &'static str,
        message: Option<String>,
    }

    let rows = vec![
        Row {
            case: "an ARN naming no policy of this account",
            policy_arn: unknown_policy_arn.clone(),
            build_workflow_id: build_workflow_id.clone(),
            test_case_ids: None,
            status: 404,
            error: "ResourceNotFoundException",
            message: None,
        },
        Row {
            case: "an id nothing started",
            policy_arn: policy_arn.clone(),
            build_workflow_id: UNSTARTED_WORKFLOW_ID.to_string(),
            test_case_ids: None,
            status: 404,
            error: "ResourceNotFoundException",
            message: Some(WORKFLOW_NOT_FOUND_MESSAGE.to_string()),
        },
        Row {
            case: "one policy's build workflow under another policy",
            policy_arn: other_arn.clone(),
            build_workflow_id: build_workflow_id.clone(),
            test_case_ids: None,
            status: 404,
            error: "ResourceNotFoundException",
            message: Some(WORKFLOW_NOT_FOUND_MESSAGE.to_string()),
        },
        Row {
            case: "a testCaseIds naming a test nothing minted",
            policy_arn: policy_arn.clone(),
            build_workflow_id: build_workflow_id.clone(),
            test_case_ids: Some(vec![UNMINTED_TEST_CASE_ID.to_string()]),
            status: 400,
            error: "ValidationException",
            message: Some(format!("Test case: {UNMINTED_TEST_CASE_ID} not found")),
        },
        Row {
            case: "a testCaseIds naming another policy's test",
            policy_arn: policy_arn.clone(),
            build_workflow_id: build_workflow_id.clone(),
            test_case_ids: Some(vec![other_test_case_id.clone()]),
            status: 400,
            error: "ValidationException",
            message: Some(format!("Test case: {other_test_case_id} not found")),
        },
    ];

    for row in rows {
        let case = row.case;
        let named: Option<Vec<&str>> = row
            .test_case_ids
            .as_ref()
            .map(|ids| ids.iter().map(String::as_str).collect());
        let refused = start_test_workflow(
            &emu,
            &row.policy_arn,
            &row.build_workflow_id,
            named.as_deref(),
            None,
        )
        .await;
        assert_eq!(
            refused.status, row.status,
            "case {case:?}: expected {}, got {} {:?}",
            row.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some(row.error),
            "case {case:?}: expected {}, got {:?}",
            row.error,
            refused.error
        );
        if let Some(message) = row.message {
            assert_eq!(
                refused.text("message"),
                message,
                "case {case:?}: expected the message {message:?}, got {:?}",
                refused.get("message")
            );
        }
    }

    // A policy holding no test at all has nothing a test workflow could run.
    let empty_arn = policy(&emu, "untested policy").await;
    let empty_build_workflow_id = completed_build_workflow(&emu, &empty_arn).await;
    let refused =
        start_test_workflow(&emu, &empty_arn, &empty_build_workflow_id, None, None).await;
    assert_eq!(
        refused.status, 400,
        "case a policy with no test: expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "case a policy with no test: expected ValidationException, got {:?}",
        refused.error
    );
    let expected = format!("Policy: {empty_arn} has no test cases to run");
    assert_eq!(
        refused.text("message"),
        expected,
        "case a policy with no test: expected the message {expected:?}, got {:?}",
        refused.get("message")
    );

    // A result nothing has run reads back as nothing, and the pair the start would accept
    // still works — so the rows above refused on what they addressed, not on the store.
    let unread = get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await;
    assert_eq!(
        unread.status, 404,
        "case a result nothing ran: expected 404, got {} {:?}",
        unread.status, unread.body
    );
    let accepted = start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, None).await;
    assert_eq!(
        accepted.status, 200,
        "case the pair the rows refused around: expected 200, got {} {:?}",
        accepted.status, accepted.body
    );
    let read = get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await;
    assert_eq!(
        read.status, 200,
        "case the pair the rows refused around: the accepted start must leave a result, got \
         {} {:?}",
        read.status, read.body
    );
    // A test of one policy names no result of another, and neither does a workflow id
    // nothing started.
    for (case, arn, workflow, test) in [
        (
            "another policy's result",
            other_arn.as_str(),
            build_workflow_id.as_str(),
            test_case_id.as_str(),
        ),
        (
            "an id nothing started",
            policy_arn.as_str(),
            UNSTARTED_WORKFLOW_ID,
            test_case_id.as_str(),
        ),
        (
            "a test nothing minted",
            policy_arn.as_str(),
            build_workflow_id.as_str(),
            UNMINTED_TEST_CASE_ID,
        ),
    ] {
        let read = get_test_result(&emu, arn, workflow, test).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a read of a triple nothing holds must answer 404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            read.error
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. One test workflow at a time per build workflow.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_build_workflow_whose_tests_are_running_refuses_another_test_workflow() {
    let case = "one test workflow at a time";
    let emu = emulator();
    let policy_arn = policy(&emu, "busy policy").await;
    let mut ids = BTreeSet::new();
    for spec in [
        TestSpec {
            guard_content: "returns close after 30 days",
            query_content: None,
            expected_result: CHECK_RESULTS[0],
            confidence_threshold: None,
        },
        TestSpec {
            guard_content: "returns close after 60 days",
            query_content: None,
            expected_result: CHECK_RESULTS[1],
            confidence_threshold: None,
        },
    ] {
        ids.insert(create_test_case(&emu, &policy_arn, &spec).await);
    }
    let build_workflow_id = completed_build_workflow(&emu, &policy_arn).await;

    let first = start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, None).await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: the first start must answer 200, got {} {:?}",
        first.status, first.body
    );

    // Every test of this build workflow is still running, so a second test workflow has
    // nothing to take over.
    let second = start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, None).await;
    assert_eq!(
        second.status, 400,
        "case {case:?}: a build workflow whose tests are running must refuse another test \
         workflow, got {} {:?}",
        second.status, second.body
    );
    assert_eq!(
        second.error.as_deref(),
        Some("ResourceInUseException"),
        "case {case:?}: expected ResourceInUseException, got {:?}",
        second.error
    );
    // The refusal is Amazon Bedrock's own fixed sentence: it names neither the build
    // workflow nor a status, so a caller keying on it backs off without being coupled to
    // either.
    let expected = "Testing workflow is currently in progress";
    assert_eq!(
        second.text("message"),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        second.get("message")
    );

    // Once every run is terminal the build workflow is free again, and the start re-runs
    // those same tests rather than adding a second result for them.
    tokio::time::sleep(PAST_THE_TEST_RUN).await;
    let first_test_case_id = ids.iter().next().expect("the policy holds tests").clone();
    let completed = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &first_test_case_id).await,
    );
    let completed_at = read_instant(case, &completed);

    let third = start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, None).await;
    assert_eq!(
        third.status, 200,
        "case {case:?}: a build workflow whose runs are terminal must accept another test \
         workflow, got {} {:?}",
        third.status, third.body
    );
    let rerun = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &first_test_case_id).await,
    );
    assert_eq!(
        text_at(case, &rerun, &["testRunStatus"]),
        SCHEDULED,
        "case {case:?}: the re-run must schedule the test again, got {rerun:?}"
    );
    assert!(
        read_instant(case, &rerun) > completed_at,
        "case {case:?}: the re-run must move updatedAt past the run it replaced, got {} after \
         {completed_at}",
        read_instant(case, &rerun)
    );
}

// ---------------------------------------------------------------------------------------
// 6. A replayed clientRequestToken schedules nothing new.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_schedules_nothing_new() {
    let case = "a replayed clientRequestToken";
    let emu = emulator();
    let policy_arn = policy(&emu, "idempotent policy").await;
    let test_case_id = create_test_case(
        &emu,
        &policy_arn,
        &TestSpec {
            guard_content: "returns close after 30 days",
            query_content: None,
            expected_result: CHECK_RESULTS[0],
            confidence_threshold: None,
        },
    )
    .await;
    let build_workflow_id = completed_build_workflow(&emu, &policy_arn).await;

    let token = "one-test-workflow";
    let first =
        start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, Some(token)).await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: the first start must answer 200, got {} {:?}",
        first.status, first.body
    );

    // A replay while the run is still running is NOT the in-use refusal a fresh request
    // would get: the token says this request is the one already accepted.
    let replayed =
        start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, Some(token)).await;
    assert_eq!(
        replayed.status, 200,
        "case {case:?}: a replay must answer 200, got {} {:?}",
        replayed.status, replayed.body
    );
    assert_eq!(
        replayed.text("policyArn"),
        policy_arn,
        "case {case:?}: a replay must answer the policy the first start ran the tests of, got \
         {:?}",
        replayed.get("policyArn")
    );

    // The SAME token with a different selection is a different request, so it is not the
    // one already accepted: it is held to the in-use check like any other fresh request.
    let narrowed = start_test_workflow(
        &emu,
        &policy_arn,
        &build_workflow_id,
        Some(&[test_case_id.as_str()]),
        Some(token),
    )
    .await;
    assert_eq!(
        narrowed.status, 400,
        "case {case:?}: the same token naming other tests asks for other work, so it must \
         not be answered as already done, got {} {:?}",
        narrowed.status, narrowed.body
    );
    assert_eq!(
        narrowed.error.as_deref(),
        Some("ResourceInUseException"),
        "case {case:?}: expected ResourceInUseException, got {:?}",
        narrowed.error
    );

    tokio::time::sleep(PAST_THE_TEST_RUN).await;
    let completed = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await,
    );
    assert_eq!(
        text_at(case, &completed, &["testRunStatus"]),
        COMPLETED,
        "case {case:?}: the clock owes the first run its completion, got {completed:?}"
    );
    let completed_at = read_instant(case, &completed);

    // A replay of the same token after the run finished re-runs nothing either.
    let again =
        start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, Some(token)).await;
    assert_eq!(
        again.status, 200,
        "case {case:?}: a replay must answer 200, got {} {:?}",
        again.status, again.body
    );
    let unchanged = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await,
    );
    assert_eq!(
        text_at(case, &unchanged, &["testRunStatus"]),
        COMPLETED,
        "case {case:?}: a replay must schedule nothing new, got {unchanged:?}"
    );
    assert!(
        (read_instant(case, &unchanged) - completed_at).abs() < f64::EPSILON,
        "case {case:?}: a replay must leave the run it replays where it was, got {} then {}",
        completed_at,
        read_instant(case, &unchanged)
    );

    // A FRESH token is another request, so it runs the tests again.
    let fresh =
        start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, Some("another-token"))
            .await;
    assert_eq!(
        fresh.status, 200,
        "case {case:?}: a fresh token must be accepted, got {} {:?}",
        fresh.status, fresh.body
    );
    let rerun = test_result(
        case,
        &get_test_result(&emu, &policy_arn, &build_workflow_id, &test_case_id).await,
    );
    assert_eq!(
        text_at(case, &rerun, &["testRunStatus"]),
        SCHEDULED,
        "case {case:?}: a fresh token must run the tests again, got {rerun:?}"
    );
    assert!(
        read_instant(case, &rerun) > completed_at,
        "case {case:?}: the re-run must move updatedAt past the run it replaced, got {} after \
         {completed_at}",
        read_instant(case, &rerun)
    );

    // The token that fresh one superseded is no longer a replay, so it is held to the same
    // check every other request is: this build workflow's runs are running again.
    let superseded =
        start_test_workflow(&emu, &policy_arn, &build_workflow_id, None, Some(token)).await;
    assert_eq!(
        superseded.status, 400,
        "case {case:?}: a superseded token is a request of its own, so it must be refused \
         while the runs are running, got {} {:?}",
        superseded.status, superseded.body
    );
    assert_eq!(
        superseded.error.as_deref(),
        Some("ResourceInUseException"),
        "case {case:?}: expected ResourceInUseException, got {:?}",
        superseded.error
    );
}
