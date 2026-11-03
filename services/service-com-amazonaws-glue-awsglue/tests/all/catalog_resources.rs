// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue catalog-resource slice: `CreateCatalog` and the
//! `GetCatalog` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip suite is a table: each row is one input along the axis the rule
//! generalises over (which members of the `CatalogInput` the create carried, the form of
//! the federation it points at, the size and content of the parameter and property maps,
//! the form and length of the name, whether tags were sent), and the federated catalog a
//! lake-formation client registers is one row among them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    CatalogDefinition, ComAmazonawsGlueAwsglueState, MAX_CATALOGS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, error_message, ok,
    ACCOUNT_ID, REGION};

/// The longest `Name` the model accepts: "len 1..64" on `CatalogNameString`
/// (`smithy_query operation=CreateCatalog`).
const MAX_CATALOG_NAME_LENGTH: usize = 64;

/// A `FederatedCatalog` naming the source outside the Glue Data Catalog that the catalog
/// `name` points at.
///
/// `CreateCatalog` registers only a catalog that names such a source, so every row below
/// that is not a Redshift resource link carries one; the rule itself is pinned by
/// [`crate::federated_catalogs`].
fn federation_source(name: &str) -> Value {
    json!({
        "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
        "ConnectionName": "aws:s3tables",
    })
}

/// Create a catalog and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateCatalog", body).await
}

