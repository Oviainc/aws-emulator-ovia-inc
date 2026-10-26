// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExportIdleRecommendations` and the objects it writes.
//!
//! The rule under test: the operation "exports idle resource recommendations ... in a
//! comma-separated values (CSV) file, and its metadata in a JavaScript Object Notation (JSON)
//! file, to an existing Amazon Simple Storage Service (Amazon S3) bucket that you specify", so
//! once the job it created reports `Complete` the two objects the call answered —
//! `s3Destination.key` and `s3Destination.metadataKey` — are READABLE from that bucket, the
//! export file carries one row per idle resource recommendation `GetIdleRecommendations`
//! reports for the same analysis, and each cell is the value the reader reports for the
//! `ExportableIdleField` naming that column.
//!
//! Both halves are asserted over a TABLE: the destination over the forms a key prefix can
//! take, and the file's content over the field selections a request can ask for, including the
//! whole modelled field set a request that selects none gets. The reader is the ORACLE for
//! every cell, so no expectation here restates a figure this analysis happens to hold.
//!
//! The rules this export shares with every other `Export*Recommendations` operation of the
//! service — the destination bucket must exist, nothing is written before the job completes,
//! and a destination that has gone away fails the job — have their own suite
//! (`recommendation_export_objects.rs`); this one is about the idle surface's own file.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::EXPORTABLE_IDLE_FIELD_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    round_hundredths, ANALYSIS_DELAY, EXPORT_JOB_STEP, JOB_STATUS_COMPLETE, RESOURCE_TYPE_IDLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cell, create_bucket, csv_rows, emulator, export_destination, get_object, opt_in,
};

/// The operation under test.
const EXPORT: &str = "ExportIdleRecommendations";

/// The destination bucket these fixtures export to. Compute Optimizer does not create it, so
/// each fixture creates it through Amazon S3 first.
const BUCKET: &str = "idle-recommendation-exports";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// How a cell holding several values separates them in the export file, so one exportable
/// field is still one column. AWS publishes no layout for the file, so this is the emulator's
/// own, asserted here as what a client reading the file sees.
const LIST_SEPARATOR: &str = "|";

/// The `ExportableIdleField` column each `IdleMetricName` the analysis can observe lands in.
///
/// Written out from the model's own two enums (`smithy_query query="shape"
/// shape="ExportableIdleField,IdleMetricName"`): the field set names one column per metric,
/// carrying the statistic the export file reports that metric under — which is not always the
/// `Maximum` statistic this API reports utilization metrics with (`IdleUtilizationMetric`:
/// "The Compute Optimizer API, Command Line Interface (CLI), and SDKs return utilization
/// metrics using only the `Maximum` statistic"). `ConsumedChangeDataCaptureUnits` is the one
/// metric name the field set declares NO column for, so nothing in the file reports it.
const METRIC_COLUMNS: &[(&str, &str)] = &[
    ("ActiveConnectionCount", "UtilizationMetricsActiveConnectionCountMaximum"),
    ("CPU", "UtilizationMetricsCpuMaximum"),
    ("CacheHits", "UtilizationMetricsCacheHitsSum"),
    ("CacheMisses", "UtilizationMetricsCacheMissesSum"),
    ("ConsumedChangeDataCaptureUnits", ""),
    ("ConsumedReadCapacityUnits", "UtilizationMetricsConsumedReadCapacityUnitsSum"),
    ("ConsumedWriteCapacityUnits", "UtilizationMetricsConsumedWriteCapacityUnitsSum"),
    ("CurrConnections", "UtilizationMetricsCurrConnectionsSum"),
    // The one metric the field set gives a column per statistic; the statistic the reader
    // reports is what picks between them, so the `Sum` column is reported by nothing.
    ("DatabaseConnections", "UtilizationMetricsDatabaseConnectionsMaximum"),
    ("EBSVolumeReadIOPS", "UtilizationMetricsEBSVolumeReadIOPSMaximum"),
    ("EBSVolumeWriteIOPS", "UtilizationMetricsEBSVolumeWriteIOPSMaximum"),
    ("ElastiCacheProcessingUnits", "UtilizationMetricsElastiCacheProcessingUnitsSum"),
    ("EngineCPUUtilization", "UtilizationMetricsEngineCPUUtilizationMaximum"),
    ("GetTypeCmds", "UtilizationMetricsGetTypeCmdsSum"),
    ("Invocations", "UtilizationMetricsInvocationsSum"),
    ("IsIdle", "UtilizationMetricsIsIdleMinimum"),
    ("KeyspaceHits", "UtilizationMetricsKeyspaceHitsSum"),
    ("KeyspaceMisses", "UtilizationMetricsKeyspaceMissesSum"),
    ("Memory", "UtilizationMetricsMemoryMaximum"),
    ("NetworkInBytesPerSecond", "UtilizationMetricsNetworkInBytesPerSecondMaximum"),
    ("NetworkOutBytesPerSecond", "UtilizationMetricsNetworkOutBytesPerSecondMaximum"),
    ("NewConnections", "UtilizationMetricsNewConnectionsSum"),
    ("PacketsInFromDestination", "UtilizationMetricsPacketsInFromDestinationMaximum"),
    ("PacketsInFromSource", "UtilizationMetricsPacketsInFromSourceMaximum"),
    ("SetTypeCmds", "UtilizationMetricsSetTypeCmdsSum"),
    ("UserConnected", "UtilizationMetricsUserConnectedSum"),
    ("VolumeReadOpsPerSecond", "UtilizationMetricsVolumeReadOpsPerSecondMaximum"),
    ("VolumeWriteOpsPerSecond", "UtilizationMetricsVolumeWriteOpsPerSecondMaximum"),
];

