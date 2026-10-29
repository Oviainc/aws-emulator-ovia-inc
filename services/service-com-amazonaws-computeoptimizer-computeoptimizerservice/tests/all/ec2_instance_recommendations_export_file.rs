// SPDX-License-Identifier: CC0-1.0

//! What the `ExportEC2InstanceRecommendations` export FILE holds.
//!
//! The rule under test: the export of an instance analysis holds the recommendation OPTIONS
//! the request's `recommendationPreferences` admit — one row per admitted option, since each
//! `RecommendationOptions*` field names ONE option — and every cell is the value
//! `GetEC2InstanceRecommendations` reports for that member of the same analysis, so a file and
//! a read under one preference cannot disagree about which options the recommendation consists
//! of. An instance the preference leaves no option of still has its own row, with the option
//! columns empty.
//!
//! The reader is the ORACLE in every assertion below, so nothing here restates an instance
//! type, a figure or an architecture this analysis happens to hold. The table spans the
//! preference selections the rule generalises over: the member absent, an empty list, each
//! `CpuVendorArchitecture` the model declares on its own, and every one of them at once.
//!
//! Every call goes through the same `MultiServiceRouter` dispatch a client uses: the Compute
//! Optimizer calls as `POST /` with `X-Amz-Target`, and the exported object is downloaded over
//! Amazon S3's own `GetObject`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::CPU_VENDOR_ARCHITECTURE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    ANALYSIS_DELAY, EXPORT_JOB_STEP, JOB_STATUS_COMPLETE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    assert_figure, call, cell, create_bucket, csv_rows, emulator, export_destination, get_object,
    metrics, opt_in, strings,
};

/// The operation under test, and the reader that is the oracle for it.
const EXPORT: &str = "ExportEC2InstanceRecommendations";
/// See [`EXPORT`].
const READER: &str = "GetEC2InstanceRecommendations";

/// The destination bucket these fixtures export to. Compute Optimizer does not create it, so
/// each fixture creates it through Amazon S3 first.
const BUCKET: &str = "instance-export-file";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// How a cell holding several values separates them, so one exportable field is still one
/// column.
const LIST_SEPARATOR: &str = "|";

/// The column an instance's own ARN lands in (`ExportableInstanceField$InstanceArn`).
const ARN_COLUMN: &str = "InstanceArn";
/// The column naming the instance type ONE exported recommendation option would move the
/// instance to, which is what shows which options the preference admitted.
const OPTION_COLUMN: &str = "RecommendationOptionsInstanceType";
/// The column stating the architectures the exported options consist of.
const ARCHITECTURES_COLUMN: &str = "EffectiveRecommendationPreferencesCpuVendorArchitectures";

/// The `ExportableInstanceField` each utilization metric this analysis observes lands under:
/// the field set spells a `MetricName` in UpperCamelCase
/// (`smithy_query query="shape" shape="MetricName,ExportableInstanceField"`), so the series an
/// instance type sizes and one it does not are both covered — the network series is one the
/// instance type does not size.
const METRIC_COLUMNS: &[(&str, &str)] = &[
    ("Cpu", "UtilizationMetricsCpuMaximum"),
    ("Memory", "UtilizationMetricsMemoryMaximum"),
    (
        "NETWORK_IN_BYTES_PER_SECOND",
        "UtilizationMetricsNetworkInBytesPerSecondMaximum",
    ),
];

/// The `ExportableInstanceField` each projected utilization metric of ONE option lands under:
/// "the `Cpu` and `Memory` metrics are the only projected utilization metrics returned".
const PROJECTED_COLUMNS: &[(&str, &str)] = &[
    (
        "Cpu",
        "RecommendationOptionsProjectedUtilizationMetricsCpuMaximum",
    ),
    (
        "Memory",
        "RecommendationOptionsProjectedUtilizationMetricsMemoryMaximum",
    ),
];

/// Every column these fixtures ask for: the instance's own, the ones naming an option's values
/// and the metric columns above.
fn requested_fields() -> Vec<&'static str> {
    let mut fields = vec![
        ARN_COLUMN,
        "AccountId",
        "CurrentInstanceType",
        "Finding",
        "InstanceState",
        "Idle",
        ARCHITECTURES_COLUMN,
        OPTION_COLUMN,
        "RecommendationOptionsVcpus",
        "RecommendationOptionsMemory",
        "RecommendationOptionsOnDemandPrice",
        "RecommendationOptionsMigrationEffort",
        "RecommendationOptionsPlatformDifferences",
        "RecommendationOptionsSavingsOpportunityPercentage",
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
    ];
    fields.extend(METRIC_COLUMNS.iter().map(|(_, column)| *column));
    fields.extend(PROJECTED_COLUMNS.iter().map(|(_, column)| *column));
    fields
}

