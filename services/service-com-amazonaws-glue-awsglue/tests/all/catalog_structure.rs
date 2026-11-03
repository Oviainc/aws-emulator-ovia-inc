// SPDX-License-Identifier: CC0-1.0

//! What the whole `Catalog` structure a `GetCatalog` reports carries, beyond the members
//! a `CreateCatalog` sent.
//!
//! One rule under test: every catalog a read reports carries its identity
//! (`CatalogIdentifier`, the catalog's own `CatalogId`), its kind (`CatalogType`), its
//! description, and its properties map and both Lake Formation default-permission lists
//! reported DEFINITELY — the empty map and the empty lists where nothing is configured,
//! never an absent member — so a client can never read "nothing configured" as "not
//! reported".
//!
//! Each suite is a TABLE: the rows differ along what the rule generalises over — whether
//! the catalog is the account's own Data Catalog or a record a create wrote, which source
//! outside the Data Catalog the record points at, which of the members were configured
//! and how many grants each list carries, and the Region the read is served in.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call_in_region, emulator, ok, ACCOUNT_ID, PRINCIPAL_PERMISSIONS_TAG, REGION};

/// `Catalog$CatalogType` of a catalog whose databases live in the Glue Data Catalog
/// itself.
///
/// Source: the `CatalogType` member of `com.amazonaws.glue#Catalog` in this crate's model
/// (`smithy_query shape=Catalog`), which documents this spelling for a catalog held
/// natively, and `NATIVE` is what the account's own Data Catalog reports on the wire.
const NATIVE: &str = "NATIVE";

/// `Catalog$CatalogType` of a catalog that points at an entity OUTSIDE the Glue Data
/// Catalog — a `FederatedCatalog` source or a `TargetRedshiftCatalog` resource link.
///
/// Source: the same `CatalogType` member's documentation.
const FEDERATED: &str = "FEDERATED";

/// `Catalog$Description` of the account's own Data Catalog: Glue's own fixed text for the
/// catalog every account has, which no `CreateCatalog` wrote.
const DEFAULT_CATALOG_DESCRIPTION: &str = "This is the Default Catalog.";

/// A `FederatedCatalog` naming a source outside the Glue Data Catalog that `name` points
/// at. `CreateCatalog` registers only a catalog naming such a source (pinned by
/// [`crate::federated_catalogs`]).
fn federation_source(name: &str) -> Value {
    json!({
        "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
        "ConnectionName": "aws:s3tables",
    })
}

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
        .unwrap_or_else(|| panic!("case {catalog_id:?}: expected a Catalog, got {body}"))
}

/// The `Catalog` a `GetCatalog` reports for `catalog_id`, in the default Region.
async fn get(router: &MultiServiceRouter, catalog_id: &str) -> Value {
    get_in(router, REGION, catalog_id).await
}

