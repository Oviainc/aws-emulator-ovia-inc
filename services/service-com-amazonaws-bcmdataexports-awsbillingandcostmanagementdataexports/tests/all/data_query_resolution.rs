// SPDX-License-Identifier: CC0-1.0

//! ONE rule, the two operations that accept an export definition: `CreateExport` and
//! `UpdateExport` resolve a submitted `DataQuery` through the same single pass, so for one
//! definition they answer the same verdict and store the same table configuration.
//!
//! The rule: a submitted definition's tables are resolved against the one published
//! catalog — "To view the available tables and columns, see the Data Exports table
//! dictionary" (the modelled doc for `CreateExport`'s `Export`) — and each resolved
//! table's properties are the ones IN FORCE, at the requested value where the request
//! names one and at the published default otherwise ("Each table property has a default
//! value that it assumes if not specified" — the modelled doc for
//! `GetTableRequest$TableProperties`); those same properties decide both the verdict on
//! the definition (its tables, its property values and the columns its statement may name)
//! and what the definition is STORED with, for whichever operation submitted it.
//!
//! Every row below therefore goes through BOTH operations — a create of the row's
//! definition, and an update of an export created from a baseline definition — and the two
//! answers are compared with each other as well as with the expectation computed from the
//! service's own published catalog. A row that is served is checked by reading the stored
//! definition back through `GetExport`; a row that is refused must be refused identically
//! by both and must leave the stored definition untouched.
//!
//! Each row restates the `TableConfigurations` an update owes (`UpdateExport` "[updates]
//! an existing data export by overwriting all export parameters" — smithy_query operation
//! `UpdateExport`, the asymmetry `export_definition_validation` pins), so what remains
//! between the two operations is the resolution pass itself.
//!
//! The rows differ along what the rule generalises over: how many tables a statement
//! names, whether the request names a property at all, a published value that is not the
//! default, a property the catalog publishes no default for, a table the request
//! configures without querying it, and each way a definition misses the catalog — an
//! unpublished table (queried, or configured only), an unpublished property, a value
//! outside a property's published values, and a column that no queried table publishes
//! under the properties in force.

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    TableSpec, TABLE_CATALOG,
};

use crate::export_lifecycle::{cases, effective_table_configurations, error_code, Client};

/// The statement the baseline export an update replaces is created with, over a published
/// table with the properties it publishes left to their defaults.
const BASELINE_STATEMENT: &str = "SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT";

/// What a row expects of the resolution pass.
#[derive(Debug)]
enum Expected {
    /// The definition is served, and stored with the configuration in force.
    Served,
    /// The definition is refused with this message — the modelled `ValidationException`
    /// (HTTP 400) — and nothing is stored.
    Refused(String),
}

/// One submitted `DataQuery`, and what both operations owe it.
#[derive(Debug)]
struct Row {
    label: String,
    statement: String,
    /// The catalog tables `statement` reads, which are the tables whose properties a
    /// served definition is stored with (beside any the request configures on its own).
    queried_tables: Vec<&'static str>,
    configurations: Option<Value>,
    expected: Expected,
}

/// The `Export` a row submits: a definition that satisfies every modelled constraint,
/// with the name and the data query substituted.
fn export_body(name: &str, statement: &str, configurations: Option<Value>) -> Value {
    let mut export = cases().remove(0).create_request()["Export"].clone();
    let object = export
        .as_object_mut()
        .expect("the Export structure is a JSON object");
    object.insert("Name".to_string(), json!(name));
    let mut data_query = Map::new();
    data_query.insert("QueryStatement".to_string(), json!(statement));
    if let Some(configurations) = configurations {
        data_query.insert("TableConfigurations".to_string(), configurations);
    }
    object.insert("DataQuery".to_string(), Value::Object(data_query));
    export
}

/// The catalog entry called `name`.
fn spec_of(name: &str) -> &'static TableSpec {
    TABLE_CATALOG
        .iter()
        .find(|candidate| candidate.name == name)
        .unwrap_or_else(|| panic!("{name} is not a published catalog table"))
}

/// The first property-gated column the catalog publishes: the table that publishes it, the
/// column, the property that gates it, the value that column needs, and another value that
/// property publishes.
///
/// Read off the catalog (`ColumnSpec::requires` and the gating `TablePropertySpec`'s
/// `valid_values`) rather than named here, so the row covers whichever gated column the
/// catalog carries and not one this suite happens to spell.
fn property_gated_column() -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    for table in TABLE_CATALOG {
        for column in table.columns() {
            let Some((property, required)) = column.requires else {
                continue;
            };
            let Some(gate) = table.property(property) else {
                continue;
            };
            let Some(other) = gate
                .valid_values
                .iter()
                .copied()
                .find(|value| *value != required)
            else {
                continue;
            };
            return (table.name, column.name, property, required, other);
        }
    }
    panic!("the catalog publishes at least one property-gated column");
}

/// The refusal a property no table publishes is owed, with the supported list read off the
/// catalog entry itself.
fn unsupported_property_message(table: &str, property: &str) -> String {
    let supported = spec_of(table)
        .properties
        .iter()
        .map(|published| published.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Table property {property} is not supported for table {table}. \
         Supported table properties: [{supported}]."
    )
}

