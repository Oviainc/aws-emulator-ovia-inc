// SPDX-License-Identifier: CC0-1.0

//! Every report definition this service reports is a definition it accepts back.
//!
//! The rule: a member the service fills on a read is filled only with a value the
//! constraints its own shape carries in the model admit, and an OPTIONAL member the
//! service holds no value for is left out of the response rather than filled with a
//! stand-in those constraints reject. `ModifyReportDefinition` replaces a definition
//! wholesale, so read-modify-write is this API's normal way to change one: the definition
//! `DescribeReportDefinitions` answers has to be a definition `PutReportDefinition` and
//! `ModifyReportDefinition` take, and the boundary validator (whose refusals
//! `modelled_constraints.rs` pins) is what decides that.
//!
//! The report's delivery status is the member the rule bites on: both members of the
//! modelled `ReportStatus` are optional and constrained — `lastDelivery` to length 16..20
//! and the pattern `^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$`, `lastStatus` to the
//! `LastStatus` catalog `SUCCESS | ERROR_PERMISSIONS | ERROR_NO_BUCKET`
//! (`smithy_query(shape="LastDelivery,LastStatus")`) — and nothing in this service's
//! modelled surface delivers a report, so a report created through the API has no
//! delivery status to state.
//!
//! The table's rows differ along what the rule generalises over: whether a delivery has
//! been recorded against the report, how that delivery ended (every value of the modelled
//! catalog), the instant it was delivered, and which optional members the stored
//! definition carries and in which form. The create leg of each row also replays a
//! reported definition that CLAIMS a delivery status, which the new report must not take
//! as its own.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice as cur;

use cur::ops::types::{ReportDefinition, LAST_STATUS_VALUES};
use cur::state::{ReportDelivery, StoredReport};

use super::report_definition_lifecycle::{
    error_code, is_report_timestamp, minimal_definition, Cur,
};

/// The strings of a JSON list member.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One optional string member of a JSON definition.
fn text(definition: &Value, member: &str) -> Option<String> {
    definition[member].as_str().map(str::to_string)
}

/// A stored report holding `definition`, created at `created_at`, with `last_delivery`
/// recorded against it.
///
/// Seeding is how a delivered report is set up at all: nothing in this service's modelled
/// surface delivers a report, so the API cannot put a delivery on one.
fn stored_report(
    definition: &Value,
    created_at: SystemTime,
    last_delivery: Option<ReportDelivery>,
) -> StoredReport {
    StoredReport {
        definition: ReportDefinition {
            report_name: definition["ReportName"].as_str().unwrap_or_default().to_string(),
            time_unit: definition["TimeUnit"].as_str().unwrap_or_default().to_string(),
            format: definition["Format"].as_str().unwrap_or_default().to_string(),
            compression: definition["Compression"].as_str().unwrap_or_default().to_string(),
            additional_schema_elements: strings(&definition["AdditionalSchemaElements"]),
            s3_bucket: definition["S3Bucket"].as_str().unwrap_or_default().to_string(),
            s3_prefix: definition["S3Prefix"].as_str().unwrap_or_default().to_string(),
            s3_region: definition["S3Region"].as_str().unwrap_or_default().to_string(),
            additional_artifacts: definition.get("AdditionalArtifacts").map(strings),
            refresh_closed_reports: definition["RefreshClosedReports"].as_bool(),
            report_versioning: text(definition, "ReportVersioning"),
            billing_view_arn: text(definition, "BillingViewArn"),
            // Both are facts about the report the service holds, never the request's.
            report_status: None,
            report_creation_date: None,
        },
        tags: BTreeMap::new(),
        created_at,
        last_delivery,
    }
}

