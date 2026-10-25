// SPDX-License-Identifier: CC0-1.0

//! The STATUS GATE `StopSearchJob` acts behind, driven over HTTP exactly as a client
//! drives it: `StartSearchJob`, `GetSearchJob`, `StopSearchJob` and
//! `ListSearchJobResults`.
//!
//! The rule under test: the operation acts on any search job the service has accepted
//! that has not yet reached a status it never leaves — the set-up phase included — so the
//! stop replaces whatever the scan still owed and the job converges on `STOPPED` instead
//! of on the terminal status its own scan would have reached. The statuses that already
//! satisfy the call answer the modelled empty 200 unchanged, as the `@idempotent` trait
//! asks; the statuses a stop is refused from (the terminal ones it does not produce) are
//! asserted in `tests/all/stop_search_job_cancel_window.rs`.
//!
//! Each assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — the phase the job is in when the stop arrives, what the scan still owed (a
//! scope that resolved to recovery points with items, one that resolved to recovery
//! points holding none, and one that resolved to nothing at all) and the signing region —
//! never over one request.

use serde_json::{json, Value};

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, recovery_point, seed_indexed_recovery_points,
    start_s3_search_job, PAST_THE_CREATING_INSTANT, PAST_THE_DUE_INSTANT,
    WITHIN_THE_DUE_INSTANT,
};

/// One row of the table: what this account holds, so the terminal status and the results
/// the scan is owed differ per row, and the region the row's calls are signed for.
struct ScanCase {
    case: &'static str,
    region: &'static str,
    index: Vec<state::IndexedRecoveryPoint>,
    /// The terminal status the scan settles to when nothing stops it.
    owed_terminal: &'static str,
    /// The matches the scan records, which `ListSearchJobResults` then pages.
    owed_results: usize,
}

/// One S3 recovery point holding `objects` objects in its backup index.
fn indexed(suffix: &str, objects: usize) -> state::IndexedRecoveryPoint {
    state::IndexedRecoveryPoint {
        backup: recovery_point(
            "S3",
            &format!("arn:aws:backup:us-west-2:000000000000:recovery-point:rp-{suffix}"),
            &format!("arn:aws:s3:::bucket-{suffix}"),
            std::time::SystemTime::now(),
        ),
        backup_vault_name: Some(format!("vault-{suffix}")),
        items: (0..objects)
            .map(|index| {
                types::ResultItem::S3ResultItem(types::S3ResultItem {
                    object_key: Some(format!("{suffix}/object-{index}")),
                    object_size: Some(1024 * (index as i64 + 1)),
                    ..Default::default()
                })
            })
            .collect(),
    }
}

fn scan_cases() -> Vec<ScanCase> {
    vec![
        ScanCase {
            case: "a scope that resolves to nothing at all",
            region: "us-west-2",
            index: Vec::new(),
            owed_terminal: state::FAILED,
            owed_results: 0,
        },
        ScanCase {
            case: "a scope that resolves to one recovery point holding two objects",
            region: "eu-west-1",
            index: vec![indexed("one", 2)],
            owed_terminal: state::COMPLETED,
            owed_results: 2,
        },
        ScanCase {
            case: "a scope that resolves to two recovery points, one of them empty",
            region: "ap-southeast-2",
            index: vec![indexed("two", 3), indexed("three", 0)],
            owed_terminal: state::COMPLETED,
            owed_results: 3,
        },
    ]
}

