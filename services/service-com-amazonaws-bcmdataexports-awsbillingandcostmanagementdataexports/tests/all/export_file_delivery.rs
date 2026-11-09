// SPDX-License-Identifier: CC0-1.0

//! The export FILE a delivery writes, over the awsJson1_1 wire with Amazon S3 beside this
//! service on one engine: `CreateExport` / `UpdateExport` -> the object in the destination
//! bucket -> `GetExecution` / `GetExport`.
//!
//! The rule under test: the service PERFORMS each delivery it owes an export — it runs the
//! definition's `DataQuery` and writes the result, in the format and compression the
//! definition's `S3OutputConfigurations` states, into the bucket and prefix its
//! `S3Destination` names — and the execution recording that delivery reports the outcome of
//! the write that really happened: the modelled `DELIVERY_SUCCESS` once the object is in the
//! bucket, with the export's `ExportStatus.LastRefreshedAt` ("The timestamp of when the
//! export was last generated") moved to the instant the delivery finished, and the modelled
//! `DELIVERY_FAILURE` carrying `INSUFFICIENT_PERMISSION` for a destination that would not
//! take it — which generates no file and so refreshes nothing.
//!
//! Why a client cares: a data export exists to put a file in a bucket. A client reads that
//! object with Amazon S3's own operations, so it has to be an object that service really
//! holds, in the encoding that was asked for, and the execution a client polls has to say
//! whether the file is there.
//!
//! It is asserted over a TABLE whose rows differ along what the rule generalises over: the
//! output encoding (every `Format`/`Compression` pair this service publishes), the
//! `Overwrite` disposition (both modelled values), the `S3Prefix` (nested, single-segment,
//! trailing-slash and empty), what the statement projects (one column, several, an alias, a
//! qualified reference, a wildcard) and which catalog table it reads — never along the
//! identity of a bucket. A second test walks every non-terminal status of the delivery
//! pipeline, and a third the destinations a delivery cannot be written to.
//!
//! Nothing here sleeps for a due instant: the instants are STORED on the export and on the
//! execution, so a test moves them to put a transition in the past — exactly the state the
//! service reads — instead of waiting.

use std::io::Read;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::OVERWRITE_OPTION_VALUES;
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    DELIVERY_ACTION, DELIVERY_INITIATION_DELAY, DELIVERY_PIPELINE, DELIVERY_SETTLES_AFTER,
    EXECUTION_STATUS_DELIVERY_FAILURE, EXECUTION_STATUS_DELIVERY_SUCCESS,
    EXECUTION_STATUS_QUERY_FAILURE, EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION,
    EXECUTION_STATUS_REASON_INTERNAL_FAILURE, MAX_DELIVERED_COLUMNS, OVERWRITE_REPORT,
    SUPPORTED_OUTPUT_ENCODINGS,
};

use crate::delivery_executions::reschedule_delivery;
use crate::delivery_grant::{
    grant, objects_of, policy, published_grant, source_condition, Deliveries,
};
use crate::export_lifecycle::{timestamp, PINNED_REGION};

/// The `OverwriteOption` under which each delivery is delivered IN ADDITION to the files
/// already there rather than replacing them: the other of the two modelled values
/// (smithy_query shape `OverwriteOption`), `OVERWRITE_REPORT` being the one this service
/// states.
const CREATE_NEW_REPORT: &str = "CREATE_NEW_REPORT";

/// The `S3OutputType` every row states: one of the modelled values (smithy_query shape
/// `S3OutputType`), and not what this suite varies.
const OUTPUT_TYPE: &str = "CUSTOM";

/// The part number one delivery's single file carries, which every key ends with.
const REPORT_PART: &str = "00001";

