// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue Data Catalog slice: `CreateDatabase`, `GetDatabase`,
//! `CreateTable`, `GetTable`, `GetTables`, `DeleteTable` and `DeleteDatabase`.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front
//! door uses — so what is asserted here is the HTTP behaviour and not a direct call
//! into a handler.
//!
//! The suites are tables: each row is one input along the axis the rule generalises
//! over (member set, name casing, table count, page size, pattern shape), and the
//! recorded bootstrap case is one row among them.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::{DatabaseInput, TableInput};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DatabaseEntry, TableEntry, MAX_DATABASES_PER_CATALOG,
    MAX_STORED_TABLES, MAX_TABLES_PER_DATABASE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    assert_echoed, call, emulator, emulator_on, error_code, error_message, ok, table_names,
    ACCOUNT_ID, CALLER_PRINCIPAL, REGION,
};

/// A `CreateDatabase` body for `name`, with no optional members beyond the name.
fn database_body(name: &str) -> Value {
    json!({ "DatabaseInput": { "Name": name } })
}

/// Create `name` as a database and assert it succeeded.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    ok(router, "CreateDatabase", &database_body(name)).await;
}

/// Create `table` in `database` with a minimal `TableInput`.
async fn create_table(router: &MultiServiceRouter, database: &str, table: &str) {
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": { "Name": table } }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Create -> read round trip
// ---------------------------------------------------------------------------

/// Whatever member set a client sends to `CreateDatabase` / `CreateTable` is what
/// `GetDatabase` / `GetTable` read back, beside the members only the server can set.
///
/// The rows differ along the member set sent: the bare required name, the bootstrap
/// issue's database + external table, and a view-shaped table with a fuller
/// `StorageDescriptor` (serde, bucketing, sort order, parameters).
#[tokio::test]
async fn created_resources_are_read_back_from_the_store() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "required members only",
            json!({ "Name": "minimal_db" }),
            json!({ "Name": "minimal_table" }),
        ),
        (
            "external table with columns and partition keys",
            json!({
                "Name": "bootstrap_db",
                "Description": "demo",
                "LocationUri": "s3://bucket/db/",
                "Parameters": { "owner": "data-eng" }
            }),
            json!({
                "Name": "events",
                "TableType": "EXTERNAL_TABLE",
                "Owner": "data-eng",
                "Retention": 7,
                "Description": "raw events",
                "StorageDescriptor": {
                    "Columns": [
                        { "Name": "id", "Type": "string" },
                        { "Name": "amount", "Type": "double", "Comment": "cents" }
                    ],
                    "Location": "s3://bucket/events/",
                    "InputFormat": "org.apache.hadoop.mapred.TextInputFormat"
                },
                "PartitionKeys": [ { "Name": "dt", "Type": "string" } ]
            }),
        ),
        (
            "fully described storage",
            json!({
                "Name": "Warehouse_DB",
                "Description": "mixed case name, folded on store",
                "LocationUri": "s3://bucket/warehouse/"
            }),
            json!({
                "Name": "Sales_By_Day",
                "TableType": "GOVERNED",
                "Retention": 0,
                "Parameters": { "classification": "parquet", "compressionType": "snappy" },
                "ViewOriginalText": "SELECT 1",
                "ViewExpandedText": "SELECT 1 FROM dual",
                "StorageDescriptor": {
                    "Columns": [ { "Name": "day", "Type": "date", "Parameters": { "k": "v" } } ],
                    "Location": "s3://bucket/warehouse/sales/",
                    "InputFormat": "org.apache.hadoop.mapred.SequenceFileInputFormat",
                    "OutputFormat": "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat",
                    "Compressed": true,
                    "NumberOfBuckets": 4,
                    "BucketColumns": [ "day" ],
                    "SortColumns": [ { "Column": "day", "SortOrder": 1 } ],
                    "SerdeInfo": {
                        "Name": "parquet",
                        "SerializationLibrary": "org.apache.hadoop.hive.ql.io.parquet.serde.ParquetHiveSerDe",
                        "Parameters": { "serialization.format": "1" }
                    },
                    "Parameters": { "skip.header.line.count": "1" }
                }
            }),
        ),
    ];

    for (case, database_input, table_input) in &cases {
        let database_name = database_input["Name"].as_str().expect("a database name");
        let folded_database = database_name.to_lowercase();
        let table_name = table_input["Name"].as_str().expect("a table name");
        let folded_table = table_name.to_lowercase();

        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": database_input }),
        )
        .await;
        ok(
            &router,
            "CreateTable",
            &json!({ "DatabaseName": database_name, "TableInput": table_input }),
        )
        .await;

        let fetched = ok(
            &router,
            "GetDatabase",
            &json!({ "Name": database_name }),
        )
        .await;
        let database = &fetched["Database"];

        let mut expected_database = database_input.clone();
        expected_database
            .as_object_mut()
            .expect("a structure")
            .remove("Name");
        assert_echoed(case, &expected_database, database);
        assert_eq!(
            database["Name"],
            json!(folded_database),
            "case {case}: Database.Name is folded to lowercase when stored, got {:?}",
            database["Name"]
        );
        assert_eq!(
            database["CatalogId"],
            json!(ACCOUNT_ID),
            "case {case}: CatalogId defaults to the account id, got {:?}",
            database["CatalogId"]
        );
        assert!(
            database["CreateTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: Database.CreateTime must be a populated timestamp, got {:?}",
            database["CreateTime"]
        );

        let fetched = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database_name, "Name": table_name }),
        )
        .await;
        let table = &fetched["Table"];

        let mut expected_table = table_input.clone();
        expected_table
            .as_object_mut()
            .expect("a structure")
            .remove("Name");
        assert_echoed(case, &expected_table, table);
        assert_eq!(
            table["Name"],
            json!(folded_table),
            "case {case}: Table.Name is folded to lowercase when stored, got {:?}",
            table["Name"]
        );
        assert_eq!(
            table["DatabaseName"],
            json!(folded_database),
            "case {case}: Table.DatabaseName is set by the server, got {:?}",
            table["DatabaseName"]
        );
        assert_eq!(
            table["CatalogId"],
            json!(ACCOUNT_ID),
            "case {case}: Table.CatalogId is set by the server, got {:?}",
            table["CatalogId"]
        );
        assert_eq!(
            table["VersionId"],
            json!("0"),
            "case {case}: the version a create makes is the first of the table's history, \
             \"0\", got {:?}",
            table["VersionId"]
        );
        assert_eq!(
            table["CreatedBy"],
            json!(CALLER_PRINCIPAL),
            "case {case}: Table.CreatedBy is the principal ARN the request speaks for, got {:?}",
            table["CreatedBy"]
        );
        assert!(
            table["CreateTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: Table.CreateTime must be a populated timestamp, got {:?}",
            table["CreateTime"]
        );
        assert_eq!(
            table["UpdateTime"], table["CreateTime"],
            "case {case}: an unmodified table's UpdateTime equals its CreateTime, got {:?} and {:?}",
            table["UpdateTime"], table["CreateTime"]
        );
    }
}

