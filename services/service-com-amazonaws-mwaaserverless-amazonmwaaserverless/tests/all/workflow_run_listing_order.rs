// SPDX-License-Identifier: CC0-1.0

//! The ORDER `ListWorkflowRuns` pages a workflow's runs in.
//!
//! The rule under test: a run listing is a chronological view of the workflow's runs.
//! They are reported OLDEST FIRST by the instant each run was created — the instant the
//! summary itself reports as `CreatedOn`/`StartedAt` (`smithy_query(query="shape",
//! shape="RunDetailSummary")`) — with the store's creation sequence separating two runs
//! created at the same instant, and the `NextToken` cursor is a position in that same
//! creation order rather than the identifier a run is stored under. Two invariants
//! follow, and are what these tables assert:
//!
//! * the sequence a client is paged through is sorted by creation instant, whatever the
//!   identifiers the runs carry and whatever order they happened to be started in; and
//! * a run created DURING a walk sorts AFTER the cursor that walk is holding, so it is
//!   still reached by a later page and no run is skipped or reported twice.
//!
//! The rows differ along what the rule generalises over: how many runs a workflow holds,
//! the page size, how the creation instants fall relative to the order the runs were
//! started (increasing, decreasing, interleaved, one millisecond apart, all equal), and
//! where in a walk a new run appears.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::RUN_STATUS_QUEUED;

use crate::run_lifecycle_progression::with_run;
use crate::task_instance_reads::epoch_seconds;
use crate::workflow_lifecycle::Harness;
use crate::workflow_run_listing::{created_workflow, list, paged_run_ids, started_run};

/// Seed a stored run's creation instant, in a status whose next change is not due, so
/// the row's order is exactly the one the instants describe and nothing about the run
/// moves mid-walk. Written through the same store seam every other suite seeds through.
fn created_at(harness: &Harness, arn: &str, run_id: &str, instant: SystemTime) {
    with_run(harness, arn, run_id, |run| {
        run.created_at = instant;
        run.started_on = instant;
        run.modified_at = instant;
        run.status = RUN_STATUS_QUEUED.to_string();
        run.completed_on = None;
        run.duration = None;
        // Far enough out that no transition is owed for the rest of the test.
        run.next_status_at = SystemTime::now() + Duration::from_secs(3_600);
    });
}

/// The `MaxResults` values every ordering row is walked at: the modelled minimum, sizes
/// that divide the set evenly and unevenly, and the modelled maximum, which covers any
/// of these sets in one page (`MaxResults` is `@range(min: 1, max: 100)`,
/// `smithy_query(query="operation", operation="ListWorkflowRuns")`).
const PAGE_SIZES: [i64; 4] = [1, 2, 3, 100];

/// One ordering row: how far back each run's creation instant is seeded, in the order the
/// runs are started.
struct Row {
    case: &'static str,
    ages_millis: &'static [u64],
}

/// The rows. The ages are deliberately not monotonic in the order the runs are started,
/// so a listing that reported start order — or the order of the hashed `RunId`s — would
/// fail every row but the first.
const ROWS: &[Row] = &[
    Row {
        case: "a single run",
        ages_millis: &[4_000],
    },
    Row {
        case: "started oldest first",
        ages_millis: &[9_000, 7_000, 5_000, 3_000],
    },
    Row {
        case: "started newest first",
        ages_millis: &[3_000, 5_000, 7_000, 9_000],
    },
    Row {
        case: "creation instants interleaved with the start order",
        ages_millis: &[6_000, 2_000, 9_000, 4_000, 7_500],
    },
    Row {
        case: "instants one millisecond apart",
        ages_millis: &[5_002, 5_000, 5_001, 5_003],
    },
    Row {
        case: "eight runs, alternating old and new",
        ages_millis: &[8_000, 1_000, 7_000, 2_000, 6_000, 3_000, 5_000, 4_000],
    },
];

#[tokio::test]
async fn runs_are_paged_oldest_first_whatever_order_they_were_started_in() {
    let harness = Harness::new("us-east-1");

    for (index, row) in ROWS.iter().enumerate() {
        let arn = created_workflow(&harness, &format!("chronological{index}")).await;

        // Every instant in a row is measured from ONE reference, so a row's order is the
        // order of its ages even when two of them are a millisecond apart.
        let reference = SystemTime::now();
        let mut seeded: Vec<(u64, String)> = Vec::new();
        for age_millis in row.ages_millis {
            let run_id = started_run(&harness, &arn, None).await;
            created_at(
                &harness,
                &arn,
                &run_id,
                reference - Duration::from_millis(*age_millis),
            );
            seeded.push((*age_millis, run_id));
        }
        // Oldest first: the LARGEST age is the earliest creation instant. A stable sort,
        // so two runs of equal age stay in the order they were started.
        seeded.sort_by_key(|(age_millis, _)| std::cmp::Reverse(*age_millis));
        let expected: Vec<String> = seeded.iter().map(|(_, run_id)| run_id.clone()).collect();

        for page_size in PAGE_SIZES {
            let case = format!("{} at MaxResults={page_size}", row.case);
            let seen = paged_run_ids(&harness, &arn, page_size, None, &case).await;
            assert_eq!(
                seen, expected,
                "case {case:?}: expected the runs oldest first by creation instant, got \
                 {seen:?}"
            );
        }

        // And the instants the summaries themselves report rise across the listing: the
        // order a client sees is the order of the timestamps it reads, not a separate
        // claim about it.
        let (status, page) = list(&harness, &arn, Some(100), None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ListWorkflowRuns to succeed, got {status} {page:?}",
            row.case
        );
        let summaries = page["WorkflowRuns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: expected a WorkflowRuns list", row.case))
            .clone();
        for member in ["CreatedOn", "StartedAt"] {
            let instants: Vec<f64> = summaries
                .iter()
                .map(|summary| {
                    epoch_seconds(&summary["RunDetailSummary"][member]).unwrap_or_else(|| {
                        panic!(
                            "case {:?}: every summary reports {member}, got {summary:?}",
                            row.case
                        )
                    })
                })
                .collect();
            assert!(
                instants.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {:?}: expected {member} to be non-decreasing across the listing, got \
                 {instants:?}",
                row.case
            );
        }
    }
}

