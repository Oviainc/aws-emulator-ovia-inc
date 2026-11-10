// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for what a search-result export PRODUCES: the results object it writes
//! into the destination bucket, and the terminal status it settles to because of that
//! write.
//!
//! The rule under test is that `StartSearchResultExportJob` "starts a job to export the
//! results of search job to a designated S3 bucket" — so the export writes the search job's
//! recorded results as an object under the `DestinationBucket` and `DestinationPrefix` the
//! request names, resolving that destination where the object store holds it, and the status
//! the clock owes the export is what the write came to: `COMPLETED` for an object the
//! destination took, and the modelled `FAILED` carrying the object store's own refusal for
//! one it did not.
//!
//! Every assertion is therefore made over a TABLE of exports differing in what the rule
//! generalises over — whether a destination prefix is present and what shape it has, which
//! kind of result item the search job recorded and how many, the signing region, and whether
//! the object store holds the destination at all — never over one request alone. The results
//! come from a real scan of a seeded backup index and are read back through the service's own
//! `ListSearchJobResults`, so what the object carries is compared against what the service
//! reports rather than against values this suite made up.
//!
//! Amazon S3 is the one mounted beside this service on the same engine
//! ([`crate::search_jobs::emulator_with_engine`]), reached over the emulator's own in-process
//! callback ([`crate::search_jobs::s3`]) — the very seam the export writes through, so a
//! bucket created here is the bucket the export lands in.

use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_jobs::passable_role;
use crate::search_jobs::{
    call, create_bucket, emulator_with_engine, recovery_point, s3, seed_indexed_recovery_points,
    settle_a_span_ahead, Answer, PAST_THE_DUE_INSTANT, WITHIN_THE_DUE_INSTANT,
};

/// The instant the seeded index entries were created, as whole epoch seconds.
const CREATED_AT_SECS: u64 = 1_762_000_000;

/// The recovery point every row's scope resolves to, and the vault it lives in.
const BACKUP_ARN: &str = "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-exported";
const SOURCE_ARN: &str = "arn:aws:s3:::exported";
const VAULT: &str = "exported-vault";

/// An instant `offset` seconds after [`CREATED_AT_SECS`], in the form the store holds its
/// timestamps in.
fn at(offset: u64) -> ::aws_smithy_types::DateTime {
    state::timestamp(UNIX_EPOCH + Duration::from_secs(CREATED_AT_SECS + offset))
}

/// The backup index one row is scanned against: ONE recovery point of `resource_type`
/// holding `items` indexed items, which is what the scan gets through and therefore what
/// the export has to write.
///
/// The Amazon S3 keys deliberately carry a separator and a quote, because a results row is
/// CSV: a value like that is what tells a framed table from a concatenated one.
fn backup_index(resource_type: &str, items: usize) -> Vec<state::IndexedRecoveryPoint> {
    let items: Vec<types::ResultItem> = (0..items)
        .map(|index| {
            let index = index as i64;
            if resource_type == "EBS" {
                types::ResultItem::EBSResultItem(types::EBSResultItem {
                    file_path: Some(format!("/var/log/app-{index}.log")),
                    file_size: Some(128 * (index + 1)),
                    creation_time: Some(at(0)),
                    last_modified_time: Some(at(index as u64 + 1)),
                    ..Default::default()
                })
            } else {
                types::ResultItem::S3ResultItem(types::S3ResultItem {
                    object_key: Some(format!("reports/\"q{index}\",final.csv")),
                    object_size: Some(1_024 * (index + 1)),
                    creation_time: Some(at(index as u64)),
                    e_tag: Some(format!("etag-{index}")),
                    version_id: Some(format!("v{index}")),
                    ..Default::default()
                })
            }
        })
        .collect();
    vec![state::IndexedRecoveryPoint {
        backup: recovery_point(
            resource_type,
            BACKUP_ARN,
            SOURCE_ARN,
            UNIX_EPOCH + Duration::from_secs(CREATED_AT_SECS),
        ),
        backup_vault_name: Some(VAULT.to_string()),
        items,
    }]
}

