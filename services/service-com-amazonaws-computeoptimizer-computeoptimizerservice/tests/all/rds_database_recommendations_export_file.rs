// SPDX-License-Identifier: CC0-1.0

//! The CONTENT of the file `ExportRDSDatabaseRecommendations` writes.
//!
//! The rule under test: a recommendations export file reports, in each cell, the value the
//! reader of the SAME analysis reports for the recommendation member that exportable field
//! names, and a cell is empty only where the analysis holds no value for it. So every
//! `ExportableRDSDBField` a request selects — the database's engine and DB instance class, its
//! storage configuration, the utilization observed on it, the instance and storage options the
//! analysis would move it to and what adopting them saves — must carry what
//! `GetRDSDatabaseRecommendations` reports for the same database, and only the fields that name
//! no member of an `RDSDBRecommendation` are blank.
//!
//! The reader is the ORACLE for every cell, so no expectation here restates a figure this
//! analysis happens to hold. It is asserted over a TABLE of field selections — including the
//! whole modelled field set a request that selects none gets — against an analysis whose rows
//! span the class the rule generalises over: a database the analysis would move the instance
//! of, one it would move the storage of, one it would not move at all, a cluster member with a
//! promotion tier and tags, and a row whose database configuration the analysis never recorded.
//!
//! The rules this export shares with every other `Export*Recommendations` operation of the
//! service — the destination bucket must exist, both objects appear once the job completes —
//! have their own suite (`recommendation_export_objects.rs`), and the job record itself
//! `rds_database_recommendations_export.rs`; this one is about the database surface's own file.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::{
    DBStorageConfiguration, RDSDBUtilizationMetric, Tag, EXPORTABLE_R_D_S_D_B_FIELD_VALUES,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    round_hundredths, service_state, AnalysedResource, RdsDatabaseDetail, ANALYSIS_DELAY,
    EXPORT_JOB_STEP, JOB_STATUS_COMPLETE, RESOURCE_TYPE_RDS_DB_INSTANCE,
    SOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cell, create_bucket, csv_rows, emulator_with_engine, export_destination, get_object,
    opt_in,
};

/// The operation under test.
const EXPORT: &str = "ExportRDSDatabaseRecommendations";

/// The reader of the same analysis: the oracle every expected cell is derived from.
const READER: &str = "GetRDSDatabaseRecommendations";

/// The destination bucket these fixtures export to. Compute Optimizer does not create it, so
/// each fixture creates it through Amazon S3 first.
const BUCKET: &str = "rds-recommendation-export-files";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// How a cell holding several values separates them in the export file, so one exportable
/// field is still one column. AWS publishes no layout for the file, so this is the emulator's
/// own, asserted here as what a client reading the file sees.
const LIST_SEPARATOR: &str = "|";

/// The column whose cell states an instant rather than text.
const TIMESTAMP_COLUMN: &str = "LastRefreshTimestamp";

/// The `ExportableRDSDBField` column each `RDSDBMetricName` the analysis can observe lands in.
///
/// Written out from the model's own two enums (`smithy_query query="shape"
/// shape="ExportableRDSDBField,RDSDBMetricName"`): the field set names exactly one column per
/// metric, carrying the statistic the export file reports that metric under — which is not
/// always the `Maximum` statistic this API reports utilization metrics with
/// (`RDSDBUtilizationMetric$statistic`), and which spells two metric names differently from the
/// metric enum (`CPU` as `Cpu`, `AuroraMemoryNumDeclinedSql` as
/// `AuroraMemoryNumDeclinedSqlTotal`).
const METRIC_COLUMNS: &[(&str, &str)] = &[
    ("CPU", "UtilizationMetricsCpuMaximum"),
    ("Memory", "UtilizationMetricsMemoryMaximum"),
    (
        "EBSVolumeStorageSpaceUtilization",
        "UtilizationMetricsEBSVolumeStorageSpaceUtilizationMaximum",
    ),
    (
        "NetworkReceiveThroughput",
        "UtilizationMetricsNetworkReceiveThroughputMaximum",
    ),
    (
        "NetworkTransmitThroughput",
        "UtilizationMetricsNetworkTransmitThroughputMaximum",
    ),
    (
        "EBSVolumeReadIOPS",
        "UtilizationMetricsEBSVolumeReadIOPSMaximum",
    ),
    (
        "EBSVolumeWriteIOPS",
        "UtilizationMetricsEBSVolumeWriteIOPSMaximum",
    ),
    (
        "EBSVolumeReadThroughput",
        "UtilizationMetricsEBSVolumeReadThroughputMaximum",
    ),
    (
        "EBSVolumeWriteThroughput",
        "UtilizationMetricsEBSVolumeWriteThroughputMaximum",
    ),
    (
        "DatabaseConnections",
        "UtilizationMetricsDatabaseConnectionsMaximum",
    ),
    (
        "StorageNetworkReceiveThroughput",
        "UtilizationMetricsStorageNetworkReceiveThroughputMaximum",
    ),
    (
        "StorageNetworkTransmitThroughput",
        "UtilizationMetricsStorageNetworkTransmitThroughputMaximum",
    ),
    (
        "AuroraMemoryHealthState",
        "UtilizationMetricsAuroraMemoryHealthStateMaximum",
    ),
    (
        "AuroraMemoryNumDeclinedSql",
        "UtilizationMetricsAuroraMemoryNumDeclinedSqlTotalMaximum",
    ),
    (
        "AuroraMemoryNumKillConnTotal",
        "UtilizationMetricsAuroraMemoryNumKillConnTotalMaximum",
    ),
    (
        "AuroraMemoryNumKillQueryTotal",
        "UtilizationMetricsAuroraMemoryNumKillQueryTotalMaximum",
    ),
    (
        "ReadIOPSEphemeralStorage",
        "UtilizationMetricsReadIOPSEphemeralStorageMaximum",
    ),
    (
        "WriteIOPSEphemeralStorage",
        "UtilizationMetricsWriteIOPSEphemeralStorageMaximum",
    ),
    ("VolumeReadIOPs", "UtilizationMetricsVolumeReadIOPsAverage"),
    ("VolumeBytesUsed", "UtilizationMetricsVolumeBytesUsedAverage"),
    ("VolumeWriteIOPs", "UtilizationMetricsVolumeWriteIOPsAverage"),
];