/// One row of the table.
struct Row {
    label: &'static str,
    /// The `QueryStatement` the definition carries.
    statement: &'static str,
    /// The header the delivered file must carry, or `None` for a wildcard projection, whose
    /// header is cross-checked against what `GetTable` reports as [`Row::table`]'s schema.
    header: Option<&'static [&'static str]>,
    /// The catalog table the statement reads.
    table: &'static str,
    /// `Format` and `Compression`, a pair this service publishes as one it writes.
    encoding: (&'static str, &'static str),
    /// The extension the written object's key ends with, for that encoding.
    extension: &'static str,
    /// The `Overwrite` disposition of the destination.
    overwrite: &'static str,
    /// The `S3Prefix` the export is delivered under.
    prefix: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one column, gzipped CSV, overwritten",
            statement: "SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT",
            header: Some(&["line_item_usage_account_id"]),
            table: "COST_AND_USAGE_REPORT",
            encoding: ("TEXT_OR_CSV", "GZIP"),
            extension: "csv.gz",
            overwrite: OVERWRITE_REPORT,
            prefix: "exports/daily",
        },
        Row {
            label: "several columns, gzipped CSV, a new report each delivery",
            statement: "SELECT bill_payer_account_id, line_item_unblended_cost FROM \
                        COST_AND_USAGE_REPORT WHERE line_item_usage_start_date > '2026-01-01'",
            header: Some(&["bill_payer_account_id", "line_item_unblended_cost"]),
            table: "COST_AND_USAGE_REPORT",
            encoding: ("TEXT_OR_CSV", "GZIP"),
            extension: "csv.gz",
            overwrite: CREATE_NEW_REPORT,
            prefix: "cur2",
        },
        // An ALIAS names the position the file carries, whatever the column behind it is.
        Row {
            label: "an aliased column, zipped CSV, a new report each delivery",
            statement: "SELECT line_item_unblended_cost AS total_cost FROM COST_AND_USAGE_REPORT",
            header: Some(&["total_cost"]),
            table: "COST_AND_USAGE_REPORT",
            encoding: ("TEXT_OR_CSV", "ZIP"),
            extension: "csv.zip",
            overwrite: CREATE_NEW_REPORT,
            prefix: "exports/zipped",
        },
        // A QUALIFIED reference carries the published column it resolves to, and the prefix
        // is empty — `S3Prefix` is `@length 0..1024`, so a definition may name none.
        Row {
            label: "a qualified reference, zipped CSV, no prefix at all",
            statement: "SELECT COST_AND_USAGE_REPORT.identity_line_item_id FROM \
                        COST_AND_USAGE_REPORT",
            header: Some(&["identity_line_item_id"]),
            table: "COST_AND_USAGE_REPORT",
            encoding: ("TEXT_OR_CSV", "ZIP"),
            extension: "csv.zip",
            overwrite: OVERWRITE_REPORT,
            prefix: "",
        },
        // A WILDCARD over another table: the file carries that table's published schema,
        // which `GetTable` reports independently of this delivery. The prefix is written
        // with a trailing slash, which names the same place as one without it.
        Row {
            label: "a wildcard over a second table, Parquet, overwritten",
            statement: "SELECT * FROM FOCUS_1_0_AWS",
            header: None,
            table: "FOCUS_1_0_AWS",
            encoding: ("PARQUET", "PARQUET"),
            extension: "snappy.parquet",
            overwrite: OVERWRITE_REPORT,
            prefix: "focus/",
        },
        // An unaliased EXPRESSION is a position the statement names by writing it, which is
        // what the delivered header calls it — the definition is accepted because the
        // column reference inside the expression resolves, so it has to be deliverable.
        Row {
            label: "an unaliased expression, gzipped CSV, overwritten",
            statement: "SELECT SUM(line_item_unblended_cost) FROM COST_AND_USAGE_REPORT",
            header: Some(&["SUM(line_item_unblended_cost)"]),
            table: "COST_AND_USAGE_REPORT",
            encoding: ("TEXT_OR_CSV", "GZIP"),
            extension: "csv.gz",
            overwrite: OVERWRITE_REPORT,
            prefix: "aggregates",
        },
        Row {
            label: "two columns of a second table, Parquet, a new report each delivery",
            statement: "SELECT BilledCost, ChargePeriodStart FROM FOCUS_1_0_AWS",
            header: Some(&["BilledCost", "ChargePeriodStart"]),
            table: "FOCUS_1_0_AWS",
            encoding: ("PARQUET", "PARQUET"),
            extension: "snappy.parquet",
            overwrite: CREATE_NEW_REPORT,
            prefix: "exports/parquet",
        },
    ]
}

/// The `CreateExport` request of one row, delivered to `bucket`.
fn create_request(row: &Row, name: &str, bucket: &str) -> Value {
    let (format, compression) = row.encoding;
    json!({
        "Export": {
            "Name": name,
            "DataQuery": { "QueryStatement": row.statement },
            "DestinationConfigurations": {
                "S3Destination": {
                    "S3Bucket": bucket,
                    "S3Prefix": row.prefix,
                    "S3Region": PINNED_REGION,
                    "S3OutputConfigurations": {
                        "OutputType": OUTPUT_TYPE,
                        "Format": format,
                        "Compression": compression,
                        "Overwrite": row.overwrite,
                    },
                },
            },
            "RefreshCadence": { "Frequency": "SYNCHRONOUS" },
        },
    })
}

