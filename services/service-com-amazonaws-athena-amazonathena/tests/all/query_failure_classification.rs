// SPDX-License-Identifier: CC0-1.0

//! How a FAILED query execution is CLASSIFIED: the `AthenaError` a read of a failed
//! execution reports, for the two failure classes a statement's own catalog entry causes.
//!
//! The rules under test, each stated over the class and not over one statement:
//!
//! * an external table's `LOCATION` denotes the FOLDER its data lives under, so a
//!   `CREATE EXTERNAL TABLE` whose location denotes a single object instead defines
//!   nothing: the execution settles FAILED carrying the user category, the error type
//!   Athena publishes for a table definition it refuses, and that class's own fixed
//!   message — and the catalog holds no entry afterwards. What the object store holds
//!   decides it where it holds anything there — an object at the key is a file and a key
//!   with objects under it is a folder — and where it holds nothing there, the name does:
//!   a last segment closed by a file extension is a file, and a location written as a
//!   folder path is a folder;
//! * a table the catalog marks as one of the ICEBERG open table format is read THROUGH
//!   the metadata file its entry names, so a read of one that names no metadata location
//!   settles FAILED with the user category, the generic user error type and that class's
//!   own fixed message — whichever half of the entry marks the format, and whatever the
//!   objects under its location hold.
//!
//! Both message texts are Athena's own for those classes and carry no variable part: they
//! name neither the database, the table, the location nor the serialization library.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the form
//! the location takes, what the object store holds at it, and which part of a catalog
//! entry marks the Iceberg format. The recorded cases — a location ending at a `.csv`
//! object key, and a table carrying Iceberg's own serialization library — are rows among
//! them.
//!
//! Every call goes over the wire the production front door uses: Athena and Glue through
//! `POST /` with `X-Amz-Target` and Amazon S3 through its own REST data plane, all on one
//! `MultiServiceRouter` over ONE shared engine.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::catalog_sql::{
    ddl, ERROR_CATEGORY_USER, ERROR_TYPE_GENERIC_USER, ERROR_TYPE_TABLE_DEFINITION,
};
use service_com_amazonaws_athena_amazonathena::state::{
    QUERY_EXECUTION_COMPLETION, QUERY_STATE_FAILED, QUERY_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The name Athena reaches the account's AWS Glue Data Catalog by.
const GLUE_CATALOG: &str = "AwsDataCatalog";

/// The database every statement in these suites runs in.
const DATABASE: &str = "shop";

/// The bucket every table in these suites keeps its data in.
const BUCKET: &str = "athena-failure-data";

/// Where a query's results are written, which every submission has to name.
const OUTPUT_LOCATION: &str = "s3://athena-failure-data/results/";

/// How long past [`QUERY_EXECUTION_COMPLETION`] a test waits for a started query to be
/// readable in its terminal state.
const SETTLE_GRACE: Duration = Duration::from_millis(400);

/// Athena's message for a `CREATE EXTERNAL TABLE` whose `LOCATION` denotes an object,
/// which is fixed text with no variable part.
const LOCATION_ON_FILE: &str = "Cannot create table on file, please specify a directory";

/// Athena's message for a read of a table marked as an Iceberg table that names no
/// metadata location, which is fixed text with no variable part.
const ICEBERG_WITHOUT_METADATA_LOCATION: &str = "GENERIC_USER_ERROR: Detected Iceberg type \
     table without metadata location. Please make sure an Iceberg-enabled compute engine \
     such as Athena or EMR Spark is used to create the table, or the table is created by \
     using the Iceberg open source AWS library iceberg-aws. Setting table_type parameter \
     in Glue metastore to create an Iceberg table is not supported.";

/// The serialization library an Iceberg table's storage descriptor carries, as the
/// engines that write such a table record it.
const ICEBERG_SERDE: &str = "org.apache.iceberg.mr.hive.HiveIcebergSerDe";

/// The `InputFormat` an Iceberg table's storage descriptor carries.
const ICEBERG_INPUT_FORMAT: &str = "org.apache.iceberg.mr.hive.HiveIcebergInputFormat";

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. Neither the key id nor the signature is a
/// credential: this harness turns request authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
}

