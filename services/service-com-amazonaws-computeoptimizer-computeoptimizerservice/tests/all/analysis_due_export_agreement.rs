// SPDX-License-Identifier: CC0-1.0

//! The reader and the export of ONE analysis agree about whether that analysis exists yet.
//!
//! Compute Optimizer reports an analysis of an account's resources only to an ENROLLED
//! account, and only once the analysis that enrollment starts is due. That rule is about the
//! ANALYSIS rather than about one surface, so the two surfaces an analysis is served through —
//! the recommendation reader and the recommendations export — must answer the same thing at
//! the same instant: while the analysis is not due the reader reports an empty list and an
//! export asked for at that instant holds its header row alone, and once it is due both carry
//! it, over the same set of resources.
//!
//! What is asserted is the whole class of surfaces against the rule: one row per resource type
//! this service both reads and exports an analysis of, each row differing in the resource type,
//! in the member its reader reports a resource's ARN under and in the column its export file
//! names it under. `GetECSServiceRecommendations` is not a row: an Amazon ECS service
//! recommendation is computed from the Amazon ECS inventory and the utilization published for
//! it, so it is not read from the analysis this gate governs.
//!
//! Every call goes through the front door a client uses: the Compute Optimizer calls as
//! `POST /` with `X-Amz-Target`, the export file downloaded from its destination bucket over
//! Amazon S3's own path-style `GetObject`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, RdsDatabaseDetail, ANALYSIS_DELAY, EXPORT_JOB_STEP,
    SOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cell, create_bucket, csv_rows, emulator_with_engine, export_destination, get_object,
    opt_in, ACCOUNT,
};

/// The destination bucket these fixtures export to.
const BUCKET: &str = "compute-optimizer-analysis-gate";

/// Enough slack over one of the service's own spans for the clock to have made it due whatever
/// the test scheduler is doing; the spans themselves are the service's.
const SLACK: Duration = Duration::from_millis(150);

/// One surface pair of the account's analysis: the operation that READS it, the operation that
/// EXPORTS it, and how each of them names the resource a recommendation is about.
#[derive(Debug, Clone, Copy)]
struct Surface {
    /// The reader of this resource type's analysis.
    reader: &'static str,
    /// The output member it lists the recommendations under.
    items: &'static str,
    /// The member of one reported recommendation that carries the resource's ARN, as the
    /// recommendation shape declares it (`smithy_query query="shape"
    /// shape="InstanceRecommendation,VolumeRecommendation,IdleRecommendation,\
    /// LicenseRecommendation,LambdaFunctionRecommendation,RDSDBRecommendation,\
    /// AutoScalingGroupRecommendation"`).
    reported_arn: &'static str,
    /// The export of the same analysis.
    export: &'static str,
    /// The column its export file names the resource's ARN under: the field that resource
    /// type's own `Exportable*Field` enum declares for it.
    exported_arn: &'static str,
}

/// Every resource type this service both reads and exports a stored analysis of.
fn surfaces() -> Vec<Surface> {
    vec![
        Surface {
            reader: "GetAutoScalingGroupRecommendations",
            items: "autoScalingGroupRecommendations",
            reported_arn: "autoScalingGroupArn",
            export: "ExportAutoScalingGroupRecommendations",
            exported_arn: "AutoScalingGroupArn",
        },
        Surface {
            reader: "GetEC2InstanceRecommendations",
            items: "instanceRecommendations",
            reported_arn: "instanceArn",
            export: "ExportEC2InstanceRecommendations",
            exported_arn: "InstanceArn",
        },
        Surface {
            reader: "GetEBSVolumeRecommendations",
            items: "volumeRecommendations",
            reported_arn: "volumeArn",
            export: "ExportEBSVolumeRecommendations",
            exported_arn: "VolumeArn",
        },
        Surface {
            reader: "GetIdleRecommendations",
            items: "idleRecommendations",
            reported_arn: "resourceArn",
            export: "ExportIdleRecommendations",
            exported_arn: "ResourceArn",
        },
        Surface {
            reader: "GetLambdaFunctionRecommendations",
            items: "lambdaFunctionRecommendations",
            reported_arn: "functionArn",
            export: "ExportLambdaFunctionRecommendations",
            exported_arn: "FunctionArn",
        },
        Surface {
            reader: "GetLicenseRecommendations",
            items: "licenseRecommendations",
            reported_arn: "resourceArn",
            export: "ExportLicenseRecommendations",
            exported_arn: "ResourceArn",
        },
        Surface {
            reader: "GetRDSDatabaseRecommendations",
            items: "rdsDBRecommendations",
            reported_arn: "resourceArn",
            export: "ExportRDSDatabaseRecommendations",
            exported_arn: "ResourceArn",
        },
    ]
}

