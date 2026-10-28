// SPDX-License-Identifier: CC0-1.0

//! What a search job that reached a terminal status reports in `StatusMessage`, driven
//! over HTTP exactly as a client drives it: `StartSearchJob`, then `GetSearchJob`,
//! `ListSearchJobs` and `ListSearchJobBackups`.
//!
//! The rule under test: the `StatusMessage` of a settled search job is written from the
//! job's own resolved scope, so a scan that did NOT get through every recovery point its
//! scope covered reports the issues its terminal status came with — a scope that resolved
//! to nothing fails saying so, a scope holding covered recovery points the scan could not
//! read completes WITH a message saying how many of them went unscanned, and only a scan
//! that got through its whole scope completes with no message at all.
//!
//! Every assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — how many recovery points the scope covers, how many of them the scan can read,
//! the index status that makes one unreadable, and the item type the backups hold — never
//! over one request.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, seed_indexed_recovery_points, PAST_THE_CREATING_INSTANT,
    PAST_THE_DUE_INSTANT,
};

const REGION: &str = "eu-west-1";

/// The backup-index statuses a recovery point's record can report beside the modelled
/// `SearchJobState` values: the ones AWS Backup reports for an index that is being built or
/// taken down (`smithy_query(query="doc", shape="ListSearchJobBackups")` states the rule in
/// terms of the `ACTIVE` and `FAILED` ones).
const INDEX_PENDING: &str = "PENDING";
const INDEX_DELETING: &str = "DELETING";

/// The message a record carrying a non-readable index status is seeded with: the backup's
/// OWN explanation, which `ListSearchJobBackups` displays and which is not the job-level
/// message under test here.
const WHY_LEFT_OUT: &str = "the backup index could not be read";

/// The recovery-point ARN of the backup `suffix` names.
fn backup_arn(suffix: &str) -> String {
    format!(
        "arn:aws:backup:{REGION}:{}:recovery-point:rp-{suffix}",
        engine::emulator_account_id()
    )
}

/// One item of the backup `suffix`'s index, of the kind a backup of `resource_type` holds.
fn item(resource_type: &str, suffix: &str, index: usize) -> types::ResultItem {
    if resource_type == "EBS" {
        types::ResultItem::EBSResultItem(types::EBSResultItem {
            file_path: Some(format!("/{suffix}/file-{index}")),
            file_size: Some(2048),
            ..Default::default()
        })
    } else {
        types::ResultItem::S3ResultItem(types::S3ResultItem {
            object_key: Some(format!("{suffix}/object-{index}")),
            object_size: Some(2048),
            ..Default::default()
        })
    }
}

/// One recovery point as the store holds it: the status its backup index is in, the message
/// that status comes with, and two items the index holds.
fn indexed(
    suffix: &str,
    resource_type: &str,
    index_status: Option<&str>,
) -> state::IndexedRecoveryPoint {
    state::IndexedRecoveryPoint {
        backup: types::SearchJobBackupsResult {
            status: index_status.map(ToString::to_string),
            status_message: Some(WHY_LEFT_OUT.to_string()),
            resource_type: Some(resource_type.to_string()),
            backup_resource_arn: Some(backup_arn(suffix)),
            source_resource_arn: Some(format!("arn:aws:s3:::bucket-{suffix}")),
            index_creation_time: None,
            backup_creation_time: None,
        },
        backup_vault_name: Some(format!("vault-{suffix}")),
        items: (0..2).map(|index| item(resource_type, suffix, index)).collect(),
    }
}

