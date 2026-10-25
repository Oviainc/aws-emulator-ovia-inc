// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelQuery`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin, over a SET of queries rather than one: a cancel
//! RECORDS the cancellation on a query that has not reached a terminal status and
//! leaves it reporting the cancel in flight and then cancelled, as the clock makes
//! each due; a repeat cancel of such a query is a no-op reporting the status it
//! stands in, because the operation is modelled idempotent; a query already in the
//! terminal status its run earned cannot be cancelled and is the modelled conflict;
//! and a workspace or query this service does not hold is the modelled not-found
//! refusal.
//!
//! The HTTP harness (`call`, `router`, `error_code`, …) and the query helpers
//! (`fixture`, `start`, `described`, …) are the ones the asset and workspace-query
//! suites already define; this suite reuses them rather than growing a second.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    query_key, service_state, QueryOutcome, QueryRecord, QUERY_STEP,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine, REGION};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::workspace_queries::{
    create_workspace, described, fixture, query_uri, settled, start, WORKSPACE,
};

/// A second workspace, so a cancel can be shown to reach a query only inside the
/// workspace it was started in.
const OTHER_WORKSPACE: &str = "Plant2";

/// The statement the seeded queries below carry. What a cancel does cannot depend on
/// it — the suite that covers what a run produces is next door.
const STATEMENT: &str = "SELECT asset_name FROM asset";

/// `POST /workspaces/{workspaceName}/queries/{queryId}/cancel`, as `OPERATIONS.md`
/// reports the route.
fn cancel_uri(workspace: &str, query_id: &str) -> String {
    format!("/workspaces/{workspace}/queries/{query_id}/cancel")
}

/// `CancelQuery` on a query of `workspace`.
async fn cancel(router: &MultiServiceRouter, workspace: &str, query_id: &str) -> (u16, Value) {
    call(router, "POST", &cancel_uri(workspace, query_id), None).await
}

/// `CancelQuery` in [`WORKSPACE`], asserting it was accepted, and the `status` it
/// reported for the query.
async fn cancelled(router: &MultiServiceRouter, query_id: &str) -> String {
    let (status, body) = cancel(router, WORKSPACE, query_id).await;
    assert_eq!(
        status, 202,
        "cancel {query_id}: expected 202, got {status} {body}"
    );
    assert_eq!(
        body["queryId"].as_str(),
        Some(query_id),
        "cancel {query_id}: the response names the query it cancelled, got {body}"
    );
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("cancel {query_id}: no status in {body}"))
        .to_string()
}

/// Put one query straight into the store, so the suite can address a query standing
/// in EVERY status the lifecycle admits — including the ones a test cannot wait a
/// run into.
fn seed_query(
    engine: &engine::InMemoryEngine,
    workspace: &str,
    query_id: &str,
    submitted_at: SystemTime,
    cancel_requested_at: Option<SystemTime>,
    outcome: QueryOutcome,
) {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    store.queries.insert(
        query_key(workspace, query_id),
        QueryRecord {
            query_id: query_id.to_string(),
            workspace_name: workspace.to_string(),
            query_statement: STATEMENT.to_string(),
            client_token: None,
            submitted_at,
            cancel_requested_at,
            outcome,
        },
    );
}

/// The instant a cancel was recorded on a stored query, as the store holds it — the
/// only way to observe that a request recorded nothing.
fn cancel_recorded_at(
    engine: &engine::InMemoryEngine,
    workspace: &str,
    query_id: &str,
) -> Option<SystemTime> {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .queries
        .get(&query_key(workspace, query_id))
        .unwrap_or_else(|| panic!("the store holds {query_id}"))
        .cancel_requested_at
}

/// The statistics a run answers, so a seeded COMPLETED query carries the outcome a
/// real run would have left on it.
fn completed_run() -> QueryOutcome {
    QueryOutcome::Completed {
        row_count: 2,
        bytes_scanned: 64,
        execution_time_in_millis: 1,
    }
}

/// A run that did not finish, with the message it left behind.
fn failed_run() -> QueryOutcome {
    QueryOutcome::Failed {
        error_message: "the run did not finish inside the engine's budget".to_string(),
    }
}