/// The `UpdateExport` request that restates one row's definition, which owes the export its
/// NEXT delivery.
///
/// "All export parameters must be specified in the UpdateExport request", so the
/// replacement restates the table configuration of the table its statement reads.
fn update_request(row: &Row, name: &str, bucket: &str, arn: &str) -> Value {
    let mut request = create_request(row, name, bucket);
    let object = request.as_object_mut().expect("the request is an object");
    object.insert("ExportArn".to_string(), json!(arn));
    object["Export"]["DataQuery"]
        .as_object_mut()
        .expect("DataQuery is an object")
        .insert(
            "TableConfigurations".to_string(),
            json!({ row.table: {} }),
        );
    request
}

/// Make the delivery the service owes `export_arn` fall due IN THE PAST — initiated, and its
/// terminal status due — so the next request performs it, and answer the instant it is due
/// to finish at.
fn settle_delivery(emulator: &Deliveries, export_arn: &str) -> SystemTime {
    let now = SystemTime::now();
    let initiate_at = now - Duration::from_secs(2);
    let complete_at = now - Duration::from_secs(1);
    reschedule_delivery(emulator.engine(), export_arn, initiate_at, complete_at);
    complete_at
}

/// Every execution `export_arn` has accrued, as `(id, ExecutionStatus)` pairs read through
/// `ListExecutions` and `GetExecution` — the two lookups a polling client uses.
async fn executions(case: &str, emulator: &Deliveries, export_arn: &str) -> Vec<(String, Value)> {
    let listed = emulator
        .ok("ListExecutions", &json!({ "ExportArn": export_arn }))
        .await;
    let references = listed["Executions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListExecutions reports Executions: {listed}"))
        .clone();
    let mut recorded: Vec<(String, Value)> = Vec::new();
    for reference in &references {
        let id = reference["ExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a reference carries its ExecutionId"))
            .to_string();
        let described = emulator
            .ok(
                "GetExecution",
                &json!({ "ExportArn": export_arn, "ExecutionId": &id }),
            )
            .await;
        recorded.push((id, described["ExecutionStatus"].clone()));
    }
    recorded
}

/// The ONE execution `export_arn` has accrued.
async fn only_execution(case: &str, emulator: &Deliveries, export_arn: &str) -> (String, Value) {
    let recorded = executions(case, emulator, export_arn).await;
    assert_eq!(
        recorded.len(),
        1,
        "case {case:?}: expected exactly the one delivery the service owed, got {recorded:?}"
    );
    recorded.into_iter().next().expect("one execution")
}

/// The keys the destination bucket holds under `prefix`, read through Amazon S3's own
/// `ListObjectsV2` — so what this asserts on is what a client listing the bucket sees.
async fn listed_keys(emulator: &Deliveries, bucket: &str, prefix: &str) -> Vec<String> {
    let (status, body) = emulator
        .s3_request(
            "GET",
            &format!("/{bucket}?list-type=2&prefix={}", prefix.trim_matches('/')),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "bucket {bucket:?}: expected ListObjectsV2 200, got {status}"
    );
    let listing = String::from_utf8_lossy(&body).to_string();
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split("</Key>").next())
        .map(str::to_string)
        .collect()
}

/// The bytes Amazon S3 serves for `key`, through its own `GetObject`.
async fn object_body(case: &str, emulator: &Deliveries, bucket: &str, key: &str) -> Vec<u8> {
    let (status, body) = emulator
        .s3_request("GET", &format!("/{bucket}/{key}"), &[])
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetObject 200 for {key:?}, got {status}"
    );
    assert!(
        !body.is_empty(),
        "case {case:?}: the delivered object {key:?} must carry bytes"
    );
    body
}

