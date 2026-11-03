// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for four rules of the Glue catalog-entity surface that the
//! round-trip suite beside this one ([`crate::catalog_resources`]) does not pin:
//!
//! 1. `CreateCatalog` registers only a catalog that names a source OUTSIDE the Glue Data
//!    Catalog — a `FederatedCatalog` or a `TargetRedshiftCatalog`, the two members the
//!    modelled shape describes ("a logical grouping of databases in the Glue Data Catalog
//!    or a federated source … a Redshift-federated catalog or a catalog containing
//!    resource links to Redshift databases in another account or region" — `Catalog`,
//!    `smithy_query shape=Catalog`). An input naming neither describes a native catalog,
//!    which is refused with the modelled `InvalidInputException` and stores nothing.
//! 2. The data source a federated catalog points at must be one the account holds a
//!    registration for — the sources of AWS's own managed integrations, reached through
//!    the integration's own federation connection — and a create naming any other source
//!    is refused with the modelled `AccessDeniedException` and stores nothing.
//! 3. The account's own Data Catalog — the catalog a request that names no `CatalogId`
//!    resolves to — is readable by the account's own id, on an emulator where no create
//!    ever ran.
//! 4. A catalog miss is refused with ONE sentence, the same for every id that misses: the
//!    refusal carries nothing read off the request.
//!
//! Each suite is a table. The rows of the refusal suites differ along which members the
//! `CatalogInput` carried and along the form, length and qualification of the id that
//! missed; the rows of the read suites differ along the Region the read is served in and
//! along what else the store holds.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok, ACCOUNT_ID,
    REGION};

/// The refusal a `CreateCatalog` earns whose input names no source outside the Glue Data
/// Catalog. Recorded wording: the whole sentence is fixed, naming neither the catalog
/// asked for nor any member of the input.
const NATIVE_CATALOG_REFUSAL: &str = "Create glue native catalog is not supported.";

/// The refusal a read of a catalog this account holds none of earns. Recorded wording:
/// the whole sentence is fixed and the id the request addressed is withheld.
const CATALOG_MISS_REFUSAL: &str = "Catalog not found.";

/// The longest `Name` the model accepts: "len 1..64" on `CatalogNameString`
/// (`smithy_query operation=CreateCatalog`).
const MAX_CATALOG_NAME_LENGTH: usize = 64;

