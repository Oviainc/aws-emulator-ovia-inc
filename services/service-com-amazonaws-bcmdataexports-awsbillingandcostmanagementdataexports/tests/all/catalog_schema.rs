// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHAT the table catalog publishes per table — the column set, the
//! kind-of-data token of each column, and the table properties — over the awsJson1_1
//! wire.
//!
//! The rules under test, each asserted over the whole catalog rather than over the one
//! table that showed the defect:
//!
//! 1. A CURATED table (the cost and usage dashboard) publishes exactly the columns the
//!    dictionary lists for it: its aggregate dimensions and measures, plus the few
//!    columns it carries over from the underlying table under that table's own names, and
//!    none of the underlying table's other line-item columns. The dictionary's column set
//!    for a table name is that table's, so a client writing a query for it can select
//!    every column AWS publishes and cannot select one AWS would reject.
//! 2. A column's `Type` is the token ITS schema's dictionary publishes for it: the cost
//!    and usage dictionary measures costs with `Number` and publishes the Savings Plan
//!    commitment and rate as `String`, while the recommendations and carbon emissions
//!    dictionaries publish every numeric column — money, percentages and day counts
//!    alike — as `Double`. A token is never generalised from one schema onto another.
//! 3. A measure the dictionary publishes as a value and a unit is published as BOTH: a
//!    `*_value` column has a `*_unit` sibling and the other way round, so a consumer
//!    never has to assume the unit of a measure.
//! 4. A table property the dictionary publishes NO default for assumes nothing: the
//!    listing reports it with its valid values and no `DefaultValue`, `GetTable` reports
//!    it in force only for a request that names it, and it still refuses a value outside
//!    its published list.
//! 5. A table property is published by the tables that have it and by no others —
//!    deprecation by a superseded preview table, row granularity by the tables whose
//!    rows are aggregated over a period, the billing view by the tables the dictionary
//!    scopes that way.
//!
//! The literal expectations below cite their source: the AWS Data Exports table
//! dictionary, which `ListTables` mirrors and `GetTable` reports. The router, the request
//! signing and the error-code helper come from the `export_lifecycle` suite of this same
//! test target.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::export_lifecycle::{error_code, Client};

/// The table the dictionary publishes as a curated dashboard over the cost and usage
/// data.
const DASHBOARD: &str = "COST_AND_USAGE_DASHBOARD";

/// The table the dashboard above aggregates.
const COST_AND_USAGE_REPORT: &str = "COST_AND_USAGE_REPORT";

/// The table the cost optimization recommendations are published for.
const RECOMMENDATIONS: &str = "COST_OPTIMIZATION_RECOMMENDATIONS";

/// The table the carbon emissions estimates are published for.
const CARBON_EMISSIONS: &str = "CARBON_EMISSIONS";

/// What the dictionary says `COST_AND_USAGE_DASHBOARD` is (AWS Data Exports table
/// dictionary).
const DASHBOARD_DESCRIPTION: &str = "A view over CUR for CostAndUsageDashboard";

/// How many columns the dashboard publishes: its aggregate dimensions and measures and
/// the columns it carries over from the cost and usage table, and only those (AWS Data
/// Exports table dictionary).
const DASHBOARD_COLUMN_COUNT: usize = 40;

/// The columns the dashboard publishes, with the kind of data each stores (AWS Data
/// Exports table dictionary). This is the WHOLE published column set for the table, which
/// is what rule 1 is about: every column AWS publishes for it is here, and no other cost
/// and usage line-item column belongs to it.
const DASHBOARD_OWN_COLUMNS: &[(&str, &str)] = &[
    ("amortized_cost", "Number"),
    ("availability_zone", "String"),
    ("billing_entity", "String"),
    ("billing_period", "Timestamp"),
    ("charge_category", "String"),
    ("charge_type", "String"),
    ("current_generation", "String"),
    ("database_engine", "String"),
    ("instance_type", "String"),
    ("instance_type_family", "String"),
    ("invoice_id", "String"),
    ("item_description", "String"),
    ("legal_entity", "String"),
    ("linked_account_id", "String"),
    ("linked_account_name", "String"),
    ("operation", "String"),
    ("payer_account_id", "String"),
    ("payer_account_name", "String"),
    ("platform", "String"),
    // Carried over from the cost and usage table under its own name.
    ("pricing_unit", "String"),
    ("processor", "String"),
    ("processor_features", "String"),
    ("product_code", "String"),
    ("product_family", "String"),
    ("product_from_location", "String"),
    ("product_group", "String"),
    ("product_name", "String"),
    ("product_to_location", "String"),
    ("public_cost", "Number"),
    ("purchase_option", "String"),
    ("region", "String"),
    ("ri_sp_arn", "String"),
    ("ri_sp_trueup", "Number"),
    ("ri_sp_upfront_fees", "Number"),
    ("service", "String"),
    ("tenancy", "String"),
    ("unblended_cost", "Number"),
    ("usage_date", "Timestamp"),
    ("usage_quantity", "Number"),
    ("usage_type", "String"),
];

