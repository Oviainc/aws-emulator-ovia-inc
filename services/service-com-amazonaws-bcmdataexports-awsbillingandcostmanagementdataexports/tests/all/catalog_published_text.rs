// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VALUES the data-export table catalog publishes — the column
//! descriptions `GetTable` reports and the valid-value lists `ListTables` reports — over
//! the awsJson1_1 wire.
//!
//! The rule under test: every string and list in the catalog is response DATA a client
//! acts on, so each one is the AWS Data Exports table dictionary's PUBLISHED value for
//! that column or property, carried over as published rather than re-worded, and a value
//! belongs to the SCHEMA that publishes it rather than being generalised across schemas.
//! Concretely:
//!
//! 1. `GetTable` reports, for a column, the dictionary's published `Description` for that
//!    column of that table — not a shortened rewrite of it. Asserted over a TABLE of rows
//!    spanning every table in the catalog and the column families in it (identity, bill,
//!    line item, the aggregate dashboard dimensions, the carbon emissions period bounds,
//!    the recommendations columns, the FOCUS specification columns and the FOCUS 1.2
//!    additions), including columns the issue that prompted this suite did not cite. Each
//!    row's expected text is the one AWS was RECORDED publishing for that column, so the
//!    table is not a restatement of the catalog module.
//!    Also over EVERY column of EVERY table: a column carries its own description, so no
//!    two columns of one schema are described by the same string.
//! 2. A column NAME that two schemas both publish carries each schema's OWN published
//!    text: a later specification that re-words a column it carries forward does not
//!    re-word the earlier schema's copy, and the earlier wording does not leak into the
//!    later schema.
//! 3. `ListTables`'s `ValidValues` is what a client validates a `TableConfigurations`
//!    value against, so it agrees with what the service accepts: every value a property
//!    publishes is accepted, a property that publishes the wildcard (or publishes no list
//!    at all) accepts a value no list could enumerate, and an enumerated property refuses
//!    a value outside its list. Asserted over EVERY table and EVERY property the catalog
//!    publishes, not over the one property that showed the defect.
//!
//! Every literal expectation below cites its source: the AWS Data Exports table
//! dictionary, as recorded on the wire for `GetTable` / `ListTables`. The router, the
//! request signing and the error-code helper come from the `export_lifecycle` suite of
//! this same test target.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::PROPERTY_ANY_VALUE;

use super::export_lifecycle::{error_code, Client};

