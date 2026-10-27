// SPDX-License-Identifier: CC0-1.0

//! The export columns an effective recommendation preference GATES.
//!
//! The rule under test: an export row reports a member exactly as the recommendation READER
//! of the same analysis reports it, so a value the account's effective recommendation
//! preference switches off is in neither — "an exported row and a `Get*Recommendations`
//! response cannot disagree". The member this suite drives is the inferred workload types,
//! whose preference "is active by default. To deactivate it, create a recommendation
//! preference" (`PutRecommendationPreferences$inferredWorkloadTypes`): while it is active the
//! exported cell holds the types the analysis inferred, and once it is `Inactive` the cell is
//! empty — as is the member the reader reports — and the `InferredWorkloadTypes` filter,
//! which selects on that very member, then selects nothing.
//!
//! The reader is the ORACLE in every assertion below, so no row restates a value this
//! analysis happens to hold. The table spans both surfaces whose exportable field set
//! declares the column (`ExportableInstanceField`, `ExportableAutoScalingGroupField`) and
//! every `InferredWorkloadTypesPreference` the model declares, plus the account writing no
//! preference at all — and the two scope levels each surface accepts the preference at.
//!
//! Every call goes through the same `MultiServiceRouter` dispatch a client uses: the
//! Compute Optimizer calls as `POST /` with `X-Amz-Target`, and the exported object is
//! downloaded over Amazon S3's own `GetObject`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::INFERRED_WORKLOAD_TYPES_PREFERENCE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    ANALYSIS_DELAY, EXPORT_JOB_STEP, INFERRED_WORKLOAD_TYPES_ACTIVE,
    RESOURCE_TYPE_AUTO_SCALING_GROUP, RESOURCE_TYPE_EC2_INSTANCE, SCOPE_ACCOUNT_ID,
    SCOPE_RESOURCE_ARN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cell, create_bucket, csv_rows, emulator, export_destination, get_object, opt_in,
    put_preference, ACCOUNT,
};

/// The destination bucket these fixtures export to. Compute Optimizer does not create it;
/// the request just names it.
const BUCKET: &str = "preference-gated-exports";

/// Enough slack over a lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the steps themselves are the service's own.
const SLACK: Duration = Duration::from_millis(150);

/// The export column holding the member under test, and the one holding the preference in
/// force for it — the pair the row must not contradict itself across.
const WORKLOAD_COLUMN: &str = "InferredWorkloadTypes";
/// See [`WORKLOAD_COLUMN`].
const EFFECTIVE_COLUMN: &str = "EffectiveRecommendationPreferencesInferredWorkloadTypes";

/// How a cell holding several values separates them, so one exportable field is still one
/// column.
const LIST_SEPARATOR: &str = "|";

/// The `FilterName` that selects on the member under test, shared by the two surfaces whose
/// recommendations carry it (`Filter$name`).
const WORKLOAD_FILTER: &str = "InferredWorkloadTypes";

/// One recommendation surface whose recommendations carry inferred workload types — the
/// `ResourceType` the preference is written for, its export operation, the reader that is the
/// oracle for it, the reader's recommendation list member, the ARN member of one
/// recommendation and the export column that ARN lands in.
struct Surface {
    resource_type: &'static str,
    export: &'static str,
    reader: &'static str,
    list: &'static str,
    arn_member: &'static str,
    arn_column: &'static str,
}

/// The two surfaces the rule covers: `inferredWorkloadTypes` "can only be set" for these
/// resource types (the service's own refusal for any other names exactly this pair), and
/// theirs are the only exportable field sets declaring the column.
const SURFACES: &[Surface] = &[
    Surface {
        resource_type: RESOURCE_TYPE_EC2_INSTANCE,
        export: "ExportEC2InstanceRecommendations",
        reader: "GetEC2InstanceRecommendations",
        list: "instanceRecommendations",
        arn_member: "instanceArn",
        arn_column: "InstanceArn",
    },
    Surface {
        resource_type: RESOURCE_TYPE_AUTO_SCALING_GROUP,
        export: "ExportAutoScalingGroupRecommendations",
        reader: "GetAutoScalingGroupRecommendations",
        list: "autoScalingGroupRecommendations",
        arn_member: "autoScalingGroupArn",
        arn_column: "AutoScalingGroupArn",
    },
];

/// An emulator with the account opted in, the destination bucket created and the analysis the
/// enrollment starts already due — which is what a recommendation reader reports over, so it
/// is what an export file holds.
async fn analysed_emulator() -> MultiServiceRouter {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
    router
}

