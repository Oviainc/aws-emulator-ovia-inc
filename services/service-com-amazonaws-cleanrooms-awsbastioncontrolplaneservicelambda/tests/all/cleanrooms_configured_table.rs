// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms configured-table slice
//! (`CreateConfiguredTable` + `GetConfiguredTable`).
//!
//! The rules under test:
//!
//! * a create persists the resource and mints every member the SERVICE decides — a fresh
//!   `UUID` id, the `ConfiguredTableArn` for the request's region and account, the
//!   creation instant for both timestamps, an empty `analysisRuleTypes` (a table is
//!   created with no analysis rules) and a `tableType` that is the `TableReferenceType`
//!   of the `TableReference` union variant the request carried — while the members the
//!   CALLER decides are reported back unchanged, and a later read reports exactly the
//!   persisted resource;
//! * `selectedAnalysisMethods`, "when configured, you must specify at least two analysis
//!   methods", is refused as a `ValidationException` when it configures fewer;
//! * an identifier the store does not hold is a `ResourceNotFoundException`.
//!
//! Each table varies what the rules generalise over (which union variant the reference
//! is, every `AnalysisMethod`, description present or absent, the number of allowed
//! columns, the number and the order of selected analysis methods, and how many tables
//! exist), so no row is privileged. Every request goes over the wire through the same
//! `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, is_model_uuid, Emulator, REGION};

/// Every `TableReferenceType` the model declares (shape `TableReferenceType`), which is
/// also one per member of the `TableReference` union.
pub(crate) const ALL_TABLE_REFERENCE_TYPES: [&str; 4] =
    ["GLUE", "SNOWFLAKE", "ATHENA", "DATABRICKS"];

/// Every `AnalysisMethod` the model declares (shape `AnalysisMethod`).
pub(crate) const ALL_ANALYSIS_METHODS: [&str; 3] = ["DIRECT_QUERY", "DIRECT_JOB", "MULTIPLE"];

/// Every `SelectedAnalysisMethod` the model declares (shape `SelectedAnalysisMethod`).
const ALL_SELECTED_ANALYSIS_METHODS: [&str; 2] = ["DIRECT_QUERY", "DIRECT_JOB"];

/// `CreateConfiguredTableInput$selectedAnalysisMethods`: "When configured, you must
/// specify at least two analysis methods."
const MIN_SELECTED_ANALYSIS_METHODS: usize = 2;

/// The published maximum length of `ConfiguredTableArn` (model shape
/// `ConfiguredTableArn`, len ..100).
const MAX_CONFIGURED_TABLE_ARN_LEN: usize = 100;

/// A `TableReference` for one member of the union, built to the model's patterns for
/// that member's shape. `table_type` is the `TableReferenceType` the service must report
/// for it.
fn table_reference(table_type: &str) -> Value {
    match table_type {
        // `GlueTableReference`: tableName/databaseName, region from `CommercialRegion`.
        "GLUE" => json!({
            "glue": {
                "tableName": "sales_table",
                "databaseName": "analytics_db",
                "region": "us-east-2",
            }
        }),
        // `SnowflakeTableReference`: secretArn matches
        // `^arn:aws:secretsmanager:[a-z]{2}-[a-z]+-[0-9]:\d{12}:secret:.*$`.
        "SNOWFLAKE" => json!({
            "snowflake": {
                "secretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:snowflake-creds",
                "accountIdentifier": "xy12345",
                "databaseName": "SALES_DB",
                "schemaName": "PUBLIC",
                "tableName": "ORDERS",
                "tableSchema": {
                    "v1": [
                        { "columnName": "order_id", "columnType": "BIGINT" },
                        { "columnName": "amount", "columnType": "DECIMAL" },
                    ]
                },
            }
        }),
        // `AthenaTableReference`: workGroup matches `^([a-zA-Z0-9._-])*$`,
        // outputLocation `^s3://[a-z0-9.-]{3,63}(.*)$`.
        "ATHENA" => json!({
            "athena": {
                "workGroup": "primary",
                "databaseName": "athena_db",
                "tableName": "events",
                "outputLocation": "s3://clean-rooms-results/prefix/",
                "catalogName": "AwsDataCatalog",
            }
        }),
        // `DatabricksTableReference`: workspaceUrl matches
        // `^https://[a-zA-Z0-9.-]+\.cloud\.databricks\.com/?$`, and the Delta Sharing
        // endpoint carries a metastore and a recipient UUID.
        "DATABRICKS" => json!({
            "databricks": {
                "schemaName": "sales_schema",
                "tableName": "orders",
                "workspaceUrl": "https://dbc-1234abcd-5678.cloud.databricks.com",
                "connectionType": {
                    "deltaSharing": {
                        "deltaShareEndpoint": "https://dbc-1234abcd-5678.cloud.databricks.com/api/2.0/delta-sharing/metastores/11111111-2222-3333-4444-555555555555/recipients/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                        "shareName": "sales_share",
                    }
                },
            }
        }),
        other => panic!("no TableReference member for table type {other:?}"),
    }
}