/// A `CreateConnection` body registering the connection `name` in this account's Data
/// Catalog, which is how an account registers a federation source of its own.
fn connection_body(name: &str) -> Value {
    json!({
        "ConnectionInput": {
            "Name": name,
            "ConnectionType": "JDBC",
            "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:redshift://host:5439/dev" },
        },
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

// ---------------------------------------------------------------------------
// Only a catalog that names a source outside the Data Catalog is created
// ---------------------------------------------------------------------------

/// A `CatalogInput` that names neither a `FederatedCatalog` nor a `TargetRedshiftCatalog`
/// describes a native catalog, which `CreateCatalog` does not create: the request is
/// refused with the modelled `InvalidInputException` and the fixed refusal, and nothing is
/// persisted under the name it asked for.
///
/// The rows differ along which OTHER members the input carried — none, parameters that
/// describe a federation without naming one, a description, catalog properties, default
/// permissions, the external-access member — and along whether the create carried tags.
#[tokio::test]
async fn a_create_naming_no_source_outside_the_data_catalog_is_refused() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            "an input carrying nothing at all",
            json!({ "Name": "native-bare", "CatalogInput": {} }),
        ),
        (
            "parameters that describe a federation source without naming one",
            json!({
                "Name": "native-parameters",
                "CatalogInput": {
                    "Parameters": {
                        "federation.source.host": "example.us-east-1.redshift.amazonaws.com",
                        "federation.source.port": "5439",
                    },
                },
            }),
        ),
        (
            "a description alone",
            json!({
                "Name": "native-described",
                "CatalogInput": { "Description": "a catalog of this account's own" },
            }),
        ),
        (
            "catalog properties alone",
            json!({
                "Name": "native-properties",
                "CatalogInput": {
                    "CatalogProperties": {
                        "DataLakeAccessProperties": { "DataLakeAccess": true },
                        "CustomProperties": { "tier": "gold" },
                    },
                },
            }),
        ),
        (
            "default permissions alone",
            json!({
                "Name": "native-governed",
                "CatalogInput": {
                    "CreateDatabaseDefaultPermissions": [],
                    "CreateTableDefaultPermissions": [],
                },
            }),
        ),
        (
            "the external-access member alone",
            json!({
                "Name": "native-external-access",
                "CatalogInput": { "AllowFullTableExternalDataAccess": "True" },
            }),
        ),
        (
            "tags beside an input that names no source",
            json!({
                "Name": "native-tagged",
                "CatalogInput": { "Parameters": { "owner": "analytics" } },
                "Tags": { "owner": "data-platform" },
            }),
        ),
    ];

    for (case, request) in &cases {
        let (status, body) = call(&router, "CreateCatalog", request).await;
        assert_eq!(
            status, 400,
            "case {case}: a native catalog must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            NATIVE_CATALOG_REFUSAL,
            "case {case}: expected the fixed refusal, got {:?}",
            error_message(&body)
        );

        // The refusal came before any write, so the name it asked for still names no
        // catalog — and the read that proves it is the catalog miss, not a success.
        let name = request["Name"].as_str().expect("the case carries a Name");
        let (status, body) = call(&router, "GetCatalog", &json!({ "CatalogId": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must have stored nothing under {name:?}, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {}",
            error_code(&body)
        );
    }
}

/// A `CatalogInput` that DOES name a source outside the Glue Data Catalog, and names one
/// this account holds a registration for, is served, and the member it named is what a
/// read reports back: the rule above turns on that member alone, not on anything else the
/// input carried.
///
/// The rows differ along which of the two members names the source, which managed
/// integration the federation is reached through, how fully the member is filled in, and
/// whether both are named at once.
#[tokio::test]
async fn a_create_naming_a_source_outside_the_data_catalog_is_served() {
    let router = emulator();

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a managed integration's connection and an identifier",
            json!({
                "Name": "federated-identifier",
                "CatalogInput": {
                    "FederatedCatalog": {
                        "Identifier": format!(
                            "arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/analytics"
                        ),
                        "ConnectionName": "aws:s3tables",
                    },
                },
            }),
            "FederatedCatalog",
        ),
        (
            "a managed integration's connection alone",
            json!({
                "Name": "federated-connection",
                "CatalogInput": { "FederatedCatalog": { "ConnectionName": "aws:s3tables" } },
            }),
            "FederatedCatalog",
        ),
        (
            "another managed integration's connection, named under the same prefix",
            json!({
                "Name": "federated-other-integration",
                "CatalogInput": { "FederatedCatalog": { "ConnectionName": "aws:redshift" } },
            }),
            "FederatedCatalog",
        ),
        (
            "a managed integration's connection with its identifier and type",
            json!({
                "Name": "federated-full",
                "CatalogInput": {
                    "Description": "a federated Redshift catalog",
                    "Parameters": { "federation.source.port": "5439" },
                    "FederatedCatalog": {
                        "Identifier": format!(
                            "arn:aws:redshift:{REGION}:{ACCOUNT_ID}:namespace/orders"
                        ),
                        "ConnectionName": "aws:redshift",
                        "ConnectionType": "REDSHIFT",
                    },
                },
            }),
            "FederatedCatalog",
        ),
        (
            "a Redshift resource link alone",
            json!({
                "Name": "redshift-link",
                "CatalogInput": {
                    "TargetRedshiftCatalog": {
                        "CatalogArn": format!(
                            "arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog/remote"
                        ),
                    },
                },
            }),
            "TargetRedshiftCatalog",
        ),
        (
            "both members at once",
            json!({
                "Name": "federated-and-linked",
                "CatalogInput": {
                    "FederatedCatalog": { "ConnectionName": "aws:s3tables" },
                    "TargetRedshiftCatalog": {
                        "CatalogArn": format!(
                            "arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog/linked"
                        ),
                    },
                },
            }),
            "TargetRedshiftCatalog",
        ),
    ];

    for (case, request, source_member) in &cases {
        ok(&router, "CreateCatalog", request).await;

        let name = request["Name"].as_str().expect("the case carries a Name");
        let read = get_in(&router, REGION, name).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: expected the stored catalog {name:?}, got {read}"
        );
        assert_eq!(
            read.get(*source_member),
            request["CatalogInput"].get(*source_member),
            "case {case}: {source_member} must come back as the create sent it, got {read}"
        );
    }
}

