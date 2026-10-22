// SPDX-License-Identifier: CC0-1.0

//! The data-export TABLE CATALOG: every table data exports publishes, the table
//! properties each one accepts, and the columns each one's schema carries.
//!
//! `GetTable` reports one entry of this catalog and `ListTables` lists the same entries,
//! so both answer out of the one data source here: a table the listing advertises is a
//! table the reader describes, with the same description and the same table properties.
//!
//! SOURCE of every table name, table description, column name, column type, column
//! description, property name, valid-value list and default below: the AWS Data Exports
//! table dictionary, which `ListTables` mirrors and `GetTable` reports ("To view the
//! table properties available for each table, see the Data Exports table dictionary or
//! use the ListTables API to get a response of all tables and their available
//! properties" — the modelled doc for `CreateExport`'s `Export`, quoted in
//! `src/ops/create_export.rs`). It is service data, not account data: see `super`'s
//! header for why it is a static rather than a field of the state struct.
//!
//! The column TYPE vocabulary is the dictionary's: a column's `Type` names the KIND of
//! data the column stores ("The kind of data a column stores" — smithy_query shape
//! `Column`, member `Type`) in the dictionary's own mixed-case words — `String`,
//! `Number`, `Double`, `Integer`, `Timestamp`, `Boolean`, `Map` — never a storage-engine
//! type name such as `STRING` or `MAP<STRING,STRING>`, which the catalog does not
//! publish. The vocabulary is PER SCHEMA, not per kind of value: the cost and usage
//! dictionary measures costs with `Number` while the recommendations dictionary measures
//! them with `Double` and counts days with `Integer`, so each table below carries the
//! token ITS dictionary publishes rather than one token chosen for every numeric column
//! of the catalog.

use std::collections::{BTreeMap, HashMap};

use super::clip_for_message;
use crate::ops::types::{Column, ValidationException, ValidationExceptionField};

/// A column's `Type` for text the dictionary does not further constrain.
const STRING: &str = "String";
/// A column's `Type` for a numeric measure (a cost, a rate, a quantity, a count) in the
/// cost and usage dictionary.
const NUMBER: &str = "Number";
/// A column's `Type` for a floating-point measure (a cost, a percentage) in the
/// recommendations dictionary, which publishes its numeric columns as `Double`.
const DOUBLE: &str = "Double";
/// A column's `Type` for a whole-number count (a number of days) in the recommendations
/// dictionary.
const INTEGER: &str = "Integer";
/// A column's `Type` for a date and time.
const TIMESTAMP: &str = "Timestamp";
/// A column's `Type` for a true/false flag.
const BOOLEAN: &str = "Boolean";
/// A column's `Type` for the key-value columns that carry tags, product attributes,
/// cost categories and discounts.
const MAP: &str = "Map";

/// The value a boolean-valued table property takes to switch its data or columns ON.
pub const PROPERTY_TRUE: &str = "TRUE";
/// The value a boolean-valued table property takes to switch its data or columns OFF.
pub const PROPERTY_FALSE: &str = "FALSE";

/// The valid values of a boolean-valued table property, in the order the catalog
/// publishes them.
const BOOLEAN_PROPERTY_VALUES: &[&str] = &[PROPERTY_TRUE, PROPERTY_FALSE];

/// The valid values of a FREE-FORM table property: none, because the property's value
/// is not drawn from a published list but written by the caller (an ARN, a filter
/// expression). Such a property accepts any value — see [`TablePropertySpec::accepts`].
const FREE_FORM_PROPERTY_VALUES: &[&str] = &[];

/// Table property: the time granularity of each line item in a cost and usage table.
pub const PROPERTY_TIME_GRANULARITY: &str = "TIME_GRANULARITY";
/// Table property: whether the table carries the capacity reservation data.
pub const PROPERTY_INCLUDE_CAPACITY_RESERVATION_DATA: &str = "INCLUDE_CAPACITY_RESERVATION_DATA";
/// Table property: the billing view the table's data is scoped to.
pub const PROPERTY_BILLING_VIEW_ARN: &str = "BILLING_VIEW_ARN";
/// Table property: whether the schema carries the per-resource columns.
pub const PROPERTY_INCLUDE_RESOURCES: &str = "INCLUDE_RESOURCES";
/// Table property: whether the schema carries the split-cost-allocation columns.
pub const PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA: &str =
    "INCLUDE_SPLIT_COST_ALLOCATION_DATA";
/// Table property: whether the table's discount data stays compatible with the manual
/// discount columns of the legacy Cost and Usage Report.
pub const PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY: &str =
    "INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY";
/// Table property: whether the table carries the IAM principal columns.
pub const PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA: &str = "INCLUDE_IAM_PRINCIPAL_DATA";
/// Table property: whether the table is deprecated — published by a preview table the
/// catalog has superseded.
pub const PROPERTY_DEPRECATED: &str = "DEPRECATED";
/// Table property: the filter the recommendations table applies to its rows.
pub const PROPERTY_FILTER: &str = "FILTER";
/// Table property: whether the recommendations table reports every recommendation rather
/// than only the top one per resource.
pub const PROPERTY_INCLUDE_ALL_RECOMMENDATIONS: &str = "INCLUDE_ALL_RECOMMENDATIONS";

/// One table property of one table: its name, the values it accepts, and the value a
/// request that does not name it assumes.
///
/// Mirrors the modelled `TablePropertyDescription` (`Name`, `ValidValues`,
/// `DefaultValue`, `Description`), which is how `ListTables` reports the same entry.
#[derive(Debug, Clone, Copy)]
pub struct TablePropertySpec {
    /// The property name a request's `TableProperties` key must match.
    pub name: &'static str,
    /// The values this property accepts; anything else is refused. EMPTY for a free-form
    /// property, which accepts any value — see [`Self::accepts`].
    pub valid_values: &'static [&'static str],
    /// The value the property takes when a request does not name it, or `None` for a
    /// property the catalog publishes NO default for: such a property is in force only
    /// for a request that names it, so a request that does not name it reports no value
    /// for it at all.
    pub default_value: Option<&'static str>,
    /// What the property changes about the table's data or schema.
    pub description: &'static str,
}

impl TablePropertySpec {
    /// Whether this property accepts `value`: one of the values the catalog publishes
    /// for it, or — for a free-form property, whose value the caller writes rather than
    /// picks — any value at all.
    pub fn accepts(&self, value: &str) -> bool {
        self.valid_values.is_empty() || self.valid_values.contains(&value)
    }
}

/// One column of one table's schema.
#[derive(Debug, Clone, Copy)]
pub struct ColumnSpec {
    /// The column name.
    pub name: &'static str,
    /// The kind of data the column stores, as the dictionary publishes it: one of
    /// [`STRING`], [`NUMBER`], [`TIMESTAMP`], [`BOOLEAN`], [`MAP`].
    pub data_type: &'static str,
    /// What the column holds, as the dictionary documents it — the modelled
    /// `Column$Description` ("The description for a column").
    pub description: &'static str,
    /// The `(property, value)` pair that has to hold for this column to be part of the
    /// schema, or `None` for a column every schema of this table carries. This is what
    /// makes a table property "change the data and schema of a table" (the modelled doc
    /// for `GetTableRequest$TableProperties`).
    pub requires: Option<(&'static str, &'static str)>,
}

/// A column present in every schema of its table.
const fn col(
    name: &'static str,
    data_type: &'static str,
    description: &'static str,
) -> ColumnSpec {
    ColumnSpec {
        name,
        data_type,
        description,
        requires: None,
    }
}

/// A column present only while `property` holds `value`.
const fn col_when(
    name: &'static str,
    data_type: &'static str,
    description: &'static str,
    property: &'static str,
    value: &'static str,
) -> ColumnSpec {
    ColumnSpec {
        name,
        data_type,
        description,
        requires: Some((property, value)),
    }
}

