// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the three table record-expiration operations:
//! the put that configures it, the read that reports the configuration and the
//! read that reports its job's status.
//!
//! The rule under test: record expiration is a feature of the tables Amazon Web
//! Services manages on an account's behalf, so on a table of any other type
//! every one of the three calls is refused with the modeled
//! `MethodNotAllowedException` and the one fixed sentence this service states
//! that class with — whatever configuration the request carries and whichever of
//! the three it is — and the refused put persists nothing. On a table that does
//! take one, the put replaces the whole configuration, the read reports exactly
//! what the store holds (a table nothing was configured on reports expiration
//! off), and the job status is derived from that configuration: a table whose
//! records expire reports a job that has not yet run, one whose records do not
//! reports the job disabled, and neither reports a run that never happened.
//!
//! Every table this API creates is customer-created, so the eligible rows mark
//! the table AWS-managed in the store directly; the calls themselves all travel
//! the real wire path through the harness.
//!
//! The table spans what the rule generalises over: the status the configuration
//! carries, the `days` it retains records for (the modeled `@range(1,
//! 2147483647)` bounds, and values between them), a configuration with no
//! settings at all, the region, and which of the three operations is called.

use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::{
    ComAmazonawsS3tablesS3tablebucketsState as S3TablesState, OpenTableFormat, RecordExpiration,
    TableType,
};

use super::harness::{Emulator, Reply};

/// The fixed sentence this service states a record-expiration call on a table
/// that takes no record-expiration configuration with.
const NOT_ALLOWED: &str = "Table record expiration configuration API calls to this table are not \
                           allowed";

/// The fixed sentence this service states a table the store does not hold with.
const NO_TABLE: &str = "The specified table does not exist.";

/// One row: the configuration a put carries, and what the reads must report for
/// it once the table takes one.
#[derive(Debug)]
struct Case {
    label: &'static str,
    /// A short name for the resources this row creates: `TableBucketName` is
    /// `@length(3, 63)`, so a row's label is too long to name one with.
    slug: &'static str,
    region: &'static str,
    /// The `value.status` member, absent when the row carries none.
    status: Option<&'static str>,
    /// The `value.settings.days` member, absent when the row carries no
    /// settings.
    days: Option<i32>,
    /// The `TableRecordExpirationJobStatus` the job-status read must report for
    /// a table carrying this configuration.
    job_status: &'static str,
}

impl Case {
    /// The `value` member of a `PutTableRecordExpirationConfiguration` request.
    fn value(&self) -> Value {
        let mut value = Map::new();
        if let Some(status) = self.status {
            value.insert("status".to_string(), json!(status));
        }
        if let Some(days) = self.days {
            value.insert("settings".to_string(), json!({ "days": days }));
        }
        Value::Object(value)
    }

    /// A name for the table this row configures.
    fn table(&self) -> String {
        format!("{}_table", self.slug)
    }

    /// A name for the table bucket this row creates, `kind` telling its rows
    /// apart from another test's.
    fn bucket(&self, kind: &str) -> String {
        format!("expiration-{kind}-{}", self.slug.replace('_', "-"))
    }
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            // `PositiveInteger` is `@range(min: 1)`, so one day is the shortest
            // retention a configuration can name.
            label: "enabled for the shortest retention the model allows",
            slug: "min_days",
            region: "us-east-1",
            status: Some("enabled"),
            days: Some(1),
            job_status: "NotYetRun",
        },
        Case {
            label: "enabled for thirty days",
            slug: "thirty_days",
            region: "eu-west-1",
            status: Some("enabled"),
            days: Some(30),
            job_status: "NotYetRun",
        },
        Case {
            label: "enabled for a year",
            slug: "one_year",
            region: "us-west-2",
            status: Some("enabled"),
            days: Some(365),
            job_status: "NotYetRun",
        },
        Case {
            // `PositiveInteger` is `@range(max: 2147483647)`, so this is the
            // longest retention a configuration can name.
            label: "enabled for the longest retention the model allows",
            slug: "max_days",
            region: "ap-south-1",
            status: Some("enabled"),
            days: Some(2_147_483_647),
            job_status: "NotYetRun",
        },
        Case {
            label: "disabled with a retention still named",
            slug: "disabled_with_days",
            region: "us-east-1",
            status: Some("disabled"),
            days: Some(7),
            job_status: "Disabled",
        },
        Case {
            label: "disabled with no settings",
            slug: "disabled_no_settings",
            region: "eu-central-1",
            status: Some("disabled"),
            days: None,
            job_status: "Disabled",
        },
        Case {
            label: "settings alone with no status",
            slug: "settings_only",
            region: "us-east-1",
            status: None,
            days: Some(90),
            job_status: "Disabled",
        },
    ]
}