/// `ColumnName` values (pattern `^[a-z0-9_](([a-z0-9_ ]+-)*([a-z0-9_ ]+))?$`), used to
/// build `allowedColumns` lists of varying length.
pub(crate) const COLUMN_NAMES: [&str; 3] = ["order_id", "amount", "customer_region"];

/// A `CreateConfiguredTable` body.
pub(crate) fn create_body(
    name: &str,
    table_type: &str,
    analysis_method: &str,
    description: Option<&str>,
    column_count: usize,
) -> Value {
    let mut body = json!({
        "name": name,
        "tableReference": table_reference(table_type),
        "allowedColumns": COLUMN_NAMES[..column_count].to_vec(),
        "analysisMethod": analysis_method,
    });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    body
}

impl Emulator {
    pub(crate) async fn create_configured_table(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/configuredTables", Some(body)).await
    }

    pub(crate) async fn get_configured_table(&self, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/configuredTables/{id}"), None)
            .await
    }
}

/// One row of the create/read round-trip table.
struct Case {
    label: String,
    table_type: &'static str,
    analysis_method: &'static str,
    description: Option<&'static str>,
    column_count: usize,
}

/// The round-trip table: every `TableReference` member crossed with every
/// `AnalysisMethod`, with the description present on half the rows and the allowed-column
/// count cycling through 1..=3.
fn round_trip_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (i, table_type) in ALL_TABLE_REFERENCE_TYPES.iter().enumerate() {
        for (j, analysis_method) in ALL_ANALYSIS_METHODS.iter().enumerate() {
            let description = if (i + j) % 2 == 0 {
                Some("a configured table for the test table")
            } else {
                None
            };
            cases.push(Case {
                label: format!(
                    "tableReference={table_type}, analysisMethod={analysis_method}, \
                     description={}, columns={}",
                    description.is_some(),
                    (i + j) % COLUMN_NAMES.len() + 1
                ),
                table_type,
                analysis_method,
                description,
                column_count: (i + j) % COLUMN_NAMES.len() + 1,
            });
        }
    }
    cases
}