/// One group of columns a table's schema is built from.
#[derive(Debug, Clone, Copy)]
pub enum ColumnGroup {
    /// Columns this schema describes itself.
    Own(&'static [ColumnSpec]),
    /// Columns this schema takes OVER from another table's group, named in the order this
    /// schema publishes them: a table that is a VIEW over another reports the viewed
    /// column exactly as the viewed table does — one name, one kind of data, one
    /// description — so the two readers cannot drift apart. A table property of the
    /// viewed table gates nothing here, because the viewing table publishes no such
    /// property: a viewed column is part of every schema of the viewing table.
    View {
        /// The group the columns are taken from.
        of: &'static [ColumnSpec],
        /// The names taken from it, in the order this schema publishes them. A name the
        /// group does not describe contributes no column — [`crate::state`]'s tests pin
        /// the resulting schema, so a name that resolves to nothing fails them.
        columns: &'static [&'static str],
    },
}

impl ColumnGroup {
    /// This group's columns, in the order it publishes them.
    fn resolve(&self) -> Vec<ColumnSpec> {
        match *self {
            Self::Own(columns) => columns.to_vec(),
            Self::View { of, columns } => columns
                .iter()
                .filter_map(|name| of.iter().find(|column| column.name == *name))
                .map(|column| ColumnSpec {
                    requires: None,
                    ..*column
                })
                .collect(),
        }
    }
}

/// One table in the catalog: the name a request names it by, its description, the table
/// properties it publishes, and the column groups its schema is built from.
#[derive(Debug, Clone, Copy)]
pub struct TableSpec {
    /// The table name, matched exactly: the catalog publishes each one as a fixed
    /// identifier, so a name differing in case or spelling names no table.
    pub name: &'static str,
    /// What the table holds, as the dictionary labels it.
    pub description: &'static str,
    /// The table properties this table publishes, in the order the catalog publishes
    /// them — which is the order `ListTables` reports them in.
    pub properties: &'static [TablePropertySpec],
    /// The column groups this table's schema is built from, concatenated in order: a
    /// schema that extends another's (FOCUS 1.2 over FOCUS 1.0) names the shared group
    /// rather than copying it, so one column is described once.
    pub column_groups: &'static [ColumnGroup],
}

impl TableSpec {
    /// This table's property called `name`, if it publishes one.
    pub fn property(&self, name: &str) -> Option<&'static TablePropertySpec> {
        self.properties.iter().find(|property| property.name == name)
    }

    /// Every column this table can carry, including the ones a table property gates, in
    /// the order the catalog publishes them.
    pub fn columns(&self) -> impl Iterator<Item = ColumnSpec> {
        let groups: &'static [ColumnGroup] = self.column_groups;
        groups.iter().flat_map(ColumnGroup::resolve)
    }

    /// The properties in force for a request carrying `requested`: every property this
    /// table publishes, at the requested value where the request names it and at the
    /// published default otherwise ("Each table property has a default value that it
    /// assumes if not specified" — the modelled doc for
    /// `GetTableRequest$TableProperties`). A property the catalog publishes NO default
    /// for assumes nothing, so it is in force only for a request that names it.
    ///
    /// `Err` is the modelled 400 for a property this table does not publish, or a value
    /// outside the property's valid values: neither names a configuration of this table,
    /// so there is no schema to report for it.
    pub fn effective_properties(
        &self,
        requested: Option<&HashMap<String, String>>,
    ) -> Result<BTreeMap<String, String>, ValidationException> {
        let mut effective: BTreeMap<String, String> = self
            .properties
            .iter()
            .filter_map(|property| {
                property
                    .default_value
                    .map(|default| (property.name.to_string(), default.to_string()))
            })
            .collect();
        for (name, value) in requested.into_iter().flatten() {
            let property = self
                .property(name)
                .ok_or_else(|| unknown_table_property(self, name))?;
            if !property.accepts(value) {
                return Err(invalid_table_property_value(property, value));
            }
            effective.insert(name.clone(), value.clone());
        }
        Ok(effective)
    }

    /// This table's schema under `properties`: its columns in catalog order, less the
    /// ones whose gating property does not hold the value they need.
    pub fn schema(&self, properties: &BTreeMap<String, String>) -> Vec<Column> {
        self.columns()
            .filter(|column| match column.requires {
                None => true,
                Some((property, value)) => {
                    properties.get(property).map(String::as_str) == Some(value)
                }
            })
            .map(|column| Column {
                name: Some(column.name.to_string()),
                r#type: Some(column.data_type.to_string()),
                description: Some(column.description.to_string()),
            })
            .collect()
    }
}

/// The table in the catalog called `name`, if there is one. Matched exactly: see
/// [`TableSpec::name`].
pub fn table_spec(name: &str) -> Option<&'static TableSpec> {
    TABLE_CATALOG.iter().find(|table| table.name == name)
}

/// The modelled 400 for a `TableName` the catalog does not publish.
pub fn unknown_table(name: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Table {} is not supported. Call ListTables for the tables available in data exports.",
            clip_for_message(name)
        ),
        reason: Some(super::VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "TableName".to_string(),
            message: "This table is not available in data exports.".to_string(),
        }]),
    }
}

/// The modelled 400 for a `TableProperties` key `table` does not publish.
fn unknown_table_property(table: &TableSpec, property: &str) -> ValidationException {
    let supported = table
        .properties
        .iter()
        .map(|property| property.name)
        .collect::<Vec<_>>()
        .join(", ");
    ValidationException {
        message: format!(
            "Table property {} is not supported for table {}. Supported table properties: [{supported}].",
            clip_for_message(property),
            table.name
        ),
        reason: Some(super::VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "TableProperties".to_string(),
            message: format!("This table supports the table properties [{supported}]."),
        }]),
    }
}

/// The modelled 400 for a `TableProperties` value outside the property's valid values.
/// Only an enumerated property refuses a value: a free-form one accepts any.
fn invalid_table_property_value(
    property: &TablePropertySpec,
    value: &str,
) -> ValidationException {
    let valid = property.valid_values.join(", ");
    ValidationException {
        message: format!(
            "Value {} is not supported for table property {}. Valid values: [{valid}].",
            clip_for_message(value),
            property.name
        ),
        reason: Some(super::VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "TableProperties".to_string(),
            message: format!("{} accepts the values [{valid}].", property.name),
        }]),
    }
}

// ---------------------------------------------------------------------------
// Table properties
// ---------------------------------------------------------------------------

/// The table properties of the cost and usage tables, in the order the catalog
/// publishes them.
///
/// Not every property gates a column: INCLUDE_CAPACITY_RESERVATION_DATA,
/// INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY and BILLING_VIEW_ARN change the DATA the table
/// reports (which reservations are covered, how discounts are expressed, which billing
/// view the data is scoped to) rather than the set of columns, and the recorded AWS
/// schema carries the same columns at either value of each. INCLUDE_RESOURCES and
/// INCLUDE_SPLIT_COST_ALLOCATION_DATA do gate columns, and TIME_GRANULARITY changes the
/// granularity of the rows.
const COST_AND_USAGE_REPORT_PROPERTIES: &[TablePropertySpec] = &[
    TIME_GRANULARITY_PROPERTY,
    TablePropertySpec {
        name: PROPERTY_INCLUDE_CAPACITY_RESERVATION_DATA,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: Some(PROPERTY_FALSE),
        description: "Whether to include capacity reservation data.",
    },
    BILLING_VIEW_ARN_PROPERTY,
    TablePropertySpec {
        name: PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: Some(PROPERTY_FALSE),
        description: "Whether to simulate manual discounts for automated discount data.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: Some(PROPERTY_FALSE),
        description: "Whether to include fission columns.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        // The catalog publishes NO default for this property: it is in force only for a
        // request that names it, so a request that names none reports no value for it.
        default_value: None,
        description: "Whether IAM principal columns should be included or not.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_RESOURCES,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: Some(PROPERTY_FALSE),
        description: "Whether to include resource IDs on line items.",
    },
];