/// Create a table bucket, a namespace and one table in it, and answer the
/// table's ARN. Every table this API creates is customer-created.
pub(crate) async fn create_table(
    emulator: &Emulator,
    region: &str,
    bucket: &str,
    table: &str,
) -> String {
    let created = emulator
        .create_table_bucket(region, &json!({ "name": bucket }))
        .await;
    assert_eq!(
        created.status, 200,
        "creating the bucket {bucket:?} must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let bucket_arn = created.text("arn");
    let namespace = format!("{}_ns", table);
    let namespace_created = emulator
        .create_namespace(region, &bucket_arn, &json!({ "namespace": [namespace] }))
        .await;
    assert_eq!(
        namespace_created.status, 200,
        "creating the namespace for {table:?} must succeed, got {status} {body}",
        status = namespace_created.status,
        body = namespace_created.body
    );
    let table_created = emulator
        .create_table(
            region,
            &bucket_arn,
            &namespace,
            &json!({ "name": table, "format": OpenTableFormat::Iceberg.as_wire() }),
        )
        .await;
    assert_eq!(
        table_created.status, 200,
        "creating the table {table:?} must succeed, got {status} {body}",
        status = table_created.status,
        body = table_created.body
    );
    table_created.text("tableARN")
}

/// Mark the table `table_arn` one Amazon Web Services manages, the only kind
/// that takes a record-expiration configuration.
pub(crate) fn mark_managed(emulator: &Emulator, table_arn: &str) {
    let store = emulator.engine.state::<S3TablesState>();
    let mut store = store.lock();
    let table = store
        .table_by_arn_mut(table_arn)
        .unwrap_or_else(|| panic!("the table {table_arn:?} is resident"));
    table.table_type = TableType::Aws;
}

/// The record-expiration configuration the store holds for `table_arn`.
pub(crate) fn stored_expiration(emulator: &Emulator, table_arn: &str) -> Option<RecordExpiration> {
    let store = emulator.engine.state::<S3TablesState>();
    let store = store.lock();
    store
        .table_by_arn(table_arn)
        .and_then(|table| table.record_expiration.clone())
}

/// Assert that `reply` is the 405 `MethodNotAllowedException` this service
/// words with the one not-allowed sentence.
fn assert_not_allowed(reply: &Reply, case: &str, operation: &str) {
    assert_eq!(
        reply.status, 405,
        "case {case:?} on {operation}: expected 405, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("MethodNotAllowedException"),
        "case {case:?} on {operation}: expected MethodNotAllowedException, got {actual:?}",
        actual = reply.error_code
    );
    assert_eq!(
        reply.message(),
        NOT_ALLOWED,
        "case {case:?} on {operation}: expected the fixed not-allowed sentence, got {actual:?}",
        actual = reply.message()
    );
}

/// The `status` and `settings.days` members a configuration read reported.
fn reported_configuration(reply: &Reply, case: &str) -> (Option<String>, Option<i64>) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the read must succeed, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    let configuration = reply.body.get("configuration").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the read has no configuration: {body}",
            body = reply.body
        )
    });
    let status = configuration
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string);
    let days = configuration
        .get("settings")
        .and_then(|settings| settings.get("days"))
        .and_then(Value::as_i64);
    (status, days)
}

#[tokio::test]
async fn every_record_expiration_call_on_a_table_that_takes_none_is_refused() {
    let emulator = Emulator::new();

    for case in cases() {
        let table = case.table();
        let table_arn = create_table(&emulator, case.region, &case.bucket("refused"), &table).await;

        let put = emulator
            .put_table_record_expiration_configuration(case.region, &table_arn, &case.value())
            .await;
        assert_not_allowed(&put, case.label, "PutTableRecordExpirationConfiguration");
        assert_not_allowed(
            &emulator
                .get_table_record_expiration_configuration(case.region, &table_arn)
                .await,
            case.label,
            "GetTableRecordExpirationConfiguration",
        );
        assert_not_allowed(
            &emulator
                .get_table_record_expiration_job_status(case.region, &table_arn)
                .await,
            case.label,
            "GetTableRecordExpirationJobStatus",
        );

        // The refused put persisted nothing.
        assert_eq!(
            stored_expiration(&emulator, &table_arn),
            None,
            "case {label:?}: the refused put must persist nothing, got {stored:?}",
            label = case.label,
            stored = stored_expiration(&emulator, &table_arn)
        );
    }
}

