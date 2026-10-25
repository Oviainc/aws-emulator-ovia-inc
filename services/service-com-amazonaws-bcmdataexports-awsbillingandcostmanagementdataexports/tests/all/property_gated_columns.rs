// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the table properties that select a table's COLUMN SET, over the
//! awsJson1_1 wire.
//!
//! The rule under test: a table property the dictionary publishes as INCLUDING a set of
//! columns selects exactly those columns — the table's schema carries them only while that
//! property is in force at the value that includes them, and nothing else about the schema
//! changes — and the column set `GetTable` reports for a configuration is equally the set a
//! submitted `QueryStatement` may name ("TableProperties are additional configurations you
//! can provide to change the data and schema of a table" — the modelled doc for
//! `GetTableRequest$TableProperties`). A property whose published description advertises
//! columns and selects none would make its own `TablePropertyDescription` a claim no
//! configuration can act on, and would refuse every query naming those columns at every
//! value of it.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the property: every column-selecting property of the catalog, including ones the issue
//!   that prompted this suite did not cite, and both a property that publishes a default
//!   and one that publishes none (in force only for a request that names it);
//! * the value and the request form: the including value, the excluding value, the property
//!   named alone, named beside every sibling property of its table, and named by no request
//!   at all;
//! * the reading: `GetTable`'s reported schema and the columns a `CreateExport` query
//!   statement may name;
//! * and — read off the published descriptions rather than a list written here — every
//!   property the catalog advertises as including columns, so a property added later is
//!   covered too.
//!
//! The router, the request signing and the export-definition helpers come from the
//! `export_lifecycle` and `query_column_selection` suites of this same test target.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    TableSpec, PROPERTY_ANY_VALUE, PROPERTY_FALSE, PROPERTY_TRUE, TABLE_CATALOG,
};

use crate::export_lifecycle::Client;
use crate::query_column_selection::{assert_column_refused, export_body, export_name};

/// The table the Cost and Usage Report 2.0 schema is published for.
const COST_AND_USAGE_REPORT: &str = "COST_AND_USAGE_REPORT";

/// The columns one table property includes: each one's name and the kind of data it
/// stores, as the dictionary publishes them.
type IncludedColumnList = &'static [(&'static str, &'static str)];

/// One column-selecting table property: the table that publishes it, the property, and the
/// columns it includes with the kind of data each stores.
///
/// SOURCE: the AWS Data Exports table dictionary, which `ListTables` mirrors and `GetTable`
/// reports. Each property's own published description says what it includes — "Whether IAM
/// principal columns should be included or not", "Whether to include resource IDs on line
/// items", "Whether to include fission columns" — and the columns below are the ones that
/// text names. The resource-ID and fission rows are entries the issue that prompted this
/// suite did not cite.
const INCLUDED_COLUMNS: &[(&str, &str, IncludedColumnList)] = &[
    (
        COST_AND_USAGE_REPORT,
        "INCLUDE_IAM_PRINCIPAL_DATA",
        &[("line_item_user_identifier", "String")],
    ),
    (
        COST_AND_USAGE_REPORT,
        "INCLUDE_RESOURCES",
        &[("line_item_resource_id", "String")],
    ),
    (
        COST_AND_USAGE_REPORT,
        "INCLUDE_SPLIT_COST_ALLOCATION_DATA",
        &[
            ("split_line_item_actual_usage", "Number"),
            ("split_line_item_net_split_cost", "Number"),
            ("split_line_item_net_unused_cost", "Number"),
            ("split_line_item_parent_resource_id", "String"),
            ("split_line_item_public_on_demand_split_cost", "Number"),
            ("split_line_item_public_on_demand_unused_cost", "Number"),
            ("split_line_item_reserved_usage", "Number"),
            ("split_line_item_split_cost", "Number"),
            ("split_line_item_split_usage", "Number"),
            ("split_line_item_split_usage_ratio", "Number"),
            ("split_line_item_unused_cost", "Number"),
        ],
    ),
];

/// The word a published `TablePropertyDescription` uses for what a schema is made of, which
/// is how a property says it selects columns rather than data.
const COLUMNS_IN_A_DESCRIPTION: &str = "column";