/// The one projected-utilization column the field set declares: the CPU share the analysis
/// projects onto the DB instance class its option names.
const PROJECTED_CPU_COLUMN: &str =
    "InstanceRecommendationOptionsProjectedUtilizationMetricsCpuMaximum";

/// The `ExportableRDSDBField` values that name no member of an `RDSDBRecommendation`, so the
/// analysis holds no value for them and their cells are empty: whether the database is a
/// Multi-AZ deployment or the writer of its cluster, and the per-hour and per-month PRICES of
/// the current and recommended configurations together with the cluster-level cost roll-ups —
/// this analysis records a database's cost per month as a whole, and what the recommendation
/// reports is the money adopting it saves.
const NO_MEMBER_COLUMNS: &[&str] = &[
    "MultiAZDBInstance",
    "ClusterWriter",
    "CurrentInstanceOnDemandHourlyPrice",
    "CurrentStorageOnDemandMonthlyPrice",
    "CurrentStorageEstimatedClusterInstanceOnDemandMonthlyCost",
    "CurrentStorageEstimatedClusterStorageOnDemandMonthlyCost",
    "CurrentStorageEstimatedClusterStorageIOOnDemandMonthlyCost",
    "InstanceRecommendationOptionsInstanceOnDemandHourlyPrice",
    "StorageRecommendationOptionsOnDemandMonthlyPrice",
    "StorageRecommendationOptionsEstimatedClusterInstanceOnDemandMonthlyCost",
    "StorageRecommendationOptionsEstimatedClusterStorageOnDemandMonthlyCost",
    "StorageRecommendationOptionsEstimatedClusterStorageIOOnDemandMonthlyCost",
];

/// A storage configuration (`DBStorageConfiguration`); `storageType` is a free-form string in
/// the model, so these are the EBS volume type names RDS reports.
fn storage(storage_type: &str, allocated: i32, iops: i32) -> DBStorageConfiguration {
    DBStorageConfiguration {
        storage_type: Some(storage_type.to_string()),
        allocated_storage: allocated,
        iops: Some(iops),
        max_allocated_storage: Some(allocated * 2),
        storage_throughput: Some(125),
    }
}

/// One observed utilization metric (`RDSDBMetricName` / `RDSDBMetricStatistic` values).
fn metric(name: &str, statistic: &str, value: f64) -> RDSDBUtilizationMetric {
    RDSDBUtilizationMetric {
        name: Some(name.to_string()),
        statistic: Some(statistic.to_string()),
        value,
    }
}

/// One analysed database as the findings catalog records it. The catalog records the
/// resource's region-free identity: the ARN reported for it names the region of the endpoint
/// that reports it.
fn analysed_database(
    arn_resource: &str,
    finding: &str,
    finding_reason_codes: &[&str],
    current_performance_risk: &str,
    costs: (f64, f64),
    rds_database: Option<RdsDatabaseDetail>,
) -> AnalysedResource {
    AnalysedResource {
        account_id: ::engine::emulator_account_id().to_string(),
        resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
        arn_service: "rds".to_string(),
        arn_resource: arn_resource.to_string(),
        finding: finding.to_string(),
        finding_reason_codes: finding_reason_codes
            .iter()
            .map(|code| (*code).to_string())
            .collect(),
        current_performance_risk: current_performance_risk.to_string(),
        current_monthly_cost: costs.0,
        recommended_monthly_cost: costs.1,
        inferred_workload_types: Vec::new(),
        rds_database,
    }
}

