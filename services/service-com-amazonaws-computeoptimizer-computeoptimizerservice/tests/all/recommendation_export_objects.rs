// SPDX-License-Identifier: CC0-1.0

//! The OBJECTS a recommendations export writes, driven over the wire.
//!
//! The rule under test, which every `Export*Recommendations` operation of this service
//! shares: the recommendations are exported "in a comma-separated values (.csv) file, and its
//! metadata in a JavaScript Object Notation (JSON) (.json) file, to an existing Amazon Simple
//! Storage Service (Amazon S3) bucket that you specify", so
//!
//! * a request naming a destination bucket that does not exist is REFUSED with
//!   `InvalidParameterValueException` carrying the service's constant missing-destination
//!   message ("The Amazon S3 destination provided does not exist. Confirm your S3 destination
//!   and try again."), and records no job; and
//! * once the job it created reports `Complete`, the two objects the call answered —
//!   `s3Destination.key` and `s3Destination.metadataKey` — are in that bucket and carry the
//!   export and its metadata. Before the job completes neither object is there.
//!
//! Each half is checked over a TABLE of every export operation the service implements rather
//! than one of them, because the rule is the same for all of them, and the export file's
//! CONTENT is checked against what the recommendation reader of the same analysis reports.
//!
//! Compute Optimizer and Amazon S3 are mounted on ONE engine, exactly as `core/router` mounts
//! them, and every request goes through the front door a client uses: the Compute Optimizer
//! calls as `POST /` with `X-Amz-Target`, the object calls as the path-style S3 requests a
//! client sends (`PUT /<bucket>`, `GET /<bucket>/<key>`, `DELETE /<bucket>`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::export::MISSING_DESTINATION_MESSAGE;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, RdsDatabaseDetail, ANALYSIS_DELAY, EXPORT_JOB_STEP,
    JOB_STATUS_COMPLETE, JOB_STATUS_FAILED, RESOURCE_TYPE_AUTO_SCALING_GROUP,
    RESOURCE_TYPE_EBS_VOLUME, RESOURCE_TYPE_EC2_INSTANCE, RESOURCE_TYPE_ECS_SERVICE,
    RESOURCE_TYPE_IDLE, RESOURCE_TYPE_LAMBDA_FUNCTION, RESOURCE_TYPE_LICENSE,
    RESOURCE_TYPE_RDS_DB_INSTANCE, SOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cell, create_bucket, csv_rows, delete_bucket, emulator, emulator_with_engine,
    error_code, export_destination, get_object, opt_in, ACCOUNT,
};

/// The destination bucket these fixtures export to.
const BUCKET: &str = "compute-optimizer-exports";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// Every export operation this service implements, with the `ResourceType` its job carries
/// and a field set a request may ask for: the class the rule generalises over. Taken from the
/// service's own operation list, each row naming its own resource's ARN field.
const EXPORTS: &[(&str, &str, &[&str])] = &[
    (
        "ExportAutoScalingGroupRecommendations",
        RESOURCE_TYPE_AUTO_SCALING_GROUP,
        &["AccountId", "AutoScalingGroupArn", "Finding"],
    ),
    (
        "ExportEC2InstanceRecommendations",
        RESOURCE_TYPE_EC2_INSTANCE,
        &["AccountId", "InstanceArn", "Finding", "FindingReasonCodes"],
    ),
    (
        "ExportEBSVolumeRecommendations",
        RESOURCE_TYPE_EBS_VOLUME,
        &["AccountId", "VolumeArn", "Finding"],
    ),
    (
        "ExportECSServiceRecommendations",
        RESOURCE_TYPE_ECS_SERVICE,
        &["AccountId", "ServiceArn", "Finding"],
    ),
    (
        "ExportIdleRecommendations",
        RESOURCE_TYPE_IDLE,
        &["AccountId", "ResourceArn", "Finding", "ResourceType"],
    ),
    (
        "ExportLicenseRecommendations",
        RESOURCE_TYPE_LICENSE,
        &["AccountId", "ResourceArn", "Finding"],
    ),
    (
        "ExportLambdaFunctionRecommendations",
        RESOURCE_TYPE_LAMBDA_FUNCTION,
        &["AccountId", "FunctionArn", "Finding", "FindingReasonCodes"],
    ),
    (
        "ExportRDSDatabaseRecommendations",
        RESOURCE_TYPE_RDS_DB_INSTANCE,
        &["AccountId", "ResourceArn", "InstanceFinding"],
    ),
];

