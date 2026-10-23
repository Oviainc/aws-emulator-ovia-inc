// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the catalog entry `GetTable` reports PER TABLE AND PER TABLE
//! VERSION, over the awsJson1_1 wire.
//!
//! The rule under test: `GetTable` reports, for each column of the table the request
//! names, the `Name`, `Type` and `Description` the AWS Data Exports table dictionary
//! publishes for THAT table at THAT schema version — so the catalog holds one entry per
//! table version (a later specification's re-wording belongs to the version that
//! publishes it and never leaks into the version it supersedes, nor the other way round),
//! and the entry's column SET is the whole set the dictionary publishes for the table,
//! including the columns a curated VIEW carries over from the table it views.
//!
//! It is asserted over TABLES of rows that differ along what the rule generalises over:
//!
//! 1. Every column the FOCUS 1.2 specification re-words over FOCUS 1.0, spanning all
//!    three families the revision touches (the period bounds, the commitment-discount
//!    family and the unit price / effective cost), each checked in the 1.0 table, in the
//!    1.0 preview table — which publishes the same specification — and in the 1.2 table.
//!    Plus the invariants over the WHOLE shared column set: a column both versions
//!    publish keeps one `Type` across them (a re-wording is not a change of the kind of
//!    data), and the two versions are not one catalog entry under two names.
//! 2. The curated view's column set: each column it carries over from the viewed table is
//!    published and reports the viewed table's own `Type` and `Description`, while a
//!    column it renames into an aggregate of its own is described in its own words.
//! 3. The published `Description` of columns spanning every table of the catalog and the
//!    families the issue that prompted this suite cited, including columns it did not
//!    cite, and the semantic invariant that distinguishes the recommendations table's
//!    cost columns (the CURRENT resource's cost, before and after discounts) from its
//!    savings columns (what the recommendation would save).
//!
//! Every literal expectation cites its source: the AWS Data Exports table dictionary as
//! recorded on the wire for `GetTable`, which is the contract for the column set, the
//! `Type` tokens and the `Description` text (the Smithy model does not enumerate the
//! catalog's contents — `smithy_query` operation `GetTable` gives only `Schema ->
//! ColumnList` of `Column{Name, Type, Description}`). The router and the request signing
//! come from the `export_lifecycle` suite of this same test target.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::export_lifecycle::Client;

/// The table the FOCUS 1.0 specification is published for.
const FOCUS_1_0: &str = "FOCUS_1_0_AWS";

/// The superseded preview table, which publishes the same FOCUS 1.0 specification.
const FOCUS_1_0_PREVIEW: &str = "FOCUS_1_0_AWS_PREVIEW";

/// The table the FOCUS 1.2 specification is published for.
const FOCUS_1_2: &str = "FOCUS_1_2_AWS";

/// The curated view over the cost and usage data.
const DASHBOARD: &str = "COST_AND_USAGE_DASHBOARD";

/// The table the view above aggregates.
const COST_AND_USAGE_REPORT: &str = "COST_AND_USAGE_REPORT";

/// The table the cost optimization recommendations are published for.
const RECOMMENDATIONS: &str = "COST_OPTIMIZATION_RECOMMENDATIONS";

/// The table the carbon emissions estimates are published for.
const CARBON_EMISSIONS: &str = "CARBON_EMISSIONS";

