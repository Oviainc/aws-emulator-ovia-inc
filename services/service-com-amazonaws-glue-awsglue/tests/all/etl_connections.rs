// SPDX-License-Identifier: CC0-1.0

//! The CATALOG a Glue connection is created in, beside the round-trip and projection
//! suite in [`super::etl_jobs_and_connections`].
//!
//! The rule under test: `CreateConnectionRequest$CatalogId` is "The ID of the Data Catalog
//! in which to create the connection. If none is provided, the Amazon Web Services account
//! ID is used by default" — so a create that names no catalog writes into this account's
//! own, and one that names a catalog this emulator does not hold is REFUSED rather than
//! conjuring that catalog into existence. The operation models no not-found error, so the
//! refusal is its modelled client-input error.
//!
//! The rows span what the rule generalises over: the catalog named explicitly, the catalog
//! left to the default, and a catalog id no Data Catalog in this account answers to.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, ok};

/// This emulator's account id, which is also the default `CatalogId`
/// (`::emulator_server::EMULATOR_ACCOUNT_ID`, as every ARN this emulator mints carries).
const ACCOUNT_ID: &str = "000000000000";

/// One `ConnectionInput` body, named so each row writes its own connection.
fn connection_input(name: &str) -> Value {
    json!({
        "Name": name,
        "ConnectionType": "JDBC",
        "ConnectionProperties": {
            "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
            "USERNAME": "etl",
        },
    })
}

#[tokio::test]
async fn a_connection_is_created_in_the_catalog_the_request_addresses() {
    let router = emulator();

    // (case, the CatalogId the create names, the CatalogId the read names)
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        (
            "no CatalogId on either call uses the account's own catalog",
            None,
            None,
        ),
        (
            "the account id named explicitly is that same catalog",
            Some(ACCOUNT_ID),
            None,
        ),
        (
            "a create naming the catalog is read back by a read naming it too",
            Some(ACCOUNT_ID),
            Some(ACCOUNT_ID),
        ),
    ];

    for (index, (case, create_catalog, read_catalog)) in rows.into_iter().enumerate() {
        let name = format!("catalogued-{index}");
        let mut create = json!({ "ConnectionInput": connection_input(&name) });
        if let Some(catalog) = create_catalog {
            create["CatalogId"] = json!(catalog);
        }
        let (status, body) = call(&router, "CreateConnection", &create).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection must answer 200, got {status} with body {body}"
        );

        let mut read = json!({ "Name": name });
        if let Some(catalog) = read_catalog {
            read["CatalogId"] = json!(catalog);
        }
        let answer = ok(&router, "GetConnection", &read).await;
        assert_eq!(
            answer
                .get("Connection")
                .and_then(|connection| connection.get("Name"))
                .and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: the connection must be readable from that catalog, got {answer}"
        );
    }
}

#[tokio::test]
async fn a_catalog_this_account_does_not_hold_is_refused_and_writes_nothing() {
    let router = emulator();
    let unknown = "210987654321";
    let create = json!({
        "CatalogId": unknown,
        "ConnectionInput": connection_input("elsewhere"),
    });
    let (status, body) = call(&router, "CreateConnection", &create).await;
    assert_eq!(
        status, 400,
        "a create into a catalog this account does not hold must be refused, got {status} \
         with body {body}"
    );
    assert!(
        error_code(&body).ends_with("InvalidInputException"),
        "expected InvalidInputException, got {}",
        error_code(&body)
    );

    // Nothing was written: neither that catalog nor this account's own holds it.
    for read in [
        json!({ "Name": "elsewhere", "CatalogId": unknown }),
        json!({ "Name": "elsewhere" }),
    ] {
        let (status, body) = call(&router, "GetConnection", &read).await;
        assert_eq!(
            status, 400,
            "case {read}: a refused create must have written nothing, got {status} with \
             body {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {read}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
    }
}