/// Start a search job over `resource_type` and let the clock finish its scan, so the job is
/// `COMPLETED` with exactly the results the seeded index holds. Answers its identifier.
async fn completed_search_job(
    emulator: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    resource_type: &str,
) -> String {
    let started = call(
        emulator,
        region,
        "PUT",
        "/search-jobs",
        Some(json!({
            "Name": "to-export",
            "SearchScope": {"BackupResourceTypes": [resource_type]}
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("SearchJobIdentifier");
    settle_a_span_ahead(engine, PAST_THE_DUE_INSTANT);
    let fetched = call(
        emulator,
        region,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await;
    assert_eq!(
        fetched.string("Status"),
        state::COMPLETED,
        "case {case:?}: the scan must complete before the export starts, got {:?}",
        fetched.get("Status")
    );
    identifier
}

/// The results the service pages for `search_job`, which is what the export has to carry.
async fn listed_results(
    emulator: &MultiServiceRouter,
    region: &str,
    case: &str,
    search_job: &str,
) -> Vec<Value> {
    let listed = call(
        emulator,
        region,
        "GET",
        &format!("/search-jobs/{search_job}/search-results"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListSearchJobResults must succeed, got {} ({:?})",
        listed.status, listed.body
    );
    listed
        .get("Results")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `StartSearchResultExportJob` for `search_job`, into `bucket` under `prefix`.
async fn start_export(
    emulator: &MultiServiceRouter,
    region: &str,
    search_job: &str,
    bucket: &str,
    prefix: Option<&str>,
) -> Answer {
    let mut specification = json!({"DestinationBucket": bucket});
    if let Some(prefix) = prefix {
        specification["DestinationPrefix"] = json!(prefix);
    }
    call(
        emulator,
        region,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {"s3ExportSpecification": specification},
            "RoleArn": passable_role("aws", "backup-search-export"),
        })),
    )
    .await
}

/// `GetSearchResultExportJob` for `identifier`.
async fn get_export(emulator: &MultiServiceRouter, region: &str, identifier: &str) -> Answer {
    call(
        emulator,
        region,
        "GET",
        &format!("/export-search-jobs/{identifier}"),
        None,
    )
    .await
}

/// The body of one object of the emulator's own object store, or `None` when it serves no
/// such object.
async fn object_text(
    emulator: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    key: &str,
) -> Option<String> {
    let object = s3(emulator, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .ok()?;
    let bytes = object.body.collect().await.ok()?.into_bytes();
    String::from_utf8(bytes.to_vec()).ok()
}

/// The keys the object store holds in `bucket` under `prefix` — what a client that exports
/// and then lists the prefix sees.
///
/// The listing itself MUST be served: a bucket this is asked about is one the suite created,
/// so a refused listing is a broken row rather than an empty prefix, and an empty answer
/// therefore means the object store really holds nothing there.
async fn listed_keys(
    emulator: &MultiServiceRouter,
    case: &str,
    region: &str,
    bucket: &str,
    prefix: Option<&str>,
) -> Vec<String> {
    let mut request = s3(emulator, region).list_objects_v2().bucket(bucket);
    if let Some(prefix) = prefix {
        request = request.prefix(prefix);
    }
    let answer = request.send().await.unwrap_or_else(|error| {
        panic!("case {case:?}: s3:ListObjectsV2 must serve {bucket:?}, got {error:?}")
    });
    answer
        .contents()
        .iter()
        .filter_map(|object| object.key().map(ToString::to_string))
        .collect()
}

/// The rows of the exported table, as the `csv` crate reads them back: the header and one
/// record per row.
fn exported_table(case: &str, text: &str) -> (Vec<String>, Vec<::csv::StringRecord>) {
    let mut reader = ::csv::Reader::from_reader(text.as_bytes());
    let header: Vec<String> = reader
        .headers()
        .unwrap_or_else(|error| panic!("case {case:?}: the object must carry a header: {error}"))
        .iter()
        .map(ToString::to_string)
        .collect();
    let records = reader
        .records()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("case {case:?}: the object must be readable CSV: {error}"));
    (header, records)
}

/// The cell of `record` under the column `column`.
fn cell(case: &str, record: &::csv::StringRecord, column: &str) -> String {
    let index = state::EXPORT_RESULT_COLUMNS
        .iter()
        .position(|name| *name == column)
        .unwrap_or_else(|| panic!("case {case:?}: {column} must be one of the exported columns"));
    record.get(index).unwrap_or_default().to_string()
}

/// What one exported row says it is, and the item it names: the union member the result
/// came from and that member's own identifying value (an object's key, a file's path).
fn exported_item(case: &str, record: &::csv::StringRecord) -> (String, String) {
    let item_type = cell(case, record, state::ITEM_TYPE_COLUMN);
    let name = if item_type == state::EBS_ITEM_TYPE {
        cell(case, record, "FilePath")
    } else {
        cell(case, record, "ObjectKey")
    };
    (item_type, name)
}

/// The same pair, read off one entry of what `ListSearchJobResults` reported.
fn listed_item(result: &Value) -> (String, String) {
    if let Some(file) = result.get("EBSResultItem") {
        return (
            state::EBS_ITEM_TYPE.to_string(),
            file["FilePath"].as_str().unwrap_or_default().to_string(),
        );
    }
    let object = result.get("S3ResultItem").cloned().unwrap_or_default();
    (
        state::S3_ITEM_TYPE.to_string(),
        object["ObjectKey"].as_str().unwrap_or_default().to_string(),
    )
}

/// One row of the table: where the export writes, and what the search job it exports
/// recorded.
struct ExportedCase {
    case: &'static str,
    region: &'static str,
    bucket: &'static str,
    prefix: Option<&'static str>,
    resource_type: &'static str,
    items: usize,
}

#[tokio::test]
async fn an_export_writes_the_recorded_results_under_the_destination_prefix_and_completes() {
    // The rows differ along what the rule generalises over: whether a prefix is present and
    // what shape it has (absent, ending in a separator, nested, carrying no separator at
    // all), which kind of item the scan recorded, how many it recorded (including none at
    // all), and the region the call is signed for.
    let rows = [
        ExportedCase {
            case: "three backed-up objects, no prefix, us-west-2",
            region: "us-west-2",
            bucket: "export-dest",
            prefix: None,
            resource_type: "S3",
            items: 3,
        },
        ExportedCase {
            case: "one backed-up object, a prefix ending in a separator, eu-central-1",
            region: "eu-central-1",
            bucket: "results-dest",
            prefix: Some("results/"),
            resource_type: "S3",
            items: 1,
        },
        ExportedCase {
            case: "two backed-up files, a nested prefix, us-east-1",
            region: "us-east-1",
            bucket: "ebs.export.dest",
            prefix: Some("a/b/c/"),
            resource_type: "EBS",
            items: 2,
        },
        ExportedCase {
            case: "a backup holding no items, so the export is a table with no rows",
            region: "us-west-2",
            bucket: "empty-dest",
            prefix: Some("exports-2026"),
            resource_type: "S3",
            items: 0,
        },
    ];

    for row in rows {
        let ExportedCase {
            case,
            region,
            bucket,
            prefix,
            resource_type,
            items,
        } = row;
        let (emulator, engine) = emulator_with_engine();
        // The destination exists before the export starts, exactly as a client creates it:
        // the recordings of this surface create the bucket with s3:CreateBucket first.
        create_bucket(&emulator, region, bucket).await;
        seed_indexed_recovery_points(&engine, backup_index(resource_type, items));
        let search_job = completed_search_job(&emulator, &engine, region, case, resource_type).await;
        let results = listed_results(&emulator, region, case, &search_job).await;
        assert_eq!(
            results.len(),
            items,
            "case {case:?}: the scan must record one result per indexed item, got {results:?}"
        );

        let started = start_export(&emulator, region, &search_job, bucket, prefix).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSearchResultExportJob must succeed, got {} ({:?})",
            started.status, started.body
        );
        let identifier = started.string("ExportJobIdentifier");

        // Inside the span the clock owes, the export is RUNNING and carries no status
        // message: the message travels with the status it explains.
        tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
        let running = get_export(&emulator, region, &identifier).await;
        assert_eq!(
            running.string("Status"),
            state::RUNNING,
            "case {case:?}: an export started a moment ago is RUNNING, got {:?}",
            running.get("Status")
        );
        assert!(
            running.get("StatusMessage").is_null(),
            "case {case:?}: a RUNNING export reports no status message, got {:?}",
            running.get("StatusMessage")
        );

        // Once the clock has made the owed transition due, the export reports what the
        // write came to: the destination took the object, so it COMPLETED and explains
        // nothing.
        settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);
        let settled = get_export(&emulator, region, &identifier).await;
        assert_eq!(
            settled.string("Status"),
            state::COMPLETED,
            "case {case:?}: an export the destination took completes, got {:?} ({:?})",
            settled.get("Status"),
            settled.get("StatusMessage")
        );
        assert!(
            settled.get("StatusMessage").is_null(),
            "case {case:?}: a completed export explains nothing, got {:?}",
            settled.get("StatusMessage")
        );

        // The object is in the destination bucket, keyed by the submitted prefix followed
        // by the export job's own identifier — so a client that lists the prefix finds it.
        let expected_key = format!("{}{identifier}.csv", prefix.unwrap_or_default());
        let keys = listed_keys(&emulator, case, region, bucket, prefix).await;
        assert_eq!(
            keys,
            vec![expected_key.clone()],
            "case {case:?}: the destination must hold exactly the exported results object, \
             got {keys:?}"
        );
        let text = object_text(&emulator, region, bucket, &expected_key)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: the object store must serve {expected_key:?}")
            });

        // What it carries is one CSV table: the modelled member names as its header, and
        // exactly the results the service pages for that search job, in that order.
        let (header, records) = exported_table(case, &text);
        assert_eq!(
            header,
            state::EXPORT_RESULT_COLUMNS,
            "case {case:?}: expected the modelled member names as the header, got {header:?}"
        );
        let exported: Vec<(String, String)> = records
            .iter()
            .map(|record| exported_item(case, record))
            .collect();
        let expected: Vec<(String, String)> = results.iter().map(listed_item).collect();
        assert_eq!(
            exported, expected,
            "case {case:?}: the exported rows must be the results the service reports, got \
             {exported:?}"
        );
        // Each row also carries the BACKUP the item was found in, counted from the same
        // record the listing reports it from.
        for (index, record) in records.iter().enumerate() {
            assert_eq!(
                cell(case, record, "BackupResourceArn"),
                BACKUP_ARN,
                "case {case:?} row {index}: the row must name the backup it came from, got \
                 {record:?}"
            );
            assert_eq!(
                cell(case, record, "BackupVaultName"),
                VAULT,
                "case {case:?} row {index}: the row must name the vault, got {record:?}"
            );
        }
    }
}