/// Reading a resource changes nothing a later caller sees: two identical `GetTable`
/// calls, with a `GetTables` between them, answer byte-for-byte the same table.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = emulator();
    create_database(&router, "idempotent_db").await;
    create_table(&router, "idempotent_db", "events").await;

    let first = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "idempotent_db", "Name": "events" }),
    )
    .await;
    ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "idempotent_db" }),
    )
    .await;
    let second = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "idempotent_db", "Name": "events" }),
    )
    .await;

    assert_eq!(
        first, second,
        "case repeated GetTable: a read must not change the resource; first {first}, second {second}"
    );
}

/// A database or table name is reachable by any casing, because the catalog folds the
/// name on both write and lookup — and two casings of one name are one resource.
#[tokio::test]
async fn names_are_folded_to_lowercase_on_store_and_lookup() {
    let router = emulator();
    ok(&router, "CreateDatabase", &database_body("Mixed_Case_DB")).await;
    ok(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": "MIXED_case_db", "TableInput": { "Name": "Events_Raw" } }),
    )
    .await;

    for case in ["Mixed_Case_DB", "mixed_case_db", "MIXED_CASE_DB", "mIxEd_CaSe_Db"] {
        let fetched = ok(&router, "GetDatabase", &json!({ "Name": case })).await;
        assert_eq!(
            fetched["Database"]["Name"],
            json!("mixed_case_db"),
            "case {case}: every casing resolves to the one folded database, got {:?}",
            fetched["Database"]["Name"]
        );

        let fetched = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": case, "Name": "EVENTS_raw" }),
        )
        .await;
        assert_eq!(
            fetched["Table"]["Name"],
            json!("events_raw"),
            "case {case}: every casing resolves to the one folded table, got {:?}",
            fetched["Table"]["Name"]
        );
    }

    let (status, body) = call(&router, "CreateDatabase", &database_body("MIXED_CASE_DB")).await;
    assert_eq!(
        status, 400,
        "case re-create under another casing: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("AlreadyExistsException"),
        "case re-create under another casing: expected AlreadyExistsException, got {}",
        error_code(&body)
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// `GetTables` pages over the tables actually stored: each page holds at most
/// `MaxResults` entries, the returned `NextToken` resumes exactly where the page
/// ended, and the pages concatenate to the full stored set with no gap or repeat.
///
/// The rows differ along the two axes the rule generalises over — how many tables are
/// stored, and how large a page is asked for — including a page size that divides the
/// count exactly, one that does not, and one larger than the count.
#[tokio::test]
async fn get_tables_pages_over_the_stored_tables() {
    let cases: [(usize, Option<i32>); 7] = [
        (0, Some(5)),
        (1, Some(1)),
        (3, Some(2)),
        (4, Some(2)),
        (7, Some(3)),
        (5, Some(100)),
        (6, None),
    ];

    for (stored, max_results) in cases {
        let case = format!("{stored} tables, MaxResults {max_results:?}");
        let router = emulator();
        create_database(&router, "paged_db").await;

        let mut expected: Vec<String> = Vec::new();
        for index in 0..stored {
            let name = format!("table_{index:02}");
            create_table(&router, "paged_db", &name).await;
            expected.push(name);
        }
        expected.sort();

        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "DatabaseName": "paged_db" });
            let request = body.as_object_mut().expect("a structure");
            if let Some(limit) = max_results {
                request.insert("MaxResults".to_string(), json!(limit));
            }
            if let Some(cursor) = &token {
                request.insert("NextToken".to_string(), json!(cursor));
            }

            let page = ok(&router, "GetTables", &body).await;
            let names = table_names(&page);
            pages += 1;

            if let Some(limit) = max_results {
                assert!(
                    names.len() <= usize::try_from(limit).expect("a positive page size"),
                    "case {case}: a page holds at most MaxResults entries, got {}",
                    names.len()
                );
            }

            collected.extend(names);
            token = page["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= stored + 1,
                "case {case}: paging must terminate, but it produced {pages} pages"
            );
        }

        assert_eq!(
            collected, expected,
            "case {case}: the pages must concatenate to every stored table in order, got {collected:?}"
        );

        let expected_pages = if stored == 0 {
            1
        } else {
            let limit = usize::try_from(max_results.unwrap_or(100)).expect("a positive page size");
            stored.div_ceil(limit)
        };
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// The page size a `GetTables` is served is the one its `MaxResults` asks for: a value
/// above the bound the member's modelled type declares is SERVED rather than refused, which
/// is what the service does — the common "one big page" listing answers with every matching
/// table and no `NextToken` — and a value that names no servable page at all is refused with
/// Glue's own `InvalidInputException`, the error class a Glue client can match, and never
/// with the cross-service `ValidationException`.
///
/// The rows differ along the page size asked for: each published bound of the modelled
/// range (1 and 100, from `smithy_query` on `GetTablesRequest$MaxResults`), one past the
/// upper bound (101), a page far larger than the catalog holds (1000), the largest value
/// the member's type can carry, and \u2014 on the refusal side \u2014 one past the lower bound (0)
/// and a negative size.
#[tokio::test]
async fn get_tables_serves_the_page_size_the_request_asks_for() {
    let router = emulator();
    create_database(&router, "bounds_db").await;
    let stored = ["alpha", "beta", "delta", "events", "gamma"];
    for table in stored {
        create_table(&router, "bounds_db", table).await;
    }

    // (the page size asked for, how many tables the first page must carry)
    let served: [(i64, usize); 5] = [
        (1, 1),
        (100, stored.len()),
        (101, stored.len()),
        (1000, stored.len()),
        (i64::from(i32::MAX), stored.len()),
    ];
    for (page_size, expected) in served {
        let (status, body) = call(
            &router,
            "GetTables",
            &json!({ "DatabaseName": "bounds_db", "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults {page_size}: the page size a request asks for is served, got \
             {status} with {body}"
        );
        let page = body["TableList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            page, expected,
            "case MaxResults {page_size}: expected {expected} tables on the page, got {page} \
             in {body}"
        );
        // A page that held every matching table is the last one, so it carries no cursor.
        let has_token = !body["NextToken"].is_null();
        assert_eq!(
            has_token,
            expected < stored.len(),
            "case MaxResults {page_size}: NextToken is present only on a non-final page, got \
             {body}"
        );
    }

    for refused in [0, -1] {
        let (status, body) = call(
            &router,
            "GetTables",
            &json!({ "DatabaseName": "bounds_db", "MaxResults": refused }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults {refused}: a page size no listing can serve must be refused, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case MaxResults {refused}: expected Glue's InvalidInputException, got {}",
            error_code(&body)
        );
    }
}

// ---------------------------------------------------------------------------
// Expression
// ---------------------------------------------------------------------------

/// `GetTables`'s `Expression` is "A regular expression pattern. If present, only those
/// tables whose names match the pattern are returned": it selects the tables whose WHOLE
/// name matches the value as a regular expression, in which `*` stands for any run of
/// characters (so `events*` selects the `events`-prefixed tables and a bare `*` selects
/// everything).
///
/// The rows differ along the shape of the pattern: a literal, a prefix, a suffix, a
/// wildcard on both sides, an alternation, the match-everything pattern, the empty
/// pattern, a bare prefix (no match, because the name is matched in full), a pattern in
/// another case (no match, because the comparison is case-sensitive) and the regex
/// metacharacters — a `.` any-character, a `?` optional character, a character class, a
/// group and a `+` repetition — each of which keeps its regular-expression meaning.
#[tokio::test]
async fn get_tables_filters_by_the_expression_pattern() {
    let router = emulator();
    create_database(&router, "filtered_db").await;
    // Not the bootstrap issue's names alone: the set spans separators and lengths.
    for table in [
        "events",
        "events_raw",
        "orders",
        "clicks",
        "user_profile",
        "ev",
        "ev_one",
        "ev_two",
    ] {
        create_table(&router, "filtered_db", table).await;
    }

    let cases: [(&str, &[&str]); 24] = [
        ("events", &["events"]),
        // `*` is a wildcard, not a repetition of the character before it.
        ("ev*", &["ev", "ev_one", "ev_two", "events", "events_raw"]),
        ("events*", &["events", "events_raw"]),
        ("*s", &["clicks", "events", "orders"]),
        ("*_*", &["ev_one", "ev_two", "events_raw", "user_profile"]),
        (
            "*e*",
            &[
                "ev",
                "ev_one",
                "ev_two",
                "events",
                "events_raw",
                "orders",
                "user_profile",
            ],
        ),
        ("orders|clicks", &["clicks", "orders"]),
        ("events|user*", &["events", "user_profile"]),
        (
            "*",
            &[
                "clicks",
                "ev",
                "ev_one",
                "ev_two",
                "events",
                "events_raw",
                "orders",
                "user_profile",
            ],
        ),
        // A whole-name match, so a bare prefix matches nothing, and so does the empty
        // pattern (which the model's `FilterString` length 0..2048 allows).
        ("event", &[]),
        ("", &[]),
        // Every other character keeps its regular-expression meaning: `.` stands for one
        // character, `?` makes the character before it optional, `[…]` is a class, `(…)`
        // groups and `+` repeats.
        ("ev.*", &["ev_one", "ev_two", "events", "events_raw"]),
        ("ev_.*", &["ev_one", "ev_two"]),
        ("ev?", &["ev"]),
        ("events.*", &["events_raw"]),
        (
            "[a-z]*",
            &[
                "clicks",
                "ev",
                "ev_one",
                "ev_two",
                "events",
                "events_raw",
                "orders",
                "user_profile",
            ],
        ),
        ("(events)", &["events"]),
        ("ev+", &["ev"]),
        ("e(v|x)ents", &["events"]),
        // The comparison is case-sensitive, and a stored name is folded to lowercase.
        ("EV_ONE", &[]),
        ("EVENTS*", &[]),
        ("[A-Z]*", &[]),
        // A `*` that is escaped or inside a character class is the expression's own
        // literal star, not the wildcard: `ev\*_one` therefore matches the name
        // "ev*_one", which nothing stored carries — had the wildcard substitution
        // reached inside the escape it would have matched `ev_one` — and `e[*]vents`
        // matches "e*vents".
        (r"ev\*_one", &[]),
        (r"e[*]vents", &[]),
    ];

    for (expression, expected) in cases {
        let page = ok(
            &router,
            "GetTables",
            &json!({ "DatabaseName": "filtered_db", "Expression": expression }),
        )
        .await;
        assert_eq!(
            table_names(&page),
            expected,
            "case Expression {expression:?}: expected {expected:?}, got {:?}",
            table_names(&page)
        );
    }

    // A value that is no regular expression at all cannot select anything, so it is
    // refused rather than quietly answered with an empty catalog.
    for malformed in ["ev[", "(events", "a{2,1}"] {
        let (status, body) = call(
            &router,
            "GetTables",
            &json!({ "DatabaseName": "filtered_db", "Expression": malformed }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case Expression {malformed:?}: a malformed pattern must be refused, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case Expression {malformed:?}: expected InvalidInputException, got {}",
            error_code(&body)
        );
    }

    // The pattern is request text matched against every stored name, so a pattern whose
    // matcher would be larger than this operation compiles
    // (`MAX_PATTERN_PROGRAM_BYTES`) is refused too.
    let oversized = format!("(?:{}){{200}}", "[a-z0-9]{200}");
    let (status, body) = call(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "filtered_db", "Expression": oversized }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case Expression over the compiled-size ceiling: must be refused, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case Expression over the compiled-size ceiling: expected InvalidInputException, \
         got {}",
        error_code(&body)
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// Delete removes the record: the resource is gone from every reader afterwards, and
/// deleting a database takes the tables it contains with it.
#[tokio::test]
async fn delete_removes_the_record_and_cascades_to_tables() {
    let router = emulator();
    create_database(&router, "doomed_db").await;
    for table in ["events", "orders", "clicks"] {
        create_table(&router, "doomed_db", table).await;
    }

    ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": "doomed_db", "Name": "events" }),
    )
    .await;

    let (status, body) = call(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "doomed_db", "Name": "events" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case GetTable after DeleteTable: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case GetTable after DeleteTable: expected EntityNotFoundException, got {}",
        error_code(&body)
    );

    let page = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "doomed_db" }),
    )
    .await;
    assert_eq!(
        table_names(&page),
        vec!["clicks".to_string(), "orders".to_string()],
        "case GetTables after DeleteTable: the deleted table must be gone, got {:?}",
        table_names(&page)
    );

    ok(&router, "DeleteDatabase", &json!({ "Name": "doomed_db" })).await;

    for (operation, body) in [
        ("GetDatabase", json!({ "Name": "doomed_db" })),
        ("GetTables", json!({ "DatabaseName": "doomed_db" })),
        (
            "GetTable",
            json!({ "DatabaseName": "doomed_db", "Name": "orders" }),
        ),
    ] {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {operation} after DeleteDatabase: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {operation} after DeleteDatabase: expected EntityNotFoundException, got {}",
            error_code(&answer)
        );
    }

    // The cascade must not resurrect anything: re-creating the database leaves it empty.
    create_database(&router, "doomed_db").await;
    let page = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "doomed_db" }),
    )
    .await;
    assert!(
        table_names(&page).is_empty(),
        "case re-created database: the cascaded tables must not come back, got {:?}",
        table_names(&page)
    );
}