/// The databases this suite adds to the analysis: the class the rule generalises over, each
/// row differing in what the analysis recorded for it.
fn corpus() -> Vec<AnalysedResource> {
    vec![
        // An Aurora cluster member the analysis would move to a smaller DB instance class: it
        // carries a promotion tier, a cluster identifier, tags, and metrics the field set names
        // with another statistic (`VolumeBytesUsed`) or spells differently
        // (`AuroraMemoryNumDeclinedSql`).
        analysed_database(
            "db:orders-writer",
            "Overprovisioned",
            // One `RDSInstanceFindingReasonCode` and one code of neither RDS vocabulary.
            &["CPUOverprovisioned", "MemoryOverprovisioned"],
            "Low",
            (200.0, 150.0),
            Some(RdsDatabaseDetail {
                engine: Some("aurora-mysql".to_string()),
                engine_version: Some("3.05.2".to_string()),
                promotion_tier: Some(1),
                db_cluster_identifier: Some("orders-cluster".to_string()),
                current_db_instance_class: Some("db.r6g.4xlarge".to_string()),
                recommended_db_instance_class: Some("db.r6g.2xlarge".to_string()),
                current_storage_configuration: Some(storage("gp3", 400, 3_000)),
                recommended_storage_configuration: None,
                utilization_metrics: vec![
                    metric("CPU", "Maximum", 12.0),
                    metric("Memory", "Maximum", 31.25),
                    metric("AuroraMemoryNumDeclinedSql", "Maximum", 3.0),
                    metric("VolumeBytesUsed", "Average", 1_234.5),
                ],
                last_refresh_timestamp: None,
                tags: vec![
                    Tag {
                        key: Some("Owner".to_string()),
                        value: Some("TeamDb".to_string()),
                    },
                    Tag {
                        key: Some("Tier".to_string()),
                        value: Some("prod".to_string()),
                    },
                ],
            }),
        ),
        // The one finding only `RDSStorageFinding` declares: it is the STORAGE the analysis
        // would move, so the storage option and its columns are the filled ones.
        analysed_database(
            "cluster:ledger-cluster",
            "NotOptimized",
            // `RDSStorageFindingReasonCode` values.
            &[
                "EBSVolumeIOPSOverprovisioned",
                "NewGenerationStorageTypeAvailable",
            ],
            "Medium",
            (60.0, 40.0),
            Some(RdsDatabaseDetail {
                engine: Some("mysql".to_string()),
                engine_version: Some("8.0.35".to_string()),
                current_db_instance_class: Some("db.m6g.large".to_string()),
                recommended_db_instance_class: None,
                current_storage_configuration: Some(storage("io1", 200, 5_000)),
                recommended_storage_configuration: Some(storage("gp3", 200, 3_000)),
                utilization_metrics: vec![metric("EBSVolumeReadIOPS", "Maximum", 42.0)],
                ..RdsDatabaseDetail::default()
            }),
        ),
        // Under-provisioned: the recommendation is LARGER, so the configuration changes but
        // nothing is saved.
        analysed_database(
            "db:reports-primary",
            "Underprovisioned",
            &["MemoryUnderprovisioned"],
            "High",
            (100.0, 180.0),
            Some(RdsDatabaseDetail {
                engine: Some("postgres".to_string()),
                engine_version: Some("14.9".to_string()),
                current_db_instance_class: Some("db.t4g.medium".to_string()),
                recommended_db_instance_class: Some("db.t4g.large".to_string()),
                ..RdsDatabaseDetail::default()
            }),
        ),
        // Optimized, and with no database configuration recorded at all: nothing to move, so
        // neither option is reported and the configuration columns are empty.
        analysed_database("db:audit-archive", "Optimized", &[], "VeryLow", (90.0, 90.0), None),
    ]
}

/// An emulator with the destination bucket created, the analysis holding [`corpus`] beside the
/// store's own rows, the account opted in and the analysis its enrollment starts already due —
/// which is what the reader reports over, so it is what the export file holds.
async fn analysed_emulator() -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    create_bucket(&router, BUCKET).await;
    record(&engine, &corpus());
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
    router
}

/// Record `resources` into the findings catalog, the way Compute Optimizer's own analysis
/// would: no operation of this API creates a recommendation.
fn record(engine: &Arc<engine::InMemoryEngine>, resources: &[AnalysedResource]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for resource in resources {
        guard.analysed_resources.push(resource.clone());
    }
}

/// Export the account's database recommendations, asserting the call was accepted, and answer
/// the whole response body.
async fn export_ok(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, EXPORT, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    body
}

