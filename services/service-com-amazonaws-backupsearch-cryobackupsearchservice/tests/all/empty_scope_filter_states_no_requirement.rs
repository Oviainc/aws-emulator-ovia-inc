// SPDX-License-Identifier: CC0-1.0

//! What a `SearchScope` filter that is PRESENT but states no constraint means, driven over
//! HTTP exactly as a client drives it: `StartSearchJob`, then `GetSearchJob`,
//! `ListSearchJobBackups` and `ListSearchJobResults`.
//!
//! The rule under test: an optional scope filter a request carries but that constrains
//! nothing — an empty ARN list, an empty tag map, a creation window with neither bound set,
//! all of which the model allows — states no requirement, so it admits every recovery
//! point, including one whose record carries nothing to judge that filter against; only a
//! filter that states a constraint selects, and then a record that cannot be shown to meet
//! it is out of scope.
//!
//! Every assertion is made over a TABLE whose rows differ in what the rule generalises over
//! — which filter is present-but-empty, whether the record carries the property that filter
//! would be judged on, which bounds the window states, and the resource type of the backup
//! — never over one request.
//!
//! The HTTP harness (`emulator_with_engine`, `call`) and the seeding helpers are the
//! search-job suite's: every suite drives the same emulator the same way.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, recovery_point, seed_indexed_recovery_points,
    settle_a_span_ahead, PAST_THE_DUE_INSTANT,
};

/// The region every call in this suite is signed for.
const REGION: &str = "us-west-2";

/// The instant the seeded backups were taken at, as whole seconds since the epoch, so a
/// creation-window bound can be written around it exactly.
const BACKED_UP_AT_SECS: u64 = 1_760_000_000;

/// The instant [`BACKED_UP_AT_SECS`] names.
fn backed_up_at() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(BACKED_UP_AT_SECS)
}

/// The recovery-point ARN of the backup `suffix` names.
fn backup_arn(suffix: &str) -> String {
    format!(
        "arn:aws:backup:{REGION}:{}:recovery-point:rp-{suffix}",
        engine::emulator_account_id()
    )
}

/// The source resource the backup `suffix` was taken of: a bucket for S3, a volume for EBS.
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

/// The name of the one item the backup `suffix` holds.
fn item_name(resource_type: &str, suffix: &str) -> String {
    if resource_type == "EBS" {
        format!("/{suffix}/file-0")
    } else {
        format!("{suffix}/object-0")
    }
}

/// One recovery point as the store holds it: a readable backup index holding exactly one
/// item, and a record that carries the instant the backup was created only when `created`
/// names one — `SearchJobBackupsResult$BackupCreationTime` is optional in the model, so a
/// record carrying none is a legitimate record.
fn held(
    resource_type: &str,
    suffix: &str,
    created: Option<SystemTime>,
) -> state::IndexedRecoveryPoint {
    let mut backup = recovery_point(
        resource_type,
        &backup_arn(suffix),
        &source_arn(resource_type, suffix),
        created.unwrap_or_else(backed_up_at),
    );
    backup.status = Some(state::INDEX_ACTIVE.to_string());
    backup.backup_creation_time = created.map(state::timestamp);
    let name = item_name(resource_type, suffix);
    let item = if resource_type == "EBS" {
        types::ResultItem::EBSResultItem(types::EBSResultItem {
            file_path: Some(name),
            file_size: Some(2048),
            ..Default::default()
        })
    } else {
        types::ResultItem::S3ResultItem(types::S3ResultItem {
            object_key: Some(name),
            object_size: Some(2048),
            ..Default::default()
        })
    };
    state::IndexedRecoveryPoint {
        backup,
        backup_vault_name: Some(format!("vault-{suffix}")),
        items: vec![item],
    }
}

/// One row of the filter table: the one recovery point the row's store holds, the scope a
/// job is started with, and whether that scope selects the recovery point.
struct FilterCase {
    case: &'static str,
    resource_type: &'static str,
    /// Whether the held record carries the instant its backup was created.
    records_creation_time: bool,
    scope: Value,
    in_scope: bool,
}