/// The header row and the number of DATA rows the delivered file holds, decoded with the
/// maintained implementation of the encoding it was delivered in — so what is asserted is
/// that a real GZIP stream, ZIP archive or Parquet file landed in the bucket.
fn decode_export_file(case: &str, row: &Row, name: &str, body: &[u8]) -> (Vec<String>, usize) {
    match row.encoding {
        ("TEXT_OR_CSV", "GZIP") => {
            let mut text = String::new();
            ::flate2::read::GzDecoder::new(body)
                .read_to_string(&mut text)
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: the object must be a GZIP stream, got {error}")
                });
            delimited(case, &text)
        }
        ("TEXT_OR_CSV", "ZIP") => {
            let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(body.to_vec()))
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: the object must be a ZIP archive, got {error}")
                });
            assert_eq!(
                archive.len(),
                1,
                "case {case:?}: a delivery writes one file, so the archive holds one entry"
            );
            let mut entry = archive
                .by_index(0)
                .unwrap_or_else(|error| panic!("case {case:?}: the entry opens, got {error}"));
            let entry_name = entry
                .name()
                .unwrap_or_else(|error| panic!("case {case:?}: the entry is named, got {error}"))
                .to_string();
            assert_eq!(
                entry_name,
                format!("{name}-{REPORT_PART}.csv"),
                "case {case:?}: the archived file is the export's own CSV"
            );
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .unwrap_or_else(|error| panic!("case {case:?}: the entry reads, got {error}"));
            delimited(case, &text)
        }
        ("PARQUET", "PARQUET") => {
            use ::parquet::file::reader::FileReader;
            let reader = ::parquet::file::reader::SerializedFileReader::new(
                ::bytes::Bytes::copy_from_slice(body),
            )
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the object must be a Parquet file, got {error}")
            });
            let metadata = reader.metadata().file_metadata();
            let columns = metadata
                .schema_descr()
                .columns()
                .iter()
                .map(|column| column.name().to_string())
                .collect();
            let rows = usize::try_from(metadata.num_rows().max(0)).unwrap_or_default();
            (columns, rows)
        }
        other => panic!("case {case:?}: no decoder for the encoding {other:?}"),
    }
}

/// The header and the data-row count of delimited text, read with the `csv` crate.
fn delimited(case: &str, text: &str) -> (Vec<String>, usize) {
    let mut reader = ::csv::Reader::from_reader(text.as_bytes());
    let header = reader
        .headers()
        .unwrap_or_else(|error| panic!("case {case:?}: the file carries a header, got {error}"))
        .iter()
        .map(str::to_string)
        .collect();
    let rows = reader.records().count();
    (header, rows)
}

/// What `GetTable` reports as `table`'s schema, by column name: the published schema a
/// wildcard projection delivers, read through the operation that describes it rather than
/// restated here.
async fn published_schema(emulator: &Deliveries, table: &str) -> Vec<String> {
    let described = emulator
        .ok("GetTable", &json!({ "TableName": table }))
        .await;
    described["Schema"]
        .as_array()
        .unwrap_or_else(|| panic!("GetTable reports a Schema for {table:?}: {described}"))
        .iter()
        .filter_map(|column| column["Name"].as_str().map(str::to_string))
        .collect()
}

/// Assert `key` is the key a delivery of `row` writes: the configured prefix, the export's
/// name, the billing-period partition of the data, and the part file — carrying the
/// delivery's own id exactly when the definition asks for a new report each time.
fn assert_delivery_key(case: &str, row: &Row, name: &str, execution_id: &str, key: &str) {
    let expected_prefix = match row.prefix.trim_matches('/') {
        "" => format!("{name}/data/BILLING_PERIOD="),
        prefix => format!("{prefix}/{name}/data/BILLING_PERIOD="),
    };
    assert!(
        key.starts_with(&expected_prefix),
        "case {case:?}: expected a key under {expected_prefix:?}, got {key:?}"
    );
    let period: String = key[expected_prefix.len()..]
        .chars()
        .take_while(|character| *character != '/')
        .collect();
    assert!(
        period.len() == 7
            && period.chars().enumerate().all(|(index, character)| match index {
                4 => character == '-',
                _ => character.is_ascii_digit(),
            }),
        "case {case:?}: expected a YYYY-MM billing period in {key:?}, got {period:?}"
    );
    assert!(
        key.ends_with(&format!("-{REPORT_PART}.{}", row.extension)),
        "case {case:?}: expected a {} part file, got {key:?}",
        row.extension
    );
    assert_eq!(
        key.contains(execution_id),
        row.overwrite == CREATE_NEW_REPORT,
        "case {case:?}: a {} delivery must name the delivery in its key only when it is \
         delivered in addition to the previous files, got {key:?}",
        row.overwrite
    );
}