/// `(column, the FOCUS 1.0 text, the FOCUS 1.2 text)` for every column the 1.2
/// specification re-words over 1.0 (AWS Data Exports table dictionary, as recorded for
/// `GetTable` on each table). The rows span the three families the revision touches: a
/// period's limits become its BOUNDS, the commitment-discount family drops "-based", and
/// the charge classifications and the effective cost are re-defined.
const REVISED_BY_FOCUS_1_2: &[(&str, &str, &str)] = &[
    (
        "BillingPeriodEnd",
        "The exclusive end date and time of a billing period.",
        "The exclusive end bound of a billing period.",
    ),
    (
        "BillingPeriodStart",
        "The inclusive start date and time of a billing period.",
        "The inclusive start bound of a billing period.",
    ),
    (
        "ChargePeriodEnd",
        "The exclusive end date and time of a charge period.",
        "The exclusive end bound of a charge period.",
    ),
    (
        "ChargePeriodStart",
        "The inclusive start date and time of a charge period.",
        "The inclusive start bound of a charge period.",
    ),
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
    (
        "CommitmentDiscountCategory",
        "Indicates whether the commitment-based discount identified in the CommitmentDiscountId column is based on usage quantity or cost (aka \"spend\").",
        "Indicates whether the commitment discount identified in the CommitmentDiscountId column is based on usage quantity or cost (aka \"spend\").",
    ),
    (
        "CommitmentDiscountId",
        "The identifier assigned to a commitment-based discount by the provider.",
        "The identifier assigned to a commitment discount by the provider.",
    ),
    (
        "CommitmentDiscountName",
        "The display name assigned to a commitment-based discount.",
        "The display name assigned to a commitment discount.",
    ),
    (
        "CommitmentDiscountStatus",
        "Indicates whether the charge corresponds with the consumption of a commitment-based discount or the unused portion of the committed amount.",
        "Indicates whether the charge corresponds with the consumption of a commitment discount or the unused portion of the committed amount.",
    ),
    (
        "CommitmentDiscountType",
        "A provider-assigned identifier for the type of commitment-based discount applied to the row.",
        "A provider-assigned identifier for the type of commitment discount applied to the row.",
    ),
    (
        "ContractedUnitPrice",
        "The agreed-upon unit price for a single Pricing Unit of the associated SKU, inclusive of negotiated discounts, if present, while excluding negotiated commitment-based discounts or any other discounts.",
        "The agreed-upon unit price for a single Pricing Unit of the associated SKU, inclusive of negotiated discounts, if present, while excluding negotiated commitment discounts or any other discounts.",
    ),
    (
        "EffectiveCost",
        "Cost inclusive of the impacts of all reduced rates and discounts, augmented with the amortization of relevant purchases (one-time or recurring) paid to cover future eligible charges.",
        "The amortized cost of the charge after applying all reduced rates, discounts, and the applicable portion of relevant, prepaid purchases (one-time or recurring) that covered this charge.",
    ),
];

/// The columns the cost and usage dashboard carries over from the cost and usage table
/// under that table's own name (AWS Data Exports table dictionary): a column the view
/// publishes and the catalog used to omit, so a client could not discover or request it.
const VIEWED_BY_THE_DASHBOARD: &[&str] = &[
    "pricing_unit",
    "product_from_location",
    "product_to_location",
];

/// `(the view's aggregate column, the cost and usage column it aggregates)`: a column the
/// view RENAMES is its own, so it is described in the view's words rather than reported
/// as the viewed column is (AWS Data Exports table dictionary).
const RENAMED_BY_THE_DASHBOARD: &[(&str, &str)] = &[
    ("unblended_cost", "line_item_unblended_cost"),
    ("usage_quantity", "line_item_usage_amount"),
    ("item_description", "line_item_line_item_description"),
    ("usage_type", "line_item_usage_type"),
];