/// A bucket the account holds throughout the refused-destination table and which no export
/// there names, so "nothing was written" is asserted against a listing the object store
/// really serves.
const BYSTANDER_BUCKET: &str = "held-bystander-dest";

/// One row of the refused-destination table: the destination the export names, and how the
/// object store comes to hold no such bucket.
struct MissingDestination {
    case: &'static str,
    bucket: &'static str,
    prefix: Option<&'static str>,
    /// Whether the bucket is created and then deleted again, as opposed to never created.
    created_then_deleted: bool,
}

#[tokio::test]
async fn an_export_whose_destination_the_object_store_does_not_hold_fails_with_its_refusal() {
    // The rows differ in HOW the destination comes to be absent — never created, created and
    // then deleted again, and a name no bucket can be held under — because the rule is about
    // what the object store answers and not about one spelling of a missing bucket.
    let rows = [
        MissingDestination {
            case: "a bucket nothing created",
            bucket: "never-created-dest",
            prefix: None,
            created_then_deleted: false,
        },
        MissingDestination {
            case: "a bucket created and then deleted again",
            bucket: "deleted-dest",
            prefix: Some("results/"),
            created_then_deleted: true,
        },
        MissingDestination {
            case: "a name no general-purpose bucket is held under",
            bucket: "Not A Bucket",
            prefix: Some("exports/"),
            created_then_deleted: false,
        },
    ];

    let region = "us-west-2";
    for row in rows {
        let MissingDestination {
            case,
            bucket,
            prefix,
            created_then_deleted,
        } = row;
        let (emulator, engine) = emulator_with_engine();
        // A bucket the account DOES hold, which this export does not name: what proves the
        // refused export wrote nowhere rather than merely not into its own destination.
        create_bucket(&emulator, region, BYSTANDER_BUCKET).await;
        if created_then_deleted {
            create_bucket(&emulator, region, bucket).await;
            let deleted = s3(&emulator, region)
                .delete_bucket()
                .bucket(bucket)
                .send()
                .await;
            assert!(
                deleted.is_ok(),
                "case {case:?}: s3:DeleteBucket must remove {bucket:?}, got {:?}",
                deleted.err()
            );
        }
        // A real result set, so what is asserted is the destination being absent and not an
        // export that had nothing to write.
        seed_indexed_recovery_points(&engine, backup_index("S3", 2));
        let search_job = completed_search_job(&emulator, &engine, region, case, "S3").await;

        let started = start_export(&emulator, region, &search_job, bucket, prefix).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: the start is accepted — the destination is resolved by the \
             export, not by the call, got {} ({:?})",
            started.status, started.body
        );
        let identifier = started.string("ExportJobIdentifier");

        settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);
        let settled = get_export(&emulator, region, &identifier).await;
        assert_eq!(
            settled.string("Status"),
            state::FAILED,
            "case {case:?}: an export the destination did not take fails, got {:?}",
            settled.get("Status")
        );
        let message = settled.string("StatusMessage");
        assert!(
            message.contains(&format!("s3://{bucket}/")),
            "case {case:?}: the failure must name the destination it could not write to, got \
             {message:?}"
        );
        assert!(
            settled.get("CompletionTime").as_f64().is_some(),
            "case {case:?}: a failed export carries the instant it ended, got {:?}",
            settled.get("CompletionTime")
        );
        // Nothing was written ANYWHERE: the bystander bucket the account does hold — whose
        // listing is therefore served, so an empty answer is a real answer — holds no
        // object, so the refused export did not land somewhere else instead.
        let elsewhere = listed_keys(&emulator, case, region, BYSTANDER_BUCKET, None).await;
        assert!(
            elsewhere.is_empty(),
            "case {case:?}: a refused export must leave no object in any held bucket, got \
             {elsewhere:?}"
        );
        // And the destination it named serves no such object: the key it would have taken
        // cannot be read back.
        let expected_key = format!("{}{identifier}.csv", prefix.unwrap_or_default());
        let served = object_text(&emulator, region, bucket, &expected_key).await;
        assert!(
            served.is_none(),
            "case {case:?}: no results object may be served for a refused export, got \
             {served:?}"
        );
        // The search job itself is untouched by the failed export: its results are still
        // exactly what the scan recorded.
        let results = listed_results(&emulator, region, case, &search_job).await;
        assert_eq!(
            results.len(),
            2,
            "case {case:?}: a failed export must not consume the search job's results, got \
             {results:?}"
        );
    }
}

