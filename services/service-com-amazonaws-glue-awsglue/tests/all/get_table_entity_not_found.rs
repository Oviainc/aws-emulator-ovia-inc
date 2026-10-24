// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal `GetTable` answers when the database it resolved
//! does not hold the requested table.
//!
//! The rule under test: that refusal's `Message` is a FIXED CONSTANT — one string the
//! service answers for every such read, carrying no member of the request. So the body
//! is byte-identical however the request is spelled (whichever database and table it
//! names, in whatever casing, character set or length, with or without `CatalogId` and
//! `AttributesToGet`), and it discloses neither the table, the database nor the catalog
//! the read resolved.
//!
//! `error_messages.rs` pins that constant's wording against the recorded wire case and
//! pins it apart from the wording the table WRITES answer, and this suite reuses its
//! `assert_refusal`/`assert_withholds` helpers rather than restating them. What it adds
//! is the invariance: the same answer across a cross product of request shapes, which is
//! what distinguishes a constant from a template that merely happens to match on one
//! input. Its rows pair a database name WITH a table name, because the template this
//! pins out interpolated both members, so the two have to vary independently; and each
//! row also reads back a table that IS present, so a constant refusal cannot be passing
//! by refusing everything, and re-reads the database's table list afterwards, so the
//! misses are proven not to change what a later caller sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::{assert_refusal, assert_withholds};
use crate::wire::{emulator, error_message, ok, table_names, ACCOUNT_ID};

/// The wording the read's miss answers, written out here as a literal so this suite pins
/// it rather than restating the service's own constant.
const ENTITY_NOT_FOUND: &str = "Entity Not Found";

/// The table every row creates in its database: the control that proves the lookup is
/// answered from the store and not refused unconditionally.
const PRESENT_TABLE: &str = "present_tbl";

/// `(row, database, absent table)` triples spanning the axes the rule generalises over:
/// the two names' values, length, casing, character set and punctuation. The first row
/// is the shape of the recorded behavioural case; the last sits at the documented
/// ceiling of `NameString` (length 1..255, from `smithy_query` on `GetTable`'s
/// `DatabaseName`/`Name`).
fn absent_pairs() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "recorded behavioural shape",
            "sim2real_deny_db_3fd80938d8ed".to_string(),
            "sim2real_facts_c586e4884d78".to_string(),
        ),
        (
            "single character names",
            "d".to_string(),
            "t".to_string(),
        ),
        (
            "mixed case names",
            "Warehouse_DB".to_string(),
            "Sales_By_Day".to_string(),
        ),
        (
            "punctuation and digits",
            "team.data_lake-2026".to_string(),
            "raw.events-2026_01".to_string(),
        ),
        (
            "non-ascii character set",
            "donnees_brutes_\u{e9}t\u{e9}".to_string(),
            "ventes_\u{e9}t\u{e9}".to_string(),
        ),
        (
            "names at the 255 ceiling",
            "d".repeat(255),
            "t".repeat(255),
        ),
    ]
}

/// `(shape, CatalogId, AttributesToGet)` variants: the other input members a template
/// could have reached for. `CatalogId` is the account id the model documents as the
/// default ("If none is provided, the Amazon Web Services account ID is used by
/// default"), and `DEFAULT` is the `AttributesToGet` value that selects the Hive-style
/// definition this read serves (both from `smithy_query` on `GetTable`).
fn request_shapes() -> Vec<RequestShape> {
    vec![
        RequestShape {
            label: "no optional members",
            catalog_id: None,
            attributes: None,
        },
        RequestShape {
            label: "explicit default CatalogId",
            catalog_id: Some(ACCOUNT_ID),
            attributes: None,
        },
        RequestShape {
            label: "AttributesToGet DEFAULT",
            catalog_id: None,
            attributes: Some(&["DEFAULT"]),
        },
        RequestShape {
            label: "CatalogId and AttributesToGet",
            catalog_id: Some(ACCOUNT_ID),
            attributes: Some(&["DEFAULT"]),
        },
    ]
}

/// The optional input members one `GetTable` miss is asked with.
struct RequestShape {
    label: &'static str,
    catalog_id: Option<&'static str>,
    attributes: Option<&'static [&'static str]>,
}

/// One `GetTable` body for a row and a request shape.
fn get_table_request(database: &str, name: &str, shape: &RequestShape) -> Value {
    let mut request = json!({ "DatabaseName": database, "Name": name });
    if let Some(catalog_id) = shape.catalog_id {
        request["CatalogId"] = json!(catalog_id);
    }
    if let Some(attributes) = shape.attributes {
        request["AttributesToGet"] = json!(attributes);
    }
    request
}

/// Read the table that IS present back through `GetTable`, so a row cannot be satisfied
/// by a handler that refuses every read.
async fn assert_present_table_is_served(router: &MultiServiceRouter, case: &str, database: &str) {
    let served = ok(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": PRESENT_TABLE }),
    )
    .await;
    assert_eq!(
        served["Table"]["Name"].as_str(),
        Some(PRESENT_TABLE),
        "case {case}: the table that exists must still be served, got {served}"
    );
}

/// `GetTable` for a table its database does not hold is refused with HTTP 400,
/// `EntityNotFoundException` and one constant `Message` that is the same for every
/// request — it carries neither name the caller sent nor the catalog the read resolved.
#[tokio::test]
async fn the_get_table_miss_answers_one_constant_for_every_request() {
    let router = emulator();
    let mut answered: Vec<(String, String)> = Vec::new();

    for (row, database, absent) in &absent_pairs() {
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": database } }),
        )
        .await;
        ok(
            &router,
            "CreateTable",
            &json!({ "DatabaseName": database, "TableInput": { "Name": PRESENT_TABLE } }),
        )
        .await;
        assert_present_table_is_served(&router, row, database).await;

        for shape in &request_shapes() {
            let case = format!("{row} / {label}", label = shape.label);
            let request = get_table_request(database, absent, shape);
            // The status, code and wording assertions are the shared ones this crate's
            // refusal suites already use (`error_messages::assert_refusal`).
            let body = assert_refusal(
                &router,
                &case,
                "GetTable",
                &request,
                "EntityNotFoundException",
                ENTITY_NOT_FOUND,
            )
            .await;
            let message = error_message(&body);
            // The constant names neither entity the request addressed, nor the catalog
            // the emulator resolved on the caller's behalf.
            assert_withholds(&case, &message, absent);
            assert_withholds(&case, &message, database);
            assert_withholds(&case, &message, ACCOUNT_ID);
            answered.push((case, message));
        }

        // A refused read is still a read: it leaves the database holding exactly the one
        // table that was created, so the next caller sees what the first one did.
        let listed = ok(&router, "GetTables", &json!({ "DatabaseName": database })).await;
        assert_eq!(
            table_names(&listed),
            vec![PRESENT_TABLE.to_string()],
            "case {row}: a refused read must not change the catalog, got {listed}"
        );
        assert_present_table_is_served(&router, row, database).await;
    }

    // The invariance itself: every request shape above answered the SAME bytes, which a
    // message interpolated from the request could not.
    let (first_case, first) = answered
        .first()
        .cloned()
        .expect("the table above contributes at least one case");
    for (case, message) in &answered {
        assert_eq!(
            message, &first,
            "case {case}: the refusal must not vary with the request; {first_case} answered \
             {first:?} and this one answered {message:?}"
        );
    }
    assert_eq!(
        answered.len(),
        absent_pairs().len() * request_shapes().len(),
        "every row and request shape must have been exercised, got {} cases",
        answered.len()
    );
}