#[tokio::test]
async fn a_stop_before_the_scan_runs_is_accepted_and_replaces_what_the_scan_still_owed() {
    for ScanCase {
        case,
        region,
        index,
        owed_terminal,
        owed_results,
    } in scan_cases()
    {
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(&engine, index);
        let identifier = start_s3_search_job(&emulator, region, case).await;
        let job_uri = format!("/search-jobs/{identifier}");

        // Inside the set-up phase, which is the first status any caller can observe.
        tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
        let creating = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            creating.string("Status"),
            state::CREATING,
            "case {case:?}: a just-started job is being created, got {:?}",
            creating.get("Status")
        );

        let accepted = call(
            &emulator,
            region,
            "PUT",
            &format!("{job_uri}/actions/cancel"),
            None,
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a stop of a job the service has accepted must succeed, got {} ({:?})",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.body,
            json!({}),
            "case {case:?}: the modelled output is empty, got {:?}",
            accepted.body
        );

        // The stop is in flight, and nothing has completed yet.
        let after = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            after.string("Status"),
            state::STOPPING,
            "case {case:?}: a job whose stop is in flight is stopping, got {:?}",
            after.get("Status")
        );
        assert_eq!(
            after.get("CompletionTime"),
            &Value::Null,
            "case {case:?}: a stop in flight has completed nothing, got {:?}",
            after.get("CompletionTime")
        );

        // The stop replaced what the scan still owed, so the job converges on STOPPED and
        // never on the terminal status that scan would have reached.
        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            settled.string("Status"),
            state::STOPPED,
            "case {case:?}: a stopped job settles to stopped and not to the {owed_terminal} its scan would have reached, got {:?}",
            settled.get("Status")
        );
        // And a scan that never got through its scope recorded no match, so the results it
        // would have reported had it completed are not readable.
        let results = call(
            &emulator,
            region,
            "GET",
            &format!("{job_uri}/search-results"),
            None,
        )
        .await;
        assert_eq!(
            results.get("Results").as_array().map(Vec::len).unwrap_or_default(),
            0,
            "case {case:?}: a cancelled scan reports none of the {owed_results} match(es) it would have recorded, got {:?}",
            results.body
        );
    }
}

#[tokio::test]
async fn a_stop_of_a_running_scan_is_accepted_and_every_repeat_is_idempotent() {
    for ScanCase {
        case,
        region,
        index,
        ..
    } in scan_cases()
    {
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(&engine, index);
        let identifier = start_s3_search_job(&emulator, region, case).await;
        let job_uri = format!("/search-jobs/{identifier}");
        let cancel_uri = format!("{job_uri}/actions/cancel");

        // Past the set-up phase, so the scan is the running one the operation acts from.
        tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
        let running = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            running.string("Status"),
            state::RUNNING,
            "case {case:?}: the scan must be running before the stop, got {:?}",
            running.get("Status")
        );

        let stopped = call(&emulator, region, "PUT", &cancel_uri, None).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: a stop of a running scan must succeed, got {} ({:?})",
            stopped.status, stopped.body
        );
        assert_eq!(
            stopped.body,
            json!({}),
            "case {case:?}: the modelled output is empty, got {:?}",
            stopped.body
        );

        // The stop is in flight, and a repeat against it is a no-op success rather than a
        // conflict: that status already is on its way to the end state the call asks for.
        let stopping = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            stopping.string("Status"),
            state::STOPPING,
            "case {case:?}: a job whose stop is in flight is stopping, got {:?}",
            stopping.get("Status")
        );
        let repeat = call(&emulator, region, "PUT", &cancel_uri, None).await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: a repeat while the stop is in flight must be a no-op success, got {} ({:?})",
            repeat.status, repeat.body
        );

        // And once the clock has made the stop due, the repeat against the end state is
        // the same no-op success.
        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = call(&emulator, region, "GET", &job_uri, None).await;
        assert_eq!(
            settled.string("Status"),
            state::STOPPED,
            "case {case:?}: a stopped scan settles to stopped, got {:?}",
            settled.get("Status")
        );
        let again = call(&emulator, region, "PUT", &cancel_uri, None).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeat against the end state must be a no-op success, got {} ({:?})",
            again.status, again.body
        );
        assert_eq!(
            call(&emulator, region, "GET", &job_uri, None)
                .await
                .string("Status"),
            state::STOPPED,
            "case {case:?}: that repeat must leave the job stopped"
        );
    }
}