/// Cost and usage line-item columns the dashboard does NOT publish, including ones the
/// issue that prompted this suite did not cite: the dashboard is not the cost and usage
/// schema under another name.
const NOT_PUBLISHED_BY_THE_DASHBOARD: &[&str] = &[
    "identity_line_item_id",
    "bill_invoicing_entity",
    "bill_payer_account_id",
    "line_item_usage_start_date",
    "line_item_usage_amount",
    "pricing_rate_id",
    "product_instancesku",
    // The dashboard carries the two ends of a transfer over, but not the cost and usage
    // table's location TYPE and Region CODE columns for them.
    "product_from_location_type",
    "product_from_region_code",
    "product_to_location_type",
    "product_to_region_code",
    "product_location",
    "reservation_modification_status",
    "resource_tags",
    "savings_plan_used_commitment",
    "tags",
    // Columns the cost and usage table gates on a table property the dashboard does not
    // publish at all.
    "line_item_resource_id",
    "split_line_item_split_cost",
    "split_line_item_split_usage_ratio",
];

/// `(table, column, the kind of data the table's dictionary publishes it with)` — rows
/// spanning every token in the vocabulary and the schemas that disagree on the token for
/// the same kind of value (AWS Data Exports table dictionary).
const PUBLISHED_TYPES: &[(&str, &str, &str)] = &[
    // The cost and usage dictionary: costs are a numeric measure …
    (COST_AND_USAGE_REPORT, "line_item_unblended_cost", "Number"),
    (COST_AND_USAGE_REPORT, "reservation_effective_cost", "Number"),
    // … except the Savings Plan commitment and rate, which it publishes as text.
    (
        COST_AND_USAGE_REPORT,
        "savings_plan_amortized_upfront_commitment_for_billing_period",
        "String",
    ),
    (
        COST_AND_USAGE_REPORT,
        "savings_plan_savings_plan_rate",
        "String",
    ),
    (COST_AND_USAGE_REPORT, "cost_category", "Map"),
    (COST_AND_USAGE_REPORT, "bill_invoice_id", "String"),
    // The recommendations dictionary: money, percentages and the lookback day count are
    // all Double, a flag is Boolean and the resource's tags are a key-value map.
    (
        RECOMMENDATIONS,
        "estimated_monthly_cost_before_discount",
        "Double",
    ),
    (
        RECOMMENDATIONS,
        "estimated_monthly_cost_after_discount",
        "Double",
    ),
    (
        RECOMMENDATIONS,
        "estimated_monthly_savings_before_discount",
        "Double",
    ),
    (
        RECOMMENDATIONS,
        "estimated_savings_percentage_after_discount",
        "Double",
    ),
    (
        RECOMMENDATIONS,
        "recommendation_lookback_period_in_days",
        "Double",
    ),
    (RECOMMENDATIONS, "tags", "Map"),
    (RECOMMENDATIONS, "restart_needed", "Boolean"),
    // Entries the issue did not cite.
    (RECOMMENDATIONS, "account_id", "String"),
    (RECOMMENDATIONS, "implementation_effort", "String"),
    (RECOMMENDATIONS, "last_refresh_timestamp", "Timestamp"),
    (CARBON_EMISSIONS, "total_scope_1_emissions_value", "Double"),
    (CARBON_EMISSIONS, "total_scope_1_emissions_unit", "String"),
    (DASHBOARD, "billing_period", "Timestamp"),
    ("FOCUS_1_0_AWS", "BilledCost", "Number"),
    ("FOCUS_1_2_AWS", "SkuPriceDetails", "Map"),
];