fn filter_cases() -> Vec<FilterCase> {
    // `ResourceType` accepts exactly S3 and EBS and `BackupResourceTypes` is
    // @length(min: 1, max: 1), so a scope names one of them; `BackupResourceCreationTime`
    // declares BOTH of its bounds optional, `SourceResourceArns`/`BackupResourceArns` are
    // @length(min: 0, ...) and `BackupResourceTags` is a map — so an empty value of each is
    // a value a request may send (smithy_query(query="shape", shape="SearchScope") and
    // shape="BackupCreationTimeFilter").
    vec![
        FilterCase {
            case: "no creation-time filter at all, record carrying a creation time",
            resource_type: "S3",
            records_creation_time: true,
            scope: json!({"BackupResourceTypes": ["S3"]}),
            in_scope: true,
        },
        FilterCase {
            case: "no creation-time filter at all, record carrying none",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({"BackupResourceTypes": ["S3"]}),
            in_scope: true,
        },
        FilterCase {
            case: "a window stating neither bound, record carrying a creation time",
            resource_type: "S3",
            records_creation_time: true,
            scope: json!({"BackupResourceTypes": ["S3"], "BackupResourceCreationTime": {}}),
            in_scope: true,
        },
        FilterCase {
            case: "a window stating neither bound, record carrying none",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({"BackupResourceTypes": ["S3"], "BackupResourceCreationTime": {}}),
            in_scope: true,
        },
        FilterCase {
            case: "a window stating neither bound, EBS record carrying none",
            resource_type: "EBS",
            records_creation_time: false,
            scope: json!({"BackupResourceTypes": ["EBS"], "BackupResourceCreationTime": {}}),
            in_scope: true,
        },
        FilterCase {
            case: "every optional filter present but empty, record carrying none",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {},
                "BackupResourceArns": [],
                "SourceResourceArns": [],
                "BackupResourceTags": {}
            }),
            in_scope: true,
        },
        FilterCase {
            case: "every optional filter present but empty, record carrying a creation time",
            resource_type: "EBS",
            records_creation_time: true,
            scope: json!({
                "BackupResourceTypes": ["EBS"],
                "BackupResourceCreationTime": {},
                "BackupResourceArns": [],
                "SourceResourceArns": [],
                "BackupResourceTags": {}
            }),
            in_scope: true,
        },
        FilterCase {
            case: "a window stating only CreatedAfter, at the record's own instant",
            resource_type: "S3",
            records_creation_time: true,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedAfter": BACKED_UP_AT_SECS}
            }),
            in_scope: true,
        },
        FilterCase {
            case: "a window stating only CreatedAfter, one second past the record's instant",
            resource_type: "S3",
            records_creation_time: true,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedAfter": BACKED_UP_AT_SECS + 1}
            }),
            in_scope: false,
        },
        FilterCase {
            case: "a window stating only CreatedAfter, record carrying no creation time",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedAfter": BACKED_UP_AT_SECS}
            }),
            in_scope: false,
        },
        FilterCase {
            case: "a window stating only CreatedBefore, record carrying no creation time",
            resource_type: "EBS",
            records_creation_time: false,
            scope: json!({
                "BackupResourceTypes": ["EBS"],
                "BackupResourceCreationTime": {"CreatedBefore": BACKED_UP_AT_SECS}
            }),
            in_scope: false,
        },
        FilterCase {
            case: "a window stating both bounds around the record's instant",
            resource_type: "S3",
            records_creation_time: true,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {
                    "CreatedAfter": BACKED_UP_AT_SECS - 3600,
                    "CreatedBefore": BACKED_UP_AT_SECS + 3600
                }
            }),
            in_scope: true,
        },
        FilterCase {
            case: "a window stating both bounds, record carrying no creation time",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {
                    "CreatedAfter": BACKED_UP_AT_SECS - 3600,
                    "CreatedBefore": BACKED_UP_AT_SECS + 3600
                }
            }),
            in_scope: false,
        },
        FilterCase {
            case: "an empty window beside an ARN list naming the held recovery point",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {},
                "BackupResourceArns": [backup_arn("probe")]
            }),
            in_scope: true,
        },
        FilterCase {
            case: "an empty window beside an ARN list naming another recovery point",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {},
                "BackupResourceArns": [backup_arn("other")]
            }),
            in_scope: false,
        },
        FilterCase {
            case: "an empty window beside a tag filter no record can be shown to carry",
            resource_type: "S3",
            records_creation_time: false,
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {},
                "BackupResourceTags": {"team": "a"}
            }),
            in_scope: false,
        },
    ]
}

#[tokio::test]
async fn a_scope_filter_that_states_no_constraint_admits_every_recovery_point() {
    // One emulator per row, so each row's store holds exactly the recovery point it seeds.
    // The transitions a started job is owed fall due at STORED instants, so each row
    // carries the clock to them rather than the table waiting them out.
    let mut rows: Vec<(FilterCase, MultiServiceRouter, String)> = Vec::new();
    for case in filter_cases() {
        let (emulator, engine) = emulator_with_engine();
        let created = case.records_creation_time.then(backed_up_at);
        seed_indexed_recovery_points(&engine, vec![held(case.resource_type, "probe", created)]);
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/search-jobs",
            Some(json!({"Name": "empty-filter", "SearchScope": case.scope.clone()})),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartSearchJob must accept the scope, got {} ({:?})",
            case.case, started.status, started.body
        );
        settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);
        rows.push((case, emulator, started.string("SearchJobIdentifier")));
    }

    for (case, emulator, identifier) in rows {
        let FilterCase {
            case: name,
            resource_type,
            in_scope,
            ..
        } = case;
        let covered = u64::from(in_scope);
        // The terminal status follows the RESOLVED scope: a scope that selected the held
        // recovery point runs to completion, and only one that selected nothing is the
        // empty-scope failure.
        let (expected_status, expected_message) = if in_scope {
            (state::COMPLETED, Value::Null)
        } else {
            (state::FAILED, Value::String(state::SCOPE_EMPTY.to_string()))
        };

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
            "case {name:?}: expected Status {expected_status}, got {:?}",
            fetched.get("Status")
        );
        assert_eq!(
            fetched.get("StatusMessage"),
            &expected_message,
            "case {name:?}: expected StatusMessage {expected_message:?}, got {:?}",
            fetched.get("StatusMessage")
        );
        assert_eq!(
            fetched.get("SearchScopeSummary")["TotalRecoveryPointsToScanCount"].as_u64(),
            Some(covered),
            "case {name:?}: expected {covered} recovery point(s) to scan, got {:?}",
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
            Some(covered as usize),
            "case {name:?}: expected ListSearchJobBackups to report {covered} recovery point(s), got {:?}",
            backups.body
        );

        let listed = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/search-results"),
            None,
        )
        .await;
        let expected_items: Vec<String> = if in_scope {
            vec![item_name(resource_type, "probe")]
        } else {
            Vec::new()
        };
        assert_eq!(
            result_names(&listed),
            expected_items,
            "case {name:?}: expected ListSearchJobResults to page {expected_items:?}, got {:?}",
            listed.body
        );
    }
}