/// The resources the reader reports an analysis of, and the body it answered.
///
/// The read asks for no page size, and the whole selection is asserted to fit one page (no
/// `nextToken`), so what comes back is everything the reader has to report rather than a
/// prefix of it.
async fn reported_arns(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
) -> (BTreeSet<String>, Value) {
    let (status, body) = call(router, surface.reader, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: an account with nothing analysed is not a failed request, expected 200, \
         got {status}: {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: the one page is the whole selection, got {body}"
    );
    let reported = body[surface.items]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected {} to be a list, got {body}",
                surface.items
            )
        })
        .iter()
        .map(|recommendation| {
            recommendation[surface.reported_arn]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: every recommendation names its resource under {}, got \
                         {recommendation}",
                        surface.reported_arn
                    )
                })
                .to_string()
        })
        .collect();
    (reported, body)
}

/// Export the same analysis and download the file the job writes, answering the resources its
/// rows are about and how many rows it holds.
///
/// The file is of the analysis as it stood when the export was ASKED for, so an export
/// requested before the analysis was due reports that instant however long the job then takes
/// to complete.
async fn exported_arns(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
) -> (BTreeSet<String>, usize) {
    let columns = vec!["AccountId".to_string(), surface.exported_arn.to_string()];
    let (status, body) = call(
        router,
        surface.export,
        json!({
            "s3DestinationConfig": {"bucket": BUCKET},
            "fieldsToExport": columns,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the export to be accepted, got {status}: {body}"
    );
    let (bucket, key, _) = export_destination(case, &body);

    // A created job is `Queued` for one step and `InProgress` for the next, then `Complete` —
    // which is when the objects it answered are in the bucket.
    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
    let (status, downloaded) = get_object(router, &bucket, &key).await;
    assert_eq!(
        status, 200,
        "case {case:?}: downloading {key}: got {status}: {}",
        String::from_utf8_lossy(&downloaded)
    );
    let document = String::from_utf8(downloaded)
        .unwrap_or_else(|error| panic!("case {case:?}: the export file is text: {error}"));
    let rows = csv_rows(&document);
    let header = rows.first().cloned().unwrap_or_default();
    assert_eq!(
        header, columns,
        "case {case:?}: the header row is the fields the request asked for, got {document:?}"
    );
    let data = rows.get(1..).unwrap_or_default();
    for row in data {
        assert_eq!(
            cell(&header, row, "AccountId"),
            ACCOUNT,
            "case {case:?}: every row names the account that owns the resource, got {row:?}"
        );
    }
    let exported = data
        .iter()
        .map(|row| cell(&header, row, surface.exported_arn).to_string())
        .collect();
    (exported, data.len())
}

/// Record one analysed Amazon Aurora or RDS database into the findings catalog, the way
/// Compute Optimizer's own analysis of an observed workload would: a database recommendation
/// is an analysis of a DB instance the account owns, so the database surface has one to serve
/// only once such an analysis exists.
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

/// One enrollment, two instants, every surface pair: while the analysis the enrollment started
/// is not due the reader reports nothing and an export asked for then holds its header row
/// alone; once it is due the reader reports the analysis and an export asked for then holds the
/// very resources the reader reports.
#[tokio::test]
async fn the_reader_and_the_export_of_one_analysis_agree_about_whether_it_exists() {
    for surface in surfaces() {
        // A fresh emulator per row: only one export job of a resource type is in progress per
        // Region at a time, and each row creates two.
        let (router, engine) = emulator_with_engine();
        record_analysed_database(&engine);
        create_bucket(&router, BUCKET).await;
        opt_in(&router).await;

        let case = format!("just opted in/{}", surface.reader);
        let (reported, body) = reported_arns(&router, &case, surface).await;
        assert_eq!(
            reported,
            BTreeSet::new(),
            "case {case:?}: the analysis is not due yet, so nothing is reported, got {body}"
        );
        let (exported, rows) = exported_arns(&router, &case, surface).await;
        assert_eq!(
            rows, 0,
            "case {case:?}: an export of an analysis that is not due holds its header row \
             alone, got {exported:?}"
        );

        // The analysis comes due one span after the enrollment record was written
        // (`ANALYSIS_DELAY`); the extra margin covers scheduler jitter.
        tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
        let case = format!("the analysis is due/{}", surface.reader);
        let (reported, body) = reported_arns(&router, &case, surface).await;
        assert!(
            !reported.is_empty(),
            "case {case:?}: once the analysis is due it is reported, got {body}"
        );
        let (exported, rows) = exported_arns(&router, &case, surface).await;
        assert!(
            rows > 0,
            "case {case:?}: once the analysis is due an export of it holds rows, got \
             {exported:?}"
        );
        assert_eq!(
            exported, reported,
            "case {case:?}: the export holds the resources the reader reports, reported \
             {reported:?}"
        );
    }
}
