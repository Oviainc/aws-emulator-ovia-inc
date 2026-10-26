// SPDX-License-Identifier: CC0-1.0

//! `BatchDeleteConnection`: deleting a list of connection definitions from one Data
//! Catalog.
//!
//! RULE: `BatchDeleteConnection` "[d]eletes a list of connection definitions from the
//! Data Catalog" and reports the outcome PER ENTRY — a name the catalog held is listed
//! in `Succeeded` and is gone for every later reader, and a name it does not hold fails
//! only its own entry, under `Errors` ("a map of the names of connections that were not
//! successfully deleted to error details") carrying this batch's OWN per-entry error
//! code — `InternalException`, not the not-found code a single read answers — and the
//! same miss wording the single-connection read answers. A partial failure therefore
//! neither fails the whole batch nor claims a clean delete.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many names the request
//! carries (including the published lower bound of an empty list and the upper bound of
//! 25), which of them the catalog holds, the order they are interleaved in, and whether
//! the same name is deleted twice (within one request and across two). A request one past
//! a published bound — of the list or of one name — is refused instead, and the catalog
//! the batch addresses is the account's own whether it is named or left out.

use std::collections::BTreeMap;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The largest list `ConnectionNameList` accepts, from the member's own length
/// (`BatchDeleteConnectionRequest$ConnectionNameList`, `DeleteConnectionNameList`
/// length 0..25, from `smithy_query` on `BatchDeleteConnection`).
const MAX_NAMES: usize = 25;

/// Create a JDBC connection under `name`.
async fn given_connection(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateConnection",
        &json!({
            "ConnectionInput": {
                "Name": name,
                "ConnectionType": "JDBC",
                "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://db:3306/w" },
            },
        }),
    )
    .await;
}

/// The `ErrorCode` this batch reports for an entry naming a connection the catalog does
/// not hold: the batch's own per-entry code, which is NOT the code the single-connection
/// read answers for the same miss, nor the entity-miss code this service's other per-item
/// batches report.
///
/// Source: the recorded behavioural unit
/// `glue.batch-delete-connection-per-entry-errors-and-valid-deletes-land`, whose per-entry
/// assertion rejects the entity-miss code and requires this one.
const MISS_ERROR_CODE: &str = "InternalException";

/// The wording a connection miss answers, as AWS words it.
fn expected_miss(name: &str) -> String {
    format!("Connection with name {name} in account {ACCOUNT_ID} is not found")
}

/// `(Succeeded, Errors)` of one `BatchDeleteConnection`, with `Errors` flattened to
/// `name -> (ErrorCode, ErrorMessage)` and ordered so a comparison is deterministic.
async fn batch_delete(
    router: &MultiServiceRouter,
    case: &str,
    names: &[String],
) -> (Vec<String>, BTreeMap<String, (String, String)>) {
    let (status, body) = call(
        router,
        "BatchDeleteConnection",
        &json!({ "ConnectionNameList": names }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: BatchDeleteConnection must answer 200, got {status} with {body}"
    );
    let succeeded = body["Succeeded"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: Succeeded must be a list, got {body}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case}: a name is a string, got {body}"))
                .to_string()
        })
        .collect();
    let errors = body["Errors"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: Errors must be a map, got {body}"))
        .iter()
        .map(|(name, detail)| {
            let code = detail["ErrorCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: an entry carries an ErrorCode, got {body}"))
                .to_string();
            let message = detail["ErrorMessage"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: an entry carries an ErrorMessage, got {body}")
                })
                .to_string();
            (name.clone(), (code, message))
        })
        .collect();
    (succeeded, errors)
}

/// Whether `GetConnection` still resolves `name`.
async fn is_stored(router: &MultiServiceRouter, name: &str) -> bool {
    let (status, _) = call(router, "GetConnection", &json!({ "Name": name })).await;
    status == 200
}

/// One batch: which stored connections the catalog holds, and the names the request
/// carries.
struct Row {
    case: &'static str,
    stored: Vec<String>,
    requested: Vec<String>,
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "every name is held",
            stored: vec!["alpha".to_string(), "beta".to_string()],
            requested: vec!["alpha".to_string(), "beta".to_string()],
        },
        Row {
            case: "no name is held",
            stored: vec!["kept".to_string()],
            requested: vec!["absent-one".to_string(), "absent-two".to_string()],
        },
        Row {
            case: "a held and an absent name interleaved",
            stored: vec!["held-a".to_string(), "held-b".to_string()],
            requested: vec![
                "absent-x".to_string(),
                "held-a".to_string(),
                "absent-y".to_string(),
                "held-b".to_string(),
            ],
        },
        Row {
            case: "one name, held",
            stored: vec!["only".to_string()],
            requested: vec!["only".to_string()],
        },
        Row {
            case: "the empty list, the member's lower bound",
            stored: vec!["untouched".to_string()],
            requested: Vec::new(),
        },
        Row {
            case: "a name that differs only in case from a stored one",
            stored: vec!["cased-conn".to_string()],
            requested: vec!["CASED-CONN".to_string()],
        },
    ];
    // The member's documented upper bound: 25 names, the first half held.
    let stored: Vec<String> = (0..MAX_NAMES / 2).map(|at| format!("bulk-{at:02}")).collect();
    let requested: Vec<String> = (0..MAX_NAMES).map(|at| format!("bulk-{at:02}")).collect();
    rows.push(Row {
        case: "25 names, the member's upper bound, half of them held",
        stored,
        requested,
    });
    rows
}

