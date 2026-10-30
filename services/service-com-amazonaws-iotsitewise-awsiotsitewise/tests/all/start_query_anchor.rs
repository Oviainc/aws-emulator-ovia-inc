// SPDX-License-Identifier: CC0-1.0

//! What this suite pins, over a SET of retries rather than one recorded request:
//! `StartQuery` answers a retry carrying an already-spent `clientToken` with the
//! query that token's own call started, at the status that query's lifecycle makes
//! due at the ONE instant the retry's request was anchored at — whichever of the
//! handler's two token checks catches it.
//!
//! A query's status is a pure function of the clock (`SUBMITTED` for one step, then
//! `RUNNING`, then the terminal status its run earned), and the handler resolves the
//! token twice: once before the statement runs and once after the write lock is
//! re-taken, with the statement run in between. Two readings of the clock around
//! that run let one request report two different statuses for one query, so the two
//! answers for the same retry would depend on which branch happened to catch it.
//!
//! The two suites below drive the two branches:
//!
//! 1. SEQUENTIALLY, over a table of query ages: the branch before the run answers,
//!    and the status it reports is the one due at the retry's anchor — within the
//!    window the test can bound that anchor to, since the anchor is read inside the
//!    handler. A later read of the same query never reports an earlier status.
//! 2. SIMULTANEOUSLY, over a table of racer counts and held run durations: several
//!    identical retries are in flight before any of them has recorded the query, so
//!    every loser re-locks AFTER its statement has run and is answered by the second
//!    branch. The run is held for a known span (the engine interprets a statement on
//!    a blocking worker, and this suite's runtime has exactly one, so a blocking task
//!    the test holds delays every racer's run by as long as it likes), which is what
//!    puts a measured distance between a request's anchor and its post-run check.
//!    Every racer still reports the one query, at the status due at its own anchor.
//!
//! The HTTP harness (`call`, `router_with_engine`), the query helpers (`start_in`,
//! `create_workspace`, `queries_held`, `query_uri`) and the `race` helper are the
//! ones the sibling suites already define; this suite reuses them rather than
//! growing a second copy.

use std::time::{Duration, Instant};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::QUERY_STEP;

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::simultaneous_writes::race;
use crate::workspace_queries::{create_workspace, queries_held, query_uri, start_in};

/// The workspace every query below is started in.
const WORKSPACE: &str = "AnchorPlant";

/// The statement every retry repeats. One statement the engine runs over the shared
/// store, so what is varied is WHEN the retry is anchored and WHICH branch answers
/// it, not what the query does.
const STATEMENT: &str = "SELECT asset_name FROM asset";

/// A `clientToken` for row `row`: `ClientToken` is modelled len 36..64, pattern
/// `^\S{36,64}$`, so each is a 36-character token differing only in its tail.
fn token(row: usize) -> String {
    format!("44444444-4444-4444-8444-4444444444{row:02}")
}

/// The `status` a `StartQuery` answer reports.
fn status_of(body: &Value) -> &str {
    body["status"].as_str().unwrap_or_default()
}

/// The `queryId` a `StartQuery` answer reports.
fn query_id_of(body: &Value) -> &str {
    body["queryId"].as_str().unwrap_or_default()
}

/// One row of the sequential table: how long after the start the retry is sent.
#[derive(Debug)]
struct AgeCase {
    label: &'static str,
    /// How long the row waits between the start and the retry.
    age: Duration,
}

/// The rows: one inside each span the query's lifecycle has — the first step, the
/// second, and past both of them — so the rule is asserted over a retry of every age
/// a query has, not only over a fresh one.
fn age_cases() -> Vec<AgeCase> {
    vec![
        AgeCase {
            label: "a retry inside the first step",
            age: Duration::ZERO,
        },
        AgeCase {
            label: "a retry inside the second step",
            age: QUERY_STEP + QUERY_STEP / 2,
        },
        AgeCase {
            label: "a retry past the whole span",
            age: 2 * QUERY_STEP + Duration::from_millis(250),
        },
    ]
}

/// The status a query nothing cancelled, whose run answered rows, reports to a
/// request anchored `age` after it was submitted: `SUBMITTED` inside the first
/// [`QUERY_STEP`], `RUNNING` inside the second, and the terminal status its run
/// earned from then on.
///
/// Source for the values: `smithy_query(query="shape", shape="QueryStatus")` —
/// SUBMITTED | RUNNING | COMPLETED | FAILED | CANCELING | CANCELED, of which these
/// three are the ones such a query walks.
fn status_due_at(age: Duration) -> &'static str {
    if age < QUERY_STEP {
        "SUBMITTED"
    } else if age < 2 * QUERY_STEP {
        "RUNNING"
    } else {
        "COMPLETED"
    }
}