/// The recommendations the reader reports, in the order it reports them.
async fn reader_recommendations(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let (status, body) = call(router, READER, json!({"maxResults": 100})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the recommendations: got {status}: {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: the analysis fits one page, got {body}"
    );
    let reported = body["rdsDBRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a list, got {body}"))
        .clone();
    assert!(
        reported.len() > 1,
        "case {case:?}: the analysis holds several database recommendations, got {body}"
    );
    reported
}

/// Poll the job to `Complete`, the documented export-then-download workflow.
async fn polled_to_complete(router: &MultiServiceRouter, case: &str, job_id: &str) {
    let mut status = String::new();
    for _ in 0..10 {
        let (code, body) = call(
            router,
            "DescribeRecommendationExportJobs",
            json!({"jobIds": [job_id]}),
        )
        .await;
        assert_eq!(code, 200, "case {case:?}: reading job {job_id}: got {body}");
        let job = &body["recommendationExportJobs"][0];
        status = job["status"].as_str().unwrap_or_default().to_string();
        if status == JOB_STATUS_COMPLETE {
            assert!(
                job["failureReason"].is_null(),
                "case {case:?}: a completed export carries no failure reason, got {job}"
            );
            return;
        }
        tokio::time::sleep(EXPORT_JOB_STEP).await;
    }
    panic!("case {case:?}: the job never completed, last status {status:?}");
}

/// Download one object, asserting it was readable, and answer it as text.
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

/// A string member, or the empty cell where the reader reports none.
fn text(reported: &Value, member: &str) -> String {
    reported[member].as_str().unwrap_or_default().to_string()
}

/// A list-valued member as one cell ([`LIST_SEPARATOR`]).
fn list_cell(reported: &Value) -> String {
    reported
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
                .join(LIST_SEPARATOR)
        })
        .unwrap_or_default()
}

/// The value the reader reports for the metric `name` of `metrics`, whatever statistic it
/// reports it with.
fn reported_metric(metrics: &Value, name: &str) -> Option<f64> {
    metrics
        .as_array()?
        .iter()
        .find(|metric| metric["name"].as_str() == Some(name))
        .and_then(|metric| metric["value"].as_f64())
}

