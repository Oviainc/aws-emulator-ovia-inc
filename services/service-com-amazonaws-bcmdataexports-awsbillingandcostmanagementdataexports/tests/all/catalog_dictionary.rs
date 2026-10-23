// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the data-export table DICTIONARY that `GetTable` and
//! `ListTables` report, over the awsJson1_1 wire.
//!
//! The rules under test, each asserted over the WHOLE catalog rather than over the one
//! table or column that showed the defect:
//!
//! 1. A column's `Type` names the KIND of data it stores, in the dictionary's mixed-case
//!    vocabulary — `String`, `Number`, `Double`, `Timestamp`, `Boolean`, `Map` — and
//!    every column carries the dictionary's `Description` for it, within the modelled
//!    bound. No column reports a storage-engine type name (`STRING`,
//!    `MAP<STRING,STRING>`). Which token a column carries is ITS schema's: the cost and
//!    usage dictionary measures costs with `Number`, the recommendations dictionary with
//!    `Double`.
//! 2. The catalog publishes the dictionary's table set, and `GetTable` resolves the
//!    `TableName` the request names to THAT table — never to another entry of the
//!    catalog — so each table's schema is its own.
//! 3. A table property is reported as the dictionary publishes it: its name, the values
//!    it accepts (and their order), its default, and a description. A property whose
//!    value the caller writes rather than picks (a billing view ARN, a filter
//!    expression) publishes no valid values and accepts any value; an enumerated one
//!    refuses a value outside its list.
//!
//! The expectations below are LITERAL and cite their source (the AWS Data Exports table
//! dictionary, which `ListTables` mirrors), so they are not a restatement of the
//! catalog module the handlers read; they include entries the issue that prompted this
//! suite did not cite.
//!
//! The router, the request signing and the error-code helper come from the
//! `export_lifecycle` suite of this same test target.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::export_lifecycle::{error_code, Client};

/// Every value a `Column.Type` may take: the kinds of data the AWS Data Exports table
/// dictionary publishes columns with. A type name outside this list is not a kind of
/// data but a storage-engine type. The vocabulary is the catalog's as a whole: which of
/// these tokens a given column carries is its own schema's choice, so
/// `the_published_columns_are_reported_with_their_published_kind_of_data` pins the token
/// per column.
const COLUMN_TYPE_VOCABULARY: &[&str] = &[
    "String",
    "Number",
    "Double",
    "Timestamp",
    "Boolean",
    "Map",
];

/// `Column$Description`, `Column$Type` and `Column$Name` all target `GenericString`:
/// `length 0..1024` (smithy_query shape `Column`).
const GENERIC_STRING_MAX_LEN: usize = 1024;

/// Every table the dictionary publishes, in the order `ListTables` reports them (AWS
/// Data Exports table dictionary).
const PUBLISHED_TABLES: &[&str] = &[
    "CARBON_EMISSIONS",
    "COST_AND_USAGE_DASHBOARD",
    "COST_AND_USAGE_REPORT",
    "COST_OPTIMIZATION_RECOMMENDATIONS",
    "FOCUS_1_0_AWS",
    "FOCUS_1_0_AWS_PREVIEW",
    "FOCUS_1_2_AWS",
];

/// Each table's description, as the dictionary labels it (AWS Data Exports table
/// dictionary).
const TABLE_DESCRIPTIONS: &[(&str, &str)] = &[
    ("CARBON_EMISSIONS", "Carbon Emissions"),
    (
        "COST_AND_USAGE_DASHBOARD",
        "A view over CUR for CostAndUsageDashboard",
    ),
    ("COST_AND_USAGE_REPORT", "Cost and Usage Report"),
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        "Cost Optimization Recommendations from the Cost Optimization Hub.",
    ),
    ("FOCUS_1_0_AWS", "FinOps Open Cost & Usage Specification"),
    (
        "FOCUS_1_0_AWS_PREVIEW",
        "FinOps Open Cost & Usage Specification",
    ),
    ("FOCUS_1_2_AWS", "FinOps Open Cost & Usage Specification"),
];

