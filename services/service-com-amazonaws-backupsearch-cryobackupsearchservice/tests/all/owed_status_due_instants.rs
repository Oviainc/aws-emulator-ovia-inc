// SPDX-License-Identifier: CC0-1.0

//! The DUE INSTANTS this service stores: the instants a status is owed from and the
//! instant a `ClientToken`'s window closes.
//!
//! The rule under test: every clock instant this service computes from a base instant and
//! a span is computed FALLIBLY — it is `base + span` for every sum the platform can
//! represent, and for a sum it cannot the span is taken as having already elapsed (the
//! base itself), so the handler still answers with a modelled response instead of
//! aborting. One span is the published `ClientToken` window, the others are the spans the
//! clock owes a status after, and all of them go through the same computation
//! (`state::due_at`).
//!
//! The first test asserts the rule over a TABLE spanning what it generalises over — the
//! span (every one this service measures, plus the degenerate, the small and the largest
//! there is) crossed with the base (the epoch, now, a far-future instant, and the LAST
//! instant the platform can represent, found by bisection rather than hardcoded). The
//! remaining tests drive the three operations that store a due instant end-to-end over
//! HTTP, pinning that each one is owed at its span after the call that started it and that
//! the clock then materialises it — so making the computation fallible leaves every answer
//! a client sees unchanged.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_jobs::{settled_search_job, Settled};
use crate::search_jobs::{
    call, emulator_with_engine, recovery_point, seed_recovery_points, PAST_THE_CREATING_INSTANT,
    PAST_THE_DUE_INSTANT,
};

/// One nanosecond, the finest step a `Duration` takes and the step the bisection below
/// closes on.
const ONE_NANOSECOND: Duration = Duration::from_nanos(1);

/// The spans this service measures a due instant from, each named by the constant that
/// declares it, plus the ends of the class: no span at all, the finest step there is, a
/// span of a different order (a day) and the largest a `Duration` can hold.
fn spans() -> Vec<(&'static str, Duration)> {
    vec![
        ("the set-up span", state::CREATING_DELAY),
        ("the transition span", state::TRANSITION_DELAY),
        ("the published token window", state::CLIENT_TOKEN_VALIDITY),
        ("no span at all", Duration::ZERO),
        ("one nanosecond", ONE_NANOSECOND),
        ("a day", Duration::from_secs(24 * 60 * 60)),
        ("the largest span there is", Duration::MAX),
    ]
}

/// Halfway between two offsets, at nanosecond precision.
fn midpoint(low: Duration, high: Duration) -> Duration {
    let nanos = (low.as_nanos() + high.as_nanos()) / 2;
    Duration::new(
        u64::try_from(nanos / 1_000_000_000).unwrap_or(u64::MAX),
        u32::try_from(nanos % 1_000_000_000).unwrap_or(0),
    )
}

/// The LAST instant the platform can represent: adding anything at all to it is a sum
/// `SystemTime` cannot hold.
///
/// Found by bisecting the offset from the epoch, because the representable range belongs
/// to the platform and not to this crate — a hardcoded instant would pin one platform's
/// range and stop being the edge on another.
fn last_representable_instant() -> SystemTime {
    // The epoch is representable; the largest offset there is, is not.
    let mut representable = Duration::ZERO;
    let mut beyond = Duration::MAX;
    while beyond.saturating_sub(representable) > ONE_NANOSECOND {
        let middle = midpoint(representable, beyond);
        if middle == representable || middle == beyond {
            break;
        }
        if UNIX_EPOCH.checked_add(middle).is_some() {
            representable = middle;
        } else {
            beyond = middle;
        }
    }
    UNIX_EPOCH
        .checked_add(representable)
        .expect("the bisected offset is representable")
}