/// Assert the members every catalog reports definitely: the identity, the kind, and the
/// three collection members whose expected contents the caller passes.
fn assert_structure(
    case: &str,
    catalog: &Value,
    expected_identifier: &str,
    expected_type: &str,
    expected_properties: &Value,
    expected_table_grants: usize,
    expected_database_grants: usize,
) {
    assert_eq!(
        catalog["CatalogIdentifier"],
        json!(expected_identifier),
        "case {case}: expected CatalogIdentifier {expected_identifier:?}, got {:?}",
        catalog["CatalogIdentifier"]
    );
    // The identity member names the catalog the read reports, so it is the same id every
    // other operation addresses it by.
    assert_eq!(
        catalog["CatalogIdentifier"], catalog["CatalogId"],
        "case {case}: CatalogIdentifier must equal CatalogId, got {:?} against {:?}",
        catalog["CatalogIdentifier"], catalog["CatalogId"]
    );
    assert_eq!(
        catalog["CatalogType"],
        json!(expected_type),
        "case {case}: expected CatalogType {expected_type:?}, got {:?}",
        catalog["CatalogType"]
    );
    assert_eq!(
        &catalog["CatalogProperties"], expected_properties,
        "case {case}: expected CatalogProperties {expected_properties}, got {:?}",
        catalog["CatalogProperties"]
    );

    for (member, expected_grants) in [
        ("CreateTableDefaultPermissions", expected_table_grants),
        ("CreateDatabaseDefaultPermissions", expected_database_grants),
    ] {
        let grants = catalog[member].as_array().unwrap_or_else(|| {
            panic!("case {case}: {member} must be reported as a list, got {catalog}")
        });
        assert_eq!(
            grants.len(),
            expected_grants,
            "case {case}: expected {expected_grants} grant(s) in {member}, got {grants:?}"
        );
        for grant in grants {
            assert_eq!(
                grant["__type"],
                json!(PRINCIPAL_PERMISSIONS_TAG),
                "case {case}: a nested grant of {member} must carry __type \
                 {PRINCIPAL_PERMISSIONS_TAG}, got {grant:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The account's own Data Catalog
// ---------------------------------------------------------------------------

/// A read of the account's own Data Catalog reports it whole: its identifier is the
/// account's own number, its kind is the native one, its description is Glue's own fixed
/// text, and it configures no properties and no default permissions — reported as the
/// empty map and the empty lists.
///
/// The rows differ along the Region the read is served in and along how the id is
/// spelled, neither of which the rule depends on; the ARN is the only member that names
/// the Region (pinned by [`crate::catalogs`]).
#[tokio::test]
async fn the_accounts_own_data_catalog_is_reported_whole() {
    let router = emulator();

    // Rows: four Region prefixes of the `aws` partition, so nothing here depends on one
    // Region name.
    for region in ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-1"] {
        let catalog = get_in(&router, region, ACCOUNT_ID).await;
        assert_structure(
            region,
            &catalog,
            ACCOUNT_ID,
            NATIVE,
            &json!({}),
            0,
            0,
        );
        assert_eq!(
            catalog["Description"],
            json!(DEFAULT_CATALOG_DESCRIPTION),
            "case {region}: expected the default catalog's fixed description \
             {DEFAULT_CATALOG_DESCRIPTION:?}, got {:?}",
            catalog["Description"]
        );
        assert_eq!(
            catalog["Name"],
            json!(ACCOUNT_ID),
            "case {region}: the account's Data Catalog is named by the account's own \
             number, got {:?}",
            catalog["Name"]
        );
    }

    // A read reports, so two reads of the catalog answer the same body.
    let first = get(&router, ACCOUNT_ID).await;
    let again = get(&router, ACCOUNT_ID).await;
    assert_eq!(
        again, first,
        "case repeated read: a read must report the same catalog twice, got {again} after \
         {first}"
    );
}

// ---------------------------------------------------------------------------
// The catalogs a create wrote
// ---------------------------------------------------------------------------

/// One row of the created-catalog table: the case name, the `CreateCatalog` request, the
/// `CatalogProperties` the read must report, and how many grants each default-permission
/// list must carry.
type CreatedRow = (&'static str, Value, Value, usize, usize);

/// Every catalog a create wrote is reported whole too: its identifier is its own
/// `CatalogId` — the account that holds it and its name — its kind follows the source its
/// definition names, and the properties map and both grant lists are reported definitely,
/// empty where the create configured nothing.
///
/// The rows differ along the source the catalog points at (a federated source, a
/// Redshift resource link), which of the three collection members the create configured,
/// how many grants each list carries, and whether a description was sent.
#[tokio::test]
async fn every_created_catalog_is_reported_whole() {
    let router = emulator();

    let analyst = format!("arn:aws:iam::{ACCOUNT_ID}:role/analyst");
    let engineer = format!("arn:aws:iam::{ACCOUNT_ID}:role/engineer");

    let cases: Vec<CreatedRow> = vec![
        (
            "a federated source and nothing else",
            json!({
                "Name": "structure-bare",
                "CatalogInput": { "FederatedCatalog": federation_source("structure-bare") },
            }),
            json!({}),
            0,
            0,
        ),
        (
            // Explicitly empty lists are the same ANSWER as none configured, which is
            // what makes the empty list the only honest report in both rows.
            "explicitly empty grant lists",
            json!({
                "Name": "structure-empty-grants",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("structure-empty-grants"),
                    "CreateTableDefaultPermissions": [],
                    "CreateDatabaseDefaultPermissions": [],
                },
            }),
            json!({}),
            0,
            0,
        ),
        (
            "one grant in each list",
            json!({
                "Name": "structure-one-grant",
                "CatalogInput": {
                    "Description": "a governed catalog",
                    "FederatedCatalog": federation_source("structure-one-grant"),
                    "CreateTableDefaultPermissions": [{
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Permissions": ["SELECT"],
                    }],
                    "CreateDatabaseDefaultPermissions": [{
                        "Principal": { "DataLakePrincipalIdentifier": analyst },
                        "Permissions": ["ALL"],
                    }],
                },
            }),
            json!({}),
            1,
            1,
        ),
        (
            // Asymmetric counts: the two lists are reported from their own stored value,
            // not from one another.
            "two table grants and no database grant",
            json!({
                "Name": "structure-asymmetric-grants",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("structure-asymmetric-grants"),
                    "CreateTableDefaultPermissions": [
                        {
                            "Principal": { "DataLakePrincipalIdentifier": analyst },
                            "Permissions": ["SELECT", "ALTER"],
                        },
                        {
                            "Principal": { "DataLakePrincipalIdentifier": engineer },
                            "Permissions": ["CREATE_TABLE", "DATA_LOCATION_ACCESS"],
                        },
                    ],
                },
            }),
            json!({}),
            2,
            0,
        ),
        (
            "custom catalog properties, no grants",
            json!({
                "Name": "structure-custom-properties",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("structure-custom-properties"),
                    "CatalogProperties": { "CustomProperties": { "column_statistics": "enabled" } },
                },
            }),
            json!({ "CustomProperties": { "column_statistics": "enabled" } }),
            0,
            0,
        ),
        (
            // A Redshift resource link points outside the Glue Data Catalog too, so it is
            // the same kind as a federated catalog.
            "a Redshift resource link",
            json!({
                "Name": "structure-redshift-link",
                "CatalogInput": {
                    "TargetRedshiftCatalog": {
                        "CatalogArn": format!(
                            "arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog/remote"
                        ),
                    },
                    "CreateDatabaseDefaultPermissions": [{
                        "Principal": { "DataLakePrincipalIdentifier": engineer },
                        "Permissions": ["DROP"],
                    }],
                },
            }),
            json!({}),
            0,
            1,
        ),
    ];

    for (case, request, expected_properties, table_grants, database_grants) in &cases {
        ok(&router, "CreateCatalog", request).await;
        let name = request["Name"].as_str().expect("the case carries a Name");

        let catalog = get(&router, name).await;
        assert_structure(
            case,
            &catalog,
            &format!("{ACCOUNT_ID}:{name}"),
            FEDERATED,
            expected_properties,
            *table_grants,
            *database_grants,
        );

        // The description is reported as the create wrote it, and a create that wrote
        // none leaves it absent: unlike the collection members, there is no empty
        // description to report.
        let sent_description = request["CatalogInput"]
            .get("Description")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            catalog["Description"],
            sent_description,
            "case {case}: expected Description {sent_description}, got {:?}",
            catalog["Description"]
        );

        // Reported from the store, so a second read answers the same body.
        let again = get(&router, name).await;
        assert_eq!(
            again, catalog,
            "case {case}: two reads of one catalog must agree, got {again} after {catalog}"
        );
    }
}