/// One concurrency row: how many runs the workflow already holds, the page size the
/// client walks at, and after how many pages a further run is started.
struct Concurrent {
    case: &'static str,
    existing: usize,
    page_size: i64,
    start_after_pages: usize,
}

/// The rows. They differ along the page size, the number of runs already stored and how
/// far into the walk the new run appears — never so far that the walk has already ended,
/// since a completed walk issues no cursor to continue from.
const CONCURRENT: &[Concurrent] = &[
    Concurrent {
        case: "one run per page, new run after the first page",
        existing: 3,
        page_size: 1,
        start_after_pages: 1,
    },
    Concurrent {
        case: "one run per page, new run after the second page",
        existing: 3,
        page_size: 1,
        start_after_pages: 2,
    },
    Concurrent {
        case: "two runs per page, new run after the first page",
        existing: 5,
        page_size: 2,
        start_after_pages: 1,
    },
    Concurrent {
        case: "two runs per page, new run after the second page",
        existing: 5,
        page_size: 2,
        start_after_pages: 2,
    },
];

#[tokio::test]
async fn a_run_started_during_a_walk_is_still_reached_by_a_later_page() {
    let harness = Harness::new("eu-west-1");

    for (index, row) in CONCURRENT.iter().enumerate() {
        let arn = created_workflow(&harness, &format!("concurrentStart{index}")).await;
        let mut expected: Vec<String> = Vec::new();
        for _ in 0..row.existing {
            expected.push(started_run(&harness, &arn, None).await);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        let mut started_late: Option<String> = None;
        loop {
            let (status, page) =
                list(&harness, &arn, Some(row.page_size), token.as_deref(), None).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected page {} to succeed, got {status} {page:?}",
                row.case,
                pages + 1
            );
            for summary in page["WorkflowRuns"]
                .as_array()
                .unwrap_or_else(|| panic!("case {:?}: expected a WorkflowRuns list", row.case))
            {
                seen.push(
                    summary["RunId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {:?}: a summary carries its RunId", row.case)
                        })
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 32,
                "case {:?}: pagination did not terminate, saw {seen:?}",
                row.case
            );

            // The run a concurrent client starts while this one is paging.
            if pages == row.start_after_pages {
                let late = started_run(&harness, &arn, None).await;
                expected.push(late.clone());
                started_late = Some(late);
            }

            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        let late = started_late.unwrap_or_else(|| {
            panic!(
                "case {:?}: the walk must still be open after {} pages",
                row.case, row.start_after_pages
            )
        });
        assert_eq!(
            seen, expected,
            "case {:?}: expected every run exactly once, oldest first, with the run {late:?} \
             started mid-walk at the end, got {seen:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn runs_created_at_the_same_instant_keep_one_stable_total_order() {
    let harness = Harness::new("ap-south-1");
    let arn = created_workflow(&harness, "sameInstantRuns").await;

    // Five runs whose creation instant is byte-for-byte the same, so the creation
    // sequence is the only thing that can separate them. A pagination cursor must still
    // be a position in a TOTAL order: no run may be skipped or reported twice.
    let instant = SystemTime::now() - Duration::from_secs(600);
    let mut expected: Vec<String> = Vec::new();
    for _ in 0..5 {
        let run_id = started_run(&harness, &arn, None).await;
        created_at(&harness, &arn, &run_id, instant);
        expected.push(run_id);
    }

    for page_size in PAGE_SIZES {
        let case = format!("identical instants at MaxResults={page_size}");
        let seen = paged_run_ids(&harness, &arn, page_size, None, &case).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: runs the clock cannot separate are ordered by the sequence they \
             were created in, got {seen:?}"
        );
    }

    // The order is stable: a second walk of the same store reports the same sequence.
    let first = paged_run_ids(&harness, &arn, 2, None, "first walk").await;
    let again = paged_run_ids(&harness, &arn, 2, None, "second walk").await;
    assert_eq!(
        again, first,
        "case repeated walk: the listing order must not move between walks, got {again:?} \
         after {first:?}"
    );
}
