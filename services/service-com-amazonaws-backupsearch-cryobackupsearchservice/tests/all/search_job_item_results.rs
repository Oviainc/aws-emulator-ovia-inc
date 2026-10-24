// SPDX-License-Identifier: CC0-1.0

//! The ITEM layer of a search job, driven over HTTP exactly as a client drives it:
//! `StartSearchJob` with `ItemFilters`, then `GetSearchJob` and `ListSearchJobResults`.
//!
//! The rule under test: a scan gets through the items the backup indexes of the recovery
//! points in scope hold — the objects of a backed-up bucket, the files of a backed-up
//! volume — counts them as the items to scan, and records as its results exactly those
//! items its `ItemFilters` admit, which `ListSearchJobResults` then pages and
//! `CurrentSearchProgress.ItemsMatchedCount` counts.
//!
//! Every assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — item type (S3 objects, EBS files), which item property is filtered, the
//! comparison operator (every value of all three condition-operator enums the model
//! declares), how many conditions and how many filter objects are given, how many items
//! and recovery points are indexed, and the page size — never over one request.

use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, recovery_point, seed_indexed_recovery_points, Answer,
    PAST_THE_DUE_INSTANT, WITHIN_THE_DUE_INSTANT,
};

const REGION: &str = "us-west-2";

/// The instant the indexed items below were created, as whole epoch seconds, with the
/// offsets that separate them. A `TimeCondition` compares instants, so a row has to be
/// able to name one exactly.
const CREATED_AT_SECS: u64 = 1_762_000_000;

/// The two S3 recovery points and the one EBS recovery point this suite indexes, with the
/// vault each lives in.
const ALPHA_BACKUP: &str = "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-alpha";
const ALPHA_SOURCE: &str = "arn:aws:s3:::alpha";
const ALPHA_VAULT: &str = "alpha-vault";
const BETA_BACKUP: &str = "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-beta";
const BETA_SOURCE: &str = "arn:aws:s3:::beta";
const BETA_VAULT: &str = "beta-vault";
const VOLUME_BACKUP: &str = "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-volume";
const VOLUME_SOURCE: &str = "arn:aws:ec2:us-west-2:000000000000:volume/vol-1";
const VOLUME_VAULT: &str = "volume-vault";

/// An instant `offset` seconds after [`CREATED_AT_SECS`], in the millisecond-truncated
/// form the store holds its timestamps in.
fn at(offset: u64) -> aws_smithy_types::DateTime {
    state::timestamp(UNIX_EPOCH + Duration::from_secs(CREATED_AT_SECS + offset))
}

/// One object a backed-up bucket holds, as its backup index holds it: the properties an
/// `S3ItemFilter` selects on (key, size, creation time, ETag, version id). The members
/// naming the BACKUP are left to the recovery point record the item is indexed in.
fn object(key: &str, size: i64, created: u64, e_tag: &str, version_id: &str) -> types::ResultItem {
    types::ResultItem::S3ResultItem(types::S3ResultItem {
        object_key: Some(key.to_string()),
        object_size: Some(size),
        creation_time: Some(at(created)),
        e_tag: Some(e_tag.to_string()),
        version_id: Some(version_id.to_string()),
        ..Default::default()
    })
}

/// One file a backed-up volume holds, as its backup index holds it: the properties an
/// `EBSItemFilter` selects on (path, size, creation time, last modification time).
fn file(path: &str, size: i64, created: u64, modified: u64) -> types::ResultItem {
    types::ResultItem::EBSResultItem(types::EBSResultItem {
        file_path: Some(path.to_string()),
        file_size: Some(size),
        creation_time: Some(at(created)),
        last_modified_time: Some(at(modified)),
        ..Default::default()
    })
}