/// The preference selections the table spans: the member absent, an empty list, each
/// `CpuVendorArchitecture` the model declares on its own, and every one of them at once (in the
/// reverse of the order the model declares them, so a request's own order is covered).
fn selections() -> Vec<Option<Vec<&'static str>>> {
    let mut wanted: Vec<Option<Vec<&'static str>>> = vec![None, Some(Vec::new())];
    wanted.extend(
        CPU_VENDOR_ARCHITECTURE_VALUES
            .iter()
            .map(|architecture| Some(vec![*architecture])),
    );
    wanted.push(Some(
        CPU_VENDOR_ARCHITECTURE_VALUES.iter().rev().copied().collect(),
    ));
    wanted
}

/// One recommendation option as the READER reports it, in the terms the exported row states it.
#[derive(Debug, Clone, PartialEq)]
struct ReportedOption {
    instance_type: String,
    migration_effort: String,
    platform_differences: Vec<String>,
    savings_percentage: f64,
    savings_value: f64,
    projected: BTreeMap<String, f64>,
}

/// One instance's recommendation as the READER reports it.
#[derive(Debug, Clone)]
struct Reported {
    current_instance_type: String,
    finding: String,
    instance_state: String,
    idle: String,
    architectures: Vec<String>,
    observed: BTreeMap<String, f64>,
    options: Vec<ReportedOption>,
}

/// An emulator with the account opted in, the destination bucket created and the analysis the
/// enrollment starts already due — which is what the reader reports over, so it is what an
/// export file holds.
async fn analysed_emulator() -> MultiServiceRouter {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
    router
}

