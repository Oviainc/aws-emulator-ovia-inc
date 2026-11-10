// SPDX-License-Identifier: CC0-1.0

//! `ListTableMetadata` and `GetTableMetadata` over the tables the AWS Glue Data Catalog
//! holds.
//!
//! The rule under test, from the two operations' own documentation ("lists the metadata
//! for the tables in the specified data catalog database", "returns table metadata for
//! the specified catalog, database, and table") and from `TableMetadata`'s members ("the
//! name of the table", "the time that the table was created", "the type of table", "a
//! list of the columns in the table", "a list of the partition keys in the table", "a set
//! of custom key/value pairs for table properties"): Athena registers no tables of its
//! own, so for `AwsDataCatalog` — which IS the account's Glue Data Catalog — every member
//! it reports for a table is one that catalog holds for that table, whatever the table
//! defines, however it was named and however many tables the database holds. A table is
//! reported as the flat Hive property set it is, so the members of the catalog's
//! structured storage descriptor (where the data lives, the input and output formats, the
//! serde's library and that serde's own parameters) come back as table properties beside
//! the table's own, and a member the catalog holds nothing for — the partition keys of an
//! unpartitioned table, say — is absent rather than reported empty. `Expression` then
//! selects the tables whose WHOLE name matches the pattern language this catalog takes
//! (`*` any run of characters, `|` between alternatives, everything else a literal), and
//! `MaxResults` with the returned `NextToken` pages the catalog's own name order without
//! dropping or repeating a name. A data catalog a client REGISTERS decides where its own
//! metadata comes from: a `GLUE` catalog naming this account's catalog id reads the same
//! tables, and one served by an external metastore or a connector function answers the
//! metastore's error.
//!
//! Every table here is created over GLUE's real wire (`AWSGlue.CreateTable`) against the
//! same shared engine both services are mounted on, exactly as a client creates one, and
//! every Athena call goes over Athena's own wire (`POST /` with
//! `X-Amz-Target: AmazonAthena.<Operation>` and an `awsJson1_1` body) through the same
//! `MultiServiceRouter` the production front door uses — so what is asserted is HTTP
//! behaviour and not a direct call into a handler.
//!
//! The suites are tables: each row is one input along the axis the rule generalises over
//! (the member set a table defines, the casing and count of names, the shape of the
//! pattern, the page size, the type of the catalog), and the recorded case — a Glue table
//! whose schema Athena's metadata mirrors — is one row among them.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The name Athena reaches the account's AWS Glue Data Catalog by.
const GLUE_CATALOG: &str = "AwsDataCatalog";

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. The key id and signature are not
/// credentials: this harness turns request authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
}

/// Athena and Glue mounted on ONE shared engine, the way the router mounts every
/// service: the catalog Glue writes is the catalog Athena reports.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_athena_amazonathena::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        engine,
    ));
    router
}

/// Make one awsJson1_1 call to `service` and return its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target}.{operation}"))
        .header("authorization", authorization(service))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

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

/// One Athena call that must succeed, and its body.
async fn athena_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = athena(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// One Glue call that must succeed, and its body.
async fn glue_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, "glue", "AWSGlue", operation, body).await;
    assert_eq!(
        status, 200,
        "Glue {operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// The awsJson error code of a failure body (`__type`), which may carry a namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` member of a failure body, which carries the refusal's wording.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// Seconds since the epoch, the scale a reported instant is compared on.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// The same document with every number rendered one way, so an epoch instant written
/// `1.7e9` and the same instant written `1700000000` compare equal.
fn normalize(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap_or_default()),
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(member, value)| (member.clone(), normalize(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The reported metadata with `CreateTime` removed, after asserting it is the instant the
/// catalog recorded the create (within the window the creating calls were made in).
///
/// `CreateTime` is the one member no case can state: the catalog sets it when the table is
/// created, so it is checked against that window instead of compared to a literal.
fn without_create_time(case: &str, entry: &Value, from: f64, until: f64) -> Value {
    let mut entry = entry.clone();
    let Some(members) = entry.as_object_mut() else {
        panic!("case {case:?}: expected a TableMetadata object, got {entry}")
    };
    let created = members
        .remove("CreateTime")
        .as_ref()
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: every table reports the instant it was created, got {members:?}")
        });
    assert!(
        (from - 1.0..=until + 1.0).contains(&created),
        "case {case:?}: expected a CreateTime between {from} and {until}, got {created}"
    );
    entry.clone()
}

/// The `Name`s of a `TableMetadataList`, in the order the service returned them.
fn reported_names(page: &Value) -> Vec<String> {
    page["TableMetadataList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a TableMetadataList, got {page}"))
        .iter()
        .map(|table| {
            table["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a table Name, got {table}"))
                .to_string()
        })
        .collect()
}