/// A recovery point of the backup index together with the items it holds, which is how
/// the store holds the two layers of a scan on one record.
fn indexed(
    resource_type: &str,
    backup_resource_arn: &str,
    source_resource_arn: &str,
    vault: &str,
    items: Vec<types::ResultItem>,
) -> state::IndexedRecoveryPoint {
    state::IndexedRecoveryPoint {
        backup: recovery_point(
            resource_type,
            backup_resource_arn,
            source_resource_arn,
            UNIX_EPOCH + Duration::from_secs(CREATED_AT_SECS),
        ),
        backup_vault_name: Some(vault.to_string()),
        items,
    }
}

/// The backup index every row below is scanned against: two S3 recovery points holding
/// four objects between them and one EBS recovery point holding three files. An S3 scope
/// therefore has four items to scan and an EBS scope three, in recovery-point and then
/// index order.
fn backup_index() -> Vec<state::IndexedRecoveryPoint> {
    vec![
        indexed(
            "S3",
            ALPHA_BACKUP,
            ALPHA_SOURCE,
            ALPHA_VAULT,
            vec![
                object("reports/2026/q1.csv", 1024, 0, "etag-a", "v1"),
                object("reports/2026/q2.csv", 2048, 60, "etag-b", "v2"),
                object("logs/app.log", 512, 120, "etag-c", "v1"),
            ],
        ),
        indexed(
            "S3",
            BETA_BACKUP,
            BETA_SOURCE,
            BETA_VAULT,
            vec![object("reports/2025/annual.csv", 4096, 180, "etag-d", "v3")],
        ),
        indexed(
            "EBS",
            VOLUME_BACKUP,
            VOLUME_SOURCE,
            VOLUME_VAULT,
            vec![
                file("/var/log/syslog", 2048, 0, 300),
                file("/etc/hosts", 128, 0, 0),
                file("/tmp/scratch.tmp", 64, 0, 0),
            ],
        ),
    ]
}

/// The item one stored index entry names: an object by its key, a file by its path.
fn indexed_name(item: &types::ResultItem) -> String {
    match item {
        types::ResultItem::S3ResultItem(object) => object.object_key.clone().unwrap_or_default(),
        types::ResultItem::EBSResultItem(file) => file.file_path.clone().unwrap_or_default(),
    }
}

/// The item one `Results` entry names, read THROUGH its union member — so an answer that
/// reports an object where a file was indexed cannot satisfy a row.
fn result_name(entry: &Value) -> String {
    if let Some(object) = entry.get("S3ResultItem") {
        return object["ObjectKey"].as_str().unwrap_or_default().to_string();
    }
    if let Some(file) = entry.get("EBSResultItem") {
        return file["FilePath"].as_str().unwrap_or_default().to_string();
    }
    String::new()
}