/// Columns each table publishes, with the kind of data they store (AWS Data Exports
/// table dictionary). Rows span every kind in the vocabulary and every table in the
/// catalog, including columns the FOCUS 1.2 schema adds over FOCUS 1.0.
const PUBLISHED_COLUMNS: &[(&str, &[(&str, &str)])] = &[
    (
        "CARBON_EMISSIONS",
        &[
            ("usage_account_id", "String"),
            ("usage_period_end", "Timestamp"),
            ("location", "String"),
            ("total_scope_2_mbm_emissions_value", "Double"),
            ("total_scope_2_mbm_emissions_unit", "String"),
        ],
    ),
    (
        "COST_AND_USAGE_DASHBOARD",
        &[
            // The dashboard's aggregate dimensions and measures — the whole of its
            // published column set.
            ("billing_period", "Timestamp"),
            ("usage_date", "Timestamp"),
            ("linked_account_name", "String"),
            ("unblended_cost", "Number"),
            ("ri_sp_upfront_fees", "Number"),
        ],
    ),
    (
        "COST_AND_USAGE_REPORT",
        &[
            ("product_fee_description", "String"),
            ("discount_total_discount", "Number"),
            ("line_item_user_identifier", "String"),
            ("reservation_unused_quantity", "Number"),
            ("bill_billing_period_end_date", "Timestamp"),
            ("tags", "Map"),
            // The cost and usage dictionary publishes the Savings Plan commitment and
            // rate as text rather than as a numeric measure.
            (
                "savings_plan_amortized_upfront_commitment_for_billing_period",
                "String",
            ),
            ("savings_plan_savings_plan_rate", "String"),
        ],
    ),
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        &[
            ("implementation_effort", "String"),
            ("rollback_possible", "Boolean"),
            // The recommendations dictionary publishes every numeric column — money,
            // percentages and the lookback day count alike — as `Double`, and the
            // resource's tags as a key-value map: tokens of its own, not the cost and
            // usage dictionary's.
            ("recommendation_lookback_period_in_days", "Double"),
            ("estimated_monthly_cost_before_discount", "Double"),
            ("estimated_savings_percentage_after_discount", "Double"),
            ("tags", "Map"),
        ],
    ),
    (
        "FOCUS_1_0_AWS",
        &[
            ("ChargePeriodEnd", "Timestamp"),
            ("ConsumedQuantity", "Number"),
            ("x_CostCategories", "Map"),
        ],
    ),
    (
        "FOCUS_1_0_AWS_PREVIEW",
        &[("ServiceCategory", "String"), ("ListCost", "Number")],
    ),
    (
        "FOCUS_1_2_AWS",
        &[
            // The columns FOCUS 1.2 adds over FOCUS 1.0 …
            ("ServiceSubcategory", "String"),
            ("SkuMeter", "String"),
            ("CommitmentDiscountQuantity", "Number"),
            // … beside the ones it keeps from it.
            ("BilledCost", "Number"),
            ("BillingPeriodStart", "Timestamp"),
        ],
    ),
];

/// A column the FOCUS 1.0 schema does NOT publish but FOCUS 1.2 does: the two tables are
/// different schemas, not one schema under two names.
const FOCUS_1_2_ONLY_COLUMN: &str = "ServiceSubcategory";

/// One published table property: its name, the values it accepts (the wildcard `*`, or
/// empty, for a free-form property) and the value it assumes when a request does not
/// name it (`None` for a property the dictionary publishes no default for).
type PropertyRow = (
    &'static str,
    &'static [&'static str],
    Option<&'static str>,
);

/// The marker a [`PropertyRow`] names the ACCOUNT's primary billing view with.
///
/// `BILLING_VIEW_ARN` assumes the account's primary billing view
/// (`arn:aws:billing::<account>:billingview/primary`) when a request does not name it, and
/// that ARN carries the account id — so the rows below name the default through this
/// marker and [`published_default`] resolves it from the emulator's account rather than
/// pinning an account id in a literal.
const PRIMARY_BILLING_VIEW: &str = "@the account's primary billing view";

/// The value a [`PropertyRow`]'s default stands for: itself, or — for
/// [`PRIMARY_BILLING_VIEW`] — the account's primary billing view ARN.
fn published_default(row_default: Option<&'static str>) -> Option<String> {
    if row_default == Some(PRIMARY_BILLING_VIEW) {
        return Some(engine::emulator_arn("billing", "", "billingview/primary"));
    }
    row_default.map(str::to_string)
}