#[tokio::test]
async fn a_put_on_a_table_that_takes_one_is_reported_by_both_reads() {
    let emulator = Emulator::new();

    for case in cases() {
        let table = case.table();
        let table_arn = create_table(&emulator, case.region, &case.bucket("managed"), &table).await;
        mark_managed(&emulator, &table_arn);

        // A table nothing was configured on keeps its records: the read
        // reports expiration off and the job is disabled, and neither read
        // writes a configuration.
        let before = emulator
            .get_table_record_expiration_configuration(case.region, &table_arn)
            .await;
        assert_eq!(
            reported_configuration(&before, case.label),
            (Some("disabled".to_string()), None),
            "case {label:?}: an unconfigured table must report expiration off",
            label = case.label
        );
        let job_before = emulator
            .get_table_record_expiration_job_status(case.region, &table_arn)
            .await;
        assert_eq!(
            job_before.status, 200,
            "case {label:?}: the job-status read must succeed, got {status} {body}",
            label = case.label,
            status = job_before.status,
            body = job_before.body
        );
        assert_eq!(
            job_before.text("status"),
            "Disabled",
            "case {label:?}: an unconfigured table's job must be Disabled, got {actual:?}",
            label = case.label,
            actual = job_before.text("status")
        );
        assert_eq!(
            stored_expiration(&emulator, &table_arn),
            None,
            "case {label:?}: a read must not write a configuration",
            label = case.label
        );

        let put = emulator
            .put_table_record_expiration_configuration(case.region, &table_arn, &case.value())
            .await;
        assert_eq!(
            put.status, 204,
            "case {label:?}: the put must answer the modeled 204, got {status} {body}",
            label = case.label,
            status = put.status,
            body = put.body
        );
        assert_eq!(
            put.body,
            Value::Null,
            "case {label:?}: the put must carry no body, got {body}",
            label = case.label,
            body = put.body
        );

        // The read reports exactly the configuration the put persisted.
        let read = emulator
            .get_table_record_expiration_configuration(case.region, &table_arn)
            .await;
        assert_eq!(
            reported_configuration(&read, case.label),
            (
                case.status.map(str::to_string),
                case.days.map(i64::from)
            ),
            "case {label:?}: the read must report what the put persisted",
            label = case.label
        );

        // The job status is derived from that configuration, and nothing is
        // reported about a run that never happened.
        let job = emulator
            .get_table_record_expiration_job_status(case.region, &table_arn)
            .await;
        assert_eq!(
            job.status, 200,
            "case {label:?}: the job-status read must succeed, got {status} {body}",
            label = case.label,
            status = job.status,
            body = job.body
        );
        assert_eq!(
            job.text("status"),
            case.job_status,
            "case {label:?}: expected job status {expected:?}, got {actual:?}",
            label = case.label,
            expected = case.job_status,
            actual = job.text("status")
        );
        for member in ["lastRunTimestamp", "failureMessage", "metrics"] {
            assert_eq!(
                job.body.get(member),
                None,
                "case {label:?}: nothing ran, so the job must report no {member}, got {actual:?}",
                label = case.label,
                actual = job.body.get(member)
            );
        }

        // The put is `@idempotent` and its value is the whole configuration, so
        // a second put of the SAME value leaves the record as it was, and a put
        // of a different one replaces it whole.
        let repeated = emulator
            .put_table_record_expiration_configuration(case.region, &table_arn, &case.value())
            .await;
        assert_eq!(
            repeated.status, 204,
            "case {label:?}: the repeated put must succeed, got {status} {body}",
            label = case.label,
            status = repeated.status,
            body = repeated.body
        );
        assert_eq!(
            reported_configuration(
                &emulator
                    .get_table_record_expiration_configuration(case.region, &table_arn)
                    .await,
                case.label
            ),
            reported_configuration(&read, case.label),
            "case {label:?}: a repeated put must leave the configuration as it was",
            label = case.label
        );
        let replaced = emulator
            .put_table_record_expiration_configuration(
                case.region,
                &table_arn,
                &json!({ "status": "enabled", "settings": { "days": 2 } }),
            )
            .await;
        assert_eq!(
            replaced.status, 204,
            "case {label:?}: the replacing put must succeed, got {status} {body}",
            label = case.label,
            status = replaced.status,
            body = replaced.body
        );
        assert_eq!(
            reported_configuration(
                &emulator
                    .get_table_record_expiration_configuration(case.region, &table_arn)
                    .await,
                case.label
            ),
            (Some("enabled".to_string()), Some(2)),
            "case {label:?}: a put must replace the whole configuration",
            label = case.label
        );
    }
}

