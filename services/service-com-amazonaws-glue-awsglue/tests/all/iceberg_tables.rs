// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the open-table-format (Iceberg) create path of `CreateTable`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a `CreateTable` carrying an `OpenTableFormatInput` stores the table
//! like any other AND mints the Iceberg metadata for it — the table reads back with the
//! three parameters that make it an Iceberg table: `table_type`, the `metadata_location` of
//! the metadata file minted under the table's own location, and the `format-version` the
//! request asked for (2 when it asked for none). The minted location is stable across
//! reads and unique per table, and a create that names no location, or an
//! `OpenTableFormatInput` naming no format at all, is refused with `InvalidInputException`
//! having stored nothing.
//!
//! The rows below differ along what the rule generalises over: the Iceberg version asked
//! for, where the table's location is named, the form of that location, the other members
//! the table definition carries, and the way an Iceberg create can be malformed.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    DEFAULT_ICEBERG_FORMAT_VERSION, ICEBERG_FORMAT_VERSION_PARAMETER,
    ICEBERG_METADATA_LOCATION_PARAMETER, ICEBERG_TABLE_TYPE, ICEBERG_TABLE_TYPE_PARAMETER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, ok};

/// The database every row creates its table in.
const DATABASE: &str = "iceberg_db";

/// Read one table back, answering the HTTP status and body.
async fn get_table(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "GetTable",
        &json!({ "DatabaseName": DATABASE, "Name": name }),
    )
    .await
}

/// One table parameter of a read-back table.
fn parameter(table: &Value, name: &str) -> Option<String> {
    table["Table"]["Parameters"][name]
        .as_str()
        .map(str::to_string)
}

/// Assert that `location` is where Iceberg puts a table's FIRST metadata file: under the
/// table's own location, in `metadata/`, named by the commit it records (`00000`) and a
/// random id, with the `.metadata.json` suffix.
fn assert_metadata_location(case: &str, table_location: &str, location: &str) {
    let base = table_location.trim_end_matches('/');
    let expected_prefix = format!("{base}/metadata/00000-");
    assert!(
        location.starts_with(&expected_prefix),
        "case {case}: the metadata file lives under the table's location; expected the \
         prefix {expected_prefix}, got {location}"
    );
    assert!(
        location.ends_with(".metadata.json"),
        "case {case}: an Iceberg metadata file is a .metadata.json, got {location}"
    );
    let id = location
        .trim_start_matches(&expected_prefix)
        .trim_end_matches(".metadata.json");
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case}: the metadata file's id is spelled as a UUID, got {id}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case}: the metadata file's id is hexadecimal, got {id}"
    );
}

// ---------------------------------------------------------------------------
// The Iceberg create
// ---------------------------------------------------------------------------

