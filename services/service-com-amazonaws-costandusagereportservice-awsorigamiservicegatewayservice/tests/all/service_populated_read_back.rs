// SPDX-License-Identifier: CC0-1.0

//! What a read-back of a report definition carries beyond the request that created it,
//! and what a delete answers.
//!
//! Two rules, both about a SUCCESS body:
//!
//! 1. **A read-back reports the resource, not the request.** `DescribeReportDefinitions`
//!    answers the stored definition PLUS every member the service itself populates: the
//!    billing view the report is scoped to (the one the request named, or the account's
//!    primary billing view when it named none), the status of the report's last delivery
//!    (absent while nothing has delivered it) and the date the definition was
//!    created. Each comes from the stored record, so every read answers the same value and
//!    none of them can be dictated by the caller.
//! 2. **An optional output member the service does not populate is not serialized.**
//!    `DeleteReportDefinition` answers HTTP 200 with the empty object on both of its legs
//!    — the one that removed a report and the one that found nothing to remove — so no
//!    per-call diagnostic, and nothing from the request, appears in the body.
//! 3. **A definition the service could not deliver is refused, not stored.** A report's
//!    Parquet preference is its `Format`, its `Compression` and the artifacts it
//!    subscribes to taken together, and a definition that states it inconsistently is
//!    refused on the create and the update path alike — so no read-back ever reports a
//!    definition the service would not hold.
//!
//! Both are asserted over TABLES whose rows differ along what the rules generalise over:
//! whether the creation named a billing view and in which form, which optional members the
//! definition carries, how many reports the account holds, whether a delivery has been
//! recorded against the report, the instant the report was created, which part of a
//! Parquet preference the definition states, and which leg of the delete is called.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice as cur;

use cur::ops::types::ReportDefinition;
use cur::state::{ReportDelivery, StoredReport};

use super::report_definition_lifecycle::{
    expected_read_back, is_report_timestamp, minimal_definition, primary_billing_view_arn,
    read_back_of, Cur,
};