/// The table properties the dictionary publishes for a table, in the order it publishes
/// them, with the values each accepts and the value it assumes when a request does not
/// name it. An empty value list, or the wildcard `*`, is a free-form property.
const PUBLISHED_PROPERTIES: &[(&str, &[PropertyRow])] = &[
    ("CARBON_EMISSIONS", &[("BILLING_VIEW_ARN", &[], Some(PRIMARY_BILLING_VIEW))]),
    (
        "COST_AND_USAGE_DASHBOARD",
        &[("BILLING_VIEW_ARN", &[], Some(PRIMARY_BILLING_VIEW))],
    ),
    (
        "COST_AND_USAGE_REPORT",
        &[
            (
                "TIME_GRANULARITY",
                &["DAILY", "MONTHLY", "HOURLY"],
                Some("HOURLY"),
            ),
            (
                "INCLUDE_CAPACITY_RESERVATION_DATA",
                &["TRUE", "FALSE"],
                Some("FALSE"),
            ),
            ("BILLING_VIEW_ARN", &[], Some(PRIMARY_BILLING_VIEW)),
            (
                "INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY",
                &["TRUE", "FALSE"],
                Some("FALSE"),
            ),
            (
                "INCLUDE_SPLIT_COST_ALLOCATION_DATA",
                &["TRUE", "FALSE"],
                Some("FALSE"),
            ),
            // Published with no default: in force only for a request that names it.
            ("INCLUDE_IAM_PRINCIPAL_DATA", &["TRUE", "FALSE"], None),
            ("INCLUDE_RESOURCES", &["TRUE", "FALSE"], Some("FALSE")),
        ],
    ),
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        &[
            (
                "INCLUDE_ALL_RECOMMENDATIONS",
                &["TRUE", "FALSE"],
                Some("FALSE"),
            ),
            // Free-form, and advertised as such by the wildcard: a client validating a
            // TableConfigurations value against this list has to see that any JSON
            // filter is legal.
            ("FILTER", &["*"], Some("{}")),
        ],
    ),
    (
        "FOCUS_1_0_AWS",
        &[("BILLING_VIEW_ARN", &[], Some(PRIMARY_BILLING_VIEW))],
    ),
    (
        "FOCUS_1_0_AWS_PREVIEW",
        // The one thing the dictionary publishes about a superseded preview table.
        &[("DEPRECATED", &["TRUE"], Some("TRUE"))],
    ),
    (
        "FOCUS_1_2_AWS",
        &[(
            "TIME_GRANULARITY",
            &["DAILY", "MONTHLY", "HOURLY"],
            Some("HOURLY"),
        )],
    ),
];

/// `GetTable` for `table`, requiring HTTP 200.
async fn get_table(client: &Client, table: &str) -> Value {
    client.ok("GetTable", &json!({ "TableName": table })).await
}

/// The reported schema as `(Name, Type, Description)` triples, in response order.
fn schema_of(case: &str, body: &Value) -> Vec<(String, String, Option<String>)> {
    body.get("Schema")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Schema list, got {body}"))
        .iter()
        .map(|column| {
            let name = column
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a column without a Name: {column}"))
                .to_string();
            let data_type = column
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: column {name} without a Type"))
                .to_string();
            let description = column
                .get("Description")
                .and_then(Value::as_str)
                .map(str::to_string);
            (name, data_type, description)
        })
        .collect()
}

/// Every table in the catalog, as `ListTables` reports it.
async fn listed_tables(client: &Client) -> Vec<Value> {
    client
        .ok("ListTables", &json!({}))
        .await
        .get("Tables")
        .and_then(Value::as_array)
        .expect("ListTables reports Tables")
        .clone()
}

#[tokio::test]
async fn every_column_of_every_table_reports_a_kind_of_data_and_a_description() {
    let client = Client::new();

    for table in PUBLISHED_TABLES {
        let body = get_table(&client, table).await;
        let schema = schema_of(table, &body);
        assert!(
            !schema.is_empty(),
            "case {table:?}: a published table has a schema, got {body}"
        );
        for (name, data_type, description) in &schema {
            let case = format!("{table}.{name}");
            assert!(
                COLUMN_TYPE_VOCABULARY.contains(&data_type.as_str()),
                "case {case:?}: expected a Type among {COLUMN_TYPE_VOCABULARY:?}, got {data_type:?}"
            );
            let description = description.as_deref().unwrap_or_else(|| {
                panic!("case {case:?}: expected the dictionary's Description, got none")
            });
            assert!(
                !description.is_empty() && description.len() <= GENERIC_STRING_MAX_LEN,
                "case {case:?}: expected a Description within 1..={GENERIC_STRING_MAX_LEN} characters, got {} characters",
                description.len()
            );
            assert!(
                !name.is_empty() && name.len() <= GENERIC_STRING_MAX_LEN,
                "case {case:?}: expected a Name within 1..={GENERIC_STRING_MAX_LEN} characters, got {} characters",
                name.len()
            );
        }
    }
}