/// The `(Name, Type)` of each column `GetTable` reports for `table` under `properties`, in
/// the order the response lists them.
async fn schema(client: &Client, table: &str, properties: Option<Value>) -> Vec<(String, String)> {
    let request = match properties {
        None => json!({ "TableName": table }),
        Some(properties) => json!({ "TableName": table, "TableProperties": properties }),
    };
    let body = client.ok("GetTable", &request).await;
    body.get("Schema")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {table:?}: expected a Schema list, got {body}"))
        .iter()
        .map(|column| {
            let name = column
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {table:?}: a column without a Name: {column}"))
                .to_string();
            let data_type = column
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {table:?}: column {name} without a Type"))
                .to_string();
            (name, data_type)
        })
        .collect()
}

/// The table catalog's entry for `table`.
fn spec(table: &str) -> &'static TableSpec {
    TABLE_CATALOG
        .iter()
        .find(|candidate| candidate.name == table)
        .unwrap_or_else(|| panic!("the catalog publishes {table}"))
}

/// A `TableProperties` map naming EVERY enumerated property `table` publishes: the one
/// under test at `value`, and each sibling at the first value it publishes — so a row
/// asserts the property named beside its siblings, not only on its own. A free-form
/// property (one publishing no values or the wildcard) is left unnamed: its value is a
/// billing view ARN or a filter, which selects no column.
fn every_property(table: &str, property: &str, value: &str) -> Value {
    let mut configured = Map::new();
    for published in spec(table).properties {
        let chosen = if published.name == property {
            Some(value)
        } else if published.valid_values.is_empty()
            || published.valid_values.contains(&PROPERTY_ANY_VALUE)
        {
            None
        } else {
            published.valid_values.first().copied()
        };
        if let Some(chosen) = chosen {
            configured.insert(published.name.to_string(), json!(chosen));
        }
    }
    Value::Object(configured)
}

/// The names of `columns`.
fn names(columns: &[(&str, &str)]) -> BTreeSet<String> {
    columns.iter().map(|(name, _)| (*name).to_string()).collect()
}

/// RULE: a property the dictionary publishes as including columns selects exactly those
/// columns — present while it holds the including value, absent at the excluding value and
/// for a request that names it not at all, and no other column of the schema (or its order,
/// or its kind of data) changed either way.
#[tokio::test]
async fn a_property_selects_exactly_the_columns_it_includes() {
    let client = Client::new();

    for &(table, property, included) in INCLUDED_COLUMNS {
        let included_names = names(included);

        // Named alone, named beside every sibling property, and named by no request at all.
        let rows = [
            (
                format!("{table}/{property} named alone"),
                schema(&client, table, Some(json!({ property: PROPERTY_TRUE }))).await,
                schema(&client, table, Some(json!({ property: PROPERTY_FALSE }))).await,
            ),
            (
                format!("{table}/{property} named beside its siblings"),
                schema(
                    &client,
                    table,
                    Some(every_property(table, property, PROPERTY_TRUE)),
                )
                .await,
                schema(
                    &client,
                    table,
                    Some(every_property(table, property, PROPERTY_FALSE)),
                )
                .await,
            ),
            (
                format!("{table}/{property} against a request naming no property"),
                schema(&client, table, Some(json!({ property: PROPERTY_TRUE }))).await,
                schema(&client, table, None).await,
            ),
        ];

        for (case, with, without) in rows {
            for &(column, data_type) in included {
                assert_eq!(
                    with.iter()
                        .find(|(name, _)| name == column)
                        .map(|(_, reported)| reported.as_str()),
                    Some(data_type),
                    "case {case:?}: expected the included column {column} at Type {data_type:?}, got {:?}",
                    with.iter().find(|(name, _)| name == column)
                );
                assert!(
                    !without.iter().any(|(name, _)| name == column),
                    "case {case:?}: {column} is included only by {property}, got {without:?}"
                );
            }

            // EXACTLY those columns: drop the included ones and the two schemas are the
            // same columns, at the same kinds of data, in the same order.
            let remainder: Vec<(String, String)> = with
                .iter()
                .filter(|(name, _)| !included_names.contains(name))
                .cloned()
                .collect();
            assert_eq!(
                remainder, without,
                "case {case:?}: the property must change only the columns it includes, got {remainder:?} against {without:?}"
            );
        }

        // One property's columns are its own: another column-selecting property of the same
        // table does not bring them.
        for &(other_table, other_property, other_included) in INCLUDED_COLUMNS {
            if other_table != table || other_property == property {
                continue;
            }
            let case = format!("{table}/{other_property} against {property}");
            let elsewhere = schema(&client, table, Some(json!({ other_property: PROPERTY_TRUE })))
                .await;
            for &(column, _) in included {
                assert!(
                    !elsewhere.iter().any(|(name, _)| name == column),
                    "case {case:?}: {column} is included by {property}, not by {other_property}"
                );
            }
            assert!(
                !names(other_included).iter().any(|name| included_names.contains(name)),
                "case {case:?}: two properties must not claim the same column"
            );
        }
    }
}

