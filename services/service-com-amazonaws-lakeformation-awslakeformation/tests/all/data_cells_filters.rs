// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the data cells filter lifecycle: `CreateDataCellsFilter` and the
//! `GetDataCellsFilter` reader that proves the create persisted.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! The tables below span the classes the rules generalise over: how a filter names its
//! columns (an explicit list, or a wildcard with and without exclusions), how it names
//! its rows (a PartiQL expression, the all-rows wildcard, or no predicate at all), how
//! many columns it names, how deeply a nested column attribute nests, and which part of
//! the (catalog, database, table, name) identity differs.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lakeformation_awslakeformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator over one shared engine, reached exactly as the production router reaches
/// it: `MultiServiceRouter::dispatch` of a buffered HTTP request.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// `POST /<operation>` with `body` as the JSON payload, answering `(status, body)`.
///
/// The `Authorization` credential scope names `lakeformation`, which is the routing
/// signal the router reads first; no authenticator is registered, so the signature itself
/// is not checked.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://lakeformation.us-east-1.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/lakeformation/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// The wire error code (`__type`) of an error envelope, or `""` for a success body.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `GetDataCellsFilter` request that addresses the filter `table_data` describes.
fn locator(table_data: &Value) -> Value {
    json!({
        "TableCatalogId": table_data["TableCatalogId"],
        "DatabaseName": table_data["DatabaseName"],
        "TableName": table_data["TableName"],
        "Name": table_data["Name"],
    })
}

