// SPDX-License-Identifier: CC0-1.0

//! The audit record the state-changing replication, metrics and record-expiration
//! operations emit.
//!
//! The rule under test: a call that changes stored state leaves exactly ONE
//! structured record on the service's audit target, naming the operation, the
//! access key id the request presented, the resource it changed and how the call
//! came out — and a call that changes nothing (a read, or a refused write)
//! leaves none. Each case captures the records its own thread writes, drives the
//! operations through the real router, and asserts over the records that come
//! out; nothing is written to disk.
//!
//! The subscriber doing the capturing is this binary's ONE global default,
//! installed on first use ([`capture_events`]), and what scopes a case to its own
//! records is the per-thread buffer that subscriber hands them to.
//!
//! The table spans what the rule generalises over: which operation ran, whether
//! the write created, replaced, removed or left the record as it was, and
//! whether the call succeeded or was refused.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use s3tables::state::AUDIT_TARGET;

use super::harness::{expected_bucket_arn, Emulator, PRESENTED_AKID};
use super::table_record_expiration::{create_table, mark_managed};

/// One captured event: its target and its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// A field's value, with the quotes a `Debug`-recorded string carries
    /// removed, so a record reads the same whichever way the handler recorded
    /// the field.
    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .get(name)
            .map(|value| value.trim_matches('"'))
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

/// The buffer one case's records are kept in, so it can read them back.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// Every audit record captured so far for `operation`, oldest first.
    fn records_for(&self, operation: &str) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .filter(|event| {
                event.target == AUDIT_TARGET && event.field("operation") == Some(operation)
            })
            .cloned()
            .collect()
    }

    /// The ONE audit record for `operation`, or an explanation of what was
    /// captured instead.
    fn record_for(&self, operation: &str, case: &str) -> CapturedEvent {
        let matching = self.records_for(operation);
        assert_eq!(
            matching.len(),
            1,
            "case {case:?} on {operation}: expected exactly one audit record, captured \
             {matching:?}"
        );
        matching.into_iter().next().expect("the one record")
    }

    /// Assert that `operation` accounted for nothing.
    fn assert_silent(&self, operation: &str, case: &str) {
        let records = self.records_for(operation);
        assert!(
            records.is_empty(),
            "case {case:?} on {operation}: a call that changes nothing must emit no audit \
             record, captured {records:?}"
        );
    }

    /// Keep `event`, with its fields rendered as strings.
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
}

thread_local! {
    /// The buffer the calling thread is capturing into, for as long as it holds a
    /// [`CaptureGuard`]; `None` on every thread that is not capturing, whose
    /// events are dropped.
    static CAPTURING: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

/// The subscriber this binary installs: it hands every event to the buffer the
/// thread that emitted it is capturing into.
struct ThreadCapture;

impl Subscriber for ThreadCapture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // These cases assert on events, not spans; one id for every span is
        // enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        CAPTURING.with(|capturing| {
            let Ok(held) = capturing.try_borrow() else {
                return;
            };
            let Some(capture) = held.as_ref() else {
                return;
            };
            capture.event(event);
        });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Capture this thread's `tracing` events for the life of the returned guard.
///
/// The subscriber is installed ONCE, as this binary's global default, rather than
/// scoped to the calling thread: `tracing` caches PER CALLSITE, for the whole
/// process, whether any subscriber is interested in it, and recomputes that cache
/// from the subscribers registered at the moment a callsite is first used. The
/// suites of this binary run in parallel, so a record written for the first time
/// by a sibling suite's call — on a thread holding no subscriber of its own —
/// would cache its callsite as uninteresting and go unseen here ever after. One
/// subscriber for the life of the binary leaves that cache interested whatever
/// order the suites run in, and the per-thread buffer is what keeps one case's
/// records out of another's assertions.
fn capture_events() -> (Capture, CaptureGuard) {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        tracing::subscriber::set_global_default(ThreadCapture)
            .expect("this module is the only installer of a global tracing subscriber");
    });
    let capture = Capture::default();
    CAPTURING.with(|capturing| *capturing.borrow_mut() = Some(capture.clone()));
    (capture, CaptureGuard)
}

/// Stops the calling thread capturing when the case that started it ends.
struct CaptureGuard;

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        CAPTURING.with(|capturing| *capturing.borrow_mut() = None);
    }
}

