// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `GetTableRecordExpirationConfiguration` as a
//! REPORT on one table.
//!
//! The rule under test: the read answers for exactly the table its `tableArn`
//! addresses — the record-expiration configuration that table's own record
//! holds, else the state a table nothing was configured on is in (records kept,
//! so expiration off) — and it reports it without changing anything a later
//! caller sees. A table that takes no record-expiration configuration at all is
//! refused instead, and that too is decided per table.
//!
//! `table_record_expiration.rs` covers the put/read round trip and the refusal
//! on one table at a time; this suite is about WHICH table a read answers for,
//! so its rows are tables that coexist: siblings in one namespace, configured
//! and unconfigured, eligible and not, and the same table name in a second
//! bucket in another region. The rows differ along the configuration's status,
//! its `days` (both modeled `@range(1, 2147483647)` bounds and a value between
//! them), a configuration with no status at all, the table's eligibility and the
//! region.

use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::{OpenTableFormat, RecordExpiration};

use super::harness::{Emulator, Reply};
use super::table_record_expiration::{mark_managed, stored_expiration};

/// The fixed sentence this service states a record-expiration call on a table
/// that takes no record-expiration configuration with.
const NOT_ALLOWED: &str = "Table record expiration configuration API calls to this table are not \
                           allowed";

/// The namespace every row's table is created in, one per bucket.
const NAMESPACE: &str = "records";

/// What the read must answer for a row's table.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// The 200 report, as the `status` and `settings.days` members it carries.
    Reported(Option<&'static str>, Option<i64>),
    /// The 405 refusal: this table takes no record-expiration configuration.
    NotAllowed,
}

/// One row: a table that exists alongside every other row's table, the
/// configuration (if any) put on it, and what the read must answer for it.
#[derive(Debug)]
struct Row {
    label: &'static str,
    region: &'static str,
    /// The bucket the table lives in; rows sharing a name share a bucket.
    bucket: &'static str,
    /// The table's name, which two rows in different buckets deliberately share.
    table: &'static str,
    /// Whether the table is one Amazon Web Services manages, the only kind
    /// record expiration applies to.
    managed: bool,
    /// The `value.status` a put carries, when the row puts a configuration.
    status: Option<&'static str>,
    /// The `value.settings.days` a put carries, when the row puts settings.
    days: Option<i32>,
    /// Whether the row puts a configuration at all.
    configured: bool,
    expected: Expected,
}

impl Row {
    /// The `value` member of this row's `PutTableRecordExpirationConfiguration`
    /// request.
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
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a managed table enabled for seven days",
            region: "us-east-1",
            bucket: "expiration-reads-east",
            table: "sales",
            managed: true,
            status: Some("enabled"),
            days: Some(7),
            configured: true,
            expected: Expected::Reported(Some("enabled"), Some(7)),
        },
        Row {
            label: "a managed sibling in the same namespace, expiration off",
            region: "us-east-1",
            bucket: "expiration-reads-east",
            table: "orders",
            managed: true,
            status: Some("disabled"),
            days: None,
            configured: true,
            expected: Expected::Reported(Some("disabled"), None),
        },
        Row {
            label: "a managed sibling nothing was configured on",
            region: "us-east-1",
            bucket: "expiration-reads-east",
            table: "returns",
            managed: true,
            status: None,
            days: None,
            configured: false,
            expected: Expected::Reported(Some("disabled"), None),
        },
        Row {
            label: "a customer sibling in the same namespace",
            region: "us-east-1",
            bucket: "expiration-reads-east",
            table: "ledger",
            managed: false,
            status: Some("enabled"),
            days: Some(5),
            configured: true,
            expected: Expected::NotAllowed,
        },
        Row {
            // `PositiveInteger` is `@range(min: 1)`: the shortest retention a
            // configuration can name.
            label: "a managed table in another region holding the shortest retention",
            region: "eu-west-1",
            bucket: "expiration-reads-west",
            table: "sales",
            managed: true,
            status: Some("enabled"),
            days: Some(1),
            configured: true,
            expected: Expected::Reported(Some("enabled"), Some(1)),
        },
        Row {
            // `PositiveInteger` is `@range(max: 2147483647)`: the longest
            // retention a configuration can name.
            label: "a managed table holding settings alone and the longest retention",
            region: "ap-south-1",
            bucket: "expiration-reads-south",
            table: "metrics",
            managed: true,
            status: None,
            days: Some(2_147_483_647),
            configured: true,
            expected: Expected::Reported(None, Some(2_147_483_647)),
        },
    ]
}