#[tokio::test]
async fn create_then_get_reads_the_stored_filter_back() {
    // Rows differ along what the rule generalises over: the column-naming form (explicit
    // list, bare wildcard, wildcard with exclusions), the number of columns, the
    // row-predicate form (expression, all-rows wildcard, no predicate), whether the
    // caller supplied a `VersionId`, and the modelled nesting depth bound of 5
    // ("Nested attributes within this list may not exceed a depth of 5." —
    // `smithy_query(query="doc",shape="DataCellsFilter",member="ColumnNames")`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "expression over named columns",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "us-only",
                "RowFilter": { "FilterExpression": "region = 'us-east-1'" },
                "ColumnNames": ["order_id", "region"],
            }),
        ),
        (
            "all rows over one column",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "all-rows",
                "RowFilter": { "AllRowsWildcard": {} },
                "ColumnNames": ["order_id"],
            }),
        ),
        (
            "bare column wildcard",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "every-column",
                "RowFilter": { "FilterExpression": "amount > 10" },
                "ColumnWildcard": {},
            }),
        ),
        (
            "column wildcard with exclusions",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "no-pii",
                "RowFilter": { "AllRowsWildcard": {} },
                "ColumnWildcard": { "ExcludedColumnNames": ["ssn", "email"] },
            }),
        ),
        (
            "no row predicate at all",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "columns-only",
                "ColumnNames": ["order_id", "amount", "region"],
            }),
        ),
        (
            "caller-supplied version id",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "versioned",
                "RowFilter": { "FilterExpression": "amount > 0" },
                "ColumnNames": ["amount"],
                "VersionId": "7",
            }),
        ),
        (
            "nested attribute at the documented depth bound",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "deep-nesting",
                "RowFilter": { "AllRowsWildcard": {} },
                "ColumnNames": ["\"a\".\"b\".\"c\".\"d\".\"e\"", "\"address\".\"zip\""],
            }),
        ),
        (
            // A dot INSIDE a quoted segment is part of that segment's name, not a level
            // separator, so this path nests two attributes and not three.
            "quoted dot inside one segment",
            json!({
                "TableCatalogId": "111122223333",
                "DatabaseName": "sales",
                "TableName": "orders",
                "Name": "quoted-dot",
                "ColumnNames": ["\"address.line\".\"zip\""],
            }),
        ),
        (
            "a different catalog, database and table",
            json!({
                "TableCatalogId": "444455556666",
                "DatabaseName": "marketing",
                "TableName": "leads",
                "Name": "us-only",
                "RowFilter": { "FilterExpression": "country = 'US'" },
                "ColumnWildcard": { "ExcludedColumnNames": ["phone"] },
            }),
        ),
    ];

    let router = emulator();
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "CreateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDataCellsFilter 200, got {status} {payload:?}"
        );
    }

    // Every filter reads back as exactly what its request named — the create invents no
    // member and drops none.
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "GetDataCellsFilter", &locator(table_data)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDataCellsFilter 200, got {status} {payload:?}"
        );
        let actual = payload.get("DataCellsFilter").cloned().unwrap_or(Value::Null);
        assert_eq!(
            &actual, table_data,
            "case {case:?}: expected the stored filter {table_data:?}, got {actual:?}"
        );
    }

    // A read changes nothing a later caller sees: the second read answers the first.
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "GetDataCellsFilter", &locator(table_data)).await;
        let actual = payload.get("DataCellsFilter").cloned().unwrap_or(Value::Null);
        assert_eq!(status, 200, "case {case:?}: expected a repeat read to answer 200, got {status} {payload:?}");
        assert_eq!(
            &actual, table_data,
            "case {case:?}: expected a repeat read to answer {table_data:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_is_identified_by_catalog_database_table_and_name() {
    // One filter NAME under four different identities is four different filters; the rows
    // differ in exactly one component of the identity each.
    let base = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "shared-name",
        "ColumnNames": ["order_id"],
    });
    let mut cases: Vec<(&str, Value)> = vec![("the base identity", base.clone())];
    for (case, member, value) in [
        ("a different catalog", "TableCatalogId", "444455556666"),
        ("a different database", "DatabaseName", "marketing"),
        ("a different table", "TableName", "returns"),
        ("a different filter name", "Name", "other-name"),
    ] {
        let mut variant = base.clone();
        variant[member] = json!(value);
        // Each variant gets its own column list so a lookup that confused two identities
        // would answer the wrong columns rather than coincidentally matching.
        variant["ColumnNames"] = json!([format!("col_{value}")]);
        cases.push((case, variant));
    }

    let router = emulator();
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "CreateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for a distinct identity, got {status} {payload:?}"
        );
    }
    for (case, table_data) in &cases {
        let (status, payload) = call(&router, "GetDataCellsFilter", &locator(table_data)).await;
        let actual = payload.get("DataCellsFilter").cloned().unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {payload:?}"
        );
        assert_eq!(
            &actual, table_data,
            "case {case:?}: expected {table_data:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn creating_an_existing_filter_is_refused_and_keeps_the_stored_one() {
    let router = emulator();
    let stored = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "us-only",
        "RowFilter": { "FilterExpression": "region = 'us-east-1'" },
        "ColumnNames": ["order_id", "region"],
    });
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": stored })).await;
    assert_eq!(
        status, 200,
        "case \"first create\": expected 200, got {status} {payload:?}"
    );

    // A second create of the same identity, with a different body, is refused — and the
    // refusal leaves the first filter in place rather than replacing it.
    let mut replacement = stored.clone();
    replacement["ColumnNames"] = json!(["order_id"]);
    replacement["RowFilter"] = json!({ "AllRowsWildcard": {} });
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": replacement })).await;
    assert_eq!(
        status, 400,
        "case \"duplicate create\": expected 400, got {status} {payload:?}"
    );
    let code = error_code(&payload);
    assert_eq!(
        code, "AlreadyExistsException",
        "case \"duplicate create\": expected AlreadyExistsException, got {code:?} {payload:?}"
    );

    let (status, payload) = call(&router, "GetDataCellsFilter", &locator(&stored)).await;
    let actual = payload.get("DataCellsFilter").cloned().unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case \"after duplicate create\": expected 200, got {status} {payload:?}"
    );
    assert_eq!(
        &actual, &stored,
        "case \"after duplicate create\": expected the first filter {stored:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn a_filter_the_store_does_not_hold_is_not_found() {
    let router = emulator();
    let stored = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
        "Name": "us-only",
        "ColumnNames": ["order_id"],
    });
    let (status, payload) =
        call(&router, "CreateDataCellsFilter", &json!({ "TableData": stored })).await;
    assert_eq!(status, 200, "case \"seed\": expected 200, got {status} {payload:?}");

    // Each row changes exactly one component of a stored filter's identity, so the lookup
    // misses for a different reason each time.
    let cases: [(&str, &str, &str); 4] = [
        ("unknown catalog", "TableCatalogId", "444455556666"),
        ("unknown database", "DatabaseName", "marketing"),
        ("unknown table", "TableName", "returns"),
        ("unknown filter name", "Name", "eu-only"),
    ];
    for (case, member, value) in cases {
        let mut request = locator(&stored);
        request[member] = json!(value);
        let (status, payload) = call(&router, "GetDataCellsFilter", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException, got {code:?} {payload:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_that_does_not_name_its_cells_is_refused() {
    // The model states both rules in prose: "You must specify either a `ColumnNames` list
    // or the `ColumnWildCard`" (`DataCellsFilter$ColumnWildcard`), and "Nested attributes
    // within this list may not exceed a depth of 5."
    // (`smithy_query(query="doc",shape="DataCellsFilter",member="ColumnNames")`). Rows
    // span both, on each side of the bound and on each of the two column lists.
    let identity = json!({
        "TableCatalogId": "111122223333",
        "DatabaseName": "sales",
        "TableName": "orders",
    });
    let case_body = |name: &str, extra: Value| {
        let mut body = identity.clone();
        body["Name"] = json!(name);
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key] = value;
            }
        }
        body
    };

    let cases: Vec<(&str, Value)> = vec![
        (
            "neither ColumnNames nor ColumnWildcard",
            case_body("no-columns", json!({ "RowFilter": { "AllRowsWildcard": {} } })),
        ),
        (
            "both ColumnNames and ColumnWildcard",
            case_body(
                "both-columns",
                json!({ "ColumnNames": ["order_id"], "ColumnWildcard": {} }),
            ),
        ),
        (
            "an empty ColumnNames list",
            case_body("empty-columns", json!({ "ColumnNames": [] })),
        ),
        (
            "a ColumnNames path one level past the depth bound",
            case_body(
                "too-deep",
                json!({ "ColumnNames": ["\"a\".\"b\".\"c\".\"d\".\"e\".\"f\""] }),
            ),
        ),
        (
            "an ExcludedColumnNames path one level past the depth bound",
            case_body(
                "too-deep-exclusion",
                json!({
                    "ColumnWildcard": {
                        "ExcludedColumnNames": ["ssn", "a.b.c.d.e.f"]
                    }
                }),
            ),
        ),
        (
            // An unterminated quote would otherwise hide every dot after it, scoring a
            // path shallower than the levels it actually names.
            "an unterminated quoted segment hiding six levels",
            case_body(
                "unbalanced-quote",
                json!({ "ColumnNames": ["\"a\".b.c.d.e.f.g"] }),
            ),
        ),
        (
            "a RowFilter naming both alternatives",
            case_body(
                "both-rows",
                json!({
                    "ColumnNames": ["order_id"],
                    "RowFilter": { "FilterExpression": "amount > 0", "AllRowsWildcard": {} },
                }),
            ),
        ),
        (
            "a RowFilter naming neither alternative",
            case_body(
                "no-rows",
                json!({ "ColumnNames": ["order_id"], "RowFilter": {} }),
            ),
        ),
    ];

    let router = emulator();
    for (case, table_data) in &cases {
        let (status, payload) =
            call(&router, "CreateDataCellsFilter", &json!({ "TableData": table_data })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {code:?} {payload:?}"
        );
        // A refused create persists nothing: the filter it named is not in the store.
        let (status, payload) = call(&router, "GetDataCellsFilter", &locator(table_data)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refused create to persist nothing, got {status} {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException after a refusal, got {code:?} {payload:?}"
        );
    }
}