/// The table properties of `COST_OPTIMIZATION_RECOMMENDATIONS`, in the order the catalog
/// publishes them.
const COST_OPTIMIZATION_RECOMMENDATIONS_PROPERTIES: &[TablePropertySpec] = &[
    TablePropertySpec {
        name: PROPERTY_INCLUDE_ALL_RECOMMENDATIONS,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: Some(PROPERTY_FALSE),
        description: "List of all recommendations for a resource, or a single recommendation if de-duped by resourceId.",
    },
    TablePropertySpec {
        name: PROPERTY_FILTER,
        // Free-form: a JSON filter expression over the recommendation columns, which the
        // catalog cannot enumerate. The default is the empty filter: every row.
        valid_values: FREE_FORM_PROPERTY_VALUES,
        default_value: Some("{}"),
        description: "The filter that the rows in the table are restricted to.",
    },
];

/// The table properties of the general FOCUS tables.
const FOCUS_PROPERTIES: &[TablePropertySpec] = &[BILLING_VIEW_ARN_PROPERTY];

/// The table properties of the FOCUS PREVIEW table: a preview table publishes, beside
/// the billing view, that it is deprecated — which is how a client learns not to build
/// new exports on it.
const FOCUS_PREVIEW_PROPERTIES: &[TablePropertySpec] = &[
    TablePropertySpec {
        name: PROPERTY_DEPRECATED,
        // The only value this property takes for a preview table the catalog publishes as
        // deprecated: there is no configuration of it that is not deprecated.
        valid_values: &[PROPERTY_TRUE],
        default_value: Some(PROPERTY_TRUE),
        description: "Whether the table is deprecated.",
    },
    BILLING_VIEW_ARN_PROPERTY,
];

/// The table properties of the FOCUS 1.2 table, which — unlike FOCUS 1.0 — publishes the
/// granularity of its rows.
const FOCUS_1_2_PROPERTIES: &[TablePropertySpec] =
    &[TIME_GRANULARITY_PROPERTY, BILLING_VIEW_ARN_PROPERTY];

/// The table properties of the tables the catalog scopes by billing view alone.
const BILLING_VIEW_ONLY_PROPERTIES: &[TablePropertySpec] = &[BILLING_VIEW_ARN_PROPERTY];

/// The billing view property, which every table publishes: the data in any table can be
/// scoped to a billing view.
const BILLING_VIEW_ARN_PROPERTY: TablePropertySpec = TablePropertySpec {
    name: PROPERTY_BILLING_VIEW_ARN,
    // Free-form: the value is a billing view ARN the caller owns, not a value the
    // catalog can enumerate. The default is the empty string: no billing view, so the
    // table covers the whole of the account's data.
    valid_values: FREE_FORM_PROPERTY_VALUES,
    default_value: Some(""),
    description: "The arn of the billing conductor group that this CUR2.0/ FOCUS Data Export is for.",
};

/// The granularity property, published by every table whose rows are aggregated over a
/// period of time.
const TIME_GRANULARITY_PROPERTY: TablePropertySpec = TablePropertySpec {
    name: PROPERTY_TIME_GRANULARITY,
    valid_values: &["DAILY", "MONTHLY", "HOURLY"],
    default_value: Some("HOURLY"),
    description: "The granularity of the line-items in the table.",
};

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

/// The columns of `CARBON_EMISSIONS`, in the dictionary's order.
const CARBON_EMISSIONS_COLUMNS: &[ColumnSpec] = &[
    col(
        "payer_account_id",
        STRING,
        "The account ID of the paying account. For an organization in AWS Organizations, this is the account ID of the management account.",
    ),
    col(
        "usage_account_id",
        STRING,
        "The account ID of the account whose usage the emissions are estimated for.",
    ),
    col(
        "usage_period_start",
        TIMESTAMP,
        "The inclusive start of the usage period that the estimate covers, in UTC.",
    ),
    col(
        "usage_period_end",
        TIMESTAMP,
        "The exclusive end of the usage period that the estimate covers, in UTC.",
    ),
    col(
        "last_refresh_timestamp",
        TIMESTAMP,
        "The date and time that the emissions estimate was last refreshed, in UTC.",
    ),
    col(
        "model_version",
        STRING,
        "The version of the model that the emissions were estimated with.",
    ),
    col(
        "product_code",
        STRING,
        "The code of the Amazon Web Services service whose usage the emissions are estimated for.",
    ),
    col(
        "region_code",
        STRING,
        "The Region code of the usage that the emissions are estimated for.",
    ),
    col(
        "location",
        STRING,
        "The location of the usage that the emissions are estimated for.",
    ),
    // The emissions MEASURES. Each one is reported as a value and the unit that value is
    // measured in, rather than as a single column in an assumed unit: the table publishes
    // the market-based (mbm) and location-based (lbm) totals and, per emissions scope,
    // the scope's own total.
    col(
        "total_lbm_emissions_value",
        NUMBER,
        "The total estimated carbon emissions of the usage under the location-based method.",
    ),
    col(
        "total_lbm_emissions_unit",
        STRING,
        "The unit that the total location-based-method emissions are measured in.",
    ),
    col(
        "total_mbm_emissions_value",
        NUMBER,
        "The total estimated carbon emissions of the usage under the market-based method.",
    ),
    col(
        "total_mbm_emissions_unit",
        STRING,
        "The unit that the total market-based-method emissions are measured in.",
    ),
    col(
        "total_scope_1_emissions_value",
        NUMBER,
        "The total estimated scope 1 carbon emissions of the usage.",
    ),
    col(
        "total_scope_1_emissions_unit",
        STRING,
        "The unit that the total scope 1 emissions are measured in.",
    ),
    col(
        "total_scope_2_lbm_emissions_value",
        NUMBER,
        "The total estimated scope 2 carbon emissions of the usage under the location-based method.",
    ),
    col(
        "total_scope_2_lbm_emissions_unit",
        STRING,
        "The unit that the total scope 2 location-based-method emissions are measured in.",
    ),
    col(
        "total_scope_2_mbm_emissions_value",
        NUMBER,
        "The total estimated scope 2 carbon emissions of the usage under the market-based method.",
    ),
    col(
        "total_scope_2_mbm_emissions_unit",
        STRING,
        "The unit that the total scope 2 market-based-method emissions are measured in.",
    ),
    col(
        "total_scope_3_lbm_emissions_value",
        NUMBER,
        "The total estimated scope 3 carbon emissions of the usage under the location-based method.",
    ),
    col(
        "total_scope_3_lbm_emissions_unit",
        STRING,
        "The unit that the total scope 3 location-based-method emissions are measured in.",
    ),
    col(
        "total_scope_3_mbm_emissions_value",
        NUMBER,
        "The total estimated scope 3 carbon emissions of the usage under the market-based method.",
    ),
    col(
        "total_scope_3_mbm_emissions_unit",
        STRING,
        "The unit that the total scope 3 market-based-method emissions are measured in.",
    ),
];