#[tokio::test]
async fn two_exports_into_one_prefix_are_two_objects() {
    // The rule the export job's own identifier in the key exists for: one prefix holds the
    // output of several exports without one overwriting another.
    let region = "us-west-2";
    let bucket = "shared-prefix-dest";
    let prefix = "exports/";
    let (emulator, engine) = emulator_with_engine();
    create_bucket(&emulator, region, bucket).await;
    seed_indexed_recovery_points(&engine, backup_index("S3", 1));
    let search_job = completed_search_job(&emulator, &engine, region, "two exports", "S3").await;

    let mut expected: Vec<String> = Vec::new();
    for round in 0..2 {
        let started = start_export(&emulator, region, &search_job, bucket, Some(prefix)).await;
        assert_eq!(
            started.status, 200,
            "round {round}: StartSearchResultExportJob must succeed, got {} ({:?})",
            started.status, started.body
        );
        expected.push(format!(
            "{prefix}{}.csv",
            started.string("ExportJobIdentifier")
        ));
    }
    expected.sort();

    let mut keys = listed_keys(&emulator, "two exports", region, bucket, Some(prefix)).await;
    keys.sort();
    assert_eq!(
        keys, expected,
        "case two exports into one prefix: expected one object per export, got {keys:?}"
    );
}
