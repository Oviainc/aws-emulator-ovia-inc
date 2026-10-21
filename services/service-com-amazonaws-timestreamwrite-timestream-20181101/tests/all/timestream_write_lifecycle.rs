// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Timestream Write database/table/ingest lifecycle, driven
//! over the service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: Timestream_20181101.<OperationName>`).
//!
//! Every assertion goes through the router, so what is checked is the HTTP behaviour a
//! client sees — not an internal call into the handler.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_timestreamwrite_timestream_20181101::ops::types::TABLE_STATUS_VALUES;
use service_com_amazonaws_timestreamwrite_timestream_20181101::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the model's `awsJson1_0` target prefix.
const TARGET_PREFIX: &str = "Timestream_20181101";

/// The account segment every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: a fresh engine behind a router that claims `timestream` requests.
struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    fn new() -> Self {
        Self::in_region("us-east-1")
    }

    /// Call one operation and return `(status, body)`. The `Authorization` header is
    /// what routes the request to this service and names the region its ARNs carry.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{}/timestream/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef",
            self.region
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "operation {operation} answered {status} with a non-JSON body \
                     ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one operation, requiring a 200, and return the body.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }
}

/// The `__type` discriminator of an awsJson error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

async fn create_database(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("CreateDatabase", &json!({ "DatabaseName": name }))
        .await
}

async fn create_table(emulator: &Emulator, database: &str, table: &str) -> Value {
    emulator
        .ok(
            "CreateTable",
            &json!({ "DatabaseName": database, "TableName": table }),
        )
        .await
}

/// A record the model accepts: a scalar DOUBLE measure at a millisecond timestamp.
fn sample_record(index: usize) -> Value {
    json!({
        "MeasureName": "cpu",
        "MeasureValue": format!("{}.0", 40 + index),
        "MeasureValueType": "DOUBLE",
        "Time": format!("{}", 1_700_000_000_000u64 + index as u64),
        "TimeUnit": "MILLISECONDS",
    })
}

