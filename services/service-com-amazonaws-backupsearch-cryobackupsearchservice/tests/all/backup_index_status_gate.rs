// SPDX-License-Identifier: CC0-1.0

//! Which of the recovery points a `SearchScope` covers a scan actually gets into, driven
//! over HTTP exactly as a client drives it: `StartSearchJob`, then `GetSearchJob`,
//! `ListSearchJobResults` and `ListSearchJobBackups`.
//!
//! The rule under test: a covered recovery point takes part in a search only while its
//! backup index is in a state a search can read, so a backup in any other state
//! contributes neither itself nor its indexed items to the backups and items the job
//! reports as to be scanned, to the counts its progress reports, or to the results a
//! reader pages — it is only DISPLAYED, with the status and status message that say why,
//! by `ListSearchJobBackups`.
//!
//! Every assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — the index status reported (every value of the model's `SearchJobState` enum, the
//! backup-index statuses the rule is documented in, and a record reporting none at all),
//! the item type the backup holds (S3 objects, EBS files), how many recovery points are
//! covered and how many items each holds, and the item filters the scan runs with — never
//! over one request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, seed_indexed_recovery_points, Answer, PAST_THE_DUE_INSTANT,
};

const REGION: &str = "us-west-2";

/// The recovery-point ARN of the backup `suffix` names.
fn backup_arn(suffix: &str) -> String {
    format!(
        "arn:aws:backup:{REGION}:{}:recovery-point:rp-{suffix}",
        engine::emulator_account_id()
    )
}

/// The source resource the backup `suffix` names was taken of: a bucket for an S3 backup,
/// a volume for an EBS one.
fn source_arn(resource_type: &str, suffix: &str) -> String {
    if resource_type == "EBS" {
        format!(
            "arn:aws:ec2:{REGION}:{}:volume/vol-{suffix}",
            engine::emulator_account_id()
        )
    } else {
        format!("arn:aws:s3:::bucket-{suffix}")
    }
}

/// The name item `index` of the backup `suffix` carries: an object key for an S3 backup, a
/// file path for an EBS one.
fn item_name(resource_type: &str, suffix: &str, index: usize) -> String {
    if resource_type == "EBS" {
        format!("/{suffix}/file-{index}")
    } else {
        format!("{suffix}/object-{index}")
    }
}

/// Item `index` of the backup `suffix`'s index, as that index holds it.
fn item(resource_type: &str, suffix: &str, index: usize) -> types::ResultItem {
    let name = item_name(resource_type, suffix, index);
    if resource_type == "EBS" {
        types::ResultItem::EBSResultItem(types::EBSResultItem {
            file_path: Some(name),
            file_size: Some(1024 * (index as i64 + 1)),
            ..Default::default()
        })
    } else {
        types::ResultItem::S3ResultItem(types::S3ResultItem {
            object_key: Some(name),
            object_size: Some(1024 * (index as i64 + 1)),
            ..Default::default()
        })
    }
}

/// One recovery point of this account's backup index as the store holds it: the status its
/// backup index is in, the message that status comes with, and the `items` items the index
/// holds.
fn indexed(
    suffix: &str,
    resource_type: &str,
    index_status: Option<&str>,
    status_message: Option<&str>,
    items: usize,
) -> state::IndexedRecoveryPoint {
    state::IndexedRecoveryPoint {
        backup: types::SearchJobBackupsResult {
            status: index_status.map(ToString::to_string),
            status_message: status_message.map(ToString::to_string),
            resource_type: Some(resource_type.to_string()),
            backup_resource_arn: Some(backup_arn(suffix)),
            source_resource_arn: Some(source_arn(resource_type, suffix)),
            index_creation_time: None,
            backup_creation_time: None,
        },
        backup_vault_name: Some(format!("vault-{suffix}")),
        items: (0..items)
            .map(|index| item(resource_type, suffix, index))
            .collect(),
    }
}

/// The item one `Results` entry names, read THROUGH its union member.
fn result_name(entry: &Value) -> String {
    if let Some(object) = entry.get("S3ResultItem") {
        return object["ObjectKey"].as_str().unwrap_or_default().to_string();
    }
    if let Some(file) = entry.get("EBSResultItem") {
        return file["FilePath"].as_str().unwrap_or_default().to_string();
    }
    String::new()
}

/// The items one `ListSearchJobResults` answer reports, in the order it reports them.
fn result_names(answer: &Answer) -> Vec<String> {
    answer
        .get("Results")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(result_name)
        .collect()
}