/// The base instants the rule is asserted over: the epoch, a moment after it, now, a
/// far-future instant and the representable edge.
fn bases() -> Vec<(&'static str, SystemTime)> {
    let now = SystemTime::now();
    vec![
        ("the epoch", UNIX_EPOCH),
        ("a second after the epoch", UNIX_EPOCH + Duration::from_secs(1)),
        ("now", now),
        (
            "a century from now",
            now + Duration::from_secs(100 * 365 * 24 * 60 * 60),
        ),
        ("the last representable instant", last_representable_instant()),
    ]
}

#[tokio::test]
async fn a_due_instant_is_the_sum_where_it_is_representable_and_the_base_where_it_is_not() {
    let edge = last_representable_instant();
    assert!(
        edge.checked_add(ONE_NANOSECOND).is_none(),
        "case \"the representable edge\": adding anything to the last representable \
         instant must be unrepresentable"
    );

    for (base_case, base) in bases() {
        for (span_case, span) in spans() {
            let case = format!("{base_case} plus {span_case}");
            let due = state::due_at(base, span);
            match base.checked_add(span) {
                Some(sum) => assert_eq!(
                    due, sum,
                    "case {case:?}: a representable sum is exactly base + span, got {due:?}"
                ),
                None => assert_eq!(
                    due, base,
                    "case {case:?}: an unrepresentable sum is taken as already elapsed, \
                     i.e. the base itself, got {due:?}"
                ),
            }
            assert!(
                due >= base,
                "case {case:?}: a due instant is never before the instant it is measured \
                 from, got {due:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_window_that_cannot_be_represented_is_a_window_already_closed() {
    // The same rule on the one span the model publishes: a token minted at an instant
    // whose window end is not representable degrades to a CLOSED window, so the next call
    // carrying it is a new request rather than a panic.
    let edge = last_representable_instant();
    let token = state::ClientToken::minted("edge-probe".to_string(), edge);
    assert!(
        !token.is_valid_at(edge),
        "case \"a token minted at the representable edge\": its window must already be \
         closed, got valid=true"
    );
    assert!(
        !token.is_live("edge-probe", edge),
        "case \"a token minted at the representable edge\": it names no earlier request, \
         got live=true"
    );

    // And under a sane clock the window is still the full published one.
    let sane = SystemTime::now();
    let token = state::ClientToken::minted("sane-probe".to_string(), sane);
    assert!(
        token.is_valid_at(sane + state::CLIENT_TOKEN_VALIDITY - ONE_NANOSECOND),
        "case \"a token minted now\": its window is open up to the published bound, got \
         valid=false"
    );
    assert!(
        !token.is_valid_at(sane + state::CLIENT_TOKEN_VALIDITY),
        "case \"a token minted now\": its window is closed at the published bound, got \
         valid=true"
    );
}

/// One row of the start table: the `StartSearchJob` body and the recovery points its
/// scope is resolved against, which is what decides the terminal status it is owed.
struct StartRow {
    case: &'static str,
    region: &'static str,
    body: serde_json::Value,
    held: Vec<types::SearchJobBackupsResult>,
    terminal: &'static str,
}

fn start_rows() -> Vec<StartRow> {
    vec![
        StartRow {
            case: "a bare S3 scope resolving to nothing",
            region: "us-west-2",
            body: json!({"SearchScope": {"BackupResourceTypes": ["S3"]}}),
            held: Vec::new(),
            terminal: state::FAILED,
        },
        StartRow {
            case: "a named and tagged S3 scope resolving to one recovery point",
            region: "eu-central-1",
            body: json!({
                "Name": "due-instants-s3",
                "Tags": {"env": "test"},
                "SearchScope": {"BackupResourceTypes": ["S3"]}
            }),
            held: vec![recovery_point(
                "S3",
                "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-due",
                "arn:aws:s3:::bucket-due",
                SystemTime::now(),
            )],
            terminal: state::COMPLETED,
        },
        StartRow {
            case: "an EBS scope under a client token resolving to two recovery points",
            region: "ap-southeast-2",
            body: json!({
                "ClientToken": "due-instants-token",
                "SearchScope": {"BackupResourceTypes": ["EBS"]},
                "ItemFilters": {
                    "EBSItemFilters": [{
                        "FilePaths": [{"Value": "/var", "Operator": "BEGINS_WITH"}]
                    }]
                }
            }),
            held: vec![
                recovery_point(
                    "EBS",
                    "arn:aws:backup:eu-west-1:000000000000:recovery-point:rp-due-1",
                    "arn:aws:ec2:eu-west-1:000000000000:volume/vol-due-1",
                    SystemTime::now(),
                ),
                recovery_point(
                    "EBS",
                    "arn:aws:backup:eu-west-1:000000000000:recovery-point:rp-due-2",
                    "arn:aws:ec2:eu-west-1:000000000000:volume/vol-due-2",
                    SystemTime::now(),
                ),
            ],
            terminal: state::COMPLETED,
        },
    ]
}

/// The statuses the store holds as owed for one search job, each with the instant it is
/// owed from — read off the record so the assertion is about what the handler STORED and
/// not about what a reader happened to report.
fn owed_search_job_statuses(
    engine: &engine::InMemoryEngine,
    identifier: &str,
) -> Vec<(SystemTime, String)> {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    store
        .search_jobs
        .get(identifier)
        .expect("the search job the start call minted")
        .owed_statuses
        .iter()
        .map(|&(due, status, _)| (due, status.to_string()))
        .collect()
}

/// The status one export job is owed, with the instant it is owed from.
fn owed_export_job_status(
    engine: &engine::InMemoryEngine,
    identifier: &str,
) -> Option<(SystemTime, String)> {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    store
        .export_jobs
        .get(identifier)
        .expect("the export job the start call minted")
        .next_status_due
        .as_ref()
        .map(|(due, status, _)| (*due, (*status).to_string()))
}

/// Assert `due` is `span` after the call that was made between `before` and `after`: the
/// instant is measured from the handler's own clock reading, which lies in that interval.
fn assert_due_after_the_call(
    case: &str,
    what: &str,
    due: SystemTime,
    before: SystemTime,
    after: SystemTime,
    span: Duration,
) {
    assert!(
        due >= before + span && due <= after + span,
        "case {case:?}: {what} must be owed {span:?} after the call (between {:?} and \
         {:?}), got {due:?}",
        before + span,
        after + span
    );
}

#[tokio::test]
async fn a_started_search_job_owes_each_phase_at_its_span_after_the_call() {
    for row in start_rows() {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, row.held.clone());

        let before = SystemTime::now();
        let answer = call(
            &emulator,
            row.region,
            "PUT",
            "/search-jobs",
            Some(row.body.clone()),
        )
        .await;
        let after = SystemTime::now();
        assert_eq!(
            answer.status, 200,
            "case {:?}: StartSearchJob must accept the request, got {} ({:?})",
            row.case, answer.status, answer.body
        );
        let identifier = answer.string("SearchJobIdentifier");

        let owed = owed_search_job_statuses(&engine, &identifier);
        assert_eq!(
            owed.iter().map(|(_, status)| status.as_str()).collect::<Vec<_>>(),
            vec![state::RUNNING, row.terminal],
            "case {:?}: a new search job owes RUNNING and then its terminal status, got {owed:?}",
            row.case
        );
        assert_due_after_the_call(
            row.case,
            "the RUNNING phase",
            owed[0].0,
            before,
            after,
            state::CREATING_DELAY,
        );
        assert_due_after_the_call(
            row.case,
            "the terminal status",
            owed[1].0,
            before,
            after,
            state::TRANSITION_DELAY,
        );

        // The stored instants are what the clock materialises: once both spans have
        // passed the job reports the terminal status its scope owes it.
        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = call(
            &emulator,
            row.region,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            settled.string("Status"),
            row.terminal,
            "case {:?}: the job settles to the status it was owed, got {:?}",
            row.case,
            settled.get("Status")
        );
        assert!(
            owed_search_job_statuses(&engine, &identifier).is_empty(),
            "case {:?}: a settled job owes nothing more, got {:?}",
            row.case,
            owed_search_job_statuses(&engine, &identifier)
        );
    }
}

#[tokio::test]
async fn a_stopped_search_job_owes_its_end_at_the_transition_span_after_the_stop() {
    // Rows differing in what the stop replaced: a scope resolving to nothing (whose scan
    // would have FAILED) and one resolving to a recovery point (whose scan would have
    // COMPLETED), in two signing regions.
    let stop_rows: [(&str, &str, Vec<_>); 2] = [
        ("a scope resolving to nothing", "us-west-2", Vec::new()),
        (
            "a scope resolving to one recovery point",
            "eu-west-1",
            vec![recovery_point(
                "S3",
                "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-stop",
                "arn:aws:s3:::bucket-stop",
                SystemTime::now(),
            )],
        ),
    ];

    for (case, region, held) in stop_rows {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, held);
        let started = call(
            &emulator,
            region,
            "PUT",
            "/search-jobs",
            Some(json!({"Name": "to-stop", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSearchJob must accept the request, got {} ({:?})",
            started.status, started.body
        );
        let identifier = started.string("SearchJobIdentifier");
        let job_uri = format!("/search-jobs/{identifier}");

        // A stop acts from the running scan, so wait for the set-up phase to pass: the
        // span asserted below is then measured from the stop call itself.
        tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
        let before = SystemTime::now();
        let stopped = call(
            &emulator,
            region,
            "PUT",
            &format!("{job_uri}/actions/cancel"),
            None,
        )
        .await;
        let after = SystemTime::now();
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: StopSearchJob must accept the request, got {} ({:?})",
            stopped.status, stopped.body
        );

        let owed = owed_search_job_statuses(&engine, &identifier);
        assert_eq!(
            owed.iter().map(|(_, status)| status.as_str()).collect::<Vec<_>>(),
            vec![state::STOPPED],
            "case {case:?}: a stopped job owes STOPPED and nothing else, got {owed:?}"
        );
        assert_due_after_the_call(
            case,
            "the STOPPED status",
            owed[0].0,
            before,
            after,
            state::TRANSITION_DELAY,
        );

        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            settled.string("Status"),
            state::STOPPED,
            "case {case:?}: the job settles to the STOPPED it was owed, got {:?}",
            settled.get("Status")
        );
    }
}

#[tokio::test]
async fn a_started_export_job_owes_its_end_at_the_transition_span_after_the_call() {
    // Both statuses a search job can be exported from, so the rule is asserted on the
    // export of a completed scan and of a stopped one.
    for settled in [Settled::Completed, Settled::Stopped] {
        let case = format!("an export of a {} search job", settled.expected_status());
        let region = "us-west-2";
        let (emulator, engine) = emulator_with_engine();
        let (search_job, _) =
            settled_search_job(&emulator, &engine, region, &case, settled).await;

        let before = SystemTime::now();
        let started = call(
            &emulator,
            region,
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": search_job,
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "due-instants-bucket"}
                }
            })),
        )
        .await;
        let after = SystemTime::now();
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSearchResultExportJob must accept the request, got {} ({:?})",
            started.status, started.body
        );
        let identifier = started.string("ExportJobIdentifier");

        let owed = owed_export_job_status(&engine, &identifier)
            .unwrap_or_else(|| panic!("case {case:?}: a new export job owes a status"));
        assert_due_after_the_call(
            &case,
            "the export's end status",
            owed.0,
            before,
            after,
            state::TRANSITION_DELAY,
        );

        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let fetched = call(
            &emulator,
            region,
            "GET",
            &format!("/export-search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.string("Status"),
            owed.1,
            "case {case:?}: the export job settles to the status it was owed, got {:?}",
            fetched.get("Status")
        );
        assert!(
            owed_export_job_status(&engine, &identifier).is_none(),
            "case {case:?}: a settled export job owes nothing more, got {:?}",
            owed_export_job_status(&engine, &identifier)
        );
    }
}