// ---------------------------------------------------------------------------
// Modelled failures
// ---------------------------------------------------------------------------

/// Addressing a resource that is not stored is `EntityNotFoundException`, and creating
/// one that already is is `AlreadyExistsException` — both HTTP 400. The rows span every
/// operation of the slice and both axes of absence (the catalog and the parent database).
#[tokio::test]
async fn absent_and_duplicate_resources_answer_the_modelled_errors() {
    let router = emulator();
    create_database(&router, "present_db").await;
    create_table(&router, "present_db", "events").await;

    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "duplicate database",
            "CreateDatabase",
            database_body("present_db"),
            "AlreadyExistsException",
        ),
        (
            "duplicate table",
            "CreateTable",
            json!({ "DatabaseName": "present_db", "TableInput": { "Name": "events" } }),
            "AlreadyExistsException",
        ),
        (
            "unknown database",
            "GetDatabase",
            json!({ "Name": "absent_db" }),
            "EntityNotFoundException",
        ),
        (
            "known database in another account's catalog",
            "GetDatabase",
            json!({ "CatalogId": "111111111111", "Name": "present_db" }),
            "AccessDeniedException",
        ),
        (
            "table in an unknown database",
            "CreateTable",
            json!({ "DatabaseName": "absent_db", "TableInput": { "Name": "events" } }),
            "EntityNotFoundException",
        ),
        (
            "unknown table",
            "GetTable",
            json!({ "DatabaseName": "present_db", "Name": "absent_table" }),
            "EntityNotFoundException",
        ),
        (
            "table of an unknown database",
            "GetTable",
            json!({ "DatabaseName": "absent_db", "Name": "events" }),
            "EntityNotFoundException",
        ),
        (
            "tables of an unknown database",
            "GetTables",
            json!({ "DatabaseName": "absent_db" }),
            "EntityNotFoundException",
        ),
        (
            "delete an unknown table",
            "DeleteTable",
            json!({ "DatabaseName": "present_db", "Name": "absent_table" }),
            "EntityNotFoundException",
        ),
        (
            "delete a table of an unknown database",
            "DeleteTable",
            json!({ "DatabaseName": "absent_db", "Name": "events" }),
            "EntityNotFoundException",
        ),
        (
            "delete an unknown database",
            "DeleteDatabase",
            json!({ "Name": "absent_db" }),
            "EntityNotFoundException",
        ),
        (
            "create a table without a TableInput",
            "CreateTable",
            json!({ "DatabaseName": "present_db" }),
            "InvalidInputException",
        ),
    ];

    for (case, operation, body, expected) in &cases {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains(expected),
            "case {case}: expected {expected}, got {}",
            error_code(&answer)
        );
    }

    // A refused create must not have written anything.
    let page = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "present_db" }),
    )
    .await;
    assert_eq!(
        table_names(&page),
        vec!["events".to_string()],
        "case refused creates: a refusal must leave the store untouched, got {:?}",
        table_names(&page)
    );
}