/// The recovery point one `Results` entry names.
fn result_backup(entry: &Value) -> String {
    entry
        .get("S3ResultItem")
        .or_else(|| entry.get("EBSResultItem"))
        .and_then(|item| item.get("BackupResourceArn"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the status table: the index status one covered recovery point's record
/// reports, and whether a search reads that recovery point.
struct StatusCase {
    case: &'static str,
    /// `None` is a record that reports no index status at all.
    status: Option<&'static str>,
    searched: bool,
}

/// The backup-index statuses the rule is documented in
/// (`smithy_query(query="doc", shape="ListSearchJobBackups")`: only an index with a status
/// of `ACTIVE` is included in search results, and the `FAILED` one from a permissions
/// issue is displayed instead), beside the two AWS Backup also reports for an index that
/// is being built or taken down.
const INDEX_PENDING: &str = "PENDING";
const INDEX_DELETING: &str = "DELETING";

/// One row per index status a record can report: every value of the model's
/// `SearchJobState` enum, which is the type `SearchJobBackupsResult$Status` carries
/// (`types::SEARCH_JOB_STATE_VALUES`, pinned row-for-row by
/// [`the_status_table_covers_every_modelled_search_job_state`]), the backup-index statuses
/// the rule is stated in, a status outside both vocabularies, and no status at all.
fn status_cases() -> Vec<StatusCase> {
    vec![
        StatusCase {
            case: "a built index, in the vocabulary the rule is documented in",
            status: Some(state::INDEX_ACTIVE),
            searched: true,
        },
        StatusCase {
            case: "a built index, in the member enum's own vocabulary",
            status: Some(state::COMPLETED),
            searched: true,
        },
        StatusCase {
            case: "an index that failed, the case the documentation names",
            status: Some(state::FAILED),
            searched: false,
        },
        StatusCase {
            case: "an index still being built",
            status: Some(INDEX_PENDING),
            searched: false,
        },
        StatusCase {
            case: "an index being taken down",
            status: Some(INDEX_DELETING),
            searched: false,
        },
        StatusCase {
            case: "an index reported as still running",
            status: Some(state::RUNNING),
            searched: false,
        },
        StatusCase {
            case: "an index reported as winding down",
            status: Some(state::STOPPING),
            searched: false,
        },
        StatusCase {
            case: "an index reported as stopped",
            status: Some(state::STOPPED),
            searched: false,
        },
        StatusCase {
            case: "a status in neither vocabulary",
            status: Some("SOMETHING_ELSE"),
            searched: false,
        },
        StatusCase {
            case: "a record reporting no index status at all",
            status: None,
            searched: false,
        },
    ]
}

/// The items each recovery point below holds, so a row that is read and a row that is not
/// differ by more than one item.
const ITEMS_PER_BACKUP: usize = 2;

/// The message a record carrying a non-readable index status is seeded with, which is what
/// `ListSearchJobBackups` has to display for it.
const WHY_LEFT_OUT: &str = "the backup index could not be read";

#[test]
fn the_status_table_covers_every_modelled_search_job_state() {
    // The catalog's source: the model's own enum, so a value added to it cannot stay
    // untested by the table above.
    let rows: Vec<Option<&str>> = status_cases().iter().map(|row| row.status).collect();
    for status in types::SEARCH_JOB_STATE_VALUES {
        assert!(
            rows.contains(&Some(*status)),
            "case {status:?}: every modelled SearchJobState value must be a row of the status table, got {rows:?}"
        );
    }
}

#[tokio::test]
async fn only_a_covered_backup_whose_index_is_readable_is_scanned_counted_and_paged() {
    // One emulator per row, so each row's store holds exactly the index it seeds. Every
    // job is started BEFORE the single sleep, so the clock makes every row's scan
    // completion due at once instead of once per row.
    let mut rows: Vec<(StatusCase, MultiServiceRouter, String)> = Vec::new();
    for case in status_cases() {
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(
            &engine,
            vec![
                // A backup every row reads, so a row's expectations are about ITS status
                // and not about the store being empty.
                indexed(
                    "ready",
                    "S3",
                    Some(state::INDEX_ACTIVE),
                    None,
                    ITEMS_PER_BACKUP,
                ),
                indexed(
                    "probe",
                    "S3",
                    case.status,
                    Some(WHY_LEFT_OUT),
                    ITEMS_PER_BACKUP,
                ),
            ],
        );
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/search-jobs",
            Some(json!({"Name": "index-status", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
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
        let StatusCase {
            case: name,
            status,
            searched,
        } = case;
        let scanned_backups = 1 + u64::from(searched);
        let scanned_items = scanned_backups * ITEMS_PER_BACKUP as u64;
        let mut expected_items: Vec<String> = (0..ITEMS_PER_BACKUP)
            .map(|index| item_name("S3", "ready", index))
            .collect();
        if searched {
            expected_items.extend(
                (0..ITEMS_PER_BACKUP).map(|index| item_name("S3", "probe", index)),
            );
        }

        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        let summary = fetched.get("SearchScopeSummary").clone();
        let progress = fetched.get("CurrentSearchProgress").clone();
        assert_eq!(
            summary["TotalRecoveryPointsToScanCount"].as_u64(),
            Some(scanned_backups),
            "case {name:?}: only a covered backup a search can read is to be scanned, got {summary:?}"
        );
        assert_eq!(
            summary["TotalItemsToScanCount"].as_u64(),
            Some(scanned_items),
            "case {name:?}: the items to scan must be those of the readable backups, got {summary:?}"
        );
        assert_eq!(
            progress["RecoveryPointsScannedCount"].as_u64(),
            Some(scanned_backups),
            "case {name:?}: a completed scan has got through the backups it could read, got {progress:?}"
        );
        assert_eq!(
            progress["ItemsScannedCount"].as_u64(),
            Some(scanned_items),
            "case {name:?}: a completed scan has got through those backups' items, got {progress:?}"
        );
        assert_eq!(
            progress["ItemsMatchedCount"].as_u64(),
            Some(expected_items.len() as u64),
            "case {name:?}: an item of a backup the search did not read cannot be matched, got {progress:?}"
        );

        let listed = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/search-results"),
            None,
        )
        .await;
        assert_eq!(
            result_names(&listed),
            expected_items,
            "case {name:?}: the results must hold only the items of the backups the search read, got {:?}",
            listed.body
        );
        let probe = backup_arn("probe");
        for entry in listed.get("Results").as_array().cloned().unwrap_or_default() {
            assert!(
                searched || result_backup(&entry) != probe,
                "case {name:?}: no result may name the backup the search left out ({probe}), got {entry:?}"
            );
        }

        // The display path: both covered backups are reported, each with the status its
        // index was in and the message that status came with.
        let backups = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/backups"),
            None,
        )
        .await;
        let results = backups.get("Results").as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            2,
            "case {name:?}: every covered backup is displayed, read or not, got {:?}",
            backups.body
        );
        let displayed = results
            .iter()
            .find(|entry| entry["BackupResourceArn"].as_str() == Some(probe.as_str()))
            .unwrap_or_else(|| panic!("case {name:?}: the covered backup {probe} must be displayed, got {:?}", backups.body));
        assert_eq!(
            displayed["Status"].as_str(),
            status,
            "case {name:?}: the display must report the status the index was in, got {displayed:?}"
        );
        assert_eq!(
            displayed["StatusMessage"].as_str(),
            Some(WHY_LEFT_OUT),
            "case {name:?}: the display must report the message that status came with, got {displayed:?}"
        );
    }
}

/// One row of the filter table: the `ItemFilters` a scan over the mixed EBS index runs
/// with, and the files it must therefore record.
struct FilterCase {
    case: &'static str,
    filters: Option<Value>,
    matched: Vec<String>,
}

/// The one file of the readable EBS backup below.
fn readable_file() -> String {
    item_name("EBS", "ready", 0)
}

fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "no item filters at all, which constrain nothing",
            filters: None,
            matched: vec![readable_file()],
        },
        FilterCase {
            case: "a path prefix naming only the files of the backup the search left out",
            filters: Some(json!({"EBSItemFilters": [{
                "FilePaths": [{"Value": "/unreadable/", "Operator": "BEGINS_WITH"}]
            }]})),
            matched: Vec::new(),
        },
        FilterCase {
            case: "a path naming a file of the backup the search left out exactly",
            filters: Some(json!({"EBSItemFilters": [{
                "FilePaths": [{"Value": item_name("EBS", "unreadable", 1)}]
            }]})),
            matched: Vec::new(),
        },
        FilterCase {
            case: "a path prefix naming the file of the backup the search read",
            filters: Some(json!({"EBSItemFilters": [{
                "FilePaths": [{"Value": "/ready/", "Operator": "BEGINS_WITH"}]
            }]})),
            matched: vec![readable_file()],
        },
        FilterCase {
            case: "a size every indexed file satisfies",
            filters: Some(json!({"EBSItemFilters": [{
                "Sizes": [{"Value": 0, "Operator": "GREATER_THAN_EQUAL_TO"}]
            }]})),
            matched: vec![readable_file()],
        },
    ]
}