/// The cell the export file must carry under `column` for the recommendation the READER
/// reports — the whole expectation for a row, derived from the reader's own answer.
///
/// Every value of `ExportableRDSDBField` is covered: the members an `RDSDBRecommendation`
/// reports (including those of its instance and storage options), one column per observed
/// metric ([`METRIC_COLUMNS`]), and the fields that name no member at all
/// ([`NO_MEMBER_COLUMNS`]). The timestamp column is compared as an instant by the caller.
fn expected_cell(column: &str, reported: &Value) -> String {
    let instance = &reported["instanceRecommendationOptions"][0];
    let storage = &reported["storageRecommendationOptions"][0];
    let preferences = &reported["effectiveRecommendationPreferences"];
    match column {
        "AccountId" => text(reported, "accountId"),
        "ResourceArn" => text(reported, "resourceArn"),
        "Engine" => text(reported, "engine"),
        "EngineVersion" => text(reported, "engineVersion"),
        "Idle" => text(reported, "idle"),
        "DBClusterIdentifier" => text(reported, "dbClusterIdentifier"),
        "PromotionTier" => number_cell(reported["promotionTier"].as_f64()),
        "CurrentDBInstanceClass" => text(reported, "currentDBInstanceClass"),
        "CurrentStorageConfigurationStorageType" => {
            text(&reported["currentStorageConfiguration"], "storageType")
        }
        "CurrentStorageConfigurationAllocatedStorage" => {
            number_cell(reported["currentStorageConfiguration"]["allocatedStorage"].as_f64())
        }
        "CurrentStorageConfigurationIOPS" => {
            number_cell(reported["currentStorageConfiguration"]["iops"].as_f64())
        }
        "CurrentStorageConfigurationMaxAllocatedStorage" => {
            number_cell(reported["currentStorageConfiguration"]["maxAllocatedStorage"].as_f64())
        }
        "CurrentStorageConfigurationStorageThroughput" => {
            number_cell(reported["currentStorageConfiguration"]["storageThroughput"].as_f64())
        }
        "CurrentStorageEstimatedMonthlyVolumeIOPsCostVariation" => {
            text(reported, "currentStorageEstimatedMonthlyVolumeIOPsCostVariation")
        }
        "CurrentInstancePerformanceRisk" => text(reported, "currentInstancePerformanceRisk"),
        "InstanceFinding" => text(reported, "instanceFinding"),
        "StorageFinding" => text(reported, "storageFinding"),
        "InstanceFindingReasonCodes" => list_cell(&reported["instanceFindingReasonCodes"]),
        "StorageFindingReasonCodes" => list_cell(&reported["storageFindingReasonCodes"]),
        "LookbackPeriodInDays" => number_cell(reported["lookbackPeriodInDays"].as_f64()),
        "Tags" => reported["tags"]
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
        // The DB instance recommendation option.
        "InstanceRecommendationOptionsDBInstanceClass" => text(instance, "dbInstanceClass"),
        "InstanceRecommendationOptionsRank" => number_cell(instance["rank"].as_f64()),
        "InstanceRecommendationOptionsPerformanceRisk" => {
            number_cell(instance["performanceRisk"].as_f64())
        }
        PROJECTED_CPU_COLUMN => number_cell(reported_metric(
            &instance["projectedUtilizationMetrics"],
            "CPU",
        )),
        "InstanceRecommendationOptionsSavingsOpportunityPercentage" => {
            number_cell(instance["savingsOpportunity"]["savingsOpportunityPercentage"].as_f64())
        }
        "InstanceRecommendationOptionsEstimatedMonthlySavingsCurrency" => {
            text(&instance["savingsOpportunity"]["estimatedMonthlySavings"], "currency")
        }
        "InstanceRecommendationOptionsEstimatedMonthlySavingsValue" => number_cell(
            instance["savingsOpportunity"]["estimatedMonthlySavings"]["value"].as_f64(),
        ),
        "InstanceRecommendationOptionsSavingsOpportunityAfterDiscountsPercentage" => number_cell(
            instance["savingsOpportunityAfterDiscounts"]["savingsOpportunityPercentage"].as_f64(),
        ),
        "InstanceRecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts" => text(
            &instance["savingsOpportunityAfterDiscounts"]["estimatedMonthlySavings"],
            "currency",
        ),
        "InstanceRecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts" => number_cell(
            instance["savingsOpportunityAfterDiscounts"]["estimatedMonthlySavings"]["value"]
                .as_f64(),
        ),
        // The DB storage recommendation option, whose configuration is the one the analysis
        // would move the storage to.
        "StorageRecommendationOptionsStorageType" => {
            text(&storage["storageConfiguration"], "storageType")
        }
        "StorageRecommendationOptionsAllocatedStorage" => {
            number_cell(storage["storageConfiguration"]["allocatedStorage"].as_f64())
        }
        "StorageRecommendationOptionsIOPS" => {
            number_cell(storage["storageConfiguration"]["iops"].as_f64())
        }
        "StorageRecommendationOptionsMaxAllocatedStorage" => {
            number_cell(storage["storageConfiguration"]["maxAllocatedStorage"].as_f64())
        }
        "StorageRecommendationOptionsStorageThroughput" => {
            number_cell(storage["storageConfiguration"]["storageThroughput"].as_f64())
        }
        "StorageRecommendationOptionsRank" => number_cell(storage["rank"].as_f64()),
        "StorageRecommendationOptionsEstimatedMonthlyVolumeIOPsCostVariation" => {
            text(storage, "estimatedMonthlyVolumeIOPsCostVariation")
        }
        "StorageRecommendationOptionsSavingsOpportunityPercentage" => {
            number_cell(storage["savingsOpportunity"]["savingsOpportunityPercentage"].as_f64())
        }
        "StorageRecommendationOptionsEstimatedMonthlySavingsCurrency" => {
            text(&storage["savingsOpportunity"]["estimatedMonthlySavings"], "currency")
        }
        "StorageRecommendationOptionsEstimatedMonthlySavingsValue" => number_cell(
            storage["savingsOpportunity"]["estimatedMonthlySavings"]["value"].as_f64(),
        ),
        "StorageRecommendationOptionsSavingsOpportunityAfterDiscountsPercentage" => number_cell(
            storage["savingsOpportunityAfterDiscounts"]["savingsOpportunityPercentage"].as_f64(),
        ),
        "StorageRecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts" => text(
            &storage["savingsOpportunityAfterDiscounts"]["estimatedMonthlySavings"],
            "currency",
        ),
        "StorageRecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts" => number_cell(
            storage["savingsOpportunityAfterDiscounts"]["estimatedMonthlySavings"]["value"]
                .as_f64(),
        ),
        // The preferences the recommendation says it was generated under.
        "EffectiveRecommendationPreferencesCpuVendorArchitectures" => {
            list_cell(&preferences["cpuVendorArchitectures"])
        }
        "EffectiveRecommendationPreferencesEnhancedInfrastructureMetrics" => {
            text(preferences, "enhancedInfrastructureMetrics")
        }
        "EffectiveRecommendationPreferencesLookBackPeriod" => {
            text(preferences, "lookBackPeriod")
        }
        "EffectiveRecommendationPreferencesSavingsEstimationMode" => {
            text(&preferences["savingsEstimationMode"], "source")
        }
        column if NO_MEMBER_COLUMNS.contains(&column) => String::new(),
        column => METRIC_COLUMNS
            .iter()
            .find(|(_, field)| *field == column)
            .map(|(name, _)| {
                number_cell(reported_metric(&reported["utilizationMetrics"], name))
            })
            .unwrap_or_else(|| {
                panic!("column {column:?} is not covered by this expectation")
            }),
    }
}