/// Create `database` over Glue's API, then every table in `tables`.
async fn seed(router: &MultiServiceRouter, database: &Value, tables: &[Value]) {
    glue_ok(router, "CreateDatabase", &json!({ "DatabaseInput": database })).await;
    let name = database["Name"].as_str().expect("the case names a database");
    for table in tables {
        glue_ok(
            router,
            "CreateTable",
            &json!({ "DatabaseName": name, "TableInput": table }),
        )
        .await;
    }
}

/// One case: a database, and the tables to create in it over Glue's API, each beside the
/// metadata Athena must then report for it (`CreateTime` aside).
struct Case {
    name: &'static str,
    /// The `DatabaseInput` the database is created with.
    database: Value,
    /// `(TableInput sent to Glue, TableMetadata Athena must report)`.
    tables: Vec<(Value, Value)>,
}

/// The cases, differing along what the rule generalises over: the member set a table
/// defines (none beyond its name, a full external table, a governed one with a complete
/// storage descriptor), the casing of the names, the number of columns and partition
/// keys, and how many tables the database holds.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a database that holds no tables",
            database: json!({ "Name": "empty_db" }),
            tables: Vec::new(),
        },
        Case {
            name: "a table defined by its name alone",
            database: json!({ "Name": "minimal_db" }),
            tables: vec![(
                json!({ "Name": "bare" }),
                // The catalog holds no storage descriptor and no partition keys for this
                // table, so there is no column list and no key list to report; its
                // property set is empty rather than absent, a table always having one.
                json!({ "Name": "bare", "Parameters": {} }),
            )],
        },
        Case {
            name: "an external table with columns, partition keys, properties and an access time",
            database: json!({
                "Name": "analytics_db",
                "Description": "the recorded case",
                "LocationUri": "s3://bucket/analytics/"
            }),
            tables: vec![(
                json!({
                    "Name": "events",
                    "TableType": "EXTERNAL_TABLE",
                    "Owner": "data-eng",
                    "Retention": 7,
                    "LastAccessTime": 1_700_000_000,
                    "StorageDescriptor": {
                        "Columns": [
                            { "Name": "id", "Type": "string" },
                            { "Name": "amount", "Type": "double", "Comment": "cents" }
                        ],
                        "Location": "s3://bucket/events/",
                        "InputFormat": "org.apache.hadoop.mapred.TextInputFormat"
                    },
                    "PartitionKeys": [ { "Name": "dt", "Type": "string" } ],
                    "Parameters": { "EXTERNAL": "TRUE", "classification": "json" }
                }),
                json!({
                    "Name": "events",
                    "TableType": "EXTERNAL_TABLE",
                    "LastAccessTime": 1_700_000_000,
                    "Columns": [
                        { "Name": "id", "Type": "string" },
                        { "Name": "amount", "Type": "double", "Comment": "cents" }
                    ],
                    "PartitionKeys": [ { "Name": "dt", "Type": "string" } ],
                    // The table's own properties, plus the storage descriptor flattened.
                    // The descriptor names no output format and no serde, so neither
                    // contributes a property.
                    "Parameters": {
                        "EXTERNAL": "TRUE",
                        "classification": "json",
                        "location": "s3://bucket/events/",
                        "inputformat": "org.apache.hadoop.mapred.TextInputFormat"
                    }
                }),
            )],
        },
        Case {
            name: "a mixed-case name, two partition keys, a fully described storage \
                   descriptor, and a column the catalog holds parameters for",
            database: json!({ "Name": "Warehouse_DB" }),
            tables: vec![(
                json!({
                    "Name": "Sales_By_Day",
                    "TableType": "GOVERNED",
                    "StorageDescriptor": {
                        // A Glue column carries its own `Parameters` map, which Athena's
                        // `Column` (Name, Type, Comment) has nowhere to report.
                        "Columns": [
                            { "Name": "day", "Type": "date", "Parameters": { "k": "v" } },
                            { "Name": "region", "Type": "varchar(20)", "Comment": "iso" }
                        ],
                        "Location": "s3://bucket/warehouse/sales/",
                        "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
                        "OutputFormat": "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat",
                        "SerdeInfo": {
                            "SerializationLibrary": "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe",
                            "Parameters": { "field.delim": ",", "serialization.format": "1" }
                        }
                    },
                    "PartitionKeys": [
                        { "Name": "year", "Type": "int" },
                        { "Name": "month", "Type": "int" }
                    ]
                }),
                json!({
                    // Both the database and the table name are folded by the catalog
                    // ("for Hive compatibility"), so the folded name is what is reported.
                    "Name": "sales_by_day",
                    "TableType": "GOVERNED",
                    "Columns": [
                        { "Name": "day", "Type": "date" },
                        { "Name": "region", "Type": "varchar(20)", "Comment": "iso" }
                    ],
                    "PartitionKeys": [
                        { "Name": "year", "Type": "int" },
                        { "Name": "month", "Type": "int" }
                    ],
                    // The whole storage descriptor, flattened: the location, both
                    // formats, the serde's library, and each of the serde's own
                    // parameters under its prefixed name.
                    "Parameters": {
                        "location": "s3://bucket/warehouse/sales/",
                        "inputformat": "org.apache.hadoop.mapred.TextInputFormat",
                        "outputformat": "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat",
                        "serde.serialization.lib": "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe",
                        "serde.param.field.delim": ",",
                        "serde.param.serialization.format": "1"
                    }
                }),
            )],
        },
        Case {
            name: "a table property under a name the storage descriptor also fills",
            database: json!({ "Name": "collision_db" }),
            tables: vec![(
                json!({
                    "Name": "shadowed",
                    "Parameters": { "location": "s3://bucket/stale/", "owner": "data-eng" },
                    "StorageDescriptor": { "Location": "s3://bucket/current/" }
                }),
                json!({
                    "Name": "shadowed",
                    // Where the two collide, the location the catalog actually holds the
                    // table's data under is the one reported.
                    "Parameters": { "location": "s3://bucket/current/", "owner": "data-eng" }
                }),
            )],
        },
        Case {
            name: "three tables created out of name order",
            database: json!({ "Name": "unordered_db" }),
            tables: ["zeta", "alpha", "mid"]
                .into_iter()
                .map(|name| {
                    (
                        json!({ "Name": name, "TableType": "EXTERNAL_TABLE" }),
                        json!({
                            "Name": name,
                            "TableType": "EXTERNAL_TABLE",
                            "Parameters": {}
                        }),
                    )
                })
                .collect(),
        },
    ]
}

