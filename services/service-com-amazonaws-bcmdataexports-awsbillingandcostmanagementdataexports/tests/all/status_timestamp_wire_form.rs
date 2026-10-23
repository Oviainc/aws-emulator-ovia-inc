// SPDX-License-Identifier: CC0-1.0

//! The wire FORM of this service's timestamps.
//!
//! RULE: a timestamp member whose model declares `@timestampFormat("date-time")` goes onto
//! the wire as an RFC 3339 date-time STRING naming the stored instant — never as the
//! awsJson1_1 protocol default, an epoch-seconds NUMBER — on every operation that reports
//! one and for every instant.
//!
//! `smithy_query(query="shape",shape="ExportStatus,ExecutionStatus")` reports
//! `"timestampFormat": "date-time"` on `ExportStatus$CreatedAt`, `$LastUpdatedAt`,
//! `$LastRefreshedAt` and on `ExecutionStatus$CreatedAt`, `$CompletedAt`, `$LastUpdatedAt`:
//! those six are the whole timestamp surface this service models, and all six are covered
//! below, including the three no recorded request ever cited.
//!
//! Three levels, because the rule holds at each:
//!
//! 1. the member SCHEMAS carry the format (and a non-timestamp member carries none), which
//!    is what the codec reads;
//! 2. serializing a status over a TABLE of instants — the epoch, sub-second precision,
//!    before the epoch, far future — writes a string for each, never a number, and the
//!    string round-trips to the same instant;
//! 3. end to end over the router: every `ExportStatus` a 200 carries, from each operation
//!    that reports one, is such a string.

use aws_smithy_schema::serde::ShapeSerializer;
use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::json::JsonShapeSerializer;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::{
    ExecutionStatus, ExportStatus,
};

use crate::export_lifecycle::{cases, restated_table_configurations, timestamp, Client};

/// Every member the model marks `@timestampFormat("date-time")`, as `(label, schema)`.
///
/// Source: `smithy_query(query="shape",shape="ExportStatus,ExecutionStatus")` — the only
/// two shapes in this service with timestamp members.
fn date_time_members() -> Vec<(&'static str, &'static Schema)> {
    vec![
        ("ExportStatus$CreatedAt", &ExportStatus::M_CREATED_AT),
        (
            "ExportStatus$LastUpdatedAt",
            &ExportStatus::M_LAST_UPDATED_AT,
        ),
        (
            "ExportStatus$LastRefreshedAt",
            &ExportStatus::M_LAST_REFRESHED_AT,
        ),
        ("ExecutionStatus$CreatedAt", &ExecutionStatus::M_CREATED_AT),
        (
            "ExecutionStatus$CompletedAt",
            &ExecutionStatus::M_COMPLETED_AT,
        ),
        (
            "ExecutionStatus$LastUpdatedAt",
            &ExecutionStatus::M_LAST_UPDATED_AT,
        ),
    ]
}

/// The members of the same two shapes that are NOT timestamps: the model gives them no
/// `@timestampFormat`, so neither may their schema.
fn non_timestamp_members() -> Vec<(&'static str, &'static Schema)> {
    vec![
        ("ExportStatus$StatusCode", &ExportStatus::M_STATUS_CODE),
        ("ExportStatus$StatusReason", &ExportStatus::M_STATUS_REASON),
        (
            "ExecutionStatus$StatusCode",
            &ExecutionStatus::M_STATUS_CODE,
        ),
    ]
}

/// LEVEL 1: the format the codec reads is on the schema, for every modelled member.
#[test]
fn every_modelled_date_time_member_schema_carries_the_format() {
    for (case, schema) in date_time_members() {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the model declares @timestampFormat(\"date-time\"), so the member \
             schema must report it, got {format:?}"
        );
    }

    for (case, schema) in non_timestamp_members() {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model declares no @timestampFormat on this member, so its \
             schema must report none, got {format:?}"
        );
    }
}