/// The inferred workload types the reader reports for each recommendation of `surface`, keyed
/// by the resource's ARN. A recommendation reporting the member at all reports a list; one
/// whose inference is deactivated reports no member, which is the empty list here.
async fn reported_workload_types(
    router: &MultiServiceRouter,
    case: &str,
    surface: &Surface,
    filters: Option<&Value>,
) -> BTreeMap<String, Vec<String>> {
    let mut request = json!({"maxResults": 100});
    if let Some(filters) = filters {
        request["filters"] = filters.clone();
    }
    let (status, body) = call(router, surface.reader, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {} with {request}: got {status}: {body}",
        surface.reader
    );
    body[surface.list]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} reports a recommendation list, got {body}",
                surface.reader
            )
        })
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
            let inferred: Vec<String> = recommendation["inferredWorkloadTypes"]
                .as_array()
                .map(|types| {
                    types
                        .iter()
                        .map(|workload| workload.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            (arn, inferred)
        })
        .collect()
}

/// Write the inference preference for `surface` at `value`, at the scope that surface accepts
/// it at: an Auto Scaling group preference "can only be set at the resource level by
/// specifying a scope name of `ResourceArn`", while an instance preference is written here at
/// the account level, covering every instance of the account.
async fn deactivate_or_activate(
    router: &MultiServiceRouter,
    surface: &Surface,
    arns: &[String],
    value: &str,
) {
    let scopes: Vec<Value> = if surface.resource_type == RESOURCE_TYPE_AUTO_SCALING_GROUP {
        arns.iter()
            .map(|arn| json!({"name": SCOPE_RESOURCE_ARN, "value": arn}))
            .collect()
    } else {
        vec![json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})]
    };
    for scope in scopes {
        put_preference(
            router,
            &json!({
                "resourceType": surface.resource_type,
                "scope": scope,
                "inferredWorkloadTypes": value,
            }),
        )
        .await;
    }
}

/// Export `surface`'s recommendations, wait for the clock to complete the job and answer the
/// rows of the file it wrote: the header row first, then one row per exported recommendation.
async fn exported_rows(
    router: &MultiServiceRouter,
    case: &str,
    surface: &Surface,
    fields: &[&str],
    filters: Option<&Value>,
) -> Vec<Vec<String>> {
    let mut request = json!({
        "s3DestinationConfig": {"bucket": BUCKET},
        "fieldsToExport": fields,
    });
    if let Some(filters) = filters {
        request["filters"] = filters.clone();
    }
    let (status, body) = call(router, surface.export, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    let (bucket, key, _) = export_destination(case, &body);
    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
    let (status, object) = get_object(router, &bucket, &key).await;
    assert_eq!(
        status, 200,
        "case {case:?}: downloading {key}: got {status}: {}",
        String::from_utf8_lossy(&object)
    );
    let document = String::from_utf8(object)
        .unwrap_or_else(|error| panic!("case {case:?}: the export file is text: {error}"));
    csv_rows(&document)
}

/// Every exported row's `InferredWorkloadTypes` cell holds exactly what the recommendation
/// reader of the same analysis reports for that resource — the types the analysis inferred
/// while the inference is active, and nothing at all once a preference deactivated it — and
/// no row contradicts the preference it states in the same row.
#[tokio::test]
async fn an_exported_row_reports_the_inferred_workload_types_the_reader_reports() {
    // The preference values the table spans: every `InferredWorkloadTypesPreference` the
    // model declares (`INFERRED_WORKLOAD_TYPES_PREFERENCE_VALUES`: Active | Inactive), plus
    // the account writing none at all, for which the feature "is active by default".
    let mut wanted: Vec<Option<&str>> = vec![None];
    wanted.extend(
        INFERRED_WORKLOAD_TYPES_PREFERENCE_VALUES
            .iter()
            .map(|value| Some(*value)),
    );

    let mut saw_reported = false;
    let mut saw_withheld = false;
    for surface in SURFACES {
        for preference in &wanted {
            let case = format!(
                "{} with the inference preference {}",
                surface.export,
                preference.unwrap_or("unset")
            );
            // A fresh emulator per row: one export job of a resource type is in progress per
            // Region at a time, and each row writes its own preference.
            let router = analysed_emulator().await;

            // What the analysis inferred before any preference was written, which is what the
            // row under an active inference must hold and what the deactivated row withholds.
            let baseline = reported_workload_types(&router, &case, surface, None).await;
            assert!(
                baseline.values().any(|types| !types.is_empty()),
                "case {case:?}: the analysis must infer a workload type for at least one \
                 resource, or the row proves nothing, got {baseline:?}"
            );
            let arns: Vec<String> = baseline.keys().cloned().collect();
            if let Some(value) = preference {
                deactivate_or_activate(&router, surface, &arns, value).await;
            }

            // The oracle: what the reader reports with that preference in force.
            let reported = reported_workload_types(&router, &case, surface, None).await;

            let rows = exported_rows(
                &router,
                &case,
                surface,
                &[surface.arn_column, WORKLOAD_COLUMN, EFFECTIVE_COLUMN],
                None,
            )
            .await;
            let (header, data) = rows
                .split_first()
                .unwrap_or_else(|| panic!("case {case:?}: the export file carries a header row"));
            assert!(
                !data.is_empty(),
                "case {case:?}: the export holds a row per analysed resource, got {rows:?}"
            );

            let in_force = preference.unwrap_or(INFERRED_WORKLOAD_TYPES_ACTIVE);
            for row in data {
                let arn = cell(header, row, surface.arn_column);
                let expected = reported
                    .get(arn)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the export row names {arn:?}, which {} does not \
                             report, got {reported:?}",
                            surface.reader
                        )
                    })
                    .join(LIST_SEPARATOR);
                let actual = cell(header, row, WORKLOAD_COLUMN);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: the exported row of {arn:?} must hold the inferred \
                     workload types the reader reports ({expected:?}), got {actual:?}"
                );
                let effective = cell(header, row, EFFECTIVE_COLUMN);
                assert_eq!(
                    effective, in_force,
                    "case {case:?}: the row of {arn:?} states the preference in force \
                     ({in_force}), got {effective:?}"
                );
                if effective == INFERRED_WORKLOAD_TYPES_ACTIVE {
                    saw_reported |= !actual.is_empty();
                } else {
                    assert!(
                        actual.is_empty(),
                        "case {case:?}: the row of {arn:?} reports {effective} inference \
                         beside workload types {actual:?}"
                    );
                    // The analysis DID infer types for this resource, so the empty cell is
                    // the preference withholding them rather than there being none.
                    saw_withheld |= baseline
                        .get(arn)
                        .is_some_and(|inferred| !inferred.is_empty());
                }
            }
        }
    }
    assert!(
        saw_reported && saw_withheld,
        "the table must span a row whose exported cell holds inferred types and one whose \
         preference withholds types the analysis did infer, or it proves nothing (reported: \
         {saw_reported}, withheld: {saw_withheld})"
    );
}