/// Every table the Data Catalog holds in the named database is reported, in the catalog's
/// own name order, with the definition the catalog holds for it — and the single-table
/// read answers the identical metadata.
#[tokio::test]
async fn reported_metadata_mirrors_the_glue_table_definition() {
    for case in cases() {
        let router = emulator();
        let database = case.database["Name"]
            .as_str()
            .expect("the case names a database")
            .to_string();
        let inputs: Vec<Value> = case.tables.iter().map(|(input, _)| input.clone()).collect();
        let created_from = now_seconds();
        seed(&router, &case.database, &inputs).await;
        let created_until = now_seconds();

        let body = athena_ok(
            &router,
            "ListTableMetadata",
            &json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": database }),
        )
        .await;
        let reported = body["TableMetadataList"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: ListTableMetadata must report a TableMetadataList, got {body}",
                    case.name
                )
            })
            .clone();

        // The catalog's order is folded-name order, so that is the order the expectations
        // are compared in.
        let mut expected: Vec<Value> = case
            .tables
            .iter()
            .map(|(_, metadata)| metadata.clone())
            .collect();
        expected.sort_by_key(|metadata| {
            metadata["Name"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase()
        });

        assert_eq!(
            reported.len(),
            expected.len(),
            "case {:?}: expected {} tables, got {reported:?}",
            case.name,
            expected.len()
        );
        for (entry, want) in reported.iter().zip(expected.iter()) {
            let got = without_create_time(case.name, entry, created_from, created_until);
            assert_eq!(
                normalize(&got),
                normalize(want),
                "case {:?}: expected the metadata {want}, got {got}",
                case.name
            );
        }

        assert!(
            body.get("NextToken").is_none(),
            "case {:?}: the whole database fits in one page, so no NextToken may be \
             reported, got {body}",
            case.name
        );

        for want in &expected {
            let name = want["Name"].as_str().expect("every expectation names a table");
            // The catalog folds a table name, so any casing of a stored name reaches it.
            for requested in [name.to_string(), name.to_ascii_uppercase()] {
                let got = athena_ok(
                    &router,
                    "GetTableMetadata",
                    &json!({
                        "CatalogName": GLUE_CATALOG,
                        "DatabaseName": database,
                        "TableName": requested
                    }),
                )
                .await;
                let entry = without_create_time(
                    case.name,
                    &got["TableMetadata"],
                    created_from,
                    created_until,
                );
                assert_eq!(
                    normalize(&entry),
                    normalize(want),
                    "case {:?}: GetTableMetadata for {requested:?} must answer the metadata \
                     {want}, got {entry}",
                    case.name
                );
            }
        }

        // Both reads are REPORTING: a second listing with no intervening write answers
        // the identical document.
        let repeat = athena_ok(
            &router,
            "ListTableMetadata",
            &json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": database }),
        )
        .await;
        assert_eq!(
            normalize(&repeat),
            normalize(&body),
            "case {:?}: a repeated ListTableMetadata must answer identically, got {repeat}",
            case.name
        );
    }
}