/// The measures `CARBON_EMISSIONS` publishes, each as a value and the unit that value is
/// measured in (AWS Data Exports table dictionary).
const EMISSIONS_MEASURES: &[&str] = &[
    "total_lbm_emissions",
    "total_mbm_emissions",
    "total_scope_1_emissions",
    "total_scope_2_lbm_emissions",
    "total_scope_2_mbm_emissions",
    "total_scope_3_lbm_emissions",
    "total_scope_3_mbm_emissions",
];

/// `(table, property)` pairs: the table property each table publishes that no other table
/// does (AWS Data Exports table dictionary).
const PROPERTY_OWNERS: &[(&str, &str)] = &[
    ("FOCUS_1_0_AWS_PREVIEW", "DEPRECATED"),
    (COST_AND_USAGE_REPORT, "INCLUDE_IAM_PRINCIPAL_DATA"),
    (COST_AND_USAGE_REPORT, "INCLUDE_RESOURCES"),
    (RECOMMENDATIONS, "INCLUDE_ALL_RECOMMENDATIONS"),
];

/// Whether each table publishes the billing view property (AWS Data Exports table
/// dictionary): the superseded preview table and the FOCUS 1.2 table do not, and neither
/// does the recommendations table, whose rows are not scoped by a billing view.
const BILLING_VIEW_PUBLISHERS: &[(&str, bool)] = &[
    (CARBON_EMISSIONS, true),
    (DASHBOARD, true),
    (COST_AND_USAGE_REPORT, true),
    ("FOCUS_1_0_AWS", true),
    ("FOCUS_1_0_AWS_PREVIEW", false),
    ("FOCUS_1_2_AWS", false),
    (RECOMMENDATIONS, false),
];

/// Tables whose rows are aggregated over a period of time, so the dictionary publishes
/// the granularity of their rows — and one that does not.
const GRANULARITY_PUBLISHERS: &[(&str, bool)] = &[
    (COST_AND_USAGE_REPORT, true),
    ("FOCUS_1_2_AWS", true),
    ("FOCUS_1_0_AWS", false),
    (RECOMMENDATIONS, false),
];

/// The `x_`-prefixed Amazon Web Services extension columns each FOCUS schema publishes
/// (AWS Data Exports table dictionary).
const FOCUS_EXTENSION_COLUMNS: &[(&str, &[&str])] = &[
    (
        "FOCUS_1_0_AWS",
        &[
            "x_CostCategories",
            "x_Discounts",
            "x_Operation",
            "x_ServiceCode",
            "x_UsageType",
        ],
    ),
    (
        "FOCUS_1_0_AWS_PREVIEW",
        &[
            "x_CostCategories",
            "x_Discounts",
            "x_Operation",
            "x_ServiceCode",
            "x_UsageType",
        ],
    ),
    (
        "FOCUS_1_2_AWS",
        &["x_Discounts", "x_Operation", "x_ServiceCode"],
    ),
];

/// One reported column: what the catalog says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedColumn {
    data_type: String,
    description: String,
}

/// `GetTable` for `table` (with the properties a case names), requiring HTTP 200.
async fn get_table(client: &Client, table: &str, properties: Option<Value>) -> Value {
    let request = match properties {
        None => json!({ "TableName": table }),
        Some(properties) => json!({ "TableName": table, "TableProperties": properties }),
    };
    client.ok("GetTable", &request).await
}

/// The reported schema as `name -> (Type, Description)`, and the reported order of the
/// names.
fn schema_of(case: &str, body: &Value) -> (BTreeMap<String, ReportedColumn>, Vec<String>) {
    let columns = body
        .get("Schema")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Schema list, got {body}"));
    let mut reported = BTreeMap::new();
    let mut order = Vec::new();
    for column in columns {
        let name = column
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a column without a Name: {column}"))
            .to_string();
        let data_type = column
            .get("Type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: column {name} without a Type: {column}"))
            .to_string();
        let description = column
            .get("Description")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: column {name} without a Description: {column}"))
            .to_string();
        order.push(name.clone());
        reported.insert(
            name,
            ReportedColumn {
                data_type,
                description,
            },
        );
    }
    (reported, order)
}