/// An emulator with the account opted in and the destination bucket created, and with the
/// analysis the enrollment starts already due — which is what a recommendation reader reports
/// over, so it is what an export file holds.
async fn analysed_emulator() -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    record_analysed_database(&engine);
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
    router
}

/// Record one analysed Amazon Aurora or RDS database into the findings catalog, the way
/// Compute Optimizer's own analysis of an observed workload would: a database recommendation
/// is an analysis of a DB instance the account owns, so the database export holds a row only
/// once such an analysis exists.
fn record_analysed_database(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    state.lock().analysed_resources.push(AnalysedResource {
        account_id: ACCOUNT.to_string(),
        resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
        // The catalog records a resource's REGION-FREE identity; the ARN reported for it names
        // the region of the endpoint that reports it.
        arn_service: "rds".to_string(),
        arn_resource: "db:ledger-primary".to_string(),
        finding: "Overprovisioned".to_string(),
        finding_reason_codes: Vec::new(),
        current_performance_risk: "Low".to_string(),
        current_monthly_cost: 200.0,
        recommended_monthly_cost: 150.0,
        inferred_workload_types: Vec::new(),
        rds_database: Some(RdsDatabaseDetail {
            engine: Some("postgres".to_string()),
            current_db_instance_class: Some("db.r6g.2xlarge".to_string()),
            recommended_db_instance_class: Some("db.r6g.xlarge".to_string()),
            ..RdsDatabaseDetail::default()
        }),
    });
}

/// Call one export operation, asserting it was accepted, and answer its response body.
async fn export_ok(router: &MultiServiceRouter, operation: &str, request: Value) -> Value {
    let (status, body) = call(router, operation, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: expected {request} to be accepted, got {status}: {body}"
    );
    body
}

/// The job `job_id` as `DescribeRecommendationExportJobs` reports it.
async fn reported_job(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
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
    body["recommendationExportJobs"]
        .as_array()
        .and_then(|jobs| jobs.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no job reported for {job_id}, got {body}"))
}

// The destination reader, the CSV reader and the cell lookup live in `harness.rs`, shared
// with the per-surface export suites so no two of them read an export file differently.

/// "You must create the destination Amazon S3 bucket for your recommendations export before
/// you create the export job. Compute Optimizer does not create the S3 bucket for you": every
/// export operation refuses a destination bucket the object store does not hold, with
/// `InvalidParameterValueException` and no job recorded, and accepts the same request once
/// the bucket exists.
#[tokio::test]
async fn an_export_to_a_bucket_that_does_not_exist_is_refused() {
    for (operation, _, fields) in EXPORTS {
        // A fresh emulator per row: only one export job of a resource type is in progress per
        // Region at a time, and the accepted call below creates one.
        let router = emulator();
        opt_in(&router).await;

        // (case, the destination bucket the request names)
        let missing: Vec<(&str, &str)> = vec![
            ("a bucket nothing created", "no-such-bucket-at-all"),
            ("a bucket of another name than the one created", "exports-2"),
        ];
        for (case, bucket) in missing {
            let case = format!("{operation}: {case}");
            let request = json!({
                "s3DestinationConfig": {"bucket": bucket},
                "fieldsToExport": fields,
            });
            let (status, body) = call(&router, operation, request.clone()).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {request} to be refused, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterValueException"),
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                MISSING_DESTINATION_MESSAGE,
                "case {case:?}: expected the service's missing-destination refusal, got {body}"
            );
            assert!(
                body["jobId"].is_null(),
                "case {case:?}: a refused export answers no job id, got {body}"
            );
            let (status, listed) =
                call(&router, "DescribeRecommendationExportJobs", json!({})).await;
            assert_eq!(status, 200, "case {case:?}: reading the jobs: got {listed}");
            assert_eq!(
                listed["recommendationExportJobs"],
                json!([]),
                "case {case:?}: a refused export records no job, got {listed}"
            );
        }

        // The same request, once the bucket the destination names exists.
        create_bucket(&router, BUCKET).await;
        let body = export_ok(
            &router,
            operation,
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "fieldsToExport": fields,
            }),
        )
        .await;
        assert!(
            body["jobId"].is_string(),
            "case {operation:?}: an accepted export answers a job id, got {body}"
        );
    }
}