/// `CatalogId` selects the Data Catalog, and this emulator holds exactly one: the one
/// for its own account, which is the catalog the model documents as the default. So
/// naming that id explicitly is the same as omitting it, and naming ANY other id
/// addresses another account's catalog: reachable only through that catalog's own
/// resource-based policy, which this store holds none of, so every operation of the
/// slice refuses it as unauthorized — never a new, empty catalog conjured by the create,
/// and never an answer served out of the caller's own catalog.
///
/// The rows span both axes: how the own catalog is addressed (omitted vs named), and
/// every operation reached with a foreign id. The own catalog holds a database of the
/// SAME name throughout, so a row that answered out of it would be visible here.
#[tokio::test]
async fn catalog_id_selects_the_one_catalog_this_emulator_holds() {
    let router = emulator();

    // Addressing the own catalog explicitly and implicitly reaches the same resource.
    ok(
        &router,
        "CreateDatabase",
        &json!({ "CatalogId": ACCOUNT_ID, "DatabaseInput": { "Name": "shared_name" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": "shared_name", "TableInput": { "Name": "events" } }),
    )
    .await;

    for (case, catalog_id) in [
        ("CatalogId omitted", None),
        ("CatalogId named explicitly", Some(ACCOUNT_ID)),
    ] {
        let with_catalog = |mut request: Value| {
            if let Some(catalog) = catalog_id {
                request
                    .as_object_mut()
                    .expect("a structure")
                    .insert("CatalogId".to_string(), json!(catalog));
            }
            request
        };

        let page = ok(
            &router,
            "GetTables",
            &with_catalog(json!({ "DatabaseName": "shared_name" })),
        )
        .await;
        assert_eq!(
            table_names(&page),
            vec!["events".to_string()],
            "case {case}: both ways of addressing the own catalog reach it, got {:?}",
            table_names(&page)
        );

        let fetched = ok(
            &router,
            "GetDatabase",
            &with_catalog(json!({ "Name": "shared_name" })),
        )
        .await;
        assert_eq!(
            fetched["Database"]["CatalogId"],
            json!(ACCOUNT_ID),
            "case {case}: Database.CatalogId reports the catalog it lives in, got {:?}",
            fetched["Database"]["CatalogId"]
        );
    }

    // A catalog id of another account is refused as unauthorized everywhere, before any
    // lookup: the authorization decision does not depend on what the own catalog holds.
    let foreign = "222222222222";
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "CreateDatabase in a foreign catalog",
            "CreateDatabase",
            json!({ "CatalogId": foreign, "DatabaseInput": { "Name": "shared_name" } }),
            "AccessDeniedException",
        ),
        (
            "CreateTable in a foreign catalog",
            "CreateTable",
            json!({
                "CatalogId": foreign,
                "DatabaseName": "shared_name",
                "TableInput": { "Name": "events" }
            }),
            "AccessDeniedException",
        ),
        (
            "GetDatabase in a foreign catalog",
            "GetDatabase",
            json!({ "CatalogId": foreign, "Name": "shared_name" }),
            "AccessDeniedException",
        ),
        (
            "GetTable in a foreign catalog",
            "GetTable",
            json!({ "CatalogId": foreign, "DatabaseName": "shared_name", "Name": "events" }),
            "AccessDeniedException",
        ),
        (
            "GetTables in a foreign catalog",
            "GetTables",
            json!({ "CatalogId": foreign, "DatabaseName": "shared_name" }),
            "AccessDeniedException",
        ),
        (
            "DeleteTable in a foreign catalog",
            "DeleteTable",
            json!({ "CatalogId": foreign, "DatabaseName": "shared_name", "Name": "events" }),
            "AccessDeniedException",
        ),
        (
            "DeleteDatabase in a foreign catalog",
            "DeleteDatabase",
            json!({ "CatalogId": foreign, "Name": "shared_name" }),
            "AccessDeniedException",
        ),
    ];

    for (case, operation, body, expected) in &cases {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains(expected),
            "case {case}: expected {expected}, got {}",
            error_code(&answer)
        );
        // The refusal is built from the request and this emulator's own identity: the
        // calling principal, the IAM action the operation performs, and the ARN of the
        // catalog the request addressed.
        let message = error_message(&answer);
        for part in [
            CALLER_PRINCIPAL,
            &format!("glue:{operation}"),
            &format!("arn:aws:glue:{REGION}:{foreign}:catalog"),
        ] {
            assert!(
                message.contains(part),
                "case {case}: the refusal must name {part:?}, got {message:?}"
            );
        }
    }

    // And the own catalog still holds exactly what it held: nothing was created under
    // the foreign id and nothing was deleted from this one.
    let page = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "shared_name" }),
    )
    .await;
    assert_eq!(
        table_names(&page),
        vec!["events".to_string()],
        "case after the foreign-catalog calls: the own catalog is untouched, got {:?}",
        table_names(&page)
    );
}

