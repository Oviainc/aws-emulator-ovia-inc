// SPDX-License-Identifier: CC0-1.0

//! What the `ExportAutoScalingGroupRecommendations` export FILE holds.
//!
//! The rule under test: an export of a recommendation surface whose model declares
//! `recommendationPreferences` holds the recommendation OPTIONS those preferences admit — one
//! row per admitted option, since each `RecommendationOptions*` field names ONE option — and
//! every cell of that row is the value the reader of the same recommendations reports for that
//! member under the same preferences, so a file and a read cannot disagree about which options
//! a recommendation consists of or about what they are. An analysed resource the preferences
//! leave no option of still has its own row, with the option columns empty.
//!
//! `GetAutoScalingGroupRecommendations` is the ORACLE in every assertion below, so nothing
//! here restates an instance type, a figure or an architecture this analysis happens to hold.
//! The table spans the preference selections the rule generalises over: the member absent, an
//! empty list, each `CpuVendorArchitecture` the model declares on its own, and every one of
//! them at once.
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
const EXPORT: &str = "ExportAutoScalingGroupRecommendations";
/// See [`EXPORT`].
const READER: &str = "GetAutoScalingGroupRecommendations";

/// The destination bucket these fixtures export to. Compute Optimizer does not create it, so
/// each fixture creates it through Amazon S3 first.
const BUCKET: &str = "asg-export-file";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// How a cell holding several values separates them, so one exportable field is still one
/// column.
const LIST_SEPARATOR: &str = "|";

/// The column a group's own ARN lands in (`ExportableAutoScalingGroupField$AutoScalingGroupArn`).
const ARN_COLUMN: &str = "AutoScalingGroupArn";
/// The column stating the architectures the exported options consist of.
const ARCHITECTURES_COLUMN: &str = "EffectiveRecommendationPreferencesCpuVendorArchitectures";

/// The prefix the columns of the group's OWN configuration carry, and the one the columns of an
/// exported recommendation option carry
/// (`smithy_query query="shape" shape="ExportableAutoScalingGroupField"`).
const CURRENT_PREFIX: &str = "CurrentConfiguration";
/// See [`CURRENT_PREFIX`].
const OPTION_PREFIX: &str = "RecommendationOptionsConfiguration";

/// The members of one `AutoScalingGroupConfiguration` the reader reports, with the suffix the
/// field set spells each of them with under a prefix: a single-instance-type configuration
/// names one instance type and a mixed one names the types it mixes, so both appear.
const CONFIGURATION_MEMBERS: &[(&str, &str)] = &[
    ("instanceType", "InstanceType"),
    ("mixedInstanceTypes", "MixedInstanceTypes"),
    ("type", "Type"),
    ("allocationStrategy", "AllocationStrategy"),
    ("desiredCapacity", "DesiredCapacity"),
    ("minSize", "MinSize"),
    ("maxSize", "MaxSize"),
];

/// The `ExportableAutoScalingGroupField` each utilization metric this analysis observes lands
/// under: the field set spells a `MetricName` in UpperCamelCase, and an Auto Scaling group
/// analysis observes the CPU and memory series.
const METRIC_COLUMNS: &[(&str, &str)] = &[
    ("Cpu", "UtilizationMetricsCpuMaximum"),
    ("Memory", "UtilizationMetricsMemoryMaximum"),
];

/// The `ExportableAutoScalingGroupField` each projected utilization metric of ONE option lands
/// under.
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

/// The columns naming the capacity and the price of ONE instance of a configuration: members of
/// the instance TYPES the configuration names, which an Auto Scaling group recommendation
/// reports no member for. So the reader is no oracle for them, and what is asserted instead is
/// that every exported row states them, and states the SAME ones wherever the same configuration
/// is exported — under any preference, for any group.
const CAPACITY_COLUMNS: &[&str] = &[
    "RecommendationOptionsVcpus",
    "RecommendationOptionsMemory",
    "RecommendationOptionsOnDemandPrice",
];