#[tokio::test]
async fn a_cancel_takes_a_query_that_is_still_going_through_canceling_to_canceled() {
    let router = router();
    fixture(&router).await;

    // Rows differ along what the rule generalises over: the statement the query was
    // started with — each a different view, predicate, ordering and limit, so what a
    // cancel does cannot depend on the run it interrupted. None of them is terminal
    // when the cancel arrives, so every row must be cancelled rather than refused.
    // A cancel of a query that has got as far as RUNNING is pinned deterministically
    // by the seeded status table below, which needs no wall clock at all.
    let cases: Vec<(&str, &str)> = vec![
        ("a projection", "SELECT asset_name FROM asset"),
        (
            "a predicate that keeps one row",
            "SELECT * FROM asset WHERE asset_name = 'Boiler1'",
        ),
        (
            "a predicate no row satisfies",
            "SELECT asset_id FROM asset WHERE asset_name = 'Chiller9'",
        ),
        ("every stored sample", "SELECT * FROM raw_time_series"),
        (
            "an ordering with a limit",
            "SELECT asset_name FROM asset ORDER BY asset_name DESC LIMIT 1",
        ),
    ];

    let mut cancelling: Vec<(&str, String)> = Vec::new();
    for (case, statement) in &cases {
        let query_id = start(&router, statement).await;
        // The cancel follows the start immediately, so the query is still going when
        // it arrives — which is exactly what an answer of CANCELING states, since a
        // query that had reached a terminal status would have been refused.
        let status = cancelled(&router, &query_id).await;
        assert_eq!(
            status, "CANCELING",
            "case {case:?}: a recorded cancel leaves the query cancelling, got {status}"
        );

        let read = described(&router, &query_id).await;
        assert_eq!(
            read["status"].as_str(),
            Some("CANCELING"),
            "case {case:?}: a read agrees with the cancel's own answer, got {read}"
        );
        assert!(
            read["completedAt"].is_null(),
            "case {case:?}: a cancel in flight has reached no terminal state, got {read}"
        );
        assert!(
            read["statistics"].is_null(),
            "case {case:?}: statistics are a COMPLETED query's, got {read}"
        );
        assert!(
            read["errorMessage"].is_null(),
            "case {case:?}: a cancelled query carries no errorMessage, got {read}"
        );
        cancelling.push((case, query_id));
    }

    // The span the clock owes the cancellation is NON-ZERO, which is what the
    // CANCELING reads above observed; once it has elapsed every one of them is
    // cancelled, and stays cancelled however often it is read or cancelled again.
    tokio::time::sleep(settled()).await;
    for (case, query_id) in &cancelling {
        let read = described(&router, query_id).await;
        assert_eq!(
            read["status"].as_str(),
            Some("CANCELED"),
            "case {case:?}: the cancellation falls due, got {read}"
        );
        assert!(
            !read["completedAt"].is_null(),
            "case {case:?}: a CANCELED query reached a terminal state, got {read}"
        );
        assert!(
            read["statistics"].is_null(),
            "case {case:?}: a cancelled query reports no statistics, got {read}"
        );
        assert!(
            read["errorMessage"].is_null(),
            "case {case:?}: a cancelled query reports no errorMessage, got {read}"
        );

        // Idempotent: cancelling an already cancelled query reports the status it
        // stands in and moves nothing.
        let again = cancelled(&router, query_id).await;
        assert_eq!(
            again, "CANCELED",
            "case {case:?}: a repeat cancel reports the current status, got {again}"
        );
        assert_eq!(
            read,
            described(&router, query_id).await,
            "case {case:?}: a repeat cancel changes nothing about the query"
        );
    }
}

/// One row of the status table: the status the stored query stands in and what a
/// cancel of a query in that status must answer.
#[derive(Debug)]
struct StatusCase {
    /// The case name, which is also the `queryId` the row is seeded under.
    label: &'static str,
    /// How long ago the query was submitted.
    submitted_ago: Duration,
    /// How long ago a cancel was recorded on it, if one was.
    cancel_ago: Option<Duration>,
    /// What running its statement produced.
    outcome: QueryOutcome,
    /// The status a read of it reports before the cancel.
    status: &'static str,
    /// The status a cancel answers with, or `None` when the cancel is refused.
    cancelled_to: Option<&'static str>,
}