/// The five table names every pattern and paging row below selects from, in the
/// catalog's own order.
const CATALOG_TABLES: [&str; 5] = ["customers", "events", "events_raw", "orders", "orders_2024"];

/// Seed one database holding [`CATALOG_TABLES`].
async fn seed_catalog_tables(router: &MultiServiceRouter, database: &str) {
    let tables: Vec<Value> = CATALOG_TABLES
        .iter()
        .map(|name| json!({ "Name": name }))
        .collect();
    seed(router, &json!({ "Name": database }), &tables).await;
}

/// `Expression` reports exactly the names whose WHOLE name its pattern matches.
#[tokio::test]
async fn the_expression_selects_the_reported_names() {
    let router = emulator();
    seed_catalog_tables(&router, "filtered_db").await;

    // (the Expression, the names it selects). The rows differ along the shape of the
    // pattern: a bare literal, a trailing wildcard, a leading one, a wildcard in the
    // middle, an alternation of two patterns, the pattern that matches everything, the
    // regular-expression metacharacters (which keep their meaning, so `events.*` selects
    // the `events`-prefixed names and `orders_[0-9]*` the year-suffixed one), a pattern
    // in another case (no match: the comparison is case-sensitive) and a literal nothing
    // matches.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("events", vec!["events"]),
        ("events*", vec!["events", "events_raw"]),
        ("*raw", vec!["events_raw"]),
        ("o*s", vec!["orders"]),
        (
            "*s*",
            vec!["customers", "events", "events_raw", "orders", "orders_2024"],
        ),
        ("events|orders*", vec!["events", "orders", "orders_2024"]),
        ("*", CATALOG_TABLES.to_vec()),
        ("events.*", vec!["events_raw"]),
        ("orders_[0-9]*", vec!["orders_2024"]),
        ("EVENTS*", Vec::new()),
        ("clicks", Vec::new()),
    ];

    for (expression, expected) in rows {
        let page = athena_ok(
            &router,
            "ListTableMetadata",
            &json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": "filtered_db",
                "Expression": expression
            }),
        )
        .await;
        assert_eq!(
            reported_names(&page),
            expected,
            "case Expression {expression:?}: expected {expected:?}, got {:?}",
            reported_names(&page)
        );
    }

    // A listing with no Expression at all reports every table: "if no expression is
    // supplied, metadata for all tables are listed".
    let page = athena_ok(
        &router,
        "ListTableMetadata",
        &json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": "filtered_db" }),
    )
    .await;
    assert_eq!(
        reported_names(&page),
        CATALOG_TABLES.to_vec(),
        "case no Expression: expected every table, got {:?}",
        reported_names(&page)
    );

    // A value that is no pattern at all cannot select anything, so it is refused rather
    // than answered with an empty listing — and so is one whose compiled matcher would be
    // larger than this catalog compiles (`MAX_PATTERN_PROGRAM_BYTES`): the pattern is
    // request text, so the work it can buy is bounded by the handler.
    let oversized = format!("(?:{}){{200}}", "[a-z0-9]{200}");
    for pattern in [
        "events[".to_string(),
        "(events".to_string(),
        "a{2,1}".to_string(),
        oversized,
    ] {
        let (status, body) = athena(
            &router,
            "ListTableMetadata",
            &json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": "filtered_db",
                "Expression": pattern
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case Expression {pattern:?}: must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidRequestException"),
            "case Expression {pattern:?}: expected InvalidRequestException, got {}",
            error_code(&body)
        );
    }
    // A pattern of several wildcards is served: the matcher never backtracks, so what
    // bounds the work is the compiled size rather than a count of wildcards.
    let page = athena_ok(
        &router,
        "ListTableMetadata",
        &json!({
            "CatalogName": GLUE_CATALOG,
            "DatabaseName": "filtered_db",
            "Expression": "*".repeat(8)
        }),
    )
    .await;
    assert_eq!(
        reported_names(&page),
        CATALOG_TABLES.to_vec(),
        "case Expression of several wildcards: every name matches, got {:?}",
        reported_names(&page)
    );
}

