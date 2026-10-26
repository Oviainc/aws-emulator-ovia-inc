// SPDX-License-Identifier: CC0-1.0

//! `GetConnections`: the connection definitions one Data Catalog holds.
//!
//! RULE: `GetConnections` "[r]etrieves a list of connection definitions from the Data
//! Catalog" — so it reports every connection the addressed catalog actually holds, from the
//! same records `CreateConnection` wrote and `GetConnection` reads back, in name order, one
//! `MaxResults`-sized page at a time with a `NextToken` that resumes the walk, narrowed to
//! the connections that satisfy every member the `Filter` sets, and with `HidePassword`
//! withholding the password from each element exactly as it does on a single read.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many connections are stored,
//! the page size asked for (including the published bounds 1 and 1000, a size past the
//! stored count, and values outside the range), which filter members are set and to what
//! (type, schema version at both published bounds, one criterion, several, an empty list, a
//! value nothing records, and two members at once), and whether the password is hidden.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, ok, ACCOUNT_ID};

/// Create one connection from the `ConnectionInput` the row describes.
async fn given_connection(router: &MultiServiceRouter, input: Value) {
    ok(router, "CreateConnection", &json!({ "ConnectionInput": input })).await;
}

/// A plain JDBC connection under `name`, which is all a row that only cares about the
/// listing's membership and order needs.
fn jdbc_input(name: &str) -> Value {
    json!({
        "Name": name,
        "ConnectionType": "JDBC",
        "ConnectionProperties": {
            "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
            "USERNAME": "etl",
        },
    })
}

/// The `Name` of every element of a `GetConnections` answer, in the order it returned them.
fn listed_names(page: &Value) -> Vec<String> {
    page["ConnectionList"]
        .as_array()
        .expect("ConnectionList is a list")
        .iter()
        .map(|connection| {
            connection["Name"]
                .as_str()
                .expect("Connection.Name is a string")
                .to_string()
        })
        .collect()
}

/// Walk every page `GetConnections` hands out for `request`, following its own `NextToken`,
/// and return the names in the order they were listed.
async fn walk_listing(router: &MultiServiceRouter, case: &str, request: &Value) -> Vec<String> {
    let mut listed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(cursor) = &token {
            page_request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "GetConnections", &page_request).await;
        if let Some(size) = request.get("MaxResults").and_then(Value::as_u64) {
            let names = listed_names(&page);
            assert!(
                names.len() as u64 <= size,
                "case {case:?}: a page must not exceed MaxResults {size}, got {names:?}"
            );
        }
        listed.extend(listed_names(&page));
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= 64,
            "case {case:?}: the walk must terminate, listed {listed:?} so far"
        );
    }
    listed
}