/// Once the job an export created reports `Complete`, the export file and its metadata file
/// are in the destination bucket under exactly the keys the call answered — and neither is
/// there while the job has not completed. One row per export operation.
#[tokio::test]
async fn a_completed_export_writes_the_file_and_its_metadata_to_the_destination() {
    for (operation, resource_type, fields) in EXPORTS {
        // A fresh emulator per row: only one export job of a resource type is in progress per
        // Region at a time.
        let router = analysed_emulator().await;
        let body = export_ok(
            &router,
            operation,
            json!({
                "s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "quarterly"},
                "fieldsToExport": fields,
            }),
        )
        .await;
        let job_id = body["jobId"].as_str().unwrap_or_default().to_string();
        let (bucket, key, metadata_key) = export_destination(operation, &body);
        assert_eq!(
            bucket, BUCKET,
            "case {operation:?}: the export answers the destination it was given, got {body}"
        );

        // Read the destination FIRST, then the job: if the job has still not completed by the
        // later read, it had not completed at the earlier one either, so the objects had to
        // be absent then.
        let (export_status, _) = get_object(&router, &bucket, &key).await;
        let (metadata_status, _) = get_object(&router, &bucket, &metadata_key).await;
        let pending = reported_job(&router, operation, &job_id).await;
        if pending["status"].as_str() != Some(JOB_STATUS_COMPLETE) {
            assert_eq!(
                (export_status, metadata_status),
                (404, 404),
                "case {operation:?}: nothing is written while the job reports {}",
                pending["status"]
            );
        }

        // Poll the job to `Complete`, the documented workflow, then download both objects.
        let mut status = String::new();
        for _ in 0..10 {
            let job = reported_job(&router, operation, &job_id).await;
            status = job["status"].as_str().unwrap_or_default().to_string();
            if status == JOB_STATUS_COMPLETE {
                assert!(
                    job["failureReason"].is_null(),
                    "case {operation:?}: a completed export carries no failure reason, got {job}"
                );
                break;
            }
            tokio::time::sleep(EXPORT_JOB_STEP).await;
        }
        assert_eq!(
            status, JOB_STATUS_COMPLETE,
            "case {operation:?}: the job never completed, last status {status:?}"
        );

        let (status, exported) = get_object(&router, &bucket, &key).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: downloading {key}: got {status}: {}",
            String::from_utf8_lossy(&exported)
        );
        let exported = String::from_utf8(exported)
            .unwrap_or_else(|error| panic!("case {operation:?}: the export file is text: {error}"));
        let rows = csv_rows(&exported);
        assert_eq!(
            rows.first().map(Vec::as_slice),
            Some(
                fields
                    .iter()
                    .map(|field| (*field).to_string())
                    .collect::<Vec<String>>()
                    .as_slice()
            ),
            "case {operation:?}: the header row is the fields the request asked for, got \
             {exported:?}"
        );
        // The file holds the analysis, not just a header: every resource type exported here
        // has analysed resources, and each row says which account owns the resource and which
        // resource it is about.
        let header = rows.first().cloned().unwrap_or_default();
        let data = rows.get(1..).unwrap_or_default();
        assert!(
            !data.is_empty(),
            "case {operation:?}: the export file holds the analysed {resource_type} resources, \
             got {exported:?}"
        );
        let arn_column = fields
            .get(1)
            .copied()
            .unwrap_or_else(|| panic!("case {operation:?}: the field set names an ARN column"));
        for row in data {
            assert_eq!(
                cell(&header, row, "AccountId"),
                ACCOUNT,
                "case {operation:?}: every row names the account that owns the resource, got \
                 {row:?}"
            );
            assert!(
                cell(&header, row, arn_column).starts_with("arn:aws:"),
                "case {operation:?}: every row names its resource under {arn_column}, got {row:?}"
            );
        }

        let (status, metadata) = get_object(&router, &bucket, &metadata_key).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: downloading {metadata_key}: got {status}: {}",
            String::from_utf8_lossy(&metadata)
        );
        let metadata: Value = serde_json::from_slice(&metadata).unwrap_or_else(|error| {
            panic!("case {operation:?}: the metadata file is JSON: {error}")
        });
        assert_eq!(
            metadata["jobId"].as_str(),
            Some(job_id.as_str()),
            "case {operation:?}: the metadata names its own job, got {metadata}"
        );
        assert_eq!(
            metadata["resourceType"].as_str(),
            Some(*resource_type),
            "case {operation:?}: the metadata names the exported resource type, got {metadata}"
        );
        assert_eq!(
            metadata["exportFile"]["rowCount"].as_u64(),
            Some(rows.len().saturating_sub(1) as u64),
            "case {operation:?}: the metadata counts the rows the export file holds ({}), got \
             {metadata}",
            rows.len().saturating_sub(1)
        );
        assert_eq!(
            metadata["destination"],
            json!({"bucket": bucket, "key": key, "metadataKey": metadata_key}),
            "case {operation:?}: the metadata names where both objects were written, got \
             {metadata}"
        );
    }
}