/// `(table, column, the Description the dictionary publishes for it)`.
///
/// SOURCE: the AWS Data Exports table dictionary as RECORDED on the wire for `GetTable` —
/// the text quoted in the issue's recording for the columns it cited, and the text the
/// replay of that recording confirms for the rest. Rows span every table of the catalog.
const PUBLISHED_DESCRIPTIONS: &[(&str, &str, &str)] = &[
    // The carbon emissions table: an account dimension, a period bound, and the shape of
    // its "report" wording.
    (
        "CARBON_EMISSIONS",
        "usage_account_id",
        "The account ID of the account associated with the carbon footprint value. For organizations, this can be either the management account or a member account.",
    ),
    (
        "CARBON_EMISSIONS",
        "usage_period_start",
        "The start date of the period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    (
        "CARBON_EMISSIONS",
        "usage_period_end",
        "The end date of the period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    (
        "CARBON_EMISSIONS",
        "payer_account_id",
        "The account ID of the paying account. For an organization in AWS Organizations, this is the account ID of the management account.",
    ),
    // The cost and usage table: the identity, bill, line item, pricing, product and
    // reservation families.
    (
        "COST_AND_USAGE_REPORT",
        "identity_line_item_id",
        "This field is generated for each line item and is unique in a given partition. This does not guarantee that the field will be unique across an entire delivery (that is, all partitions in an update) of the AWS CUR. The line item ID isn't consistent between different Cost and Usage Reports and can't be used to identify the same line item across different reports.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "identity_time_interval",
        "The time interval that this line item applies to, in the following format: YYYY-MM-DDTHH:mm:ssZ/YYYY-MM-DDTHH:mm:ssZ. The time interval is in UTC and can be either daily or hourly, depending on the granularity of the report.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "bill_bill_type",
        "The type of bill that this report covers.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "bill_billing_entity",
        "Helps you identify whether your invoices or transactions are for AWS Marketplace or for purchases of other AWS services.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "bill_invoice_id",
        "The ID associated with a specific line item. Until the report is final, the InvoiceId is blank.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "bill_payer_account_id",
        "The account ID of the paying account. For an organization in AWS Organizations, this is the account ID of the management account.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "line_item_normalization_factor",
        "As long as the instance has shared tenancy, AWS can apply all Regional Linux or Unix Amazon EC2 and Amazon RDS RI discounts to all instance sizes in an instance family and AWS Region. This also applies to RI discounts for member accounts in an organization. All new and existing Amazon EC2 and Amazon RDS size-flexible RIs are sized according to a normalization factor, based on the instance size.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "line_item_normalized_usage_amount",
        "The amount of usage that you incurred, in normalized units, for size-flexible RIs. The NormalizedUsageAmount is equal to UsageAmount multiplied by NormalizationFactor.",
    ),
    (
        "COST_AND_USAGE_REPORT",
        "line_item_availability_zone",
        "The Availability Zone that hosts this line item.",
    ),
    // The dashboard table: its aggregate dimensions, which the dictionary words as the
    // line item they aggregate.
    (
        "COST_AND_USAGE_DASHBOARD",
        "billing_period",
        "The start date of the billing period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    (
        "COST_AND_USAGE_DASHBOARD",
        "usage_date",
        "The start date for the line item in UTC, inclusive. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    (
        "COST_AND_USAGE_DASHBOARD",
        "invoice_id",
        "The ID associated with a specific line item. Until the report is final, the InvoiceId is blank.",
    ),
    (
        "COST_AND_USAGE_DASHBOARD",
        "charge_category",
        "The category of charge covered by this line item possible values are Running_Usage and non_usage.",
    ),
    // The recommendations table: the resource it is for, and the cost it reports — which
    // the dictionary says is the CURRENT resource's, not the recommended one's.
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        "resource_arn",
        "The Amazon Resource Name (ARN) of the resource.",
    ),
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        "region",
        "The AWS Region of the resource.",
    ),
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        "estimated_monthly_cost_before_discount",
        "The estimated monthly cost of the current resource before discounts. For Reserved Instances and Savings Plans, it refers to the cost for eligible usage.",
    ),
    (
        "COST_OPTIMIZATION_RECOMMENDATIONS",
        "recommendation_id",
        "The ID for the recommendation.",
    ),
    // The FOCUS 1.0 specification columns, and an AWS `x_` extension.
    (
        "FOCUS_1_0_AWS",
        "AvailabilityZone",
        "A provider-assigned identifier for a physically separated and isolated area within a Region that provides high availability and fault tolerance.",
    ),
    (
        "FOCUS_1_0_AWS",
        "BillingCurrency",
        "Represents the currency that a charge was billed in.",
    ),
    (
        "FOCUS_1_0_AWS",
        "ChargeDescription",
        "Self-contained summary of the charge's purpose and price.",
    ),
    (
        "FOCUS_1_0_AWS",
        "CommitmentDiscountCategory",
        "Indicates whether the commitment-based discount identified in the CommitmentDiscountId column is based on usage quantity or cost (aka \"spend\").",
    ),
    (
        "FOCUS_1_0_AWS",
        "BilledCost",
        "A charge serving as the basis for invoicing, inclusive of all reduced rates and discounts while excluding the amortization of upfront charges (one-time or recurring).",
    ),
    (
        "FOCUS_1_0_AWS",
        "ProviderName",
        "The name of the entity that made the resources or services available for purchase.",
    ),
    // The preview table publishes the same FOCUS 1.0 text: it is the same specification.
    (
        "FOCUS_1_0_AWS_PREVIEW",
        "BillingCurrency",
        "Represents the currency that a charge was billed in.",
    ),
    // The FOCUS 1.2 columns, including the ones 1.2 re-words and the ones it adds.
    (
        "FOCUS_1_2_AWS",
        "ChargeCategory",
        "Represents the highest-level classification of a charge based on the nature of how it is billed.",
    ),
    (
        "FOCUS_1_2_AWS",
        "ChargeClass",
        "Indicates whether the row represents a correction to a previously invoiced billing period.",
    ),
    (
        "FOCUS_1_2_AWS",
        "CapacityReservationStatus",
        "Indicates whether the charge represents either the consumption of a capacity reservation or when a capacity reservation is unused.",
    ),
];

/// `(column, the FOCUS 1.0 text, the FOCUS 1.2 text)` for a column BOTH FOCUS schemas
/// publish and the 1.2 specification re-words (AWS Data Exports table dictionary).
const REVISED_BETWEEN_SCHEMAS: &[(&str, &str, &str)] = &[
    (
        "ChargeCategory",
        "Indicates whether the row represents an upfront or recurring fee, cost of usage that already occurred, an after-the-fact adjustment (e.g., credits), or taxes.",
        "Represents the highest-level classification of a charge based on the nature of how it is billed.",
    ),
    (
        "ChargeClass",
        "Indicates whether the row represents a regular charge or a correction to one or more previous charges, its primary use is for differentiating refunds from normal usage.",
        "Indicates whether the row represents a correction to a previously invoiced billing period.",
    ),
];