/// One instant, as a number of seconds after the epoch.
fn at_epoch_seconds(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The definitions the rows are built from: which optional members a stored definition
/// carries, and in which form.
fn definitions() -> Vec<(&'static str, Value)> {
    let mut bare_billing_view = minimal_definition("round-trip-report");
    bare_billing_view["BillingViewArn"] = json!("my-billing.view_1");
    bare_billing_view["RefreshClosedReports"] = json!(true);
    vec![
        ("required members only", minimal_definition("round-trip-report")),
        (
            "every optional member stated",
            json!({
                "ReportName": "round-trip-report",
                "TimeUnit": "HOURLY",
                "Format": "Parquet",
                "Compression": "Parquet",
                "AdditionalSchemaElements": ["RESOURCES", "SPLIT_COST_ALLOCATION_DATA"],
                "S3Bucket": "round.trip-bucket",
                "S3Prefix": "teams/finance/cur/",
                "S3Region": "eu-west-1",
                "AdditionalArtifacts": ["ATHENA"],
                "RefreshClosedReports": false,
                "ReportVersioning": "OVERWRITE_REPORT",
                "BillingViewArn": "arn:aws:billing::123456789012:billingview/123456789012",
            }),
        ),
        (
            "an empty prefix and an empty artifact list",
            json!({
                "ReportName": "round-trip-report",
                "TimeUnit": "MONTHLY",
                "Format": "textORcsv",
                "Compression": "ZIP",
                "AdditionalSchemaElements": [],
                "S3Bucket": "bucket_with_underscores",
                "S3Prefix": "",
                "S3Region": "ap-southeast-2",
                "AdditionalArtifacts": [],
            }),
        ),
        (
            "a billing view named without the ARN prefix",
            bare_billing_view,
        ),
    ]
}

/// The deliveries the rows are built from: none, and one per value of the modelled
/// `LastStatus` catalog, each at a different instant.
fn deliveries() -> Vec<(String, Option<ReportDelivery>)> {
    let mut rows: Vec<(String, Option<ReportDelivery>)> =
        vec![("no delivery recorded".to_string(), None)];
    // The instants differ per row so the reported date is not pinned to one moment: the
    // epoch, a leap day and a mid-range instant.
    let instants = [0_u64, 1_709_210_096, 1_700_000_000];
    for (index, status) in LAST_STATUS_VALUES.iter().enumerate() {
        rows.push((
            format!("a delivery that ended {status}"),
            Some(ReportDelivery {
                at: at_epoch_seconds(instants[index % instants.len()]),
                status: (*status).to_string(),
            }),
        ));
    }
    rows
}

#[tokio::test]
async fn a_reported_definition_states_a_delivery_status_only_when_one_was_recorded() {
    for (definition_case, definition) in definitions() {
        for (delivery_case, delivery) in deliveries() {
            let case = format!("{definition_case} / {delivery_case}");
            let cur = Cur::new();
            cur.seed(vec![stored_report(
                &definition,
                at_epoch_seconds(1_767_225_600),
                delivery.clone(),
            )]);

            let listed = cur.describe_all().await;
            let reported = listed.first().cloned().unwrap_or(Value::Null);
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: expected the seeded report to be listed, got {listed:?}"
            );
            let status = reported.get("ReportStatus").cloned();

            match &delivery {
                None => assert!(
                    status.is_none(),
                    "case {case:?}: no delivery was recorded, so the read-back must state no \
                     ReportStatus, got {reported:?}"
                ),
                Some(recorded) => {
                    let status = status.unwrap_or(Value::Null);
                    let last_delivery = status["lastDelivery"].as_str().unwrap_or_default();
                    assert!(
                        is_report_timestamp(last_delivery),
                        "case {case:?}: expected lastDelivery stated as yyyymmddThhmmssZ, got \
                         {status:?}"
                    );
                    assert_eq!(
                        status["lastStatus"],
                        json!(recorded.status),
                        "case {case:?}: expected lastStatus {:?}, got {status:?}",
                        recorded.status
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_reported_definition_is_accepted_back_unchanged() {
    for (definition_case, definition) in definitions() {
        for (delivery_case, delivery) in deliveries() {
            let case = format!("{definition_case} / {delivery_case}");
            let cur = Cur::new();
            cur.seed(vec![stored_report(
                &definition,
                at_epoch_seconds(1_767_225_600),
                delivery.clone(),
            )]);

            let listed = cur.describe_all().await;
            let reported = listed.first().cloned().unwrap_or(Value::Null);
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: expected the seeded report to be listed, got {listed:?}"
            );
            let report_name = reported["ReportName"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            // Read-modify-write: the definition just read, replayed wholesale.
            let (status, body) = cur
                .call(
                    "ModifyReportDefinition",
                    &json!({ "ReportName": report_name, "ReportDefinition": reported }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (200, "<no __type>"),
                "case {case:?}: the definition the service reported must be one it accepts \
                 back, got {status} {body:?}"
            );
            assert_eq!(
                cur.describe_all().await,
                listed,
                "case {case:?}: replaying the reported definition must leave the report as it \
                 was, got {reported:?}"
            );

            // The same definition under a free name, through the create path: accepted,
            // and the new report has no delivery of its own to report.
            let mut copy = reported.clone();
            copy["ReportName"] = json!(format!("{report_name}-copy"));
            let (status, body) = cur.put(&copy, None).await;
            assert_eq!(
                (status, error_code(&body)),
                (200, "<no __type>"),
                "case {case:?}: the reported definition must be accepted as a new report, got \
                 {status} {body:?}"
            );

            let created = cur
                .describe_all()
                .await
                .into_iter()
                .find(|listed| listed["ReportName"] == copy["ReportName"])
                .unwrap_or(Value::Null);
            assert!(
                created.get("ReportStatus").is_none(),
                "case {case:?}: nothing delivered the new report, so it must state no \
                 ReportStatus, got {created:?}"
            );
            // Every member of the definition is the one the service reported, so the copy
            // differs from it only in the name and in the facts the service holds.
            let definition_members = [
                "TimeUnit",
                "Format",
                "Compression",
                "AdditionalSchemaElements",
                "S3Bucket",
                "S3Prefix",
                "S3Region",
                "AdditionalArtifacts",
                "RefreshClosedReports",
                "ReportVersioning",
                "BillingViewArn",
            ];
            for member in definition_members {
                assert_eq!(
                    created[member], reported[member],
                    "case {case:?}: expected {member} {:?} on the new report, got {created:?}",
                    reported[member]
                );
            }
        }
    }
}
