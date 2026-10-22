// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetTable` over the awsJson1_1 wire.
//!
//! The rule under test: `GetTable` reports the entry of the service's table catalog the
//! request names — the table's name and description, the table properties in force (the
//! request's values over the catalog's published defaults) and the columns those
//! properties select — and refuses, with the modelled `ValidationException` (HTTP 400),
//! a table name the catalog does not publish, a property the named table does not
//! publish, or a value outside a property's valid values.
//!
//! It is asserted over a TABLE of requests that differ along everything the rule
//! generalises over: every catalog table (so property counts 1, 2 and 6 and schemas of
//! 11 to 140-odd columns), an absent / empty / partial / complete `TableProperties`
//! map, enumerated and free-form property values, and — bounded and exhaustive — every
//! combination of the values of every property of each table. Each row's expectation is
//! computed from the catalog, and a separate set of literal spot checks pins catalog
//! entries (published column names and types, property defaults and valid values) so
//! the end-to-end rows are not merely a restatement of the catalog module.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    TablePropertySpec, TableSpec, PROPERTY_FALSE, PROPERTY_INCLUDE_ALL_RECOMMENDATIONS,
    PROPERTY_INCLUDE_RESOURCES, PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
    PROPERTY_TIME_GRANULARITY, PROPERTY_TRUE, TABLE_CATALOG,
};

use super::export_lifecycle::{error_code, Client};

/// The table the Cost and Usage Report 2.0 schema is published for.
const COST_AND_USAGE_REPORT: &str = "COST_AND_USAGE_REPORT";

/// The table the cost optimization recommendations are published for.
const COST_OPTIMIZATION_RECOMMENDATIONS: &str = "COST_OPTIMIZATION_RECOMMENDATIONS";

/// The table the FOCUS 1.0 schema is published for.
const FOCUS_1_0_AWS: &str = "FOCUS_1_0_AWS";

/// A table the catalog publishes NO table properties for.
const CARBON_EMISSIONS: &str = "CARBON_EMISSIONS";

/// Values to exercise a FREE-FORM table property at — one whose value the caller writes
/// rather than picks from a published list (a billing view ARN, a filter expression), so
/// the catalog publishes no `ValidValues` for it and it accepts any value.
const FREE_FORM_VALUES: &[&str] = &["{}", "arn:aws:billing::123456789012:billingview/primary"];

/// One request's `TableProperties`, as `(property, value)` pairs.
type Properties = Vec<(&'static str, &'static str)>;

/// The values a row sets `property` to: the ones the catalog publishes for it, or — for
/// a free-form property — [`FREE_FORM_VALUES`].
fn exercised_values(property: &TablePropertySpec) -> &'static [&'static str] {
    if property.valid_values.is_empty() {
        FREE_FORM_VALUES
    } else {
        property.valid_values
    }
}

/// The `TableProperties` requests to exercise for `table`: none at all, an empty map,
/// each single property at each of the values it is exercised at, and — bounded by those
/// value lists — every complete combination of them.
fn property_requests(table: &TableSpec) -> Vec<Option<Properties>> {
    let mut requests: Vec<Option<Properties>> = vec![None, Some(Vec::new())];
    for property in table.properties {
        for value in exercised_values(property) {
            requests.push(Some(vec![(property.name, *value)]));
        }
    }
    let mut combinations: Vec<Properties> = vec![Vec::new()];
    for property in table.properties {
        let mut extended = Vec::new();
        for combination in &combinations {
            for value in exercised_values(property) {
                let mut row = combination.clone();
                row.push((property.name, *value));
                extended.push(row);
            }
        }
        combinations = extended;
    }
    // The empty combination is already covered by the first two rows.
    requests.extend(
        combinations
            .into_iter()
            .filter(|combination| !combination.is_empty())
            .map(Some),
    );
    requests
}

