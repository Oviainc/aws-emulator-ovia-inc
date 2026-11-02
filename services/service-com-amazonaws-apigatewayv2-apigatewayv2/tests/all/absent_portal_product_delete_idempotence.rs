// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the IDEMPOTENCE of `DeletePortalProduct`.
//!
//! The rule these cases assert: **`DELETE /v2/portalproducts/{PortalProductId}` answers the
//! modelled empty `204` whether or not the account holds a product under that identifier —
//! the call asks for a STATE (no product under this id), so an account already in that
//! state is answered exactly as one the call takes a product out of, and the delete reports
//! no `NotFoundException`.** The READERS and the update of one product are unchanged: they
//! have a resource to report rather than a state to reach, so an id that resolves to
//! nothing is still refused `404 NotFoundException`.
//! `smithy_query(query="operation",operation="DeletePortalProduct")` gives
//! `http {method: DELETE, uri: /v2/portalproducts/{PortalProductId}, code: 204}` over an
//! output of `Unit`, and `PortalProductId` is a required `httpLabel` targeting plain
//! `__string` — the label carries no modelled length or pattern, so any value a path
//! segment takes reaches this handler.
//!
//! The rows span what the rule generalises over: how the addressed id came to resolve to
//! nothing (a product this row deleted, an id nothing ever minted, the minted id's
//! uppercase form, a one-character id, an id of another shape entirely, a long id), how
//! many times the same delete is repeated (once, twice, three times), how many sibling
//! products the account holds while it happens, and how many pages the deleted product
//! owned. The recorded step that filed the issue — create a product, delete it, then issue
//! the identical delete again — is the first row.
//!
//! Each case also asserts the delete that removed nothing CHANGED nothing: every sibling
//! product still reads back and is still listed, and a product the row never addressed
//! keeps the pages it owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};
use crate::product_page_lifecycle::pages_of;
use crate::product_rest_endpoint_page_lifecycle::{product, PRODUCTS};

/// AWS's sentence for a `PortalProductId` the account holds no product under: a product is
/// an account-level resource, so the account the lookup was made in is named beside the id.
/// Written out here rather than taken from the service, so the cases assert the rule.
fn product_not_found(portal_product_id: &str) -> String {
    format!(
        "Product not found in account {} with id {portal_product_id}",
        engine::emulator_account_id()
    )
}

/// How a row's addressed identifier comes to resolve to no product.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// A product this row created and then deleted.
    DeletedByThisRow,
    /// An identifier nothing ever minted.
    NeverMinted,
    /// The minted id's uppercase form — the mint is lowercase, so this names no product.
    UppercaseOfMinted,
    /// A one-character identifier.
    OneCharacter,
    /// An identifier of another shape entirely, carrying characters the mint never emits.
    AnotherShape,
    /// A long identifier, far past the length the mint answers.
    Long,
}

impl Target {
    /// The `PortalProductId` this row's delete addresses, given the id of the product the
    /// row minted.
    fn id(self, minted: &str) -> String {
        match self {
            Self::DeletedByThisRow => minted.to_string(),
            Self::NeverMinted => "nosuchproduct".to_string(),
            Self::UppercaseOfMinted => minted.to_uppercase(),
            Self::OneCharacter => "a".to_string(),
            Self::AnotherShape => "NOT-A-PRODUCT-ID".to_string(),
            Self::Long => "p".repeat(80),
        }
    }
}

/// One case: how the addressed id resolves to nothing, how many times the delete is
/// repeated, how many sibling products the account holds, and how many pages the product
/// this row minted owns.
struct Row {
    case: &'static str,
    target: Target,
    deletes: usize,
    siblings: usize,
    pages: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a product deleted and then deleted again",
            target: Target::DeletedByThisRow,
            deletes: 2,
            siblings: 0,
            pages: 0,
        },
        Row {
            case: "a product deleted three times, beside two siblings",
            target: Target::DeletedByThisRow,
            deletes: 3,
            siblings: 2,
            pages: 0,
        },
        Row {
            case: "a product owning two pages, deleted twice",
            target: Target::DeletedByThisRow,
            deletes: 2,
            siblings: 1,
            pages: 2,
        },
        Row {
            case: "an id nothing ever minted",
            target: Target::NeverMinted,
            deletes: 1,
            siblings: 1,
            pages: 1,
        },
        Row {
            case: "the uppercase form of a minted id, deleted twice",
            target: Target::UppercaseOfMinted,
            deletes: 2,
            siblings: 0,
            pages: 1,
        },
        Row {
            case: "a one-character id",
            target: Target::OneCharacter,
            deletes: 1,
            siblings: 2,
            pages: 0,
        },
        Row {
            case: "an id of another shape entirely, deleted three times",
            target: Target::AnotherShape,
            deletes: 3,
            siblings: 1,
            pages: 0,
        },
        Row {
            case: "a long id",
            target: Target::Long,
            deletes: 1,
            siblings: 0,
            pages: 2,
        },
    ]
}