/// The `TableProperties` in force, as `GetTable` reports them.
fn properties_in_force(case: &str, body: &Value) -> BTreeMap<String, String> {
    match body.get("TableProperties") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(name, value)| {
                let value = value.as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: table property {name} is not a string: {value}")
                });
                (name.clone(), value.to_string())
            })
            .collect(),
        Some(other) => panic!("case {case:?}: expected TableProperties to be a map, got {other}"),
    }
}

/// Every table `ListTables` advertises, as `name -> the property descriptions it carries`.
async fn listed_properties(client: &Client) -> BTreeMap<String, Vec<Value>> {
    client
        .ok("ListTables", &json!({}))
        .await
        .get("Tables")
        .and_then(Value::as_array)
        .expect("ListTables reports Tables")
        .iter()
        .map(|table| {
            let name = table
                .get("TableName")
                .and_then(Value::as_str)
                .expect("a listed table carries TableName")
                .to_string();
            let properties = table
                .get("TableProperties")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {name:?}: TableProperties must be a list"))
                .clone();
            (name, properties)
        })
        .collect()
}

#[tokio::test]
async fn the_dashboard_publishes_its_own_curated_columns_and_no_others() {
    let client = Client::new();

    let body = get_table(&client, DASHBOARD, None).await;
    assert_eq!(
        body.get("Description").and_then(Value::as_str),
        Some(DASHBOARD_DESCRIPTION),
        "case {DASHBOARD:?}: expected the dictionary's description of the table, got {body}"
    );

    let (dashboard, order) = schema_of(DASHBOARD, &body);
    assert_eq!(
        order.len(),
        DASHBOARD_COLUMN_COUNT,
        "case {DASHBOARD:?}: expected {DASHBOARD_COLUMN_COUNT} published columns, got {} ({order:?})",
        order.len()
    );
    assert_eq!(
        dashboard.len(),
        order.len(),
        "case {DASHBOARD:?}: a column must be published once, got {order:?}"
    );

    // The published column set is exactly the dictionary's, at the kind of data it
    // publishes each column with.
    let published: BTreeSet<&str> = DASHBOARD_OWN_COLUMNS.iter().map(|(name, _)| *name).collect();
    let reported: BTreeSet<&str> = order.iter().map(String::as_str).collect();
    assert_eq!(
        reported, published,
        "case {DASHBOARD:?}: expected the dictionary's column set, extra {:?} / missing {:?}",
        reported.difference(&published).collect::<Vec<_>>(),
        published.difference(&reported).collect::<Vec<_>>()
    );
    for (name, data_type) in DASHBOARD_OWN_COLUMNS {
        let case = format!("{DASHBOARD}.{name}");
        let column = dashboard
            .get(*name)
            .unwrap_or_else(|| panic!("case {case:?}: the table must publish the column"));
        assert_eq!(
            column.data_type, *data_type,
            "case {case:?}: expected Type {data_type:?}, got {:?}",
            column.data_type
        );
    }

    // And no column of the cost and usage table it aggregates, whether that table
    // publishes it always or behind a table property.
    let (aggregated, _) = schema_of(
        COST_AND_USAGE_REPORT,
        &get_table(&client, COST_AND_USAGE_REPORT, None).await,
    );
    for name in NOT_PUBLISHED_BY_THE_DASHBOARD {
        let case = format!("{DASHBOARD}.{name}");
        assert!(
            !dashboard.contains_key(*name),
            "case {case:?}: a cost and usage line-item column is not part of this table, got {:?}",
            dashboard.get(*name)
        );
    }
    for name in aggregated.keys() {
        let case = format!("{DASHBOARD}.{name}");
        assert!(
            !dashboard.contains_key(name.as_str()) || published.contains(name.as_str()),
            "case {case:?}: only the dictionary's own columns are published by this table"
        );
    }
}