/// The inner structure of one `Results` entry, whichever item type it carries.
fn result_body(entry: &Value) -> Value {
    entry
        .get("S3ResultItem")
        .or_else(|| entry.get("EBSResultItem"))
        .cloned()
        .unwrap_or(Value::Null)
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

/// The recovery point the item `name` is indexed in, as the seeded store holds it: the
/// recovery-point ARN, the source-resource ARN and the vault a result for it must report.
fn backup_of(name: &str) -> (String, String, String) {
    for point in backup_index() {
        if point.items.iter().any(|item| indexed_name(item) == name) {
            return (
                point.backup.backup_resource_arn.clone().unwrap_or_default(),
                point.backup.source_resource_arn.clone().unwrap_or_default(),
                point.backup_vault_name.clone().unwrap_or_default(),
            );
        }
    }
    panic!("the backup index holds no item named {name:?}");
}

/// One row of the filter table: the scope to search, the `ItemFilters` to search with,
/// how many items that scope has to scan, and the items the scan must therefore record.
struct FilterCase {
    case: &'static str,
    scope: Value,
    filters: Option<Value>,
    to_scan: u64,
    matched: Vec<&'static str>,
}

fn s3_scope() -> Value {
    json!({"BackupResourceTypes": ["S3"]})
}

fn ebs_scope() -> Value {
    json!({"BackupResourceTypes": ["EBS"]})
}

fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "no item filters at all, which constrain nothing",
            scope: s3_scope(),
            filters: None,
            to_scan: 4,
            matched: vec![
                "reports/2026/q1.csv",
                "reports/2026/q2.csv",
                "logs/app.log",
                "reports/2025/annual.csv",
            ],
        },
        FilterCase {
            case: "one ObjectKeys condition with the operator left out (the model's @default is EQUALS_TO)",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [{
                "ObjectKeys": [{"Value": "logs/app.log"}]
            }]})),
            to_scan: 4,
            matched: vec!["logs/app.log"],
        },
        FilterCase {
            case: "an ObjectKeys prefix, which selects the objects under it",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [{
                "ObjectKeys": [{"Value": "reports/2026/", "Operator": "BEGINS_WITH"}]
            }]})),
            to_scan: 4,
            matched: vec!["reports/2026/q1.csv", "reports/2026/q2.csv"],
        },
        FilterCase {
            case: "two conditions on one property, which admit an item matching ANY of them",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [{
                "ObjectKeys": [
                    {"Value": "logs/", "Operator": "BEGINS_WITH"},
                    {"Value": "annual.csv", "Operator": "ENDS_WITH"}
                ]
            }]})),
            to_scan: 4,
            matched: vec!["logs/app.log", "reports/2025/annual.csv"],
        },
        FilterCase {
            case: "two properties in one filter, which admit only an item matching EVERY one",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [{
                "ObjectKeys": [{"Value": "reports/", "Operator": "BEGINS_WITH"}],
                "Sizes": [{"Value": 2048, "Operator": "GREATER_THAN_EQUAL_TO"}]
            }]})),
            to_scan: 4,
            matched: vec!["reports/2026/q2.csv", "reports/2025/annual.csv"],
        },
        FilterCase {
            case: "two filter objects, which admit an item matching ANY of them",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [
                {"ObjectKeys": [{"Value": "logs/app.log"}]},
                {"Sizes": [{"Value": 4096}]}
            ]})),
            to_scan: 4,
            matched: vec!["logs/app.log", "reports/2025/annual.csv"],
        },
        FilterCase {
            case: "an ETag and a version id, the two properties the key filters do not cover",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [{
                "ETags": [{"Value": "etag-a"}],
                "VersionIds": [{"Value": "v1"}]
            }]})),
            to_scan: 4,
            matched: vec!["reports/2026/q1.csv"],
        },
        FilterCase {
            case: "a creation-time floor on objects",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [{
                "CreationTimes": [{
                    "Value": CREATED_AT_SECS + 120,
                    "Operator": "GREATER_THAN_EQUAL_TO"
                }]
            }]})),
            to_scan: 4,
            matched: vec!["logs/app.log", "reports/2025/annual.csv"],
        },
        FilterCase {
            case: "a filter no indexed object matches, over a scope that still had items to scan",
            scope: s3_scope(),
            filters: Some(json!({"S3ItemFilters": [{
                "ObjectKeys": [{"Value": "no/such/object"}]
            }]})),
            to_scan: 4,
            matched: Vec::new(),
        },
        FilterCase {
            case: "an S3 scope filtered with EBS filters, which say nothing about objects",
            scope: s3_scope(),
            filters: Some(json!({"EBSItemFilters": [{
                "FilePaths": [{"Value": "/var", "Operator": "BEGINS_WITH"}]
            }]})),
            to_scan: 4,
            matched: vec![
                "reports/2026/q1.csv",
                "reports/2026/q2.csv",
                "logs/app.log",
                "reports/2025/annual.csv",
            ],
        },
        FilterCase {
            case: "an EBS scope with no filters, which scans the volume's files",
            scope: ebs_scope(),
            filters: None,
            to_scan: 3,
            matched: vec!["/var/log/syslog", "/etc/hosts", "/tmp/scratch.tmp"],
        },
        FilterCase {
            case: "a file-path prefix and a size ceiling, both of which must hold",
            scope: ebs_scope(),
            filters: Some(json!({"EBSItemFilters": [{
                "FilePaths": [{"Value": "/var", "Operator": "BEGINS_WITH"}],
                "Sizes": [{"Value": 2048, "Operator": "LESS_THAN_EQUAL_TO"}]
            }]})),
            to_scan: 3,
            matched: vec!["/var/log/syslog"],
        },
        FilterCase {
            case: "a last-modification instant, the property only a file carries",
            scope: ebs_scope(),
            filters: Some(json!({"EBSItemFilters": [{
                "LastModificationTimes": [{"Value": CREATED_AT_SECS + 300}]
            }]})),
            to_scan: 3,
            matched: vec!["/var/log/syslog"],
        },
        FilterCase {
            case: "a file-path suffix excluded",
            scope: ebs_scope(),
            filters: Some(json!({"EBSItemFilters": [{
                "FilePaths": [{"Value": ".tmp", "Operator": "DOES_NOT_END_WITH"}]
            }]})),
            to_scan: 3,
            matched: vec!["/var/log/syslog", "/etc/hosts"],
        },
    ]
}