#[tokio::test]
async fn a_cancel_is_accepted_or_refused_by_the_status_the_query_stands_in() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;
    let now = SystemTime::now();

    // Rows span EVERY status of the lifecycle: the two a query is still going
    // through, the two terminal ones its run earns, and the two a cancel itself
    // leaves behind. Source of the catalogue: `smithy_query(query="shape",
    // shape="QueryStatus")` — CANCELED | CANCELING | COMPLETED | FAILED | RUNNING |
    // SUBMITTED.
    let cases: Vec<StatusCase> = vec![
        StatusCase {
            label: "submitted",
            submitted_ago: Duration::ZERO,
            cancel_ago: None,
            outcome: completed_run(),
            status: "SUBMITTED",
            cancelled_to: Some("CANCELING"),
        },
        StatusCase {
            label: "running",
            submitted_ago: QUERY_STEP,
            cancel_ago: None,
            outcome: completed_run(),
            status: "RUNNING",
            cancelled_to: Some("CANCELING"),
        },
        StatusCase {
            label: "completed",
            submitted_ago: 3 * QUERY_STEP,
            cancel_ago: None,
            outcome: completed_run(),
            status: "COMPLETED",
            cancelled_to: None,
        },
        StatusCase {
            label: "failed",
            submitted_ago: 3 * QUERY_STEP,
            cancel_ago: None,
            outcome: failed_run(),
            status: "FAILED",
            cancelled_to: None,
        },
        StatusCase {
            label: "canceling",
            submitted_ago: QUERY_STEP,
            cancel_ago: Some(Duration::ZERO),
            outcome: completed_run(),
            status: "CANCELING",
            cancelled_to: Some("CANCELING"),
        },
        StatusCase {
            label: "canceled",
            submitted_ago: 3 * QUERY_STEP,
            cancel_ago: Some(2 * QUERY_STEP),
            outcome: completed_run(),
            status: "CANCELED",
            cancelled_to: Some("CANCELED"),
        },
    ];

    for case in &cases {
        let recorded_at = case.cancel_ago.map(|ago| now - ago);
        seed_query(
            &engine,
            WORKSPACE,
            case.label,
            now - case.submitted_ago,
            recorded_at,
            case.outcome.clone(),
        );
        let before = described(&router, case.label).await;
        assert_eq!(
            before["status"].as_str(),
            Some(case.status),
            "case {:?}: the row stands in {}, got {before}",
            case.label,
            case.status
        );

        let (status, body) = cancel(&router, WORKSPACE, case.label).await;
        match case.cancelled_to {
            Some(expected) => {
                assert_eq!(
                    status, 202,
                    "case {:?}: a cancellable query is accepted, got {status} {body}",
                    case.label
                );
                assert_eq!(
                    body["status"].as_str(),
                    Some(expected),
                    "case {:?}: expected {expected}, got {body}",
                    case.label
                );
                assert_eq!(
                    body["queryId"].as_str(),
                    Some(case.label),
                    "case {:?}: the response names the query it addressed, got {body}",
                    case.label
                );
                match recorded_at {
                    // A query a cancel was already recorded on is not cancelled a
                    // second time: the recorded instant is exactly the one it
                    // already carried, so the terminal status it is on its way to
                    // cannot be pushed back by a retry.
                    Some(_) => assert_eq!(
                        cancel_recorded_at(&engine, WORKSPACE, case.label),
                        recorded_at,
                        "case {:?}: a repeat cancel records no second cancellation",
                        case.label
                    ),
                    None => assert!(
                        cancel_recorded_at(&engine, WORKSPACE, case.label).is_some(),
                        "case {:?}: the cancellation is recorded on the stored query",
                        case.label
                    ),
                }
            }
            None => {
                // A terminal status its run earned: the cancel cannot apply, which
                // this service answers as the modelled conflict rather than a
                // success that cancelled nothing.
                assert_eq!(
                    status, 409,
                    "case {:?}: a terminal query cannot be cancelled, got {status} {body}",
                    case.label
                );
                assert_eq!(
                    error_code(&body),
                    "ConflictingOperationException",
                    "case {:?}: unexpected error code in {body}",
                    case.label
                );
                assert_eq!(
                    error_message(&body),
                    format!(
                        "The query {} is in the {} status; only a query that has not \
                         reached a terminal status can be cancelled.",
                        case.label, case.status
                    ),
                    "case {:?}: unexpected message in {body}",
                    case.label
                );
                assert_eq!(
                    body["resourceId"].as_str(),
                    Some(case.label),
                    "case {:?}: the conflict names the query that conflicts, got {body}",
                    case.label
                );
                assert_eq!(
                    body["resourceArn"].as_str(),
                    Some(
                        format!(
                            "arn:aws:iotsitewise:{REGION}:{}:query/{WORKSPACE}/{}",
                            engine::EMULATOR_ACCOUNT_ID, case.label
                        )
                        .as_str()
                    ),
                    "case {:?}: the conflict names the query's ARN, got {body}",
                    case.label
                );
                assert_eq!(
                    cancel_recorded_at(&engine, WORKSPACE, case.label),
                    None,
                    "case {:?}: a refused cancel records nothing",
                    case.label
                );
            }
        }

        // Whatever the answer was, a read of the query agrees with it.
        let after = described(&router, case.label).await;
        let expected = case.cancelled_to.unwrap_or(case.status);
        assert_eq!(
            after["status"].as_str(),
            Some(expected),
            "case {:?}: a read after the cancel reports {expected}, got {after}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_cancelled_query_reports_neither_statistics_nor_an_error_message() {
    let (router, engine) = router_with_engine();
    fixture(&router).await;
    let now = SystemTime::now();

    // Both rows are cancelled before the status their RUN earned falls due, so what
    // the run produced is never what the query reports: the cancellation is the
    // terminal state it reaches.
    let cases: Vec<(&str, QueryOutcome)> = vec![
        ("a-run-that-answered-rows", completed_run()),
        ("a-run-that-did-not-finish", failed_run()),
    ];

    for (query_id, outcome) in &cases {
        seed_query(&engine, WORKSPACE, query_id, now, None, outcome.clone());
        let status = cancelled(&router, query_id).await;
        assert_eq!(
            status, "CANCELING",
            "case {query_id:?}: a cancel of a query that is still going is recorded"
        );
    }

    tokio::time::sleep(settled()).await;
    for (query_id, _) in &cases {
        let read = described(&router, query_id).await;
        assert_eq!(
            read["status"].as_str(),
            Some("CANCELED"),
            "case {query_id:?}: the cancellation is the terminal status, got {read}"
        );
        assert!(
            read["statistics"].is_null(),
            "case {query_id:?}: statistics are a COMPLETED query's, got {read}"
        );
        assert!(
            read["errorMessage"].is_null(),
            "case {query_id:?}: an errorMessage is a FAILED query's, got {read}"
        );
    }
}

#[tokio::test]
async fn a_cancel_reaches_a_query_only_inside_the_workspace_it_was_started_in() {
    let router = router();
    fixture(&router).await;
    create_workspace(&router, OTHER_WORKSPACE).await;
    let query_id = start(&router, STATEMENT).await;

    // Rows span the ways a cancel addresses no query of this service: a workspace it
    // does not hold (including a name differing from a held one only in case, since
    // a workspace is named exactly), an id no query carries, and the id of a query
    // of ANOTHER workspace.
    let unknown = format!("{query_id}-other");
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "a workspace this service does not hold",
            "Plant9".to_string(),
            query_id.clone(),
            "Workspace with ID Plant9 not found".to_string(),
        ),
        (
            "a workspace whose case differs",
            "plant1".to_string(),
            query_id.clone(),
            "Workspace with ID plant1 not found".to_string(),
        ),
        (
            "an id no query carries",
            WORKSPACE.to_string(),
            unknown.clone(),
            format!("Query with ID {unknown} not found"),
        ),
        (
            "a query of another workspace",
            OTHER_WORKSPACE.to_string(),
            query_id.clone(),
            format!("Query with ID {query_id} not found"),
        ),
    ];

    for (case, workspace, addressed, expected) in &cases {
        let (status, body) = cancel(&router, workspace, addressed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            expected.as_str(),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // None of those refusals touched the query they named: it is still the one
    // `StartQuery` started, in the status the clock owes it.
    let read = described(&router, &query_id).await;
    assert!(
        matches!(
            read["status"].as_str(),
            Some("SUBMITTED") | Some("RUNNING") | Some("COMPLETED")
        ),
        "a refused cancel cancels nothing, got {read}"
    );
    assert_eq!(
        call(&router, "GET", &query_uri(WORKSPACE, &query_id), None)
            .await
            .0,
        200,
        "the query the refusals named is still readable"
    );
}