/// The properties the service must report for `requested`: every property `table`
/// publishes, at the requested value where the request names it and at the catalog
/// default otherwise.
fn expected_properties(
    table: &TableSpec,
    requested: Option<&Properties>,
) -> BTreeMap<String, String> {
    let mut effective: BTreeMap<String, String> = table
        .properties
        .iter()
        // A property the catalog publishes no default for assumes nothing, so it is in
        // force only for a request that names it.
        .filter_map(|property| {
            property
                .default_value
                .map(|default| (property.name.to_string(), default.to_string()))
        })
        .collect();
    for (name, value) in requested.into_iter().flatten() {
        effective.insert((*name).to_string(), (*value).to_string());
    }
    effective
}

/// The schema the service must report under `properties`: `table`'s columns in catalog
/// order, less those whose gating property does not hold the value they need.
fn expected_schema(table: &TableSpec, properties: &BTreeMap<String, String>) -> Vec<(String, String)> {
    table
        .columns()
        .filter(|column| match column.requires {
            None => true,
            Some((property, value)) => {
                properties.get(property).map(String::as_str) == Some(value)
            }
        })
        .map(|column| (column.name.to_string(), column.data_type.to_string()))
        .collect()
}

/// A `GetTable` request body for `table`, carrying `requested` when the row names one.
fn request_body(table: &str, requested: Option<&Properties>) -> Value {
    match requested {
        None => json!({ "TableName": table }),
        Some(pairs) => {
            let mut map = Map::new();
            for (name, value) in pairs {
                map.insert((*name).to_string(), Value::String((*value).to_string()));
            }
            json!({ "TableName": table, "TableProperties": Value::Object(map) })
        }
    }
}

/// The `(Name, Type)` of each reported column, in the order the response lists them.
fn reported_schema(case: &str, body: &Value) -> Vec<(String, String)> {
    let columns = body
        .get("Schema")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Schema list, got {body}"));
    columns
        .iter()
        .map(|column| {
            let name = column
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a column without a Name: {column}"));
            let data_type = column
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: column {name} without a Type"));
            (name.to_string(), data_type.to_string())
        })
        .collect()
}

/// The reported `TableProperties`, as a sorted map.
fn reported_properties(case: &str, body: &Value) -> BTreeMap<String, String> {
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

#[tokio::test]
async fn every_catalog_table_reports_the_schema_its_properties_select() {
    let client = Client::new();

    for table in TABLE_CATALOG {
        for requested in property_requests(table) {
            let case = format!("{} with TableProperties {requested:?}", table.name);
            let body = client
                .ok("GetTable", &request_body(table.name, requested.as_ref()))
                .await;

            assert_eq!(
                body.get("TableName").and_then(Value::as_str),
                Some(table.name),
                "case {case:?}: expected TableName {}, got {body}",
                table.name
            );
            assert_eq!(
                body.get("Description").and_then(Value::as_str),
                Some(table.description),
                "case {case:?}: expected the catalog description, got {body}"
            );

            let expected = expected_properties(table, requested.as_ref());
            let actual = reported_properties(&case, &body);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the properties in force {expected:?}, got {actual:?}"
            );

            let expected_columns = expected_schema(table, &expected);
            let actual_columns = reported_schema(&case, &body);
            assert_eq!(
                actual_columns, expected_columns,
                "case {case:?}: expected {} columns, got {} ({actual_columns:?})",
                expected_columns.len(),
                actual_columns.len()
            );
            assert!(
                !actual_columns.is_empty(),
                "case {case:?}: a catalog table always has a schema"
            );

            let mut names: Vec<&str> = actual_columns
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            let reported = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(
                names.len(),
                reported,
                "case {case:?}: the schema reports a column name more than once"
            );
        }
    }
}