/// `(table, column, the Description the dictionary publishes for it)` — rows spanning
/// every table of the catalog and the column families the issue cited, plus columns it did
/// not cite (AWS Data Exports table dictionary, as recorded for `GetTable`).
const PUBLISHED_DESCRIPTIONS: &[(&str, &str, &str)] = &[
    // The carbon emissions table: the product measured and the place the usage came from,
    // published as a Region code and as the human-readable location that code stands for.
    (
        CARBON_EMISSIONS,
        "product_code",
        "The code of the product measured. For example, Amazon EC2 is the product code for Amazon Elastic Compute Cloud.",
    ),
    (
        CARBON_EMISSIONS,
        "region_code",
        "Describes the Region code where the usage originated from.",
    ),
    (
        CARBON_EMISSIONS,
        "location",
        "Describes the human-readable location where the usage originated from, corresponding to the region_code.",
    ),
    // The cost and usage table: the unblended pair, worded as the dictionary words the
    // blended pair, and the usage amount with the caveats the dictionary ships with it.
    (
        COST_AND_USAGE_REPORT,
        "line_item_unblended_cost",
        "The UnblendedCost is the UnblendedRate multiplied by the UsageAmount.",
    ),
    (
        COST_AND_USAGE_REPORT,
        "line_item_unblended_rate",
        "In consolidated billing for accounts using Reserved Instances, the UnblendedRate is the rate associated with an individual account's service usage.",
    ),
    (
        COST_AND_USAGE_REPORT,
        "line_item_blended_cost",
        "The BlendedRate multiplied by the UsageAmount.",
    ),
    (
        COST_AND_USAGE_REPORT,
        "line_item_usage_amount",
        "The amount of usage that you incurred during the specified time period. For size-flexible Reserved Instances, use the reservation_total_reserved_units column instead. Certain subscription charges will have a UsageAmount of 0.",
    ),
    // The recommendations table: both cost columns report the CURRENT resource's cost.
    (
        RECOMMENDATIONS,
        "estimated_monthly_cost_before_discount",
        "The estimated monthly cost of the current resource before discounts. For Reserved Instances and Savings Plans, it refers to the cost for eligible usage.",
    ),
    (
        RECOMMENDATIONS,
        "estimated_monthly_cost_after_discount",
        "The estimated monthly cost of the current resource after discounts. For Reserved Instances and Savings Plans, it refers to the cost for eligible usage.",
    ),
    // The view, which reports a carried-over column as the viewed table does.
    (
        DASHBOARD,
        "pricing_unit",
        "The pricing unit that AWS used for calculating your usage cost. For example, the pricing unit for Amazon EC2 instance usage is in hours.",
    ),
    (
        DASHBOARD,
        "product_from_location",
        "Describes the location where the usage originated from.",
    ),
    // The view words a commitment as a "Reservation", with the "possible values are …"
    // form it uses for its enumerated dimensions.
    (
        DASHBOARD,
        "purchase_option",
        "The purchase option for this line item possible values are SavingsPLan, Reserved, Spot, and OnDemand.",
    ),
    (
        DASHBOARD,
        "ri_sp_arn",
        "The Amazon Resource Name (ARN) of the Reservation or Savings Plan that this line item benefited from.",
    ),
    // The FOCUS schemas, each at its own version's text.
    (
        FOCUS_1_0,
        "EffectiveCost",
        "Cost inclusive of the impacts of all reduced rates and discounts, augmented with the amortization of relevant purchases (one-time or recurring) paid to cover future eligible charges.",
    ),
    (
        FOCUS_1_0_PREVIEW,
        "BillingPeriodEnd",
        "The exclusive end date and time of a billing period.",
    ),
    (
        FOCUS_1_2,
        "CommitmentDiscountQuantity",
        "The amount of a commitment discount purchased or accounted for in commitment discount related rows.",
    ),
    (
        FOCUS_1_2,
        "CommitmentDiscountUnit",
        "The provider-specified measurement unit indicating the quantity of units of a commitment discount purchased or accounted for in commitment discount related rows.",
    ),
];

/// The recommendations table's estimated monthly COST columns, which report the current
/// resource's cost, and its estimated monthly SAVINGS columns, which report what the
/// recommendation would save: the invariant rule 3 pins over the pair.
const RECOMMENDATION_COST_COLUMNS: &[&str] = &[
    "estimated_monthly_cost_before_discount",
    "estimated_monthly_cost_after_discount",
];

/// The savings counterparts of [`RECOMMENDATION_COST_COLUMNS`].
const RECOMMENDATION_SAVINGS_COLUMNS: &[&str] = &[
    "estimated_monthly_savings_before_discount",
    "estimated_monthly_savings_after_discount",
];

/// One reported column.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedColumn {
    data_type: String,
    description: String,
}

/// The schema `GetTable` reports for `table`, as `name -> (Type, Description)`.
async fn schema_of(client: &Client, table: &str) -> BTreeMap<String, ReportedColumn> {
    let body = client.ok("GetTable", &json!({ "TableName": table })).await;
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
                .unwrap_or_else(|| panic!("case {table:?}: {name} without a Type: {column}"))
                .to_string();
            let description = column
                .get("Description")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {table:?}: {name} without a Description: {column}"))
                .to_string();
            (
                name,
                ReportedColumn {
                    data_type,
                    description,
                },
            )
        })
        .collect()
}