/// The columns of the cost and usage tables (Cost and Usage Report 2.0), in the
/// dictionary's order. The gated ones are exactly the columns a table property adds.
const COST_AND_USAGE_REPORT_COLUMNS: &[ColumnSpec] = &[
    col(
        "bill_bill_type",
        STRING,
        "The type of bill that this line item covers.",
    ),
    col(
        "bill_billing_entity",
        STRING,
        "The Amazon Web Services organization that the charge in this line item was made through, which identifies an AWS Marketplace purchase.",
    ),
    col(
        "bill_billing_period_end_date",
        TIMESTAMP,
        "The end date and time of the billing period that includes this line item, in UTC.",
    ),
    col(
        "bill_billing_period_start_date",
        TIMESTAMP,
        "The start date and time of the billing period that includes this line item, in UTC.",
    ),
    col(
        "bill_invoice_id",
        STRING,
        "The ID of the invoice that includes this line item.",
    ),
    col(
        "bill_invoicing_entity",
        STRING,
        "The Amazon Web Services entity that issues the invoice for this line item.",
    ),
    col(
        "bill_payer_account_id",
        STRING,
        "The account ID of the management account that pays for the usage in this line item.",
    ),
    col(
        "bill_payer_account_name",
        STRING,
        "The name of the management account that pays for the usage in this line item.",
    ),
    col(
        "cost_category",
        MAP,
        "The cost categories that apply to this line item, as key-value pairs.",
    ),
    col(
        "discount",
        MAP,
        "The discounts that apply to this line item, as key-value pairs.",
    ),
    col(
        "discount_bundled_discount",
        NUMBER,
        "The bundled discount applied to the usage in this line item.",
    ),
    col(
        "discount_total_discount",
        NUMBER,
        "The sum of the discounts applied to this line item.",
    ),
    col(
        "identity_line_item_id",
        STRING,
        "A unique identifier for every line item in a single delivery of the export.",
    ),
    col(
        "identity_time_interval",
        STRING,
        "The time interval that this line item applies to, in UTC.",
    ),
    col(
        "line_item_availability_zone",
        STRING,
        "The Availability Zone that hosts the usage in this line item.",
    ),
    col(
        "line_item_blended_cost",
        NUMBER,
        "The blended rate multiplied by the usage amount of this line item.",
    ),
    col(
        "line_item_blended_rate",
        STRING,
        "The average rate of the usage in this line item across the consolidated billing family.",
    ),
    col(
        "line_item_currency_code",
        STRING,
        "The currency that the cost of this line item is reported in.",
    ),
    col(
        "line_item_legal_entity",
        STRING,
        "The Amazon Web Services seller of record for the usage in this line item.",
    ),
    col(
        "line_item_line_item_description",
        STRING,
        "The description of the usage in this line item.",
    ),
    col(
        "line_item_line_item_type",
        STRING,
        "The type of charge that this line item covers, such as usage, tax, credit or refund.",
    ),
    col(
        "line_item_net_unblended_cost",
        NUMBER,
        "The unblended cost of this line item after all discounts.",
    ),
    col(
        "line_item_net_unblended_rate",
        STRING,
        "The unblended rate of this line item after all discounts.",
    ),
    col(
        "line_item_normalization_factor",
        NUMBER,
        "The normalization factor of the instance size that the usage in this line item ran on.",
    ),
    col(
        "line_item_normalized_usage_amount",
        NUMBER,
        "The usage amount of this line item normalized to the smallest size of its instance family.",
    ),
    col(
        "line_item_operation",
        STRING,
        "The specific operation covered by this line item.",
    ),
    col(
        "line_item_product_code",
        STRING,
        "The code of the Amazon Web Services service that the usage in this line item belongs to.",
    ),
    col_when(
        "line_item_resource_id",
        STRING,
        "The identifier of the resource that the usage in this line item ran on.",
        PROPERTY_INCLUDE_RESOURCES,
        PROPERTY_TRUE,
    ),
    col(
        "line_item_tax_type",
        STRING,
        "The type of tax that this line item charges.",
    ),
    col(
        "line_item_unblended_cost",
        NUMBER,
        "The unblended rate multiplied by the usage amount of this line item.",
    ),
    col(
        "line_item_unblended_rate",
        STRING,
        "The rate charged for the usage in this line item.",
    ),
    col(
        "line_item_usage_account_id",
        STRING,
        "The account ID of the account that used the resource in this line item.",
    ),
    col(
        "line_item_usage_account_name",
        STRING,
        "The name of the account that used the resource in this line item.",
    ),
    col(
        "line_item_usage_amount",
        NUMBER,
        "The amount of usage that this line item covers.",
    ),
    col(
        "line_item_usage_end_date",
        TIMESTAMP,
        "The exclusive end date and time of the usage in this line item, in UTC.",
    ),
    col(
        "line_item_usage_start_date",
        TIMESTAMP,
        "The inclusive start date and time of the usage in this line item, in UTC.",
    ),
    col(
        "line_item_usage_type",
        STRING,
        "The usage details of this line item.",
    ),
    col(
        "line_item_user_identifier",
        STRING,
        "The identifier of the identity that the usage in this line item is attributed to.",
    ),
    col(
        "pricing_currency",
        STRING,
        "The currency that the pricing of this line item is expressed in.",
    ),
    col(
        "pricing_lease_contract_length",
        STRING,
        "The length of the term of the Reserved Instance or Savings Plan that applies to this line item.",
    ),
    col(
        "pricing_offering_class",
        STRING,
        "The offering class of the Reserved Instance that applies to this line item.",
    ),
    col(
        "pricing_public_on_demand_cost",
        NUMBER,
        "The cost of the usage in this line item at public On-Demand rates.",
    ),
    col(
        "pricing_public_on_demand_rate",
        STRING,
        "The public On-Demand rate of the usage in this line item.",
    ),
    col(
        "pricing_purchase_option",
        STRING,
        "How the usage in this line item is paid for: all upfront, partial upfront or no upfront.",
    ),
    col(
        "pricing_rate_code",
        STRING,
        "The code of the rate that applies to this line item.",
    ),
    col(
        "pricing_rate_id",
        STRING,
        "The identifier of the rate that applies to this line item.",
    ),
    col(
        "pricing_term",
        STRING,
        "Whether the usage in this line item is reserved or On-Demand.",
    ),
    col(
        "pricing_unit",
        STRING,
        "The unit that Amazon Web Services prices the usage in this line item in.",
    ),
    col(
        "product",
        MAP,
        "The attributes of the product that the usage in this line item covers, as key-value pairs.",
    ),
    col(
        "product_comment",
        STRING,
        "A comment about the product that the usage in this line item covers.",
    ),
    col(
        "product_fee_code",
        STRING,
        "The fee code of the product that the usage in this line item covers.",
    ),
    col(
        "product_fee_description",
        STRING,
        "The description of the fee that this line item charges.",
    ),
    col(
        "product_from_location",
        STRING,
        "The location that the request in this line item came from.",
    ),
    col(
        "product_from_location_type",
        STRING,
        "The type of the location that the request in this line item came from.",
    ),
    col(
        "product_from_region_code",
        STRING,
        "The Region code that the request in this line item came from.",
    ),
    col(
        "product_instance_family",
        STRING,
        "The instance family of the instance that the usage in this line item ran on.",
    ),
    col(
        "product_instance_type",
        STRING,
        "The instance type of the instance that the usage in this line item ran on.",
    ),
    col(
        "product_instancesku",
        STRING,
        "The SKU of the instance that the usage in this line item ran on.",
    ),
    col(
        "product_location",
        STRING,
        "The location of the resource that the usage in this line item ran on.",
    ),
    col(
        "product_location_type",
        STRING,
        "The type of the location of the resource that the usage in this line item ran on.",
    ),
    col(
        "product_operation",
        STRING,
        "The operation that the product in this line item performed.",
    ),
    col(
        "product_pricing_unit",
        STRING,
        "The smallest billing unit that the product in this line item is priced in.",
    ),
    col(
        "product_product_family",
        STRING,
        "The family of the product that the usage in this line item covers.",
    ),
    col(
        "product_region_code",
        STRING,
        "The Region code of the resource that the usage in this line item ran in.",
    ),
    col(
        "product_servicecode",
        STRING,
        "The code of the Amazon Web Services service that the product in this line item belongs to.",
    ),
    col(
        "product_sku",
        STRING,
        "The SKU of the product that the usage in this line item covers.",
    ),
    col(
        "product_to_location",
        STRING,
        "The location that the request in this line item was sent to.",
    ),
    col(
        "product_to_location_type",
        STRING,
        "The type of the location that the request in this line item was sent to.",
    ),
    col(
        "product_to_region_code",
        STRING,
        "The Region code that the request in this line item was sent to.",
    ),
    col(
        "product_usagetype",
        STRING,
        "The usage type of the product that the usage in this line item covers.",
    ),
    col(
        "reservation_amortized_upfront_cost_for_usage",
        NUMBER,
        "The upfront payment for the Reserved Instance, amortized over the usage in this line item.",
    ),
    col(
        "reservation_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "The upfront fee of the Reserved Instance, amortized over the billing period.",
    ),
    col(
        "reservation_availability_zone",
        STRING,
        "The Availability Zone of the resource covered by the Reserved Instance.",
    ),
    col(
        "reservation_effective_cost",
        NUMBER,
        "The amortized upfront fee plus the recurring fee of the usage covered by the Reserved Instance.",
    ),
    col(
        "reservation_end_time",
        STRING,
        "The end date and time of the term of the Reserved Instance.",
    ),
    col(
        "reservation_modification_status",
        STRING,
        "Whether the Reserved Instance was modified, and how.",
    ),
    col(
        "reservation_net_amortized_upfront_cost_for_usage",
        NUMBER,
        "The amortized upfront cost of the usage covered by the Reserved Instance, after discounts.",
    ),
    col(
        "reservation_net_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "The amortized upfront fee of the Reserved Instance for the billing period, after discounts.",
    ),
    col(
        "reservation_net_effective_cost",
        NUMBER,
        "The effective cost of the usage covered by the Reserved Instance, after discounts.",
    ),
    col(
        "reservation_net_recurring_fee_for_usage",
        NUMBER,
        "The recurring fee of the usage covered by the Reserved Instance, after discounts.",
    ),
    col(
        "reservation_net_unused_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "The amortized upfront fee of the unused Reserved Instance capacity in the billing period, after discounts.",
    ),
    col(
        "reservation_net_unused_recurring_fee",
        NUMBER,
        "The recurring fee of the unused Reserved Instance capacity, after discounts.",
    ),
    col(
        "reservation_net_upfront_value",
        NUMBER,
        "The upfront value of the Reserved Instance, after discounts.",
    ),
    col(
        "reservation_normalized_units_per_reservation",
        STRING,
        "The number of normalized units of each reservation.",
    ),
    col(
        "reservation_number_of_reservations",
        STRING,
        "The number of reservations that this line item covers.",
    ),
    col(
        "reservation_recurring_fee_for_usage",
        NUMBER,
        "The recurring fee of the usage covered by the Reserved Instance.",
    ),
    col(
        "reservation_reservation_a_r_n",
        STRING,
        "The ARN of the Reserved Instance that applies to this line item.",
    ),
    col(
        "reservation_start_time",
        STRING,
        "The start date and time of the term of the Reserved Instance.",
    ),
    col(
        "reservation_subscription_id",
        STRING,
        "The identifier of the subscription that the Reserved Instance belongs to.",
    ),
    col(
        "reservation_total_reserved_normalized_units",
        STRING,
        "The total number of normalized units that the reservations cover.",
    ),
    col(
        "reservation_total_reserved_units",
        STRING,
        "The total number of units that the reservations cover.",
    ),
    col(
        "reservation_units_per_reservation",
        STRING,
        "The number of units of each reservation.",
    ),
    col(
        "reservation_unused_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "The amortized upfront fee of the unused Reserved Instance capacity in the billing period.",
    ),
    col(
        "reservation_unused_normalized_unit_quantity",
        NUMBER,
        "The number of unused normalized units of the Reserved Instance.",
    ),
    col(
        "reservation_unused_quantity",
        NUMBER,
        "The number of unused units of the Reserved Instance.",
    ),
    col(
        "reservation_unused_recurring_fee",
        NUMBER,
        "The recurring fee of the unused Reserved Instance capacity.",
    ),
    col(
        "reservation_upfront_value",
        NUMBER,
        "The upfront value of the Reserved Instance.",
    ),
    col(
        "resource_tags",
        MAP,
        "The tags of the resource that the usage in this line item ran on, as key-value pairs.",
    ),
    col(
        "savings_plan_amortized_upfront_commitment_for_billing_period",
        // The cost and usage dictionary publishes the Savings Plan commitment and rate as
        // TEXT, not as a numeric measure like the other cost columns of this schema.
        STRING,
        "The upfront payment for the Savings Plan, amortized over the billing period.",
    ),
    col(
        "savings_plan_end_time",
        STRING,
        "The end date and time of the term of the Savings Plan.",
    ),
    col(
        "savings_plan_instance_type_family",
        STRING,
        "The instance family that the Savings Plan applies to.",
    ),
    col(
        "savings_plan_net_amortized_upfront_commitment_for_billing_period",
        NUMBER,
        "The amortized upfront commitment of the Savings Plan for the billing period, after discounts.",
    ),
    col(
        "savings_plan_net_recurring_commitment_for_billing_period",
        NUMBER,
        "The recurring commitment of the Savings Plan for the billing period, after discounts.",
    ),
    col(
        "savings_plan_net_savings_plan_effective_cost",
        NUMBER,
        "The effective cost of the usage covered by the Savings Plan, after discounts.",
    ),
    col(
        "savings_plan_offering_type",
        STRING,
        "The type of the Savings Plan offering.",
    ),
    col(
        "savings_plan_payment_option",
        STRING,
        "How the Savings Plan is paid for: all upfront, partial upfront or no upfront.",
    ),
    col(
        "savings_plan_purchase_term",
        STRING,
        "The length of the term of the Savings Plan.",
    ),
    col(
        "savings_plan_recurring_commitment_for_billing_period",
        NUMBER,
        "The recurring commitment of the Savings Plan for the billing period.",
    ),
    col(
        "savings_plan_region",
        STRING,
        "The Region that the Savings Plan was purchased in.",
    ),
    col(
        "savings_plan_savings_plan_a_r_n",
        STRING,
        "The ARN of the Savings Plan that applies to this line item.",
    ),
    col(
        "savings_plan_savings_plan_effective_cost",
        NUMBER,
        "The effective cost of the usage covered by the Savings Plan.",
    ),
    col(
        "savings_plan_savings_plan_rate",
        STRING,
        "The Savings Plan rate of the usage in this line item.",
    ),
    col(
        "savings_plan_start_time",
        STRING,
        "The start date and time of the term of the Savings Plan.",
    ),
    col(
        "savings_plan_total_commitment_to_date",
        NUMBER,
        "The commitment of the Savings Plan used to date.",
    ),
    col(
        "savings_plan_used_commitment",
        NUMBER,
        "The portion of the Savings Plan commitment that this line item uses.",
    ),
    col_when(
        "split_line_item_actual_usage",
        NUMBER,
        "The usage of the shared resource that this line item is attributed.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_net_split_cost",
        NUMBER,
        "The split cost of the shared resource attributed to this line item, after discounts.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_net_unused_cost",
        NUMBER,
        "The cost of the unused capacity of the shared resource, after discounts.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_parent_resource_id",
        STRING,
        "The identifier of the shared resource whose cost this line item splits.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_public_on_demand_split_cost",
        NUMBER,
        "The split cost of the shared resource attributed to this line item at public On-Demand rates.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_public_on_demand_unused_cost",
        NUMBER,
        "The cost of the unused capacity of the shared resource at public On-Demand rates.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_reserved_usage",
        NUMBER,
        "The reserved usage of the shared resource that this line item is attributed.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_cost",
        NUMBER,
        "The cost of the shared resource attributed to this line item.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_usage",
        NUMBER,
        "The usage of the shared resource split to this line item.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_usage_ratio",
        NUMBER,
        "The ratio of the shared resource's usage that this line item is attributed.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_unused_cost",
        NUMBER,
        "The cost of the unused capacity of the shared resource.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col(
        "tags",
        MAP,
        "The user-defined tags of the resource that the usage in this line item ran on, as key-value pairs.",
    ),
];