/// Assert the shape every audit record of this service shares: the operation,
/// the identity the request presented, the resource the call changed and the
/// outcome.
fn assert_record(
    capture: &Capture,
    operation: &str,
    resource: &str,
    outcome: &str,
    case: &str,
) {
    let record = capture.record_for(operation, case);
    assert_eq!(
        record.field("caller"),
        Some(PRESENTED_AKID),
        "case {case:?} on {operation}: the record must attribute the call to the presented \
         access key id: {record:?}"
    );
    assert_eq!(
        record.field("resource"),
        Some(resource),
        "case {case:?} on {operation}: the record must name the resource it changed: {record:?}"
    );
    assert_eq!(
        record.field("outcome"),
        Some(outcome),
        "case {case:?} on {operation}: expected outcome {outcome:?}: {record:?}"
    );
}

/// A `TableBucketReplicationConfiguration` naming one destination.
fn replication_configuration(role: &str, destination: &str) -> serde_json::Value {
    json!({
        "role": role,
        "rules": [{ "destinations": [{ "destinationTableBucketARN": destination }] }],
    })
}

#[tokio::test]
async fn each_replication_write_accounts_for_itself() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let source = emulator
        .create_table_bucket(region, &json!({ "name": "audited-replication-source" }))
        .await
        .text("arn");
    let destination = emulator
        .create_table_bucket(region, &json!({ "name": "audited-replication-target" }))
        .await
        .text("arn");
    let configuration = replication_configuration(
        "arn:aws:iam::000000000000:role/audited-replication",
        &destination,
    );

    // The put that creates the configuration.
    let token = {
        let (capture, _guard) = capture_events();
        let put = emulator
            .put_table_bucket_replication(
                region,
                &[("tableBucketARN", source.clone())],
                &configuration,
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case \"the first put\": the put must succeed, got {status} {body}",
            status = put.status,
            body = put.body
        );
        assert_record(
            &capture,
            "PutTableBucketReplication",
            &source,
            "created",
            "the first put",
        );
        put.text("versionToken")
    };

    // The put that replaces it, presenting the current token.
    let replaced_token = {
        let (capture, _guard) = capture_events();
        let put = emulator
            .put_table_bucket_replication(
                region,
                &[
                    ("tableBucketARN", source.clone()),
                    ("versionToken", token.clone()),
                ],
                &configuration,
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case \"the replacing put\": the put must succeed, got {status} {body}",
            status = put.status,
            body = put.body
        );
        assert_record(
            &capture,
            "PutTableBucketReplication",
            &source,
            "replaced",
            "the replacing put",
        );
        put.text("versionToken")
    };

    // A read changes nothing, so it accounts for nothing.
    {
        let (capture, _guard) = capture_events();
        let read = emulator.get_table_bucket_replication(region, &source).await;
        assert_eq!(
            read.status, 200,
            "case \"the read\": the read must succeed, got {status} {body}",
            status = read.status,
            body = read.body
        );
        capture.assert_silent("GetTableBucketReplication", "the read");
    }

    // A refused write changes nothing either: the stale-token conflict and the
    // absent-bucket not-found both account for nothing.
    for (case, query) in [
        (
            "the stale-token conflict",
            vec![
                ("tableBucketARN", source.clone()),
                ("versionToken", token.clone()),
            ],
        ),
        (
            "a bucket the store does not hold",
            vec![(
                "tableBucketARN",
                expected_bucket_arn(region, "audited-replication-absent"),
            )],
        ),
    ] {
        let (capture, _guard) = capture_events();
        let refused = emulator
            .put_table_bucket_replication(region, &query, &configuration)
            .await;
        assert!(
            refused.status == 409 || refused.status == 404,
            "case {case:?}: the write must be refused, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        capture.assert_silent("PutTableBucketReplication", case);
        let refused = emulator
            .delete_table_bucket_replication(region, &query)
            .await;
        assert!(
            refused.status == 409 || refused.status == 404,
            "case {case:?}: the delete must be refused, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        capture.assert_silent("DeleteTableBucketReplication", case);
    }

    // The delete that removes the configuration.
    {
        let (capture, _guard) = capture_events();
        let deleted = emulator
            .delete_table_bucket_replication(
                region,
                &[
                    ("tableBucketARN", source.clone()),
                    ("versionToken", replaced_token),
                ],
            )
            .await;
        assert_eq!(
            deleted.status, 204,
            "case \"the delete\": the delete must succeed, got {status} {body}",
            status = deleted.status,
            body = deleted.body
        );
        assert_record(
            &capture,
            "DeleteTableBucketReplication",
            &source,
            "deleted",
            "the delete",
        );
    }
}

#[tokio::test]
async fn each_metrics_delete_accounts_for_what_it_changed() {
    let emulator = Emulator::new();
    let region = "eu-west-1";
    let arn = emulator
        .create_table_bucket(region, &json!({ "name": "audited-metrics-bucket" }))
        .await
        .text("arn");

    // The delete of a bucket that has no configuration leaves it as it was,
    // which is the outcome its record carries.
    {
        let (capture, _guard) = capture_events();
        let deleted = emulator
            .delete_table_bucket_metrics_configuration(region, &arn)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case \"nothing configured\": the delete must succeed, got {status} {body}",
            status = deleted.status,
            body = deleted.body
        );
        assert_record(
            &capture,
            "DeleteTableBucketMetricsConfiguration",
            &arn,
            "unchanged",
            "nothing configured",
        );
    }

    let put = emulator
        .put_table_bucket_metrics_configuration(region, &arn)
        .await;
    assert_eq!(
        put.status, 204,
        "case \"the configured bucket\": the put must succeed, got {status} {body}",
        status = put.status,
        body = put.body
    );

    // A read changes nothing, so it accounts for nothing.
    {
        let (capture, _guard) = capture_events();
        let read = emulator
            .get_table_bucket_metrics_configuration(region, &arn)
            .await;
        assert_eq!(
            read.status, 200,
            "case \"the read\": the read must succeed, got {status} {body}",
            status = read.status,
            body = read.body
        );
        capture.assert_silent("GetTableBucketMetricsConfiguration", "the read");
    }

    // The delete that removes the configuration.
    {
        let (capture, _guard) = capture_events();
        let deleted = emulator
            .delete_table_bucket_metrics_configuration(region, &arn)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case \"the configured bucket\": the delete must succeed, got {status} {body}",
            status = deleted.status,
            body = deleted.body
        );
        assert_record(
            &capture,
            "DeleteTableBucketMetricsConfiguration",
            &arn,
            "deleted",
            "the configured bucket",
        );
    }

    // A delete against a bucket the store does not hold changes nothing.
    {
        let (capture, _guard) = capture_events();
        let refused = emulator
            .delete_table_bucket_metrics_configuration(
                region,
                &expected_bucket_arn(region, "audited-metrics-absent"),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case \"a bucket the store does not hold\": expected 404, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        capture.assert_silent(
            "DeleteTableBucketMetricsConfiguration",
            "a bucket the store does not hold",
        );
    }
}