#[tokio::test]
async fn a_scan_records_the_indexed_items_its_item_filters_admit() {
    // One emulator per row, so each row's store holds exactly the index it seeds. Every
    // job is started BEFORE the single sleep, so the clock makes every row's scan
    // completion due at once instead of once per row.
    let mut rows: Vec<(FilterCase, MultiServiceRouter, String)> = Vec::new();
    for case in filter_cases() {
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(&engine, backup_index());
        let mut body = json!({"Name": "items", "SearchScope": case.scope.clone()});
        if let Some(filters) = case.filters.clone() {
            body["ItemFilters"] = filters;
        }
        let started = call(&emulator, REGION, "PUT", "/search-jobs", Some(body)).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartSearchJob must accept the request, got {} ({:?})",
            case.case, started.status, started.body
        );
        let identifier = started.string("SearchJobIdentifier");
        rows.push((case, emulator, identifier));
    }

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

    for (case, emulator, identifier) in rows {
        let FilterCase {
            case: name,
            to_scan,
            matched,
            ..
        } = &case;
        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.string("Status"),
            state::COMPLETED,
            "case {name:?}: a scan over a scope that resolved runs to completion, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.get("SearchScopeSummary")["TotalItemsToScanCount"].as_u64(),
            Some(*to_scan),
            "case {name:?}: the scan must count the items the indexes in scope hold, got {:?}",
            fetched.get("SearchScopeSummary")
        );
        assert_eq!(
            fetched.get("CurrentSearchProgress")["ItemsScannedCount"].as_u64(),
            Some(*to_scan),
            "case {name:?}: a completed scan has got through every item in scope, got {:?}",
            fetched.get("CurrentSearchProgress")
        );
        assert_eq!(
            fetched.get("CurrentSearchProgress")["ItemsMatchedCount"].as_u64(),
            Some(matched.len() as u64),
            "case {name:?}: the matched count must count the items the filters admitted, got {:?}",
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
            listed.status, 200,
            "case {name:?}: expected 200 from ListSearchJobResults, got {} ({:?})",
            listed.status, listed.body
        );
        assert_eq!(
            result_names(&listed),
            *matched,
            "case {name:?}: the results must be the admitted items in index order, got {:?}",
            listed.body
        );

        // Every result names the backup it was indexed in, read off that record rather
        // than off the request.
        for entry in listed.get("Results").as_array().cloned().unwrap_or_default() {
            let item = result_name(&entry);
            let (backup_arn, source_arn, vault) = backup_of(&item);
            let body = result_body(&entry);
            assert_eq!(
                body["BackupResourceArn"].as_str(),
                Some(backup_arn.as_str()),
                "case {name:?}: item {item:?} must name the recovery point it was indexed in, got {body:?}"
            );
            assert_eq!(
                body["SourceResourceArn"].as_str(),
                Some(source_arn.as_str()),
                "case {name:?}: item {item:?} must name that backup's source resource, got {body:?}"
            );
            assert_eq!(
                body["BackupVaultName"].as_str(),
                Some(vault.as_str()),
                "case {name:?}: item {item:?} must name the vault holding that backup, got {body:?}"
            );
        }
    }
}

