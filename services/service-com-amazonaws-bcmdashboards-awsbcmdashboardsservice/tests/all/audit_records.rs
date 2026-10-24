// SPDX-License-Identifier: CC0-1.0

//! The audit record every state-changing dashboard operation emits.
//!
//! A create, an update and a delete each have to leave a structured record saying when
//! the call happened, where it came from, which access key id it presented, which
//! operation ran, which resource it touched and how it came out — on the refusal paths
//! too, and never for an operation that only reports. These tests install a capturing
//! `tracing` subscriber scoped to the calling thread, drive the operations through the
//! real router, and assert over the records that come out. Nothing is written to disk
//! and no global subscriber is installed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::AUDIT_TARGET;

use super::dashboard_lifecycle::{call, ok_call, router, PRESENTED_AKID, REQUEST_SOURCE};

/// One captured event: its target and its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings, whatever type they were recorded as.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a test can read the
/// audit records back.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// The audit records captured so far, oldest first.
    fn audit_records(&self) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .filter(|event| event.target == AUDIT_TARGET)
            .cloned()
            .collect()
    }

    /// Every audit record for `operation`.
    fn records_for(&self, operation: &str) -> Vec<CapturedEvent> {
        self.audit_records()
            .into_iter()
            .filter(|event| event.field("operation") == Some(operation))
            .collect()
    }

    /// The one audit record for `operation`, or an explanation of what was captured.
    fn record_for(&self, operation: &str) -> CapturedEvent {
        let matching = self.records_for(operation);
        assert_eq!(
            matching.len(),
            1,
            "case {operation}: expected exactly one audit record, captured {matching:?}"
        );
        matching.into_iter().next().expect("the one record")
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // These tests assert on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields,
            });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Capture this thread's `tracing` events for the life of the returned guard.
fn capture_events() -> (Capture, tracing::subscriber::DefaultGuard) {
    let capture = Capture::default();
    let guard = tracing::subscriber::set_default(capture.clone());
    (capture, guard)
}

/// Assert the shape every audit record shares, whatever the operation and outcome.
fn assert_record(record: &CapturedEvent, operation: &str, resource: &str, outcome: &str) {
    assert_eq!(
        record.field("operation"),
        Some(operation),
        "case {operation}: wrong operation in {record:?}"
    );
    assert_eq!(
        record.field("resource"),
        Some(resource),
        "case {operation}: wrong resource in {record:?}"
    );
    assert_eq!(
        record.field("outcome"),
        Some(outcome),
        "case {operation}: wrong outcome in {record:?}"
    );
    assert_eq!(
        record.field("akid"),
        Some(PRESENTED_AKID),
        "case {operation}: the record must attribute the call to the presented access key id: {record:?}"
    );
    assert_eq!(
        record.field("source"),
        Some(REQUEST_SOURCE),
        "case {operation}: wrong request source in {record:?}"
    );
    let timestamp_ms: u64 = record
        .field("timestamp_ms")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("case {operation}: no timestamp_ms in {record:?}"));
    assert!(
        timestamp_ms > 1_700_000_000_000,
        "case {operation}: timestamp_ms {timestamp_ms} is not a plausible epoch-milliseconds instant"
    );
}

