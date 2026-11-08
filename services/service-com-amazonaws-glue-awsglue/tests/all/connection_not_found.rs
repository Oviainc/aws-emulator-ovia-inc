// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wording of the refusal a Glue operation that resolves a
//! CONNECTION definition by name answers when no such connection exists.
//!
//! The rule under test: a READ that resolves one connection by name and finds none
//! refuses with 400 `EntityNotFoundException` whose `Message` names the requested
//! connection and scopes the miss to the ACCOUNT — "Connection with name <Name> in
//! account <accountId> is not found" — with `<Name>` taken verbatim from the request and
//! `<accountId>` the account the caller's credentials speak for, never the `CatalogId`
//! the request happened to carry. The `UpdateConnection` that addresses one connection by
//! name words the same miss its own way — "Connection not found: <Name>" — so a client can
//! tell the two apart, exactly as the table read and the table writes are told apart. The
//! DELETE words it a third way, with no name at all, which `not_found_wording.rs` pins.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! requested name's value, length, casing and character set, whether the request names
//! its catalog explicitly, which operation asks, and the Region the request is served
//! in. The expected strings are written out here as literals (not read from the service's
//! own helpers) so the test pins the wording rather than restating it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// Names that span the axis the template generalises over: the recorded fixture's own
/// shape, a short name, a mixed-case name, one carrying the punctuation `NameString`'s
/// pattern admits, and one at the member's documented 255-character ceiling
/// (`ConnectionInput$Name` / `GetConnectionRequest$Name` target `NameString`,
/// length 1..255, from `smithy_query` on `GetConnection`).
fn absent_names() -> Vec<(String, String)> {
    vec![
        (
            "recorded fixture shape".to_string(),
            "sim2real-behav-conn-d216e3be9dcc".to_string(),
        ),
        ("single character".to_string(), "c".to_string()),
        ("mixed case".to_string(), "MissingConnection".to_string()),
        (
            "punctuation and digits".to_string(),
            "team.lake_conn-2026/raw".to_string(),
        ),
        ("at the 255 ceiling".to_string(), "n".repeat(255)),
    ]
}

/// The wording a READ's connection miss must answer, as AWS words it: fixed text around
/// the requested name and the caller's account id.
fn expected_read_message(name: &str) -> String {
    format!("Connection with name {name} in account {ACCOUNT_ID} is not found")
}

/// The wording the UPDATE's connection miss must answer: the service's own update
/// sentence, carrying the requested name alone.
fn expected_update_message(name: &str) -> String {
    format!("Connection not found: {name}")
}

/// A `CreateConnection` body for `name`.
fn connection_body(name: &str) -> Value {
    json!({
        "ConnectionInput": {
            "Name": name,
            "ConnectionType": "JDBC",
            "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://host:3306/db" },
        },
    })
}

/// An `UpdateConnection` body addressing `name` (whose `ConnectionInput$Name` must name
/// the same connection).
fn update_body(name: &str) -> Value {
    json!({
        "Name": name,
        "ConnectionInput": {
            "Name": name,
            "ConnectionType": "JDBC",
            "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://host:3306/db" },
        },
    })
}

/// Assert one refusal served in `region`: HTTP 400, `EntityNotFoundException`, and the
/// expected `Message` verbatim.
async fn assert_miss(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    operation: &str,
    request: &Value,
    expected: &str,
) {
    let (status, body) = call_in_region(router, region, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?} via {operation}: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case {case:?} via {operation}: expected EntityNotFoundException, got {}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        expected,
        "case {case:?} via {operation}: expected Message {expected:?}, got {:?}",
        error_message(&body)
    );
}

/// Every operation that resolves a connection by name answers its OWN wording for a name
/// no connection has — the read's account-scoped template and the write's own sentence —
/// however the name is spelled and whether or not the request names its catalog.
///
/// The rows are the cross product of the name axis of [`absent_names`] with the two
/// operations that address one connection by name (`GetConnection`, `UpdateConnection`),
/// each asked both without a `CatalogId` and with the account's own id named explicitly.
#[tokio::test]
async fn a_connection_miss_is_worded_per_operation() {
    let router = emulator();
    // A connection that DOES exist, so each refusal below is about the name it asked for
    // and not about an empty catalog.
    ok(&router, "CreateConnection", &connection_body("present-conn")).await;

    let mut cases = absent_names();
    // Connection names are stored and matched exactly as sent, so a name that differs
    // from a stored one only in case is a miss too.
    cases.push((
        "differs only in case from a stored connection".to_string(),
        "PRESENT-CONN".to_string(),
    ));

    for (case, name) in &cases {
        for named_catalog in [false, true] {
            let label = format!("{case} (CatalogId named: {named_catalog})");

            let mut read = json!({ "Name": name });
            let mut update = update_body(name);
            if named_catalog {
                read["CatalogId"] = json!(ACCOUNT_ID);
                update["CatalogId"] = json!(ACCOUNT_ID);
            }

            assert_miss(
                &router,
                crate::wire::REGION,
                &label,
                "GetConnection",
                &read,
                &expected_read_message(name),
            )
            .await;
            assert_miss(
                &router,
                crate::wire::REGION,
                &label,
                "UpdateConnection",
                &update,
                &expected_update_message(name),
            )
            .await;
            assert_ne!(
                expected_read_message(name),
                expected_update_message(name),
                "case {label}: the read and the write must be distinguishable by message"
            );
        }
    }

    // The connection that exists is still served, so the refusals above are about the
    // requested name alone.
    let present = ok(&router, "GetConnection", &json!({ "Name": "present-conn" })).await;
    assert_eq!(
        present["Connection"]["Name"].as_str(),
        Some("present-conn"),
        "the stored connection must still be reported, got {present}"
    );
}

/// The refusal's wording does not depend on the endpoint's Region, and a connection the
/// Data Catalog holds is reachable from any Region's endpoint — the Data Catalog this
/// store serves is the account's, so the miss is scoped to the account either way.
#[tokio::test]
async fn the_connection_miss_wording_is_the_same_in_every_region() {
    let router = emulator();
    ok(&router, "CreateConnection", &connection_body("shared-conn")).await;

    // Regions spanning three prefixes, so nothing here depends on one Region name.
    for region in ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-1"] {
        assert_miss(
            &router,
            region,
            region,
            "GetConnection",
            &json!({ "Name": "no-such-conn" }),
            &expected_read_message("no-such-conn"),
        )
        .await;
    }
}

/// A `CatalogId` naming ANOTHER account is an authorization refusal, not this miss: the
/// not-found wording is reserved for a name the caller's own account does not hold, so a
/// cross-account read never reports the requested name as absent.
#[tokio::test]
async fn a_foreign_catalog_is_not_reported_as_a_missing_connection() {
    let router = emulator();

    let (status, body) = call(
        &router,
        "GetConnection",
        &json!({ "CatalogId": "210987654321", "Name": "no-such-conn" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a foreign catalog read must be refused, got {status} with {body}"
    );
    assert!(
        !error_code(&body).ends_with("EntityNotFoundException"),
        "case foreign catalog: the miss wording is not used for another account's \
         catalog, got {}",
        error_code(&body)
    );
    assert_ne!(
        error_message(&body),
        expected_read_message("no-such-conn"),
        "case foreign catalog: expected an authorization refusal, got the miss wording"
    );
}