/// One paging row: the page size and pattern to walk the catalog with, and the names the
/// walk must report.
struct PagingCase {
    name: &'static str,
    /// The `MaxResults` the request names, or `None` for a request that names none.
    max_results: Option<i64>,
    /// The `Expression` in force while paging, if any.
    expression: Option<&'static str>,
    /// The names the whole walk must report, in the catalog's own order.
    expected: Vec<&'static str>,
}

/// `MaxResults` bounds the page and the `NextToken` resumes where it ended, so walking
/// the pages reports the catalog's order once through — at every page size, and with a
/// pattern in force.
#[tokio::test]
async fn max_results_pages_the_catalog_order() {
    let router = emulator();
    seed_catalog_tables(&router, "paged_db").await;

    // The page sizes span one result per page, a page that divides the set, the exact size
    // of the set, and the model's maximum (`MaxTableMetadataCount` is 1..50) — plus a
    // request that names no MaxResults at all, and one paging a filtered set.
    let rows: Vec<PagingCase> = vec![
        PagingCase {
            name: "one table per page",
            max_results: Some(1),
            expression: None,
            expected: CATALOG_TABLES.to_vec(),
        },
        PagingCase {
            name: "two tables per page",
            max_results: Some(2),
            expression: None,
            expected: CATALOG_TABLES.to_vec(),
        },
        PagingCase {
            name: "a page the size of the set",
            max_results: Some(i64::try_from(CATALOG_TABLES.len()).expect("the set is small")),
            expression: None,
            expected: CATALOG_TABLES.to_vec(),
        },
        PagingCase {
            name: "the largest page the model allows",
            max_results: Some(50),
            expression: None,
            expected: CATALOG_TABLES.to_vec(),
        },
        PagingCase {
            name: "no MaxResults",
            max_results: None,
            expression: None,
            expected: CATALOG_TABLES.to_vec(),
        },
        PagingCase {
            name: "one table per page, filtered",
            max_results: Some(1),
            expression: Some("events*"),
            expected: vec!["events", "events_raw"],
        },
    ];

    for PagingCase {
        name: case,
        max_results,
        expression,
        expected,
    } in rows
    {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": "paged_db"
            });
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(expression) = expression {
                request["Expression"] = json!(expression);
            }
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let page = athena_ok(&router, "ListTableMetadata", &request).await;
            let names = reported_names(&page);
            if let Some(limit) = max_results {
                assert!(
                    i64::try_from(names.len()).unwrap_or(i64::MAX) <= limit,
                    "case {case:?}: a page must hold at most {limit} tables, got {names:?}"
                );
            }
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= CATALOG_TABLES.len() + 1,
                "case {case:?}: the walk must end, got {walked:?} after {pages} pages"
            );
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the walk to report {expected:?} once through, got \
             {walked:?}"
        );
    }
}