/// Every column these fixtures ask for: the group's own, the ones naming an option's values and
/// the metric columns above.
fn requested_fields() -> Vec<String> {
    let mut fields: Vec<String> = vec![
        ARN_COLUMN.to_string(),
        "AccountId".to_string(),
        "AutoScalingGroupName".to_string(),
        "Finding".to_string(),
        "CurrentPerformanceRisk".to_string(),
        "LookbackPeriodInDays".to_string(),
        "InferredWorkloadTypes".to_string(),
        ARCHITECTURES_COLUMN.to_string(),
        "RecommendationOptionsPerformanceRisk".to_string(),
        "RecommendationOptionsMigrationEffort".to_string(),
        "RecommendationOptionsSavingsOpportunityPercentage".to_string(),
        "RecommendationOptionsEstimatedMonthlySavingsValue".to_string(),
        "RecommendationOptionsEstimatedMonthlySavingsCurrency".to_string(),
    ];
    for prefix in [CURRENT_PREFIX, OPTION_PREFIX] {
        fields.extend(
            CONFIGURATION_MEMBERS
                .iter()
                .map(|(_, suffix)| format!("{prefix}{suffix}")),
        );
    }
    fields.extend(CAPACITY_COLUMNS.iter().map(|column| (*column).to_string()));
    fields.extend(
        METRIC_COLUMNS
            .iter()
            .chain(PROJECTED_COLUMNS)
            .map(|(_, column)| (*column).to_string()),
    );
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
#[derive(Debug, Clone)]
struct ReportedOption {
    /// The configuration this option would move the group to, as one string — the single
    /// instance type, or the types a mixed-instance-type option mixes.
    identity: String,
    /// That configuration as the reader reports it, member by member.
    configuration: Value,
    performance_risk: f64,
    migration_effort: String,
    savings_percentage: f64,
    savings_value: f64,
    savings_currency: String,
    projected: BTreeMap<String, f64>,
}

/// One Auto Scaling group's recommendation as the READER reports it.
#[derive(Debug, Clone)]
struct Reported {
    account_id: String,
    name: String,
    finding: String,
    current_performance_risk: String,
    look_back_period_in_days: f64,
    architectures: Vec<String>,
    inferred_workload_types: Vec<String>,
    current_configuration: Value,
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

/// The configuration one recommendation option names, as one string.
fn option_identity(configuration: &Value) -> String {
    let instance_type = configuration["instanceType"].as_str().unwrap_or_default();
    if instance_type.is_empty() {
        strings(&configuration["mixedInstanceTypes"]).join(LIST_SEPARATOR)
    } else {
        instance_type.to_string()
    }
}

/// What the reader reports for every analysed group under `preference`, keyed by ARN.
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
    let recommendations = body["autoScalingGroupRecommendations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: {READER} reports a recommendation list, got {body}")
        });
    assert!(
        !recommendations.is_empty(),
        "case {case:?}: the analysis holds Auto Scaling group recommendations, got {body}"
    );
    recommendations
        .iter()
        .map(|recommendation| {
            let arn = recommendation["autoScalingGroupArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: every recommendation carries autoScalingGroupArn, got \
                         {recommendation}"
                    )
                })
                .to_string();
            let options = recommendation["recommendationOptions"]
                .as_array()
                .map(|options| {
                    options
                        .iter()
                        .map(|option| ReportedOption {
                            identity: option_identity(&option["configuration"]),
                            configuration: option["configuration"].clone(),
                            performance_risk: option["performanceRisk"]
                                .as_f64()
                                .unwrap_or_default(),
                            migration_effort: option["migrationEffort"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            savings_percentage: option["savingsOpportunity"]
                                ["savingsOpportunityPercentage"]
                                .as_f64()
                                .unwrap_or_default(),
                            savings_value: option["savingsOpportunity"]["estimatedMonthlySavings"]
                                ["value"]
                                .as_f64()
                                .unwrap_or_default(),
                            savings_currency: option["savingsOpportunity"]
                                ["estimatedMonthlySavings"]["currency"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            projected: metrics(&option["projectedUtilizationMetrics"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let held = Reported {
                account_id: recommendation["accountId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                name: recommendation["autoScalingGroupName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                finding: recommendation["finding"].as_str().unwrap_or_default().to_string(),
                current_performance_risk: recommendation["currentPerformanceRisk"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                look_back_period_in_days: recommendation["lookBackPeriodInDays"]
                    .as_f64()
                    .unwrap_or_default(),
                architectures: strings(
                    &recommendation["effectiveRecommendationPreferences"]
                        ["cpuVendorArchitectures"],
                ),
                inferred_workload_types: strings(&recommendation["inferredWorkloadTypes"]),
                current_configuration: recommendation["currentConfiguration"].clone(),
                observed: metrics(&recommendation["utilizationMetrics"]),
                options,
            };
            (arn, held)
        })
        .collect()
}

/// Export the account's Auto Scaling group recommendations under `preference`, poll the job to
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

/// Assert `column` of `row` holds `expected`.
fn assert_cell(case: &str, header: &[String], row: &[String], column: &str, expected: &str) {
    let held = cell(header, row, column);
    assert_eq!(
        held, expected,
        "case {case:?}: {column} must hold what the reader reports ({expected:?}), got {held:?} \
         in {row:?}"
    );
}

/// Assert the configuration columns under `prefix` state the `AutoScalingGroupConfiguration`
/// the reader reports, member by member.
fn assert_configuration(
    case: &str,
    header: &[String],
    row: &[String],
    prefix: &str,
    configuration: &Value,
) {
    for (member, suffix) in CONFIGURATION_MEMBERS {
        let column = format!("{prefix}{suffix}");
        let reported = &configuration[*member];
        let expected = if let Some(text) = reported.as_str() {
            text.to_string()
        } else if let Some(number) = reported.as_i64() {
            number.to_string()
        } else {
            strings(reported).join(LIST_SEPARATOR)
        };
        assert_cell(case, header, row, &column, &expected);
    }
}

/// The export file holds one row per recommendation option the request's
/// `recommendationPreferences` admit, and every cell of that row is the value the reader of the
/// same analysis reports under the same preferences — the group's finding and observed
/// utilization, the configuration an option would move it to, the risk it runs, the effort of
/// moving to it, what adopting it saves and the utilization projected onto it — so narrowing
/// the preferences narrows the exported options.
#[tokio::test]
async fn an_export_holds_one_row_per_recommendation_option_the_preference_admits() {
    // What the reader reports with no preference at all: the baseline a narrowing preference is
    // narrower than, so the table is shown to prove something.
    let baseline_router = analysed_emulator().await;
    let baseline = reported(&baseline_router, "the baseline", None).await;
    assert!(
        baseline.values().any(|held| held.options.len() > 1),
        "the analysis must hold a group with several options, or the table proves nothing, got \
         {baseline:?}"
    );

    let mut saw_narrowed = false;
    let mut saw_optionless = false;
    // The capacity and price cells each exported configuration was first seen with, so a second
    // row naming that configuration is checked against them rather than only against zero.
    let mut capacity_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for architectures in selections() {
        let case = format!(
            "{EXPORT} asking for {}",
            architectures
                .as_ref()
                .map_or_else(|| "no preference".to_string(), |values| values.join(","))
        );
        // A fresh emulator per row: one Auto Scaling group export job is in progress per Region
        // at a time, so each row gets its own store rather than waiting on the previous row's
        // job.
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

        // The file names exactly the groups the reader reports: a preference selects which
        // OPTIONS are exported, never which groups are analysed.
        let exported_arns: BTreeSet<&str> =
            data.iter().map(|row| cell(header, row, ARN_COLUMN)).collect();
        let reported_arns: BTreeSet<&str> = reported.keys().map(String::as_str).collect();
        assert_eq!(
            exported_arns, reported_arns,
            "case {case:?}: the export names the groups the reader reports, got {rows:?}"
        );

        for (arn, held) in &reported {
            let group_rows: Vec<&Vec<String>> = data
                .iter()
                .filter(|row| cell(header, row, ARN_COLUMN) == arn)
                .collect();
            // One row per admitted option, and the group's own row alone where the preference
            // admits none.
            let expected_rows = held.options.len().max(1);
            assert_eq!(
                group_rows.len(),
                expected_rows,
                "case {case:?}: expected {expected_rows} exported row(s) for {arn}, one per \
                 option the reader reports ({:?}), got {rows:?}",
                held.options
            );

            // The group's own columns are the same on every one of its rows, whichever option
            // the row is of.
            for row in &group_rows {
                assert_cell(&case, header, row, "AccountId", &held.account_id);
                assert_cell(&case, header, row, "AutoScalingGroupName", &held.name);
                assert_cell(&case, header, row, "Finding", &held.finding);
                assert_cell(
                    &case,
                    header,
                    row,
                    "CurrentPerformanceRisk",
                    &held.current_performance_risk,
                );
                assert_cell(
                    &case,
                    header,
                    row,
                    "InferredWorkloadTypes",
                    &held.inferred_workload_types.join(LIST_SEPARATOR),
                );
                assert_cell(
                    &case,
                    header,
                    row,
                    ARCHITECTURES_COLUMN,
                    &held.architectures.join(LIST_SEPARATOR),
                );
                assert_figure(
                    &case,
                    header,
                    row,
                    "LookbackPeriodInDays",
                    held.look_back_period_in_days,
                );
                assert_configuration(
                    &case,
                    header,
                    row,
                    CURRENT_PREFIX,
                    &held.current_configuration,
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
                let row = group_rows
                    .first()
                    .unwrap_or_else(|| panic!("case {case:?}: {arn} has its own row"));
                for (_, suffix) in CONFIGURATION_MEMBERS {
                    let column = format!("{OPTION_PREFIX}{suffix}");
                    assert!(
                        cell(header, row, &column).is_empty(),
                        "case {case:?}: the preference admits no option of {arn}, so {column} is \
                         empty, got {row:?}"
                    );
                }
                continue;
            }

            // Exactly the options the reader reports, each row holding that option's values.
            let exported_options: BTreeSet<String> = group_rows
                .iter()
                .map(|row| {
                    option_identity(&json!({
                        "instanceType": cell(header, row, &format!("{OPTION_PREFIX}InstanceType")),
                        "mixedInstanceTypes": cell(
                            header,
                            row,
                            &format!("{OPTION_PREFIX}MixedInstanceTypes"),
                        )
                        .split(LIST_SEPARATOR)
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<&str>>(),
                    }))
                })
                .collect();
            let reported_options: BTreeSet<String> = held
                .options
                .iter()
                .map(|option| option.identity.clone())
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
                let row = group_rows
                    .iter()
                    .find(|row| {
                        let instance_type =
                            cell(header, row, &format!("{OPTION_PREFIX}InstanceType"));
                        let mixed = cell(header, row, &format!("{OPTION_PREFIX}MixedInstanceTypes"));
                        option.identity
                            == if instance_type.is_empty() {
                                mixed.to_string()
                            } else {
                                instance_type.to_string()
                            }
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the export holds a row for the {} option of {arn}, \
                             got {rows:?}",
                            option.identity
                        )
                    });
                assert_configuration(&case, header, row, OPTION_PREFIX, &option.configuration);
                assert_cell(
                    &case,
                    header,
                    row,
                    "RecommendationOptionsMigrationEffort",
                    &option.migration_effort,
                );
                assert_cell(
                    &case,
                    header,
                    row,
                    "RecommendationOptionsEstimatedMonthlySavingsCurrency",
                    &option.savings_currency,
                );
                assert_figure(
                    &case,
                    header,
                    row,
                    "RecommendationOptionsPerformanceRisk",
                    option.performance_risk,
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
                            option.identity
                        ),
                    }
                }
                for column in CAPACITY_COLUMNS {
                    let held = cell(header, row, column);
                    assert!(
                        held.parse::<f64>().is_ok_and(|value| value > 0.0),
                        "case {case:?}: the option row of {arn} states {column}, got {held:?} in \
                         {row:?}"
                    );
                }
                let capacity: Vec<String> = CAPACITY_COLUMNS
                    .iter()
                    .map(|column| cell(header, row, column).to_string())
                    .collect();
                match capacity_of.get(&option.identity).cloned() {
                    Some(first) => assert_eq!(
                        capacity, first,
                        "case {case:?}: the {} option of {arn} must state the capacity and price \
                         it was exported with before ({CAPACITY_COLUMNS:?} = {first:?}), got \
                         {capacity:?}",
                        option.identity
                    ),
                    None => {
                        capacity_of.insert(option.identity.clone(), capacity);
                    }
                }
            }
        }
    }

    assert!(
        saw_narrowed && saw_optionless,
        "the table must span a row whose preference narrowed the exported options and one whose \
         preference admitted none of a group, or it proves nothing (narrowed: {saw_narrowed}, \
         none: {saw_optionless})"
    );
}