/// The columns of `COST_OPTIMIZATION_RECOMMENDATIONS`, in the dictionary's order.
const COST_OPTIMIZATION_RECOMMENDATIONS_COLUMNS: &[ColumnSpec] = &[
    col(
        "account_id",
        STRING,
        "The account ID that the recommendation is for.",
    ),
    col(
        "account_name",
        STRING,
        "The account name that the recommendation is for.",
    ),
    col(
        "action_type",
        STRING,
        "The action to take to realize the recommendation.",
    ),
    col(
        "currency_code",
        STRING,
        "The currency that the estimated costs and savings are reported in.",
    ),
    col(
        "current_resource_details",
        STRING,
        "The configuration of the resource in its current state.",
    ),
    col(
        "current_resource_summary",
        STRING,
        "A summary of the resource in its current state.",
    ),
    col(
        "current_resource_type",
        STRING,
        "The type of the resource in its current state.",
    ),
    col(
        "estimated_monthly_cost_after_discount",
        DOUBLE,
        "The estimated monthly cost of the recommended resource, after discounts.",
    ),
    col(
        "estimated_monthly_cost_before_discount",
        DOUBLE,
        "The estimated monthly cost of the recommended resource, before discounts.",
    ),
    col(
        "estimated_monthly_savings_after_discount",
        DOUBLE,
        "The estimated monthly savings of the recommendation, after discounts.",
    ),
    col(
        "estimated_monthly_savings_before_discount",
        DOUBLE,
        "The estimated monthly savings of the recommendation, before discounts.",
    ),
    col(
        "estimated_savings_percentage_after_discount",
        DOUBLE,
        "The estimated savings of the recommendation as a percentage of the current cost, after discounts.",
    ),
    col(
        "estimated_savings_percentage_before_discount",
        DOUBLE,
        "The estimated savings of the recommendation as a percentage of the current cost, before discounts.",
    ),
    col(
        "implementation_effort",
        STRING,
        "The effort required to implement the recommendation.",
    ),
    col(
        "last_refresh_timestamp",
        TIMESTAMP,
        "The date and time that the recommendation was last refreshed, in UTC.",
    ),
    col("recommendation_id", STRING, "The ID for the recommendation."),
    col(
        "recommendation_lookback_period_in_days",
        INTEGER,
        "The number of days of usage that the recommendation was generated from.",
    ),
    col(
        "recommendation_source",
        STRING,
        "The source that generated the recommendation.",
    ),
    col(
        "recommended_resource_details",
        STRING,
        "The configuration of the resource that the recommendation proposes.",
    ),
    col(
        "recommended_resource_summary",
        STRING,
        "A summary of the resource that the recommendation proposes.",
    ),
    col(
        "recommended_resource_type",
        STRING,
        "The type of the resource that the recommendation proposes.",
    ),
    col(
        "region",
        STRING,
        "The Region of the resource that the recommendation is for.",
    ),
    col(
        "resource_arn",
        STRING,
        "The ARN of the resource that the recommendation is for.",
    ),
    col(
        "restart_needed",
        BOOLEAN,
        "Whether implementing the recommendation requires restarting the resource.",
    ),
    col(
        "rollback_possible",
        BOOLEAN,
        "Whether the recommendation can be rolled back once it is implemented.",
    ),
    col(
        "tags",
        // The recommendations dictionary publishes the resource's tags as TEXT: one
        // column carrying the tag set, not a key-value map column like the cost and
        // usage dictionary's.
        STRING,
        "The tags of the resource that the recommendation is for.",
    ),
];