#[tokio::test]
async fn every_column_reports_the_token_its_own_dictionary_publishes() {
    let client = Client::new();

    let mut schemas: BTreeMap<&str, BTreeMap<String, ReportedColumn>> = BTreeMap::new();
    for (table, column, data_type) in PUBLISHED_TYPES {
        if !schemas.contains_key(table) {
            let (schema, _) = schema_of(table, &get_table(&client, table, None).await);
            schemas.insert(table, schema);
        }
        let case = format!("{table}.{column}");
        let reported = schemas[table]
            .get(*column)
            .unwrap_or_else(|| panic!("case {case:?}: the table must publish the column"));
        assert_eq!(
            reported.data_type, *data_type,
            "case {case:?}: expected the token its dictionary publishes, {data_type:?}, got {:?}",
            reported.data_type
        );
    }

    // The two cost schemas disagree on the token for a cost, which is the point of rule
    // 2: a token is not generalised from one schema onto another.
    let cur = &schemas[COST_AND_USAGE_REPORT];
    let recommendations = &schemas[RECOMMENDATIONS];
    assert_ne!(
        cur["line_item_unblended_cost"].data_type,
        recommendations["estimated_monthly_cost_before_discount"].data_type,
        "case \"cost token\": the cost and usage and recommendations dictionaries publish different tokens for a cost"
    );
}

#[tokio::test]
async fn a_measure_published_as_a_value_and_a_unit_reports_both() {
    let client = Client::new();

    let (schema, order) = schema_of(
        CARBON_EMISSIONS,
        &get_table(&client, CARBON_EMISSIONS, None).await,
    );

    // The invariant, over every column of the schema: a value column has a unit sibling
    // and the other way round.
    for name in &order {
        for (suffix, sibling_suffix, sibling_type) in [
            ("_value", "_unit", "String"),
            ("_unit", "_value", "Double"),
        ] {
            let Some(measure) = name.strip_suffix(suffix) else {
                continue;
            };
            let sibling = format!("{measure}{sibling_suffix}");
            let case = format!("{CARBON_EMISSIONS}.{name}");
            let reported = schema.get(&sibling).unwrap_or_else(|| {
                panic!("case {case:?}: expected the sibling column {sibling}, got {order:?}")
            });
            assert_eq!(
                reported.data_type, sibling_type,
                "case {case:?}: expected {sibling} at Type {sibling_type:?}, got {:?}",
                reported.data_type
            );
        }
    }

    // The measures the dictionary publishes, no more and no less.
    let measures: BTreeSet<String> = order
        .iter()
        .filter_map(|name| {
            name.strip_suffix("_value")
                .or_else(|| name.strip_suffix("_unit"))
                .map(str::to_string)
        })
        .collect();
    let expected: BTreeSet<String> = EMISSIONS_MEASURES.iter().map(|m| (*m).to_string()).collect();
    assert_eq!(
        measures, expected,
        "case {CARBON_EMISSIONS:?}: expected the dictionary's emissions measures, got {measures:?}"
    );
    // A measure is never published as a single column in an assumed unit.
    for name in &order {
        assert!(
            !name.contains("carbon_emissions"),
            "case {CARBON_EMISSIONS:?}: {name} publishes a measure without its unit"
        );
    }
}