/// The `Catalog` a `GetCatalog` for `catalog_id` reports.
async fn get(router: &MultiServiceRouter, catalog_id: &str) -> Value {
    let read = ok(router, "GetCatalog", &json!({ "CatalogId": catalog_id })).await;
    read.get("Catalog")
        .cloned()
        .unwrap_or_else(|| panic!("GetCatalog must report a Catalog, got {read}"))
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateCatalog` persists is what `GetCatalog` reports: every member of the
/// `CatalogInput` the request carried comes back from the store unchanged, beside the
/// three members the server sets — the id the catalog is addressed by, its ARN and the
/// two instants of the write.
///
/// The rows differ along which members the input carried, the form of the federation it
/// names, the size and content of its parameter and property maps, the form and length of
/// the name, and whether the create carried tags.
#[tokio::test]
async fn created_catalogs_are_read_back_from_the_store() {
    let router = emulator();

    let long_name = "c".repeat(MAX_CATALOG_NAME_LENGTH);
    let many_parameters: serde_json::Map<String, Value> = (0..32)
        .map(|index| (format!("parameter-{index}"), json!(format!("value-{index}"))))
        .collect();

    let cases: Vec<(&str, Value)> = vec![
        (
            // The shape a lifecycle client sends: the required members alone.
            "a CatalogInput naming its federation source and nothing else",
            json!({
                "Name": "bare-catalog",
                "CatalogInput": { "FederatedCatalog": federation_source("bare-catalog") },
            }),
        ),
        (
            "a federated catalog, as a lake-formation client registers one",
            json!({
                "Name": "federated-catalog",
                "CatalogInput": {
                    "FederatedCatalog": {
                        "Identifier": "arn:aws:s3tables:us-east-1:000000000000:bucket/analytics",
                        "ConnectionName": "aws:s3tables",
                    },
                    "CreateDatabaseDefaultPermissions": [],
                    "CreateTableDefaultPermissions": [],
                },
            }),
        ),
        (
            "a federated catalog naming its connection type too",
            json!({
                "Name": "redshift-federated",
                "CatalogInput": {
                    "Description": "a federated Redshift catalog",
                    "FederatedCatalog": {
                        "Identifier": "arn:aws:redshift:us-east-1:000000000000:namespace/orders",
                        "ConnectionName": "aws:redshift",
                        "ConnectionType": "REDSHIFT",
                    },
                },
            }),
        ),
        (
            "parameters and a description",
            json!({
                "Name": "parameterised",
                "CatalogInput": {
                    "Description": "a catalog carrying parameters",
                    "Parameters": { "owner": "analytics", "tier": "gold" },
                    "FederatedCatalog": federation_source("parameterised"),
                },
            }),
        ),
        (
            "an empty parameter map, which is not the same as none",
            json!({
                "Name": "empty-parameters",
                "CatalogInput": {
                    "Parameters": {},
                    "FederatedCatalog": federation_source("empty-parameters"),
                },
            }),
        ),
        (
            "many parameters",
            json!({
                "Name": "many-parameters",
                "CatalogInput": {
                    "Parameters": Value::Object(many_parameters),
                    "FederatedCatalog": federation_source("many-parameters"),
                },
            }),
        ),
        (
            "catalog properties: data lake access, iceberg optimisation and custom keys",
            json!({
                "Name": "property-rich",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("property-rich"),
                    "CatalogProperties": {
                        "DataLakeAccessProperties": {
                            "DataLakeAccess": true,
                            "DataTransferRole": "arn:aws:iam::000000000000:role/glue-transfer",
                            "KmsKey": "arn:aws:kms:us-east-1:000000000000:key/abc",
                            "CatalogType": "aws:redshift",
                        },
                        "IcebergOptimizationProperties": {
                            "RoleArn": "arn:aws:iam::000000000000:role/iceberg",
                            "Compaction": { "strategy": "binpack" },
                            "Retention": { "snapshot_retention_period_in_days": "7" },
                            "OrphanFileDeletion": { "orphan_file_retention_period_in_days": "3" },
                        },
                        "CustomProperties": { "column_statistics": "enabled" },
                    },
                },
            }),
        ),
        (
            "data lake access turned off, which is the member's modelled default",
            json!({
                "Name": "no-data-lake-access",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("no-data-lake-access"),
                    "CatalogProperties": {
                        "DataLakeAccessProperties": { "DataLakeAccess": false },
                    },
                },
            }),
        ),
        (
            "a resource link to another catalog",
            json!({
                "Name": "linked-catalog",
                "CatalogInput": {
                    "TargetRedshiftCatalog": {
                        "CatalogArn": "arn:aws:glue:us-east-1:000000000000:catalog/remote",
                    },
                    "AllowFullTableExternalDataAccess": "True",
                },
            }),
        ),
        (
            "default permissions that grant something",
            json!({
                "Name": "governed-catalog",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("governed-catalog"),
                    "CreateDatabaseDefaultPermissions": [{
                        "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                        "Permissions": ["ALL"],
                    }],
                    "CreateTableDefaultPermissions": [{
                        "Principal": { "DataLakePrincipalIdentifier": "IAM_ALLOWED_PRINCIPALS" },
                        "Permissions": ["SELECT", "ALTER"],
                    }],
                },
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({
                "Name": long_name,
                "CatalogInput": {
                    "Description": "the longest name",
                    "FederatedCatalog": federation_source("longest"),
                },
            }),
        ),
        (
            "a name carrying a space and a tab",
            json!({
                "Name": "catalog one\ttwo",
                "CatalogInput": { "FederatedCatalog": federation_source("spaced") },
            }),
        ),
        (
            "tags sent with the create",
            json!({
                "Name": "tagged-catalog",
                "CatalogInput": {
                    "Parameters": { "kept": "yes" },
                    "FederatedCatalog": federation_source("tagged-catalog"),
                },
                "Tags": { "owner": "data-platform", "tier": "silver" },
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        create(&router, request).await;

        let read = get(&router, name).await;
        // "The name of the catalog": reported as the create spelled it.
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: expected Name {name:?}, got {read}"
        );
        // "The ID of the catalog": the account the catalog was created in and its name,
        // which is the id every other operation addresses it by.
        assert_eq!(
            read.get("CatalogId").and_then(Value::as_str),
            Some(format!("{ACCOUNT_ID}:{name}").as_str()),
            "case {case}: expected the catalog's own id, got {read}"
        );
        // "The Amazon Resource Name (ARN) assigned to the catalog resource", in the
        // Region the read was served in.
        assert_eq!(
            read.get("ResourceArn").and_then(Value::as_str),
            Some(format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog/{name}").as_str()),
            "case {case}: expected the catalog's ARN, got {read}"
        );

        // Every member of the definition the request carried reappears unchanged. The
        // input's `CatalogProperties` is reported through the OUTPUT shape of the same
        // properties, so the members it shares with the input are compared on their own.
        let sent = &request["CatalogInput"];
        let expected = json!({
            "Description": sent.get("Description").cloned().unwrap_or(Value::Null),
            "Parameters": sent.get("Parameters").cloned().unwrap_or(Value::Null),
            "FederatedCatalog": sent.get("FederatedCatalog").cloned().unwrap_or(Value::Null),
            "TargetRedshiftCatalog": sent
                .get("TargetRedshiftCatalog")
                .cloned()
                .unwrap_or(Value::Null),
            // The properties map and both default-permission lists are reported
            // DEFINITELY: a create that configured none of them reads back as the empty
            // map and the empty lists, never as absent members.
            "CreateTableDefaultPermissions": sent
                .get("CreateTableDefaultPermissions")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "CreateDatabaseDefaultPermissions": sent
                .get("CreateDatabaseDefaultPermissions")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "AllowFullTableExternalDataAccess": sent
                .get("AllowFullTableExternalDataAccess")
                .cloned()
                .unwrap_or(Value::Null),
            "CatalogProperties": sent
                .get("CatalogProperties")
                .cloned()
                .unwrap_or_else(|| json!({})),
        });
        assert_echoed(case, &expected, &read);

        // "The time at which the catalog was created" / "was last updated": both set by
        // the write, so a catalog nothing has updated reports one instant twice.
        let create_time = read.get("CreateTime").and_then(Value::as_f64);
        assert!(
            create_time.is_some_and(|at| at > 0.0),
            "case {case}: expected a populated CreateTime, got {read}"
        );
        assert_eq!(
            read.get("UpdateTime").and_then(Value::as_f64),
            create_time,
            "case {case}: an unmodified catalog's UpdateTime equals its CreateTime, got {read}"
        );

        // `CreateCatalogResponse` models no member, so the create discloses nothing and
        // the tags a create carried are not part of the catalog a read reports
        // (`Catalog` carries no `Tags` member).
        assert!(
            read.get("Tags").is_none(),
            "case {case}: GetCatalog must report no Tags, got {read}"
        );
    }

    // Each catalog is its own record: reading one back after every create has landed
    // still reports that catalog's own definition, so one create never overwrote another.
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert_eq!(
            read.get("Description"),
            request["CatalogInput"].get("Description"),
            "case {case}: each catalog keeps its own Description, got {read}"
        );
    }
}

/// A catalog with no `IcebergOptimizationProperties` reports none, and one that carries
/// them reports the instant they were last written beside them: the output shape's
/// `LastUpdatedTime` is server-set, so it is filled only where properties were stored.
#[tokio::test]
async fn iceberg_optimization_properties_report_when_they_were_written() {
    let router = emulator();

    for (case, request, expects_properties) in [
        (
            "no catalog properties at all",
            json!({
                "Name": "no-properties",
                "CatalogInput": { "FederatedCatalog": federation_source("no-properties") },
            }),
            false,
        ),
        (
            "catalog properties without the iceberg section",
            json!({
                "Name": "custom-properties-only",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("custom-properties-only"),
                    "CatalogProperties": { "CustomProperties": { "a": "b" } },
                },
            }),
            false,
        ),
        (
            "an iceberg section",
            json!({
                "Name": "iceberg-properties",
                "CatalogInput": {
                    "FederatedCatalog": federation_source("iceberg-properties"),
                    "CatalogProperties": {
                        "IcebergOptimizationProperties": {
                            "RoleArn": "arn:aws:iam::000000000000:role/iceberg",
                        },
                    },
                },
            }),
            true,
        ),
    ] {
        create(&router, &request).await;
        let name = request["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        let last_updated = read
            .pointer("/CatalogProperties/IcebergOptimizationProperties/LastUpdatedTime")
            .and_then(Value::as_f64);
        assert_eq!(
            last_updated.is_some(),
            expects_properties,
            "case {case}: expected LastUpdatedTime present={expects_properties}, got {read}"
        );
        if expects_properties {
            assert_eq!(
                last_updated,
                read.get("UpdateTime").and_then(Value::as_f64),
                "case {case}: the properties were written by this create, so their \
                 LastUpdatedTime is the catalog's own UpdateTime, got {read}"
            );
        }
    }
}

/// This emulator provisions no managed Redshift Serverless compute for a catalog, so the
/// output members that would report one stay unset even where data lake access is asked
/// for: a name or a status for a compute nothing created would describe something that
/// does not exist.
#[tokio::test]
async fn no_managed_compute_is_reported_for_a_data_lake_access_catalog() {
    let router = emulator();

    for (case, data_lake_access) in [("access on", true), ("access off", false)] {
        let name = format!("managed-{}", data_lake_access);
        create(
            &router,
            &json!({
                "Name": name,
                "CatalogInput": {
                    "FederatedCatalog": federation_source(&name),
                    "CatalogProperties": {
                        "DataLakeAccessProperties": { "DataLakeAccess": data_lake_access },
                    },
                },
            }),
        )
        .await;

        let read = get(&router, &name).await;
        let properties = read
            .pointer("/CatalogProperties/DataLakeAccessProperties")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            properties.get("DataLakeAccess").and_then(Value::as_bool),
            Some(data_lake_access),
            "case {case}: DataLakeAccess must round-trip, got {read}"
        );
        for member in [
            "ManagedWorkgroupName",
            "ManagedWorkgroupStatus",
            "RedshiftDatabaseName",
            "StatusMessage",
        ] {
            assert!(
                properties.get(member).is_none_or(Value::is_null),
                "case {case}: expected no {member}, got {read}"
            );
        }
    }
}

/// A catalog is reachable under any casing of its name, and under its full id — the
/// account that holds it and the name — because that id is what the catalog reports and
/// what every other operation addresses it by ("This should be all lowercase" —
/// `GetCatalog`).
#[tokio::test]
async fn a_catalog_is_reached_by_name_and_by_its_full_id() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "MixedCaseCatalog",
            "CatalogInput": {
                "Description": "reached under any casing",
                "FederatedCatalog": federation_source("mixedcasecatalog"),
            },
        }),
    )
    .await;

    for (case, catalog_id) in [
        ("the name as created", "MixedCaseCatalog".to_string()),
        ("the name lowercased", "mixedcasecatalog".to_string()),
        ("the name uppercased", "MIXEDCASECATALOG".to_string()),
        (
            "the account and the name",
            format!("{ACCOUNT_ID}:MixedCaseCatalog"),
        ),
        (
            "the account and the name lowercased",
            format!("{ACCOUNT_ID}:mixedcasecatalog"),
        ),
    ] {
        let read = get(&router, &catalog_id).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some("MixedCaseCatalog"),
            "case {case}: expected the stored catalog for {catalog_id:?}, got {read}"
        );
    }

    // A catalog name may carry a colon of its own — `CatalogNameString`'s pattern forbids
    // the three path separators and the `aws:` prefix, not a colon — so the account
    // prefix of an id is told apart from the name by being an account number.
    create(
        &router,
        &json!({
            "Name": "team:catalog",
            "CatalogInput": {
                "Description": "a colon inside",
                "FederatedCatalog": federation_source("team-catalog"),
            },
        }),
    )
    .await;
    for (case, catalog_id) in [
        ("a name carrying a colon", "team:catalog".to_string()),
        (
            "the account and a name carrying a colon",
            format!("{ACCOUNT_ID}:team:catalog"),
        ),
    ] {
        let read = get(&router, &catalog_id).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some("team:catalog"),
            "case {case}: expected the stored catalog for {catalog_id:?}, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A catalog name is unique inside the Data Catalog that holds it: the second create
/// under a name already taken is refused with the modelled `AlreadyExistsException`, and
/// the stored catalog is left as the first create wrote it. The rows differ along how the
/// second create spells the same name and what definition it carries.
#[tokio::test]
async fn a_duplicate_catalog_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the same name, a different description",
            json!({
                "Name": "duplicate-one",
                "CatalogInput": {
                    "Description": "first",
                    "FederatedCatalog": federation_source("duplicate-one"),
                },
            }),
            json!({
                "Name": "duplicate-one",
                "CatalogInput": {
                    "Description": "second",
                    "FederatedCatalog": federation_source("duplicate-one"),
                },
            }),
        ),
        (
            "the same name in another casing",
            json!({
                "Name": "duplicate-two",
                "CatalogInput": {
                    "Description": "first",
                    "FederatedCatalog": federation_source("duplicate-two"),
                },
            }),
            json!({
                "Name": "DUPLICATE-TWO",
                "CatalogInput": {
                    "Description": "second",
                    "FederatedCatalog": federation_source("duplicate-two"),
                },
            }),
        ),
        (
            "the same name and the identical definition",
            json!({
                "Name": "duplicate-three",
                "CatalogInput": {
                    "Description": "same",
                    "FederatedCatalog": federation_source("duplicate-three"),
                },
            }),
            json!({
                "Name": "duplicate-three",
                "CatalogInput": {
                    "Description": "same",
                    "FederatedCatalog": federation_source("duplicate-three"),
                },
            }),
        ),
    ];

    for (case, first, second) in &cases {
        create(&router, first).await;

        let (status, body) = call(&router, "CreateCatalog", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );

        let name = first["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert_eq!(
            read.get("Description"),
            first["CatalogInput"].get("Description"),
            "case {case}: the refused create must not have changed the stored catalog, got {read}"
        );
    }
}

/// A `Name` outside the `@pattern` its modelled type declares is refused with the
/// modelled `InvalidInputException` and stores nothing: the pattern forbids the three
/// path separators anywhere in the name and the `aws:` prefix AWS reserves
/// (`^(?!(.*[.\/\\]|aws:)).*$` on `CatalogNameString`, `smithy_query
/// operation=CreateCatalog`). Its negative look-ahead cannot be compiled by the
/// framework's regex engine, so the handler answers it.
///
/// The rows run over each half of the pattern and over the near misses neither half
/// excludes.
#[tokio::test]
async fn a_catalog_name_outside_its_modelled_pattern_is_refused() {
    let router = emulator();

    for (case, name) in [
        ("a dot inside the name", "my.catalog"),
        ("a dot at the end", "catalog."),
        ("a dot at the start", ".catalog"),
        ("a forward slash", "s3tablescatalog/analytics"),
        ("a backslash", "my\\catalog"),
        ("the reserved prefix", "aws:catalog"),
        ("the reserved prefix alone", "aws:"),
    ] {
        let (status, body) = call(
            &router,
            "CreateCatalog",
            &json!({
                "Name": name,
                "CatalogInput": { "FederatedCatalog": federation_source("patterns") },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException for {name:?}, got {body}"
        );

        // The refused create stored nothing, so the name still names no catalog.
        let (status, _) = call(&router, "GetCatalog", &json!({ "CatalogId": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must have stored nothing under {name:?}"
        );
    }

    // The near misses the pattern does NOT exclude: the reserved prefix without its
    // colon, those letters inside the name, and a colon that is not the prefix's.
    for (case, name) in [
        ("the reserved letters without the colon", "awscatalog"),
        ("the reserved letters as a word", "aws-catalog"),
        ("a colon that is not the prefix", "my-aws:catalog"),
        ("a dash and digits", "catalog-2"),
    ] {
        create(
            &router,
            &json!({
                "Name": name,
                "CatalogInput": { "FederatedCatalog": federation_source("near-misses") },
            }),
        )
        .await;
        let read = get(&router, name).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: {name:?} satisfies the pattern and must be stored, got {read}"
        );
    }
}

/// A catalog's name "Cannot be the same as the account ID" (`Catalog$Name`), because that
/// id already addresses the account's own Data Catalog: such a create is refused with the
/// modelled `InvalidInputException` and stores nothing.
#[tokio::test]
async fn a_catalog_named_after_the_account_is_refused() {
    let router = emulator();

    let (status, body) = call(
        &router,
        "CreateCatalog",
        &json!({
            "Name": ACCOUNT_ID,
            "CatalogInput": { "FederatedCatalog": federation_source("account-named") },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the account id as a name: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case the account id as a name: expected InvalidInputException, got {body}"
    );
    assert!(
        error_message(&body).contains("account ID"),
        "case the account id as a name: the refusal must state the requirement, got {body}"
    );

    // Nothing was stored under that id, so the account's own id still reports the
    // account's own Data Catalog — the container every account has, which carries no
    // definition a create wrote.
    let read = get(&router, ACCOUNT_ID).await;
    assert!(
        read.get("FederatedCatalog").is_none_or(Value::is_null),
        "case the account id as a name: the refused create must have stored nothing, got \
         {read}"
    );
}

/// A read for an id this account holds no catalog under is refused with the modelled
/// `EntityNotFoundException`. The rows differ along how the requested id misses: never
/// created, a prefix of a stored name, a suffix of one, and a name qualified by this
/// account that still names nothing.
#[tokio::test]
async fn reading_an_unknown_catalog_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "stored-catalog",
            "CatalogInput": { "FederatedCatalog": federation_source("stored-catalog") },
        }),
    )
    .await;

    for (case, catalog_id) in [
        ("never created", "no-such-catalog".to_string()),
        ("a prefix of a stored name", "stored".to_string()),
        ("a suffix of a stored name", "catalog".to_string()),
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
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {catalog_id:?}, got {body}"
        );
    }

    // The stored catalog still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get(&router, "stored-catalog").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("stored-catalog"),
        "case the stored name: expected the stored catalog, got {read}"
    );
}