/// The data source a federated catalog points at must be one this account holds a
/// registration for: the only sources an account holds one for without registering
/// anything are those of AWS's own managed integrations, reached through the integration's
/// own federation connection. A create naming any other source is refused as unauthorized,
/// stores nothing, and stays refused after the account creates the Glue connection the
/// federation names — a connection TO a source is not a registration OF it.
///
/// The rows differ along how the source goes unregistered (no connection at all, a
/// connection nothing created, a connection whose name only resembles a managed one) and
/// along the form of the identifier the refusal names.
#[tokio::test]
async fn a_create_on_an_unregistered_federation_source_is_refused() {
    let router = emulator();

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "no connection at all, so nothing reaches the source",
            "arn:aws:redshift:us-east-1:000000000000:namespace/unregistered",
            json!({ "Identifier": "arn:aws:redshift:us-east-1:000000000000:namespace/unregistered" }),
        ),
        (
            "a connection nothing created",
            "arn:aws:s3tables:us-east-1:000000000000:bucket/unregistered",
            json!({
                "Identifier": "arn:aws:s3tables:us-east-1:000000000000:bucket/unregistered",
                "ConnectionName": "never-created",
            }),
        ),
        (
            "a connection whose name only resembles a managed one",
            "redshift-cluster-1.000000000000.us-east-1.redshift.amazonaws.com",
            json!({
                "Identifier": "redshift-cluster-1.000000000000.us-east-1.redshift.amazonaws.com",
                "ConnectionName": "aws-s3tables",
                "ConnectionType": "REDSHIFT",
            }),
        ),
        (
            "a connection nothing created and no identifier, so the connection names the source",
            "orphan-connection",
            json!({ "ConnectionName": "orphan-connection", "ConnectionType": "REDSHIFT" }),
        ),
    ];

    for (index, (case, source, federated)) in cases.iter().enumerate() {
        let name = format!("unregistered-{index}");
        let request = json!({
            "Name": name,
            "CatalogInput": { "FederatedCatalog": federated },
        });

        let (status, body) = call(&router, "CreateCatalog", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: an unregistered source must be refused with 400, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("AccessDeniedException"),
            "case {case}: expected AccessDeniedException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("The data source [{source}] is not registered."),
            "case {case}: the refusal must name the source the input pointed at, got {:?}",
            error_message(&body)
        );

        // The refusal came before any write, so the name is still free.
        let (status, body) = call(&router, "GetCatalog", &json!({ "CatalogId": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must have stored nothing under {name:?}, got \
             {status} with {body}"
        );

        // Creating the Glue connection the federation names changes nothing: a connection
        // TO a source is not a registration OF it, so the same create is refused again,
        // with the same sentence, and still stores nothing.
        if let Some(connection) = federated.get("ConnectionName").and_then(Value::as_str) {
            ok(&router, "CreateConnection", &connection_body(connection)).await;
            let (status, body) = call(&router, "CreateCatalog", &request).await;
            assert_eq!(
                status, 400,
                "case {case}: a connection of this account's own must not register the \
                 source, got {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("The data source [{source}] is not registered."),
                "case {case}: expected the same refusal after the connection exists, got {:?}",
                error_message(&body)
            );
            let (status, _) = call(&router, "GetCatalog", &json!({ "CatalogId": name })).await;
            assert_eq!(
                status, 400,
                "case {case}: the second refused create must have stored nothing under \
                 {name:?}"
            );
        }
    }

    // A federation through one of AWS's own managed integrations is served on the very
    // same emulator, so the refusals above turn on the registration and not on federation
    // being refused outright.
    ok(
        &router,
        "CreateCatalog",
        &json!({
            "Name": "registered-source",
            "CatalogInput": {
                "FederatedCatalog": {
                    "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/ok"),
                    "ConnectionName": "aws:s3tables",
                },
            },
        }),
    )
    .await;
    let read = get_in(&router, REGION, "registered-source").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("registered-source"),
        "case a registered source: the create must be served and stored, got {read}"
    );
}

// ---------------------------------------------------------------------------
// The account's own Data Catalog
// ---------------------------------------------------------------------------

/// The account's own Data Catalog is readable by the account's own id on an emulator where
/// no create ever ran: it is the catalog a request that names no `CatalogId` resolves to,
/// so it exists for the account this endpoint serves rather than being a record a create
/// wrote. Its name and id are the account's number, and its ARN is the Data Catalog's own.
///
/// The rows are the Regions the read is served in, because a Glue record carries no Region
/// of its own and the ARN is minted per request.
#[tokio::test]
async fn the_accounts_own_data_catalog_is_read_by_the_account_id() {
    let router = emulator();

    for region in ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-1"] {
        let read = get_in(&router, region, ACCOUNT_ID).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case {region}: expected the account's own id as the Name, got {read}"
        );
        assert_eq!(
            read.get("CatalogId").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case {region}: expected the account's own id as the CatalogId, got {read}"
        );
        let expected = format!("arn:aws:glue:{region}:{ACCOUNT_ID}:catalog");
        assert_eq!(
            read.get("ResourceArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region}: expected the Data Catalog's own ARN {expected:?}, got {read}"
        );
        // No `CreateCatalog` wrote this catalog, so it carries no definition a
        // `CatalogInput` could have set and no instant at which it was written. Its
        // description, its kind and its collection members are Glue's own, not a
        // client's, and are pinned by [`crate::catalog_structure`].
        for member in [
            "Parameters",
            "FederatedCatalog",
            "TargetRedshiftCatalog",
            "CreateTime",
            "UpdateTime",
        ] {
            assert!(
                read.get(member).is_none_or(Value::is_null),
                "case {region}: the account's Data Catalog carries no {member}, got {read}"
            );
        }
    }
}