/// The instant one cell of the timestamp column states, as a count of whole milliseconds —
/// `None` for a cell that states no instant at all.
///
/// The file states the instant in RFC 3339 while a modelled timestamp crosses the wire as a
/// JSON NUMBER, and an `f64` of a present-day instant carries less than a microsecond, so the
/// two renderings of one instant can land on neighbouring microseconds. Both sides are
/// therefore compared at the millisecond resolution an AWS JSON timestamp states.
fn stated_millis(cell: &str) -> Option<i64> {
    let at = ::aws_smithy_types::DateTime::from_str(
        cell,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(millis_of(at))
}

/// One `DateTime` as a count of whole milliseconds since the epoch.
fn millis_of(at: ::aws_smithy_types::DateTime) -> i64 {
    at.secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(at.subsec_nanos() / 1_000_000))
}

/// The instant this account's analysis was generated, as another reader of the same analysis
/// reports it: one analysis has ONE refresh instant across every reader of it
/// (`timestamp_probe.rs`), and it is what an export file states where the analysed row records
/// no instant of its own.
async fn analysis_instant(router: &MultiServiceRouter, case: &str) -> f64 {
    let (status, body) = call(router, "GetEBSVolumeRecommendations", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the analysis instant: got {status}: {body}"
    );
    body["volumeRecommendations"][0]["lastRefreshTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a reader states when the analysis was generated, got {body}"))
}

/// The catalog above is the model's: every column it names is an `ExportableRDSDBField`, it
/// covers every utilization field the field set declares, and every field of that set has an
/// expectation — so a field the model adds fails here rather than going unasserted.
#[test]
fn the_expectation_covers_every_modelled_field() {
    for (metric, column) in METRIC_COLUMNS {
        assert!(
            EXPORTABLE_R_D_S_D_B_FIELD_VALUES.contains(column),
            "case {metric:?}: {column} is not an ExportableRDSDBField"
        );
    }
    let declared = EXPORTABLE_R_D_S_D_B_FIELD_VALUES
        .iter()
        .filter(|field| field.starts_with("UtilizationMetrics"))
        .count();
    assert_eq!(
        METRIC_COLUMNS.len(),
        declared,
        "the catalog must name one metric per utilization field the model declares, got {} of \
         {declared}",
        METRIC_COLUMNS.len()
    );
    // Every field the model declares has an expectation: `expected_cell` panics on any other.
    // The timestamp column is the one the caller compares as an instant rather than as text.
    for field in EXPORTABLE_R_D_S_D_B_FIELD_VALUES {
        if *field == TIMESTAMP_COLUMN {
            continue;
        }
        let expected = expected_cell(field, &json!({}));
        assert!(
            expected.is_empty(),
            "field {field:?}: a recommendation reporting nothing has an empty cell, got \
             {expected:?}"
        );
    }
}

