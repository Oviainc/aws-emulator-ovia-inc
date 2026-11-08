// SPDX-License-Identifier: CC0-1.0

//! The `recommendationPreferences` an `Export*Recommendations` request carries.
//!
//! The rule under test: an export operation whose model declares
//! `recommendationPreferences` is of the recommendations those preferences settle, by the same
//! resolution the reader of the same recommendations applies — the CPU vendor architectures it
//! names select which recommendation options the file holds, and the exported
//! `EffectiveRecommendationPreferences*` cell states the architectures the exported options
//! consist of — and the metadata file states the preferences the export was asked for.
//!
//! The reader is the ORACLE in every assertion below, so nothing here restates an architecture
//! this analysis happens to hold. The table spans the three surfaces whose model declares the
//! member (`ExportAutoScalingGroupRecommendations`, `ExportEC2InstanceRecommendations`,
//! `ExportRDSDatabaseRecommendations`) and, per surface, the preference selections the rule
//! generalises over: the member absent, an empty list, each `CpuVendorArchitecture` the model
//! declares on its own, and every one of them at once.
//!
//! Every call goes through the same `MultiServiceRouter` dispatch a client uses: the Compute
//! Optimizer calls as `POST /` with `X-Amz-Target`, and the exported objects are downloaded
//! over Amazon S3's own `GetObject`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::CPU_VENDOR_ARCHITECTURE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, RdsDatabaseDetail, ANALYSIS_DELAY, EXPORT_JOB_STEP,
    JOB_STATUS_COMPLETE, SOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cell, create_bucket, csv_rows, emulator_with_engine, export_destination, get_object,
    opt_in, strings, ACCOUNT,
};

/// The destination bucket these fixtures export to. Compute Optimizer does not create it, so
/// each fixture creates it through Amazon S3 first.
const BUCKET: &str = "export-recommendation-preferences";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// The export column stating the architectures the exported recommendation was generated
/// under, declared by every exportable field set of the three surfaces below.
const ARCHITECTURES_COLUMN: &str = "EffectiveRecommendationPreferencesCpuVendorArchitectures";

/// How a cell holding several values separates them, so one exportable field is still one
/// column.
const LIST_SEPARATOR: &str = "|";

/// The Auto Scaling group surface's option columns: its export file holds ONE row per exported
/// recommendation option, so the configuration an option would move the group to is what shows
/// which options the preference admitted. A `SingleInstanceType` option names one instance type
/// and a `MixedInstanceTypes` one names the types it mixes, so both columns identify an option
/// ([`option_identity`]).
const OPTION_COLUMN: &str = "RecommendationOptionsConfigurationInstanceType";
/// See [`OPTION_COLUMN`].
const MIXED_OPTION_COLUMN: &str = "RecommendationOptionsConfigurationMixedInstanceTypes";

/// The configuration one recommendation option names, as one string: the single instance type
/// it would move the group to, or the types a mixed-instance-type option mixes.
fn option_identity(instance_type: &str, mixed_instance_types: &str) -> String {
    if instance_type.is_empty() {
        mixed_instance_types.to_string()
    } else {
        instance_type.to_string()
    }
}

/// One recommendation surface whose export declares `recommendationPreferences` — the export
/// operation, the reader that is the oracle for it, the reader's recommendation list member,
/// the ARN member of one recommendation and the export column that ARN lands in.
struct Surface {
    export: &'static str,
    reader: &'static str,
    list: &'static str,
    arn_member: &'static str,
    arn_column: &'static str,
    /// Whether this surface's export file holds one row per recommendation option (the Auto
    /// Scaling group field set names an option's own configuration), rather than one row per
    /// analysed resource.
    rows_per_option: bool,
}

/// The three surfaces the rule covers: the only `Export*Recommendations` operations whose
/// input declares `recommendationPreferences` (`smithy_query query="operation"`).
const SURFACES: &[Surface] = &[
    Surface {
        export: "ExportAutoScalingGroupRecommendations",
        reader: "GetAutoScalingGroupRecommendations",
        list: "autoScalingGroupRecommendations",
        arn_member: "autoScalingGroupArn",
        arn_column: "AutoScalingGroupArn",
        rows_per_option: true,
    },
    Surface {
        export: "ExportEC2InstanceRecommendations",
        reader: "GetEC2InstanceRecommendations",
        list: "instanceRecommendations",
        arn_member: "instanceArn",
        arn_column: "InstanceArn",
        rows_per_option: false,
    },
    Surface {
        export: "ExportRDSDatabaseRecommendations",
        reader: "GetRDSDatabaseRecommendations",
        list: "rdsDBRecommendations",
        arn_member: "resourceArn",
        arn_column: "ResourceArn",
        rows_per_option: false,
    },
];

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