#[tokio::test]
async fn the_published_columns_are_reported_with_their_published_kind_of_data() {
    let client = Client::new();

    for (table, columns) in PUBLISHED_COLUMNS {
        let body = get_table(&client, table).await;
        let reported: BTreeMap<String, String> = schema_of(table, &body)
            .into_iter()
            .map(|(name, data_type, _)| (name, data_type))
            .collect();
        for (name, data_type) in *columns {
            let case = format!("{table}.{name}");
            assert_eq!(
                reported.get(*name).map(String::as_str),
                Some(*data_type),
                "case {case:?}: expected the published column at type {data_type:?}, got {:?}",
                reported.get(*name)
            );
        }
    }
}

#[tokio::test]
async fn the_catalog_publishes_the_dictionarys_table_set() {
    let client = Client::new();

    let listed: Vec<String> = listed_tables(&client)
        .await
        .iter()
        .map(|table| {
            table
                .get("TableName")
                .and_then(Value::as_str)
                .expect("a listed table carries TableName")
                .to_string()
        })
        .collect();
    assert_eq!(
        listed,
        PUBLISHED_TABLES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>(),
        "case \"catalog\": expected the dictionary's table set, got {listed:?}"
    );

    for (table, description) in TABLE_DESCRIPTIONS {
        let body = get_table(&client, table).await;
        assert_eq!(
            body.get("Description").and_then(Value::as_str),
            Some(*description),
            "case {table:?}: expected the dictionary's table description, got {body}"
        );
    }
}

#[tokio::test]
async fn get_table_resolves_the_name_the_request_names() {
    let client = Client::new();

    // Each table's schema is its own: the reader answers for the name it was given, not
    // for a neighbouring entry of the catalog.
    let mut schemas: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for table in PUBLISHED_TABLES {
        let body = get_table(&client, table).await;
        assert_eq!(
            body.get("TableName").and_then(Value::as_str),
            Some(*table),
            "case {table:?}: GetTable must report the table it was asked for, got {body}"
        );
        schemas.insert(
            table,
            schema_of(table, &body)
                .into_iter()
                .map(|(name, _, _)| name)
                .collect(),
        );
    }

    // The cost and usage schema and the FOCUS schema share no column at all, so a reader
    // that answered the wrong table could not pass by accident.
    let cur = &schemas["COST_AND_USAGE_REPORT"];
    let focus = &schemas["FOCUS_1_0_AWS"];
    assert!(
        cur.is_disjoint(focus),
        "case \"CUR vs FOCUS\": the two schemas must share no column, got {:?}",
        cur.intersection(focus).collect::<Vec<_>>()
    );
    for pinned in [
        "identity_line_item_id",
        "bill_invoice_id",
        "savings_plan_used_commitment",
    ] {
        assert!(
            cur.contains(pinned),
            "case \"COST_AND_USAGE_REPORT\": expected the cost and usage column {pinned}, got {cur:?}"
        );
    }

    // FOCUS 1.2 extends the FOCUS 1.0 SPECIFICATION columns rather than aliasing them.
    // The `x_`-prefixed provider extensions are each schema's own — the columns FOCUS 1.2
    // standardises cover two of FOCUS 1.0's — so they are compared per schema in
    // `catalog_schema.rs` instead.
    let focus_1_2 = &schemas["FOCUS_1_2_AWS"];
    let specified = |columns: &BTreeSet<String>| -> BTreeSet<String> {
        columns
            .iter()
            .filter(|name| !name.starts_with("x_"))
            .cloned()
            .collect()
    };
    let specified_1_0 = specified(focus);
    let specified_1_2 = specified(focus_1_2);
    assert!(
        specified_1_0.is_subset(&specified_1_2),
        "case \"FOCUS 1.2\": every FOCUS 1.0 specification column must still be published, missing {:?}",
        specified_1_0.difference(&specified_1_2).collect::<Vec<_>>()
    );
    assert!(
        focus_1_2.contains(FOCUS_1_2_ONLY_COLUMN) && !focus.contains(FOCUS_1_2_ONLY_COLUMN),
        "case \"FOCUS 1.2\": {FOCUS_1_2_ONLY_COLUMN} must be published by FOCUS 1.2 and not by FOCUS 1.0"
    );
}