#[tokio::test]
async fn a_property_with_no_published_default_is_in_force_only_when_named() {
    let client = Client::new();

    let listed = listed_properties(&client).await;
    let mut checked = 0usize;
    for (table, properties) in &listed {
        let in_force = properties_in_force(table, &get_table(&client, table, None).await);
        for property in properties {
            let name = property
                .get("Name")
                .and_then(Value::as_str)
                .expect("a listed property carries Name");
            if property.get("DefaultValue").is_some() {
                continue;
            }
            checked += 1;
            let case = format!("{table}/{name}");
            let valid_values: Vec<&str> = property
                .get("ValidValues")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: ValidValues must be a list"))
                .iter()
                .map(|value| value.as_str().expect("a valid value is a string"))
                .collect();
            assert!(
                !valid_values.is_empty(),
                "case {case:?}: a property with no default must still publish the values it accepts"
            );
            assert!(
                !in_force.contains_key(name),
                "case {case:?}: a property with no default must not be in force for a request that names none, got {in_force:?}"
            );

            // Named by the request, it IS in force — at each value it accepts — and the
            // schema is the table's own either way.
            let (baseline, _) = schema_of(
                &case,
                &get_table(&client, table, None).await,
            );
            for value in &valid_values {
                let case = format!("{case}={value}");
                let body = get_table(&client, table, Some(json!({ name: value }))).await;
                assert_eq!(
                    properties_in_force(&case, &body)
                        .get(name)
                        .map(String::as_str),
                    Some(*value),
                    "case {case:?}: expected the named property in force, got {body}"
                );
                let (schema, _) = schema_of(&case, &body);
                assert_eq!(
                    schema, baseline,
                    "case {case:?}: this property changes the data of the table, not its columns"
                );
            }

            // A value outside the published list is still refused.
            let (status, body) = client
                .call(
                    "GetTable",
                    &json!({ "TableName": table, "TableProperties": { name: "MAYBE" } }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a value outside the published list must be refused, got {body}"
            );
        }
    }
    assert!(
        checked > 0,
        "case \"no-default properties\": the catalog publishes at least one property without a default"
    );
}

#[tokio::test]
async fn a_table_property_is_published_by_the_tables_that_have_it() {
    let client = Client::new();

    let listed = listed_properties(&client).await;
    let names_of = |table: &str| -> BTreeSet<String> {
        listed[table]
            .iter()
            .map(|property| {
                property
                    .get("Name")
                    .and_then(Value::as_str)
                    .expect("a listed property carries Name")
                    .to_string()
            })
            .collect()
    };

    for (owner, property) in PROPERTY_OWNERS {
        for table in listed.keys() {
            let case = format!("{table}/{property}");
            let published = names_of(table).contains(*property);
            assert_eq!(
                published,
                table == owner,
                "case {case:?}: only {owner} publishes this property, got published={published}"
            );
        }
    }

    for (table, publishes) in GRANULARITY_PUBLISHERS {
        let case = format!("{table}/TIME_GRANULARITY");
        assert_eq!(
            names_of(table).contains("TIME_GRANULARITY"),
            *publishes,
            "case {case:?}: expected published={publishes}"
        );
    }

    for (table, publishes) in BILLING_VIEW_PUBLISHERS {
        let case = format!("{table}/BILLING_VIEW_ARN");
        assert_eq!(
            names_of(table).contains("BILLING_VIEW_ARN"),
            *publishes,
            "case {case:?}: expected published={publishes}, got {:?}",
            names_of(table)
        );
        // A property a table does not publish is refused for it, so a client cannot set
        // a configuration the dictionary does not advertise.
        if !*publishes {
            let (status, body) = client
                .call(
                    "GetTable",
                    &json!({
                        "TableName": table,
                        "TableProperties": { "BILLING_VIEW_ARN": "arn:aws:billing::123456789012:billingview/primary" },
                    }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a property this table does not publish must be refused, got {body}"
            );
        }
    }

    // The preview table reports its deprecation in force, so a client reading one table
    // learns not to build new exports on it.
    let preview = get_table(&client, "FOCUS_1_0_AWS_PREVIEW", None).await;
    assert_eq!(
        properties_in_force("FOCUS_1_0_AWS_PREVIEW", &preview)
            .get("DEPRECATED")
            .map(String::as_str),
        Some("TRUE"),
        "case \"FOCUS_1_0_AWS_PREVIEW/DEPRECATED\": the preview table reports its deprecation, got {preview}"
    );
    assert!(
        !properties_in_force("FOCUS_1_0_AWS", &get_table(&client, "FOCUS_1_0_AWS", None).await)
            .contains_key("DEPRECATED"),
        "case \"FOCUS_1_0_AWS/DEPRECATED\": the general table is not deprecated"
    );
}

#[tokio::test]
async fn each_focus_schema_publishes_its_own_extension_columns() {
    let client = Client::new();

    for (table, extensions) in FOCUS_EXTENSION_COLUMNS {
        let (_, order) = schema_of(table, &get_table(&client, table, None).await);
        let reported: Vec<&str> = order
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("x_"))
            .collect();
        assert_eq!(
            reported, *extensions,
            "case {table:?}: expected the extension columns this schema publishes, got {reported:?}"
        );
    }
}