// ---------------------------------------------------------------------------
// Members this slice does not serve
// ---------------------------------------------------------------------------

/// A member this slice cannot serve faithfully is REFUSED, never accepted and
/// dropped: answering 200 for work nothing did is the outcome that makes a passing
/// test misleading.
///
/// The rows span every such member of the slice. Governed/transactional and
/// point-in-time reads (`TransactionId`, `QueryAsOfTime`) and a transactional write
/// answer 501 `NotImplemented` — "nobody
/// has written it yet" — while the one combination the model forbids outright
/// (`QueryAsOfTime` together with `TransactionId`) answers `InvalidInputException`,
/// because that is a client error whatever else is supported.
#[tokio::test]
async fn unsupported_members_are_refused_not_silently_dropped() {
    let router = emulator();
    create_database(&router, "guarded_db").await;
    create_table(&router, "guarded_db", "events").await;

    let unimplemented: Vec<(&str, &str, Value)> = vec![
        (
            "CreateTable with a ViewDefinition",
            "CreateTable",
            json!({
                "DatabaseName": "guarded_db",
                "TableInput": {
                    "Name": "dialect_view",
                    "TableType": "VIRTUAL_VIEW",
                    "ViewDefinition": {
                        "Representations": [
                            {
                                "Dialect": "ATHENA",
                                "DialectVersion": "3",
                                "ViewOriginalText": "SELECT 1"
                            }
                        ]
                    }
                }
            }),
        ),
        (
            "CreateTable with TransactionId",
            "CreateTable",
            json!({
                "DatabaseName": "guarded_db",
                "TableInput": { "Name": "governed_table" },
                "TransactionId": "txn-1"
            }),
        ),
        (
            "DeleteTable with TransactionId",
            "DeleteTable",
            json!({ "DatabaseName": "guarded_db", "Name": "events", "TransactionId": "txn-1" }),
        ),
        (
            "GetTable with TransactionId",
            "GetTable",
            json!({ "DatabaseName": "guarded_db", "Name": "events", "TransactionId": "txn-1" }),
        ),
        (
            "GetTable with QueryAsOfTime",
            "GetTable",
            json!({ "DatabaseName": "guarded_db", "Name": "events", "QueryAsOfTime": 1_700_000_000 }),
        ),
        (
            "GetTables with TransactionId",
            "GetTables",
            json!({ "DatabaseName": "guarded_db", "TransactionId": "txn-1" }),
        ),
        (
            "GetTables with QueryAsOfTime",
            "GetTables",
            json!({ "DatabaseName": "guarded_db", "QueryAsOfTime": 1_700_000_000 }),
        ),
    ];

    for (case, operation, body) in &unimplemented {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 501,
            "case {case}: an unserved member must answer 501 NotImplemented, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("NotImplemented"),
            "case {case}: expected NotImplemented, got {}",
            error_code(&answer)
        );
    }

    // The model states QueryAsOfTime "cannot be specified along with TransactionId".
    let forbidden_pair: Vec<(&str, &str, Value)> = vec![
        (
            "GetTable with both QueryAsOfTime and TransactionId",
            "GetTable",
            json!({
                "DatabaseName": "guarded_db",
                "Name": "events",
                "TransactionId": "txn-1",
                "QueryAsOfTime": 1_700_000_000
            }),
        ),
        (
            "GetTables with both QueryAsOfTime and TransactionId",
            "GetTables",
            json!({
                "DatabaseName": "guarded_db",
                "TransactionId": "txn-1",
                "QueryAsOfTime": 1_700_000_000
            }),
        ),
    ];

    for (case, operation, body) in &forbidden_pair {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: the forbidden combination must answer 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {}",
            error_code(&answer)
        );
    }

    // None of the refusals wrote anything, and the database still reads normally.
    let page = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "guarded_db" }),
    )
    .await;
    assert_eq!(
        table_names(&page),
        vec!["events".to_string()],
        "case after every refusal: a refused request must leave the store untouched, got {:?}",
        table_names(&page)
    );

    // `IncludeStatusDetails: false` asks for nothing this slice lacks, so it is served.
    for (case, operation, body) in [
        (
            "GetTable with IncludeStatusDetails false",
            "GetTable",
            json!({
                "DatabaseName": "guarded_db",
                "Name": "events",
                "IncludeStatusDetails": false
            }),
        ),
        (
            "GetTables with IncludeStatusDetails false",
            "GetTables",
            json!({ "DatabaseName": "guarded_db", "IncludeStatusDetails": false }),
        ),
    ] {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: not asking for status details must be served, got {status} with {answer}"
        );
    }
}