/// The standard columns of the FOCUS 1.0 schema, in the dictionary's order. Shared by
/// every FOCUS table, because FOCUS 1.2 extends this schema rather than replacing it.
const FOCUS_1_0_COLUMNS: &[ColumnSpec] = &[
    col(
        "AvailabilityZone",
        STRING,
        "A provider-assigned identifier for a physically separated and isolated area within a region.",
    ),
    col(
        "BilledCost",
        NUMBER,
        "A charge serving as the basis for invoicing, inclusive of all reduced rates and discounts while excluding the amortization of upfront charges (one-time or recurring).",
    ),
    col(
        "BillingAccountId",
        STRING,
        "The identifier assigned to a billing account by the provider.",
    ),
    col(
        "BillingAccountName",
        STRING,
        "The display name assigned to a billing account.",
    ),
    col(
        "BillingCurrency",
        STRING,
        "The currency that a charge was billed in.",
    ),
    col(
        "BillingPeriodEnd",
        TIMESTAMP,
        "The exclusive end bound of a billing period.",
    ),
    col(
        "BillingPeriodStart",
        TIMESTAMP,
        "The inclusive start bound of a billing period.",
    ),
    col(
        "ChargeCategory",
        STRING,
        "Indicates whether the row represents an upfront or recurring fee, cost of usage that already occurred, an after-the-fact adjustment (e.g., credits), or taxes.",
    ),
    col(
        "ChargeClass",
        STRING,
        "Indicates whether the row represents a regular charge or a correction to one or more previous charges, its primary use is for differentiating refunds from normal usage.",
    ),
    col(
        "ChargeDescription",
        STRING,
        "A self-contained summary of the charge.",
    ),
    col("ChargeFrequency", STRING, "How often a charge occurs."),
    col(
        "ChargePeriodEnd",
        TIMESTAMP,
        "The exclusive end bound of a charge period.",
    ),
    col(
        "ChargePeriodStart",
        TIMESTAMP,
        "The inclusive start bound of a charge period.",
    ),
    col(
        "CommitmentDiscountCategory",
        STRING,
        "Whether the commitment discount applies to spend or to usage quantity.",
    ),
    col(
        "CommitmentDiscountId",
        STRING,
        "The identifier assigned to a commitment discount by the provider.",
    ),
    col(
        "CommitmentDiscountName",
        STRING,
        "The display name assigned to a commitment discount.",
    ),
    col(
        "CommitmentDiscountStatus",
        STRING,
        "Whether the charge covers used or unused commitment discount capacity.",
    ),
    col(
        "CommitmentDiscountType",
        STRING,
        "The provider's name for the type of commitment discount applied to the charge.",
    ),
    col(
        "ConsumedQuantity",
        NUMBER,
        "The volume of a metered SKU associated with a resource or service used.",
    ),
    col(
        "ConsumedUnit",
        STRING,
        "The unit that the consumed quantity is measured in.",
    ),
    col(
        "ContractedCost",
        NUMBER,
        "The contracted unit price multiplied by the pricing quantity.",
    ),
    col(
        "ContractedUnitPrice",
        NUMBER,
        "The agreed-upon unit price for a single pricing unit of the SKU.",
    ),
    col(
        "EffectiveCost",
        NUMBER,
        "The amortized cost of the charge, including the portion of upfront payments that applies to the charge period.",
    ),
    col(
        "InvoiceIssuerName",
        STRING,
        "The name of the entity responsible for invoicing the resources or services consumed.",
    ),
    col(
        "ListCost",
        NUMBER,
        "The list unit price multiplied by the pricing quantity.",
    ),
    col(
        "ListUnitPrice",
        NUMBER,
        "The suggested provider-published unit price for a single pricing unit of the SKU.",
    ),
    col(
        "PricingCategory",
        STRING,
        "The pricing model of the charge at the time of use or purchase.",
    ),
    col(
        "PricingQuantity",
        NUMBER,
        "The volume of a given SKU associated with a resource or service used, measured in pricing units.",
    ),
    col(
        "PricingUnit",
        STRING,
        "The unit that the provider prices a SKU in.",
    ),
    col(
        "ProviderName",
        STRING,
        "The name of the entity that made the resources or services available for purchase.",
    ),
    col(
        "PublisherName",
        STRING,
        "The name of the entity that produced the resources or services purchased.",
    ),
    col(
        "RegionId",
        STRING,
        "The provider-assigned identifier of an isolated geographic area where a resource is provisioned.",
    ),
    col(
        "RegionName",
        STRING,
        "The name of an isolated geographic area where a resource is provisioned.",
    ),
    col(
        "ResourceId",
        STRING,
        "The identifier assigned to a resource by the provider.",
    ),
    col(
        "ResourceName",
        STRING,
        "The display name assigned to a resource.",
    ),
    col(
        "ResourceType",
        STRING,
        "The kind of resource that the charge applies to.",
    ),
    col(
        "ServiceCategory",
        STRING,
        "The highest-level classification of a service, based on its core function.",
    ),
    col(
        "ServiceName",
        STRING,
        "The name of the offering that the charge applies to.",
    ),
    col(
        "SkuId",
        STRING,
        "The provider-assigned identifier of a SKU.",
    ),
    col(
        "SkuPriceId",
        STRING,
        "The provider-assigned identifier of the SKU price that the charge was rated at.",
    ),
    col(
        "SubAccountId",
        STRING,
        "The identifier assigned to a grouping of resources or services by the provider.",
    ),
    col(
        "SubAccountName",
        STRING,
        "The display name assigned to a sub account.",
    ),
    col(
        "Tags",
        MAP,
        "The tags of the charge's resource or service, as key-value pairs.",
    ),
];