/// Each entry is reported on its own: the names the catalog held land in `Succeeded` and
/// are gone afterwards, and every other name earns an `Errors` entry carrying the
/// not-found code and the connection-miss wording.
#[tokio::test]
async fn each_entry_is_reported_on_its_own() {
    for row in rows() {
        let router = emulator();
        for name in &row.stored {
            given_connection(&router, name).await;
        }

        let (succeeded, errors) = batch_delete(&router, row.case, &row.requested).await;

        let mut expected_succeeded: Vec<String> = row
            .requested
            .iter()
            .filter(|name| row.stored.contains(name))
            .cloned()
            .collect();
        let mut got_succeeded = succeeded.clone();
        expected_succeeded.sort();
        got_succeeded.sort();
        assert_eq!(
            got_succeeded, expected_succeeded,
            "case {}: Succeeded must name exactly the requested connections the catalog \
             held, got {succeeded:?}",
            row.case
        );

        let expected_failed: Vec<String> = row
            .requested
            .iter()
            .filter(|name| !row.stored.contains(name))
            .cloned()
            .collect();
        let failed: Vec<String> = errors.keys().cloned().collect();
        let mut expected_sorted = expected_failed.clone();
        expected_sorted.sort();
        expected_sorted.dedup();
        assert_eq!(
            failed, expected_sorted,
            "case {}: Errors must name exactly the requested connections the catalog did \
             not hold, got {failed:?}",
            row.case
        );
        for (name, (code, message)) in &errors {
            assert_eq!(
                code, MISS_ERROR_CODE,
                "case {}: entry {name:?} must carry this batch's per-entry code, got \
                 {code:?}",
                row.case
            );
            assert_eq!(
                message,
                &expected_miss(name),
                "case {}: entry {name:?} must carry the connection-miss wording {:?}, got \
                 {message:?}",
                row.case,
                expected_miss(name)
            );
        }

        // What was deleted is gone for the read path, and what was not asked for stays.
        for name in &row.stored {
            let still_there = is_stored(&router, name).await;
            let deleted = row.requested.contains(name);
            assert_eq!(
                still_there, !deleted,
                "case {}: after the batch, {name:?} stored={still_there} but deleted={deleted}",
                row.case
            );
        }
    }
}

/// A deleted connection is a MISS for every operation that resolves one by name, each
/// wording it the way that operation words a miss — so the batch really removed the
/// record rather than hiding it from the listing.
#[tokio::test]
async fn a_deleted_connection_is_a_miss_for_every_reader() {
    let router = emulator();
    given_connection(&router, "doomed").await;
    given_connection(&router, "survivor").await;

    let (succeeded, errors) =
        batch_delete(&router, "one of two", &["doomed".to_string()]).await;
    assert_eq!(
        succeeded,
        vec!["doomed".to_string()],
        "the held name must be reported as deleted, got {succeeded:?}"
    );
    assert!(
        errors.is_empty(),
        "nothing else was asked for, so Errors must be empty, got {errors:?}"
    );

    for (operation, request, expected) in [
        (
            "GetConnection",
            json!({ "Name": "doomed" }),
            expected_miss("doomed"),
        ),
        (
            "UpdateConnection",
            json!({
                "Name": "doomed",
                "ConnectionInput": {
                    "Name": "doomed",
                    "ConnectionType": "JDBC",
                    "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://db:3306/w" },
                },
            }),
            // A write words the same miss its own way.
            "Connection not found: doomed".to_string(),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation}: a deleted connection must be a miss, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {operation}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {operation}: expected Message {expected:?}, got {:?}",
            error_message(&body)
        );
    }

    // Deleting it again reports the same miss, per entry.
    let (succeeded, errors) =
        batch_delete(&router, "deleted twice", &["doomed".to_string()]).await;
    assert!(
        succeeded.is_empty(),
        "a second delete removes nothing, got {succeeded:?}"
    );
    assert_eq!(
        errors.get("doomed").map(|entry| entry.0.clone()),
        Some(MISS_ERROR_CODE.to_string()),
        "a second delete must report the name as a miss, got {errors:?}"
    );

    // The connection nobody asked about is untouched.
    let kept = ok(&router, "GetConnection", &json!({ "Name": "survivor" })).await;
    assert_eq!(
        kept["Connection"]["Name"].as_str(),
        Some("survivor"),
        "the connection the batch did not name must survive, got {kept}"
    );
    let listed = ok(&router, "GetConnections", &json!({})).await;
    let names: Vec<&str> = listed["ConnectionList"]
        .as_array()
        .expect("ConnectionList is a list")
        .iter()
        .filter_map(|connection| connection["Name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["survivor"],
        "the listing must report only what survives, got {names:?}"
    );
}