/// RULE: a due delivery runs the export's query and writes the file into the destination,
/// the execution reports `DELIVERY_SUCCESS`, and the export reports when it was generated.
#[tokio::test]
async fn a_due_delivery_writes_the_export_file_to_the_destination() {
    // Every encoding this service publishes as one it writes is a row below, so an encoding
    // added to that table is uncovered until a row states what it delivers.
    for (format, compressions) in SUPPORTED_OUTPUT_ENCODINGS {
        for compression in *compressions {
            assert!(
                rows()
                    .iter()
                    .any(|row| row.encoding == (*format, *compression)),
                "case {:?}: every published encoding must be delivered by a row",
                (format, compression)
            );
        }
    }
    // And both modelled `Overwrite` dispositions, read from the service's own generated
    // catalogue of that enum's values.
    for overwrite in OVERWRITE_OPTION_VALUES {
        assert!(
            rows().iter().any(|row| row.overwrite == *overwrite),
            "case {overwrite:?}: every modelled Overwrite must be delivered by a row"
        );
    }

    for (index, row) in rows().into_iter().enumerate() {
        let emulator = Deliveries::new();
        let bucket = format!("export-file-{index}");
        let name = format!("export_file_{index}");
        emulator
            .bucket_with(&bucket, Some(&published_grant(&bucket)))
            .await;

        let created = emulator
            .ok("CreateExport", &create_request(&row, &name, &bucket))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: CreateExport returns an ExportArn", row.label))
            .to_string();

        // BEFORE the delivery falls due nothing has been written: the span is never zero.
        assert_eq!(
            listed_keys(&emulator, &bucket, row.prefix).await,
            Vec::<String>::new(),
            "case {:?}: a delivery that is not due yet must have written nothing",
            row.label
        );

        // ONCE IT FALLS DUE the service performs it, with no caller call of its own.
        let due_at = settle_delivery(&emulator, &arn);
        let (execution_id, status) = only_execution(row.label, &emulator, &arn).await;
        assert_eq!(
            (
                status["StatusCode"].as_str(),
                status.get("StatusReason").and_then(Value::as_str),
            ),
            (Some(EXECUTION_STATUS_DELIVERY_SUCCESS), None),
            "case {:?}: a delivery whose file is in the bucket succeeded, got {status}",
            row.label
        );
        let completed_at = timestamp(row.label, &status, "CompletedAt").secs();
        assert_eq!(
            completed_at,
            epoch_second(due_at),
            "case {:?}: expected CompletedAt at the instant the delivery was due, got {status}",
            row.label
        );

        // The export reports that a file WAS generated, at that same instant.
        let export_status = emulator
            .ok("GetExport", &json!({ "ExportArn": &arn }))
            .await["ExportStatus"]
            .clone();
        assert_eq!(
            timestamp(row.label, &export_status, "LastRefreshedAt").secs(),
            completed_at,
            "case {:?}: expected LastRefreshedAt at the instant the export was generated, got \
             {export_status}",
            row.label
        );

        // THE OBJECT is in the bucket, under the configured prefix, and Amazon S3 serves it.
        let keys = listed_keys(&emulator, &bucket, row.prefix).await;
        assert_eq!(
            keys.len(),
            1,
            "case {:?}: one delivery writes one file, got {keys:?}",
            row.label
        );
        assert_delivery_key(row.label, &row, &name, &execution_id, &keys[0]);
        let first_key = keys[0].clone();
        let body = object_body(row.label, &emulator, &bucket, &keys[0]).await;
        let (header, data_rows) = decode_export_file(row.label, &row, &name, &body);
        let expected: Vec<String> = match row.header {
            Some(columns) => columns.iter().map(|column| (*column).to_string()).collect(),
            None => published_schema(&emulator, row.table).await,
        };
        assert_eq!(
            header, expected,
            "case {:?}: the file carries the columns the query projects, got {header:?}",
            row.label
        );
        // The rows are this account's cost and usage data, of which the emulator holds
        // none: the file carries its header and no line item.
        assert_eq!(
            data_rows, 0,
            "case {:?}: expected no data row, got {data_rows}",
            row.label
        );

        // THE NEXT DELIVERY, owed by an update of the same definition, follows the
        // `Overwrite` the definition states.
        emulator
            .ok("UpdateExport", &update_request(&row, &name, &bucket, &arn))
            .await;
        let second_due_at = settle_delivery(&emulator, &arn);
        let recorded = executions(row.label, &emulator, &arn).await;
        assert_eq!(
            recorded.len(),
            2,
            "case {:?}: each persisted definition is owed its own delivery, got {recorded:?}",
            row.label
        );
        let (second_id, second_status) = recorded
            .into_iter()
            .find(|(id, _)| *id != execution_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the second delivery is an execution of its own",
                    row.label
                )
            });
        assert_eq!(
            second_status["StatusCode"].as_str(),
            Some(EXECUTION_STATUS_DELIVERY_SUCCESS),
            "case {:?}: the second delivery must have been written too, got {second_status}",
            row.label
        );
        let keys = listed_keys(&emulator, &bucket, row.prefix).await;
        let expected_files = if row.overwrite == OVERWRITE_REPORT { 1 } else { 2 };
        assert_eq!(
            keys.len(),
            expected_files,
            "case {:?}: a {} destination must hold {expected_files} file(s) after two \
             deliveries, got {keys:?}",
            row.label,
            row.overwrite
        );
        if row.overwrite == OVERWRITE_REPORT {
            // One file, REPLACED: the key is the one the first delivery wrote.
            assert_eq!(
                keys[0], first_key,
                "case {:?}: an overwriting delivery replaces the previous file rather than \
                 adding one, got {keys:?}",
                row.label
            );
            assert_delivery_key(row.label, &row, &name, &second_id, &keys[0]);
        } else {
            // Two files: the first delivery's is still there and the second's is beside it.
            assert!(
                keys.contains(&first_key),
                "case {:?}: a new report is delivered in addition to the previous files, got \
                 {keys:?}",
                row.label
            );
            let second_key = keys
                .iter()
                .find(|key| key.contains(&second_id))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the second delivery's own file must be in the bucket, got \
                         {keys:?}",
                        row.label
                    )
                });
            assert_delivery_key(row.label, &row, &name, &second_id, second_key);
        }
        // And the export was generated again, at the later delivery's instant.
        let refreshed = emulator
            .ok("GetExport", &json!({ "ExportArn": &arn }))
            .await["ExportStatus"]
            .clone();
        assert_eq!(
            timestamp(row.label, &refreshed, "LastRefreshedAt").secs(),
            epoch_second(second_due_at),
            "case {:?}: expected LastRefreshedAt at the latest generation, got {refreshed}",
            row.label
        );
    }
}