/// `AttributesToGet` selects what comes back, and the model pins both the valid
/// combinations and the two refusals ("doesn't accept an empty list"; for `GetTables`
/// "must include NAME").
///
/// The rows are every documented combination plus the refusals, for both readers.
#[tokio::test]
async fn attributes_to_get_projects_the_answer_and_enforces_its_combinations() {
    let router = emulator();
    create_database(&router, "projected_db").await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "projected_db",
            "TableInput": {
                "Name": "events",
                "TableType": "EXTERNAL_TABLE",
                "Owner": "data-eng",
                "StorageDescriptor": { "Location": "s3://bucket/events/" }
            }
        }),
    )
    .await;

    // GetTables: NAME alone, then NAME with TABLE_TYPE.
    let page = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": "projected_db", "AttributesToGet": [ "NAME" ] }),
    )
    .await;
    let table = &page["TableList"][0];
    assert_eq!(
        table["Name"],
        json!("events"),
        "case AttributesToGet [NAME]: the name is returned, got {table}"
    );
    for omitted in ["TableType", "Owner", "StorageDescriptor", "CreateTime"] {
        assert!(
            table.get(omitted).is_none_or(Value::is_null),
            "case AttributesToGet [NAME]: {omitted} must not be projected, got {table}"
        );
    }

    let page = ok(
        &router,
        "GetTables",
        &json!({
            "DatabaseName": "projected_db",
            "AttributesToGet": [ "NAME", "TABLE_TYPE" ]
        }),
    )
    .await;
    let table = &page["TableList"][0];
    assert_eq!(
        (table["Name"].clone(), table["TableType"].clone()),
        (json!("events"), json!("EXTERNAL_TABLE")),
        "case AttributesToGet [NAME, TABLE_TYPE]: both are returned, got {table}"
    );
    assert!(
        table.get("Owner").is_none_or(Value::is_null),
        "case AttributesToGet [NAME, TABLE_TYPE]: Owner must not be projected, got {table}"
    );

    // GetTable: DEFAULT is the Hive-style definition this slice stores.
    let fetched = ok(
        &router,
        "GetTable",
        &json!({
            "DatabaseName": "projected_db",
            "Name": "events",
            "AttributesToGet": [ "DEFAULT" ]
        }),
    )
    .await;
    assert_eq!(
        fetched["Table"]["Owner"],
        json!("data-eng"),
        "case AttributesToGet [DEFAULT]: the whole Hive-style definition is returned, got {:?}",
        fetched["Table"]
    );

    // The refusals the model states, and the Iceberg half this slice does not keep.
    let refused: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "GetTables with an empty AttributesToGet",
            "GetTables",
            json!({ "DatabaseName": "projected_db", "AttributesToGet": [] }),
            400,
            "InvalidInputException",
        ),
        (
            "GetTables without NAME",
            "GetTables",
            json!({ "DatabaseName": "projected_db", "AttributesToGet": [ "TABLE_TYPE" ] }),
            400,
            "InvalidInputException",
        ),
        (
            "GetTable with an empty AttributesToGet",
            "GetTable",
            json!({
                "DatabaseName": "projected_db",
                "Name": "events",
                "AttributesToGet": []
            }),
            400,
            "InvalidInputException",
        ),
        (
            "GetTable with a combination it does not document",
            "GetTable",
            json!({
                "DatabaseName": "projected_db",
                "Name": "events",
                "AttributesToGet": [ "NAME" ]
            }),
            400,
            "InvalidInputException",
        ),
        (
            "GetTable asking for the latest Iceberg metadata",
            "GetTable",
            json!({
                "DatabaseName": "projected_db",
                "Name": "events",
                "AttributesToGet": [ "DEFAULT", "LATEST_ICEBERG_METADATA" ]
            }),
            501,
            "NotImplemented",
        ),
    ];

    for (case, operation, body, expected_status, expected_code) in &refused {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, *expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains(expected_code),
            "case {case}: expected {expected_code}, got {}",
            error_code(&answer)
        );
    }
}