/// The refusal a value outside a property's published values is owed, with the valid list
/// read off the catalog entry itself.
fn unsupported_value_message(table: &str, property: &str, value: &str) -> String {
    let valid = spec_of(table)
        .property(property)
        .unwrap_or_else(|| panic!("{table} publishes the property {property}"))
        .valid_values
        .join(", ");
    format!(
        "Value {value} is not supported for table property {property}. Valid values: [{valid}]."
    )
}

/// The rows: one submitted definition each, with the verdict both operations owe it.
fn rows() -> Vec<Row> {
    let (gated_table, gated_column, gate, in_force, switched_off) = property_gated_column();

    vec![
        // The request names no property at all: every published property of the queried
        // table is in force at its default, and that is what is stored.
        Row {
            label: "no property named".to_string(),
            statement: "SELECT * FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            configurations: Some(json!({ "COST_AND_USAGE_REPORT": {} })),
            expected: Expected::Served,
        },
        // A property at a published value that is not its default (TIME_GRANULARITY
        // publishes DAILY, MONTHLY and HOURLY, and defaults to HOURLY — TABLE_CATALOG).
        Row {
            label: "a property away from its default".to_string(),
            statement: "SELECT * FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            configurations: Some(json!({
                "COST_AND_USAGE_REPORT": { "TIME_GRANULARITY": "DAILY" },
            })),
            expected: Expected::Served,
        },
        // A property the catalog publishes NO default for: in force only because this
        // request names it (INCLUDE_IAM_PRINCIPAL_DATA — TABLE_CATALOG).
        Row {
            label: "a property with no published default".to_string(),
            statement: "SELECT * FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            configurations: Some(json!({
                "COST_AND_USAGE_REPORT": { "INCLUDE_IAM_PRINCIPAL_DATA": "TRUE" },
            })),
            expected: Expected::Served,
        },
        // Two tables in one statement: each is resolved, and each contributes its own
        // effective configuration.
        Row {
            label: "two queried tables".to_string(),
            statement: "SELECT 1 FROM COST_AND_USAGE_REPORT a JOIN CARBON_EMISSIONS b ON 1 = 1"
                .to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT", "CARBON_EMISSIONS"],
            configurations: Some(json!({
                "COST_AND_USAGE_REPORT": {},
                "CARBON_EMISSIONS": {},
            })),
            expected: Expected::Served,
        },
        // A table the request configures without querying it: still the caller's, so it is
        // resolved and reported like a queried one.
        Row {
            label: "a table configured but not queried".to_string(),
            statement: "SELECT * FROM FOCUS_1_2_AWS".to_string(),
            queried_tables: vec!["FOCUS_1_2_AWS"],
            configurations: Some(json!({
                "FOCUS_1_2_AWS": { "TIME_GRANULARITY": "MONTHLY" },
                "CARBON_EMISSIONS": {},
            })),
            expected: Expected::Served,
        },
        // The column a table publishes only at one value of a gating property: in force
        // for a request that sets that value, so the statement may name it.
        Row {
            label: format!("a gated column in force ({gated_column})"),
            statement: format!("SELECT {gated_column} FROM {gated_table}"),
            queried_tables: vec![gated_table],
            configurations: Some(json!({ gated_table: { gate: in_force } })),
            expected: Expected::Served,
        },
        // The same column with the gate at another published value: the request's own
        // configuration switches it off, so it is as unservable as a column no table
        // publishes.
        Row {
            label: format!("a gated column switched off ({gated_column})"),
            statement: format!("SELECT {gated_column} FROM {gated_table}"),
            queried_tables: vec![gated_table],
            configurations: Some(json!({ gated_table: { gate: switched_off } })),
            expected: Expected::Refused(format!("Column name {gated_column} is invalid")),
        },
        // A queried table the catalog does not publish.
        Row {
            label: "a queried table outside the catalog".to_string(),
            statement: "SELECT some_column FROM NOT_A_REAL_TABLE_XYZ".to_string(),
            queried_tables: vec![],
            configurations: None,
            expected: Expected::Refused("Table name NOT_A_REAL_TABLE_XYZ is invalid".to_string()),
        },
        // A table the request configures on its own and the catalog does not publish: the
        // statement is fine, the configuration is not.
        Row {
            label: "a configured-only table outside the catalog".to_string(),
            statement: "SELECT * FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            configurations: Some(json!({
                "COST_AND_USAGE_REPORT": {},
                "NOT_A_PUBLISHED_TABLE": {},
            })),
            expected: Expected::Refused("Table name NOT_A_PUBLISHED_TABLE is invalid".to_string()),
        },
        // A property the queried table does not publish.
        Row {
            label: "a property the table does not publish".to_string(),
            statement: "SELECT * FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            configurations: Some(json!({
                "COST_AND_USAGE_REPORT": { "NOT_A_TABLE_PROPERTY": "TRUE" },
            })),
            expected: Expected::Refused(unsupported_property_message(
                "COST_AND_USAGE_REPORT",
                "NOT_A_TABLE_PROPERTY",
            )),
        },
        // A value outside an enumerated property's published values.
        Row {
            label: "a value outside the property's published values".to_string(),
            statement: "SELECT * FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            configurations: Some(json!({
                "COST_AND_USAGE_REPORT": { "TIME_GRANULARITY": "WEEKLY" },
            })),
            expected: Expected::Refused(unsupported_value_message(
                "COST_AND_USAGE_REPORT",
                "TIME_GRANULARITY",
                "WEEKLY",
            )),
        },
        // A column no queried table publishes at any value of any property.
        Row {
            label: "a column outside the catalog".to_string(),
            statement: "SELECT not_a_real_column FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            configurations: Some(json!({ "COST_AND_USAGE_REPORT": {} })),
            expected: Expected::Refused("Column name not_a_real_column is invalid".to_string()),
        },
    ]
}