/// RULE, read off the PUBLISHED descriptions rather than a list written here: every property
/// the catalog advertises as including columns includes at least one, so a property whose
/// description promises columns can never be a configuration that changes nothing.
#[tokio::test]
async fn every_property_advertised_as_including_columns_includes_some() {
    let client = Client::new();
    let mut checked = 0usize;

    for table in TABLE_CATALOG {
        for property in table.properties {
            let advertises_columns = property
                .description
                .to_ascii_lowercase()
                .contains(COLUMNS_IN_A_DESCRIPTION);
            let switchable = property.valid_values.contains(&PROPERTY_TRUE)
                && property.valid_values.contains(&PROPERTY_FALSE);
            if !advertises_columns || !switchable {
                continue;
            }
            checked += 1;
            let case = format!("{}/{}", table.name, property.name);
            let with = schema(
                &client,
                table.name,
                Some(json!({ property.name: PROPERTY_TRUE })),
            )
            .await;
            let without = schema(
                &client,
                table.name,
                Some(json!({ property.name: PROPERTY_FALSE })),
            )
            .await;
            let added: Vec<&str> = with
                .iter()
                .map(|(name, _)| name.as_str())
                .filter(|name| !without.iter().any(|(other, _)| other == name))
                .collect();
            assert!(
                !added.is_empty(),
                "case {case:?}: the property publishes {:?}, so it must include at least one column, got the same {} columns either way",
                property.description,
                with.len()
            );
        }
    }

    assert!(
        checked > 0,
        "case \"advertised columns\": the catalog publishes at least one property that advertises columns"
    );
}

/// RULE, the query half: the column set `GetTable` reports for a configuration is the set a
/// query statement may name, so a create selecting an included column is served exactly
/// when its own `TableConfigurations` includes it, and refused — naming that column — when
/// it does not.
#[tokio::test]
async fn a_query_may_name_an_included_column_exactly_when_the_configuration_includes_it() {
    let client = Client::new();

    for (index, &(table, property, included)) in INCLUDED_COLUMNS.iter().enumerate() {
        let (column, _) = included[0];
        let statement = format!("SELECT {column} FROM {table}");

        let case = format!("{table}/{property}={PROPERTY_TRUE} selecting {column}");
        let created = client
            .ok(
                "CreateExport",
                &json!({
                    "Export": export_body(
                        &export_name(&format!("included_{index}")),
                        &statement,
                        Some(json!({ table: { property: PROPERTY_TRUE } })),
                    ),
                }),
            )
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateExport must return an ExportArn"))
            .to_string();
        let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            fetched["Export"]["DataQuery"]["QueryStatement"].as_str(),
            Some(statement.as_str()),
            "case {case:?}: GetExport must report the accepted statement, got {fetched}"
        );
        assert_eq!(
            fetched["Export"]["DataQuery"]["TableConfigurations"][table][property].as_str(),
            Some(PROPERTY_TRUE),
            "case {case:?}: the stored definition must carry the configuration that includes the column, got {fetched}"
        );

        // At the excluding value, and with no configuration at all (the property is then at
        // its published default, or — publishing none — in force at no value), the same
        // statement names a column the export could never deliver.
        for (row, (label, configurations)) in [
            (
                format!("{property}={PROPERTY_FALSE}"),
                Some(json!({ table: { property: PROPERTY_FALSE } })),
            ),
            ("no TableConfigurations".to_string(), None),
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!("{table} with {label} selecting {column}");
            let (status, body) = client
                .call(
                    "CreateExport",
                    &json!({
                        "Export": export_body(
                            &export_name(&format!("excluded_{index}_{row}")),
                            &statement,
                            configurations,
                        ),
                    }),
                )
                .await;
            assert_column_refused(&case, status, &body, column);
        }
    }
}