/// Published columns and the types the catalog publishes them with (AWS Data Exports
/// table dictionary) — spot checks on catalog entries, so the computed rows above cannot
/// pass on an empty or invented catalog.
const KNOWN_COLUMNS: &[(&str, &[(&str, &str)])] = &[
    (
        COST_AND_USAGE_REPORT,
        &[
            ("identity_line_item_id", "String"),
            ("identity_time_interval", "String"),
            ("bill_payer_account_id", "String"),
            ("bill_billing_period_start_date", "Timestamp"),
            ("line_item_usage_amount", "Number"),
            ("line_item_unblended_cost", "Number"),
            ("line_item_usage_start_date", "Timestamp"),
            ("pricing_unit", "String"),
            ("product", "Map"),
            ("cost_category", "Map"),
            ("savings_plan_savings_plan_effective_cost", "Number"),
        ],
    ),
    (
        COST_OPTIMIZATION_RECOMMENDATIONS,
        &[
            ("recommendation_id", "String"),
            ("account_id", "String"),
            ("account_name", "String"),
            ("estimated_monthly_savings_after_discount", "Double"),
            ("recommendation_lookback_period_in_days", "Integer"),
            ("last_refresh_timestamp", "Timestamp"),
            ("restart_needed", "Boolean"),
            ("tags", "String"),
        ],
    ),
    (
        FOCUS_1_0_AWS,
        &[
            ("BilledCost", "Number"),
            ("EffectiveCost", "Number"),
            ("BillingPeriodStart", "Timestamp"),
            ("ChargeCategory", "String"),
            ("ResourceId", "String"),
            ("Tags", "Map"),
            ("x_ServiceCode", "String"),
        ],
    ),
    (
        CARBON_EMISSIONS,
        &[
            ("payer_account_id", "String"),
            ("usage_account_id", "String"),
            ("usage_period_start", "Timestamp"),
            ("usage_period_end", "Timestamp"),
            ("last_refresh_timestamp", "Timestamp"),
            ("product_code", "String"),
            ("region_code", "String"),
            ("location", "String"),
        ],
    ),
];

#[tokio::test]
async fn published_columns_are_reported_with_their_published_types() {
    let client = Client::new();

    for (table, columns) in KNOWN_COLUMNS {
        let body = client.ok("GetTable", &json!({ "TableName": table })).await;
        let reported: BTreeMap<String, String> = reported_schema(table, &body).into_iter().collect();
        for (name, data_type) in *columns {
            let case = format!("{table}.{name}");
            assert_eq!(
                reported.get(*name).map(String::as_str),
                Some(*data_type),
                "case {case:?}: expected type {data_type}, got {:?}",
                reported.get(*name)
            );
        }
    }
}

/// The default of every `COST_AND_USAGE_REPORT` table property (AWS Data Exports table
/// dictionary), which a request naming no property must report.
const COST_AND_USAGE_REPORT_DEFAULTS: &[(&str, &str)] = &[
    ("BILLING_VIEW_ARN", ""),
    ("INCLUDE_CAPACITY_RESERVATION_DATA", "FALSE"),
    ("INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY", "FALSE"),
    ("INCLUDE_RESOURCES", "FALSE"),
    ("INCLUDE_SPLIT_COST_ALLOCATION_DATA", "FALSE"),
    ("TIME_GRANULARITY", "HOURLY"),
];

#[tokio::test]
async fn a_request_naming_no_property_reports_the_published_defaults() {
    let client = Client::new();

    let body = client
        .ok("GetTable", &json!({ "TableName": COST_AND_USAGE_REPORT }))
        .await;
    let reported = reported_properties("defaults", &body);
    let expected: BTreeMap<String, String> = COST_AND_USAGE_REPORT_DEFAULTS
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        reported, expected,
        "case \"defaults\": expected the published defaults {expected:?}, got {reported:?}"
    );

    // A table the catalog publishes one property for reports exactly that one.
    let carbon = client
        .ok("GetTable", &json!({ "TableName": CARBON_EMISSIONS }))
        .await;
    assert_eq!(
        reported_properties("one property", &carbon),
        BTreeMap::from([("BILLING_VIEW_ARN".to_string(), String::new())]),
        "case \"one property\": {CARBON_EMISSIONS} publishes the billing view property alone, got {carbon}"
    );
}

/// Columns a table property adds to the `COST_AND_USAGE_REPORT` schema: absent while the
/// property holds its default, present while it holds `TRUE` (AWS Data Exports table
/// dictionary).
const GATED_COLUMNS: &[(&str, &[&str])] = &[
    (PROPERTY_INCLUDE_RESOURCES, &["line_item_resource_id"]),
    (
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        &[
            "split_line_item_parent_resource_id",
            "split_line_item_split_cost",
            "split_line_item_split_usage_ratio",
        ],
    ),
];