/// A catalog in ANOTHER account's Data Catalog is refused as unauthorized rather than as
/// a miss: it is reachable on the real service only through that catalog's own
/// resource-based policy, and this store can hold no such grant. The rows differ along
/// the account named and whether this account holds a catalog of that name.
#[tokio::test]
async fn reading_another_accounts_catalog_is_refused_as_unauthorized() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "shared",
            "CatalogInput": { "FederatedCatalog": federation_source("shared") },
        }),
    )
    .await;

    for (case, catalog_id) in [
        (
            "another account, a name it does not hold",
            "111122223333:absent",
        ),
        ("another account, a name this one holds", "111122223333:shared"),
        // An id that is an account number and nothing else names that account's own Data
        // Catalog, which holds catalogs but is not one of them.
        ("another account's Data Catalog itself", "111122223333"),
    ] {
        let (status, body) = call(&router, "GetCatalog", &json!({ "CatalogId": catalog_id })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {catalog_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AccessDeniedException"),
            "case {case}: expected AccessDeniedException for {catalog_id:?}, got {body}"
        );
        assert!(
            error_message(&body).contains("glue:GetCatalog"),
            "case {case}: the refusal must name the action, got {body}"
        );
    }
}

/// The store holds a bounded number of catalogs: the create at the ceiling succeeds, the
/// one past it is refused with the modelled limit error, and freeing a slot lets a create
/// succeed again — so the ceiling bounds what is stored rather than latching on how many
/// creates were ever made.
#[tokio::test]
async fn a_create_past_the_catalog_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would exercise the transport rather than the
    // bound.
    let seeded: Vec<String> = (0..MAX_CATALOGS - 1)
        .map(|index| format!("{ACCOUNT_ID}:seeded-{index}"))
        .collect();
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for key in &seeded {
            let definition = CatalogDefinition::from_create_input(
                key,
                key.clone(),
                &Default::default(),
                None,
                std::time::SystemTime::now(),
            );
            let catalog = store
                .catalog_mut_for_create(key)
                .expect("the seeded catalogs are inside the ceiling");
            catalog.definition = Some(definition);
        }
    }

    let last = json!({
        "Name": "last-catalog",
        "CatalogInput": { "FederatedCatalog": federation_source("last-catalog") },
    });
    let past = json!({
        "Name": "one-too-many",
        "CatalogInput": { "FederatedCatalog": federation_source("one-too-many") },
    });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateCatalog", &past).await;
    assert_eq!(
        status, 400,
        "case catalog {} of {MAX_CATALOGS}: expected 400, got {status} with {body}",
        MAX_CATALOGS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one catalog too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().catalogs.remove(&seeded[0]);
    }
    create(&router, &past).await;
}