#[tokio::test]
async fn each_focus_version_reports_its_own_specifications_text() {
    let client = Client::new();

    let focus_1_0 = schema_of(&client, FOCUS_1_0).await;
    let preview = schema_of(&client, FOCUS_1_0_PREVIEW).await;
    let focus_1_2 = schema_of(&client, FOCUS_1_2).await;

    for (column, text_1_0, text_1_2) in REVISED_BY_FOCUS_1_2 {
        assert_ne!(
            text_1_0, text_1_2,
            "case {column:?}: the two specifications word this column differently, which is what this rule is about"
        );
        for (schema, table, expected, other) in [
            (&focus_1_0, FOCUS_1_0, text_1_0, text_1_2),
            (&preview, FOCUS_1_0_PREVIEW, text_1_0, text_1_2),
            (&focus_1_2, FOCUS_1_2, text_1_2, text_1_0),
        ] {
            let case = format!("{table}.{column}");
            let reported = schema
                .get(*column)
                .unwrap_or_else(|| panic!("case {case:?}: the schema must publish the column"));
            assert_eq!(
                reported.description, *expected,
                "case {case:?}: expected this version's own published text {expected:?}, got {:?}",
                reported.description
            );
            assert_ne!(
                reported.description, *other,
                "case {case:?}: the other version's text must not leak into this one"
            );
        }
    }

    // Over the WHOLE shared column set: a column both versions publish keeps one `Type`
    // across them, because a re-wording is not a change of the kind of data stored …
    let shared: BTreeSet<&str> = focus_1_0
        .keys()
        .map(String::as_str)
        .filter(|name| focus_1_2.contains_key(*name))
        .collect();
    assert!(
        shared.len() >= REVISED_BY_FOCUS_1_2.len(),
        "case \"FOCUS versions\": the two versions share the specification's columns, got {} shared",
        shared.len()
    );
    for name in &shared {
        let case = format!("FOCUS.{name}");
        assert_eq!(
            focus_1_0[*name].data_type, focus_1_2[*name].data_type,
            "case {case:?}: a re-worded column keeps its kind of data, got {:?} and {:?}",
            focus_1_0[*name].data_type, focus_1_2[*name].data_type
        );
    }

    // … and the two entries are not ONE entry serving two versions: the columns whose
    // text the 1.2 specification revises really do read differently across them.
    let differing: Vec<&str> = shared
        .iter()
        .copied()
        .filter(|name| focus_1_0[*name].description != focus_1_2[*name].description)
        .collect();
    assert_eq!(
        differing.len(),
        REVISED_BY_FOCUS_1_2.len(),
        "case \"FOCUS versions\": expected exactly the revised columns to differ, got {differing:?}"
    );

    // The preview table publishes the 1.0 specification byte for byte: it is the same
    // specification offered for evaluation, not a third version.
    assert_eq!(
        preview, focus_1_0,
        "case \"FOCUS_1_0_AWS_PREVIEW\": the preview table publishes the FOCUS 1.0 schema"
    );
}

#[tokio::test]
async fn a_view_publishes_every_column_the_dictionary_lists_for_it() {
    let client = Client::new();

    let dashboard = schema_of(&client, DASHBOARD).await;
    let viewed = schema_of(&client, COST_AND_USAGE_REPORT).await;

    // A column the view carries over is published, and reported exactly as the viewed
    // table reports it: one column, one kind of data, one description.
    for name in VIEWED_BY_THE_DASHBOARD {
        let case = format!("{DASHBOARD}.{name}");
        let reported = dashboard.get(*name).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the view publishes this column, got {:?}",
                dashboard.keys().collect::<Vec<_>>()
            )
        });
        let source = viewed
            .get(*name)
            .unwrap_or_else(|| panic!("case {case:?}: the viewed table must publish the column"));
        assert_eq!(
            reported, source,
            "case {case:?}: expected the viewed table's own column {source:?}, got {reported:?}"
        );
    }

    // A column the view RENAMES into an aggregate of its own is the view's column, so it
    // is described in the view's words rather than copied from the column it aggregates.
    for (aggregate, aggregated) in RENAMED_BY_THE_DASHBOARD {
        let case = format!("{DASHBOARD}.{aggregate}");
        let reported = dashboard
            .get(*aggregate)
            .unwrap_or_else(|| panic!("case {case:?}: the view publishes this column"));
        assert!(
            !dashboard.contains_key(*aggregated),
            "case {case:?}: the view publishes the aggregate, not the {aggregated} column it aggregates"
        );
        let source = viewed
            .get(*aggregated)
            .unwrap_or_else(|| panic!("case {case:?}: the viewed table publishes {aggregated}"));
        assert!(
            !reported.description.is_empty(),
            "case {case:?}: a published column carries the dictionary's description"
        );
        assert_eq!(
            reported.data_type, source.data_type,
            "case {case:?}: an aggregate stores the same kind of data as what it aggregates, got {:?} and {:?}",
            reported.data_type, source.data_type
        );
    }
}