/// A column name that more than one table publishes, with the tables that publish it:
/// each one reports its own schema's text for it, so a description is not generalised
/// from one schema onto another.
const SHARED_COLUMN_NAMES: &[(&str, &[&str])] = &[
    (
        "region",
        &["COST_AND_USAGE_DASHBOARD", "COST_OPTIMIZATION_RECOMMENDATIONS"],
    ),
    (
        "last_refresh_timestamp",
        &["CARBON_EMISSIONS", "COST_OPTIMIZATION_RECOMMENDATIONS"],
    ),
    (
        "tags",
        &["COST_AND_USAGE_REPORT", "COST_OPTIMIZATION_RECOMMENDATIONS"],
    ),
];

/// A value no published list could enumerate, for a free-form property: used to check
/// that a property advertising the wildcard really does accept any value.
const UNENUMERABLE_VALUES: &[&str] = &[
    "{}",
    "{\"dimensions\":{\"key\":\"REGION\",\"values\":[\"us-east-1\",\"eu-west-1\"]}}",
    "arn:aws:billing::123456789012:billingview/primary",
];

/// A value no enumerated property publishes, used to check that it is refused.
const VALUE_OUTSIDE_EVERY_LIST: &str = "NOT_A_PUBLISHED_VALUE";

/// `GetTable` for `table`, requiring HTTP 200.
async fn get_table(client: &Client, table: &str) -> Value {
    client.ok("GetTable", &json!({ "TableName": table })).await
}

/// The reported schema as `name -> Description`.
async fn descriptions_of(client: &Client, table: &str) -> BTreeMap<String, String> {
    let body = get_table(client, table).await;
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
            let description = column
                .get("Description")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {table:?}: column {name} without a Description: {column}")
                })
                .to_string();
            (name, description)
        })
        .collect()
}

/// One table property as `ListTables` reports it.
#[derive(Debug, Clone)]
struct ListedProperty {
    name: String,
    valid_values: Vec<String>,
    default_value: Option<String>,
}

/// Every table `ListTables` advertises, as `name -> its reported properties`.
async fn listed_properties(client: &Client) -> BTreeMap<String, Vec<ListedProperty>> {
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
                .iter()
                .map(|property| ListedProperty {
                    name: property
                        .get("Name")
                        .and_then(Value::as_str)
                        .expect("a listed property carries Name")
                        .to_string(),
                    valid_values: property
                        .get("ValidValues")
                        .and_then(Value::as_array)
                        .unwrap_or_else(|| {
                            panic!("case {name:?}: ValidValues must be a list, got {property}")
                        })
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .expect("a valid value is a string")
                                .to_string()
                        })
                        .collect(),
                    default_value: property
                        .get("DefaultValue")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
                .collect();
            (name, properties)
        })
        .collect()
}

#[tokio::test]
async fn every_column_reports_the_description_its_dictionary_publishes() {
    let client = Client::new();

    let mut schemas: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    let mut tables_covered: BTreeSet<&str> = BTreeSet::new();
    for (table, column, published) in PUBLISHED_DESCRIPTIONS {
        if !schemas.contains_key(table) {
            schemas.insert(table, descriptions_of(&client, table).await);
        }
        tables_covered.insert(table);
        let case = format!("{table}.{column}");
        let reported = schemas[table]
            .get(*column)
            .unwrap_or_else(|| panic!("case {case:?}: the table must publish the column"));
        assert_eq!(
            reported, published,
            "case {case:?}: expected the dictionary's published description {published:?}, got {reported:?}"
        );
    }

    // The rows span the whole catalog, so a table slipping out of the catalog cannot pass
    // this unnoticed.
    let listed: BTreeSet<String> = listed_properties(&client).await.into_keys().collect();
    for table in &listed {
        assert!(
            tables_covered.contains(table.as_str()),
            "case {table:?}: every published table must be covered by a row of PUBLISHED_DESCRIPTIONS, got {tables_covered:?}"
        );
    }

    // And over every column of every table: each one carries ITS OWN description, so no
    // column of a schema is described by another column's string or by a generic one.
    for table in &listed {
        let described = descriptions_of(&client, table).await;
        let mut by_text: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (name, description) in &described {
            by_text
                .entry(description.as_str())
                .or_default()
                .push(name.as_str());
        }
        for (description, columns) in &by_text {
            assert_eq!(
                columns.len(),
                1,
                "case {table:?}: the columns {columns:?} share one description {description:?}, so at least one is not described by the dictionary's own text for it"
            );
        }
    }
}