/// `ResourceShareType` selects which tables `GetTables` returns: per the model,
/// `FEDERATED` returns "only federated tables, which reference an entity outside the
/// Glue Data Catalog" and `ALL` returns "all tables in the database, both federated
/// and non-federated". A stored table is federated exactly when it carries a
/// `FederatedTable`.
#[tokio::test]
async fn get_tables_resource_share_type_selects_federated_tables() {
    let router = emulator();
    create_database(&router, "shared_db").await;
    create_table(&router, "shared_db", "local_events").await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "shared_db",
            "TableInput": {
                "Name": "federated_events",
                "FederatedTable": {
                    "Identifier": "arn:aws:redshift:us-east-1:000000000000:table/events",
                    "DatabaseIdentifier": "arn:aws:redshift:us-east-1:000000000000:database/dev",
                    "ConnectionName": "redshift",
                    "ConnectionType": "REDSHIFT"
                }
            }
        }),
    )
    .await;

    for (case, share_type, expected) in [
        ("omitted", None, vec!["federated_events", "local_events"]),
        ("ALL", Some("ALL"), vec!["federated_events", "local_events"]),
        ("FEDERATED", Some("FEDERATED"), vec!["federated_events"]),
    ] {
        let mut body = json!({ "DatabaseName": "shared_db" });
        if let Some(share_type) = share_type {
            body.as_object_mut()
                .expect("a structure")
                .insert("ResourceShareType".to_string(), json!(share_type));
        }
        let page = ok(&router, "GetTables", &body).await;
        assert_eq!(
            table_names(&page),
            expected,
            "case ResourceShareType {case}: expected {expected:?}, got {:?}",
            table_names(&page)
        );
    }
}

