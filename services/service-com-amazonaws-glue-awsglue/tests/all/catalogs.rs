// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a catalog READ is: `GetCatalog` reports the stored catalog
//! resource and nothing else changes.
//!
//! Two rules, neither of which the round-trip suite beside this one
//! ([`crate::catalog_resources`]) pins:
//!
//! 1. A read is a REPORTING operation: it writes nothing a later caller can see. Repeated
//!    reads of one catalog report the same body, a read that misses leaves the id it
//!    missed on free for a later create, and neither kind of read disturbs a catalog
//!    already stored.
//! 2. `Catalog$ResourceArn` names the REGION the read was served in, because a Glue
//!    record carries no Region of its own; every other member of the catalog is the same
//!    whichever Region's endpoint reports it.
//!
//! Each suite is a table: the rows differ along how often and in which order the reads
//! come, and along the Region the read is served in.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    call, call_in_region, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION,
};

/// The `Catalog` a `GetCatalog` served in `region` reports for `catalog_id`.
async fn get_in(router: &MultiServiceRouter, region: &str, catalog_id: &str) -> Value {
    let (status, body) =
        call_in_region(router, region, "GetCatalog", &json!({ "CatalogId": catalog_id })).await;
    assert_eq!(
        status, 200,
        "case {region} / {catalog_id:?}: GetCatalog must answer 200, got {status} with {body}"
    );
    body.get("Catalog")
        .cloned()
        .unwrap_or_else(|| panic!("GetCatalog must report a Catalog, got {body}"))
}

/// Register the catalog `name` with a definition that is distinguishable per name.
///
/// Every create names a source outside the Glue Data Catalog, because that is the only
/// catalog `CreateCatalog` registers (the rule is pinned by
/// [`crate::federated_catalogs`]).
async fn create_catalog(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateCatalog",
        &json!({
            "Name": name,
            "CatalogInput": {
                "Description": format!("the {name} catalog"),
                "Parameters": { "owner": name },
                "FederatedCatalog": {
                    "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
                    "ConnectionName": "aws:s3tables",
                },
            },
        }),
    )
    .await;
}

/// Reading a catalog reports it and changes nothing: the same read repeated answers the
/// same body, and the misses in between leave the store as it was — the id a read missed
/// on is still free for a create, which is what proves the read materialised no record
/// under it.
#[tokio::test]
async fn reading_a_catalog_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    create_catalog(&router, "stable-catalog").await;

    let first = get_in(&router, REGION, "stable-catalog").await;
    // Rows: the same id again, the id spelled in another casing, and the id qualified by
    // the account that holds the catalog — every spelling of one read, repeated.
    for spelling in [
        "stable-catalog".to_string(),
        "STABLE-CATALOG".to_string(),
        format!("{ACCOUNT_ID}:stable-catalog"),
    ] {
        let again = get_in(&router, REGION, &spelling).await;
        assert_eq!(
            again, first,
            "case {spelling:?}: a repeated read must report the catalog the first read \
             did, got {again}"
        );
    }

    // A read that MISSES must not leave a record behind either: the same id is read
    // twice, refused both times, and is then taken by a create.
    for attempt in ["first", "second"] {
        let (status, body) = call(
            &router,
            "GetCatalog",
            &json!({ "CatalogId": "not-yet-created" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {attempt} miss: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {attempt} miss: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            "Catalog not found.",
            "case {attempt} miss: expected the miss wording, got {:?}",
            error_message(&body)
        );
    }
    create_catalog(&router, "not-yet-created").await;
    let created = get_in(&router, REGION, "not-yet-created").await;
    assert_eq!(
        created["Description"].as_str(),
        Some("the not-yet-created catalog"),
        "a create after a miss on that id must be served from its own definition, got \
         {created}"
    );

    // The catalog read at the start is still exactly as it was, so none of the reads or
    // the later create touched it.
    let unchanged = get_in(&router, REGION, "stable-catalog").await;
    assert_eq!(
        unchanged, first,
        "the stored catalog must be untouched by every read in between, got {unchanged}"
    );
}

/// A catalog's `ResourceArn` names the Region the read was served in, and nothing else
/// about the catalog depends on that Region: the record carries no Region of its own, so
/// the ARN is minted per request while the definition is reported from the store.
#[tokio::test]
async fn a_catalogs_arn_names_the_region_the_read_was_served_in() {
    let router = emulator();
    create_catalog(&router, "regional-catalog").await;

    let mut without_arn: Option<Value> = None;
    // Rows spanning four Region prefixes of the `aws` partition, so nothing here depends
    // on one Region name.
    for region in ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-1"] {
        let mut catalog = get_in(&router, region, "regional-catalog").await;
        let expected = format!("arn:aws:glue:{region}:{ACCOUNT_ID}:catalog/regional-catalog");
        assert_eq!(
            catalog["ResourceArn"].as_str(),
            Some(expected.as_str()),
            "case {region}: expected ResourceArn {expected:?}, got {catalog}"
        );

        // Everything but that ARN is the same answer in every Region.
        catalog
            .as_object_mut()
            .expect("a Catalog is an object")
            .remove("ResourceArn");
        match &without_arn {
            None => without_arn = Some(catalog),
            Some(first) => assert_eq!(
                &catalog, first,
                "case {region}: only the ARN may differ between Regions, got {catalog}"
            ),
        }
    }
}