/// `at` as the whole epoch second this service reports every instant at.
fn epoch_second(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs() as i64
}

/// RULE: a delivery that has not finished is in the pipeline status the clock has moved it
/// to — every non-terminal `ExecutionStatusCode` of the delivery is reachable, in order,
/// across the span between the delivery's initiation and its terminal status.
#[tokio::test]
async fn a_delivery_reports_every_step_of_the_pipeline_it_is_in() {
    /// The span each step of the pipeline is held for in this test: long enough that a slow
    /// machine still reads the step the stored instants put the delivery in.
    const STEP: Duration = Duration::from_secs(10);

    for (index, expected) in DELIVERY_PIPELINE.iter().enumerate() {
        let emulator = Deliveries::new();
        let bucket = format!("export-pipeline-{index}");
        let name = format!("export_pipeline_{index}");
        emulator
            .bucket_with(&bucket, Some(&published_grant(&bucket)))
            .await;
        let table = rows();
        let row = &table[0];
        let created = emulator
            .ok("CreateExport", &create_request(row, &name, &bucket))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .expect("CreateExport returns an ExportArn")
            .to_string();

        // The delivery was initiated `index` steps ago and finishes `DELIVERY_PIPELINE.len()`
        // steps after it started, so the step in force is exactly the `index`-th.
        let now = SystemTime::now();
        let initiate_at = now - STEP * u32::try_from(index).expect("a small index");
        let complete_at =
            initiate_at + STEP * u32::try_from(DELIVERY_PIPELINE.len()).expect("a small count");
        reschedule_delivery(emulator.engine(), &arn, initiate_at, complete_at);

        let (_, status) = only_execution(expected, &emulator, &arn).await;
        assert_eq!(
            status["StatusCode"].as_str(),
            Some(*expected),
            "case {expected:?}: expected the pipeline step the clock moved the delivery to, \
             got {status}"
        );
        assert_eq!(
            status.get("CompletedAt"),
            None,
            "case {expected:?}: a delivery that has not finished reports no CompletedAt, got \
             {status}"
        );
        // And nothing has been written yet: the file is written when the delivery finishes.
        assert_eq!(
            listed_keys(&emulator, &bucket, row.prefix).await,
            Vec::<String>::new(),
            "case {expected:?}: a delivery still in the pipeline must have written nothing"
        );
    }
}