/// The `InferredWorkloadTypes` filter of an export selects exactly what the same filter
/// selects for the reader — it selects on the member the recommendation reports, so once a
/// preference deactivated the inference it selects no recommendation and no export row.
#[tokio::test]
async fn the_inference_preference_gates_what_the_workload_filter_selects() {
    // The value that deactivates the inference: the `InferredWorkloadTypesPreference` value
    // the model declares beside the one that leaves it on, rather than a spelling written
    // into the test.
    let inactive = INFERRED_WORKLOAD_TYPES_PREFERENCE_VALUES
        .iter()
        .copied()
        .find(|value| *value != INFERRED_WORKLOAD_TYPES_ACTIVE)
        .expect("the model declares a value that deactivates the inference");

    for surface in SURFACES {
        // (the preference written before the filtered call, whether anything is selected)
        let rows: Vec<(Option<&str>, bool)> = vec![
            (None, true),
            (Some(INFERRED_WORKLOAD_TYPES_ACTIVE), true),
            (Some(inactive), false),
        ];
        for (preference, expect_selected) in rows {
            let case = format!(
                "{} filtered with the inference preference {}",
                surface.export,
                preference.unwrap_or("unset")
            );
            // A fresh emulator per row, for the same reason as above.
            let router = analysed_emulator().await;

            // The value filtered on is one this analysis inferred, taken from the reader
            // rather than written into the test.
            let baseline = reported_workload_types(&router, &case, surface, None).await;
            let arns: Vec<String> = baseline.keys().cloned().collect();
            let workload = baseline
                .values()
                .flatten()
                .next()
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the analysis infers a workload type, got {baseline:?}")
                });
            if let Some(value) = preference {
                deactivate_or_activate(&router, surface, &arns, value).await;
            }

            let filters = json!([{"name": WORKLOAD_FILTER, "values": [workload.clone()]}]);
            let selected = reported_workload_types(&router, &case, surface, Some(&filters)).await;
            assert_eq!(
                !selected.is_empty(),
                expect_selected,
                "case {case:?}: the filter on {workload:?} selected {} recommendation(s) for \
                 the reader",
                selected.len()
            );

            let rows = exported_rows(
                &router,
                &case,
                surface,
                &[surface.arn_column, WORKLOAD_COLUMN],
                Some(&filters),
            )
            .await;
            let exported: Vec<&Vec<String>> = rows.iter().skip(1).collect();
            assert_eq!(
                !exported.is_empty(),
                !selected.is_empty(),
                "case {case:?}: the filter on {workload:?} selected {} recommendation(s) for \
                 the reader but {} row(s) for the export, got {rows:?}",
                selected.len(),
                exported.len()
            );
        }
    }
}