/// A name repeated WITHIN one request addresses the same connection twice: the first
/// entry deletes it and the second finds nothing, so the batch reports it as deleted once
/// and as a miss once.
#[tokio::test]
async fn a_name_repeated_in_one_request_is_deleted_once_and_missed_once() {
    let router = emulator();
    given_connection(&router, "twice-named").await;

    let (succeeded, errors) = batch_delete(
        &router,
        "the same name twice",
        &["twice-named".to_string(), "twice-named".to_string()],
    )
    .await;
    assert_eq!(
        succeeded,
        vec!["twice-named".to_string()],
        "only one entry can have deleted the connection, got {succeeded:?}"
    );
    assert_eq!(
        errors.get("twice-named").map(|entry| entry.1.clone()),
        Some(expected_miss("twice-named")),
        "the second entry must report the connection-miss wording, got {errors:?}"
    );
    assert!(
        !is_stored(&router, "twice-named").await,
        "the connection must be gone after the batch"
    );
}

/// The request is bounded by the model: `ConnectionNameList` is `DeleteConnectionNameList`
/// (length 0..25) over `NameString` (length 1..255), so a list one past the published
/// maximum, or an entry outside the published name length, is refused at the request
/// boundary — and a refused batch deletes nothing, not even the entries that would have
/// been valid on their own. The bounds themselves (the empty list and 25 names) are served
/// by the table above.
///
/// Source: `smithy_query(query="shape",shape="DeleteConnectionNameList")`.
#[tokio::test]
async fn a_name_list_outside_its_published_bounds_is_refused_and_deletes_nothing() {
    let router = emulator();
    let held = "bounded-conn";
    given_connection(&router, held).await;

    // (case, the names of a request lying one past a published bound)
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one name past the list maximum", {
            let mut names: Vec<String> =
                (0..MAX_NAMES).map(|at| format!("filler-{at:02}")).collect();
            names.push(held.to_string());
            names
        }),
        (
            "a name one character past the member maximum",
            vec![held.to_string(), "n".repeat(256)],
        ),
        (
            "a name one character below the member minimum",
            vec![held.to_string(), String::new()],
        ),
    ];

    for (case, names) in rows {
        let (status, body) = call(
            &router,
            "BatchDeleteConnection",
            &json!({ "ConnectionNameList": names }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the published bounds must be refused, got \
             {status} with {body}"
        );
        assert!(
            is_stored(&router, held).await,
            "case {case:?}: a refused batch must delete nothing, but {held:?} is gone"
        );
    }
}

/// Addressing this account's own catalog EXPLICITLY is the same batch as leaving
/// `CatalogId` out: "[i]f none is provided, the Amazon Web Services account ID is used by
/// default" (`BatchDeleteConnectionRequest$CatalogId`), so both spellings delete the same
/// records and report the same per-entry outcome.
#[tokio::test]
async fn the_account_catalog_addressed_explicitly_is_the_default_catalog() {
    for (case, catalog_id) in [
        ("the catalog left out", None),
        ("the catalog named explicitly", Some(ACCOUNT_ID)),
    ] {
        let router = emulator();
        given_connection(&router, "addressed").await;

        let mut request = json!({ "ConnectionNameList": ["addressed", "never-created"] });
        if let Some(catalog_id) = catalog_id {
            request["CatalogId"] = json!(catalog_id);
        }
        let (status, body) = call(&router, "BatchDeleteConnection", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the account's own catalog is addressable, got {status} with {body}"
        );
        assert_eq!(
            body["Succeeded"],
            json!(["addressed"]),
            "case {case:?}: the held connection must be deleted from the default catalog, got \
             {body}"
        );
        assert_eq!(
            body["Errors"]["never-created"]["ErrorMessage"].as_str(),
            Some(expected_miss("never-created").as_str()),
            "case {case:?}: the absent name must report the connection-miss wording, got {body}"
        );
        assert!(
            !is_stored(&router, "addressed").await,
            "case {case:?}: the deleted connection must be gone from the default catalog"
        );
    }
}

/// A `CatalogId` naming ANOTHER account is refused as unauthorized, before any entry is
/// looked up: nothing of this account's catalog is deleted and no per-entry miss is
/// reported for a catalog the caller holds no grant on.
#[tokio::test]
async fn a_foreign_catalog_batch_is_refused_and_deletes_nothing() {
    let router = emulator();
    given_connection(&router, "local-conn").await;

    let (status, body) = call(
        &router,
        "BatchDeleteConnection",
        &json!({ "CatalogId": "210987654321", "ConnectionNameList": ["local-conn"] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a foreign catalog batch must be refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("AccessDeniedException"),
        "expected AccessDeniedException, got {}",
        error_code(&body)
    );
    assert!(
        is_stored(&router, "local-conn").await,
        "the refused batch must delete nothing"
    );
}