/// A stored report created at `created_at`, carrying `last_delivery`, so a test can pin
/// what the projection makes of a record it could not reach through the API.
fn stored_at(
    report_name: &str,
    created_at: SystemTime,
    last_delivery: Option<ReportDelivery>,
) -> StoredReport {
    StoredReport {
        definition: ReportDefinition {
            report_name: report_name.to_string(),
            time_unit: "DAILY".to_string(),
            format: "textORcsv".to_string(),
            compression: "GZIP".to_string(),
            additional_schema_elements: vec!["RESOURCES".to_string()],
            s3_bucket: "seeded-bucket".to_string(),
            s3_prefix: "cur/".to_string(),
            s3_region: "us-east-1".to_string(),
            ..Default::default()
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

#[tokio::test]
async fn a_read_back_carries_the_billing_view_the_report_is_scoped_to() {
    // Rows differ in whether the creation named a billing view and in the form it named
    // it in: both forms the modelled `BillingViewArn` pattern admits — a full
    // `arn:aws:billing::<12 digits>:billingview/<name>` and the bare view name its
    // optional ARN prefix leaves out (`smithy_query(shape="BillingViewArn")`) — plus the
    // creation that names none, which is the recorded case.
    let primary = primary_billing_view_arn();
    let table: Vec<(&str, Option<&str>, &str)> = vec![
        ("no billing view named", None, primary.as_str()),
        (
            "a billing group's own billing view",
            Some("arn:aws:billing::123456789012:billingview/123456789012"),
            "arn:aws:billing::123456789012:billingview/123456789012",
        ),
        (
            "a billing view named without the ARN prefix",
            Some("my-billing.view_1"),
            "my-billing.view_1",
        ),
        (
            "a billing view in the China partition",
            Some("arn:aws-cn:billing::210987654321:billingview/primary"),
            "arn:aws-cn:billing::210987654321:billingview/primary",
        ),
    ];

    for (case, named, expected) in table {
        let cur = Cur::new();
        let mut definition = minimal_definition("billing-view-report");
        if let Some(named) = named {
            definition["BillingViewArn"] = json!(named);
        }

        let (status, body) = cur.put(&definition, None).await;
        assert_eq!(status, 200, "case {case:?}: PutReportDefinition got {body:?}");

        let listed = cur.describe_all().await;
        let first = listed.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            first["BillingViewArn"],
            json!(expected),
            "case {case:?}: expected BillingViewArn {expected:?}, got {first:?}"
        );
        // The whole element is the definition plus the service-populated members — the
        // billing view is not the only one that must be there.
        assert_eq!(
            read_back_of(&first, case),
            expected_read_back(&definition),
            "case {case:?}: expected the stored definition plus the service-populated \
             members, got {first:?}"
        );
    }
}

#[tokio::test]
async fn a_read_back_carries_the_status_of_the_report_last_delivery() {
    // Rows differ in whether a delivery has been recorded against the report and in how
    // that delivery ended: every value of the modelled `LastStatus` enum
    // (`smithy_query(shape="LastStatus")`) is covered, beside the report nothing has
    // delivered, whose read-back states no `ReportStatus` at all — the member is optional
    // and neither of its values has a form the model admits for "never delivered".
    let delivered_at = at_epoch_seconds(1_700_000_000);
    let mut table: Vec<(String, Option<ReportDelivery>, Option<Value>)> =
        vec![("no delivery recorded".to_string(), None, None)];
    for status in cur::ops::types::LAST_STATUS_VALUES {
        table.push((
            format!("a delivery that ended {status}"),
            Some(ReportDelivery {
                at: delivered_at,
                status: (*status).to_string(),
            }),
            Some(json!({ "lastDelivery": "20231114T221320Z", "lastStatus": status })),
        ));
    }

    for (case, delivery, expected) in table {
        let cur = Cur::new();
        cur.seed(vec![stored_at(
            "delivered-report",
            at_epoch_seconds(1_767_225_600),
            delivery,
        )]);

        let listed = cur.describe_all().await;
        let first = listed.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            first["ReportName"],
            json!("delivered-report"),
            "case {case:?}: expected the seeded report to be listed, got {listed:?}"
        );
        assert_eq!(
            first.get("ReportStatus").cloned(),
            expected,
            "case {case:?}: expected ReportStatus {expected:?}, got {first:?}"
        );
    }
}

#[tokio::test]
async fn a_read_back_carries_the_date_the_definition_was_created() {
    // The creation date is the stored instant stated in the form this service states a
    // report timestamp in (the modelled `LastDelivery` pattern
    // `^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$`, `smithy_query(shape="LastDelivery")`).
    // Rows differ in the instant: the epoch, a mid-range instant, a leap day, and a
    // whole-minute instant, so the rendering is pinned over the calendar and not over one
    // recorded moment.
    let table: Vec<(&str, u64, &str)> = vec![
        ("the epoch", 0, "19700101T000000Z"),
        ("a mid-range instant", 1_700_000_000, "20231114T221320Z"),
        ("a leap day", 1_709_210_096, "20240229T123456Z"),
        ("the start of a year", 1_767_225_600, "20260101T000000Z"),
    ];

    for (case, seconds, expected) in table {
        let cur = Cur::new();
        cur.seed(vec![stored_at(
            "dated-report",
            at_epoch_seconds(seconds),
            None,
        )]);

        let listed = cur.describe_all().await;
        let first = listed.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            first["ReportCreationDate"],
            json!(expected),
            "case {case:?}: expected ReportCreationDate {expected:?}, got {first:?}"
        );
    }

    // A report created through the API carries the instant of its creation: the value is
    // this run's, so what is asserted is its form and that it does not move between reads.
    let cur = Cur::new();
    for index in 0..3 {
        let (status, body) = cur
            .put(&minimal_definition(&format!("created-report-{index}")), None)
            .await;
        assert_eq!(status, 200, "case created report {index}: got {body:?}");
    }
    let first_pass = cur.describe_all().await;
    for listed in &first_pass {
        let created = listed["ReportCreationDate"].as_str().unwrap_or_default();
        assert!(
            is_report_timestamp(created),
            "case created report: expected a creation date stated as yyyymmddThhmmssZ, got \
             {listed:?}"
        );
    }
    assert_eq!(
        cur.describe_all().await,
        first_pass,
        "case created report: a creation date must not move between reads, got {first_pass:?}"
    );
}