/// RULE, with the CLOCK and nothing else: the service delivers the export file on its own.
/// No call to THIS service is made after the create — the only calls are to Amazon S3, to
/// read the bucket — and the file is there, decodable, with the columns the query projects.
#[tokio::test]
async fn the_clock_alone_delivers_the_export_file() {
    /// The margin this test waits past the published spans, so a scheduler that runs the
    /// delivery a moment late is still observed.
    const CLOCK_MARGIN: Duration = Duration::from_millis(500);

    let emulator = Deliveries::new();
    let bucket = "export-file-clock-alone";
    let name = "export_file_clock_alone";
    emulator
        .bucket_with(bucket, Some(&published_grant(bucket)))
        .await;
    let table = rows();
    let row = &table[0];
    let created = emulator
        .ok("CreateExport", &create_request(row, name, bucket))
        .await;
    assert!(
        created["ExportArn"].is_string(),
        "CreateExport returns an ExportArn, got {created}"
    );

    ::tokio::time::sleep(DELIVERY_INITIATION_DELAY + DELIVERY_SETTLES_AFTER + CLOCK_MARGIN).await;

    /// How often the bucket is read while waiting for the delivery the clock owes.
    const LOOK_INTERVAL: Duration = Duration::from_millis(100);
    /// How many looks that is, which bounds the wait past the margin above at 120 s. The
    /// bound is patience, not a weakened expectation: the assertion below is unchanged, and
    /// a whole-workspace test run shares its cores with every other suite, so the task the
    /// clock owes can be scheduled far later than it is owed.
    const LOOKS: usize = 1_200;

    // The delivery is the CLOCK's, and a loaded machine can run it later than the margin
    // above: the bucket is read repeatedly, within a bound, rather than once after a fixed
    // sleep. The assertion below is unchanged — exactly one file, decodable, with the
    // columns the query projects.
    let mut keys = listed_keys(&emulator, bucket, row.prefix).await;
    for _ in 0..LOOKS {
        if !keys.is_empty() {
            break;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
        keys = listed_keys(&emulator, bucket, row.prefix).await;
    }
    assert_eq!(
        keys.len(),
        1,
        "case \"clock alone\": the service must have delivered the export file on its own, got \
         {keys:?}"
    );
    let body = object_body("clock alone", &emulator, bucket, &keys[0]).await;
    let (header, data_rows) = decode_export_file("clock alone", row, name, &body);
    let expected: Vec<String> = row
        .header
        .expect("the first row states its header")
        .iter()
        .map(|column| (*column).to_string())
        .collect();
    assert_eq!(
        header, expected,
        "case \"clock alone\": the delivered file carries the projected columns, got {header:?}"
    );
    assert_eq!(
        data_rows, 0,
        "case \"clock alone\": expected no data row, got {data_rows}"
    );
}

/// RULE, the bound on what one delivery writes: a select list that projects more columns
/// than an export file carries is not delivered — the delivery fails at the step that reads
/// the query, and nothing is written to the destination.
///
/// The number of wildcards is computed from the table's OWN published schema, so the row
/// states the bound rather than a count that happens to exceed today's catalog.
#[tokio::test]
async fn a_projection_past_the_delivered_column_bound_is_not_delivered() {
    let emulator = Deliveries::new();
    let bucket = "export-file-too-wide";
    let name = "export_file_too_wide";
    emulator
        .bucket_with(bucket, Some(&published_grant(bucket)))
        .await;
    let table = rows();
    let row = &table[0];

    // One wildcard projects the table's whole published schema, so this many of them
    // project more than a file carries.
    let published = published_schema(&emulator, row.table).await;
    assert!(
        !published.is_empty(),
        "case \"too wide\": the queried table publishes a schema"
    );
    let wildcards = MAX_DELIVERED_COLUMNS / published.len() + 1;
    let projection = vec!["*"; wildcards].join(", ");
    let mut request = create_request(row, name, bucket);
    request["Export"]["DataQuery"]["QueryStatement"] =
        json!(format!("SELECT {projection} FROM {}", row.table));
    let created = emulator.ok("CreateExport", &request).await;
    let arn = created["ExportArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"too wide\": CreateExport returns an ExportArn: {created}"))
        .to_string();

    settle_delivery(&emulator, &arn);
    let (_, status) = only_execution("too wide", &emulator, &arn).await;
    assert_eq!(
        (
            status["StatusCode"].as_str(),
            status["StatusReason"].as_str(),
        ),
        (
            Some(EXECUTION_STATUS_QUERY_FAILURE),
            Some(EXECUTION_STATUS_REASON_INTERNAL_FAILURE),
        ),
        "case \"too wide\": a projection past the bound fails where the query is read, got \
         {status}"
    );
    assert_eq!(
        listed_keys(&emulator, bucket, row.prefix).await,
        Vec::<String>::new(),
        "case \"too wide\": a delivery that did not run must write no object"
    );
    let export_status = emulator
        .ok("GetExport", &json!({ "ExportArn": &arn }))
        .await["ExportStatus"]
        .clone();
    assert_eq!(
        export_status.get("LastRefreshedAt"),
        None,
        "case \"too wide\": no file was generated, so LastRefreshedAt must stay absent, got \
         {export_status}"
    );
}