/// Reading the account's own Data Catalog reports it and changes nothing a later caller
/// sees: the read repeated answers the same body, the catalogs stored inside it are
/// untouched and still their own records, and an id that missed before the read misses
/// after it.
#[tokio::test]
async fn reading_the_accounts_data_catalog_changes_nothing() {
    let router = emulator();
    ok(
        &router,
        "CreateCatalog",
        &json!({
            "Name": "inside-the-account",
            "CatalogInput": { "FederatedCatalog": { "ConnectionName": "aws:s3tables" } },
        }),
    )
    .await;

    let first = get_in(&router, REGION, ACCOUNT_ID).await;
    for attempt in ["second", "third"] {
        let again = get_in(&router, REGION, ACCOUNT_ID).await;
        assert_eq!(
            again, first,
            "case {attempt} read: the account's Data Catalog must report the same body, got \
             {again}"
        );
    }

    // The catalog created INSIDE the account is a record of its own: the account's id
    // reports the Data Catalog, not that record, and the record is still readable.
    assert_ne!(
        first.get("Name").and_then(Value::as_str),
        Some("inside-the-account"),
        "the account's id must not report a catalog created inside it, got {first}"
    );
    let child = get_in(&router, REGION, "inside-the-account").await;
    assert_eq!(
        child.get("CatalogId").and_then(Value::as_str),
        Some(format!("{ACCOUNT_ID}:inside-the-account").as_str()),
        "the catalog inside the account keeps its own id, got {child}"
    );

    // The read materialised no record under any other id either.
    let (status, body) = call(&router, "GetCatalog", &json!({ "CatalogId": "never-created" })).await;
    assert_eq!(
        status, 400,
        "a read of the account's Data Catalog must leave other ids missing, got {status} with \
         {body}"
    );
}

// ---------------------------------------------------------------------------
// One fixed sentence for every catalog miss
// ---------------------------------------------------------------------------

/// Every read of a catalog this account holds none of is refused with the SAME sentence:
/// the refusal is fixed text and carries nothing read off the request — not the id, not
/// the account that was searched.
///
/// The rows differ along the form, casing, length and qualification of the id that misses,
/// including a near miss of a stored catalog's name, so the sentence is pinned for an id
/// that nearly resolves as much as for one nothing resembles.
#[tokio::test]
async fn every_catalog_miss_is_refused_with_one_fixed_sentence() {
    let router = emulator();
    ok(
        &router,
        "CreateCatalog",
        &json!({
            "Name": "stored",
            "CatalogInput": { "FederatedCatalog": { "ConnectionName": "aws:s3tables" } },
        }),
    )
    .await;

    let long_name = "m".repeat(MAX_CATALOG_NAME_LENGTH);
    for (case, catalog_id) in [
        ("never created", "no-such-catalog".to_string()),
        ("a prefix of a stored name", "stor".to_string()),
        ("a mixed-case miss", "MissingCatalog".to_string()),
        ("a name carrying a colon", "team:missing".to_string()),
        ("an id at the modelled name length", long_name),
        ("eleven digits, which is no account number", "99999999999".to_string()),
        (
            "this account and a name nothing created",
            format!("{ACCOUNT_ID}:absent"),
        ),
    ] {
        let (status, body) = call(&router, "GetCatalog", &json!({ "CatalogId": catalog_id })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {catalog_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {catalog_id:?}, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            CATALOG_MISS_REFUSAL,
            "case {case}: expected the fixed refusal for {catalog_id:?}, got {:?}",
            error_message(&body)
        );
        assert!(
            !error_message(&body).contains(&catalog_id),
            "case {case}: the refusal must disclose nothing of {catalog_id:?}, got {:?}",
            error_message(&body)
        );
    }

    // The stored catalog still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get_in(&router, REGION, "stored").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("stored"),
        "case the stored name: expected the stored catalog, got {read}"
    );
}