/// Every status a request whose anchor fell somewhere between `earliest` and
/// `latest` after the query was submitted may report, in lifecycle order.
///
/// The test can only BOUND where a request's anchor fell — it is read inside the
/// handler, between the instant the test sent the request and the instant the answer
/// came back — so the expectation is the status due anywhere in that measured
/// window. Ordinarily the window is a couple of milliseconds wide and names exactly
/// one status; it widens only when the machine is loaded, which is the one case a
/// fixed expectation would fail for reasons that are not this rule.
fn statuses_due_between(earliest: Duration, latest: Duration) -> Vec<&'static str> {
    let mut due = vec![status_due_at(earliest)];
    for boundary in [QUERY_STEP, 2 * QUERY_STEP] {
        if earliest < boundary && boundary <= latest {
            due.push(status_due_at(boundary));
        }
    }
    due.dedup();
    due
}

/// How far through its lifecycle a status is, so a later read can be asserted never
/// to report an earlier one.
fn lifecycle_rank(status: &str) -> usize {
    match status {
        "SUBMITTED" => 0,
        "RUNNING" => 1,
        _ => 2,
    }
}

/// `StartQuery` in [`WORKSPACE`] under `token`, asserting it was accepted, and the
/// body it answered with.
async fn start_under(router: &MultiServiceRouter, case: &str, token: &str) -> Value {
    let (status, body) = start_in(router, WORKSPACE, STATEMENT, Some(token)).await;
    assert_eq!(
        status, 202,
        "case {case:?}: StartQuery expected 202, got {status} {body}"
    );
    body
}

#[tokio::test]
async fn a_retry_reports_the_status_due_at_the_instant_its_request_was_anchored() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;

    for (row, case) in age_cases().iter().enumerate() {
        let token = token(row);
        let held_before = queries_held(&engine);

        // The instants around each call BOUND where that request's anchor fell: it
        // is read inside the handler, so it lies between the two.
        let sent = Instant::now();
        let started = start_under(&router, case.label, &token).await;
        let answered = Instant::now();
        let query_id = query_id_of(&started).to_string();
        assert_eq!(
            status_of(&started),
            "SUBMITTED",
            "case {:?}: a started query reports SUBMITTED, got {started}",
            case.label
        );

        tokio::time::sleep(case.age).await;
        let retry_sent = Instant::now();
        let (status, replayed) = start_in(&router, WORKSPACE, STATEMENT, Some(&token)).await;
        let retry_answered = Instant::now();
        assert_eq!(
            status, 202,
            "case {:?}: a retry expected 202, got {status} {replayed}",
            case.label
        );
        assert_eq!(
            query_id_of(&replayed),
            query_id,
            "case {:?}: a retry must report the query its token already started, \
             expected {query_id:?}, got {replayed}",
            case.label
        );
        // The retry's anchor fell no less than `retry_sent - answered` and no more
        // than `retry_answered - sent` after the query was submitted, so the status
        // it reports is the one due somewhere in that window.
        let due = statuses_due_between(
            retry_sent.saturating_duration_since(answered),
            retry_answered.saturating_duration_since(sent),
        );
        assert!(
            due.contains(&status_of(&replayed)),
            "case {:?}: expected the status due at the retry's own anchor (one of {due:?}), \
             got {replayed}",
            case.label
        );
        // And the reader of that query agrees with the retry: both derive the one
        // status the clock has made due, so a read taken after the retry reports the
        // same status or a later one in the lifecycle, never an earlier one.
        let (read, described) = call(&router, "GET", &query_uri(WORKSPACE, &query_id), None).await;
        let read_answered = Instant::now();
        assert_eq!(
            read, 200,
            "case {:?}: DescribeQuery got {read} {described}",
            case.label
        );
        let reported = described["status"].as_str().unwrap_or_default();
        let due_to_reader = statuses_due_between(
            retry_sent.saturating_duration_since(answered),
            read_answered.saturating_duration_since(sent),
        );
        assert!(
            due_to_reader.contains(&reported),
            "case {:?}: the reader must report the status due at its own instant \
             (one of {due_to_reader:?}), got {described}",
            case.label
        );
        assert!(
            lifecycle_rank(reported) >= lifecycle_rank(status_of(&replayed)),
            "case {:?}: the later read must not report an earlier status than the retry \
             ({:?}), got {reported:?}",
            case.label,
            status_of(&replayed)
        );
        assert_eq!(
            queries_held(&engine),
            held_before + 1,
            "case {:?}: the retry must start no second query",
            case.label
        );
    }
}