/// The `TableConfigurations` of the definition `fetched` reports, if it carries any.
fn stored_configurations(fetched: &Value) -> Option<Value> {
    fetched["Export"]["DataQuery"]
        .get("TableConfigurations")
        .cloned()
}

#[tokio::test]
async fn one_resolution_pass_answers_create_and_update_alike() {
    let client = Client::new();

    for (index, row) in rows().into_iter().enumerate() {
        let label = &row.label;

        // THE CREATE PATH: the row's definition submitted to `CreateExport`.
        let create_name = format!("created_{index}");
        let (create_status, create_body) = client
            .call(
                "CreateExport",
                &json!({
                    "Export": export_body(
                        &create_name,
                        &row.statement,
                        row.configurations.clone(),
                    ),
                }),
            )
            .await;

        // THE UPDATE PATH: an export created from the baseline definition, then the row's
        // definition submitted to `UpdateExport` as the whole replacement.
        let update_name = format!("updated_{index}");
        let baseline = client
            .ok(
                "CreateExport",
                &json!({
                    "Export": export_body(
                        &update_name,
                        BASELINE_STATEMENT,
                        Some(json!({ "COST_AND_USAGE_REPORT": {} })),
                    ),
                }),
            )
            .await;
        let arn = baseline["ExportArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: the baseline create must return an ExportArn, got {baseline}")
            })
            .to_string();
        let before = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        let (update_status, update_body) = client
            .call(
                "UpdateExport",
                &json!({
                    "ExportArn": arn,
                    "Export": export_body(
                        &update_name,
                        &row.statement,
                        row.configurations.clone(),
                    ),
                }),
            )
            .await;

        // One definition, one verdict: whichever operation submitted it.
        assert_eq!(
            create_status, update_status,
            "case {label:?}: CreateExport answered {create_status} with {create_body} and \
             UpdateExport answered {update_status} with {update_body}"
        );

        match &row.expected {
            Expected::Refused(message) => {
                for (operation, status, body) in [
                    ("CreateExport", create_status, &create_body),
                    ("UpdateExport", update_status, &update_body),
                ] {
                    assert_eq!(
                        (status, error_code(body).as_str()),
                        (400, "ValidationException"),
                        "case {label:?}: {operation} must answer HTTP 400 ValidationException, \
                         got {status} with {body}"
                    );
                    assert_eq!(
                        body["Message"].as_str(),
                        Some(message.as_str()),
                        "case {label:?}: {operation} must state the refusal, got {body}"
                    );
                }
                assert!(
                    create_body.get("ExportArn").is_none(),
                    "case {label:?}: a refused create must mint no export, got {create_body}"
                );
                let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
                assert_eq!(
                    after, before,
                    "case {label:?}: a refused update must leave the stored definition untouched"
                );
            }
            Expected::Served => {
                assert_eq!(
                    create_status, 200,
                    "case {label:?}: CreateExport must serve this definition, got {create_body}"
                );
                let created_arn = create_body["ExportArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {label:?}: CreateExport must return an ExportArn, \
                             got {create_body}"
                        )
                    })
                    .to_string();
                let created = client
                    .ok("GetExport", &json!({ "ExportArn": created_arn }))
                    .await;
                let updated = client.ok("GetExport", &json!({ "ExportArn": arn })).await;

                // The configuration IN FORCE, computed from the service's own published
                // catalog: every property each resolved table publishes, at the requested
                // value where the request names it and at the published default otherwise.
                let in_force = effective_table_configurations(
                    &row.queried_tables,
                    row.configurations.as_ref(),
                );
                assert_eq!(
                    stored_configurations(&created),
                    in_force,
                    "case {label:?}: the created definition must be stored with the \
                     configuration in force, got {created}"
                );
                assert_eq!(
                    stored_configurations(&updated),
                    stored_configurations(&created),
                    "case {label:?}: the updated definition must be stored exactly as the \
                     created one is, got {updated}"
                );
                for (operation, fetched) in [("CreateExport", &created), ("UpdateExport", &updated)]
                {
                    assert_eq!(
                        fetched["Export"]["DataQuery"]["QueryStatement"].as_str(),
                        Some(row.statement.as_str()),
                        "case {label:?}: the definition {operation} stored must report the \
                         submitted statement, got {fetched}"
                    );
                }
            }
        }
    }
}