/// An Iceberg create stores the table and mints its metadata, and the table reads back with
/// the parameters that make it an Iceberg table.
///
/// The rows differ along the version asked for (none, 1, 2), where the location is named
/// (on the `TableInput`'s storage descriptor, on the `CreateIcebergTableInput`), the form of
/// that location (with and without a trailing slash, nested prefixes), and the other
/// members the definition carries (client parameters, partition keys, columns).
#[tokio::test]
async fn an_iceberg_create_mints_the_metadata_and_the_table_reads_back() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;

    // (case, the request body minus DatabaseName/Name, the table location, the expected
    // format version)
    let cases: [(&str, Value, &str, &str); 5] = [
        (
            "no version, location on the table input",
            json!({
                "TableInput": {
                    "Name": "events",
                    "StorageDescriptor": { "Location": "s3://lake/events/" }
                },
                "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } }
            }),
            "s3://lake/events/",
            DEFAULT_ICEBERG_FORMAT_VERSION,
        ),
        (
            "version 2, location without a trailing slash",
            json!({
                "TableInput": {
                    "Name": "orders",
                    "StorageDescriptor": { "Location": "s3://lake/orders" }
                },
                "OpenTableFormatInput": {
                    "IcebergInput": { "MetadataOperation": "CREATE", "Version": "2" }
                }
            }),
            "s3://lake/orders",
            "2",
        ),
        (
            "version 1, a nested prefix and client parameters",
            json!({
                "TableInput": {
                    "Name": "clicks",
                    "Parameters": { "classification": "parquet", "owner_team": "analytics" },
                    "PartitionKeys": [ { "Name": "dt", "Type": "string" } ],
                    "StorageDescriptor": {
                        "Location": "s3://lake/raw/clicks/v1/",
                        "Columns": [ { "Name": "id", "Type": "bigint" } ]
                    }
                },
                "OpenTableFormatInput": {
                    "IcebergInput": { "MetadataOperation": "CREATE", "Version": "1" }
                }
            }),
            "s3://lake/raw/clicks/v1/",
            "1",
        ),
        (
            "the location named on the Iceberg input alone",
            json!({
                "TableInput": { "Name": "sessions" },
                "OpenTableFormatInput": {
                    "IcebergInput": {
                        "MetadataOperation": "CREATE",
                        "CreateIcebergTableInput": {
                            "Location": "s3://lake/sessions/",
                            "Schema": {
                                "Fields": [
                                    { "Id": 1, "Name": "id", "Type": "long", "Required": true },
                                    {
                                        "Id": 2,
                                        "Name": "started_at",
                                        "Type": "timestamp",
                                        "Required": false
                                    }
                                ]
                            }
                        }
                    }
                }
            }),
            "s3://lake/sessions/",
            DEFAULT_ICEBERG_FORMAT_VERSION,
        ),
        (
            "no table input at all, the Iceberg input defining the table",
            json!({
                "Name": "visits",
                "OpenTableFormatInput": {
                    "IcebergInput": {
                        "MetadataOperation": "CREATE",
                        "Version": "2",
                        "CreateIcebergTableInput": {
                            "Location": "s3://lake/visits/",
                            "Schema": {
                                "Fields": [
                                    { "Id": 1, "Name": "visitor", "Type": "string", "Required": true }
                                ]
                            },
                            "Properties": { "write.format.default": "parquet" }
                        }
                    }
                }
            }),
            "s3://lake/visits/",
            "2",
        ),
    ];

    let mut minted_locations: Vec<String> = Vec::new();
    for (case, request, table_location, expected_version) in cases {
        let mut body = request.clone();
        body["DatabaseName"] = json!(DATABASE);
        ok(&router, "CreateTable", &body).await;

        let name = request["TableInput"]["Name"]
            .as_str()
            .or_else(|| request["Name"].as_str())
            .expect("the row names the table");
        let (status, read) = get_table(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case}: the Iceberg table must be readable, got {status} with {read}"
        );
        assert_eq!(
            parameter(&read, ICEBERG_TABLE_TYPE_PARAMETER).as_deref(),
            Some(ICEBERG_TABLE_TYPE),
            "case {case}: the table must read back as an open table format, got {read}"
        );
        assert_eq!(
            parameter(&read, ICEBERG_FORMAT_VERSION_PARAMETER).as_deref(),
            Some(expected_version),
            "case {case}: the format version the create asked for must be reported, got {read}"
        );
        let location = parameter(&read, ICEBERG_METADATA_LOCATION_PARAMETER)
            .unwrap_or_else(|| panic!("case {case}: the create must mint a metadata location"));
        assert_metadata_location(case, table_location, &location);
        assert_eq!(
            read["Table"]["StorageDescriptor"]["Location"].as_str(),
            Some(table_location),
            "case {case}: the stored table must carry the location its metadata was minted \
             under, got {read}"
        );

        // The client's own parameters survive beside the three the service sets.
        if let Some(sent) = request["TableInput"]["Parameters"].as_object() {
            for (member, value) in sent {
                assert_eq!(
                    parameter(&read, member).as_deref(),
                    value.as_str(),
                    "case {case}: the client parameter {member} must survive the Iceberg \
                     marker, got {read}"
                );
            }
        }

        // The minted location is STABLE: a second read serves the same metadata file.
        let (_, again) = get_table(&router, name).await;
        assert_eq!(
            parameter(&again, ICEBERG_METADATA_LOCATION_PARAMETER).as_deref(),
            Some(location.as_str()),
            "case {case}: the metadata location must not change between reads, got {again}"
        );
        minted_locations.push(location);
    }

    // Each table got a metadata file of its own.
    let unique: std::collections::BTreeSet<&String> = minted_locations.iter().collect();
    assert_eq!(
        unique.len(),
        minted_locations.len(),
        "each Iceberg table gets its own metadata file, got {minted_locations:?}"
    );
}