/// Create the bucket `name` with the one namespace this suite's tables live in,
/// and answer the bucket's ARN.
async fn create_bucket(emulator: &Emulator, region: &str, name: &str) -> String {
    let created = emulator
        .create_table_bucket(region, &json!({ "name": name }))
        .await;
    assert_eq!(
        created.status, 200,
        "creating the bucket {name:?} must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let bucket_arn = created.text("arn");
    let namespace = emulator
        .create_namespace(region, &bucket_arn, &json!({ "namespace": [NAMESPACE] }))
        .await;
    assert_eq!(
        namespace.status, 200,
        "creating the namespace in {name:?} must succeed, got {status} {body}",
        status = namespace.status,
        body = namespace.body
    );
    bucket_arn
}

/// Create one table in `bucket_arn`'s namespace and answer its ARN.
async fn create_table(emulator: &Emulator, region: &str, bucket_arn: &str, name: &str) -> String {
    let created = emulator
        .create_table(
            region,
            bucket_arn,
            NAMESPACE,
            &json!({ "name": name, "format": OpenTableFormat::Iceberg.as_wire() }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the table {name:?} must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    created.text("tableARN")
}

/// The `status` and `settings.days` members a read reported, or the refusal it
/// answered with.
fn answered(reply: &Reply, label: &str) -> Expected {
    if reply.status == 405 {
        assert_eq!(
            reply.error_code.as_deref(),
            Some("MethodNotAllowedException"),
            "case {label:?}: expected MethodNotAllowedException, got {actual:?}",
            actual = reply.error_code
        );
        assert_eq!(
            reply.message(),
            NOT_ALLOWED,
            "case {label:?}: expected the fixed not-allowed sentence, got {actual:?}",
            actual = reply.message()
        );
        return Expected::NotAllowed;
    }
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected the modeled 200, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    // `configuration` is a required output member, so a success always carries
    // it whatever the table's stored state is.
    let configuration = reply.body.get("configuration").unwrap_or_else(|| {
        panic!(
            "case {label:?}: the read carries no configuration: {body}",
            body = reply.body
        )
    });
    let status = configuration.get("status").and_then(Value::as_str);
    let days = configuration
        .get("settings")
        .and_then(|settings| settings.get("days"))
        .and_then(Value::as_i64);
    // A row's expectation names a `&'static str`, so compare by the wire text
    // rather than allocating one.
    match status {
        Some("enabled") => Expected::Reported(Some("enabled"), days),
        Some("disabled") => Expected::Reported(Some("disabled"), days),
        None => Expected::Reported(None, days),
        Some(other) => panic!(
            "case {label:?}: `status` must be one of the modeled values, got {other:?}: {body}",
            body = reply.body
        ),
    }
}

/// Every row's table, created and configured, as their ARNs in row order.
async fn planted(emulator: &Emulator, rows: &[Row]) -> Vec<String> {
    let mut buckets: Vec<(&str, String)> = Vec::new();
    let mut arns = Vec::new();
    for row in rows {
        let bucket_arn = match buckets.iter().find(|(name, _)| *name == row.bucket) {
            Some((_, arn)) => arn.clone(),
            None => {
                let arn = create_bucket(emulator, row.region, row.bucket).await;
                buckets.push((row.bucket, arn.clone()));
                arn
            }
        };
        let table_arn = create_table(emulator, row.region, &bucket_arn, row.table).await;
        // Every table this API creates is customer-created, so an AWS-managed
        // row is marked in the store directly; every call below travels the
        // real wire path.
        if row.managed {
            mark_managed(emulator, &table_arn);
        }
        if row.configured {
            let put = emulator
                .put_table_record_expiration_configuration(row.region, &table_arn, &row.value())
                .await;
            let expected_status = if row.managed { 204 } else { 405 };
            assert_eq!(
                put.status, expected_status,
                "case {label:?}: the put must answer {expected_status}, got {status} {body}",
                label = row.label,
                status = put.status,
                body = put.body
            );
        }
        arns.push(table_arn);
    }
    arns
}

#[tokio::test]
async fn a_read_reports_the_configuration_of_the_table_its_arn_addresses() {
    let emulator = Emulator::new();
    let rows = rows();
    let arns = planted(&emulator, &rows).await;

    for (row, table_arn) in rows.iter().zip(&arns) {
        let reply = emulator
            .get_table_record_expiration_configuration(row.region, table_arn)
            .await;
        assert_eq!(
            answered(&reply, row.label),
            row.expected,
            "case {label:?}: expected {expected:?}, got {body}",
            label = row.label,
            expected = row.expected,
            body = reply.body
        );
    }
}

#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let rows = rows();
    let arns = planted(&emulator, &rows).await;

    let before: Vec<Option<RecordExpiration>> = arns
        .iter()
        .map(|arn| stored_expiration(&emulator, arn))
        .collect();

    // Reading every table twice over, in a different order the second time, so
    // a read that moved a status or minted a configuration would show up.
    let mut first = Vec::new();
    for (row, table_arn) in rows.iter().zip(&arns) {
        first.push(answered(
            &emulator
                .get_table_record_expiration_configuration(row.region, table_arn)
                .await,
            row.label,
        ));
    }
    let mut second = Vec::new();
    for (row, table_arn) in rows.iter().zip(&arns).rev() {
        second.push(answered(
            &emulator
                .get_table_record_expiration_configuration(row.region, table_arn)
                .await,
            row.label,
        ));
    }
    second.reverse();
    assert_eq!(
        first, second,
        "a repeated read must answer the same for every table, got {first:?} then {second:?}"
    );

    let after: Vec<Option<RecordExpiration>> = arns
        .iter()
        .map(|arn| stored_expiration(&emulator, arn))
        .collect();
    assert_eq!(
        before, after,
        "reading must leave every table's stored configuration as it was, got {before:?} then \
         {after:?}"
    );

    // And the job-status read, which derives its answer from the same record,
    // still reports what that record says — no read moved it.
    for (row, table_arn) in rows.iter().zip(&arns) {
        let job = emulator
            .get_table_record_expiration_job_status(row.region, table_arn)
            .await;
        match row.expected {
            Expected::NotAllowed => assert_eq!(
                job.status, 405,
                "case {label:?}: the job status of a table that takes no expiration \
                 configuration must be refused, got {status} {body}",
                label = row.label,
                status = job.status,
                body = job.body
            ),
            Expected::Reported(status, _) => {
                let expected_job = if status == Some("enabled") {
                    "NotYetRun"
                } else {
                    "Disabled"
                };
                assert_eq!(
                    job.status, 200,
                    "case {label:?}: the job-status read must succeed, got {status} {body}",
                    label = row.label,
                    status = job.status,
                    body = job.body
                );
                assert_eq!(
                    job.text("status"),
                    expected_job,
                    "case {label:?}: expected job status {expected_job:?}, got {actual:?}",
                    label = row.label,
                    actual = job.text("status")
                );
            }
        }
    }
}