#[tokio::test]
async fn a_caller_cannot_dictate_the_members_the_service_populates() {
    // A `ReportDefinition` the caller sends may carry `ReportStatus` and
    // `ReportCreationDate`, but both are facts about the report the SERVICE holds — the
    // last delivery and the moment of creation — so what comes back is the service's, not
    // the request's. Rows differ in which of the two the request tries to set and in the
    // operation that carries it (create and update).
    let table: Vec<(&str, Value)> = vec![
        (
            "a delivery status the request claims",
            json!({ "ReportStatus": { "lastDelivery": "20200101T000000Z", "lastStatus": "SUCCESS" } }),
        ),
        (
            "a creation date the request claims",
            json!({ "ReportCreationDate": "20200101T000000Z" }),
        ),
        (
            "both of them at once",
            json!({
                "ReportStatus": { "lastDelivery": "20200101T000000Z", "lastStatus": "ERROR_PERMISSIONS" },
                "ReportCreationDate": "20200101T000000Z",
            }),
        ),
    ];

    for (case, claimed) in table {
        for operation in ["PutReportDefinition", "ModifyReportDefinition"] {
            let cur = Cur::new();
            let mut definition = minimal_definition("claiming-report");
            let (status, body) = cur.put(&definition, None).await;
            assert_eq!(status, 200, "case {case:?} via {operation}: setup got {body:?}");

            for (member, value) in claimed.as_object().expect("an object of claims") {
                definition[member] = value.clone();
            }
            let request = match operation {
                "PutReportDefinition" => {
                    // The name is free in a fresh account for the create leg.
                    definition["ReportName"] = json!("second-claiming-report");
                    json!({ "ReportDefinition": definition })
                }
                _ => json!({
                    "ReportName": "claiming-report",
                    "ReportDefinition": definition,
                }),
            };
            let (status, body) = cur.call(operation, &request).await;
            assert_eq!(status, 200, "case {case:?} via {operation}: got {body:?}");

            let listed = cur.describe_all().await;
            let claimed_name = definition["ReportName"].as_str().expect("a report name");
            let subject = listed
                .iter()
                .find(|definition| definition["ReportName"] == json!(claimed_name))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                subject["ReportName"],
                json!(claimed_name),
                "case {case:?} via {operation}: expected the report to be listed, got {listed:?}"
            );
            assert!(
                subject.get("ReportStatus").is_none(),
                "case {case:?} via {operation}: nothing delivered this report, so its read-back \
                 must state no ReportStatus, got {subject:?}"
            );
            let created = subject["ReportCreationDate"].as_str().unwrap_or_default();
            assert!(
                is_report_timestamp(created) && created != "20200101T000000Z",
                "case {case:?} via {operation}: expected the creation date the service holds, \
                 got {subject:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_definition_whose_parquet_preference_is_not_usable_is_refused() {
    // A report's Parquet preference is `Format`, `Compression` and the artifacts it
    // subscribes to taken together: a definition that states Parquet at all must state it
    // in both members and may not also ask for the manifests built from a delimited-text
    // report. Rows differ along every part of that rule — which member states Parquet,
    // which artifacts are subscribed to, and how many — and the recorded case is one of
    // them. The artifact catalog is the modelled one
    // (`smithy_query(shape="AdditionalArtifact")`).
    let table: Vec<(&str, &str, &str, Value, bool)> = vec![
        // case, Format, Compression, AdditionalArtifacts, accepted
        (
            "no Parquet preference at all",
            "textORcsv",
            "GZIP",
            Value::Null,
            true,
        ),
        (
            "no Parquet preference, subscribing to the delimited-text manifests",
            "textORcsv",
            "ZIP",
            json!(["QUICKSIGHT", "REDSHIFT"]),
            true,
        ),
        (
            "a whole Parquet preference",
            "Parquet",
            "Parquet",
            Value::Null,
            true,
        ),
        (
            "a whole Parquet preference with the Athena manifest",
            "Parquet",
            "Parquet",
            json!(["ATHENA"]),
            true,
        ),
        (
            "Parquet stated by the format alone",
            "Parquet",
            "GZIP",
            Value::Null,
            false,
        ),
        (
            "Parquet stated by the compression alone",
            "textORcsv",
            "Parquet",
            Value::Null,
            false,
        ),
        (
            "a Parquet report subscribing to the Redshift manifest",
            "Parquet",
            "Parquet",
            json!(["REDSHIFT"]),
            false,
        ),
        (
            "a Parquet report subscribing to the QuickSight manifest",
            "Parquet",
            "Parquet",
            json!(["QUICKSIGHT"]),
            false,
        ),
        (
            "a Parquet report subscribing to Athena beside a delimited-text manifest",
            "Parquet",
            "Parquet",
            json!(["ATHENA", "REDSHIFT"]),
            false,
        ),
    ];

    /// The fixed text the refusal carries: it names the rule, not a member of the request.
    const REFUSAL: &str =
        "For valid Parquet preference, report format and compression must be 'Parquet' \
         and cannot subscribe to QuickSight and/or Redshift";

    for (case, format, compression, artifacts, accepted) in table {
        // Through the create path...
        let cur = Cur::new();
        let mut definition = minimal_definition("parquet-report");
        definition["Format"] = json!(format);
        definition["Compression"] = json!(compression);
        if !artifacts.is_null() {
            definition["AdditionalArtifacts"] = artifacts.clone();
        }

        let (status, body) = cur.put(&definition, None).await;
        let listed = cur.describe_all().await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} via PutReportDefinition: expected HTTP 200, got {status} {body:?}"
            );
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: the accepted definition must be stored, got {listed:?}"
            );
        } else {
            assert_eq!(
                (status, body["__type"].as_str().unwrap_or_default()),
                (400, "ValidationException"),
                "case {case:?} via PutReportDefinition: expected a 400 ValidationException, \
                 got {status} {body:?}"
            );
            assert_eq!(
                body["Message"].as_str().or(body["message"].as_str()),
                Some(REFUSAL),
                "case {case:?} via PutReportDefinition: expected the Parquet-preference \
                 refusal, got {body:?}"
            );
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused create must store nothing, got {listed:?}"
            );
        }

        // ...and through the update path, where the stored definition must survive a
        // refusal untouched.
        let cur = Cur::new();
        let base = minimal_definition("parquet-report");
        let (status, body) = cur.put(&base, None).await;
        assert_eq!(status, 200, "case {case:?}: setup got {body:?}");
        let (status, body) = cur
            .call(
                "ModifyReportDefinition",
                &json!({ "ReportName": "parquet-report", "ReportDefinition": definition }),
            )
            .await;
        let listed = cur.read_back_all(case).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} via ModifyReportDefinition: expected HTTP 200, got {status} \
                 {body:?}"
            );
            assert_eq!(
                listed,
                vec![expected_read_back(&definition)],
                "case {case:?}: the accepted replacement must be stored, got {listed:?}"
            );
        } else {
            assert_eq!(
                (status, body["__type"].as_str().unwrap_or_default()),
                (400, "ValidationException"),
                "case {case:?} via ModifyReportDefinition: expected a 400 ValidationException, \
                 got {status} {body:?}"
            );
            assert_eq!(
                body["Message"].as_str().or(body["message"].as_str()),
                Some(REFUSAL),
                "case {case:?} via ModifyReportDefinition: expected the Parquet-preference \
                 refusal, got {body:?}"
            );
            assert_eq!(
                listed,
                vec![expected_read_back(&base)],
                "case {case:?}: a refused update must leave the stored definition alone, got \
                 {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn delete_answers_the_empty_object_on_both_legs() {
    // Rows differ in what the account holds when the delete arrives and in the name's
    // spelling: a report that exists, the same name once it is gone, a name never
    // created, and a name using the punctuation the `ReportName` pattern allows. The body
    // is the same empty object every time.
    let table: Vec<(&str, &str, bool)> = vec![
        ("a report the account holds", "held-report", true),
        ("a name whose report was already deleted", "held-report", false),
        ("a name never created", "never-created", false),
        (
            "a punctuated name the account holds",
            "r3port!-_.*'()",
            true,
        ),
    ];

    let cur = Cur::new();
    for (case, report_name, create_first) in table {
        if create_first {
            let (status, body) = cur.put(&minimal_definition(report_name), None).await;
            assert_eq!(status, 200, "case {case:?}: setup got {body:?}");
        }

        let (status, body) = cur
            .call(
                "DeleteReportDefinition",
                &json!({ "ReportName": report_name }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {body:?}");
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: expected an empty body carrying no ResponseMessage, got {body:?}"
        );
        assert!(
            !body.to_string().contains(report_name),
            "case {case:?}: the body must not echo the report name, got {body:?}"
        );

        let listed = cur.describe_all().await;
        assert!(
            !listed
                .iter()
                .any(|definition| definition["ReportName"] == json!(report_name)),
            "case {case:?}: the deleted report must be gone, got {listed:?}"
        );
    }
}