// ---------------------------------------------------------------------------
// The lifecycle, end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_database_table_and_ingest_lifecycle_is_stateful() {
    let emulator = Emulator::new();

    // CreateDatabase returns what was stored.
    let created = create_database(&emulator, "demo_db").await;
    let database = &created["Database"];
    assert_eq!(
        database["DatabaseName"], "demo_db",
        "case CreateDatabase: expected DatabaseName demo_db, got {database:?}"
    );
    assert_eq!(
        database["Arn"],
        json!(format!("arn:aws:timestream:us-east-1:{ACCOUNT}:database/demo_db")),
        "case CreateDatabase: unexpected ARN, got {database:?}"
    );
    assert_eq!(
        database["TableCount"], 0,
        "case CreateDatabase: a new database has no tables, got {database:?}"
    );
    assert!(
        database["CreationTime"].is_number(),
        "case CreateDatabase: expected an epoch CreationTime, got {database:?}"
    );
    assert!(
        database["KmsKeyId"].as_str().is_some_and(|k| !k.is_empty()),
        "case CreateDatabase: expected a default KmsKeyId, got {database:?}"
    );

    // A second create of the same name conflicts.
    let (status, body) = emulator
        .call("CreateDatabase", &json!({ "DatabaseName": "demo_db" }))
        .await;
    assert_eq!(
        status, 409,
        "case CreateDatabase(duplicate): expected 409, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case CreateDatabase(duplicate): unexpected error, got {body:?}"
    );

    // CreateTable into a database that does not exist is a 404.
    let (status, body) = emulator
        .call(
            "CreateTable",
            &json!({ "DatabaseName": "missing_db", "TableName": "t" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case CreateTable(missing database): expected 404, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case CreateTable(missing database): unexpected error, got {body:?}"
    );

    // CreateTable stores the table and returns it.
    let created = emulator
        .ok(
            "CreateTable",
            &json!({
                "DatabaseName": "demo_db",
                "TableName": "demo_table",
                "RetentionProperties": {
                    "MemoryStoreRetentionPeriodInHours": 24,
                    "MagneticStoreRetentionPeriodInDays": 7,
                },
            }),
        )
        .await;
    let table = created["Table"].clone();
    assert_eq!(
        table["TableStatus"], "ACTIVE",
        "case CreateTable: expected TableStatus ACTIVE, got {table:?}"
    );
    // The status reported is one the model's `TableStatus` enum declares.
    assert!(
        TABLE_STATUS_VALUES.contains(&table["TableStatus"].as_str().unwrap_or_default()),
        "case CreateTable: TableStatus must be one of {TABLE_STATUS_VALUES:?}, got {table:?}"
    );
    assert_eq!(
        table["Arn"],
        json!(format!(
            "arn:aws:timestream:us-east-1:{ACCOUNT}:database/demo_db/table/demo_table"
        )),
        "case CreateTable: unexpected ARN, got {table:?}"
    );

    // A second create of the same table name conflicts.
    let (status, body) = emulator
        .call(
            "CreateTable",
            &json!({ "DatabaseName": "demo_db", "TableName": "demo_table" }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case CreateTable(duplicate): expected 409, got {status} with {body:?}"
    );

    // DescribeTable returns exactly what CreateTable stored.
    let described = emulator
        .ok(
            "DescribeTable",
            &json!({ "DatabaseName": "demo_db", "TableName": "demo_table" }),
        )
        .await;
    assert_eq!(
        described["Table"], table,
        "case DescribeTable: expected the stored table {table:?}, got {:?}",
        described["Table"]
    );

    // Creating a table bumps the owning database's derived TableCount.
    let described = emulator
        .ok("DescribeDatabase", &json!({ "DatabaseName": "demo_db" }))
        .await;
    assert_eq!(
        described["Database"]["TableCount"], 1,
        "case DescribeDatabase: expected TableCount 1, got {:?}",
        described["Database"]
    );

    // WriteRecords ingests into the stored table.
    let written = emulator
        .ok(
            "WriteRecords",
            &json!({
                "DatabaseName": "demo_db",
                "TableName": "demo_table",
                "CommonAttributes": { "Dimensions": [{ "Name": "host", "Value": "h1" }] },
                "Records": [sample_record(0), sample_record(1), sample_record(2)],
            }),
        )
        .await;
    assert_eq!(
        written["RecordsIngested"],
        json!({ "Total": 3, "MemoryStore": 3, "MagneticStore": 0 }),
        "case WriteRecords: expected 3 records in the memory store, got {written:?}"
    );

    // Deleting the table is observable by the next reader, and by DescribeDatabase.
    let (status, body) = emulator
        .call(
            "DeleteTable",
            &json!({ "DatabaseName": "demo_db", "TableName": "demo_table" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case DeleteTable: expected 200, got {status} with {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DescribeTable",
            &json!({ "DatabaseName": "demo_db", "TableName": "demo_table" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case DescribeTable(after delete): expected 404, got {status} with {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DeleteTable",
            &json!({ "DatabaseName": "demo_db", "TableName": "demo_table" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case DeleteTable(second): expected 404, got {status} with {body:?}"
    );
    let described = emulator
        .ok("DescribeDatabase", &json!({ "DatabaseName": "demo_db" }))
        .await;
    assert_eq!(
        described["Database"]["TableCount"], 0,
        "case DescribeDatabase(after DeleteTable): expected TableCount 0, got {:?}",
        described["Database"]
    );

    // And deleting the database is observable too.
    let (status, body) = emulator
        .call("DeleteDatabase", &json!({ "DatabaseName": "demo_db" }))
        .await;
    assert_eq!(
        status, 200,
        "case DeleteDatabase: expected 200, got {status} with {body:?}"
    );
    for operation in ["DescribeDatabase", "DeleteDatabase"] {
        let (status, body) = emulator
            .call(operation, &json!({ "DatabaseName": "demo_db" }))
            .await;
        assert_eq!(
            status, 404,
            "case {operation}(after DeleteDatabase): expected 404, got {status} with {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules asserted over a set of inputs
// ---------------------------------------------------------------------------

/// Every ARN the service mints carries the region the caller signed for, not a
/// baked-in one. The regions span three partitions' worth of prefixes.
#[tokio::test]
async fn arns_carry_the_region_the_request_was_signed_for() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        let database = create_database(&emulator, "regional_db").await;
        assert_eq!(
            database["Database"]["Arn"],
            json!(format!(
                "arn:aws:timestream:{region}:{ACCOUNT}:database/regional_db"
            )),
            "case region {region}: unexpected database ARN, got {database:?}"
        );

        let table = create_table(&emulator, "regional_db", "regional_table").await;
        assert_eq!(
            table["Table"]["Arn"],
            json!(format!(
                "arn:aws:timestream:{region}:{ACCOUNT}:database/regional_db/table/regional_table"
            )),
            "case region {region}: unexpected table ARN, got {table:?}"
        );
    }
}

/// `RetentionProperties` round-trip unchanged through CreateTable/DescribeTable for
/// every value the model admits, and absent properties take the service defaults
/// (6 hours memory / 73000 days magnetic).
///
/// The bounds come from the model's `RetentionProperties` members:
/// `MemoryStoreRetentionPeriodInHours` range 1..8766 and
/// `MagneticStoreRetentionPeriodInDays` range 1..73000.
#[tokio::test]
async fn retention_properties_round_trip_and_default_to_the_service_defaults() {
    let emulator = Emulator::new();
    create_database(&emulator, "retention_db").await;

    // (table name, requested properties, expected stored properties)
    struct RetentionCase {
        name: &'static str,
        requested: Option<(i64, i64)>,
        expected: (i64, i64),
    }
    let cases = [
        RetentionCase {
            name: "absent",
            requested: None,
            expected: (6, 73_000),
        },
        RetentionCase {
            name: "lower_bounds",
            requested: Some((1, 1)),
            expected: (1, 1),
        },
        RetentionCase {
            name: "upper_bounds",
            requested: Some((8766, 73_000)),
            expected: (8766, 73_000),
        },
        RetentionCase {
            name: "middle",
            requested: Some((24, 7)),
            expected: (24, 7),
        },
        RetentionCase {
            name: "asymmetric",
            requested: Some((8766, 1)),
            expected: (8766, 1),
        },
    ];

    for RetentionCase {
        name,
        requested,
        expected: (hours, days),
    } in cases
    {
        let mut request = json!({ "DatabaseName": "retention_db", "TableName": name });
        if let Some((request_hours, request_days)) = requested {
            request["RetentionProperties"] = json!({
                "MemoryStoreRetentionPeriodInHours": request_hours,
                "MagneticStoreRetentionPeriodInDays": request_days,
            });
        }
        let created = emulator.ok("CreateTable", &request).await;
        let expected = json!({
            "MemoryStoreRetentionPeriodInHours": hours,
            "MagneticStoreRetentionPeriodInDays": days,
        });
        assert_eq!(
            created["Table"]["RetentionProperties"], expected,
            "case {name}: expected {expected:?} from CreateTable, got {:?}",
            created["Table"]
        );

        let described = emulator
            .ok(
                "DescribeTable",
                &json!({ "DatabaseName": "retention_db", "TableName": name }),
            )
            .await;
        assert_eq!(
            described["Table"], created["Table"],
            "case {name}: DescribeTable must return the stored table {:?}, got {:?}",
            created["Table"], described["Table"]
        );
    }

    // One past each published bound is refused, so an out-of-range value can never
    // be stored.
    for (name, hours, days) in [
        ("hours_below", 0, 7),
        ("hours_above", 8767, 7),
        ("days_below", 24, 0),
        ("days_above", 24, 73_001),
    ] {
        let (status, body) = emulator
            .call(
                "CreateTable",
                &json!({
                    "DatabaseName": "retention_db",
                    "TableName": name,
                    "RetentionProperties": {
                        "MemoryStoreRetentionPeriodInHours": hours,
                        "MagneticStoreRetentionPeriodInDays": days,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name}: a retention period outside the model's range must be \
             refused, got {status} with {body:?}"
        );
        let (status, body) = emulator
            .call(
                "DescribeTable",
                &json!({ "DatabaseName": "retention_db", "TableName": name }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {name}: a refused CreateTable must store nothing, got {status} \
             with {body:?}"
        );
    }
}

/// `RecordsIngested` counts the records actually stored — for any accepted batch
/// size, with or without `CommonAttributes`, and accumulating across calls.
#[tokio::test]
async fn records_ingested_counts_what_was_stored() {
    let emulator = Emulator::new();
    create_database(&emulator, "ingest_db").await;

    // `Records` is bounded 1..100 by the model, so these span the whole admitted range.
    for batch in [1usize, 2, 17, 100] {
        for with_common in [false, true] {
            let table = format!("t_{batch}_{with_common}");
            create_table(&emulator, "ingest_db", &table).await;

            let records: Vec<Value> = (0..batch).map(sample_record).collect();
            let mut request = json!({
                "DatabaseName": "ingest_db",
                "TableName": table,
                "Records": records,
            });
            if with_common {
                request["CommonAttributes"] =
                    json!({ "Dimensions": [{ "Name": "host", "Value": "h1" }] });
            }

            let first = emulator.ok("WriteRecords", &request).await;
            let expected = json!({
                "Total": batch,
                "MemoryStore": batch,
                "MagneticStore": 0,
            });
            assert_eq!(
                first["RecordsIngested"], expected,
                "case batch {batch} common {with_common}: expected {expected:?}, \
                 got {first:?}"
            );

            // A second identical write ingests the same count again — the counts
            // report this request, not the table's running total.
            let second = emulator.ok("WriteRecords", &request).await;
            assert_eq!(
                second["RecordsIngested"], expected,
                "case batch {batch} common {with_common} (second write): expected \
                 {expected:?}, got {second:?}"
            );
        }
    }

    // An unknown database or table is a 404, and ingests nothing.
    for (database, table) in [("ingest_db", "nope"), ("nope", "t_1_false")] {
        let (status, body) = emulator
            .call(
                "WriteRecords",
                &json!({
                    "DatabaseName": database,
                    "TableName": table,
                    "Records": [sample_record(0)],
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {database}/{table}: expected 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {database}/{table}: unexpected error, got {body:?}"
        );
    }
}

/// The per-table record buffer is bounded, and the ingest counts stay exact past that
/// bound: `WriteRecords` is the only caller-driven growth site, so the table keeps a
/// bounded recent window instead of growing without limit, while every response still
/// reports the records that request accepted.
#[tokio::test]
async fn the_record_buffer_is_bounded_and_the_counts_stay_exact() {
    let emulator = Emulator::new();
    create_database(&emulator, "bounded_db").await;
    create_table(&emulator, "bounded_db", "bounded_table").await;

    // `Records` is bounded 1..100 per request, so this many requests takes the table
    // past RECORD_BUFFER_LIMIT (10_000) records with room to spare.
    let batch = 100usize;
    let requests = 105usize;
    let records: Vec<Value> = (0..batch).map(sample_record).collect();
    let request = json!({
        "DatabaseName": "bounded_db",
        "TableName": "bounded_table",
        "Records": records,
    });

    for round in 0..requests {
        let written = emulator.ok("WriteRecords", &request).await;
        assert_eq!(
            written["RecordsIngested"],
            json!({ "Total": batch, "MemoryStore": batch, "MagneticStore": 0 }),
            "case round {round}: expected {batch} records ingested, got {written:?}"
        );
    }

    // The table is still readable and unchanged in its description.
    let described = emulator
        .ok(
            "DescribeTable",
            &json!({ "DatabaseName": "bounded_db", "TableName": "bounded_table" }),
        )
        .await;
    assert_eq!(
        described["Table"]["TableStatus"], "ACTIVE",
        "case after {requests} writes: the table must still be ACTIVE, got {described:?}"
    );
}

/// A record whose `Version` is below 1 is refused: the model's own doc for
/// `Record$Version` says it "must be 1 or greater, or you will receive a
/// ValidationException error".
#[tokio::test]
async fn a_record_version_below_one_is_refused() {
    let emulator = Emulator::new();
    create_database(&emulator, "version_db").await;
    create_table(&emulator, "version_db", "version_table").await;

    for version in [-1i64, 0] {
        let mut record = sample_record(0);
        record["Version"] = json!(version);
        let (status, body) = emulator
            .call(
                "WriteRecords",
                &json!({
                    "DatabaseName": "version_db",
                    "TableName": "version_table",
                    "Records": [record],
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case Version {version}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case Version {version}: unexpected error, got {body:?}"
        );
    }

    // And 1 and above are accepted.
    for version in [1i64, 2, 9_999] {
        let mut record = sample_record(0);
        record["Version"] = json!(version);
        let written = emulator
            .ok(
                "WriteRecords",
                &json!({
                    "DatabaseName": "version_db",
                    "TableName": "version_table",
                    "Records": [record],
                }),
            )
            .await;
        assert_eq!(
            written["RecordsIngested"]["Total"], 1,
            "case Version {version}: expected one record ingested, got {written:?}"
        );
    }
}

/// Paging over `ListTables` terminates and yields every stored table exactly once,
/// for any page size the model admits and any number of tables.
#[tokio::test]
async fn list_tables_pages_to_exhaustion() {
    // (table count, MaxResults) — `MaxResults` is bounded 1..20 by the model.
    for table_count in [0usize, 1, 3, 25] {
        for max_results in [None, Some(1), Some(2), Some(20)] {
            let emulator = Emulator::new();
            create_database(&emulator, "page_db").await;
            let mut expected: Vec<String> = Vec::new();
            for index in 0..table_count {
                let name = format!("table_{index:02}");
                create_table(&emulator, "page_db", &name).await;
                expected.push(name);
            }
            expected.sort();

            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut request = json!({ "DatabaseName": "page_db" });
                if let Some(limit) = max_results {
                    request["MaxResults"] = json!(limit);
                }
                if let Some(token) = &next_token {
                    request["NextToken"] = json!(token);
                }
                let page = emulator.ok("ListTables", &request).await;
                let tables = page["Tables"].as_array().cloned().unwrap_or_default();
                let page_len = tables.len();
                assert!(
                    page_len <= max_results.unwrap_or(20) as usize,
                    "case {table_count} tables / {max_results:?}: a page of {page_len} \
                     exceeds the requested page size, got {page:?}"
                );
                for table in tables {
                    seen.push(
                        table["TableName"]
                            .as_str()
                            .expect("every listed table names itself")
                            .to_string(),
                    );
                }
                pages += 1;
                assert!(
                    pages <= table_count + 2,
                    "case {table_count} tables / {max_results:?}: paging did not \
                     terminate after {pages} pages, saw {seen:?}"
                );
                match page["NextToken"].as_str() {
                    Some(token) => next_token = Some(token.to_string()),
                    None => break,
                }
            }

            assert_eq!(
                seen, expected,
                "case {table_count} tables / {max_results:?}: expected {expected:?} \
                 exactly once in name order, got {seen:?}"
            );
        }
    }
}

/// Paging over `ListTables` with `DatabaseName` omitted terminates and yields every
/// table exactly once even when the database names are in a prefix relation through
/// `-` or `.`.
///
/// Both characters are admitted by the model's `ResourceCreateAPIName`
/// (`^[a-zA-Z0-9_.-]+$`) and both sort BELOW the `/` that joins the cursor's two
/// halves, so a cursor compared as a joined string would disagree with the stored
/// `(database, table)` order and re-issue its own token forever.
#[tokio::test]
async fn list_tables_pages_across_databases_whose_names_share_a_prefix() {
    // Each row is a database name; `-` (0x2D) and `.` (0x2E) sort below `/` (0x2F),
    // `_` (0x5F) above it.
    let databases = ["a", "a-b", "a.c", "a_d", "ab"];

    for max_results in [1, 2, 3, 20] {
        let emulator = Emulator::new();
        let mut pairs: Vec<(String, String)> = Vec::new();
        for database in databases {
            create_database(&emulator, database).await;
            for table in ["t1", "t2"] {
                create_table(&emulator, database, table).await;
                pairs.push((database.to_string(), table.to_string()));
            }
        }
        // The expected order is the STORED order: the (database, table) pair, which
        // is not the order the joined "<db>/<table>" strings sort in.
        pairs.sort();
        let expected: Vec<String> = pairs
            .iter()
            .map(|(database, table)| format!("{database}/{table}"))
            .collect();

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({ "MaxResults": max_results });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = emulator.ok("ListTables", &request).await;
            for table in page["Tables"].as_array().cloned().unwrap_or_default() {
                seen.push(format!(
                    "{}/{}",
                    table["DatabaseName"].as_str().unwrap_or_default(),
                    table["TableName"].as_str().unwrap_or_default()
                ));
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case MaxResults {max_results}: paging did not terminate after \
                 {pages} pages, saw {seen:?}"
            );
            match page["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case MaxResults {max_results}: expected {expected:?} exactly once in \
             (database, table) order, got {seen:?}"
        );
    }
}

/// With `DatabaseName` omitted, `ListTables` reports every database's tables; with it
/// set, only that database's. An unknown database is a 404.
#[tokio::test]
async fn list_tables_scopes_to_the_named_database() {
    let emulator = Emulator::new();
    for (database, tables) in [("db_a", vec!["t1", "t2"]), ("db_b", vec!["t3"])] {
        create_database(&emulator, database).await;
        for table in tables {
            create_table(&emulator, database, table).await;
        }
    }

    let names = |body: &Value| -> Vec<String> {
        body["Tables"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|table| {
                format!(
                    "{}/{}",
                    table["DatabaseName"].as_str().unwrap_or_default(),
                    table["TableName"].as_str().unwrap_or_default()
                )
            })
            .collect()
    };

    let all = emulator.ok("ListTables", &json!({})).await;
    assert_eq!(
        names(&all),
        vec!["db_a/t1", "db_a/t2", "db_b/t3"],
        "case ListTables(all): expected every database's tables, got {all:?}"
    );

    for (database, expected) in [("db_a", vec!["db_a/t1", "db_a/t2"]), ("db_b", vec!["db_b/t3"])] {
        let scoped = emulator
            .ok("ListTables", &json!({ "DatabaseName": database }))
            .await;
        assert_eq!(
            names(&scoped),
            expected,
            "case ListTables({database}): expected {expected:?}, got {scoped:?}"
        );
    }

    let (status, body) = emulator
        .call("ListTables", &json!({ "DatabaseName": "missing_db" }))
        .await;
    assert_eq!(
        status, 404,
        "case ListTables(missing database): expected 404, got {status} with {body:?}"
    );
}

/// `ListDatabases` pages the same way and reports each database's derived
/// `TableCount`.
#[tokio::test]
async fn list_databases_pages_and_reports_table_counts() {
    let emulator = Emulator::new();
    // database name -> how many tables it holds
    let fixtures = [("db_0", 0usize), ("db_1", 1), ("db_2", 2)];
    for (database, tables) in fixtures {
        create_database(&emulator, database).await;
        for index in 0..tables {
            create_table(&emulator, database, &format!("t{index}")).await;
        }
    }

    for max_results in [None, Some(1), Some(2), Some(20)] {
        let mut seen: Vec<(String, u64)> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(limit) = max_results {
                request["MaxResults"] = json!(limit);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = emulator.ok("ListDatabases", &request).await;
            for database in page["Databases"].as_array().cloned().unwrap_or_default() {
                seen.push((
                    database["DatabaseName"]
                        .as_str()
                        .expect("every listed database names itself")
                        .to_string(),
                    database["TableCount"]
                        .as_u64()
                        .expect("TableCount is a number"),
                ));
            }
            match page["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
            assert!(
                seen.len() <= fixtures.len(),
                "case MaxResults {max_results:?}: paging repeated entries, got {seen:?}"
            );
        }

        let expected: Vec<(String, u64)> = fixtures
            .iter()
            .map(|(name, tables)| ((*name).to_string(), *tables as u64))
            .collect();
        assert_eq!(
            seen, expected,
            "case MaxResults {max_results:?}: expected {expected:?}, got {seen:?}"
        );
    }
}

/// A `NextToken` this service did not issue is refused, rather than silently served
/// page one.
#[tokio::test]
async fn an_unissued_pagination_token_is_refused() {
    let emulator = Emulator::new();
    create_database(&emulator, "token_db").await;
    create_table(&emulator, "token_db", "token_table").await;

    for token in ["not-a-token", "zzzz", "0", "%%%%"] {
        for operation in ["ListTables", "ListDatabases"] {
            let (status, body) = emulator
                .call(operation, &json!({ "NextToken": token }))
                .await;
            assert_eq!(
                status, 400,
                "case {operation} token {token:?}: expected 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {operation} token {token:?}: unexpected error, got {body:?}"
            );
        }
    }
}

/// A database that still holds tables cannot be deleted: AWS documents that all
/// tables must be deleted first, or a `ValidationException` is thrown. The database
/// and its tables survive the refusal.
#[tokio::test]
async fn a_database_with_tables_is_not_deleted() {
    let emulator = Emulator::new();
    create_database(&emulator, "busy_db").await;
    for table in ["t1", "t2"] {
        create_table(&emulator, "busy_db", table).await;
    }

    let (status, body) = emulator
        .call("DeleteDatabase", &json!({ "DatabaseName": "busy_db" }))
        .await;
    assert_eq!(
        status, 400,
        "case DeleteDatabase(non-empty): expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case DeleteDatabase(non-empty): unexpected error, got {body:?}"
    );

    let described = emulator
        .ok("DescribeDatabase", &json!({ "DatabaseName": "busy_db" }))
        .await;
    assert_eq!(
        described["Database"]["TableCount"], 2,
        "case DeleteDatabase(non-empty): the database must survive, got {described:?}"
    );

    // Delete the tables and the database goes.
    for table in ["t1", "t2"] {
        let (status, body) = emulator
            .call(
                "DeleteTable",
                &json!({ "DatabaseName": "busy_db", "TableName": table }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case DeleteTable({table}): expected 200, got {status} with {body:?}"
        );
    }
    let (status, body) = emulator
        .call("DeleteDatabase", &json!({ "DatabaseName": "busy_db" }))
        .await;
    assert_eq!(
        status, 200,
        "case DeleteDatabase(emptied): expected 200, got {status} with {body:?}"
    );
}

/// A caller-supplied `KmsKeyId` is stored and reported; an absent one takes the
/// account's Timestream managed key, which is the same for every database in the
/// region rather than a per-call value.
#[tokio::test]
async fn the_kms_key_id_is_stored_or_defaulted() {
    let emulator = Emulator::new();

    let explicit = emulator
        .ok(
            "CreateDatabase",
            &json!({ "DatabaseName": "byok_db", "KmsKeyId": "alias/my-own-key" }),
        )
        .await;
    assert_eq!(
        explicit["Database"]["KmsKeyId"], "alias/my-own-key",
        "case explicit KmsKeyId: expected the caller's key, got {explicit:?}"
    );

    let defaulted_one = create_database(&emulator, "default_db_one").await;
    let defaulted_two = create_database(&emulator, "default_db_two").await;
    assert_eq!(
        defaulted_one["Database"]["KmsKeyId"], defaulted_two["Database"]["KmsKeyId"],
        "case default KmsKeyId: the managed key is one per account and region, got \
         {defaulted_one:?} and {defaulted_two:?}"
    );

    // The stored key survives a read, for both.
    for database in ["byok_db", "default_db_one"] {
        let created_key = if database == "byok_db" {
            explicit["Database"]["KmsKeyId"].clone()
        } else {
            defaulted_one["Database"]["KmsKeyId"].clone()
        };
        let described = emulator
            .ok("DescribeDatabase", &json!({ "DatabaseName": database }))
            .await;
        assert_eq!(
            described["Database"]["KmsKeyId"], created_key,
            "case {database}: DescribeDatabase must report the stored key \
             {created_key:?}, got {described:?}"
        );
    }
}

/// An operation this slice does not implement still answers 501, never an empty 200:
/// a client must be able to tell "not written yet" from a real answer.
#[tokio::test]
async fn an_unimplemented_operation_still_answers_501() {
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call(
            "UpdateDatabase",
            &json!({ "DatabaseName": "demo_db", "KmsKeyId": "alias/x" }),
        )
        .await;
    assert_eq!(
        status, 501,
        "case UpdateDatabase: expected 501, got {status} with {body:?}"
    );
}