/// The instants level 2 writes out: the epoch itself, whole seconds, sub-second precision
/// at millisecond and microsecond scale, before the epoch, and far future. The rule
/// generalises over the instant, so the table differs along nothing else.
///
/// Sub-second rows are microsecond-aligned because the RFC 3339 rendering carries
/// microseconds — the resolution of the form itself, not of this service.
fn instants() -> Vec<(&'static str, DateTime)> {
    vec![
        ("the epoch", DateTime::from_secs(0)),
        ("whole seconds", DateTime::from_secs(1_761_112_361)),
        (
            "millisecond precision",
            DateTime::from_secs_and_nanos(1_761_112_361, 123_000_000),
        ),
        (
            "microsecond precision",
            DateTime::from_secs_and_nanos(1_761_112_361, 987_654_000),
        ),
        ("before the epoch", DateTime::from_secs(-1_000_000_000)),
        ("far future", DateTime::from_secs(4_102_444_800)),
    ]
}

/// Assert `value` is an RFC 3339 date-time string naming `expected`.
fn assert_date_time_string(case: &str, member: &str, value: Option<&Value>, expected: DateTime) {
    let Some(raw) = value else {
        panic!("case {case:?}: {member} is absent from the serialized status")
    };
    let Value::String(text) = raw else {
        panic!(
            "case {case:?}: {member} is @timestampFormat(\"date-time\"), so it must be an RFC \
             3339 string and never the protocol's epoch number, got {raw}"
        )
    };
    let parsed = DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {member} {text:?} is not RFC 3339: {e}"));
    assert_eq!(
        (parsed.secs(), parsed.subsec_nanos()),
        (expected.secs(), expected.subsec_nanos()),
        "case {case:?}: {member} must name the stored instant, got {text:?}"
    );
    assert!(
        text.contains('T') && text.ends_with('Z'),
        "case {case:?}: {member} must be the UTC RFC 3339 rendering (T separator, Z offset), \
         got {text:?}"
    );
}

/// LEVEL 2: serializing either status shape writes each date-time member as a string that
/// names the instant it was given, for every row of [`instants`].
#[test]
fn a_status_serializes_every_instant_as_a_date_time_string() {
    for (case, instant) in instants() {
        let export = ExportStatus {
            status_code: Some("HEALTHY".to_string()),
            status_reason: None,
            created_at: Some(instant),
            last_updated_at: Some(instant),
            last_refreshed_at: Some(instant),
        };
        let serialized = serialize(&ExportStatus::SCHEMA, &export, case);
        for member in ["CreatedAt", "LastUpdatedAt", "LastRefreshedAt"] {
            assert_date_time_string(case, member, serialized.get(member), instant);
        }
        assert_eq!(
            serialized.get("StatusCode").and_then(Value::as_str),
            Some("HEALTHY"),
            "case {case:?}: a non-timestamp member must still serialize, got {serialized}"
        );

        let execution = ExecutionStatus {
            status_code: Some("QUERY_QUEUED".to_string()),
            status_reason: None,
            created_at: Some(instant),
            completed_at: Some(instant),
            last_updated_at: Some(instant),
        };
        let serialized = serialize(&ExecutionStatus::SCHEMA, &execution, case);
        for member in ["CreatedAt", "CompletedAt", "LastUpdatedAt"] {
            assert_date_time_string(case, member, serialized.get(member), instant);
        }
    }
}

/// `value` through the same JSON codec the awsJson1_1 wire uses, as parsed JSON.
fn serialize(
    schema: &Schema,
    value: &dyn aws_smithy_schema::serde::SerializableStruct,
    case: &str,
) -> Value {
    let mut serializer = JsonShapeSerializer::new();
    serializer
        .write_struct(schema, value)
        .unwrap_or_else(|e| panic!("case {case:?}: the status serializes: {e}"));
    let text = serializer.into_string();
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("case {case:?}: the codec wrote invalid JSON ({e}): {text}"))
}