/// What each read refuses, and with which modelled error: a catalog nothing registered is
/// wrong INPUT, while a database or table the catalog does not hold is what the metastore
/// answered.
#[tokio::test]
async fn refusals_name_what_was_asked_for() {
    let router = emulator();
    seed(
        &router,
        &json!({ "Name": "refusal_db" }),
        &[json!({ "Name": "events" })],
    )
    .await;

    // (the case, the operation, the request, the expected error code, a fragment the
    // message must name)
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a catalog nothing registered, listing",
            "ListTableMetadata",
            json!({ "CatalogName": "NotACatalog", "DatabaseName": "refusal_db" }),
            "InvalidRequestException",
            "NotACatalog",
        ),
        (
            "a catalog nothing registered, single table",
            "GetTableMetadata",
            json!({
                "CatalogName": "NotACatalog",
                "DatabaseName": "refusal_db",
                "TableName": "events"
            }),
            "InvalidRequestException",
            "NotACatalog",
        ),
        (
            "a database the catalog does not hold, listing",
            "ListTableMetadata",
            json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": "no_such_db" }),
            "MetadataException",
            "no_such_db",
        ),
        (
            "a database the catalog does not hold, single table",
            "GetTableMetadata",
            json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": "no_such_db",
                "TableName": "events"
            }),
            "MetadataException",
            "no_such_db",
        ),
        (
            "a table the database does not hold",
            "GetTableMetadata",
            json!({
                "CatalogName": GLUE_CATALOG,
                "DatabaseName": "refusal_db",
                "TableName": "no_such_table"
            }),
            "MetadataException",
            // The metastore's own answer for a table it does not hold, which names no
            // entity of the request.
            service_com_amazonaws_athena_amazonathena::state::table_not_found(),
        ),
    ];

    for (case, operation, request, expected_code, fragment) in rows {
        let (status, body) = athena(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: must be refused with HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&body)
        );
        assert!(
            error_message(&body).contains(fragment),
            "case {case:?}: the message must name {fragment:?}, got {:?}",
            error_message(&body)
        );
    }

    // The catalog's own name is case-insensitive, so every casing of it is served.
    for catalog in ["AwsDataCatalog", "awsdatacatalog", "AWSDATACATALOG"] {
        let page = athena_ok(
            &router,
            "ListTableMetadata",
            &json!({ "CatalogName": catalog, "DatabaseName": "refusal_db" }),
        )
        .await;
        assert_eq!(
            reported_names(&page),
            ["events"],
            "case CatalogName {catalog:?}: expected the database's one table, got {:?}",
            reported_names(&page)
        );
    }
}

/// A data catalog a client REGISTERS decides where its metadata comes from: a `GLUE`
/// catalog naming the Glue catalog this account holds reads the same tables under a
/// second name, while a catalog served by an external metastore, a connector function or
/// another account's catalog is refused — and each refusal names the obstacle it hit: the
/// catalog id a Glue catalog of another account points at, and the catalog name of one
/// whose metastore is not hosted here.
#[tokio::test]
async fn a_registered_catalog_reads_the_catalog_its_parameters_name() {
    let router = emulator();
    seed(
        &router,
        &json!({ "Name": "registered_db" }),
        &[json!({ "Name": "events" }), json!({ "Name": "orders" })],
    )
    .await;
    // The `catalog-id` a GLUE data catalog is registered with: Glue's own default, the
    // account id, which is the catalog this emulator holds.
    let account = engine::emulator_account_id().to_string();

    // (the case, the CreateDataCatalog request, the names the listing then reports, or
    // `None` when the read must be refused — and then the fragment the refusal's message
    // must name, which is the obstacle it reports: the CATALOG ID a Glue catalog of
    // another account points at, and the CATALOG NAME of one whose metastore is not
    // hosted here)
    let rows: Vec<(&str, Value, Option<Vec<&str>>, &str)> = vec![
        (
            "a GLUE catalog naming this account's catalog id",
            json!({
                "Name": "second_name_for_glue",
                "Type": "GLUE",
                "Parameters": { "catalog-id": account }
            }),
            Some(vec!["events", "orders"]),
            "",
        ),
        (
            "a GLUE catalog naming another account's catalog id",
            json!({
                "Name": "other_account_glue",
                "Type": "GLUE",
                "Parameters": { "catalog-id": "210987654321" }
            }),
            None,
            // The catalog it names IS a Glue Data Catalog — another account's, which this
            // emulator does not hold — so the refusal names that catalog id rather than
            // claiming an external metastore serves it.
            "210987654321",
        ),
        (
            "a HIVE catalog, whose metadata an external metastore serves",
            json!({
                "Name": "hive_metastore",
                "Type": "HIVE",
                "Parameters": { "metadata-function": "arn:aws:lambda:us-east-1:000000000000:function:hms" }
            }),
            None,
            "hive_metastore",
        ),
        (
            "a LAMBDA catalog, whose metadata a connector function serves",
            json!({
                "Name": "lambda_connector",
                "Type": "LAMBDA",
                "Parameters": { "function": "arn:aws:lambda:us-east-1:000000000000:function:connector" }
            }),
            None,
            "lambda_connector",
        ),
    ];

    for (case, create, expected, fragment) in rows {
        athena_ok(&router, "CreateDataCatalog", &create).await;
        let catalog = create["Name"].as_str().expect("the case names a catalog");
        let request = json!({ "CatalogName": catalog, "DatabaseName": "registered_db" });
        match expected {
            Some(expected) => {
                let page = athena_ok(&router, "ListTableMetadata", &request).await;
                assert_eq!(
                    reported_names(&page),
                    expected,
                    "case {case:?}: expected {expected:?}, got {:?}",
                    reported_names(&page)
                );
                // The single-table read resolves the catalog the same way.
                let got = athena_ok(
                    &router,
                    "GetTableMetadata",
                    &json!({
                        "CatalogName": catalog,
                        "DatabaseName": "registered_db",
                        "TableName": "events"
                    }),
                )
                .await;
                assert_eq!(
                    got["TableMetadata"]["Name"],
                    json!("events"),
                    "case {case:?}: expected the stored table, got {got}"
                );
            }
            None => {
                for (operation, request) in [
                    ("ListTableMetadata", request.clone()),
                    (
                        "GetTableMetadata",
                        json!({
                            "CatalogName": catalog,
                            "DatabaseName": "registered_db",
                            "TableName": "events"
                        }),
                    ),
                ] {
                    let (status, body) = athena(&router, operation, &request).await;
                    assert_eq!(
                        status, 400,
                        "case {case:?}: {operation} must be refused, got {status} with {body}"
                    );
                    assert!(
                        error_code(&body).contains("MetadataException"),
                        "case {case:?}: {operation} must answer the metastore's error, got {}",
                        error_code(&body)
                    );
                    assert!(
                        error_message(&body).contains(fragment),
                        "case {case:?}: the message must name {fragment:?}, got {:?}",
                        error_message(&body)
                    );
                }
            }
        }
    }
}