#[tokio::test]
async fn create_configured_table_persists_the_resource_a_later_read_reports() {
    let emulator = Emulator::new();

    for (row, case) in round_trip_cases().into_iter().enumerate() {
        let label = &case.label;
        let name = format!("table-{row}");
        let body = create_body(
            &name,
            case.table_type,
            case.analysis_method,
            case.description,
            case.column_count,
        );
        let expected_reference = table_reference(case.table_type);
        let expected_columns: Vec<&str> = COLUMN_NAMES[..case.column_count].to_vec();

        let (status, created) = emulator.create_configured_table(body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTable 200, got {status} with {created}"
        );
        let table = &created["configuredTable"];

        // What the caller decided is reported back unchanged.
        assert_eq!(
            table["name"].as_str(),
            Some(name.as_str()),
            "case {label:?}: expected name {name:?}, got {table}"
        );
        assert_eq!(
            table["description"].as_str(),
            case.description,
            "case {label:?}: expected description {:?}, got {table}",
            case.description
        );
        assert_eq!(
            table["analysisMethod"].as_str(),
            Some(case.analysis_method),
            "case {label:?}: expected analysisMethod {:?}, got {table}",
            case.analysis_method
        );
        assert_eq!(
            table["allowedColumns"], json!(expected_columns),
            "case {label:?}: expected allowedColumns {expected_columns:?}, got {}",
            table["allowedColumns"]
        );
        assert_eq!(
            table["tableReference"], expected_reference,
            "case {label:?}: expected tableReference {expected_reference}, got {}",
            table["tableReference"]
        );
        assert!(
            table.get("selectedAnalysisMethods").is_none(),
            "case {label:?}: a request that configured no analysis methods must report none, got {}",
            table["selectedAnalysisMethods"]
        );

        // What the SERVICE decides is computed, and the request never carried it.
        let id = table["id"].as_str().unwrap_or_default();
        assert!(
            is_model_uuid(id),
            "case {label:?}: expected a model UUID id, got {id:?}"
        );
        let expected_arn =
            format!("arn:aws:cleanrooms:{REGION}:{}:configuredtable/{id}", account_id());
        assert_eq!(
            table["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {label:?}: expected arn {expected_arn:?}, got {}",
            table["arn"]
        );
        assert!(
            expected_arn.len() <= MAX_CONFIGURED_TABLE_ARN_LEN,
            "case {label:?}: ConfiguredTableArn is bounded at {MAX_CONFIGURED_TABLE_ARN_LEN}, got {} for {expected_arn:?}",
            expected_arn.len()
        );
        assert_eq!(
            table["tableType"].as_str(),
            Some(case.table_type),
            "case {label:?}: tableType must be the TableReference variant's type {:?}, got {}",
            case.table_type,
            table["tableType"]
        );
        assert_eq!(
            table["analysisRuleTypes"], json!([]),
            "case {label:?}: a table created with no analysis rules reports none, got {}",
            table["analysisRuleTypes"]
        );
        assert_eq!(
            table["createTime"], table["updateTime"],
            "case {label:?}: a table never updated reports createTime == updateTime, got {} and {}",
            table["createTime"], table["updateTime"]
        );

        // The read reports the persisted resource, and reading does not change it.
        let (status, read) = emulator.get_configured_table(id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTable 200 for {id}, got {status} with {read}"
        );
        assert_eq!(
            read["configuredTable"], *table,
            "case {label:?}: the read must report the created table {table}, got {}",
            read["configuredTable"]
        );
        let (status, reread) = emulator.get_configured_table(id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a second GetConfiguredTable 200 for {id}, got {status} with {reread}"
        );
        assert_eq!(
            reread, read,
            "case {label:?}: a read must not change what a later read sees; first {read}, then {reread}"
        );
    }
}

#[tokio::test]
async fn every_create_mints_its_own_configured_table() {
    let emulator = Emulator::new();

    // The same name twice, then a different one: a configured table is identified by the
    // id the service mints, so none of these collides and each is readable on its own.
    let rows: [(&str, &str, usize); 3] = [
        ("same-name", "GLUE", 1),
        ("same-name", "ATHENA", 2),
        ("other-name", "SNOWFLAKE", 3),
    ];
    let mut seen: Vec<(String, String)> = Vec::new();

    for (name, table_type, column_count) in rows {
        let label = format!("name={name}, tableReference={table_type}, columns={column_count}");
        let (status, created) = emulator
            .create_configured_table(create_body(
                name,
                table_type,
                ALL_ANALYSIS_METHODS[0],
                None,
                column_count,
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTable 200, got {status} with {created}"
        );
        let table = &created["configuredTable"];
        let id = table["id"].as_str().unwrap_or_default().to_string();
        let arn = table["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            !seen.iter().any(|(seen_id, _)| *seen_id == id),
            "case {label:?}: expected a fresh id, got {id:?} which is already in {seen:?}"
        );
        assert!(
            !seen.iter().any(|(_, seen_arn)| *seen_arn == arn),
            "case {label:?}: expected a fresh arn, got {arn:?} which is already in {seen:?}"
        );
        seen.push((id, arn));
    }

    // Every table created above is still readable, with its own members.
    for (row, (id, arn)) in seen.iter().enumerate() {
        let (name, table_type, column_count) = rows[row];
        let label = format!("row {row}: id={id}");
        let (status, read) = emulator.get_configured_table(id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTable 200, got {status} with {read}"
        );
        let table = &read["configuredTable"];
        assert_eq!(
            table["name"].as_str(),
            Some(name),
            "case {label:?}: expected name {name:?}, got {table}"
        );
        assert_eq!(
            table["arn"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected arn {arn:?}, got {table}"
        );
        assert_eq!(
            table["tableType"].as_str(),
            Some(table_type),
            "case {label:?}: expected tableType {table_type:?}, got {table}"
        );
        assert_eq!(
            table["allowedColumns"].as_array().map(Vec::len),
            Some(column_count),
            "case {label:?}: expected {column_count} allowed columns, got {}",
            table["allowedColumns"]
        );
    }
}

#[tokio::test]
async fn selected_analysis_methods_must_configure_at_least_two_methods() {
    let emulator = Emulator::new();

    // Rows span the member's domain: absent, every list the two-value
    // `SelectedAnalysisMethod` enum admits (both orders), the one-method lists, a list
    // that repeats one method (so it still configures ONE), and the empty list.
    let both = vec![
        ALL_SELECTED_ANALYSIS_METHODS[0].to_string(),
        ALL_SELECTED_ANALYSIS_METHODS[1].to_string(),
    ];
    let both_reversed = vec![
        ALL_SELECTED_ANALYSIS_METHODS[1].to_string(),
        ALL_SELECTED_ANALYSIS_METHODS[0].to_string(),
    ];
    let rows: Vec<(Option<Vec<String>>, bool)> = vec![
        (None, true),
        (Some(both.clone()), true),
        (Some(both_reversed.clone()), true),
        (Some(vec![ALL_SELECTED_ANALYSIS_METHODS[0].to_string()]), false),
        (Some(vec![ALL_SELECTED_ANALYSIS_METHODS[1].to_string()]), false),
        (
            Some(vec![
                ALL_SELECTED_ANALYSIS_METHODS[0].to_string(),
                ALL_SELECTED_ANALYSIS_METHODS[0].to_string(),
            ]),
            false,
        ),
        (Some(Vec::new()), false),
    ];

    for (row, (selected, accepted)) in rows.into_iter().enumerate() {
        let label = format!("selectedAnalysisMethods={selected:?}");
        // `MULTIPLE` is the analysis method that allows both, so it is the one these
        // rows vary the selection under.
        let mut body = create_body(
            &format!("selected-{row}"),
            ALL_TABLE_REFERENCE_TYPES[0],
            "MULTIPLE",
            None,
            1,
        );
        if let Some(selected) = selected.clone() {
            body["selectedAnalysisMethods"] = json!(selected);
        }
        let (status, response) = emulator.create_configured_table(body).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected CreateConfiguredTable 200, got {status} with {response}"
            );
            let table = &response["configuredTable"];
            match &selected {
                Some(selected) => assert_eq!(
                    table["selectedAnalysisMethods"], json!(selected),
                    "case {label:?}: expected selectedAnalysisMethods {selected:?}, got {}",
                    table["selectedAnalysisMethods"]
                ),
                None => assert!(
                    table.get("selectedAnalysisMethods").is_none(),
                    "case {label:?}: expected no selectedAnalysisMethods, got {}",
                    table["selectedAnalysisMethods"]
                ),
            }
            // The accepted selection survives the round trip.
            let id = table["id"].as_str().unwrap_or_default();
            let (status, read) = emulator.get_configured_table(id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected GetConfiguredTable 200 for {id}, got {status} with {read}"
            );
            assert_eq!(
                read["configuredTable"]["selectedAnalysisMethods"],
                table["selectedAnalysisMethods"],
                "case {label:?}: the read must report the stored selection {}, got {}",
                table["selectedAnalysisMethods"],
                read["configuredTable"]["selectedAnalysisMethods"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: fewer than {MIN_SELECTED_ANALYSIS_METHODS} analysis methods must be refused with 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {response}"
            );
            assert!(
                response.get("configuredTable").is_none(),
                "case {label:?}: a refused create must report no configured table, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn get_configured_table_of_an_unheld_identifier_is_not_found() {
    let emulator = Emulator::new();

    // One table exists, so the 404 below is about the identifier and not about an empty
    // store.
    let (status, created) = emulator
        .create_configured_table(create_body(
            "the-one-table",
            ALL_TABLE_REFERENCE_TYPES[0],
            ALL_ANALYSIS_METHODS[0],
            None,
            1,
        ))
        .await;
    assert_eq!(
        status, 200,
        "expected CreateConfiguredTable 200, got {status} with {created}"
    );
    let held = created["configuredTable"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Identifiers in the form the model's `ConfiguredTableIdentifier` requires that no
    // create minted: the two extremes of the space and one in between.
    let unheld = [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "deadbeef-0000-4000-8000-000000000001",
    ];
    for id in unheld {
        assert_ne!(id, held, "the unheld identifiers must not include the created one");
        let (status, response) = emulator.get_configured_table(id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected GetConfiguredTable 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["resourceId"].as_str(),
            Some(id),
            "case {id:?}: expected resourceId {id:?}, got {response}"
        );
        assert_eq!(
            response["resourceType"].as_str(),
            Some("CONFIGURED_TABLE"),
            "case {id:?}: expected resourceType CONFIGURED_TABLE, got {response}"
        );
    }

    // The failed reads left the held table exactly where it was.
    let (status, read) = emulator.get_configured_table(&held).await;
    assert_eq!(
        status, 200,
        "expected GetConfiguredTable 200 for {held}, got {status} with {read}"
    );
    assert_eq!(
        read["configuredTable"], created["configuredTable"],
        "the created table must still read back as {}, got {}",
        created["configuredTable"], read["configuredTable"]
    );
}