#[tokio::test]
async fn a_column_two_schemas_publish_carries_each_schemas_own_text() {
    let client = Client::new();

    let focus_1_0 = descriptions_of(&client, "FOCUS_1_0_AWS").await;
    let focus_1_0_preview = descriptions_of(&client, "FOCUS_1_0_AWS_PREVIEW").await;
    let focus_1_2 = descriptions_of(&client, "FOCUS_1_2_AWS").await;

    for (column, text_1_0, text_1_2) in REVISED_BETWEEN_SCHEMAS {
        for (schema, schema_name, expected) in [
            (&focus_1_0, "FOCUS_1_0_AWS", text_1_0),
            (&focus_1_0_preview, "FOCUS_1_0_AWS_PREVIEW", text_1_0),
            (&focus_1_2, "FOCUS_1_2_AWS", text_1_2),
        ] {
            let case = format!("{schema_name}.{column}");
            let reported = schema
                .get(*column)
                .unwrap_or_else(|| panic!("case {case:?}: the schema must publish the column"));
            assert_eq!(
                reported, expected,
                "case {case:?}: expected this schema's own published text {expected:?}, got {reported:?}"
            );
        }
        assert_ne!(
            text_1_0, text_1_2,
            "case {column:?}: the two specifications word this column differently, which is what this rule is about"
        );
    }

    // The same holds for a column NAME two unrelated tables publish: each reports its own
    // schema's text, which the dictionary words for that table's rows.
    for (column, tables) in SHARED_COLUMN_NAMES {
        let mut texts: Vec<(String, String)> = Vec::new();
        for table in *tables {
            let case = format!("{table}.{column}");
            let reported = descriptions_of(&client, table)
                .await
                .get(*column)
                .unwrap_or_else(|| panic!("case {case:?}: the table must publish the column"))
                .clone();
            assert!(
                !reported.is_empty(),
                "case {case:?}: a published column carries the dictionary's description"
            );
            texts.push(((*table).to_string(), reported));
        }
        assert_eq!(
            texts.len(),
            tables.len(),
            "case {column:?}: every table that publishes the column must report it, got {texts:?}"
        );
    }
}

#[tokio::test]
async fn a_property_accepts_exactly_what_its_valid_values_advertise() {
    let client = Client::new();

    let listed = listed_properties(&client).await;
    assert!(
        !listed.is_empty(),
        "case \"catalog\": the listing must advertise tables"
    );

    let mut enumerated = 0usize;
    let mut free_form = 0usize;
    for (table, properties) in &listed {
        for property in properties {
            let name = property.name.as_str();
            let case = format!("{table}/{name}");
            let wildcard = property
                .valid_values
                .iter()
                .any(|value| value == PROPERTY_ANY_VALUE);
            let unconstrained = wildcard || property.valid_values.is_empty();

            // A property that publishes a default publishes it as one of the values it
            // accepts, so a client that echoes the default back cannot be refused.
            if let Some(default) = &property.default_value {
                let body = client
                    .ok(
                        "GetTable",
                        &json!({ "TableName": table, "TableProperties": { name: default } }),
                    )
                    .await;
                assert_eq!(
                    body.get("TableProperties")
                        .and_then(|reported| reported.get(name))
                        .and_then(Value::as_str),
                    Some(default.as_str()),
                    "case {case:?}: the published default must be accepted and reported in force, got {body}"
                );
            }

            if unconstrained {
                free_form += 1;
                // A value no list could enumerate is accepted and reported in force.
                for value in UNENUMERABLE_VALUES {
                    let case = format!("{case}={value}");
                    let (status, body) = client
                        .call(
                            "GetTable",
                            &json!({ "TableName": table, "TableProperties": { name: value } }),
                        )
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a property advertising {:?} accepts any value, got {status} with {body}",
                        property.valid_values
                    );
                    assert_eq!(
                        body.get("TableProperties")
                            .and_then(|reported| reported.get(name))
                            .and_then(Value::as_str),
                        Some(*value),
                        "case {case:?}: the reported properties must carry the requested value, got {body}"
                    );
                }
            } else {
                enumerated += 1;
                // Every value the list publishes is accepted …
                for value in &property.valid_values {
                    let case = format!("{case}={value}");
                    let body = client
                        .ok(
                            "GetTable",
                            &json!({ "TableName": table, "TableProperties": { name: value } }),
                        )
                        .await;
                    assert_eq!(
                        body.get("TableProperties")
                            .and_then(|reported| reported.get(name))
                            .and_then(Value::as_str),
                        Some(value.as_str()),
                        "case {case:?}: a published value must be accepted, got {body}"
                    );
                }
                // … and a value outside it is refused as a validation failure.
                let (status, body) = client
                    .call(
                        "GetTable",
                        &json!({
                            "TableName": table,
                            "TableProperties": { name: VALUE_OUTSIDE_EVERY_LIST },
                        }),
                    )
                    .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: a value outside the published list must be refused, got {body}"
                );
            }
        }
    }

    assert!(
        enumerated > 0 && free_form > 0,
        "case \"catalog\": the catalog publishes both enumerated ({enumerated}) and free-form ({free_form}) properties, so both branches above are exercised"
    );
}