/// What the reader reports for every analysed instance under `preference`, keyed by ARN.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    preference: Option<&Value>,
) -> BTreeMap<String, Reported> {
    let mut request = json!({"maxResults": 100});
    if let Some(preference) = preference {
        request["recommendationPreferences"] = preference.clone();
    }
    let (status, body) = call(router, READER, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {READER} with {request}: got {status}: {body}"
    );
    let recommendations = body["instanceRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {READER} reports a recommendation list, got {body}"));
    assert!(
        !recommendations.is_empty(),
        "case {case:?}: the analysis holds instance recommendations, got {body}"
    );
    recommendations
        .iter()
        .map(|recommendation| {
            let arn = recommendation["instanceArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every recommendation carries instanceArn, got {recommendation}")
                })
                .to_string();
            let options = recommendation["recommendationOptions"]
                .as_array()
                .map(|options| {
                    options
                        .iter()
                        .map(|option| ReportedOption {
                            instance_type: option["instanceType"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            migration_effort: option["migrationEffort"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            platform_differences: strings(&option["platformDifferences"]),
                            savings_percentage: option["savingsOpportunity"]
                                ["savingsOpportunityPercentage"]
                                .as_f64()
                                .unwrap_or_default(),
                            savings_value: option["savingsOpportunity"]["estimatedMonthlySavings"]
                                ["value"]
                                .as_f64()
                                .unwrap_or_default(),
                            projected: metrics(&option["projectedUtilizationMetrics"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let held = Reported {
                current_instance_type: recommendation["currentInstanceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                finding: recommendation["finding"].as_str().unwrap_or_default().to_string(),
                instance_state: recommendation["instanceState"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                idle: recommendation["idle"].as_str().unwrap_or_default().to_string(),
                architectures: strings(
                    &recommendation["effectiveRecommendationPreferences"]
                        ["cpuVendorArchitectures"],
                ),
                observed: metrics(&recommendation["utilizationMetrics"]),
                options,
            };
            (arn, held)
        })
        .collect()
}

/// Export the account's instance recommendations under `preference`, poll the job to
/// `Complete` — the documented export-then-download workflow — and answer the file's rows.
async fn exported_rows(
    router: &MultiServiceRouter,
    case: &str,
    preference: Option<&Value>,
) -> Vec<Vec<String>> {
    let mut request = json!({
        "s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "options"},
        "fieldsToExport": requested_fields(),
    });
    if let Some(preference) = preference {
        request["recommendationPreferences"] = preference.clone();
    }
    let (status, body) = call(router, EXPORT, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    let job_id = body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an accepted export answers a job id, got {body}"))
        .to_string();
    let (bucket, key, _) = export_destination(case, &body);

    let mut status = String::new();
    for _ in 0..10 {
        let (code, reported) = call(
            router,
            "DescribeRecommendationExportJobs",
            json!({"jobIds": [job_id]}),
        )
        .await;
        assert_eq!(code, 200, "case {case:?}: reading job {job_id}: got {reported}");
        status = reported["recommendationExportJobs"][0]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status == JOB_STATUS_COMPLETE {
            break;
        }
        tokio::time::sleep(EXPORT_JOB_STEP).await;
    }
    assert_eq!(
        status, JOB_STATUS_COMPLETE,
        "case {case:?}: the job never completed, last status {status:?}"
    );

    let (code, object) = get_object(router, &bucket, &key).await;
    assert_eq!(
        code, 200,
        "case {case:?}: downloading {key}: got {code}: {}",
        String::from_utf8_lossy(&object)
    );
    let document = String::from_utf8(object)
        .unwrap_or_else(|error| panic!("case {case:?}: the export file is text: {error}"));
    csv_rows(&document)
}

/// The export file holds one row per recommendation option the request's preference admits, and
/// every cell of that row is the value the reader of the same analysis reports — the option's
/// instance type, its migration effort, its platform differences, what adopting it saves and
/// the utilization projected onto it — so narrowing the preference narrows the exported options.
#[tokio::test]
async fn an_export_holds_one_row_per_recommendation_option_the_preference_admits() {
    // What the reader reports with no preference at all: the baseline a narrowing preference is
    // narrower than, so the table is shown to prove something.
    let baseline_router = analysed_emulator().await;
    let baseline = reported(&baseline_router, "the baseline", None).await;
    assert!(
        baseline.values().any(|held| held.options.len() > 1),
        "the analysis must hold an instance with several options, or the table proves nothing, \
         got {baseline:?}"
    );

    let mut saw_narrowed = false;
    let mut saw_optionless = false;
    for architectures in selections() {
        let case = format!(
            "{EXPORT} asking for {}",
            architectures
                .as_ref()
                .map_or_else(|| "no preference".to_string(), |values| values.join(","))
        );
        // A fresh emulator per row: one instance export job is in progress per Region at a
        // time, so each row gets its own store rather than waiting on the previous row's job.
        let router = analysed_emulator().await;
        let preference = architectures
            .as_ref()
            .map(|values| json!({"cpuVendorArchitectures": values}));
        let reported = reported(&router, &case, preference.as_ref()).await;

        let rows = exported_rows(&router, &case, preference.as_ref()).await;
        let (header, data) = rows
            .split_first()
            .unwrap_or_else(|| panic!("case {case:?}: the export file carries a header row"));
        assert!(
            !data.is_empty(),
            "case {case:?}: the export holds a row per exported recommendation, got {rows:?}"
        );

        // The file names exactly the instances the reader reports: a preference selects which
        // OPTIONS are exported, never which instances are analysed.
        let exported_arns: BTreeSet<&str> =
            data.iter().map(|row| cell(header, row, ARN_COLUMN)).collect();
        let reported_arns: BTreeSet<&str> = reported.keys().map(String::as_str).collect();
        assert_eq!(
            exported_arns, reported_arns,
            "case {case:?}: the export names the instances the reader reports, got {rows:?}"
        );

        for (arn, held) in &reported {
            let instance_rows: Vec<&Vec<String>> = data
                .iter()
                .filter(|row| cell(header, row, ARN_COLUMN) == arn)
                .collect();
            // One row per admitted option, and the instance's own row alone where the
            // preference admits none.
            let expected_rows = held.options.len().max(1);
            assert_eq!(
                instance_rows.len(),
                expected_rows,
                "case {case:?}: expected {expected_rows} exported row(s) for {arn}, one per \
                 option the reader reports ({:?}), got {rows:?}",
                held.options
            );

            // The instance's own columns are the same on every one of its rows, whichever
            // option the row is of.
            let architectures_cell = held.architectures.join(LIST_SEPARATOR);
            for row in &instance_rows {
                assert_eq!(
                    cell(header, row, "CurrentInstanceType"),
                    held.current_instance_type,
                    "case {case:?}: the row of {arn} states the instance type the reader \
                     reports, got {row:?}"
                );
                assert_eq!(
                    cell(header, row, "Finding"),
                    held.finding,
                    "case {case:?}: the row of {arn} states the finding the reader reports, \
                     got {row:?}"
                );
                assert_eq!(
                    cell(header, row, "InstanceState"),
                    held.instance_state,
                    "case {case:?}: the row of {arn} states the instance state the reader \
                     reports, got {row:?}"
                );
                assert_eq!(
                    cell(header, row, "Idle"),
                    held.idle,
                    "case {case:?}: the row of {arn} states whether the reader reports it \
                     idle, got {row:?}"
                );
                assert_eq!(
                    cell(header, row, ARCHITECTURES_COLUMN),
                    architectures_cell,
                    "case {case:?}: the row of {arn} must state the architectures the reader \
                     reports ({architectures_cell:?}), got {row:?}"
                );
                for (metric, column) in METRIC_COLUMNS {
                    match held.observed.get(*metric) {
                        Some(value) => assert_figure(&case, header, row, column, *value),
                        None => assert!(
                            cell(header, row, column).is_empty(),
                            "case {case:?}: the reader reports no {metric} for {arn}, so \
                             {column} is empty, got {row:?}"
                        ),
                    }
                }
            }

            if held.options.is_empty() {
                saw_optionless = true;
                let row = instance_rows
                    .first()
                    .unwrap_or_else(|| panic!("case {case:?}: {arn} has its own row"));
                assert!(
                    cell(header, row, OPTION_COLUMN).is_empty(),
                    "case {case:?}: the preference admits no option of {arn}, so its row names \
                     none, got {row:?}"
                );
                continue;
            }

            // Exactly the options the reader reports, each row holding that option's values.
            let exported_options: BTreeSet<&str> = instance_rows
                .iter()
                .map(|row| cell(header, row, OPTION_COLUMN))
                .collect();
            let reported_options: BTreeSet<&str> = held
                .options
                .iter()
                .map(|option| option.instance_type.as_str())
                .collect();
            assert_eq!(
                exported_options, reported_options,
                "case {case:?}: the exported options of {arn} are the ones the reader reports, \
                 got {rows:?}"
            );
            saw_narrowed |= baseline
                .get(arn)
                .is_some_and(|before| before.options.len() > held.options.len());

            for option in &held.options {
                let row = instance_rows
                    .iter()
                    .find(|row| cell(header, row, OPTION_COLUMN) == option.instance_type)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the export holds a row for the {} option of {arn}, \
                             got {rows:?}",
                            option.instance_type
                        )
                    });
                assert_eq!(
                    cell(header, row, "RecommendationOptionsMigrationEffort"),
                    option.migration_effort,
                    "case {case:?}: the option row of {arn} states the migration effort the \
                     reader reports, got {row:?}"
                );
                assert_eq!(
                    cell(header, row, "RecommendationOptionsPlatformDifferences"),
                    option.platform_differences.join(LIST_SEPARATOR),
                    "case {case:?}: the option row of {arn} states the platform differences \
                     the reader reports, got {row:?}"
                );
                assert_figure(
                    &case,
                    header,
                    row,
                    "RecommendationOptionsSavingsOpportunityPercentage",
                    option.savings_percentage,
                );
                assert_figure(
                    &case,
                    header,
                    row,
                    "RecommendationOptionsEstimatedMonthlySavingsValue",
                    option.savings_value,
                );
                for (metric, column) in PROJECTED_COLUMNS {
                    match option.projected.get(*metric) {
                        Some(value) => assert_figure(&case, header, row, column, *value),
                        None => assert!(
                            cell(header, row, column).is_empty(),
                            "case {case:?}: the reader projects no {metric} onto the {} option \
                             of {arn}, so {column} is empty, got {row:?}",
                            option.instance_type
                        ),
                    }
                }
                // The option's own capacity and price: members of the instance TYPE the option
                // names, which the reader reports no member for, so the assertion is that the
                // row carries them rather than what they are.
                for column in [
                    "RecommendationOptionsVcpus",
                    "RecommendationOptionsMemory",
                    "RecommendationOptionsOnDemandPrice",
                ] {
                    let held = cell(header, row, column);
                    assert!(
                        held.parse::<f64>().is_ok_and(|value| value > 0.0),
                        "case {case:?}: the option row of {arn} states {column}, got {held:?} \
                         in {row:?}"
                    );
                }
            }
        }
    }

    assert!(
        saw_narrowed && saw_optionless,
        "the table must span a row whose preference narrowed the exported options and one whose \
         preference admitted none of an instance, or it proves nothing (narrowed: {saw_narrowed}, \
         none: {saw_optionless})"
    );
}