/// The items one `ListSearchJobResults` answer reports, in the order it reports them, read
/// THROUGH the union member each entry carries.
fn result_names(answer: &crate::search_jobs::Answer) -> Vec<String> {
    answer
        .get("Results")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            if let Some(object) = entry.get("S3ResultItem") {
                return object["ObjectKey"].as_str().unwrap_or_default().to_string();
            }
            if let Some(file) = entry.get("EBSResultItem") {
                return file["FilePath"].as_str().unwrap_or_default().to_string();
            }
            String::new()
        })
        .collect()
}

#[tokio::test]
async fn an_empty_creation_window_resolves_the_same_scope_as_no_window_at_all() {
    // The invariant behind the rule, over a store holding a MIX of records — some carrying
    // the instant their backup was created, some carrying none, of both resource types: a
    // window that states no bound cannot change which recovery points a scope resolves to,
    // so the two jobs below must report the same scope, the same backups and the same
    // items.
    for resource_type in ["S3", "EBS"] {
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(
            &engine,
            vec![
                held(resource_type, "timed", Some(backed_up_at())),
                held(resource_type, "untimed", None),
                held(resource_type, "later", Some(backed_up_at() + Duration::from_secs(7200))),
                held(resource_type, "untimed-too", None),
            ],
        );

        let mut jobs: Vec<(&str, String)> = Vec::new();
        for (case, scope) in [
            ("no window at all", json!({"BackupResourceTypes": [resource_type]})),
            (
                "a window stating neither bound",
                json!({
                    "BackupResourceTypes": [resource_type],
                    "BackupResourceCreationTime": {}
                }),
            ),
        ] {
            let started = call(
                &emulator,
                REGION,
                "PUT",
                "/search-jobs",
                Some(json!({"Name": case, "SearchScope": scope})),
            )
            .await;
            assert_eq!(
                started.status, 200,
                "case {case:?} ({resource_type}): StartSearchJob must accept the scope, got {} ({:?})",
                started.status, started.body
            );
            jobs.push((case, started.string("SearchJobIdentifier")));
        }

        settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);

        let mut answers: Vec<(&str, u64, usize, Vec<String>)> = Vec::new();
        for (case, identifier) in jobs {
            let fetched = call(
                &emulator,
                REGION,
                "GET",
                &format!("/search-jobs/{identifier}"),
                None,
            )
            .await;
            let covered = fetched.get("SearchScopeSummary")["TotalRecoveryPointsToScanCount"]
                .as_u64()
                .unwrap_or_default();
            assert_eq!(
                fetched.string("Status"),
                state::COMPLETED,
                "case {case:?} ({resource_type}): every held recovery point is in scope, so the scan completes, got {:?}",
                fetched.body
            );
            let backups = call(
                &emulator,
                REGION,
                "GET",
                &format!("/search-jobs/{identifier}/backups"),
                None,
            )
            .await;
            let reported = backups.get("Results").as_array().map_or(0, Vec::len);
            let listed = call(
                &emulator,
                REGION,
                "GET",
                &format!("/search-jobs/{identifier}/search-results"),
                None,
            )
            .await;
            answers.push((case, covered, reported, result_names(&listed)));
        }

        let (base_case, base_covered, base_reported, base_items) = answers[0].clone();
        assert_eq!(
            base_covered, 4,
            "case {base_case:?} ({resource_type}): expected all 4 held recovery points in scope, got {base_covered}"
        );
        for (case, covered, reported, items) in &answers[1..] {
            assert_eq!(
                (*covered, *reported, items.clone()),
                (base_covered, base_reported, base_items.clone()),
                "case {case:?} ({resource_type}): expected the same resolved scope as {base_case:?}, got {covered} to scan, {reported} reported and {items:?}"
            );
        }
    }
}