#[tokio::test]
async fn an_item_filter_cannot_match_an_item_of_a_backup_the_search_left_out() {
    // The index every row searches: an unreadable backup holding three files FIRST, so a
    // scan that read it would report its files before the readable backup's one, and a
    // readable backup holding one.
    let index = || {
        vec![
            indexed("unreadable", "EBS", Some(state::FAILED), Some(WHY_LEFT_OUT), 3),
            indexed("ready", "EBS", Some(state::INDEX_ACTIVE), None, 1),
        ]
    };

    let mut rows: Vec<(FilterCase, MultiServiceRouter, String)> = Vec::new();
    for case in filter_cases() {
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(&engine, index());
        let mut body = json!({"Name": "filters", "SearchScope": {"BackupResourceTypes": ["EBS"]}});
        if let Some(filters) = case.filters.clone() {
            body["ItemFilters"] = filters;
        }
        let started = call(&emulator, REGION, "PUT", "/search-jobs", Some(body)).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartSearchJob must accept the request, got {} ({:?})",
            case.case, started.status, started.body
        );
        rows.push((case, emulator, started.string("SearchJobIdentifier")));
    }

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

    for (case, emulator, identifier) in rows {
        let FilterCase {
            case: name,
            matched,
            ..
        } = case;
        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        let summary = fetched.get("SearchScopeSummary").clone();
        assert_eq!(
            summary["TotalItemsToScanCount"].as_u64(),
            Some(1),
            "case {name:?}: the three files of the unreadable backup are not to be scanned, got {summary:?}"
        );
        assert_eq!(
            fetched.get("CurrentSearchProgress")["ItemsMatchedCount"].as_u64(),
            Some(matched.len() as u64),
            "case {name:?}: the matched count counts only the files the scan read, got {:?}",
            fetched.get("CurrentSearchProgress")
        );

        let listed = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/search-results"),
            None,
        )
        .await;
        assert_eq!(
            result_names(&listed),
            matched,
            "case {name:?}: a filter naming a file of the unreadable backup matches nothing, got {:?}",
            listed.body
        );
    }
}