/// Create one page under `portal_product_id` and answer its minted id.
async fn create_page(
    emulator: &MultiServiceRouter,
    case: &str,
    portal_product_id: &str,
    title: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &pages_of(portal_product_id),
        Some(json!({
            "displayContent": { "title": title, "body": format!("The body of {title}") }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateProductPage, got {status} {created}"
    );
    as_str(&created, "productPageId")
}

/// Assert one DELETE answers the modelled empty 204.
async fn assert_empty_204(emulator: &MultiServiceRouter, case: &str, uri: &str, attempt: usize) {
    let (status, body) = call(emulator, "DELETE", uri, None).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected 204 from delete attempt {attempt} of {uri}, got {status} {body}"
    );
    assert_eq!(
        body,
        Value::Null,
        "case {case:?}: expected an empty body from delete attempt {attempt} of {uri}, got {body}"
    );
}

/// The identifiers `ListPortalProducts` reports for the account.
async fn listed_products(emulator: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, listed) = call(emulator, "GET", PRODUCTS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListPortalProducts, got {status} {listed}"
    );
    listed["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| as_str(item, "portalProductId"))
        .collect()
}

#[tokio::test]
async fn deleting_a_portal_product_that_is_already_gone_answers_the_empty_204() {
    let emulator = emulator();

    for row in rows() {
        let minted = product(&emulator, row.case).await;
        let mut page_ids = Vec::new();
        for index in 0..row.pages {
            page_ids.push(create_page(&emulator, row.case, &minted, &format!("page{index}")).await);
        }
        let mut siblings = Vec::new();
        for index in 0..row.siblings {
            siblings.push(product(&emulator, &format!("{} sibling {index}", row.case)).await);
        }

        let target = row.target.id(&minted);
        let uri = format!("{PRODUCTS}/{target}");

        // The row's first delete may be the one that removes the product; every delete
        // after it addresses a product that is already gone. Either way each answers the
        // modelled empty 204.
        for attempt in 1..=row.deletes {
            assert_empty_204(&emulator, row.case, &uri, attempt).await;
        }

        // The id the row addressed resolves to nothing afterwards — for a row that
        // deleted the product, because the delete removed it; for every other row,
        // because it never named a product. Reading is not deleting: the reader still
        // refuses, with the sentence an account-level resource is reported with.
        let (status, read) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from GetPortalProduct of {target}, got {status} {read}",
            row.case
        );
        assert_eq!(
            error_code(&read),
            "NotFoundException",
            "case {:?}: expected NotFoundException from GetPortalProduct, got {read}",
            row.case
        );
        assert_eq!(
            as_str(&read, "message"),
            product_not_found(&target),
            "case {:?}: expected the account-qualified product sentence, got {read}",
            row.case
        );

        // Neither does the idempotent delete resurrect anything: the update of the id is
        // refused the same way.
        let (status, patched) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "displayName": "Renamed" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from UpdatePortalProduct of {target}, got {status} \
             {patched}",
            row.case
        );

        // A delete that removed nothing changed nothing: every sibling still reads back
        // and is still listed, and so is the minted product where the row addressed
        // something else.
        let survivors: Vec<String> = match row.target {
            Target::DeletedByThisRow => siblings.clone(),
            _ => siblings.iter().cloned().chain([minted.clone()]).collect(),
        };
        let reported = listed_products(&emulator, row.case).await;
        for survivor in &survivors {
            let (status, fetched) =
                call(&emulator, "GET", &format!("{PRODUCTS}/{survivor}"), None).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected product {survivor} to survive the delete of {uri}, got \
                 {status} {fetched}",
                row.case
            );
            assert!(
                reported.contains(survivor),
                "case {:?}: expected ListPortalProducts to still report {survivor}, got \
                 {reported:?}",
                row.case
            );
        }
        if matches!(row.target, Target::DeletedByThisRow) {
            assert!(
                !reported.contains(&minted),
                "case {:?}: expected the deleted {minted} to be gone from the listing, got \
                 {reported:?}",
                row.case
            );
        }

        // The pages of a product the row did NOT delete are untouched; the pages of one it
        // deleted went with their owner, and the repeat delete did not change that either.
        for page_id in &page_ids {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("{}/{page_id}", pages_of(&minted)),
                None,
            )
            .await;
            let expected = if matches!(row.target, Target::DeletedByThisRow) {
                404
            } else {
                200
            };
            assert_eq!(
                status, expected,
                "case {:?}: expected {expected} for page {page_id} after the delete of {uri}, \
                 got {status} {fetched}",
                row.case
            );
        }
    }
}

/// The id a deleted product held is free again, and the repeat delete a teardown issues
/// does not take the REPLACEMENT with it: a create after the deletes mints an id of its
/// own, and the record it mints survives.
#[tokio::test]
async fn a_repeated_delete_does_not_touch_the_product_created_after_it() {
    let emulator = emulator();

    let first = product(&emulator, "the first product").await;
    for attempt in 1..=3 {
        assert_empty_204(
            &emulator,
            "the first product",
            &format!("{PRODUCTS}/{first}"),
            attempt,
        )
        .await;
    }

    let second = product(&emulator, "the replacement product").await;
    assert_ne!(
        second, first,
        "expected the replacement to mint an id of its own, got {first} twice"
    );

    // One more delete of the id that is gone, after the replacement exists.
    assert_empty_204(
        &emulator,
        "the first product once more",
        &format!("{PRODUCTS}/{first}"),
        4,
    )
    .await;

    let (status, fetched) = call(&emulator, "GET", &format!("{PRODUCTS}/{second}"), None).await;
    assert_eq!(
        status, 200,
        "expected the replacement {second} to survive, got {status} {fetched}"
    );
    let reported = listed_products(&emulator, "after the deletes").await;
    assert_eq!(
        reported,
        vec![second],
        "expected only the replacement to be listed, got {reported:?}"
    );
}