// ---------------------------------------------------------------------------
// Capacity ceilings
// ---------------------------------------------------------------------------

/// Seed `databases` empty databases, and `tables_each` tables in each of them, straight
/// into the store. Reaching a ceiling over the wire would take thousands of requests
/// and would exercise the transport rather than the ceiling.
fn seed(engine: &Arc<engine::InMemoryEngine>, databases: usize, tables_each: usize) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let catalog = store.catalogs.entry(ACCOUNT_ID.to_string()).or_default();
    for database_index in 0..databases {
        let database_name = format!("seeded_db_{database_index:05}");
        let database_input = DatabaseInput {
            name: database_name.clone(),
            ..DatabaseInput::default()
        };
        let mut entry =
            DatabaseEntry::from_input(&database_input, ACCOUNT_ID, None, SystemTime::now());
        for table_index in 0..tables_each {
            let table_name = format!("seeded_table_{table_index:05}");
            let table_input = TableInput {
                name: table_name.clone(),
                ..TableInput::default()
            };
            entry.tables.insert(
                table_name,
                TableEntry::from_input(
                    &table_input,
                    &database_name,
                    ACCOUNT_ID,
                    None,
                    SystemTime::now(),
                ),
            );
        }
        catalog.databases.insert(database_name, entry);
    }
}

/// The store is resident memory and a create is reachable by any caller, so the number
/// of records one process holds is capped. The last record under each ceiling is
/// accepted, the one past it is refused with the modelled
/// `ResourceNumberLimitExceededException`, and deleting a record frees the capacity
/// again — so the ceiling is a bound, not a one-way latch.
///
/// The rows are the three ceilings: databases per catalog, tables per database, and
/// tables store-wide.
#[tokio::test]
async fn a_create_past_a_capacity_ceiling_is_refused() {
    // Databases per catalog.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    seed(&engine, MAX_DATABASES_PER_CATALOG - 1, 0);

    ok(&router, "CreateDatabase", &database_body("last_db")).await;
    let (status, body) = call(&router, "CreateDatabase", &database_body("one_too_many")).await;
    assert_eq!(
        status, 400,
        "case database {} of {MAX_DATABASES_PER_CATALOG}: expected 400, got {status} with {body}",
        MAX_DATABASES_PER_CATALOG + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one database too many: expected ResourceNumberLimitExceededException, got {}",
        error_code(&body)
    );
    // Deleting one frees the capacity it held.
    ok(&router, "DeleteDatabase", &json!({ "Name": "last_db" })).await;
    ok(&router, "CreateDatabase", &database_body("one_too_many")).await;

    // Tables per database.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    seed(&engine, 1, MAX_TABLES_PER_DATABASE - 1);
    let seeded_db = "seeded_db_00000";

    create_table(&router, seeded_db, "last_table").await;
    let (status, body) = call(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": seeded_db, "TableInput": { "Name": "one_too_many" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case table {} of {MAX_TABLES_PER_DATABASE}: expected 400, got {status} with {body}",
        MAX_TABLES_PER_DATABASE + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one table too many: expected ResourceNumberLimitExceededException, got {}",
        error_code(&body)
    );
    ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": seeded_db, "Name": "last_table" }),
    )
    .await;
    create_table(&router, seeded_db, "one_too_many").await;

    // Tables store-wide, spread over databases that are each under their own ceiling.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let databases = MAX_STORED_TABLES / MAX_TABLES_PER_DATABASE;
    seed(&engine, databases, MAX_TABLES_PER_DATABASE);
    create_database(&router, "head_room").await;

    // A full store does not blur the other errors: a create naming a database that does
    // not exist still gets the precise not-found answer.
    let (status, body) = call(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": "absent_db", "TableInput": { "Name": "one_too_many" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case full store, unknown database: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case full store, unknown database: expected EntityNotFoundException, got {}",
        error_code(&body)
    );

    let (status, body) = call(
        &router,
        "CreateTable",
        &json!({ "DatabaseName": "head_room", "TableInput": { "Name": "one_too_many" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case table {} store-wide of {MAX_STORED_TABLES}: expected 400, got {status} with {body}",
        MAX_STORED_TABLES + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one table too many store-wide: expected ResourceNumberLimitExceededException, got {}",
        error_code(&body)
    );
    // Dropping a whole database frees every table it held.
    ok(
        &router,
        "DeleteDatabase",
        &json!({ "Name": "seeded_db_00000" }),
    )
    .await;
    create_table(&router, "head_room", "one_too_many").await;
}