/// Each state-changing operation leaves one audit record naming the resource it touched
/// and a SUCCESS outcome — and the reporting operations leave none.
#[tokio::test]
async fn state_changing_operations_emit_one_audit_record_each() {
    let router = router();
    let (capture, _guard) = capture_events();

    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Audited Dashboard",
            "widgets": [],
            "resourceTags": [{"key": "env", "value": "prod"}],
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    assert_record(
        &capture.record_for("CreateDashboard"),
        "CreateDashboard",
        &arn,
        "SUCCESS",
    );

    ok_call(
        &router,
        "UpdateDashboard",
        &json!({"arn": arn, "name": "Audited Dashboard 2"}),
    )
    .await;
    assert_record(
        &capture.record_for("UpdateDashboard"),
        "UpdateDashboard",
        &arn,
        "SUCCESS",
    );

    // Adding a tag changes the stored record, so it accounts for itself.
    ok_call(
        &router,
        "TagResource",
        &json!({"resourceArn": arn, "resourceTags": [{"key": "team", "value": "finops"}]}),
    )
    .await;
    assert_record(
        &capture.record_for("TagResource"),
        "TagResource",
        &arn,
        "SUCCESS",
    );

    // Removing a tag changes the stored record too, so it accounts for itself.
    ok_call(
        &router,
        "UntagResource",
        &json!({"resourceArn": arn, "resourceTagKeys": ["env"]}),
    )
    .await;
    assert_record(
        &capture.record_for("UntagResource"),
        "UntagResource",
        &arn,
        "SUCCESS",
    );

    // The reporting operations change nothing, so they account for nothing.
    ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    ok_call(&router, "ListDashboards", &json!({})).await;
    ok_call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": arn}),
    )
    .await;
    for operation in ["GetDashboard", "ListDashboards", "ListTagsForResource"] {
        let leaked = capture.records_for(operation);
        assert!(
            leaked.is_empty(),
            "case {operation}: a reporting operation must emit no audit record, captured {leaked:?}"
        );
    }

    ok_call(&router, "DeleteDashboard", &json!({"arn": arn})).await;
    assert_record(
        &capture.record_for("DeleteDashboard"),
        "DeleteDashboard",
        &arn,
        "SUCCESS",
    );
}

/// A refused state change is audited too, with the modelled error as its outcome: a
/// table over the refusals the three state-changing operations can answer.
#[tokio::test]
async fn refused_state_changes_are_audited_with_their_outcome() {
    let absent = "arn:aws:bcm-dashboards::000000000000:dashboard/never-created";

    // (operation, request, expected status, expected audited resource, expected outcome)
    let cases: &[(&str, Value, u16, &str, &str)] = &[
        (
            "UpdateDashboard",
            json!({"arn": absent, "name": "Nothing To Update"}),
            400,
            absent,
            "ResourceNotFoundException",
        ),
        (
            "DeleteDashboard",
            json!({"arn": absent}),
            400,
            absent,
            "ValidationException",
        ),
        (
            "UntagResource",
            json!({"resourceArn": absent, "resourceTagKeys": ["env"]}),
            400,
            absent,
            "ResourceNotFoundException",
        ),
        (
            "TagResource",
            json!({"resourceArn": absent, "resourceTags": [{"key": "env", "value": "prod"}]}),
            400,
            absent,
            "ResourceNotFoundException",
        ),
    ];

    for (operation, request, expected_status, resource, outcome) in cases {
        let router = router();
        let (capture, _guard) = capture_events();
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, *expected_status,
            "case {operation} refusal: expected {expected_status}, got {status}: {body}"
        );
        assert_record(&capture.record_for(operation), operation, resource, outcome);
    }
}

/// CreateDashboard's quota refusal is audited as well, with the modelled
/// `ServiceQuotaExceededException` as its outcome and the sentinel resource the handler
/// uses when no dashboard was reached.
#[tokio::test]
async fn the_quota_refusal_is_audited() {
    use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::MAX_DASHBOARDS;

    let router = router();
    let request = json!({"name": "Quota Filler", "widgets": []});
    for _ in 0..MAX_DASHBOARDS {
        ok_call(&router, "CreateDashboard", &request).await;
    }

    let (capture, _guard) = capture_events();
    let (status, body) = call(&router, "CreateDashboard", &request).await;
    assert_eq!(
        status, 402,
        "case quota refusal: expected 402, got {status}: {body}"
    );
    assert_record(
        &capture.record_for("CreateDashboard"),
        "CreateDashboard",
        "-",
        "ServiceQuotaExceededException",
    );
}