/// One row of the operator tables: the condition to search with and the items it admits.
struct OperatorCase {
    operator: &'static str,
    condition: Value,
    matched: Vec<&'static str>,
}

/// The three objects the operator rows are judged against: distinct keys, sizes and
/// creation instants, so every comparison the model declares separates them differently.
fn operator_index() -> Vec<state::IndexedRecoveryPoint> {
    vec![indexed(
        "S3",
        ALPHA_BACKUP,
        ALPHA_SOURCE,
        ALPHA_VAULT,
        vec![
            object("reports/a.csv", 10, 0, "etag-a", "v1"),
            object("logs/b.txt", 200, 60, "etag-b", "v2"),
            object("reports/c.log", 3000, 120, "etag-c", "v3"),
        ],
    )]
}

/// A row per value of `StringConditionOperator`
/// (`types::STRING_CONDITION_OPERATOR_VALUES`), judged on `ObjectKeys`.
fn string_operator_cases() -> Vec<OperatorCase> {
    vec![
        OperatorCase {
            operator: state::EQUALS_TO,
            condition: json!({"Value": "logs/b.txt", "Operator": state::EQUALS_TO}),
            matched: vec!["logs/b.txt"],
        },
        OperatorCase {
            operator: state::NOT_EQUALS_TO,
            condition: json!({"Value": "logs/b.txt", "Operator": state::NOT_EQUALS_TO}),
            matched: vec!["reports/a.csv", "reports/c.log"],
        },
        OperatorCase {
            operator: state::CONTAINS,
            condition: json!({"Value": "/b.", "Operator": state::CONTAINS}),
            matched: vec!["logs/b.txt"],
        },
        OperatorCase {
            operator: state::DOES_NOT_CONTAIN,
            condition: json!({"Value": "reports/", "Operator": state::DOES_NOT_CONTAIN}),
            matched: vec!["logs/b.txt"],
        },
        OperatorCase {
            operator: state::BEGINS_WITH,
            condition: json!({"Value": "reports/", "Operator": state::BEGINS_WITH}),
            matched: vec!["reports/a.csv", "reports/c.log"],
        },
        OperatorCase {
            operator: state::DOES_NOT_BEGIN_WITH,
            condition: json!({"Value": "reports/", "Operator": state::DOES_NOT_BEGIN_WITH}),
            matched: vec!["logs/b.txt"],
        },
        OperatorCase {
            operator: state::ENDS_WITH,
            condition: json!({"Value": ".csv", "Operator": state::ENDS_WITH}),
            matched: vec!["reports/a.csv"],
        },
        OperatorCase {
            operator: state::DOES_NOT_END_WITH,
            condition: json!({"Value": ".csv", "Operator": state::DOES_NOT_END_WITH}),
            matched: vec!["logs/b.txt", "reports/c.log"],
        },
    ]
}