/// The export file holds the recommendations `GetRDSDatabaseRecommendations` reports for the
/// same analysis: one row per recommendation, in the order the reader reports them, every cell
/// the value the reader reports for the `ExportableRDSDBField` naming that column — and the
/// header is exactly the fields the request asked for, or the whole modelled field set when it
/// asked for none. One row per field selection.
#[tokio::test]
async fn the_export_file_holds_what_the_database_reader_reports() {
    let every_field: Vec<&str> = EXPORTABLE_R_D_S_D_B_FIELD_VALUES.to_vec();

    // (case, the `fieldsToExport` the request carries, the columns the file must then carry)
    type Selection = (&'static str, Option<Vec<&'static str>>, Vec<&'static str>);
    let selections: Vec<Selection> = vec![
        (
            "the resource and what it runs",
            Some(vec![
                "AccountId",
                "ResourceArn",
                "Engine",
                "EngineVersion",
                "CurrentDBInstanceClass",
            ]),
            vec![
                "AccountId",
                "ResourceArn",
                "Engine",
                "EngineVersion",
                "CurrentDBInstanceClass",
            ],
        ),
        (
            "the two findings and their reason codes, in the request's own order",
            Some(vec![
                "StorageFindingReasonCodes",
                "StorageFinding",
                "InstanceFindingReasonCodes",
                "InstanceFinding",
                "Idle",
                "ResourceArn",
            ]),
            vec![
                "StorageFindingReasonCodes",
                "StorageFinding",
                "InstanceFindingReasonCodes",
                "InstanceFinding",
                "Idle",
                "ResourceArn",
            ],
        ),
        (
            "the current storage configuration and the cluster the database belongs to",
            Some(vec![
                "ResourceArn",
                "DBClusterIdentifier",
                "PromotionTier",
                "CurrentStorageConfigurationStorageType",
                "CurrentStorageConfigurationAllocatedStorage",
                "CurrentStorageConfigurationIOPS",
                "CurrentStorageConfigurationMaxAllocatedStorage",
                "CurrentStorageConfigurationStorageThroughput",
                "Tags",
            ]),
            vec![
                "ResourceArn",
                "DBClusterIdentifier",
                "PromotionTier",
                "CurrentStorageConfigurationStorageType",
                "CurrentStorageConfigurationAllocatedStorage",
                "CurrentStorageConfigurationIOPS",
                "CurrentStorageConfigurationMaxAllocatedStorage",
                "CurrentStorageConfigurationStorageThroughput",
                "Tags",
            ],
        ),
        (
            // A field asked for twice is one column, so neither the file nor the work of
            // building it grows with a list that repeats.
            "the instance option, its projection and its savings, one field asked for twice",
            Some(vec![
                "ResourceArn",
                "InstanceRecommendationOptionsDBInstanceClass",
                "InstanceRecommendationOptionsRank",
                "InstanceRecommendationOptionsPerformanceRisk",
                PROJECTED_CPU_COLUMN,
                "InstanceRecommendationOptionsSavingsOpportunityPercentage",
                "InstanceRecommendationOptionsEstimatedMonthlySavingsCurrency",
                "InstanceRecommendationOptionsEstimatedMonthlySavingsValue",
                "ResourceArn",
            ]),
            vec![
                "ResourceArn",
                "InstanceRecommendationOptionsDBInstanceClass",
                "InstanceRecommendationOptionsRank",
                "InstanceRecommendationOptionsPerformanceRisk",
                PROJECTED_CPU_COLUMN,
                "InstanceRecommendationOptionsSavingsOpportunityPercentage",
                "InstanceRecommendationOptionsEstimatedMonthlySavingsCurrency",
                "InstanceRecommendationOptionsEstimatedMonthlySavingsValue",
            ],
        ),
        (
            "the storage option and its savings",
            Some(vec![
                "ResourceArn",
                "StorageRecommendationOptionsStorageType",
                "StorageRecommendationOptionsAllocatedStorage",
                "StorageRecommendationOptionsIOPS",
                "StorageRecommendationOptionsRank",
                "StorageRecommendationOptionsSavingsOpportunityPercentage",
                "StorageRecommendationOptionsEstimatedMonthlySavingsValue",
            ]),
            vec![
                "ResourceArn",
                "StorageRecommendationOptionsStorageType",
                "StorageRecommendationOptionsAllocatedStorage",
                "StorageRecommendationOptionsIOPS",
                "StorageRecommendationOptionsRank",
                "StorageRecommendationOptionsSavingsOpportunityPercentage",
                "StorageRecommendationOptionsEstimatedMonthlySavingsValue",
            ],
        ),
        (
            // Metric columns the field set names with another statistic or another spelling
            // than the metric enum, beside a field that names no member of a recommendation.
            "utilization columns, the preferences and a field no member fills",
            Some(vec![
                "ResourceArn",
                "UtilizationMetricsCpuMaximum",
                "UtilizationMetricsMemoryMaximum",
                "UtilizationMetricsAuroraMemoryNumDeclinedSqlTotalMaximum",
                "UtilizationMetricsVolumeBytesUsedAverage",
                "EffectiveRecommendationPreferencesLookBackPeriod",
                "EffectiveRecommendationPreferencesSavingsEstimationMode",
                "LookbackPeriodInDays",
                "MultiAZDBInstance",
                TIMESTAMP_COLUMN,
            ]),
            vec![
                "ResourceArn",
                "UtilizationMetricsCpuMaximum",
                "UtilizationMetricsMemoryMaximum",
                "UtilizationMetricsAuroraMemoryNumDeclinedSqlTotalMaximum",
                "UtilizationMetricsVolumeBytesUsedAverage",
                "EffectiveRecommendationPreferencesLookBackPeriod",
                "EffectiveRecommendationPreferencesSavingsEstimationMode",
                "LookbackPeriodInDays",
                "MultiAZDBInstance",
                TIMESTAMP_COLUMN,
            ],
        ),
        (
            // A request that selects no field gets every field the model declares for the
            // resource type (`ExportableRDSDBField`).
            "no field selection at all",
            None,
            every_field,
        ),
    ];

    for (case, fields, columns) in selections {
        // A fresh emulator per row: one database export job is in progress per Region at a
        // time, so each row gets its own store rather than waiting on the previous row's job.
        let router = analysed_emulator().await;
        let reported = reader_recommendations(&router, case).await;
        let generated = analysis_instant(&router, case).await;

        let mut request = json!({"s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "rds"}});
        if let Some(fields) = &fields {
            request["fieldsToExport"] = json!(fields);
        }
        let body = export_ok(&router, case, request).await;
        let job_id = body["jobId"].as_str().unwrap_or_default().to_string();
        let (bucket, key, _) = export_destination(case, &body);
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
            let database = text(recommendation, "resourceArn");
            for column in &header {
                let held = cell(&header, row, column);
                if column == TIMESTAMP_COLUMN {
                    // "The timestamp of when the DB instance recommendation was last
                    // generated": the instant the recommendation reports where the analysed row
                    // records one, and otherwise the instant the account's analysis was
                    // generated, which every export file of this service states.
                    let expected = recommendation["lastRefreshTimestamp"]
                        .as_f64()
                        .unwrap_or(generated);
                    assert_eq!(
                        stated_millis(held),
                        Some(millis_of(::aws_smithy_types::DateTime::from_secs_f64(
                            expected
                        ))),
                        "case {case:?}: column {column} of {database} must state the instant the \
                         analysis was generated ({expected}), got {held:?} in {row:?}"
                    );
                    continue;
                }
                let expected = expected_cell(column, recommendation);
                assert_eq!(
                    held, expected,
                    "case {case:?}: column {column} of {database} must hold what the reader \
                     reports ({expected:?}), got {row:?}"
                );
            }
        }
    }
}

/// The file is not a sheet of empty cells: the members the database reader reports BEYOND the
/// findings every analysed resource carries — the engine, the DB instance class, the storage
/// configuration, the observed utilization, the options the analysis would move the database to
/// and what adopting them saves — are each filled for at least one exported row, under the
/// column the field set names for them.
#[tokio::test]
async fn the_exported_rows_carry_the_database_members_the_reader_reports() {
    let router = analysed_emulator().await;
    let reported = reader_recommendations(&router, "the filled columns").await;
    let body = export_ok(
        &router,
        "the filled columns",
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    let job_id = body["jobId"].as_str().unwrap_or_default().to_string();
    let (bucket, key, _) = export_destination("the filled columns", &body);
    polled_to_complete(&router, "the filled columns", &job_id).await;

    let exported = downloaded(&router, "the filled columns", &bucket, &key).await;
    let rows = csv_rows(&exported);
    let header = rows.first().cloned().unwrap_or_default();
    let data = rows.get(1..).unwrap_or_default();
    assert_eq!(
        data.len(),
        reported.len(),
        "expected one row per reported recommendation ({}), got {exported:?}",
        reported.len()
    );

    // One column per member the database surface reports beyond the shared findings columns.
    let must_be_filled = [
        "Engine",
        "EngineVersion",
        "DBClusterIdentifier",
        "PromotionTier",
        "CurrentDBInstanceClass",
        "CurrentStorageConfigurationStorageType",
        "CurrentStorageConfigurationAllocatedStorage",
        "CurrentStorageConfigurationIOPS",
        "UtilizationMetricsCpuMaximum",
        "UtilizationMetricsVolumeBytesUsedAverage",
        "InstanceRecommendationOptionsDBInstanceClass",
        "InstanceRecommendationOptionsRank",
        PROJECTED_CPU_COLUMN,
        "InstanceRecommendationOptionsEstimatedMonthlySavingsValue",
        "StorageRecommendationOptionsStorageType",
        "StorageRecommendationOptionsAllocatedStorage",
        "StorageRecommendationOptionsRank",
        "Tags",
    ];
    for column in must_be_filled {
        let filled = data
            .iter()
            .filter(|row| !cell(&header, row, column).is_empty())
            .count();
        assert!(
            filled > 0,
            "column {column} must carry what the reader reports for at least one of the \
             analysed databases, got {exported:?}"
        );
    }

    // And a column no member of a recommendation fills is empty in every row rather than
    // carrying an invented value.
    for column in NO_MEMBER_COLUMNS {
        for row in data {
            assert!(
                cell(&header, row, column).is_empty(),
                "column {column} names no member of a recommendation, so it is empty, got {row:?}"
            );
        }
    }

    // The metadata file beside it states the resource type the export was of.
    let (_, _, metadata_key) = export_destination("the filled columns", &body);
    let metadata = downloaded(&router, "the filled columns", &bucket, &metadata_key).await;
    let metadata: Value = serde_json::from_slice(metadata.as_bytes())
        .unwrap_or_else(|error| panic!("the metadata file is JSON: {error}"));
    assert_eq!(
        metadata["resourceType"].as_str(),
        Some(RESOURCE_TYPE_RDS_DB_INSTANCE),
        "the metadata names the exported resource type, got {metadata}"
    );
    assert_eq!(
        metadata["exportFile"]["rowCount"].as_u64(),
        Some(data.len() as u64),
        "the metadata counts the rows the file holds ({}), got {metadata}",
        data.len()
    );
}