/// Athena, Glue and Amazon S3 mounted on ONE shared engine, the way the router mounts
/// every service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_athena_amazonathena::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_1 call to `service`, and its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.us-east-1.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target}.{operation}"))
        .header("authorization", authorization(service))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Athena call, as a client makes it.
async fn athena(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "athena", "AmazonAthena", operation, body).await
}

/// One Glue call that must succeed.
async fn glue_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = call(router, "glue", "AWSGlue", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status} with {answer:?}"
    );
    answer
}

/// Create the bucket every table's data lives in.
async fn create_bucket(router: &MultiServiceRouter) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{BUCKET}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        response.status().is_success(),
        "setup: creating s3://{BUCKET} must succeed, got {}",
        response.status()
    );
}

/// Write one object, which is what a key then holds.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, body: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{BUCKET}/{key}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        response.status().is_success(),
        "case {case:?}: writing s3://{BUCKET}/{key} must succeed, got {}",
        response.status()
    );
}

/// Create the one database these suites run in.
async fn create_database(router: &MultiServiceRouter) {
    glue_ok(
        router,
        "setup",
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
}

/// Start one statement and return the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, case: &str, sql: &str) -> String {
    let (status, answer) = athena(
        router,
        "StartQueryExecution",
        &json!({
            "QueryString": sql,
            "QueryExecutionContext": { "Database": DATABASE, "Catalog": GLUE_CATALOG },
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartQueryExecution({sql}) must be accepted, got {status} with \
         {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {answer:?}"))
        .to_string()
}

/// Wait out the span a completion is owed after, once for a whole suite.
async fn settle() {
    ::tokio::time::sleep(QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).await;
}

/// The stored `QueryExecution` of `id`.
async fn execution(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = athena(
        router,
        "GetQueryExecution",
        &json!({ "QueryExecutionId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueryExecution must answer 200, got {status} with {body:?}"
    );
    body["QueryExecution"].clone()
}

/// Assert that `id` settled FAILED carrying exactly the category, the error type and the
/// message of one published failure class, reported as both the reason and the error
/// message, and never as retryable.
async fn failed_as(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    category: i64,
    error_type: i64,
    message: &str,
) {
    let found = execution(router, case, id).await;
    let status = found["Status"].clone();
    assert_eq!(
        status["State"].as_str(),
        Some(QUERY_STATE_FAILED),
        "case {case:?}: expected {QUERY_STATE_FAILED}, got {status:?}"
    );
    assert_eq!(
        status["AthenaError"]["ErrorCategory"].as_i64(),
        Some(category),
        "case {case:?}: expected error category {category}, got {status:?}"
    );
    assert_eq!(
        status["AthenaError"]["ErrorType"].as_i64(),
        Some(error_type),
        "case {case:?}: expected error type {error_type}, got {status:?}"
    );
    assert_eq!(
        status["AthenaError"]["ErrorMessage"].as_str(),
        Some(message),
        "case {case:?}: expected the published message, got {status:?}"
    );
    assert_eq!(
        status["StateChangeReason"].as_str(),
        Some(message),
        "case {case:?}: the reason and the error message are the same sentence, got \
         {status:?}"
    );
    assert_eq!(
        status["AthenaError"]["Retryable"].as_bool(),
        Some(false),
        "case {case:?}: a failed execution is terminal, so it is not retryable, got \
         {status:?}"
    );
}

/// Assert that `id` SUCCEEDED, carrying no failure of any kind.
async fn succeeded(router: &MultiServiceRouter, case: &str, id: &str) {
    let found = execution(router, case, id).await;
    let status = found["Status"].clone();
    assert_eq!(
        status["State"].as_str(),
        Some(QUERY_STATE_SUCCEEDED),
        "case {case:?}: expected {QUERY_STATE_SUCCEEDED}, got {status:?}"
    );
    assert!(
        status["AthenaError"].is_null() && status["StateChangeReason"].is_null(),
        "case {case:?}: a succeeded execution carries no failure, got {status:?}"
    );
}

/// The stored Glue table `shop`.`table`, or `None` when the catalog holds none.
async fn glue_table(router: &MultiServiceRouter, table: &str) -> Option<Value> {
    let (status, answer) = call(
        router,
        "glue",
        "AWSGlue",
        "GetTable",
        &json!({ "DatabaseName": DATABASE, "Name": table }),
    )
    .await;
    (status == 200).then(|| answer["Table"].clone())
}

/// One row of the LOCATION table: a location a `CREATE EXTERNAL TABLE` declares, and
/// whether it denotes a file rather than the folder a table's data lives under.
struct LocationCase {
    /// What this row is about.
    case: &'static str,
    /// The table the statement defines.
    table: &'static str,
    /// The `LOCATION` it declares, as submitted.
    location: &'static str,
    /// Whether that location denotes an OBJECT, which is what refuses the statement.
    on_file: bool,
}

/// An external table's `LOCATION` denotes a FOLDER: a statement whose location denotes a
/// single object fails with the published table-definition class and registers nothing,
/// and every location that denotes a folder — one holding objects, one holding none, one
/// that is merely a prefix of other keys, and one written as a folder path — registers the
/// table.
#[tokio::test]
async fn an_external_table_location_has_to_denote_a_folder() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router).await;
    // The objects a location may end AT: one named like a data file, one named with no
    // extension at all, and one under a deeper prefix. The last one lives UNDER a key
    // whose own name carries an extension, which makes that key a folder.
    put_object(&router, "setup", "products/category_a/items.csv", "1,desk,20\n").await;
    put_object(&router, "setup", "drops/payload", "1,desk,20\n").await;
    put_object(&router, "setup", "logs/2024/01/raw.json", "{}\n").await;
    put_object(&router, "setup", "archive.csv/part-0", "1,desk,20\n").await;

    let cases: Vec<LocationCase> = vec![
        LocationCase {
            case: "a location ending at an object named like a data file",
            table: "items_on_file",
            location: "s3://athena-failure-data/products/category_a/items.csv",
            on_file: true,
        },
        LocationCase {
            case: "a file name the object store holds nothing at",
            table: "absent_items_on_file",
            location: "s3://athena-failure-data/products/category_b/items.csv",
            on_file: true,
        },
        LocationCase {
            case: "a file name of another storage format, nothing stored at it",
            table: "absent_parquet_on_file",
            location: "s3://athena-failure-data/exports/day=1/part-00000.parquet",
            on_file: true,
        },
        LocationCase {
            case: "a location ending at an object whose name has no extension",
            table: "payload_on_file",
            location: "s3://athena-failure-data/drops/payload",
            on_file: true,
        },
        LocationCase {
            case: "a location ending at an object, written with a trailing separator",
            table: "raw_on_file",
            location: "s3://athena-failure-data/logs/2024/01/raw.json/",
            on_file: true,
        },
        LocationCase {
            case: "the prefix the object lives under",
            table: "items_under_prefix",
            location: "s3://athena-failure-data/products/category_a/",
            on_file: false,
        },
        LocationCase {
            case: "a prefix the object store holds nothing under",
            table: "nothing_under_it",
            location: "s3://athena-failure-data/products/category_z/",
            on_file: false,
        },
        LocationCase {
            case: "a key that is a prefix of an object key but is no object",
            table: "partial_key",
            location: "s3://athena-failure-data/products/category",
            on_file: false,
        },
        LocationCase {
            case: "a key with objects under it although its own name has an extension",
            table: "folder_named_like_a_file",
            location: "s3://athena-failure-data/archive.csv",
            on_file: false,
        },
        LocationCase {
            case: "a file name written as a folder path, nothing stored at it",
            table: "file_name_as_folder",
            location: "s3://athena-failure-data/staging/items.csv/",
            on_file: false,
        },
        LocationCase {
            case: "a folder whose name carries a dotted version, nothing stored at it",
            table: "dotted_version_folder",
            location: "s3://athena-failure-data/exports/v1.0",
            on_file: false,
        },
        LocationCase {
            case: "the bucket itself",
            table: "bucket_root",
            location: "s3://athena-failure-data/",
            on_file: false,
        },
        LocationCase {
            case: "a location that names no object store path at all",
            table: "no_path",
            location: "hdfs://cluster/warehouse/items",
            on_file: false,
        },
    ];

    let mut started: Vec<(&LocationCase, String)> = Vec::new();
    for case in &cases {
        let sql = format!(
            "CREATE EXTERNAL TABLE {DATABASE}.{} (item_id string, name string, price int) \
             ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION '{}' \
             TBLPROPERTIES ('skip.header.line.count'='1')",
            case.table, case.location
        );
        started.push((case, start(&router, case.case, &sql).await));
    }
    settle().await;

    for (case, id) in &started {
        if case.on_file {
            failed_as(
                &router,
                case.case,
                id,
                i64::from(ERROR_CATEGORY_USER),
                i64::from(ERROR_TYPE_TABLE_DEFINITION),
                LOCATION_ON_FILE,
            )
            .await;
            assert!(
                glue_table(&router, case.table).await.is_none(),
                "case {:?}: a refused statement registers nothing, yet the catalog holds \
                 {}",
                case.case,
                case.table
            );
        } else {
            succeeded(&router, case.case, id).await;
            let table = glue_table(&router, case.table).await.unwrap_or_else(|| {
                panic!(
                    "case {:?}: the catalog must hold {} after the statement succeeded",
                    case.case, case.table
                )
            });
            assert_eq!(
                table["StorageDescriptor"]["Location"].as_str(),
                Some(case.location.trim_end_matches('/')),
                "case {:?}: the registered location is the declared prefix, got {table:?}",
                case.case
            );
            assert_eq!(
                table["TableType"].as_str(),
                Some(ddl::TABLE_TYPE_EXTERNAL),
                "case {:?}: the statement registers an external table, got {table:?}",
                case.case
            );
        }
    }
}

/// One row of the Iceberg table: the catalog entry a read addresses, and whether that
/// entry is an Iceberg table with no metadata location.
struct IcebergCase {
    /// What this row is about.
    case: &'static str,
    /// The table the read addresses.
    table: &'static str,
    /// The `SerdeInfo.SerializationLibrary` its entry carries.
    serde: &'static str,
    /// The `StorageDescriptor.InputFormat` its entry carries, if any.
    input_format: Option<&'static str>,
    /// Its table properties.
    properties: Value,
    /// Whether a read of it is the Iceberg-without-metadata-location class.
    without_metadata: bool,
}

/// A read of a table the catalog marks as an ICEBERG table and holds no metadata location
/// for is the published generic user error, whichever half of the entry marks the format;
/// an Iceberg table that names a metadata location is NOT that class, and a table marked
/// as no open table format reads its objects as before.
#[tokio::test]
async fn an_iceberg_table_without_a_metadata_location_is_a_user_error() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router).await;

    let cases: Vec<IcebergCase> = vec![
        IcebergCase {
            case: "the table_type property names the format",
            table: "by_property",
            serde: ddl::LAZY_SIMPLE_SERDE,
            input_format: Some(ddl::TEXT_INPUT_FORMAT),
            properties: json!({ "table_type": "ICEBERG" }),
            without_metadata: true,
        },
        IcebergCase {
            case: "the table_type property names the format in lower case",
            table: "by_lowercase_property",
            serde: ddl::LAZY_SIMPLE_SERDE,
            input_format: Some(ddl::TEXT_INPUT_FORMAT),
            properties: json!({ "table_type": "iceberg" }),
            without_metadata: true,
        },
        IcebergCase {
            case: "the property name itself is written in upper case",
            table: "by_uppercase_name",
            serde: ddl::LAZY_SIMPLE_SERDE,
            input_format: Some(ddl::TEXT_INPUT_FORMAT),
            properties: json!({ "TABLE_TYPE": "ICEBERG" }),
            without_metadata: true,
        },
        IcebergCase {
            case: "the serialization library is Iceberg's own",
            table: "by_serde",
            serde: ICEBERG_SERDE,
            input_format: None,
            properties: json!({}),
            without_metadata: true,
        },
        IcebergCase {
            case: "the input format is Iceberg's own",
            table: "by_input_format",
            serde: ddl::LAZY_SIMPLE_SERDE,
            input_format: Some(ICEBERG_INPUT_FORMAT),
            properties: json!({}),
            without_metadata: true,
        },
        IcebergCase {
            case: "an Iceberg table that names its metadata location",
            table: "with_metadata",
            serde: ICEBERG_SERDE,
            input_format: Some(ICEBERG_INPUT_FORMAT),
            properties: json!({
                "table_type": "ICEBERG",
                "metadata_location":
                    "s3://athena-failure-data/with_metadata/metadata/00000.json",
            }),
            without_metadata: false,
        },
        IcebergCase {
            case: "a table marked as no open table format",
            table: "plain_hive",
            serde: ddl::LAZY_SIMPLE_SERDE,
            input_format: Some(ddl::TEXT_INPUT_FORMAT),
            properties: json!({}),
            without_metadata: false,
        },
    ];

    for case in &cases {
        let mut storage = json!({
            "Columns": [
                { "Name": "item_id", "Type": "bigint" },
                { "Name": "name", "Type": "string" },
            ],
            "Location": format!("s3://{BUCKET}/{}/", case.table),
            "SerdeInfo": {
                "SerializationLibrary": case.serde,
                "Parameters": { "field.delim": "," },
            },
        });
        if let Some(format) = case.input_format {
            storage["InputFormat"] = json!(format);
        }
        glue_ok(
            &router,
            case.case,
            "CreateTable",
            &json!({
                "DatabaseName": DATABASE,
                "TableInput": {
                    "Name": case.table,
                    "TableType": "EXTERNAL_TABLE",
                    "StorageDescriptor": storage,
                    "Parameters": case.properties,
                },
            }),
        )
        .await;
        // Objects ARE there under every location, so what fails a read is the entry's own
        // missing metadata location and never an empty prefix.
        put_object(
            &router,
            case.case,
            &format!("{}/rows.csv", case.table),
            "1,desk\n2,lamp\n",
        )
        .await;
    }

    let mut started: Vec<(&IcebergCase, String)> = Vec::new();
    for case in &cases {
        let sql = format!(
            "SELECT count(*) AS n FROM \"{DATABASE}\".\"{}\"",
            case.table
        );
        started.push((case, start(&router, case.case, &sql).await));
    }
    settle().await;

    for (case, id) in &started {
        if case.without_metadata {
            failed_as(
                &router,
                case.case,
                id,
                i64::from(ERROR_CATEGORY_USER),
                i64::from(ERROR_TYPE_GENERIC_USER),
                ICEBERG_WITHOUT_METADATA_LOCATION,
            )
            .await;
            continue;
        }
        let found = execution(&router, case.case, id).await;
        let status = found["Status"].clone();
        assert_ne!(
            status["AthenaError"]["ErrorMessage"].as_str(),
            Some(ICEBERG_WITHOUT_METADATA_LOCATION),
            "case {:?}: this entry is not the Iceberg-without-metadata-location class, got \
             {status:?}",
            case.case
        );
        assert_ne!(
            status["AthenaError"]["ErrorType"].as_i64(),
            Some(i64::from(ERROR_TYPE_GENERIC_USER)),
            "case {:?}: this entry is not the generic user error class, got {status:?}",
            case.case
        );
    }

    // The table marked as no open table format reads the objects under its location, so
    // the rule narrows to the entries that ARE marked and nothing else.
    let plain = started
        .iter()
        .find(|(case, _)| case.table == "plain_hive")
        .map(|(_, id)| id.clone())
        .expect("the plain Hive row is started");
    succeeded(&router, "a table marked as no open table format", &plain).await;
    let (status, results) = athena(
        &router,
        "GetQueryResults",
        &json!({ "QueryExecutionId": plain, "MaxResults": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetQueryResults must answer 200, got {status} with {results:?}"
    );
    let counted = results["ResultSet"]["Rows"]
        .as_array()
        .and_then(|rows| rows.last())
        .map(|row| row["Data"][0]["VarCharValue"].clone());
    assert_eq!(
        counted,
        Some(json!("2")),
        "a plain Hive table counts the rows its objects hold, got {results:?}"
    );
}