/// An Iceberg table created this way is a table like any other: it is listed, it can carry
/// an optimizer, and a plain `CreateTable` of the same name is the duplicate it would be
/// for any table.
#[tokio::test]
async fn an_iceberg_table_is_a_catalog_table_like_any_other() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": {
                "Name": "events",
                "StorageDescriptor": { "Location": "s3://lake/events/" }
            },
            "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } }
        }),
    )
    .await;

    let listed = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": DATABASE }),
    )
    .await;
    assert_eq!(
        crate::wire::table_names(&listed),
        vec!["events".to_string()],
        "the Iceberg table must be listed like any other, got {listed}"
    );

    // An optimizer can be registered on it, which is what an Iceberg table is for.
    ok(
        &router,
        "CreateTableOptimizer",
        &json!({
            "CatalogId": crate::wire::ACCOUNT_ID,
            "DatabaseName": DATABASE,
            "TableName": "events",
            "Type": "compaction",
            "TableOptimizerConfiguration": {
                "roleArn": "arn:aws:iam::000000000000:role/glue-table-optimizer",
                "enabled": true
            }
        }),
    )
    .await;

    let (status, duplicate) = call(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": { "Name": "events" }
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a table of that name already exists, so the create must be refused, got {status} \
         with {duplicate}"
    );
    assert!(
        error_code(&duplicate).ends_with("AlreadyExistsException"),
        "expected AlreadyExistsException, got {} in {duplicate}",
        error_code(&duplicate)
    );
}

/// An Iceberg create the service cannot mint metadata for is refused with
/// `InvalidInputException` and stores NOTHING, so the read that follows is a miss.
///
/// The rows differ along what is missing: the location the metadata would be minted under,
/// the format inside the `OpenTableFormatInput`, and the table definition itself.
#[tokio::test]
async fn a_malformed_iceberg_create_is_refused_and_stores_nothing() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;

    // (case, the table name the request addresses, the request body minus DatabaseName)
    let cases: [(&str, &str, Value); 4] = [
        (
            "no location anywhere",
            "no_location",
            json!({
                "TableInput": { "Name": "no_location" },
                "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } }
            }),
        ),
        (
            "an empty location",
            "empty_location",
            json!({
                "TableInput": {
                    "Name": "empty_location",
                    "StorageDescriptor": { "Location": "" }
                },
                "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } }
            }),
        ),
        (
            "an open table format naming no format",
            "no_format",
            json!({
                "TableInput": {
                    "Name": "no_format",
                    "StorageDescriptor": { "Location": "s3://lake/no_format/" }
                },
                "OpenTableFormatInput": {}
            }),
        ),
        (
            "no table definition at all",
            "no_definition",
            json!({
                "OpenTableFormatInput": { "IcebergInput": { "MetadataOperation": "CREATE" } }
            }),
        ),
    ];

    for (case, name, request) in cases {
        let mut body = request.clone();
        body["DatabaseName"] = json!(DATABASE);
        let (status, answer) = call(&router, "CreateTable", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: a create the service cannot mint metadata for must be refused, got \
             {status} with {answer}"
        );
        assert!(
            error_code(&answer).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {answer}",
            error_code(&answer)
        );

        let (status, read) = get_table(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused create stores nothing, so the read is a miss, got {status} \
             with {read}"
        );
    }
}