/// A table created AFTER a first read is reported by the next one, and a table the
/// catalog dropped stops being reported: both reads answer out of Glue's store rather
/// than a snapshot of their own.
#[tokio::test]
async fn the_reads_follow_the_catalog() {
    let router = emulator();
    seed(
        &router,
        &json!({ "Name": "moving_db" }),
        &[json!({ "Name": "first" })],
    )
    .await;

    let before = athena_ok(
        &router,
        "ListTableMetadata",
        &json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": "moving_db" }),
    )
    .await;
    assert_eq!(
        reported_names(&before),
        ["first"],
        "the first listing must report the one stored table, got {:?}",
        reported_names(&before)
    );

    glue_ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "moving_db",
            "TableInput": { "Name": "second", "TableType": "EXTERNAL_TABLE" }
        }),
    )
    .await;

    let after = athena_ok(
        &router,
        "ListTableMetadata",
        &json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": "moving_db" }),
    )
    .await;
    assert_eq!(
        reported_names(&after),
        ["first", "second"],
        "a table created after the first listing must be reported by the next one, got {:?}",
        reported_names(&after)
    );
    let second = athena_ok(
        &router,
        "GetTableMetadata",
        &json!({
            "CatalogName": GLUE_CATALOG,
            "DatabaseName": "moving_db",
            "TableName": "second"
        }),
    )
    .await;
    assert_eq!(
        second["TableMetadata"]["TableType"],
        json!("EXTERNAL_TABLE"),
        "the single-table read must report the type the catalog holds, got {second}"
    );

    glue_ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": "moving_db", "Name": "first" }),
    )
    .await;

    let dropped = athena_ok(
        &router,
        "ListTableMetadata",
        &json!({ "CatalogName": GLUE_CATALOG, "DatabaseName": "moving_db" }),
    )
    .await;
    assert_eq!(
        reported_names(&dropped),
        ["second"],
        "a table the catalog dropped must stop being reported, got {:?}",
        reported_names(&dropped)
    );
    let (status, body) = athena(
        &router,
        "GetTableMetadata",
        &json!({
            "CatalogName": GLUE_CATALOG,
            "DatabaseName": "moving_db",
            "TableName": "first"
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "reading the dropped table must be refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("MetadataException"),
        "expected MetadataException for the dropped table, got {}",
        error_code(&body)
    );
}