#[tokio::test]
async fn every_cited_column_family_reports_the_dictionarys_text() {
    let client = Client::new();

    let mut schemas: BTreeMap<&str, BTreeMap<String, ReportedColumn>> = BTreeMap::new();
    for (table, column, published) in PUBLISHED_DESCRIPTIONS {
        if !schemas.contains_key(table) {
            schemas.insert(table, schema_of(&client, table).await);
        }
        let case = format!("{table}.{column}");
        let reported = schemas[table]
            .get(*column)
            .unwrap_or_else(|| panic!("case {case:?}: the table must publish the column"));
        assert_eq!(
            reported.description, *published,
            "case {case:?}: expected the dictionary's published description {published:?}, got {:?}",
            reported.description
        );
    }

    // The invariant behind one of those rows: the recommendations table's estimated
    // monthly COST columns are the CURRENT resource's cost — before and after discounts
    // alike — and it is the SAVINGS columns that report what the recommendation changes.
    // A cost column attributed to the recommended resource inverts what it means.
    let recommendations = schema_of(&client, RECOMMENDATIONS).await;
    for name in RECOMMENDATION_COST_COLUMNS {
        let case = format!("{RECOMMENDATIONS}.{name}");
        let reported = &recommendations
            .get(*name)
            .unwrap_or_else(|| panic!("case {case:?}: the table must publish the column"))
            .description;
        assert!(
            reported.contains("current resource"),
            "case {case:?}: an estimated monthly cost is the current resource's, got {reported:?}"
        );
        assert!(
            !reported.contains("recommended resource"),
            "case {case:?}: an estimated monthly cost is not the recommended resource's, got {reported:?}"
        );
    }
    for name in RECOMMENDATION_SAVINGS_COLUMNS {
        let case = format!("{RECOMMENDATIONS}.{name}");
        let reported = &recommendations
            .get(*name)
            .unwrap_or_else(|| panic!("case {case:?}: the table must publish the column"))
            .description;
        assert!(
            reported.contains("recommendation"),
            "case {case:?}: an estimated monthly saving is the recommendation's, got {reported:?}"
        );
    }

    // And the invariant behind the view's commitment columns, over EVERY column of both
    // schemas: the dashboard dictionary calls a commitment a "Reservation", the cost and
    // usage dictionary calls it a "Reserved Instance", and neither word is generalised
    // onto the other schema.
    let dashboard = schema_of(&client, DASHBOARD).await;
    let viewed = schema_of(&client, COST_AND_USAGE_REPORT).await;
    let mut dashboard_mentions = 0usize;
    for (name, column) in &dashboard {
        let case = format!("{DASHBOARD}.{name}");
        if column.description.contains("Reservation") {
            dashboard_mentions += 1;
        }
        assert!(
            !column.description.contains("Reserved Instance"),
            "case {case:?}: this schema words a commitment as a Reservation, got {:?}",
            column.description
        );
    }
    assert!(
        dashboard_mentions > 0,
        "case {DASHBOARD:?}: the view publishes columns about a commitment, so the wording above is exercised"
    );
    let viewed_mentions = viewed
        .values()
        .filter(|column| column.description.contains("Reserved Instance"))
        .count();
    assert!(
        viewed_mentions > 0,
        "case {COST_AND_USAGE_REPORT:?}: the cost and usage dictionary keeps its own \"Reserved Instance\" wording"
    );
}