#[tokio::test]
async fn every_stored_connection_is_listed_in_name_order_and_paged() {
    // (case, how many connections are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no connections at all", 0, None),
        ("one connection, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize`, range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page exactly the stored count", 4, Some(4)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("conn-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_connection(&router, jdbc_input(name)).await;
        }

        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        let listed = walk_listing(&router, case, &request).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: every stored connection must be listed in name order, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_last_page_of_a_listing_carries_no_next_token() {
    let router = emulator();
    for index in 0..3 {
        given_connection(&router, jdbc_input(&format!("conn-{index:02}"))).await;
    }

    // (case, the page size, whether that page is the last one)
    let rows: &[(&str, i32, bool)] = &[
        ("a page that stops short of the end continues", 2, false),
        ("a page that reaches the end stops", 3, true),
        ("a page past the end stops", 10, true),
    ];
    for (case, size, is_last) in rows {
        let page = ok(&router, "GetConnections", &json!({ "MaxResults": size })).await;
        let token = page.get("NextToken").and_then(Value::as_str);
        assert_eq!(
            token.is_none(),
            *is_last,
            "case {case:?}: expected last page {is_last}, got token {token:?} in {page}"
        );
    }
}

#[tokio::test]
async fn the_filter_selects_only_the_connections_it_names() {
    let router = emulator();
    // Four stored connections spanning the members the filter can set: two types, both
    // schema versions (version 2 is the one created with per-compute-environment
    // properties) and three different recorded match-criteria lists.
    given_connection(
        &router,
        json!({
            "Name": "jdbc-both",
            "ConnectionType": "JDBC",
            "MatchCriteria": ["prod", "analytics"],
            "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://a/b" },
        }),
    )
    .await;
    given_connection(
        &router,
        json!({
            "Name": "jdbc-prod",
            "ConnectionType": "JDBC",
            "MatchCriteria": ["prod"],
            "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://c/d" },
        }),
    )
    .await;
    given_connection(
        &router,
        json!({
            "Name": "kafka-plain",
            "ConnectionType": "KAFKA",
            "ConnectionProperties": { "KAFKA_BOOTSTRAP_SERVERS": "broker.internal:9094" },
        }),
    )
    .await;
    given_connection(
        &router,
        json!({
            "Name": "mongo-v2",
            "ConnectionType": "MONGODB",
            "MatchCriteria": ["prod"],
            "ConnectionProperties": { "CONNECTION_URL": "mongodb://e/f" },
            "SparkProperties": { "USERNAME": "spark-etl" },
        }),
    )
    .await;

    // (case, the Filter sent, the names it must select)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no Filter at all selects every stored connection",
            Value::Null,
            vec!["jdbc-both", "jdbc-prod", "kafka-plain", "mongo-v2"],
        ),
        (
            "a Filter that sets no member narrows nothing",
            json!({}),
            vec!["jdbc-both", "jdbc-prod", "kafka-plain", "mongo-v2"],
        ),
        (
            "ConnectionType selects the connections of that type",
            json!({ "ConnectionType": "JDBC" }),
            vec!["jdbc-both", "jdbc-prod"],
        ),
        (
            // A type the issue did not cite, from the model's own `ConnectionType` enum.
            "another type selects only its own connection",
            json!({ "ConnectionType": "MONGODB" }),
            vec!["mongo-v2"],
        ),
        (
            "a type nothing records selects nothing",
            json!({ "ConnectionType": "NETWORK" }),
            vec![],
        ),
        (
            // The published lower bound of `ConnectionSchemaVersion` (range 1..2).
            "schema version 1 selects the connections created without environment properties",
            json!({ "ConnectionSchemaVersion": 1 }),
            vec!["jdbc-both", "jdbc-prod", "kafka-plain"],
        ),
        (
            // The published upper bound.
            "schema version 2 selects the connection created with them",
            json!({ "ConnectionSchemaVersion": 2 }),
            vec!["mongo-v2"],
        ),
        (
            "an empty MatchCriteria names no criterion and excludes nothing",
            json!({ "MatchCriteria": [] }),
            vec!["jdbc-both", "jdbc-prod", "kafka-plain", "mongo-v2"],
        ),
        (
            "one criterion selects every connection that records it",
            json!({ "MatchCriteria": ["prod"] }),
            vec!["jdbc-both", "jdbc-prod", "mongo-v2"],
        ),
        (
            "every criterion named must be recorded",
            json!({ "MatchCriteria": ["prod", "analytics"] }),
            vec!["jdbc-both"],
        ),
        (
            "the criteria are matched as a set, not in order",
            json!({ "MatchCriteria": ["analytics", "prod"] }),
            vec!["jdbc-both"],
        ),
        (
            "a criterion nothing records selects nothing",
            json!({ "MatchCriteria": ["staging"] }),
            vec![],
        ),
        (
            "two members set must both be satisfied",
            json!({ "ConnectionType": "JDBC", "MatchCriteria": ["analytics"] }),
            vec!["jdbc-both"],
        ),
        (
            "two members that no one connection satisfies select nothing",
            json!({ "ConnectionType": "KAFKA", "ConnectionSchemaVersion": 2 }),
            vec![],
        ),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Filter"] = filter;
        }
        let listed = walk_listing(&router, case, &request).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn hide_password_withholds_the_password_from_every_element() {
    let router = emulator();
    given_connection(
        &router,
        json!({
            "Name": "warehouse",
            "ConnectionType": "JDBC",
            "ConnectionProperties": {
                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                "USERNAME": "etl",
                "PASSWORD": "a-secret",
                "ENCRYPTED_PASSWORD": "an-encrypted-secret",
            },
            "PythonProperties": { "PASSWORD": "a-python-secret" },
        }),
    )
    .await;
    given_connection(
        &router,
        json!({
            "Name": "lakehouse",
            "ConnectionType": "JDBC",
            "ConnectionProperties": {
                "JDBC_CONNECTION_URL": "jdbc:mysql://db.internal:3306/lake",
                "PASSWORD": "another-secret",
            },
        }),
    )
    .await;

    // (case, what HidePassword the request carries, the property maps every element of the
    // answer must carry, keyed by the connection's name)
    let rows: Vec<(&str, Option<bool>, Value)> = vec![
        (
            "the member left absent defaults to false",
            None,
            expected_properties(true),
        ),
        (
            "HidePassword false reports the password",
            Some(false),
            expected_properties(true),
        ),
        (
            "HidePassword true withholds it from every map",
            Some(true),
            expected_properties(false),
        ),
    ];

    for (case, hide_password, expected) in rows {
        let mut request = json!({});
        if let Some(hide) = hide_password {
            request["HidePassword"] = json!(hide);
        }
        let answer = ok(&router, "GetConnections", &request).await;
        let elements = answer["ConnectionList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectionList, got {answer}"))
            .clone();
        assert_eq!(
            listed_names(&answer),
            vec!["lakehouse".to_string(), "warehouse".to_string()],
            "case {case:?}: both connections must be listed, got {answer}"
        );
        for element in &elements {
            let name = element["Name"].as_str().expect("a name");
            // `HidePassword` is about the password, not about one member: no property map
            // the answer carries may hold one when it is set.
            for member in ["ConnectionProperties", "PythonProperties"] {
                let actual = element.get(member).cloned().unwrap_or(Value::Null);
                let wanted = expected[name]
                    .get(member)
                    .cloned()
                    .unwrap_or(Value::Null);
                assert_eq!(
                    actual, wanted,
                    "case {case:?}: {name}.{member} must be {wanted}, got {actual}"
                );
            }
        }
    }
}