/// What `GetSearchJob` reports for the job `identifier` names.
async fn fetch(emulator: &MultiServiceRouter, identifier: &str) -> crate::search_jobs::Answer {
    call(
        emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await
}

/// One row of the outcome table: the recovery points the store holds for the row, the item
/// type the row's scope selects, and the terminal status and `StatusMessage` the job must
/// settle to.
struct OutcomeCase {
    case: &'static str,
    resource_type: &'static str,
    /// Each covered recovery point's index status; `None` is a record reporting none.
    held: Vec<Option<&'static str>>,
}

impl OutcomeCase {
    /// How many of the row's recovery points the row's scope covers — all of them: every
    /// record the row seeds is of the resource type the scope names.
    fn covered(&self) -> usize {
        self.held.len()
    }

    /// How many of those the scan cannot read, which is what the job has to explain.
    fn unscanned(&self) -> usize {
        self.held
            .iter()
            .filter(|status| {
                status.is_none_or(|status| !state::SEARCHABLE_INDEX_STATUSES.contains(&status))
            })
            .count()
    }

    /// The terminal status and `StatusMessage` the rule requires of this row.
    fn expected(&self) -> (&'static str, Option<String>) {
        if self.covered() == 0 {
            return (state::FAILED, Some(state::SCOPE_EMPTY.to_string()));
        }
        if self.unscanned() == 0 {
            return (state::COMPLETED, None);
        }
        (
            state::COMPLETED,
            Some(state::unscanned_points_message(
                self.unscanned(),
                self.covered(),
            )),
        )
    }
}

fn outcome_cases() -> Vec<OutcomeCase> {
    vec![
        OutcomeCase {
            case: "a scope resolving to nothing at all",
            resource_type: "S3",
            held: Vec::new(),
        },
        OutcomeCase {
            case: "one covered backup, readable",
            resource_type: "S3",
            held: vec![Some(state::INDEX_ACTIVE)],
        },
        OutcomeCase {
            case: "three covered backups, all readable",
            resource_type: "S3",
            held: vec![
                Some(state::INDEX_ACTIVE),
                Some(state::COMPLETED),
                Some(state::INDEX_ACTIVE),
            ],
        },
        OutcomeCase {
            case: "one of two covered backups failed, the permissions case the model names",
            resource_type: "S3",
            held: vec![Some(state::INDEX_ACTIVE), Some(state::FAILED)],
        },
        OutcomeCase {
            case: "one of three covered backups still being indexed",
            resource_type: "S3",
            held: vec![
                Some(state::INDEX_ACTIVE),
                Some(INDEX_PENDING),
                Some(state::COMPLETED),
            ],
        },
        OutcomeCase {
            case: "two of three covered backups unreadable",
            resource_type: "S3",
            held: vec![
                Some(INDEX_DELETING),
                Some(state::INDEX_ACTIVE),
                Some(state::STOPPED),
            ],
        },
        OutcomeCase {
            case: "one of two covered backups reporting no index status at all",
            resource_type: "S3",
            held: vec![Some(state::INDEX_ACTIVE), None],
        },
        OutcomeCase {
            case: "every covered backup unreadable",
            resource_type: "S3",
            held: vec![Some(state::FAILED), Some(state::RUNNING), None],
        },
        OutcomeCase {
            case: "an EBS scope with one of two covered backups unreadable",
            resource_type: "EBS",
            held: vec![Some(state::INDEX_ACTIVE), Some(state::FAILED)],
        },
        OutcomeCase {
            case: "an EBS scope whose single covered backup is readable",
            resource_type: "EBS",
            held: vec![Some(state::COMPLETED)],
        },
    ]
}

#[test]
fn the_completion_with_issues_message_is_the_documented_sentence_over_the_jobs_own_counts() {
    // The template, pinned on counts no row of the table above uses, so the sentence the
    // service answers with is fixed here and not only compared against itself: the case the
    // model states this member in is a search that "contained recovery points unable to be
    // scanned" (`smithy_query(query="doc", shape="GetSearchJobOutput",
    // member="StatusMessage")`), and the counts are the job's own.
    assert_eq!(
        state::unscanned_points_message(4, 9),
        "Search contained 4 of 9 recovery points unable to be scanned",
        "case \"the message template\": the message must state how much of the covered scope \
         went unscanned, got {:?}",
        state::unscanned_points_message(4, 9)
    );
}

#[tokio::test]
async fn a_settled_search_job_reports_the_issues_its_terminal_status_came_with() {
    // One emulator per row, so each row's store holds exactly the recovery points it
    // seeds. Every job is started BEFORE the single sleep, so the clock makes every row's
    // terminal status due at once instead of once per row.
    let mut rows: Vec<(OutcomeCase, MultiServiceRouter, String)> = Vec::new();
    for case in outcome_cases() {
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(
            &engine,
            case.held
                .iter()
                .enumerate()
                .map(|(index, status)| {
                    indexed(&format!("rp-{index}"), case.resource_type, *status)
                })
                .collect(),
        );
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/search-jobs",
            Some(json!({
                "Name": "with-issues",
                "SearchScope": {"BackupResourceTypes": [case.resource_type]}
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartSearchJob must accept the request, got {} ({:?})",
            case.case, started.status, started.body
        );
        rows.push((case, emulator, started.string("SearchJobIdentifier")));
    }

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

    for (case, emulator, identifier) in rows {
        let name = case.case;
        let (expected_status, expected_message) = case.expected();

        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {name:?}: expected 200 from GetSearchJob, got {} ({:?})",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("Status"),
            expected_status,
            "case {name:?}: expected the terminal status {expected_status}, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.get("StatusMessage").as_str(),
            expected_message.as_deref(),
            "case {name:?}: expected StatusMessage {expected_message:?}, got {:?}",
            fetched.get("StatusMessage")
        );

        // The listing reports the same record, so a client that polls either one reads one
        // answer about whether the search got through its scope.
        let listed = call(&emulator, REGION, "GET", "/search-jobs", None).await;
        let summary = listed.get("SearchJobs")[0].clone();
        assert_eq!(
            summary["Status"].as_str(),
            Some(expected_status),
            "case {name:?}: ListSearchJobs must report the same Status, got {summary:?}"
        );
        assert_eq!(
            summary["StatusMessage"].as_str(),
            expected_message.as_deref(),
            "case {name:?}: ListSearchJobs must report the same StatusMessage, got {summary:?}"
        );

        // The counts and the display agree with that message: the scan got through only the
        // readable backups, while every covered one is still displayed.
        let scanned = case.covered() - case.unscanned();
        assert_eq!(
            fetched.get("SearchScopeSummary")["TotalRecoveryPointsToScanCount"].as_u64(),
            Some(scanned as u64),
            "case {name:?}: expected {scanned} recovery point(s) to scan, got {:?}",
            fetched.get("SearchScopeSummary")
        );
        let backups = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/backups"),
            None,
        )
        .await;
        assert_eq!(
            backups.get("Results").as_array().map(Vec::len),
            Some(case.covered()),
            "case {name:?}: expected all {} covered backup(s) to be displayed, got {:?}",
            case.covered(),
            backups.body
        );
    }
}

#[tokio::test]
async fn the_issues_message_is_written_only_once_the_terminal_status_is_due() {
    // The message travels WITH the terminal status, so a job still being set up or still
    // scanning reports no message at all: the record cannot explain a completion it has
    // not reached.
    let (emulator, engine) = emulator_with_engine();
    seed_indexed_recovery_points(
        &engine,
        vec![
            indexed("readable", "S3", Some(state::INDEX_ACTIVE)),
            indexed("failed", "S3", Some(state::FAILED)),
        ],
    );
    let started = call(
        &emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "phases", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case \"the transient phases\": StartSearchJob must accept the request, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("SearchJobIdentifier");

    let creating = fetch(&emulator, &identifier).await;
    assert_eq!(
        creating.string("Status"),
        state::CREATING,
        "case \"being set up\": expected the set-up phase, got {:?}",
        creating.body
    );
    assert_eq!(
        creating.get("StatusMessage").as_str(),
        None,
        "case \"being set up\": a job that has not settled reports no StatusMessage, got {:?}",
        creating.get("StatusMessage")
    );

    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
    let running = fetch(&emulator, &identifier).await;
    assert_eq!(
        running.string("Status"),
        state::RUNNING,
        "case \"scanning\": expected the scanning phase, got {:?}",
        running.body
    );
    assert_eq!(
        running.get("StatusMessage").as_str(),
        None,
        "case \"scanning\": a scan still running reports no StatusMessage, got {:?}",
        running.get("StatusMessage")
    );

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let settled = fetch(&emulator, &identifier).await;
    let expected = state::unscanned_points_message(1, 2);
    assert_eq!(
        settled.string("Status"),
        state::COMPLETED,
        "case \"settled\": the scan ran, so it completes, got {:?}",
        settled.body
    );
    assert_eq!(
        settled.get("StatusMessage").as_str(),
        Some(expected.as_str()),
        "case \"settled\": expected StatusMessage {expected:?}, got {:?}",
        settled.get("StatusMessage")
    );
}