/// One row of the simultaneous table: how many identical retries are in flight at
/// once, and how long their statement runs are HELD, measured from the instant they
/// were fired.
#[derive(Debug)]
struct RaceCase {
    label: &'static str,
    racers: usize,
    /// How long the one blocking worker is kept busy after the racers are fired, so
    /// every racer's run — and so its post-run token check — lands that much later
    /// than the instant its request was anchored at.
    held_for: Duration,
}

/// The rows: the smallest race there is and two larger ones, each holding the run
/// for a span that carries the clock a different distance past the query's
/// submission — within the first step, past it, and past the whole lifecycle — since
/// that distance is what a second reading of the clock would report instead of the
/// anchor.
fn race_cases() -> Vec<RaceCase> {
    vec![
        RaceCase {
            label: "two retries, the run not held",
            racers: 2,
            held_for: Duration::ZERO,
        },
        RaceCase {
            label: "two retries, the run held past the first step",
            racers: 2,
            held_for: QUERY_STEP + Duration::from_millis(100),
        },
        RaceCase {
            label: "three retries, the run held past the whole span",
            racers: 3,
            held_for: 2 * QUERY_STEP + Duration::from_millis(200),
        },
        RaceCase {
            label: "five retries, the run held past the whole span",
            racers: 5,
            held_for: 2 * QUERY_STEP + Duration::from_millis(200),
        },
    ]
}

/// How long the test waits for the blocking task it spawned to have TAKEN the one
/// blocking worker this suite's runtime has, before it fires the racers.
const LEAD: Duration = Duration::from_millis(100);

#[test]
fn simultaneous_retries_of_one_call_are_answered_from_one_anchor() {
    // Built here rather than by `#[tokio::test]` because the suite needs a runtime
    // with exactly ONE blocking worker: the query engine interprets a statement on a
    // blocking worker, so a blocking task the test holds is what keeps every racer's
    // run — and its post-run token check — a known span behind its anchor.
    let runtime = ::tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("a test runtime");

    runtime.block_on(async {
        let (router, engine) = router_with_engine();
        create_workspace(&router, WORKSPACE).await;

        for (row, case) in race_cases().iter().enumerate() {
            // Each row's token is its own, so each row is one call retried, never a
            // retry of the row before it.
            let token = token(row + age_cases().len());
            let held_before = queries_held(&engine);

            // Take the one blocking worker, so every racer's statement run is queued
            // behind this task and the clock moves `held_for` between each racer's
            // anchor and the branch that answers it.
            let hold = LEAD + case.held_for;
            let holder = ::tokio::task::spawn_blocking(move || ::std::thread::sleep(hold));
            ::tokio::time::sleep(LEAD).await;

            let answers = race(&router, case.racers, |_index, router| {
                let token = token.clone();
                async move { start_in(&router, WORKSPACE, STATEMENT, Some(&token)).await }
            })
            .await;
            holder.await.expect("the holding task must not panic");

            // Every racer is answered, and answered about ONE query: the one the
            // single call they all repeat started.
            let first = query_id_of(&answers[0].2).to_string();
            for (index, status, body) in &answers {
                assert_eq!(
                    *status, 202,
                    "case {:?}: racer {index} expected 202, got {status} {} {body}",
                    case.label,
                    error_code(body)
                );
                assert_eq!(
                    query_id_of(body),
                    first,
                    "case {:?}: racer {index} must report the one query, expected {first:?}, \
                     got {body}",
                    case.label
                );
                // Every one of these requests was anchored before the query was
                // recorded, so the status due at its anchor is the initial one —
                // whichever of the handler's two token checks answered it. A second
                // reading of the clock, taken after the run this row held, would
                // report a later status for the same query in the same request.
                assert_eq!(
                    status_of(body),
                    "SUBMITTED",
                    "case {:?}: racer {index} must report the status due at its own anchor \
                     (SUBMITTED), got {body}",
                    case.label
                );
            }
            assert_eq!(
                queries_held(&engine),
                held_before + 1,
                "case {:?}: the racers must start exactly one query between them",
                case.label
            );
        }
    });
}