/// An emulator with the account opted in, the destination bucket created and the analysis the
/// enrollment starts already due — which is what a recommendation reader reports over, so it is
/// what an export file holds.
async fn analysed_emulator() -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    record_analysed_database(&engine);
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
    router
}

/// Record one analysed Amazon Aurora or RDS database into the findings catalog, the way
/// Compute Optimizer's own analysis of an observed workload would: the database surface has a
/// recommendation to read and export only once such an analysis exists.
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

/// The request member the selection of `architectures` carries, or no member at all.
fn preference_member(architectures: Option<&Vec<&'static str>>) -> Option<Value> {
    architectures.map(|architectures| json!({"cpuVendorArchitectures": architectures}))
}

/// What the READER reports for each recommendation of `surface` under `preference`: the
/// architectures its effective recommendation preferences state, and the configurations its
/// recommendation options name — keyed by the resource's ARN.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    surface: &Surface,
    preference: Option<&Value>,
) -> BTreeMap<String, (Vec<String>, Vec<String>)> {
    let mut request = json!({"maxResults": 100});
    if let Some(preference) = preference {
        request["recommendationPreferences"] = preference.clone();
    }
    let (status, body) = call(router, surface.reader, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {} with {request}: got {status}: {body}",
        surface.reader
    );
    let reported = body[surface.list].as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {} reports a recommendation list, got {body}",
            surface.reader
        )
    });
    assert!(
        !reported.is_empty(),
        "case {case:?}: the analysis holds a recommendation for this surface, got {body}"
    );
    reported
        .iter()
        .map(|recommendation| {
            let arn = recommendation[surface.arn_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: every recommendation carries {}, got {recommendation}",
                        surface.arn_member
                    )
                })
                .to_string();
            let architectures = strings(
                &recommendation["effectiveRecommendationPreferences"]["cpuVendorArchitectures"],
            );
            let options: Vec<String> = recommendation["recommendationOptions"]
                .as_array()
                .map(|options| {
                    options
                        .iter()
                        .map(|option| {
                            let configuration = &option["configuration"];
                            option_identity(
                                configuration["instanceType"].as_str().unwrap_or_default(),
                                &strings(&configuration["mixedInstanceTypes"])
                                    .join(LIST_SEPARATOR),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            (arn, (architectures, options))
        })
        .collect()
}

/// Export `surface`'s recommendations under `preference`, answering the job id and the
/// destination the call named.
async fn export_ok(
    router: &MultiServiceRouter,
    case: &str,
    surface: &Surface,
    preference: Option<&Value>,
    fields: &[&str],
) -> (String, String, String, String) {
    let mut request = json!({
        "s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "preferences"},
        "fieldsToExport": fields,
    });
    if let Some(preference) = preference {
        request["recommendationPreferences"] = preference.clone();
    }
    let (status, body) = call(router, surface.export, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    let job_id = body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an accepted export answers a job id, got {body}"))
        .to_string();
    let (bucket, key, metadata_key) = export_destination(case, &body);
    (job_id, bucket, key, metadata_key)
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
    String::from_utf8(body).unwrap_or_else(|error| panic!("case {case:?}: the object is text: {error}"))
}

/// Every exported row states the CPU vendor architectures the reader of the same analysis
/// reports under the same `recommendationPreferences`, and a surface whose file holds one row
/// per recommendation option holds exactly the options that preference admits.
#[tokio::test]
async fn an_export_holds_the_recommendations_its_preferences_select() {
    let mut saw_stated = false;
    let mut saw_narrowed = false;

    for surface in SURFACES {
        // What the reader reports with no preference at all: the baseline a narrowing
        // preference is narrower than, so the table is shown to prove something.
        let baseline_router = analysed_emulator().await;
        let baseline = reported(&baseline_router, "the baseline", surface, None).await;

        for architectures in selections() {
            let case = format!(
                "{} asking for {}",
                surface.export,
                architectures
                    .as_ref()
                    .map_or_else(|| "no preference".to_string(), |values| values.join(","))
            );
            // A fresh emulator per row: one export job of a resource type is in progress per
            // Region at a time, so each row gets its own store rather than waiting on the
            // previous row's job.
            let router = analysed_emulator().await;
            let preference = preference_member(architectures.as_ref());
            let reported = reported(&router, &case, surface, preference.as_ref()).await;

            let mut fields = vec![surface.arn_column, ARCHITECTURES_COLUMN];
            if surface.rows_per_option {
                fields.push(OPTION_COLUMN);
                fields.push(MIXED_OPTION_COLUMN);
            }
            let (job_id, bucket, key, _) =
                export_ok(&router, &case, surface, preference.as_ref(), &fields).await;
            polled_to_complete(&router, &case, &job_id).await;
            let exported = downloaded(&router, &case, &bucket, &key).await;

            let rows = csv_rows(&exported);
            let (header, data) = rows
                .split_first()
                .unwrap_or_else(|| panic!("case {case:?}: the export file carries a header row"));
            assert!(
                !data.is_empty(),
                "case {case:?}: the export holds a row per exported recommendation, got {rows:?}"
            );

            // The file names exactly the resources the reader reports — a preference selects
            // which OPTIONS are exported, never which resources are analysed.
            let exported_arns: BTreeSet<&str> = data
                .iter()
                .map(|row| cell(header, row, surface.arn_column))
                .collect();
            let reported_arns: BTreeSet<&str> =
                reported.keys().map(String::as_str).collect();
            assert_eq!(
                exported_arns, reported_arns,
                "case {case:?}: the export names the resources the reader reports, got {rows:?}"
            );

            for (arn, (expected_architectures, options)) in &reported {
                let resource_rows: Vec<&Vec<String>> = data
                    .iter()
                    .filter(|row| cell(header, row, surface.arn_column) == arn)
                    .collect();
                let expected_cell = expected_architectures.join(LIST_SEPARATOR);
                for row in &resource_rows {
                    let held = cell(header, row, ARCHITECTURES_COLUMN);
                    assert_eq!(
                        held, expected_cell,
                        "case {case:?}: the exported row of {arn} must state the architectures \
                         the reader reports ({expected_cell:?}), got {row:?}"
                    );
                }
                saw_stated |= !expected_cell.is_empty();
                saw_narrowed |= baseline.get(arn).is_some_and(|(held, _)| {
                    held.len() > expected_architectures.len() && !expected_architectures.is_empty()
                });

                if !surface.rows_per_option {
                    continue;
                }
                // One row per option the preference admits, and the group's own row alone
                // where it admits none — exactly the options the reader reports.
                let expected_rows = options.len().max(1);
                assert_eq!(
                    resource_rows.len(),
                    expected_rows,
                    "case {case:?}: expected {expected_rows} exported row(s) for {arn}, one per \
                     option the reader reports ({options:?}), got {rows:?}"
                );
                let exported_options: BTreeSet<String> = resource_rows
                    .iter()
                    .map(|row| {
                        option_identity(
                            cell(header, row, OPTION_COLUMN),
                            cell(header, row, MIXED_OPTION_COLUMN),
                        )
                    })
                    .filter(|option| !option.is_empty())
                    .collect();
                let reported_options: BTreeSet<String> = options.iter().cloned().collect();
                assert_eq!(
                    exported_options, reported_options,
                    "case {case:?}: the exported options of {arn} are the ones the reader \
                     reports, got {rows:?}"
                );
            }
        }
    }

    assert!(
        saw_stated && saw_narrowed,
        "the table must span a row whose exported cell states an architecture and one whose \
         preference narrowed what the export holds, or it proves nothing (stated: {saw_stated}, \
         narrowed: {saw_narrowed})"
    );
}

/// The metadata file beside the export file states the recommendation preferences the export
/// was asked for, and states none for a request that named none.
#[tokio::test]
async fn the_metadata_file_states_the_preferences_the_export_was_asked_for() {
    for surface in SURFACES {
        for architectures in selections() {
            let case = format!(
                "{} metadata for {}",
                surface.export,
                architectures
                    .as_ref()
                    .map_or_else(|| "no preference".to_string(), |values| values.join(","))
            );
            let router = analysed_emulator().await;
            let preference = preference_member(architectures.as_ref());
            let (job_id, bucket, _, metadata_key) = export_ok(
                &router,
                &case,
                surface,
                preference.as_ref(),
                &[surface.arn_column],
            )
            .await;
            polled_to_complete(&router, &case, &job_id).await;

            let metadata = downloaded(&router, &case, &bucket, &metadata_key).await;
            let metadata: Value = serde_json::from_str(&metadata)
                .unwrap_or_else(|error| panic!("case {case:?}: the metadata file is JSON: {error}"));
            // A request that named no architecture asked for no preference, so the metadata
            // states none rather than an empty one.
            let asked_for: Vec<String> = architectures
                .unwrap_or_default()
                .iter()
                .map(|architecture| (*architecture).to_string())
                .collect();
            if asked_for.is_empty() {
                assert!(
                    metadata["recommendationPreferences"].is_null(),
                    "case {case:?}: an export asked for no preference states none, got {metadata}"
                );
                continue;
            }
            assert_eq!(
                strings(&metadata["recommendationPreferences"]["cpuVendorArchitectures"]),
                asked_for,
                "case {case:?}: the metadata states the architectures the export was asked for \
                 ({asked_for:?}), got {metadata}"
            );
        }
    }
}