/// The identity and kind a created catalog reports are its OWN, not the account Data
/// Catalog's: reading the container and reading a catalog inside it answer different
/// identifiers and different kinds.
#[tokio::test]
async fn a_created_catalog_is_told_apart_from_the_data_catalog_that_holds_it() {
    let router = emulator();
    ok(
        &router,
        "CreateCatalog",
        &json!({
            "Name": "structure-inside",
            "CatalogInput": { "FederatedCatalog": federation_source("structure-inside") },
        }),
    )
    .await;

    // Rows: every spelling the inner catalog is reachable under, against the container.
    for spelling in [
        "structure-inside".to_string(),
        "STRUCTURE-INSIDE".to_string(),
        format!("{ACCOUNT_ID}:structure-inside"),
    ] {
        let inner = get(&router, &spelling).await;
        let container = get(&router, ACCOUNT_ID).await;
        assert_eq!(
            inner["CatalogIdentifier"],
            json!(format!("{ACCOUNT_ID}:structure-inside")),
            "case {spelling:?}: the inner catalog reports its own identifier, got {:?}",
            inner["CatalogIdentifier"]
        );
        assert_ne!(
            inner["CatalogIdentifier"], container["CatalogIdentifier"],
            "case {spelling:?}: a catalog and the Data Catalog holding it must not report \
             one identifier, got {:?}",
            inner["CatalogIdentifier"]
        );
        assert_eq!(
            inner["CatalogType"],
            json!(FEDERATED),
            "case {spelling:?}: a catalog pointing outside the Data Catalog is {FEDERATED}, \
             got {:?}",
            inner["CatalogType"]
        );
        assert_eq!(
            container["CatalogType"],
            json!(NATIVE),
            "case {spelling:?}: the account's Data Catalog is {NATIVE}, got {:?}",
            container["CatalogType"]
        );
    }
}