/// An emulator with the destination bucket created, the account opted in and the analysis its
/// enrollment starts already due — which is what the recommendation reader reports over, so it
/// is what the export file holds.
async fn analysed_emulator() -> MultiServiceRouter {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
    router
}

/// Export the account's idle resource recommendations, asserting the call was accepted, and
/// answer the whole response body.
async fn export_ok(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, EXPORT, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    body
}

/// The idle resource recommendations `GetIdleRecommendations` reports, in the order it reports
/// them: the ORACLE every expected cell below is derived from.
async fn reader_recommendations(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let (status, body) = call(router, "GetIdleRecommendations", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the recommendations: got {status}: {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: the analysis fits one page, got {body}"
    );
    let reported = body["idleRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a list, got {body}"))
        .clone();
    assert!(
        !reported.is_empty(),
        "case {case:?}: the analysis holds idle resource recommendations, got {body}"
    );
    reported
}

/// Poll `DescribeRecommendationExportJobs` until the job reports `Complete` — the documented
/// workflow a client follows before downloading the file — and answer the job as reported.
async fn polled_to_complete(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    for _ in 0..10 {
        let (status, body) = call(
            router,
            "DescribeRecommendationExportJobs",
            json!({"jobIds": [job_id]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading job {job_id}: got {status}: {body}"
        );
        let job = body["recommendationExportJobs"]
            .as_array()
            .and_then(|jobs| jobs.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no job reported for {job_id}, got {body}"));
        if job["status"].as_str() == Some(JOB_STATUS_COMPLETE) {
            assert!(
                job["failureReason"].is_null(),
                "case {case:?}: a completed export carries no failure reason, got {job}"
            );
            return job;
        }
        tokio::time::sleep(EXPORT_JOB_STEP).await;
    }
    panic!("case {case:?}: job {job_id} never completed");
}

/// Download one object, asserting it was served, and answer its bytes as text.
async fn downloaded(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str) -> String {
    let (status, body) = get_object(router, bucket, key).await;
    assert_eq!(
        status, 200,
        "case {case:?}: downloading {key}: got {status}: {}",
        String::from_utf8_lossy(&body)
    );
    String::from_utf8(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the object is text: {error}"))
}

/// A figure as the export file states it: at the two decimal places every figure this service
/// reports is rounded to.
fn number_cell(value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| round_hundredths(value).to_string())
}

/// A string member of the reported recommendation, or the empty cell where it reports none.
fn text(recommendation: &Value, member: &str) -> String {
    recommendation[member].as_str().unwrap_or_default().to_string()
}

/// The value the reader reports for the metric `name`, whatever statistic it reports it with.
fn reported_metric(recommendation: &Value, name: &str) -> Option<f64> {
    recommendation["utilizationMetrics"]
        .as_array()?
        .iter()
        .find(|metric| metric["name"].as_str() == Some(name))
        .and_then(|metric| metric["value"].as_f64())
}

/// The cell the export file must carry under `column` for the recommendation the READER
/// reports — the whole expectation for a row, derived from the reader's own answer.
///
/// Every value of `ExportableIdleField` is covered: the members an `IdleRecommendation`
/// reports, and one column per observed metric ([`METRIC_COLUMNS`]). A column the
/// recommendation reports nothing for is empty.
fn expected_cell(column: &str, recommendation: &Value) -> String {
    match column {
        "AccountId" => text(recommendation, "accountId"),
        "ResourceArn" => text(recommendation, "resourceArn"),
        "ResourceId" => text(recommendation, "resourceId"),
        "ResourceType" => text(recommendation, "resourceType"),
        "Finding" => text(recommendation, "finding"),
        "FindingDescription" => text(recommendation, "findingDescription"),
        "LookbackPeriodInDays" => number_cell(recommendation["lookBackPeriodInDays"].as_f64()),
        // An `IdleSavingsOpportunity` is named for its percentage: "the estimated monthly
        // savings possible as a percentage of monthly cost".
        "SavingsOpportunity" => number_cell(
            recommendation["savingsOpportunity"]["savingsOpportunityPercentage"].as_f64(),
        ),
        "SavingsOpportunityAfterDiscount" => number_cell(
            recommendation["savingsOpportunityAfterDiscounts"]["savingsOpportunityPercentage"]
                .as_f64(),
        ),
        "Tags" => recommendation["tags"]
            .as_array()
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        format!(
                            "{}={}",
                            tag["key"].as_str().unwrap_or_default(),
                            tag["value"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<String>>()
                    .join(LIST_SEPARATOR)
            })
            .unwrap_or_default(),
        // "The timestamp of when the idle resource recommendation was last generated", as the
        // file states an instant: RFC 3339 in UTC.
        "LastRefreshTimestamp" => recommendation["lastRefreshTimestamp"]
            .as_f64()
            .and_then(|at| {
                ::aws_smithy_types::DateTime::from_secs_f64(at)
                    .fmt(::aws_smithy_types::date_time::Format::DateTime)
                    .ok()
            })
            .unwrap_or_default(),
        column => METRIC_COLUMNS
            .iter()
            .find(|(_, field)| *field == column)
            .and_then(|(metric, _)| reported_metric(recommendation, metric))
            .map_or_else(String::new, |value| number_cell(Some(value))),
    }
}

/// The catalog above is the model's: every column it names is an `ExportableIdleField`, and
/// every metric name it covers is one the file can report, so an expectation built from it
/// cannot name a column the export file could never carry.
#[test]
fn the_metric_column_catalog_names_only_modelled_fields() {
    for (metric, column) in METRIC_COLUMNS {
        if column.is_empty() {
            continue;
        }
        assert!(
            EXPORTABLE_IDLE_FIELD_VALUES.contains(column),
            "case {metric:?}: {column} is not an ExportableIdleField"
        );
    }
    let covered = METRIC_COLUMNS
        .iter()
        .filter(|(_, column)| !column.is_empty())
        .count();
    let declared = EXPORTABLE_IDLE_FIELD_VALUES
        .iter()
        .filter(|field| field.starts_with("UtilizationMetrics"))
        .count();
    // One declared field is the second statistic of a metric that has two columns, which no
    // metric name maps to.
    assert_eq!(
        covered + 1,
        declared,
        "the catalog must cover every utilization field the model declares bar the duplicate \
         statistic, got {covered} of {declared}"
    );
}

/// The destination keys an idle export answers are derived from the request's bucket and key
/// prefix, the account, the resource type and the minted job id — one row per form a key
/// prefix can take — and once the job reports `Complete` BOTH objects are readable at exactly
/// those keys.
#[tokio::test]
async fn the_objects_appear_at_exactly_the_keys_the_call_answered() {
    let account = engine::emulator_account_id();

    // (case, the `keyPrefix` the request carries, the prefix the keys are expected under)
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no key prefix at all", None, ""),
        ("an empty key prefix", Some(""), ""),
        ("a single-segment prefix", Some("quarterly"), "quarterly/"),
        ("a prefix with a trailing slash", Some("exports/"), "exports/"),
        (
            "a prefix with leading and trailing slashes",
            Some("/nested/idle-exports/"),
            "nested/idle-exports/",
        ),
        ("a multi-segment prefix", Some("a/b/c"), "a/b/c/"),
    ];

    for (case, key_prefix, expected_prefix) in rows {
        // A fresh emulator per row: only one idle export job is in progress per Region at a
        // time, so each row gets its own store rather than waiting on the previous row's job.
        let router = analysed_emulator().await;
        let mut destination_config = json!({"bucket": BUCKET});
        if let Some(key_prefix) = key_prefix {
            destination_config["keyPrefix"] = json!(key_prefix);
        }
        let body = export_ok(
            &router,
            case,
            json!({"s3DestinationConfig": destination_config}),
        )
        .await;
        let job_id = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an accepted export answers a jobId, got {body}"))
            .to_string();
        let stem =
            format!("{expected_prefix}{account}/ComputeOptimizer/{RESOURCE_TYPE_IDLE}/{job_id}");
        assert_eq!(
            body["s3Destination"],
            json!({
                "bucket": BUCKET,
                "key": format!("{stem}.csv"),
                "metadataKey": format!("{stem}-metadata.json"),
            }),
            "case {case:?}: unexpected destination, got {body}"
        );
        let (bucket, key, metadata_key) = export_destination(case, &body);

        // Read the destination FIRST, then the job: if the job has still not completed by the
        // later read, it had not completed at the earlier one either, so the objects had to be
        // absent then.
        let (export_status, _) = get_object(&router, &bucket, &key).await;
        let (metadata_status, _) = get_object(&router, &bucket, &metadata_key).await;
        let (status, pending) = call(
            &router,
            "DescribeRecommendationExportJobs",
            json!({"jobIds": [&job_id]}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: reading the job: got {pending}");
        if pending["recommendationExportJobs"][0]["status"].as_str() != Some(JOB_STATUS_COMPLETE) {
            assert_eq!(
                (export_status, metadata_status),
                (404, 404),
                "case {case:?}: nothing is written while the job reports {}",
                pending["recommendationExportJobs"][0]["status"]
            );
        }

        polled_to_complete(&router, case, &job_id).await;
        let exported = downloaded(&router, case, &bucket, &key).await;
        assert!(
            csv_rows(&exported).len() > 1,
            "case {case:?}: the export file holds the analysed idle resources, got {exported:?}"
        );
        let metadata = downloaded(&router, case, &bucket, &metadata_key).await;
        let metadata: Value = serde_json::from_slice(metadata.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: the metadata file is JSON: {error}"));
        assert_eq!(
            metadata["destination"],
            json!({"bucket": bucket, "key": key, "metadataKey": metadata_key}),
            "case {case:?}: the metadata names where both objects were written, got {metadata}"
        );
    }
}

/// The export file holds the recommendations `GetIdleRecommendations` reports for the same
/// analysis: one row per recommendation, in the order the reader reports them, every cell the
/// value the reader reports for the `ExportableIdleField` naming that column — and the header
/// is exactly the fields the request asked for, or the whole modelled field set when it asked
/// for none. One row per field selection, and the metadata file states what the export was.
#[tokio::test]
async fn the_export_file_holds_the_recommendations_the_reader_reports() {
    let every_field: Vec<&str> = EXPORTABLE_IDLE_FIELD_VALUES.to_vec();

    // (case, the `fieldsToExport` the request carries, the columns the file must then carry)
    type Selection = (&'static str, Option<Vec<&'static str>>, Vec<&'static str>);
    let selections: Vec<Selection> = vec![
        (
            "a three-field selection",
            Some(vec!["AccountId", "ResourceArn", "Finding"]),
            vec!["AccountId", "ResourceArn", "Finding"],
        ),
        (
            "fields asked for in the request's own order",
            Some(vec![
                "Tags",
                "FindingDescription",
                "ResourceType",
                "LookbackPeriodInDays",
                "ResourceId",
            ]),
            vec![
                "Tags",
                "FindingDescription",
                "ResourceType",
                "LookbackPeriodInDays",
                "ResourceId",
            ],
        ),
        (
            // A field asked for twice is one column, so neither the file nor the work of
            // building it grows with a list that repeats.
            "a field asked for twice",
            Some(vec![
                "ResourceArn",
                "SavingsOpportunity",
                "SavingsOpportunityAfterDiscount",
                "ResourceArn",
            ]),
            vec![
                "ResourceArn",
                "SavingsOpportunity",
                "SavingsOpportunityAfterDiscount",
            ],
        ),
        (
            // Metric columns whose statistic is the one the field set names for the metric —
            // `Sum` and `Minimum` among them, which is not the statistic this API reports a
            // utilization metric with.
            "a selection of utilization metric columns",
            Some(vec![
                "ResourceArn",
                "UtilizationMetricsCpuMaximum",
                "UtilizationMetricsKeyspaceHitsSum",
                "UtilizationMetricsIsIdleMinimum",
                "UtilizationMetricsDatabaseConnectionsSum",
                "LastRefreshTimestamp",
            ]),
            vec![
                "ResourceArn",
                "UtilizationMetricsCpuMaximum",
                "UtilizationMetricsKeyspaceHitsSum",
                "UtilizationMetricsIsIdleMinimum",
                "UtilizationMetricsDatabaseConnectionsSum",
                "LastRefreshTimestamp",
            ],
        ),
        (
            // A request that selects no field gets every field the model declares for the
            // resource type (`ExportableIdleField`).
            "no field selection at all",
            None,
            every_field,
        ),
    ];

    for (case, fields, columns) in selections {
        // A fresh emulator per row: one idle export job is in progress per Region at a time.
        let router = analysed_emulator().await;
        let reported = reader_recommendations(&router, case).await;

        let mut request = json!({"s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "idle"}});
        if let Some(fields) = &fields {
            request["fieldsToExport"] = json!(fields);
        }
        let body = export_ok(&router, case, request).await;
        let job_id = body["jobId"].as_str().unwrap_or_default().to_string();
        let (bucket, key, metadata_key) = export_destination(case, &body);
        polled_to_complete(&router, case, &job_id).await;

        let exported = downloaded(&router, case, &bucket, &key).await;
        let rows = csv_rows(&exported);
        let header = rows.first().cloned().unwrap_or_default();
        assert_eq!(
            header, columns,
            "case {case:?}: the header row is the fields the request asked for, got {exported:?}"
        );
        let data = rows.get(1..).unwrap_or_default();
        assert_eq!(
            data.len(),
            reported.len(),
            "case {case:?}: expected one row per reported recommendation ({}), got {exported:?}",
            reported.len()
        );

        for (row, recommendation) in data.iter().zip(&reported) {
            for column in &header {
                let expected = expected_cell(column, recommendation);
                assert_eq!(
                    cell(&header, row, column),
                    expected,
                    "case {case:?}: column {column} of {} must hold what the reader reports \
                     ({expected:?}), got {row:?}",
                    text(recommendation, "resourceArn")
                );
            }
        }

        // The file is not a sheet of empty cells: every metric the reader reports for a
        // recommendation is in the row under the column the field set names for it, and the
        // columns filled that way include ones whose statistic is not the reported one.
        let mut filled_under_other_statistic = 0;
        for (row, recommendation) in data.iter().zip(&reported) {
            for (metric, column) in METRIC_COLUMNS {
                if column.is_empty() || !header.iter().any(|held| held == column) {
                    continue;
                }
                let Some(value) = reported_metric(recommendation, metric) else {
                    continue;
                };
                assert_eq!(
                    cell(&header, row, column),
                    number_cell(Some(value)),
                    "case {case:?}: the {metric} the reader reports for {} is exported under \
                     {column}, got {row:?}",
                    text(recommendation, "resourceArn")
                );
                if !column.ends_with("Maximum") {
                    filled_under_other_statistic += 1;
                }
            }
        }
        if fields.is_none() {
            assert!(
                filled_under_other_statistic > 0,
                "case {case:?}: the analysis observes metrics the field set names with another \
                 statistic, and the file must report them, got {exported:?}"
            );
        }

        let metadata = downloaded(&router, case, &bucket, &metadata_key).await;
        let metadata: Value = serde_json::from_slice(metadata.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: the metadata file is JSON: {error}"));
        assert_eq!(
            metadata["jobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: the metadata names its own job, got {metadata}"
        );
        assert_eq!(
            metadata["resourceType"].as_str(),
            Some(RESOURCE_TYPE_IDLE),
            "case {case:?}: the metadata names the exported resource type, got {metadata}"
        );
        assert_eq!(
            metadata["exportFile"]["columns"],
            json!(columns),
            "case {case:?}: the metadata states the file's columns, got {metadata}"
        );
        assert_eq!(
            metadata["exportFile"]["fileFormat"].as_str(),
            Some("Csv"),
            "case {case:?}: the only modelled file format is Csv, got {metadata}"
        );
        assert_eq!(
            metadata["exportFile"]["rowCount"].as_u64(),
            Some(data.len() as u64),
            "case {case:?}: the metadata counts the rows the file holds ({}), got {metadata}",
            data.len()
        );
    }
}