#[tokio::test]
async fn every_table_property_is_reported_as_the_dictionary_publishes_it() {
    let client = Client::new();

    let listed = listed_tables(&client).await;
    for (table, properties) in PUBLISHED_PROPERTIES {
        let entry = listed
            .iter()
            .find(|candidate| candidate.get("TableName").and_then(Value::as_str) == Some(*table))
            .unwrap_or_else(|| panic!("case {table:?}: the listing must advertise the table"));
        let reported = entry
            .get("TableProperties")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {table:?}: TableProperties must be a list, got {entry}"));
        assert_eq!(
            reported.len(),
            properties.len(),
            "case {table:?}: expected {} table propert(ies), got {reported:?}",
            properties.len()
        );
        for (index, (name, valid_values, default_value)) in properties.iter().enumerate() {
            let case = format!("{table}/{name}");
            let property = &reported[index];
            assert_eq!(
                property.get("Name").and_then(Value::as_str),
                Some(*name),
                "case {case:?}: expected the property the dictionary publishes at position {index}, got {property}"
            );
            let reported_values: Vec<&str> = property
                .get("ValidValues")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: ValidValues must be a list, got {property}"))
                .iter()
                .map(|value| value.as_str().expect("a valid value is a string"))
                .collect();
            assert_eq!(
                reported_values, *valid_values,
                "case {case:?}: expected the published values in order, got {reported_values:?}"
            );
            assert_eq!(
                property.get("DefaultValue").and_then(Value::as_str),
                published_default(*default_value).as_deref(),
                "case {case:?}: expected the published default, got {property}"
            );
            let description = property
                .get("Description")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a property carries a Description, got {property}"));
            assert!(
                !description.is_empty() && description.len() <= GENERIC_STRING_MAX_LEN,
                "case {case:?}: expected a Description within 1..={GENERIC_STRING_MAX_LEN} characters, got {} characters",
                description.len()
            );
        }

        // GetTable reports the same properties, at their defaults, for a request that
        // names none.
        let body = get_table(&client, table).await;
        let described: BTreeMap<String, String> = body
            .get("TableProperties")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            value.as_str().expect("a property value is a string").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected: BTreeMap<String, String> = properties
            .iter()
            .filter_map(|(name, _, default_value)| {
                published_default(*default_value).map(|default| ((*name).to_string(), default))
            })
            .collect();
        assert_eq!(
            described, expected,
            "case {table:?}: expected GetTable to report the published defaults {expected:?}, got {described:?}"
        );
    }
}

/// One free-form property value per table that publishes one: a value no list could
/// enumerate, which the property must accept and the response must report in force.
const FREE_FORM_CASES: &[(&str, &str, &str)] = &[
    (
        "COST_AND_USAGE_REPORT",
        "BILLING_VIEW_ARN",
        "arn:aws:billing::123456789012:billingview/custom-view",
    ),
    (
        "FOCUS_1_0_AWS",
        "BILLING_VIEW_ARN",
        "arn:aws:billing::210987654321:billingview/primary",
    ),
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        "FILTER",
        "{\"dimensions\":{\"key\":\"REGION\",\"values\":[\"us-east-1\"]}}",
    ),
];

#[tokio::test]
async fn a_free_form_property_accepts_a_value_no_list_enumerates() {
    let client = Client::new();

    for (table, property, value) in FREE_FORM_CASES {
        let case = format!("{table}/{property}={value}");
        let (status, body) = client
            .call(
                "GetTable",
                &json!({ "TableName": table, "TableProperties": { (*property): value } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a free-form property accepts any value, got {status} with {body}"
        );
        assert_eq!(
            body.get("TableProperties")
                .and_then(|properties| properties.get(*property))
                .and_then(Value::as_str),
            Some(*value),
            "case {case:?}: the reported properties must carry the requested value, got {body}"
        );
        // The schema is the table's own either way: a free-form property gates no column.
        let baseline = schema_of(&case, &get_table(&client, table).await);
        assert_eq!(
            schema_of(&case, &body),
            baseline,
            "case {case:?}: a free-form property must not change the schema"
        );
    }

    // An ENUMERATED property still refuses a value outside its published list.
    let (status, body) = client
        .call(
            "GetTable",
            &json!({
                "TableName": "COST_OPTIMIZATION_RECOMMENDATIONS",
                "TableProperties": { "INCLUDE_ALL_RECOMMENDATIONS": "{}" },
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"INCLUDE_ALL_RECOMMENDATIONS={{}}\": an enumerated property refuses a value outside its list, got {body}"
    );
}