/// A row per value of `LongConditionOperator`
/// (`types::LONG_CONDITION_OPERATOR_VALUES`), judged on `Sizes` against the middle size.
fn long_operator_cases() -> Vec<OperatorCase> {
    vec![
        OperatorCase {
            operator: state::EQUALS_TO,
            condition: json!({"Value": 200, "Operator": state::EQUALS_TO}),
            matched: vec!["logs/b.txt"],
        },
        OperatorCase {
            operator: state::NOT_EQUALS_TO,
            condition: json!({"Value": 200, "Operator": state::NOT_EQUALS_TO}),
            matched: vec!["reports/a.csv", "reports/c.log"],
        },
        OperatorCase {
            operator: state::LESS_THAN_EQUAL_TO,
            condition: json!({"Value": 200, "Operator": state::LESS_THAN_EQUAL_TO}),
            matched: vec!["reports/a.csv", "logs/b.txt"],
        },
        OperatorCase {
            operator: state::GREATER_THAN_EQUAL_TO,
            condition: json!({"Value": 200, "Operator": state::GREATER_THAN_EQUAL_TO}),
            matched: vec!["logs/b.txt", "reports/c.log"],
        },
    ]
}

/// A row per value of `TimeConditionOperator`
/// (`types::TIME_CONDITION_OPERATOR_VALUES`), judged on `CreationTimes` against the
/// middle instant.
fn time_operator_cases() -> Vec<OperatorCase> {
    let middle = CREATED_AT_SECS + 60;
    vec![
        OperatorCase {
            operator: state::EQUALS_TO,
            condition: json!({"Value": middle, "Operator": state::EQUALS_TO}),
            matched: vec!["logs/b.txt"],
        },
        OperatorCase {
            operator: state::NOT_EQUALS_TO,
            condition: json!({"Value": middle, "Operator": state::NOT_EQUALS_TO}),
            matched: vec!["reports/a.csv", "reports/c.log"],
        },
        OperatorCase {
            operator: state::LESS_THAN_EQUAL_TO,
            condition: json!({"Value": middle, "Operator": state::LESS_THAN_EQUAL_TO}),
            matched: vec!["reports/a.csv", "logs/b.txt"],
        },
        OperatorCase {
            operator: state::GREATER_THAN_EQUAL_TO,
            condition: json!({"Value": middle, "Operator": state::GREATER_THAN_EQUAL_TO}),
            matched: vec!["logs/b.txt", "reports/c.log"],
        },
    ]
}