#[tokio::test]
async fn a_table_property_changes_the_schema_it_gates_and_nothing_else() {
    let client = Client::new();

    let baseline = reported_schema(
        "baseline",
        &client
            .ok("GetTable", &json!({ "TableName": COST_AND_USAGE_REPORT }))
            .await,
    );

    for (property, gated) in GATED_COLUMNS {
        for column in *gated {
            assert!(
                !baseline.iter().any(|(name, _)| name == column),
                "case {property:?}: {column} must be absent while the property holds its default, got {baseline:?}"
            );
        }

        let case = format!("{property}=TRUE");
        let body = client
            .ok(
                "GetTable",
                &json!({
                    "TableName": COST_AND_USAGE_REPORT,
                    "TableProperties": { (*property): PROPERTY_TRUE },
                }),
            )
            .await;
        let with_property = reported_schema(&case, &body);
        for column in *gated {
            assert!(
                with_property.iter().any(|(name, _)| name == column),
                "case {case:?}: expected {column} in the schema, got {with_property:?}"
            );
        }

        // Switching one property on adds exactly the columns that property gates: every
        // other column, and their order, is the baseline's.
        let added: Vec<&str> = with_property
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| !baseline.iter().any(|(baseline_name, _)| baseline_name == name))
            .collect();
        for column in *gated {
            assert!(
                added.contains(column),
                "case {case:?}: expected {column} among the added columns, got {added:?}"
            );
        }
        let spec = TABLE_CATALOG
            .iter()
            .find(|table| table.name == COST_AND_USAGE_REPORT)
            .expect("the catalog publishes the Cost and Usage Report table");
        for column in &added {
            let requires = spec
                .columns()
                .find(|candidate| candidate.name == *column)
                .and_then(|candidate| candidate.requires);
            assert_eq!(
                requires,
                Some((*property, PROPERTY_TRUE)),
                "case {case:?}: {column} was added by a request that only set {property}, but the catalog gates it on {requires:?}"
            );
        }
        assert_eq!(
            reported_properties(&case, &body)
                .get(*property)
                .map(String::as_str),
            Some(PROPERTY_TRUE),
            "case {case:?}: the reported properties must carry the requested value"
        );
    }
}

/// One refused request: why it is refused, the table name, the properties, and the
/// `ValidationException` field the refusal names.
struct Refusal {
    label: &'static str,
    table: &'static str,
    properties: Option<Properties>,
    field: &'static str,
}