#[tokio::test]
async fn a_record_expiration_write_accounts_for_the_table_it_changed() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    let managed = create_table(
        &emulator,
        region,
        "audited-expiration-bucket",
        "audited_table",
    )
    .await;
    mark_managed(&emulator, &managed);
    let value = json!({ "status": "enabled", "settings": { "days": 30 } });

    // The put that configures expiration on a table that takes one.
    {
        let (capture, _guard) = capture_events();
        let put = emulator
            .put_table_record_expiration_configuration(region, &managed, &value)
            .await;
        assert_eq!(
            put.status, 204,
            "case \"a table that takes one\": the put must succeed, got {status} {body}",
            status = put.status,
            body = put.body
        );
        assert_record(
            &capture,
            "PutTableRecordExpirationConfiguration",
            &managed,
            "configured",
            "a table that takes one",
        );
    }

    // Both reads change nothing, so neither accounts for anything.
    {
        let (capture, _guard) = capture_events();
        for (operation, reply) in [
            (
                "GetTableRecordExpirationConfiguration",
                emulator
                    .get_table_record_expiration_configuration(region, &managed)
                    .await,
            ),
            (
                "GetTableRecordExpirationJobStatus",
                emulator
                    .get_table_record_expiration_job_status(region, &managed)
                    .await,
            ),
        ] {
            assert_eq!(
                reply.status, 200,
                "case \"the reads\" on {operation}: the read must succeed, got {status} {body}",
                status = reply.status,
                body = reply.body
            );
            capture.assert_silent(operation, "the reads");
        }
    }

    // A put a table takes no expiration configuration for, and one against a
    // table the store does not hold, change nothing.
    let customer = create_table(
        &emulator,
        region,
        "audited-expiration-customer",
        "customer_table",
    )
    .await;
    for (case, table_arn, expected_status) in [
        ("a table that takes none", customer.clone(), 405),
        (
            "a table the store does not hold",
            format!("{}/table/absent-table-id", {
                expected_bucket_arn(region, "audited-expiration-bucket")
            }),
            404,
        ),
    ] {
        let (capture, _guard) = capture_events();
        let refused = emulator
            .put_table_record_expiration_configuration(region, &table_arn, &value)
            .await;
        assert_eq!(
            refused.status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        capture.assert_silent("PutTableRecordExpirationConfiguration", case);
    }
}