#[tokio::test]
async fn every_condition_operator_the_model_declares_selects_by_its_own_comparison() {
    // The three tables below must cover their enum exactly: an operator with no row would
    // be an untested comparison, and one that is not in the enum is not an operator.
    for (property, cases, declared) in [
        (
            "ObjectKeys",
            string_operator_cases(),
            types::STRING_CONDITION_OPERATOR_VALUES,
        ),
        ("Sizes", long_operator_cases(), types::LONG_CONDITION_OPERATOR_VALUES),
        (
            "CreationTimes",
            time_operator_cases(),
            types::TIME_CONDITION_OPERATOR_VALUES,
        ),
    ] {
        let mut covered: Vec<&str> = cases.iter().map(|row| row.operator).collect();
        covered.sort_unstable();
        let mut expected: Vec<&str> = declared.to_vec();
        expected.sort_unstable();
        assert_eq!(
            covered, expected,
            "property {property:?}: every declared operator needs a row, got {covered:?}"
        );

        // One emulator holds every row's job: each job carries its own filters, so the
        // store's single index is searched once per operator.
        let (emulator, engine) = emulator_with_engine();
        seed_indexed_recovery_points(&engine, operator_index());
        let mut started: Vec<(OperatorCase, String)> = Vec::new();
        for row in cases {
            let body = json!({
                "SearchScope": {"BackupResourceTypes": ["S3"]},
                "ItemFilters": {"S3ItemFilters": [{property: [row.condition.clone()]}]}
            });
            let answer = call(&emulator, REGION, "PUT", "/search-jobs", Some(body)).await;
            assert_eq!(
                answer.status, 200,
                "property {property:?}, operator {:?}: StartSearchJob must accept the filter, got {} ({:?})",
                row.operator, answer.status, answer.body
            );
            let identifier = answer.string("SearchJobIdentifier");
            started.push((row, identifier));
        }

        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

        for (row, identifier) in started {
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
                row.matched,
                "property {property:?}, operator {:?}: the results must be the items that comparison admits, got {:?}",
                row.operator,
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn the_matches_become_readable_when_the_scan_completes_and_are_paged_by_max_results() {
    // Seven indexed objects, so a page size can divide the sequence evenly, unevenly, or
    // not at all.
    let objects: Vec<types::ResultItem> = (0..7)
        .map(|index| {
            object(
                &format!("item-{index}"),
                100 * (index as i64 + 1),
                index as u64,
                &format!("etag-{index}"),
                "v1",
            )
        })
        .collect();
    let expected: Vec<String> = objects.iter().map(indexed_name).collect();

    let (emulator, engine) = emulator_with_engine();
    seed_indexed_recovery_points(
        &engine,
        vec![indexed(
            "S3",
            ALPHA_BACKUP,
            ALPHA_SOURCE,
            ALPHA_VAULT,
            objects,
        )],
    );
    let started = call(
        &emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "paged", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSearchJob must accept the request, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("SearchJobIdentifier");
    let results_uri = format!("/search-jobs/{identifier}/search-results");

    // A scan still being set up has RECORDED nothing, so the listing and the matched
    // count agree on nothing having been matched yet.
    tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
    let creating = call(&emulator, REGION, "GET", &results_uri, None).await;
    assert_eq!(
        result_names(&creating),
        Vec::<String>::new(),
        "a scan that has not got through its scope has recorded no result, got {:?}",
        creating.body
    );
    let fetched = call(
        &emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await;
    assert_eq!(
        fetched.string("Status"),
        state::CREATING,
        "the job is still being set up at this point, got {:?}",
        fetched.get("Status")
    );
    assert_eq!(
        fetched.get("SearchScopeSummary")["TotalItemsToScanCount"].as_u64(),
        Some(expected.len() as u64),
        "the items to scan are known as soon as the scope is resolved, got {:?}",
        fetched.get("SearchScopeSummary")
    );
    assert_eq!(
        fetched.get("CurrentSearchProgress")["ItemsMatchedCount"].as_u64(),
        Some(0),
        "nothing is matched before the scan has scanned anything, got {:?}",
        fetched.get("CurrentSearchProgress")
    );

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let settled = call(
        &emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await;
    assert_eq!(
        settled.get("CurrentSearchProgress")["ItemsMatchedCount"].as_u64(),
        Some(expected.len() as u64),
        "a completed scan matched every item, no filter having excluded any, got {:?}",
        settled.get("CurrentSearchProgress")
    );

    // The page size the request asks for bounds the page, a full page carries a token
    // (it says nothing about what follows it) and the pages walked end to end are the
    // whole sequence, once, in order.
    for max_results in [1_usize, 2, 3, 7, state::MAX_PAGE_SIZE] {
        let case = format!("MaxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let uri = match &token {
                Some(next) => format!("{results_uri}?maxResults={max_results}&nextToken={next}"),
                None => format!("{results_uri}?maxResults={max_results}"),
            };
            let page = call(&emulator, REGION, "GET", &uri, None).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200 from ListSearchJobResults, got {} ({:?})",
                page.status, page.body
            );
            let names = result_names(&page);
            assert!(
                names.len() <= max_results,
                "case {case:?}: a page must hold at most MaxResults items, got {}",
                names.len()
            );
            let next = page.get("NextToken").as_str().map(ToString::to_string);
            assert_eq!(
                next.is_some(),
                names.len() == max_results,
                "case {case:?}: a page carries a NextToken exactly when it filled MaxResults, got {:?}",
                page.body
            );
            seen.extend(names);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages walked end to end must be every match in index order, got {seen:?}"
        );
    }

    // And reading the results did not change what a later reader sees.
    let again = call(&emulator, REGION, "GET", &results_uri, None).await;
    assert_eq!(
        result_names(&again),
        expected,
        "reading the results must not change what a later caller sees, got {:?}",
        again.body
    );
}