/// LEVEL 3: end to end — every `ExportStatus` a 200 carries is a date-time string, for each
/// operation that reports one and for each export definition in the lifecycle table.
#[tokio::test]
async fn every_reported_export_status_is_a_date_time_string() {
    for case in cases() {
        let client = Client::new();
        let created = client.ok("CreateExport", &case.create_request()).await;
        let arn = created
            .get("ExportArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateExport must return an ExportArn, got {created}"))
            .to_string();

        // GetExport reports the status directly.
        let got = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        let status = got
            .get("ExportStatus")
            .unwrap_or_else(|| panic!("GetExport must report an ExportStatus, got {got}"));
        let label = format!("GetExport {arn}");
        // `timestamp` refuses anything but the RFC 3339 string.
        let created_at = timestamp(&label, status, "CreatedAt");
        let last_updated_at = timestamp(&label, status, "LastUpdatedAt");
        assert!(
            created_at.secs() > 0 && last_updated_at.secs() > 0,
            "case {label:?}: a created export's instants come from the clock, got {status}"
        );
        assert_date_time_string(&label, "CreatedAt", status.get("CreatedAt"), created_at);
        assert_date_time_string(
            &label,
            "LastUpdatedAt",
            status.get("LastUpdatedAt"),
            last_updated_at,
        );

        // ListExports reports the SAME status nested in an ExportReference.
        let listed = client.ok("ListExports", &json!({})).await;
        let reference = listed
            .get("Exports")
            .and_then(Value::as_array)
            .and_then(|exports| {
                exports
                    .iter()
                    .find(|e| e.get("ExportArn").and_then(Value::as_str) == Some(arn.as_str()))
            })
            .unwrap_or_else(|| panic!("ListExports must list {arn}, got {listed}"))
            .clone();
        let nested = reference.get("ExportStatus").unwrap_or_else(|| {
            panic!("case {arn:?}: an ExportReference carries an ExportStatus, got {reference}")
        });
        let nested_label = format!("ListExports {arn}");
        assert_date_time_string(
            &nested_label,
            "CreatedAt",
            nested.get("CreatedAt"),
            created_at,
        );
        assert_date_time_string(
            &nested_label,
            "LastUpdatedAt",
            nested.get("LastUpdatedAt"),
            last_updated_at,
        );

        // An update writes a NEW instant, which is reported in the same form. The update
        // restates the definition it read back, with the `TableConfigurations` entry an
        // update owes for each queried table ("All export parameters must be specified in
        // the UpdateExport request").
        let mut definition = got
            .get("Export")
            .cloned()
            .unwrap_or_else(|| panic!("GetExport must report the Export, got {got}"));
        {
            let object = definition
                .as_object_mut()
                .expect("the Export structure is a JSON object");
            object.insert("Description".to_string(), json!("updated"));
            let data_query = object
                .get_mut("DataQuery")
                .and_then(Value::as_object_mut)
                .expect("the DataQuery structure is a JSON object");
            if let Some(restated) = restated_table_configurations(
                case.queried_tables(),
                data_query.get("TableConfigurations"),
            ) {
                data_query.insert("TableConfigurations".to_string(), restated);
            }
        }
        client
            .ok(
                "UpdateExport",
                &json!({ "ExportArn": arn, "Export": definition }),
            )
            .await;
        let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        let after_status = after
            .get("ExportStatus")
            .unwrap_or_else(|| panic!("GetExport must report an ExportStatus, got {after}"));
        let after_label = format!("GetExport after UpdateExport {arn}");
        let updated_at = timestamp(&after_label, after_status, "LastUpdatedAt");
        assert_date_time_string(
            &after_label,
            "LastUpdatedAt",
            after_status.get("LastUpdatedAt"),
            updated_at,
        );
        assert_date_time_string(
            &after_label,
            "CreatedAt",
            after_status.get("CreatedAt"),
            created_at,
        );
    }
}