/// RULE, the other half: a destination this service cannot write to fails the delivery with
/// the modelled reason, writes no object, and leaves the export reporting no generation.
///
/// The rows differ along WHY the write cannot be made: a destination the object store held
/// when the definition was accepted and does not hold any more, and one whose policy granted
/// the delivery then and does not grant it now. Both are changes made AFTER the create — a
/// destination the object store already reported absent is refused by the create itself (see
/// `tests/all/referenced_resource_resolution.rs`), so it never reaches a delivery.
#[tokio::test]
async fn a_destination_that_refuses_the_write_fails_the_delivery() {
    for (index, (case, remove_bucket, revoke)) in [
        ("a destination the object store no longer holds", true, false),
        ("a destination whose grant was taken away", false, true),
    ]
    .into_iter()
    .enumerate()
    {
        let emulator = Deliveries::new();
        let bucket = format!("export-refused-{index}");
        let name = format!("export_refused_{index}");
        emulator
            .bucket_with(&bucket, Some(&published_grant(&bucket)))
            .await;
        let table = rows();
        let row = &table[0];
        let created = emulator
            .ok("CreateExport", &create_request(row, &name, &bucket))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateExport returns an ExportArn"))
            .to_string();
        if remove_bucket {
            // `DELETE /<bucket>` — the destination is taken away through Amazon S3's own
            // `DeleteBucket`, so the delivery finds what a client would have left it.
            let (status, body) = emulator.s3_request("DELETE", &format!("/{bucket}"), &[]).await;
            assert_eq!(
                status,
                204,
                "case {case:?}: expected DeleteBucket 204, got {status} with {}",
                String::from_utf8_lossy(&body)
            );
        }
        if revoke {
            // The same document, granting the write to another service's principal: this
            // one is no longer permitted to deliver.
            emulator
                .put_bucket_policy(
                    &bucket,
                    &policy(grant(
                        json!({ "Service": ["cur.amazonaws.com"] }),
                        json!([DELIVERY_ACTION]),
                        json!([objects_of(&bucket)]),
                        Some(source_condition(PINNED_REGION)),
                    )),
                )
                .await;
        }

        settle_delivery(&emulator, &arn);
        let (_, status) = only_execution(case, &emulator, &arn).await;
        assert_eq!(
            (
                status["StatusCode"].as_str(),
                status["StatusReason"].as_str(),
            ),
            (
                Some(EXECUTION_STATUS_DELIVERY_FAILURE),
                Some(EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION),
            ),
            "case {case:?}: a destination that would not take the file fails the delivery, got \
             {status}"
        );
        assert!(
            status.get("CompletedAt").is_some(),
            "case {case:?}: a delivery that ended reports when it ended, got {status}"
        );
        let export_status = emulator
            .ok("GetExport", &json!({ "ExportArn": &arn }))
            .await["ExportStatus"]
            .clone();
        assert_eq!(
            export_status.get("LastRefreshedAt"),
            None,
            "case {case:?}: no file was generated, so LastRefreshedAt must stay absent, got \
             {export_status}"
        );
        if !remove_bucket {
            assert_eq!(
                listed_keys(&emulator, &bucket, row.prefix).await,
                Vec::<String>::new(),
                "case {case:?}: a refused delivery must write no object"
            );
        }
    }
}