/// What the two connections of the `HidePassword` suite report, with the password
/// properties either reported or withheld. Every other property is reported either way.
fn expected_properties(password_reported: bool) -> Value {
    let mut warehouse = json!({
        "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
        "USERNAME": "etl",
    });
    let mut warehouse_python = json!({});
    let mut lakehouse = json!({ "JDBC_CONNECTION_URL": "jdbc:mysql://db.internal:3306/lake" });
    if password_reported {
        warehouse["PASSWORD"] = json!("a-secret");
        warehouse["ENCRYPTED_PASSWORD"] = json!("an-encrypted-secret");
        warehouse_python["PASSWORD"] = json!("a-python-secret");
        lakehouse["PASSWORD"] = json!("another-secret");
    }
    json!({
        "warehouse": {
            "ConnectionProperties": warehouse,
            "PythonProperties": warehouse_python,
        },
        "lakehouse": { "ConnectionProperties": lakehouse },
    })
}

#[tokio::test]
async fn a_listed_element_is_what_the_single_connection_read_reports() {
    let router = emulator();
    let input = json!({
        "Name": "round-tripped",
        "Description": "the connection the listing and the read must agree on",
        "ConnectionType": "JDBC",
        "MatchCriteria": ["prod"],
        "ConnectionProperties": {
            "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
            "USERNAME": "etl",
            "PASSWORD": "a-secret",
        },
        "SparkProperties": { "USERNAME": "spark-etl" },
    });
    given_connection(&router, input).await;

    // (case, HidePassword) — the projection must agree under both.
    for (case, hide_password) in [("reported", false), ("withheld", true)] {
        let listing = ok(
            &router,
            "GetConnections",
            &json!({ "HidePassword": hide_password }),
        )
        .await;
        let listed = listing["ConnectionList"]
            .as_array()
            .and_then(|elements| elements.first())
            .unwrap_or_else(|| panic!("case {case:?}: expected one element, got {listing}"))
            .clone();
        let read = ok(
            &router,
            "GetConnection",
            &json!({ "Name": "round-tripped", "HidePassword": hide_password }),
        )
        .await;
        assert_eq!(
            listed, read["Connection"],
            "case {case:?}: the listed element must be what the read reports, got {listed} \
             against {}",
            read["Connection"]
        );
    }
}

#[tokio::test]
async fn the_catalog_the_request_addresses_is_the_one_listed() {
    let router = emulator();
    given_connection(&router, jdbc_input("catalogued")).await;

    // The account's own catalog, named explicitly or left to the default, is the same one.
    for (case, catalog_id) in [("left to the default", None), ("named", Some(ACCOUNT_ID))] {
        let mut request = json!({});
        if let Some(catalog) = catalog_id {
            request["CatalogId"] = json!(catalog);
        }
        let listed = walk_listing(&router, case, &request).await;
        assert_eq!(
            listed,
            vec!["catalogued".to_string()],
            "case {case:?}: the account's own catalog must be listed, got {listed:?}"
        );
    }

    // Another account's Data Catalog is reachable only through its own resource policy, so
    // a listing of one is refused rather than answered with this account's connections.
    let (status, body) = call(
        &router,
        "GetConnections",
        &json!({ "CatalogId": "210987654321" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a listing of another account's catalog must be refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("AccessDeniedException"),
        "expected AccessDeniedException, got {} in {body}",
        error_code(&body)
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_range_is_refused() {
    let router = emulator();
    given_connection(&router, jdbc_input("bounded")).await;

    // (case, the request whose member lies one past a published bound)
    let rows: Vec<(&str, Value)> = vec![
        // `MaxResults` targets `PageSize`, range 1..1000.
        ("MaxResults one below the range", json!({ "MaxResults": 0 })),
        (
            "MaxResults one above the range",
            json!({ "MaxResults": 1_001 }),
        ),
        // `GetConnectionsFilter$ConnectionSchemaVersion`, range 1..2.
        (
            "a schema version one below the range",
            json!({ "Filter": { "ConnectionSchemaVersion": 0 } }),
        ),
        (
            "a schema version one above the range",
            json!({ "Filter": { "ConnectionSchemaVersion": 3 } }),
        ),
        // `GetConnectionsFilter$MatchCriteria`, length 0..10.
        (
            "more criteria than the list accepts",
            json!({ "Filter": { "MatchCriteria": [
                "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k",
            ] } }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = call(&router, "GetConnections", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the modelled range must be refused, got {status} \
             with {body}"
        );
    }
}