#[tokio::test]
async fn a_scope_whose_every_covered_backup_is_unreadable_scans_nothing_and_displays_them_all() {
    // The rule holds for a scope in which NOTHING is readable too: the covered backups are
    // still covered — the scope resolved to them and the display lists them — so the scan
    // runs, reads none of them and records nothing.
    let (emulator, engine) = emulator_with_engine();
    let covered = [
        ("failed", Some(state::FAILED), 2usize),
        ("stopped", Some(state::STOPPED), 1),
        ("unreported", None, 3),
    ];
    seed_indexed_recovery_points(
        &engine,
        covered
            .iter()
            .map(|(suffix, status, items)| indexed(suffix, "S3", *status, Some(WHY_LEFT_OUT), *items))
            .collect(),
    );
    let started = call(
        &emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "none-readable", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case \"no readable backup\": StartSearchJob must accept the request, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("SearchJobIdentifier");

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

    let fetched = call(
        &emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await;
    let summary = fetched.get("SearchScopeSummary").clone();
    assert_eq!(
        summary["TotalRecoveryPointsToScanCount"].as_u64(),
        Some(0),
        "case \"no readable backup\": none of the covered backups is to be scanned, got {summary:?}"
    );
    assert_eq!(
        summary["TotalItemsToScanCount"].as_u64(),
        Some(0),
        "case \"no readable backup\": none of their items is to be scanned, got {summary:?}"
    );
    assert_eq!(
        fetched.get("CurrentSearchProgress")["ItemsMatchedCount"].as_u64(),
        Some(0),
        "case \"no readable backup\": nothing can be matched, got {:?}",
        fetched.get("CurrentSearchProgress")
    );
    assert_eq!(
        fetched.string("Status"),
        state::COMPLETED,
        "case \"no readable backup\": the scope resolved to backups, so the scan is not the empty-scope failure, got {:?}",
        fetched.body
    );

    let listed = call(
        &emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}/search-results"),
        None,
    )
    .await;
    assert_eq!(
        result_names(&listed),
        Vec::<String>::new(),
        "case \"no readable backup\": no result may be drawn from a backup the search did not read, got {:?}",
        listed.body
    );

    let backups = call(
        &emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}/backups"),
        None,
    )
    .await;
    let results = backups.get("Results").as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        covered.len(),
        "case \"no readable backup\": every covered backup is displayed, got {:?}",
        backups.body
    );
    for ((suffix, status, _), entry) in covered.iter().zip(results.iter()) {
        assert_eq!(
            entry["BackupResourceArn"].as_str(),
            Some(backup_arn(suffix).as_str()),
            "case {suffix:?}: the display reports the covered backups in scope order, got {entry:?}"
        );
        assert_eq!(
            entry["Status"].as_str(),
            *status,
            "case {suffix:?}: the display reports the status the index was in, got {entry:?}"
        );
        assert_eq!(
            entry["StatusMessage"].as_str(),
            Some(WHY_LEFT_OUT),
            "case {suffix:?}: the display reports the message that status came with, got {entry:?}"
        );
    }
}