/// The columns FOCUS 1.2 adds to the FOCUS 1.0 schema, in the dictionary's order.
const FOCUS_1_2_ADDED_COLUMNS: &[ColumnSpec] = &[
    col(
        "BillingAccountType",
        STRING,
        "A provider-assigned name to identify the type of billing account.",
    ),
    col(
        "CapacityReservationId",
        STRING,
        "The identifier assigned to a capacity reservation by the provider.",
    ),
    col(
        "CapacityReservationStatus",
        STRING,
        "Whether the charge covers used or unused capacity reservation capacity.",
    ),
    col(
        "CommitmentDiscountQuantity",
        NUMBER,
        "The amount of a commitment discount purchased or accounted for in commitment-discount-related rows.",
    ),
    col(
        "CommitmentDiscountUnit",
        STRING,
        "The unit that the commitment discount quantity is measured in.",
    ),
    col(
        "InvoiceId",
        STRING,
        "The identifier assigned to an invoice by the invoice issuer.",
    ),
    col(
        "PricingCurrency",
        STRING,
        "The currency that the pricing of a charge is expressed in.",
    ),
    col(
        "PricingCurrencyContractedUnitPrice",
        NUMBER,
        "The contracted unit price, expressed in the pricing currency.",
    ),
    col(
        "PricingCurrencyEffectiveCost",
        NUMBER,
        "The effective cost, expressed in the pricing currency.",
    ),
    col(
        "PricingCurrencyListUnitPrice",
        NUMBER,
        "The list unit price, expressed in the pricing currency.",
    ),
    col(
        "ServiceSubcategory",
        STRING,
        "The secondary classification of a service within its service category.",
    ),
    col(
        "SkuMeter",
        STRING,
        "A provider-specified description of what the SKU meters.",
    ),
    col(
        "SkuPriceDetails",
        MAP,
        "The provider-specified details of the SKU price, as key-value pairs.",
    ),
    col(
        "SubAccountType",
        STRING,
        "The provider's name for the type of the sub account.",
    ),
];

/// The `x_`-prefixed provider extensions Amazon Web Services publishes beside the FOCUS
/// 1.0 columns, in the dictionary's order.
const FOCUS_AWS_EXTENSION_COLUMNS: &[ColumnSpec] = &[
    col(
        "x_CostCategories",
        MAP,
        "The cost categories that apply to the charge, as key-value pairs.",
    ),
    col(
        "x_Discounts",
        MAP,
        "The discounts that apply to the charge, as key-value pairs.",
    ),
    col(
        "x_Operation",
        STRING,
        "The specific Amazon Web Services operation that the charge covers.",
    ),
    col(
        "x_ServiceCode",
        STRING,
        "The code of the Amazon Web Services service that the charge applies to.",
    ),
    col(
        "x_UsageType",
        STRING,
        "The usage type of the Amazon Web Services usage that the charge covers.",
    ),
];

/// The `x_`-prefixed provider extensions the FOCUS 1.2 schema publishes. FOCUS 1.2 does
/// NOT carry the cost-category and usage-type extensions of FOCUS 1.0: the columns the
/// 1.2 specification standardises cover them, so the 1.2 schema publishes only the
/// extensions it still needs. Named out of the FOCUS 1.0 group rather than copied, so
/// one extension column is described once.
const FOCUS_1_2_AWS_EXTENSION_COLUMNS: &[&str] = &["x_Discounts", "x_Operation", "x_ServiceCode"];

/// The description every FOCUS table carries: the specification the schema implements.
const FOCUS_DESCRIPTION: &str = "FinOps Open Cost & Usage Specification";

/// What `COST_AND_USAGE_DASHBOARD` is: a VIEW over the cost and usage data, aggregated
/// per usage date, rather than a dataset of its own.
const COST_AND_USAGE_DASHBOARD_DESCRIPTION: &str = "A view over CUR for CostAndUsageDashboard";

/// The columns `COST_AND_USAGE_DASHBOARD` describes itself: the dimensions and measures
/// the view aggregates the underlying cost and usage line items into. The line-item
/// columns it carries unchanged are taken over from the cost and usage group instead —
/// see [`COST_AND_USAGE_DASHBOARD_VIEWED_COLUMNS`].
const COST_AND_USAGE_DASHBOARD_COLUMNS: &[ColumnSpec] = &[
    col(
        "billing_period",
        TIMESTAMP,
        "The start date of the billing period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "usage_date",
        TIMESTAMP,
        "The date of the usage that this row aggregates, in UTC.",
    ),
    col(
        "payer_account_id",
        STRING,
        "The account ID of the paying account. For an organization in AWS Organizations, this is the account ID of the management account.",
    ),
    col(
        "payer_account_name",
        STRING,
        "The name of the account that pays for the usage in this row.",
    ),
    col(
        "linked_account_id",
        STRING,
        "The account ID of the member account that the usage in this row belongs to.",
    ),
    col(
        "linked_account_name",
        STRING,
        "The name of the member account that the usage in this row belongs to.",
    ),
    col(
        "invoice_id",
        STRING,
        "The ID of the invoice that includes the usage in this row.",
    ),
    col(
        "charge_type",
        STRING,
        "The type of charge that this row covers, such as usage, tax, credit or refund.",
    ),
    col(
        "charge_category",
        STRING,
        "The classification of the charge that this row covers, based on the nature of how it is billed.",
    ),
    col(
        "billing_entity",
        STRING,
        "The Amazon Web Services organization that the charge in this row was made through, which identifies an AWS Marketplace purchase.",
    ),
    col(
        "legal_entity",
        STRING,
        "The Amazon Web Services seller of record for the usage in this row.",
    ),
    col(
        "service",
        STRING,
        "The Amazon Web Services service that the usage in this row belongs to.",
    ),
    col(
        "product_code",
        STRING,
        "The code of the Amazon Web Services service that the usage in this row belongs to.",
    ),
    col(
        "product_name",
        STRING,
        "The name of the product that the usage in this row covers.",
    ),
    col(
        "product_family",
        STRING,
        "The family of the product that the usage in this row covers.",
    ),
    col(
        "product_group",
        STRING,
        "The group of the product that the usage in this row covers.",
    ),
    col(
        "operation",
        STRING,
        "The specific operation that the usage in this row covers.",
    ),
    col("usage_type", STRING, "The usage details of this row."),
    col(
        "item_description",
        STRING,
        "The description of the usage in this row.",
    ),
    col(
        "region",
        STRING,
        "The Region that the usage in this row ran in.",
    ),
    col(
        "availability_zone",
        STRING,
        "The Availability Zone that hosted the usage in this row.",
    ),
    col(
        "instance_type",
        STRING,
        "The instance type of the instance that the usage in this row ran on.",
    ),
    col(
        "instance_type_family",
        STRING,
        "The instance family of the instance that the usage in this row ran on.",
    ),
    col(
        "current_generation",
        STRING,
        "Whether the instance that the usage in this row ran on is of the current generation.",
    ),
    col(
        "platform",
        STRING,
        "The operating system of the instance that the usage in this row ran on.",
    ),
    col(
        "processor",
        STRING,
        "The processor of the instance that the usage in this row ran on.",
    ),
    col(
        "processor_features",
        STRING,
        "The processor features of the instance that the usage in this row ran on.",
    ),
    col(
        "database_engine",
        STRING,
        "The database engine that the usage in this row ran.",
    ),
    col(
        "tenancy",
        STRING,
        "The tenancy of the instance that the usage in this row ran on.",
    ),
    col(
        "purchase_option",
        STRING,
        "How the usage in this row is paid for: On-Demand, Reserved or Spot.",
    ),
    col(
        "usage_quantity",
        NUMBER,
        "The amount of usage that this row aggregates.",
    ),
    col(
        "unblended_cost",
        NUMBER,
        "The cost of the usage in this row at the rate it was charged at.",
    ),
    col(
        "amortized_cost",
        NUMBER,
        "The cost of the usage in this row with upfront payments amortized over the term they cover.",
    ),
    col(
        "public_cost",
        NUMBER,
        "The cost of the usage in this row at public On-Demand rates.",
    ),
    col(
        "ri_sp_arn",
        STRING,
        "The ARN of the Reserved Instance or Savings Plan that covers the usage in this row.",
    ),
    col(
        "ri_sp_trueup",
        NUMBER,
        "The true-up charge of the Reserved Instance or Savings Plan that covers the usage in this row.",
    ),
    col(
        "ri_sp_upfront_fees",
        NUMBER,
        "The upfront fees of the Reserved Instance or Savings Plan that covers the usage in this row.",
    ),
];