/// The exported file holds the recommendations the READER of the same analysis reports: one
/// row per recommendation option of every analysed Auto Scaling group, with each cell the
/// value that group's recommendation carries. Checked over a table of column selections,
/// including the whole modelled field set a request that selects none gets.
#[tokio::test]
async fn the_export_file_holds_the_recommendations_the_reader_reports() {
    // The groups the reader reports, with the finding and the option count of each.
    let router = analysed_emulator().await;
    let (status, reported) = call(
        &router,
        "GetAutoScalingGroupRecommendations",
        json!({"maxResults": 100}),
    )
    .await;
    assert_eq!(status, 200, "reading the recommendations: got {reported}");
    let groups: Vec<(String, String, usize)> = reported["autoScalingGroupRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("the reader reports a recommendation list, got {reported}"))
        .iter()
        .map(|recommendation| {
            (
                recommendation["autoScalingGroupArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                recommendation["finding"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                recommendation["recommendationOptions"]
                    .as_array()
                    .map_or(0, Vec::len)
                    .max(1),
            )
        })
        .collect();
    assert!(
        !groups.is_empty(),
        "the analysis holds at least one Auto Scaling group recommendation, got {reported}"
    );
    let expected_rows: usize = groups.iter().map(|(_, _, options)| options).sum();

    // (case, the `fieldsToExport` the request carries, the columns the file must then carry)
    type Selection = (&'static str, Option<Vec<String>>, Vec<String>);
    let selections: Vec<Selection> = vec![
        (
            "a three-field selection",
            Some(vec![
                "AccountId".to_string(),
                "AutoScalingGroupArn".to_string(),
                "Finding".to_string(),
            ]),
            vec![
                "AccountId".to_string(),
                "AutoScalingGroupArn".to_string(),
                "Finding".to_string(),
            ],
        ),
        (
            "fields asked for in the request's own order",
            Some(vec![
                "Finding".to_string(),
                "AutoScalingGroupName".to_string(),
                "CurrentConfigurationInstanceType".to_string(),
                "AutoScalingGroupArn".to_string(),
            ]),
            vec![
                "Finding".to_string(),
                "AutoScalingGroupName".to_string(),
                "CurrentConfigurationInstanceType".to_string(),
                "AutoScalingGroupArn".to_string(),
            ],
        ),
        (
            // A field asked for twice is one column, so neither the file nor the work of
            // building it grows with a list that repeats.
            "a field asked for twice",
            Some(vec![
                "AccountId".to_string(),
                "AutoScalingGroupArn".to_string(),
                "Finding".to_string(),
                "AccountId".to_string(),
            ]),
            vec![
                "AccountId".to_string(),
                "AutoScalingGroupArn".to_string(),
                "Finding".to_string(),
            ],
        ),
        (
            // A request that selects no field gets every field the model declares for the
            // resource type (`ExportableAutoScalingGroupField`).
            "no field selection at all",
            None,
            EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES
                .iter()
                .map(|field| (*field).to_string())
                .collect(),
        ),
    ];

    for (case, fields, columns) in selections {
        // One Auto Scaling group export job is in progress per Region at a time, so each row
        // gets its own store rather than waiting on the previous row's job.
        let router = analysed_emulator().await;
        let mut request = json!({"s3DestinationConfig": {"bucket": BUCKET}});
        if let Some(fields) = &fields {
            request["fieldsToExport"] = json!(fields);
        }
        let body = export_ok(&router, "ExportAutoScalingGroupRecommendations", request).await;
        let (bucket, key, _) = export_destination(case, &body);

        tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
        let (status, exported) = get_object(&router, &bucket, &key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: downloading {key}: got {status}: {}",
            String::from_utf8_lossy(&exported)
        );
        let exported = String::from_utf8(exported)
            .unwrap_or_else(|error| panic!("case {case:?}: the export file is text: {error}"));
        let rows = csv_rows(&exported);
        let header = rows.first().cloned().unwrap_or_default();
        assert_eq!(
            header, columns,
            "case {case:?}: unexpected columns, got {exported:?}"
        );
        let data = rows.get(1..).unwrap_or_default();
        assert_eq!(
            data.len(),
            expected_rows,
            "case {case:?}: expected one row per recommendation option ({expected_rows}), got \
             {exported:?}"
        );

        // Every group the reader reports has its rows, and every row says about the group
        // what the reader says about it.
        for (arn, finding, options) in &groups {
            let group_rows: Vec<&Vec<String>> = data
                .iter()
                .filter(|row| cell(&header, row, "AutoScalingGroupArn") == arn)
                .collect();
            assert_eq!(
                group_rows.len(),
                *options,
                "case {case:?}: expected {options} row(s) for {arn}, got {exported:?}"
            );
            for row in group_rows {
                assert_eq!(
                    cell(&header, row, "Finding"),
                    finding,
                    "case {case:?}: the exported finding of {arn} is the reported one, got {row:?}"
                );
            }
        }

        // The columns an Auto Scaling group recommendation DOES carry are filled, so a
        // selection of them is not a file of empty cells. Each is a member the reader reports
        // for every group of this analysis.
        if fields.is_none() {
            let filled = [
                "AccountId",
                "AutoScalingGroupArn",
                "AutoScalingGroupName",
                "Finding",
                "CurrentPerformanceRisk",
                "UtilizationMetricsCpuMaximum",
                "UtilizationMetricsMemoryMaximum",
                "LookbackPeriodInDays",
                "CurrentConfigurationDesiredCapacity",
                "CurrentConfigurationMinSize",
                "CurrentConfigurationMaxSize",
                "CurrentConfigurationType",
                "CurrentVCpus",
                "CurrentMemory",
                "CurrentOnDemandPrice",
                "EffectiveRecommendationPreferencesLookBackPeriod",
                "EffectiveRecommendationPreferencesInferredWorkloadTypes",
                "EffectiveRecommendationPreferencesSavingsEstimationMode",
                "EffectiveRecommendationPreferencesCpuVendorArchitectures",
                "LastRefreshTimestamp",
                "RecommendationOptionsConfigurationDesiredCapacity",
                "RecommendationOptionsConfigurationType",
                "RecommendationOptionsVcpus",
                "RecommendationOptionsMemory",
                "RecommendationOptionsOnDemandPrice",
                "RecommendationOptionsPerformanceRisk",
                "RecommendationOptionsProjectedUtilizationMetricsCpuMaximum",
                "RecommendationOptionsProjectedUtilizationMetricsMemoryMaximum",
                "RecommendationOptionsMigrationEffort",
                "RecommendationOptionsSavingsOpportunityPercentage",
                "RecommendationOptionsEstimatedMonthlySavingsValue",
                "RecommendationOptionsEstimatedMonthlySavingsCurrency",
            ];
            for column in filled {
                assert!(
                    data.iter()
                        .all(|row| !cell(&header, row, column).is_empty()),
                    "case {case:?}: every exported row carries {column}, got {exported:?}"
                );
            }
        }
    }
}

/// A destination the export can no longer be written to FAILS the job: the bucket is taken
/// away while the job is still running, so the write the clock owes cannot be performed, and
/// the job then reports `Failed` with the reason — rather than `Complete` over objects that
/// are not there.
#[tokio::test]
async fn an_export_whose_destination_is_gone_fails_the_job() {
    let router = analysed_emulator().await;
    let body = export_ok(
        &router,
        "ExportAutoScalingGroupRecommendations",
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    let job_id = body["jobId"].as_str().unwrap_or_default().to_string();

    let status = delete_bucket(&router, BUCKET).await;
    assert_eq!(
        status, 204,
        "deleting the destination bucket: expected 204, got {status}"
    );

    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
    let job = reported_job(&router, "a destination that is gone", &job_id).await;
    assert_eq!(
        job["status"].as_str(),
        Some(JOB_STATUS_FAILED),
        "an export that could not be written reports Failed, got {job}"
    );
    assert!(
        job["failureReason"]
            .as_str()
            .is_some_and(|reason| reason.contains(BUCKET)),
        "a failed export says why, naming the destination it could not write, got {job}"
    );
}

/// The filters an export carries select which recommendations the export file holds — "an
/// array of objects to specify a filter that exports a more specific set of ...
/// recommendations" — and they select exactly what the recommendation READER of the same
/// analysis selects with the same filter, which is the rule each surface's filter vocabulary
/// states.
///
/// The reader is the oracle, so each row asserts the export and the reader AGREE rather than
/// restating a value this analysis happens to hold; the table spans the filter names of every
/// surface whose reader reports from the same stored analysis the export file is built from,
/// and the closing assertion is what keeps it from passing vacuously.
#[tokio::test]
async fn the_requests_filters_select_what_the_reader_selects() {
    // (case, the export operation, its reader, the reader's recommendation list, the filter)
    let rows: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "a group finding the analysis reached",
            "ExportAutoScalingGroupRecommendations",
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
            json!([{"name": "Finding", "values": ["NotOptimized"]}]),
        ),
        (
            "a group finding no Auto Scaling group recommendation can carry",
            "ExportAutoScalingGroupRecommendations",
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
            json!([{"name": "Finding", "values": ["Underprovisioned"]}]),
        ),
        (
            // An `AutoScalingGroupRecommendation` declares no finding reason codes, so this
            // name selects nothing on that surface — for the export as for the reader.
            "a finding reason code on the group surface",
            "ExportAutoScalingGroupRecommendations",
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
            json!([{"name": "FindingReasonCodes", "values": ["MemoryOverprovisioned"]}]),
        ),
        (
            "an inferred workload type the analysis inferred",
            "ExportAutoScalingGroupRecommendations",
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
            json!([{"name": "InferredWorkloadTypes", "values": ["Nginx"]}]),
        ),
        (
            "an inferred workload type it did not",
            "ExportAutoScalingGroupRecommendations",
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
            json!([{"name": "InferredWorkloadTypes", "values": ["Memcached"]}]),
        ),
        (
            "the resource type this export is of",
            "ExportAutoScalingGroupRecommendations",
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
            json!([{"name": "RecommendationSourceType", "values": ["AutoScalingGroup"]}]),
        ),
        (
            "another resource type",
            "ExportAutoScalingGroupRecommendations",
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
            json!([{"name": "RecommendationSourceType", "values": ["EbsVolume"]}]),
        ),
        (
            "a volume finding",
            "ExportEBSVolumeRecommendations",
            "GetEBSVolumeRecommendations",
            "volumeRecommendations",
            json!([{"name": "Finding", "values": ["NotOptimized"]}]),
        ),
        (
            "the other volume finding",
            "ExportEBSVolumeRecommendations",
            "GetEBSVolumeRecommendations",
            "volumeRecommendations",
            json!([{"name": "Finding", "values": ["Optimized"]}]),
        ),
        (
            // `LicenseName` names the licensed product, not an exportable field of the same
            // spelling: the export resolves it to the license configuration's name.
            "the licensed product the inventory holds",
            "ExportLicenseRecommendations",
            "GetLicenseRecommendations",
            "licenseRecommendations",
            json!([{"name": "LicenseName", "values": ["SQLServer"]}]),
        ),
        (
            "a license finding reason code",
            "ExportLicenseRecommendations",
            "GetLicenseRecommendations",
            "licenseRecommendations",
            json!([{"name": "FindingReasonCode", "values": ["LicenseOverprovisioned"]}]),
        ),
        (
            "another license finding reason code",
            "ExportLicenseRecommendations",
            "GetLicenseRecommendations",
            "licenseRecommendations",
            json!([{"name": "FindingReasonCode", "values": ["CloudWatchApplicationInsightsError"]}]),
        ),
        (
            "a database instance finding",
            "ExportRDSDatabaseRecommendations",
            "GetRDSDatabaseRecommendations",
            "rdsDBRecommendations",
            json!([{"name": "InstanceFinding", "values": ["Overprovisioned"]}]),
        ),
        (
            "a database storage finding",
            "ExportRDSDatabaseRecommendations",
            "GetRDSDatabaseRecommendations",
            "rdsDBRecommendations",
            json!([{"name": "StorageFinding", "values": ["Optimized"]}]),
        ),
        (
            "a database instance finding reason code",
            "ExportRDSDatabaseRecommendations",
            "GetRDSDatabaseRecommendations",
            "rdsDBRecommendations",
            json!([{"name": "InstanceFindingReasonCode", "values": ["CPUOverprovisioned"]}]),
        ),
        (
            "a database storage finding reason code",
            "ExportRDSDatabaseRecommendations",
            "GetRDSDatabaseRecommendations",
            "rdsDBRecommendations",
            json!([{"name": "StorageFindingReasonCode", "values": ["EBSVolumeIOPSOverprovisioned"]}]),
        ),
        (
            "a database that is not idle",
            "ExportRDSDatabaseRecommendations",
            "GetRDSDatabaseRecommendations",
            "rdsDBRecommendations",
            json!([{"name": "Idle", "values": ["False"]}]),
        ),
        (
            "a database that is idle",
            "ExportRDSDatabaseRecommendations",
            "GetRDSDatabaseRecommendations",
            "rdsDBRecommendations",
            json!([{"name": "Idle", "values": ["True"]}]),
        ),
        (
            "a lambda function finding",
            "ExportLambdaFunctionRecommendations",
            "GetLambdaFunctionRecommendations",
            "lambdaFunctionRecommendations",
            json!([{"name": "Finding", "values": ["NotOptimized"]}]),
        ),
        (
            "a lambda function finding reason code",
            "ExportLambdaFunctionRecommendations",
            "GetLambdaFunctionRecommendations",
            "lambdaFunctionRecommendations",
            json!([{"name": "FindingReasonCode", "values": ["MemoryOverprovisioned"]}]),
        ),
        (
            "an idle resource type the analysis found",
            "ExportIdleRecommendations",
            "GetIdleRecommendations",
            "idleRecommendations",
            json!([{"name": "ResourceType", "values": ["NatGateway"]}]),
        ),
        (
            "an idle resource type it did not",
            "ExportIdleRecommendations",
            "GetIdleRecommendations",
            "idleRecommendations",
            json!([{"name": "ResourceType", "values": ["WorkSpaces"]}]),
        ),
    ];

    let mut saw_selected = false;
    let mut saw_nothing = false;
    for (case, export, reader, member, filters) in rows {
        // A fresh emulator per row: one export job of a resource type is in progress per
        // Region at a time.
        let router = analysed_emulator().await;
        let (status, reported) = call(&router, reader, json!({"filters": filters.clone()})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading with {filters}: got {status}: {reported}"
        );
        let selected = reported[member].as_array().map_or(0, Vec::len);

        let body = export_ok(
            &router,
            export,
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "filters": filters.clone()}),
        )
        .await;
        let (bucket, key, _) = export_destination(case, &body);
        tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
        let (status, exported) = get_object(&router, &bucket, &key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: downloading {key}: got {status}: {}",
            String::from_utf8_lossy(&exported)
        );
        let exported = String::from_utf8(exported)
            .unwrap_or_else(|error| panic!("case {case:?}: the export file is text: {error}"));
        let data = csv_rows(&exported).len().saturating_sub(1);
        assert_eq!(
            data > 0,
            selected > 0,
            "case {case:?}: the filter {filters} selected {selected} recommendation(s) for the \
             reader but {data} row(s) for the export, got {exported:?}"
        );
        saw_selected |= selected > 0;
        saw_nothing |= selected == 0;
    }
    assert!(
        saw_selected && saw_nothing,
        "the table must span a filter that selects recommendations and one that selects none, \
         or it proves nothing (selected some: {saw_selected}, selected none: {saw_nothing})"
    );
}