#[tokio::test]
async fn a_retention_outside_the_modeled_range_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let table_arn = create_table(
        &emulator,
        "us-east-1",
        "expiration-range-bucket",
        "range_table",
    )
    .await;
    mark_managed(&emulator, &table_arn);

    // `PositiveInteger` is `@range(1, 2147483647)`: a day count below the
    // minimum is refused as client input before the handler runs, and nothing
    // is persisted for the table.
    for days in [0, -1] {
        let refused = emulator
            .put_table_record_expiration_configuration(
                "us-east-1",
                &table_arn,
                &json!({ "status": "enabled", "settings": { "days": days } }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case \"days={days}\": expected the client-input refusal 400, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        assert_eq!(
            stored_expiration(&emulator, &table_arn),
            None,
            "case \"days={days}\": a refused put must persist nothing"
        );
    }

    // And the bounds themselves are accepted.
    for days in [1, 2_147_483_647] {
        let accepted = emulator
            .put_table_record_expiration_configuration(
                "us-east-1",
                &table_arn,
                &json!({ "status": "enabled", "settings": { "days": days } }),
            )
            .await;
        assert_eq!(
            accepted.status, 204,
            "case \"days={days}\": the modeled bound must be accepted, got {status} {body}",
            status = accepted.status,
            body = accepted.body
        );
        assert_eq!(
            reported_configuration(
                &emulator
                    .get_table_record_expiration_configuration("us-east-1", &table_arn)
                    .await,
                "the modeled bound"
            ),
            (Some("enabled".to_string()), Some(i64::from(days))),
            "case \"days={days}\": the read must report the stored retention"
        );
    }
}

#[tokio::test]
async fn a_call_against_a_table_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // One resident table, so the refusals below are about the ARN they name and
    // not about an empty store.
    let resident = create_table(
        &emulator,
        "us-east-1",
        "expiration-resident-bucket",
        "resident_table",
    )
    .await;

    let doomed = create_table(
        &emulator,
        "us-east-1",
        "expiration-doomed-bucket",
        "doomed_table",
    )
    .await;
    let deleted = emulator
        .delete_table(
            "us-east-1",
            "arn:aws:s3tables:us-east-1:000000000000:bucket/expiration-doomed-bucket",
            "doomed_table_ns",
            "doomed_table",
            &[],
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "case \"a deleted table\": the delete must succeed, got {status} {body}",
        status = deleted.status,
        body = deleted.body
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "a table that was never created",
            format!("{}/table/0e8d9f0c-7a6b-4c2d-9e1f-2a3b4c5d6e7f", {
                "arn:aws:s3tables:us-east-1:000000000000:bucket/expiration-resident-bucket"
            }),
        ),
        ("a table that was deleted", doomed.clone()),
        (
            "the resident table's id in another account",
            resident.replace("000000000000", "123456789012"),
        ),
    ];

    for (label, table_arn) in &cases {
        for (operation, reply) in [
            (
                "PutTableRecordExpirationConfiguration",
                emulator
                    .put_table_record_expiration_configuration(
                        "us-east-1",
                        table_arn,
                        &json!({ "status": "enabled", "settings": { "days": 30 } }),
                    )
                    .await,
            ),
            (
                "GetTableRecordExpirationConfiguration",
                emulator
                    .get_table_record_expiration_configuration("us-east-1", table_arn)
                    .await,
            ),
            (
                "GetTableRecordExpirationJobStatus",
                emulator
                    .get_table_record_expiration_job_status("us-east-1", table_arn)
                    .await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {label:?} on {operation}: expected 404, got {status} {body}",
                status = reply.status,
                body = reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("NotFoundException"),
                "case {label:?} on {operation}: expected NotFoundException, got {actual:?}",
                actual = reply.error_code
            );
            assert_eq!(
                reply.message(),
                NO_TABLE,
                "case {label:?} on {operation}: expected the fixed not-found sentence, got \
                 {actual:?}",
                actual = reply.message()
            );
        }
    }

    // The resident table still answers the eligibility refusal, so the
    // not-found refusals above are about the ARN each named.
    assert_not_allowed(
        &emulator
            .get_table_record_expiration_configuration("us-east-1", &resident)
            .await,
        "the resident table",
        "GetTableRecordExpirationConfiguration",
    );
}