/// The cost and usage columns `COST_AND_USAGE_DASHBOARD` carries unchanged, in the order
/// it publishes them: the view reports the same column, kind of data and description as
/// the table it views, so a client that moves a query between the two reads one schema.
///
/// The cost and usage columns NOT named here are the ones the view aggregates away: the
/// line item's own identifier is kept, while the per-resource and split-cost columns the
/// cost and usage table gates on a table property are not part of this table at all —
/// this table publishes no such property.
const COST_AND_USAGE_DASHBOARD_VIEWED_COLUMNS: &[&str] = &[
    "bill_bill_type",
    "bill_billing_entity",
    "bill_billing_period_end_date",
    "bill_billing_period_start_date",
    "bill_invoice_id",
    "bill_invoicing_entity",
    "bill_payer_account_id",
    "bill_payer_account_name",
    "cost_category",
    "discount",
    "discount_bundled_discount",
    "discount_total_discount",
    "identity_line_item_id",
    "identity_time_interval",
    "line_item_availability_zone",
    "line_item_blended_cost",
    "line_item_blended_rate",
    "line_item_currency_code",
    "line_item_legal_entity",
    "line_item_line_item_description",
    "line_item_line_item_type",
    "line_item_net_unblended_cost",
    "line_item_net_unblended_rate",
    "line_item_normalization_factor",
    "line_item_normalized_usage_amount",
    "line_item_operation",
    "line_item_product_code",
    "line_item_tax_type",
    "line_item_unblended_cost",
    "line_item_unblended_rate",
    "line_item_usage_account_id",
    "line_item_usage_account_name",
    "line_item_usage_amount",
    "line_item_usage_end_date",
    "line_item_usage_start_date",
    "line_item_usage_type",
    "line_item_user_identifier",
    "pricing_currency",
    "pricing_lease_contract_length",
    "pricing_offering_class",
    "pricing_public_on_demand_cost",
    "pricing_public_on_demand_rate",
    "pricing_purchase_option",
    "pricing_rate_code",
    "pricing_rate_id",
    "pricing_term",
    "product",
    "product_comment",
    "product_fee_code",
    "product_fee_description",
    "product_from_location_type",
    "product_from_region_code",
    "product_instance_family",
    "product_instance_type",
    "product_instancesku",
    "product_location",
    "product_location_type",
    "product_operation",
    "product_pricing_unit",
    "product_product_family",
    "product_region_code",
    "product_servicecode",
    "product_sku",
    "product_to_location_type",
    "product_to_region_code",
    "product_usagetype",
    "reservation_amortized_upfront_cost_for_usage",
    "reservation_amortized_upfront_fee_for_billing_period",
    "reservation_availability_zone",
    "reservation_effective_cost",
    "reservation_end_time",
    "reservation_modification_status",
    "reservation_net_amortized_upfront_cost_for_usage",
    "reservation_net_amortized_upfront_fee_for_billing_period",
    "reservation_net_effective_cost",
    "reservation_net_recurring_fee_for_usage",
    "reservation_net_unused_amortized_upfront_fee_for_billing_period",
    "reservation_net_unused_recurring_fee",
    "reservation_net_upfront_value",
    "reservation_normalized_units_per_reservation",
    "reservation_number_of_reservations",
    "reservation_recurring_fee_for_usage",
    "reservation_reservation_a_r_n",
    "reservation_start_time",
    "reservation_subscription_id",
    "reservation_total_reserved_normalized_units",
    "reservation_total_reserved_units",
    "reservation_units_per_reservation",
    "reservation_unused_amortized_upfront_fee_for_billing_period",
    "reservation_unused_normalized_unit_quantity",
    "reservation_unused_quantity",
    "reservation_unused_recurring_fee",
    "reservation_upfront_value",
    "resource_tags",
    "savings_plan_amortized_upfront_commitment_for_billing_period",
    "savings_plan_end_time",
    "savings_plan_instance_type_family",
    "savings_plan_net_amortized_upfront_commitment_for_billing_period",
    "savings_plan_net_recurring_commitment_for_billing_period",
    "savings_plan_net_savings_plan_effective_cost",
    "savings_plan_offering_type",
    "savings_plan_payment_option",
    "savings_plan_purchase_term",
    "savings_plan_recurring_commitment_for_billing_period",
    "savings_plan_region",
    "savings_plan_savings_plan_a_r_n",
    "savings_plan_savings_plan_effective_cost",
    "savings_plan_savings_plan_rate",
    "savings_plan_start_time",
    "savings_plan_total_commitment_to_date",
    "savings_plan_used_commitment",
    "tags",
];

/// Every table data exports publishes, ordered by name — the one catalog `GetTable`
/// reports an entry of and `ListTables` lists.
pub const TABLE_CATALOG: &[TableSpec] = &[
    TableSpec {
        name: "CARBON_EMISSIONS",
        description: "Carbon Emissions",
        properties: BILLING_VIEW_ONLY_PROPERTIES,
        column_groups: &[ColumnGroup::Own(CARBON_EMISSIONS_COLUMNS)],
    },
    TableSpec {
        name: "COST_AND_USAGE_DASHBOARD",
        description: COST_AND_USAGE_DASHBOARD_DESCRIPTION,
        properties: BILLING_VIEW_ONLY_PROPERTIES,
        // A VIEW over the cost and usage data: its own aggregate dimensions and measures
        // first, then the line-item columns it carries over from the viewed table.
        column_groups: &[
            ColumnGroup::Own(COST_AND_USAGE_DASHBOARD_COLUMNS),
            ColumnGroup::View {
                of: COST_AND_USAGE_REPORT_COLUMNS,
                columns: COST_AND_USAGE_DASHBOARD_VIEWED_COLUMNS,
            },
        ],
    },
    TableSpec {
        name: "COST_AND_USAGE_REPORT",
        description: "Cost and Usage Report",
        properties: COST_AND_USAGE_REPORT_PROPERTIES,
        column_groups: &[ColumnGroup::Own(COST_AND_USAGE_REPORT_COLUMNS)],
    },
    TableSpec {
        name: "COST_OPTIMIZATION_RECOMMENDATIONS",
        description: "Cost Optimization Recommendations from the Cost Optimization Hub.",
        properties: COST_OPTIMIZATION_RECOMMENDATIONS_PROPERTIES,
        column_groups: &[ColumnGroup::Own(COST_OPTIMIZATION_RECOMMENDATIONS_COLUMNS)],
    },
    TableSpec {
        name: "FOCUS_1_0_AWS",
        description: FOCUS_DESCRIPTION,
        properties: FOCUS_PROPERTIES,
        column_groups: &[
            ColumnGroup::Own(FOCUS_1_0_COLUMNS),
            ColumnGroup::Own(FOCUS_AWS_EXTENSION_COLUMNS),
        ],
    },
    TableSpec {
        name: "FOCUS_1_0_AWS_PREVIEW",
        description: FOCUS_DESCRIPTION,
        properties: FOCUS_PREVIEW_PROPERTIES,
        // The preview table publishes the FOCUS 1.0 schema: it is the same
        // specification, offered for evaluation before the table was general.
        column_groups: &[
            ColumnGroup::Own(FOCUS_1_0_COLUMNS),
            ColumnGroup::Own(FOCUS_AWS_EXTENSION_COLUMNS),
        ],
    },
    TableSpec {
        name: "FOCUS_1_2_AWS",
        description: FOCUS_DESCRIPTION,
        properties: FOCUS_1_2_PROPERTIES,
        column_groups: &[
            ColumnGroup::Own(FOCUS_1_0_COLUMNS),
            ColumnGroup::Own(FOCUS_1_2_ADDED_COLUMNS),
            ColumnGroup::View {
                of: FOCUS_AWS_EXTENSION_COLUMNS,
                columns: FOCUS_1_2_AWS_EXTENSION_COLUMNS,
            },
        ],
    },
];