/// Requests outside the catalog, along each way a request can fall outside it.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            label: "a table name the catalog does not publish",
            table: "NOT_A_TABLE",
            properties: None,
            field: "TableName",
        },
        Refusal {
            label: "a published name in the wrong case",
            table: "cost_and_usage_report",
            properties: None,
            field: "TableName",
        },
        Refusal {
            label: "a published name with trailing whitespace",
            table: "COST_AND_USAGE_REPORT ",
            properties: None,
            field: "TableName",
        },
        Refusal {
            label: "an empty table name",
            table: "",
            properties: None,
            field: "TableName",
        },
        Refusal {
            label: "a very long table name",
            // `TableName` admits 1024 characters, so an outsized name is refused as
            // unknown rather than rejected at the boundary.
            table: "COST_AND_USAGE_REPORT_COST_AND_USAGE_REPORT_COST_AND_USAGE_REPORT_COST_AND_USAGE_REPORT_COST_AND_USAGE_REPORT_COST_AND_USAGE_REPORT_COST_AND_USAGE_REPORT",
            properties: None,
            field: "TableName",
        },
        Refusal {
            label: "a property no table publishes",
            table: COST_AND_USAGE_REPORT,
            properties: Some(vec![("NOT_A_PROPERTY", PROPERTY_TRUE)]),
            field: "TableProperties",
        },
        Refusal {
            label: "a property another table publishes",
            table: COST_AND_USAGE_REPORT,
            properties: Some(vec![(PROPERTY_INCLUDE_ALL_RECOMMENDATIONS, PROPERTY_TRUE)]),
            field: "TableProperties",
        },
        Refusal {
            label: "a cost and usage property on the carbon emissions table",
            table: CARBON_EMISSIONS,
            properties: Some(vec![(PROPERTY_INCLUDE_RESOURCES, PROPERTY_TRUE)]),
            field: "TableProperties",
        },
        Refusal {
            label: "a cost and usage property on a FOCUS table",
            table: FOCUS_1_0_AWS,
            properties: Some(vec![(PROPERTY_INCLUDE_RESOURCES, PROPERTY_TRUE)]),
            field: "TableProperties",
        },
        Refusal {
            label: "a value outside a boolean property's valid values",
            table: COST_AND_USAGE_REPORT,
            properties: Some(vec![(PROPERTY_INCLUDE_RESOURCES, "MAYBE")]),
            field: "TableProperties",
        },
        Refusal {
            label: "a valid value in the wrong case",
            table: COST_AND_USAGE_REPORT,
            properties: Some(vec![(PROPERTY_INCLUDE_RESOURCES, "true")]),
            field: "TableProperties",
        },
        Refusal {
            label: "an empty property value",
            table: COST_AND_USAGE_REPORT,
            properties: Some(vec![(PROPERTY_TIME_GRANULARITY, "")]),
            field: "TableProperties",
        },
        Refusal {
            label: "a granularity outside the published values",
            table: COST_AND_USAGE_REPORT,
            properties: Some(vec![(PROPERTY_TIME_GRANULARITY, "WEEKLY")]),
            field: "TableProperties",
        },
        Refusal {
            label: "one valid property beside one invalid value",
            table: COST_AND_USAGE_REPORT,
            properties: Some(vec![
                (PROPERTY_INCLUDE_RESOURCES, PROPERTY_FALSE),
                (PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA, "YES"),
            ]),
            field: "TableProperties",
        },
        Refusal {
            label: "a property the recommendations table does not publish",
            table: COST_OPTIMIZATION_RECOMMENDATIONS,
            properties: Some(vec![(PROPERTY_TIME_GRANULARITY, "DAILY")]),
            field: "TableProperties",
        },
    ]
}

#[tokio::test]
async fn a_request_outside_the_catalog_is_refused_as_a_validation_failure() {
    let client = Client::new();

    for refusal in refusals() {
        let case = refusal.label;
        let (status, body) = client
            .call(
                "GetTable",
                &request_body(refusal.table, refusal.properties.as_ref()),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let message = body.get("Message").and_then(Value::as_str).unwrap_or("");
        assert!(
            !message.is_empty() && message.len() <= 1024,
            "case {case:?}: expected a message within the modelled 0..1024 bound, got {message:?}"
        );
        let fields = body
            .get("Fields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected the refusal to name a field: {body}"));
        let named: Vec<&str> = fields
            .iter()
            .filter_map(|field| field.get("Name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            named,
            vec![refusal.field],
            "case {case:?}: expected the refusal to name {}, got {named:?}",
            refusal.field
        );
    }
}

#[tokio::test]
async fn reporting_a_table_changes_nothing_a_later_caller_reads() {
    let client = Client::new();

    let request = json!({ "TableName": COST_AND_USAGE_REPORT });
    let first = client.ok("GetTable", &request).await;

    // Reads in between, including refused ones and ones selecting other configurations.
    for table in TABLE_CATALOG {
        client.ok("GetTable", &json!({ "TableName": table.name })).await;
    }
    client
        .ok(
            "GetTable",
            &json!({
                "TableName": COST_AND_USAGE_REPORT,
                "TableProperties": { PROPERTY_INCLUDE_RESOURCES: PROPERTY_TRUE },
            }),
        )
        .await;
    let (refused, _) = client
        .call("GetTable", &json!({ "TableName": "NOT_A_TABLE" }))
        .await;
    assert_eq!(refused, 400, "a request outside the catalog stays refused");

    let again = client.ok("GetTable", &request).await;
    assert_eq!(
        again, first,
        "case \"repeat read\": GetTable must report the same entry as before, got {again} after {first}"
    );
}
