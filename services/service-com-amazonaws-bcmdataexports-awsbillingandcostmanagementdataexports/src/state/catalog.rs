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
//! The catalog is RESPONSE DATA, not documentation: a client reads `Column.Description`
//! and `Column.Type` to decide what a column holds and what to cast it to, and reads a
//! `TablePropertyDescription`'s `ValidValues` to decide whether a `TableConfigurations`
//! value is legal before calling `CreateExport`. So every string below is the
//! dictionary's PUBLISHED text for that column or property, carried over as published,
//! rather than a shorter re-wording of it: a paraphrase is a different value on the wire.
//!
//! The column TYPE vocabulary is the dictionary's: a column's `Type` names the KIND of
//! data the column stores ("The kind of data a column stores" — smithy_query shape
//! `Column`, member `Type`) in the dictionary's own mixed-case words — `String`,
//! `Number`, `Double`, `Timestamp`, `Boolean`, `Map` — never a storage-engine type name
//! such as `STRING` or `MAP<STRING,STRING>`, which the catalog does not publish. The
//! vocabulary is PER SCHEMA, not per kind of value: the cost and usage dictionary
//! measures costs with `Number` while the recommendations and carbon emissions
//! dictionaries measure theirs with `Double`, so each table below carries the token ITS
//! dictionary publishes rather than one token chosen for every numeric column of the
//! catalog.
//!
//! A schema that SUPERSEDES another (FOCUS 1.2 over FOCUS 1.0) publishes its own text for
//! the columns its specification re-words, and the superseded schema keeps the text it
//! was published with: see [`ColumnGroup::Revised`]. So the catalog is keyed per table AND
//! per table VERSION — one entry per version, never one entry serving two versions, which
//! would make the two versions' column semantics indistinguishable to a client reading the
//! catalog to tell them apart.
//!
//! A table that is a VIEW over another publishes its own curated columns AND the columns
//! it carries over from the viewed table under the viewed table's names: see
//! [`ColumnGroup::View`]. A column the viewing table carries over is named out of the
//! viewed group rather than copied, so one column is described once. A column the
//! dictionary publishes for a table and the catalog omits is a column a client cannot
//! discover or ask for in an export definition, so the column SET is part of the
//! published data just as the text is.

use std::collections::{BTreeMap, HashMap};

use super::clip_for_message;
use crate::ops::types::{Column, ValidationException, ValidationExceptionField};

/// A column's `Type` for text the dictionary does not further constrain.
const STRING: &str = "String";
/// A column's `Type` for a numeric measure (a cost, a rate, a quantity, a count) in the
/// cost and usage and FOCUS dictionaries.
const NUMBER: &str = "Number";
/// A column's `Type` for a floating-point measure (a cost, a percentage, an emissions
/// quantity, a day count) in the recommendations and carbon emissions dictionaries, which
/// publish every numeric column as `Double`.
const DOUBLE: &str = "Double";
/// A column's `Type` for a date and time.
const TIMESTAMP: &str = "Timestamp";
/// A column's `Type` for a true/false flag.
const BOOLEAN: &str = "Boolean";
/// A column's `Type` for the key-value columns that carry tags, product attributes,
/// cost categories, discounts and SKU price details.
const MAP: &str = "Map";

/// The value a boolean-valued table property takes to switch its data or columns ON.
pub const PROPERTY_TRUE: &str = "TRUE";
/// The value a boolean-valued table property takes to switch its data or columns OFF.
pub const PROPERTY_FALSE: &str = "FALSE";

/// The valid values of a boolean-valued table property, in the order the catalog
/// publishes them.
const BOOLEAN_PROPERTY_VALUES: &[&str] = &[PROPERTY_TRUE, PROPERTY_FALSE];

/// The WILDCARD valid value: the catalog publishes it for a property whose value the
/// caller writes rather than picks, to say that any value is legal. A client validates a
/// `TableConfigurations` value against `ValidValues` before calling `CreateExport`, so a
/// property that accepts anything has to publish that fact rather than an empty list —
/// see [`TablePropertySpec::accepts`].
pub const PROPERTY_ANY_VALUE: &str = "*";

/// The valid values of a free-form table property the catalog publishes the wildcard for.
const WILDCARD_PROPERTY_VALUES: &[&str] = &[PROPERTY_ANY_VALUE];

/// The valid values of a free-form table property the catalog publishes NO list for.
/// Such a property accepts any value too — see [`TablePropertySpec::accepts`].
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

/// The value a table property assumes when a request does not name it.
///
/// A default is not always a fixed string: one of them names a RESOURCE OF THE ACCOUNT,
/// so it is built from the account id rather than written as a literal — which is why this
/// is an enum and not a `&'static str`. The catalog states WHICH default a property
/// publishes; [`PropertyDefault::value`] is the one place each one is resolved.
#[derive(Debug, Clone, Copy)]
pub enum PropertyDefault {
    /// A fixed value, the same for every account.
    Literal(&'static str),
    /// The ACCOUNT's primary billing view — see [`primary_billing_view_arn`].
    PrimaryBillingView,
    /// The catalog publishes NO default for this property: it assumes nothing, so it is
    /// in force only for a request that names it and a request that does not name it
    /// reports no value for it at all.
    Unset,
}

impl PropertyDefault {
    /// The value this default resolves to, or `None` for [`Self::Unset`].
    pub fn value(self) -> Option<&'static str> {
        match self {
            Self::Literal(value) => Some(value),
            Self::PrimaryBillingView => Some(primary_billing_view_arn()),
            Self::Unset => None,
        }
    }
}

/// The ARN of the account's PRIMARY billing view:
/// `arn:aws:billing::<account>:billingview/primary`.
///
/// Every account has one — the view over the whole of its billing data — and it is what
/// an export's billing view is scoped to when the definition names none: RECORDED AWS
/// behaviour, where an export created without a `BILLING_VIEW_ARN` table property is read
/// back carrying this ARN. The billing view is a resource of the ACCOUNT, so it is built
/// through `engine::emulator_arn` (account segment = the one canonical emulator account,
/// no region segment, as a billing view ARN is global) rather than written as a literal.
/// Built once and kept, because the catalog hands out `&'static str` values.
pub fn primary_billing_view_arn() -> &'static str {
    static ARN: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new();
    ARN.get_or_init(|| {
        ::engine::emulator_arn(BILLING_VIEW_ARN_SERVICE, "", PRIMARY_BILLING_VIEW_RESOURCE)
    })
    .as_str()
}

/// The service segment of a billing view ARN: the billing service owns billing views,
/// not data exports.
const BILLING_VIEW_ARN_SERVICE: &str = "billing";

/// The resource segment of the primary billing view's ARN.
const PRIMARY_BILLING_VIEW_RESOURCE: &str = "billingview/primary";

/// One table property of one table: its name, the values it accepts, and the value a
/// request that does not name it assumes.
///
/// Mirrors the modelled `TablePropertyDescription` (`Name`, `ValidValues`,
/// `DefaultValue`, `Description`), which is how `ListTables` reports the same entry.
#[derive(Debug, Clone, Copy)]
pub struct TablePropertySpec {
    /// The property name a request's `TableProperties` key must match.
    pub name: &'static str,
    /// The values this property accepts, as the catalog publishes them — the wildcard
    /// [`PROPERTY_ANY_VALUE`] for a free-form property that publishes one, and empty for
    /// a free-form property the catalog publishes no list for. See [`Self::accepts`].
    pub valid_values: &'static [&'static str],
    /// Which default the catalog publishes for this property — read through
    /// [`Self::default_value`], never directly, so an account-derived default is resolved
    /// in one place.
    pub published_default: PropertyDefault,
    /// What the property changes about the table's data or schema.
    pub description: &'static str,
}

impl TablePropertySpec {
    /// Whether this property accepts `value`: one of the values the catalog publishes
    /// for it, or — for a free-form property, whose value the caller writes rather than
    /// picks — any value at all. A free-form property is one that publishes the wildcard
    /// [`PROPERTY_ANY_VALUE`] or publishes no list.
    pub fn accepts(&self, value: &str) -> bool {
        self.valid_values.is_empty()
            || self.valid_values.contains(&PROPERTY_ANY_VALUE)
            || self.valid_values.contains(&value)
    }

    /// The value this property takes when a request does not name it, or `None` for a
    /// property the catalog publishes no default for.
    pub fn default_value(&self) -> Option<&'static str> {
        self.published_default.value()
    }
}

/// One column of one table's schema.
#[derive(Debug, Clone, Copy)]
pub struct ColumnSpec {
    /// The column name.
    pub name: &'static str,
    /// The kind of data the column stores, as the dictionary publishes it: one of
    /// [`STRING`], [`NUMBER`], [`DOUBLE`], [`TIMESTAMP`], [`BOOLEAN`], [`MAP`].
    pub data_type: &'static str,
    /// What the column holds, as the dictionary publishes it — the modelled
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
    /// Every column of another schema's group, with the DESCRIPTIONS this schema's own
    /// specification revises applied over them. A later version of a specification
    /// re-words some of the columns it carries forward while keeping the rest, and the
    /// earlier version keeps the text IT was published with, so the two schemas publish
    /// the same column with different text — which is why a revision cannot be edited
    /// into the shared group.
    Revised {
        /// The group the columns are carried forward from, in its own order.
        of: &'static [ColumnSpec],
        /// `(column name, the description this schema publishes for it)`. A name the
        /// group does not describe revises nothing.
        revisions: &'static [(&'static str, &'static str)],
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
            Self::Revised { of, revisions } => of
                .iter()
                .map(|column| match revisions
                    .iter()
                    .find(|(name, _)| *name == column.name)
                {
                    Some((_, description)) => ColumnSpec {
                        description,
                        ..*column
                    },
                    None => *column,
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
                    .default_value()
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
///
/// The sentence is BUILT FROM THE REQUEST: it names the table that was asked for and
/// reports that it was not found, which is the whole of what the caller has to act on.
/// (Recorded AWS behavior shows that form, e.g. "Table NOT_A_REAL_TABLE_XYZ not found";
/// the requirement is the request-derived form, not that one string.) The tables that ARE
/// published are what `ListTables` reports, so the message does not restate them.
pub fn unknown_table(name: &str) -> ValidationException {
    ValidationException {
        message: format!("Table {} not found", clip_for_message(name)),
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
        published_default: PropertyDefault::Literal(PROPERTY_FALSE),
        description: "Whether to include capacity reservation data.",
    },
    BILLING_VIEW_ARN_PROPERTY,
    TablePropertySpec {
        name: PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        published_default: PropertyDefault::Literal(PROPERTY_FALSE),
        description: "Whether to simulate manual discounts for automated discount data.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        published_default: PropertyDefault::Literal(PROPERTY_FALSE),
        description: "Whether to include fission columns.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        // The catalog publishes NO default for this property: it is in force only for a
        // request that names it, so a request that names none reports no value for it.
        published_default: PropertyDefault::Unset,
        description: "Whether IAM principal columns should be included or not.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_RESOURCES,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        published_default: PropertyDefault::Literal(PROPERTY_FALSE),
        description: "Whether to include resource IDs on line items.",
    },
];

/// The table properties of `COST_OPTIMIZATION_RECOMMENDATIONS`, in the order the catalog
/// publishes them.
const COST_OPTIMIZATION_RECOMMENDATIONS_PROPERTIES: &[TablePropertySpec] = &[
    TablePropertySpec {
        name: PROPERTY_INCLUDE_ALL_RECOMMENDATIONS,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        published_default: PropertyDefault::Literal(PROPERTY_FALSE),
        description: "List of all recommendations for a resource, or a single recommendation if de-duped by resourceId.",
    },
    TablePropertySpec {
        name: PROPERTY_FILTER,
        // Free-form, and the catalog SAYS so by publishing the wildcard: the value is a
        // JSON filter over the recommendation columns, which no list could enumerate, so
        // a client validating against the published list has to see that any value is
        // legal. The default is the empty filter: every row.
        valid_values: WILDCARD_PROPERTY_VALUES,
        published_default: PropertyDefault::Literal("{}"),
        description: "The constraints that you want all returned recommendations to match. Expressed as a json of AWS Cost Optimization Hub's Filter SDK object. See COST_OPTIMIZATION_RECOMMENDATIONS table documentation for further details.",
    },
];

/// The table properties of the general FOCUS 1.0 table.
const FOCUS_PROPERTIES: &[TablePropertySpec] = &[BILLING_VIEW_ARN_PROPERTY];

/// The table properties of the FOCUS PREVIEW table: the one thing the catalog publishes
/// about a superseded preview table is that it is deprecated, which is how a client
/// learns not to build new exports on it. It publishes no billing view, because no new
/// export is configured against it at all.
const FOCUS_PREVIEW_PROPERTIES: &[TablePropertySpec] = &[TablePropertySpec {
    name: PROPERTY_DEPRECATED,
    // The only value this property takes for a preview table the catalog publishes as
    // deprecated: there is no configuration of it that is not deprecated.
    valid_values: &[PROPERTY_TRUE],
    published_default: PropertyDefault::Literal(PROPERTY_TRUE),
    description: "This table has been deprecated. New exports for this table are no longer supported.",
}];

/// The table properties of the FOCUS 1.2 table, whose rows — unlike FOCUS 1.0's — are
/// aggregated over a period of time, so it publishes the granularity of those rows.
const FOCUS_1_2_PROPERTIES: &[TablePropertySpec] = &[TIME_GRANULARITY_PROPERTY];

/// The table properties of the tables the catalog scopes by billing view alone.
const BILLING_VIEW_ONLY_PROPERTIES: &[TablePropertySpec] = &[BILLING_VIEW_ARN_PROPERTY];

/// The billing view property: the data in the table it is published for can be scoped to
/// a billing view.
const BILLING_VIEW_ARN_PROPERTY: TablePropertySpec = TablePropertySpec {
    name: PROPERTY_BILLING_VIEW_ARN,
    // Free-form: the value is a billing view ARN the caller owns, not a value the
    // catalog can enumerate. The default is the ACCOUNT's primary billing view — the view
    // over the whole of its billing data — so a table a definition does not scope is
    // scoped to that, which is what a reader of the definition reports.
    valid_values: FREE_FORM_PROPERTY_VALUES,
    published_default: PropertyDefault::PrimaryBillingView,
    description: "The arn of the billing conductor group that this CUR2.0/ FOCUS Data Export is for.",
};

/// The granularity property, published by every table whose rows are aggregated over a
/// period of time.
const TIME_GRANULARITY_PROPERTY: TablePropertySpec = TablePropertySpec {
    name: PROPERTY_TIME_GRANULARITY,
    valid_values: &["DAILY", "MONTHLY", "HOURLY"],
    published_default: PropertyDefault::Literal("HOURLY"),
    description: "The granularity of the line-items in the table.",
};

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

/// The columns of `CARBON_EMISSIONS`, in the dictionary's order. Every emissions measure
/// is published as a `Double` value beside the `String` unit it is measured in.
const CARBON_EMISSIONS_COLUMNS: &[ColumnSpec] = &[
    col(
        "payer_account_id",
        STRING,
        "The account ID of the paying account. For an organization in AWS Organizations, this is the account ID of the management account.",
    ),
    col(
        "usage_account_id",
        STRING,
        "The account ID of the account associated with the carbon footprint value. For organizations, this can be either the management account or a member account.",
    ),
    col(
        "usage_period_start",
        TIMESTAMP,
        "The start date of the period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "usage_period_end",
        TIMESTAMP,
        "The end date of the period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "last_refresh_timestamp",
        TIMESTAMP,
        "The date and time that the data in this report was last refreshed, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "model_version",
        STRING,
        "The version of the carbon emissions model that was used to calculate the carbon footprint value.",
    ),
    col(
        "product_code",
        STRING,
        "The code of the product measured. For example, Amazon EC2 is the product code for Amazon Elastic Compute Cloud.",
    ),
    // The Region the usage originated in, published as a code and — in `location` — as the
    // human-readable place that code stands for, which is why `location` names
    // `region_code` in its own text.
    col(
        "region_code",
        STRING,
        "Describes the Region code where the usage originated from.",
    ),
    col(
        "location",
        STRING,
        "Describes the human-readable location where the usage originated from, corresponding to the region_code.",
    ),
    // The emissions MEASURES. Each one is reported as a value and the unit that value is
    // measured in, rather than as a single column in an assumed unit: the table publishes
    // the market-based (mbm) and location-based (lbm) totals and, per emissions scope,
    // the scope's own total.
    col(
        "total_lbm_emissions_value",
        DOUBLE,
        "The total carbon emissions associated with your AWS usage, calculated using the location-based method (LBM).",
    ),
    col(
        "total_lbm_emissions_unit",
        STRING,
        "The unit of measurement for the total location-based method (LBM) carbon emissions.",
    ),
    col(
        "total_mbm_emissions_value",
        DOUBLE,
        "The total carbon emissions associated with your AWS usage, calculated using the market-based method (MBM).",
    ),
    col(
        "total_mbm_emissions_unit",
        STRING,
        "The unit of measurement for the total market-based method (MBM) carbon emissions.",
    ),
    col(
        "total_scope_1_emissions_value",
        DOUBLE,
        "The total scope 1 carbon emissions associated with your AWS usage. Scope 1 covers the direct emissions from sources that AWS owns or controls.",
    ),
    col(
        "total_scope_1_emissions_unit",
        STRING,
        "The unit of measurement for the total scope 1 carbon emissions.",
    ),
    col(
        "total_scope_2_lbm_emissions_value",
        DOUBLE,
        "The total scope 2 carbon emissions associated with your AWS usage, calculated using the location-based method (LBM). Scope 2 covers the indirect emissions from the generation of purchased energy.",
    ),
    col(
        "total_scope_2_lbm_emissions_unit",
        STRING,
        "The unit of measurement for the total scope 2 location-based method (LBM) carbon emissions.",
    ),
    col(
        "total_scope_2_mbm_emissions_value",
        DOUBLE,
        "The total scope 2 carbon emissions associated with your AWS usage, calculated using the market-based method (MBM). Scope 2 covers the indirect emissions from the generation of purchased energy.",
    ),
    col(
        "total_scope_2_mbm_emissions_unit",
        STRING,
        "The unit of measurement for the total scope 2 market-based method (MBM) carbon emissions.",
    ),
    col(
        "total_scope_3_lbm_emissions_value",
        DOUBLE,
        "The total scope 3 carbon emissions associated with your AWS usage, calculated using the location-based method (LBM). Scope 3 covers the indirect emissions that occur in the value chain.",
    ),
    col(
        "total_scope_3_lbm_emissions_unit",
        STRING,
        "The unit of measurement for the total scope 3 location-based method (LBM) carbon emissions.",
    ),
    col(
        "total_scope_3_mbm_emissions_value",
        DOUBLE,
        "The total scope 3 carbon emissions associated with your AWS usage, calculated using the market-based method (MBM). Scope 3 covers the indirect emissions that occur in the value chain.",
    ),
    col(
        "total_scope_3_mbm_emissions_unit",
        STRING,
        "The unit of measurement for the total scope 3 market-based method (MBM) carbon emissions.",
    ),
];

/// The columns of the cost and usage tables (Cost and Usage Report 2.0), in the
/// dictionary's order. The gated ones are exactly the columns a table property adds.
const COST_AND_USAGE_REPORT_COLUMNS: &[ColumnSpec] = &[
    col(
        "bill_bill_type",
        STRING,
        "The type of bill that this report covers.",
    ),
    col(
        "bill_billing_entity",
        STRING,
        "Helps you identify whether your invoices or transactions are for AWS Marketplace or for purchases of other AWS services.",
    ),
    col(
        "bill_billing_period_end_date",
        TIMESTAMP,
        "The end date of the billing period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "bill_billing_period_start_date",
        TIMESTAMP,
        "The start date of the billing period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "bill_invoice_id",
        STRING,
        "The ID associated with a specific line item. Until the report is final, the InvoiceId is blank.",
    ),
    col(
        "bill_invoicing_entity",
        STRING,
        "The AWS entity that issues the invoice.",
    ),
    col(
        "bill_payer_account_id",
        STRING,
        "The account ID of the paying account. For an organization in AWS Organizations, this is the account ID of the management account.",
    ),
    col(
        "bill_payer_account_name",
        STRING,
        "The name of the paying account.",
    ),
    col(
        "cost_category",
        MAP,
        "Cost categories that are applied to the line item.",
    ),
    col(
        "discount",
        MAP,
        "Discounts that are applied to the line item.",
    ),
    col(
        "discount_bundled_discount",
        NUMBER,
        "The bundled discount applied to the line item. A bundled discount is a usage-based discount that provides free or discounted usage of a service or feature based on the usage of another service or feature.",
    ),
    col(
        "discount_total_discount",
        NUMBER,
        "The sum of all the discount columns for the corresponding line item.",
    ),
    col(
        "identity_line_item_id",
        STRING,
        "This field is generated for each line item and is unique in a given partition. This does not guarantee that the field will be unique across an entire delivery (that is, all partitions in an update) of the AWS CUR. The line item ID isn't consistent between different Cost and Usage Reports and can't be used to identify the same line item across different reports.",
    ),
    col(
        "identity_time_interval",
        STRING,
        "The time interval that this line item applies to, in the following format: YYYY-MM-DDTHH:mm:ssZ/YYYY-MM-DDTHH:mm:ssZ. The time interval is in UTC and can be either daily or hourly, depending on the granularity of the report.",
    ),
    col(
        "line_item_availability_zone",
        STRING,
        "The Availability Zone that hosts this line item.",
    ),
    col(
        "line_item_blended_cost",
        NUMBER,
        "The BlendedRate multiplied by the UsageAmount.",
    ),
    col(
        "line_item_blended_rate",
        STRING,
        "The BlendedRate is the average cost incurred for each SKU across an organization.",
    ),
    col(
        "line_item_currency_code",
        STRING,
        "The currency that this line item is shown in. All AWS customers are billed in US dollars by default. To change your billing currency, see Changing which currency you use to pay your bill in the AWS Billing User Guide.",
    ),
    col(
        "line_item_legal_entity",
        STRING,
        "The seller of record for a specific product or service.",
    ),
    col(
        "line_item_line_item_description",
        STRING,
        "The description of the line item type.",
    ),
    col(
        "line_item_line_item_type",
        STRING,
        "The type of charge covered by this line item.",
    ),
    col(
        "line_item_net_unblended_cost",
        NUMBER,
        "The actual after-discount cost that you're paying for the line item.",
    ),
    col(
        "line_item_net_unblended_rate",
        STRING,
        "The actual after-discount rate that you're paying for the line item.",
    ),
    col(
        "line_item_normalization_factor",
        NUMBER,
        "As long as the instance has shared tenancy, AWS can apply all Regional Linux or Unix Amazon EC2 and Amazon RDS RI discounts to all instance sizes in an instance family and AWS Region. This also applies to RI discounts for member accounts in an organization. All new and existing Amazon EC2 and Amazon RDS size-flexible RIs are sized according to a normalization factor, based on the instance size.",
    ),
    col(
        "line_item_normalized_usage_amount",
        NUMBER,
        "The amount of usage that you incurred, in normalized units, for size-flexible RIs. The NormalizedUsageAmount is equal to UsageAmount multiplied by NormalizationFactor.",
    ),
    col(
        "line_item_operation",
        STRING,
        "The specific AWS operation covered by this line item. This describes the specific usage of the line item.",
    ),
    col(
        "line_item_product_code",
        STRING,
        "The code of the product measured.",
    ),
    col_when(
        "line_item_resource_id",
        STRING,
        "If you chose to include individual resource IDs in your report, this column contains the ID of the resource that you provisioned.",
        PROPERTY_INCLUDE_RESOURCES,
        PROPERTY_TRUE,
    ),
    col(
        "line_item_tax_type",
        STRING,
        "The type of tax that AWS applied to this line item.",
    ),
    // The unblended pair, worded as the dictionary words the blended pair above: the rate
    // an individual account is charged, and that rate times the usage.
    col(
        "line_item_unblended_cost",
        NUMBER,
        "The UnblendedCost is the UnblendedRate multiplied by the UsageAmount.",
    ),
    col(
        "line_item_unblended_rate",
        STRING,
        "In consolidated billing for accounts using Reserved Instances, the UnblendedRate is the rate associated with an individual account's service usage.",
    ),
    col(
        "line_item_usage_account_id",
        STRING,
        "The account ID of the account that used this line item. For organizations, this can be either the management account or a member account. You can use this field to track costs and usage.",
    ),
    col(
        "line_item_usage_account_name",
        STRING,
        "The name of the account that used this line item. For organizations, this can be either the management account or a member account.",
    ),
    col(
        "line_item_usage_amount",
        NUMBER,
        "The amount of usage that you incurred during the specified time period. For size-flexible Reserved Instances, use the reservation_total_reserved_units column instead. Certain subscription charges will have a UsageAmount of 0.",
    ),
    col(
        "line_item_usage_end_date",
        TIMESTAMP,
        "The end date and time for the corresponding line item in UTC, exclusive. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "line_item_usage_start_date",
        TIMESTAMP,
        "The start date and time for the line item in UTC, inclusive. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "line_item_usage_type",
        STRING,
        "The usage details of the line item.",
    ),
    col(
        "line_item_user_identifier",
        STRING,
        "The unique identifier of the IAM principal that made the request that generated this line item.",
    ),
    col(
        "pricing_currency",
        STRING,
        "The currency that the pricing data is shown in.",
    ),
    col(
        "pricing_lease_contract_length",
        STRING,
        "The length of time that your Reserved Instance is reserved for.",
    ),
    col(
        "pricing_offering_class",
        STRING,
        "The offering class of the Reserved Instance.",
    ),
    col(
        "pricing_public_on_demand_cost",
        NUMBER,
        "The total cost for the line item based on public On-Demand Instance rates.",
    ),
    col(
        "pricing_public_on_demand_rate",
        STRING,
        "The public On-Demand Instance rate in this billing period for the specific line item of usage.",
    ),
    col(
        "pricing_purchase_option",
        STRING,
        "How you chose to pay for this line item. Valid values are All Upfront, Partial Upfront, and No Upfront.",
    ),
    col(
        "pricing_rate_code",
        STRING,
        "A unique code for a product/offer/pricing-tier combination. The product and term combinations can have multiple price dimensions, such as a free tier, low-use tier, and high-use tier.",
    ),
    col(
        "pricing_rate_id",
        STRING,
        "The ID of the rate for a line item.",
    ),
    col(
        "pricing_term",
        STRING,
        "Whether your AWS usage is Reserved or On-Demand.",
    ),
    col(
        "pricing_unit",
        STRING,
        "The pricing unit that AWS used for calculating your usage cost. For example, the pricing unit for Amazon EC2 instance usage is in hours.",
    ),
    col(
        "product",
        MAP,
        "A map column containing key-value pairs of the product attributes of the line item, where the key is the attribute name and the value is the attribute value.",
    ),
    col(
        "product_comment",
        STRING,
        "A comment regarding the product.",
    ),
    col(
        "product_fee_code",
        STRING,
        "The fee code of the product.",
    ),
    col(
        "product_fee_description",
        STRING,
        "The description of the fee for the product.",
    ),
    col(
        "product_from_location",
        STRING,
        "Describes the location where the usage originated from.",
    ),
    col(
        "product_from_location_type",
        STRING,
        "Describes the location type where the usage originated from.",
    ),
    col(
        "product_from_region_code",
        STRING,
        "Describes the Region code where the usage originated from.",
    ),
    col(
        "product_instance_family",
        STRING,
        "Describes the Amazon EC2 instance family.",
    ),
    col(
        "product_instance_type",
        STRING,
        "Describes the instance type, size, and family, which define the CPU, networking, and storage capacity of your instance.",
    ),
    col(
        "product_instancesku",
        STRING,
        "The SKU of the instance.",
    ),
    col(
        "product_location",
        STRING,
        "Describes the Region that your resource resides in.",
    ),
    col(
        "product_location_type",
        STRING,
        "Describes the endpoint of your task.",
    ),
    col(
        "product_operation",
        STRING,
        "The specific AWS operation covered by this line item.",
    ),
    col(
        "product_pricing_unit",
        STRING,
        "The smallest billing unit for an AWS service. For example, 0.01c per API call.",
    ),
    col(
        "product_product_family",
        STRING,
        "The category for the type of product.",
    ),
    col(
        "product_region_code",
        STRING,
        "The Region that the resource resides in.",
    ),
    col(
        "product_servicecode",
        STRING,
        "The code of the AWS service.",
    ),
    col(
        "product_sku",
        STRING,
        "A unique code for a product. The SKU is created by combining the ProductCode, UsageType, and Operation.",
    ),
    col(
        "product_to_location",
        STRING,
        "Describes the location where the usage is sent to.",
    ),
    col(
        "product_to_location_type",
        STRING,
        "Describes the location type where the usage is sent to.",
    ),
    col(
        "product_to_region_code",
        STRING,
        "Describes the Region code where the usage is sent to.",
    ),
    col(
        "product_usagetype",
        STRING,
        "The usage type of the product.",
    ),
    col(
        "reservation_amortized_upfront_cost_for_usage",
        NUMBER,
        "The initial upfront payment for all upfront Reserved Instances and partial upfront Reserved Instances amortized for usage time.",
    ),
    col(
        "reservation_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "Describes how much of your upfront fee for this reservation is costing you for the billing period.",
    ),
    col(
        "reservation_availability_zone",
        STRING,
        "The Availability Zone of the resource that is associated with this line item.",
    ),
    col(
        "reservation_effective_cost",
        NUMBER,
        "The sum of both the upfront and hourly rate of your Reserved Instance, averaged into an effective hourly rate.",
    ),
    col(
        "reservation_end_time",
        STRING,
        "The end date of the associated Reserved Instance lease term.",
    ),
    col(
        "reservation_modification_status",
        STRING,
        "Shows whether the Reserved Instance lease was modified or if it is unaltered.",
    ),
    col(
        "reservation_net_amortized_upfront_cost_for_usage",
        NUMBER,
        "The initial upfront payment for All Upfront Reserved Instances and Partial Upfront Reserved Instances amortized for usage time, with discounts applied.",
    ),
    col(
        "reservation_net_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "The cost of the reservation's upfront fee for the billing period, with discounts applied.",
    ),
    col(
        "reservation_net_effective_cost",
        NUMBER,
        "The effective cost for the Reserved Instance, with discounts applied.",
    ),
    col(
        "reservation_net_recurring_fee_for_usage",
        NUMBER,
        "The after-discount cost of the recurring usage fee.",
    ),
    col(
        "reservation_net_unused_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "The amortized portion of the initial upfront fee for unused reservation hours, with discounts applied.",
    ),
    col(
        "reservation_net_unused_recurring_fee",
        NUMBER,
        "The recurring fees associated with unused reservation hours for Partial Upfront and No Upfront Reserved Instances, with discounts applied.",
    ),
    col(
        "reservation_net_upfront_value",
        NUMBER,
        "The upfront value of the Reserved Instance, with discounts applied.",
    ),
    col(
        "reservation_normalized_units_per_reservation",
        STRING,
        "The number of normalized units for each instance of a reservation subscription.",
    ),
    col(
        "reservation_number_of_reservations",
        STRING,
        "The number of reservations that are covered by this subscription. For example, one Reserved Instance subscription might have four associated Reserved Instance reservations.",
    ),
    col(
        "reservation_recurring_fee_for_usage",
        NUMBER,
        "The recurring fee amortized for usage time, for partial upfront Reserved Instances and no upfront Reserved Instances.",
    ),
    col(
        "reservation_reservation_a_r_n",
        STRING,
        "The Amazon Resource Name (ARN) of the Reserved Instance that this line item benefited from. This is also called the Reserved Instance Lease ID. The value string also contains the AWS service name and the Region where the Reserved Instance was purchased.",
    ),
    col(
        "reservation_start_time",
        STRING,
        "The start date of the term of the associated Reserved Instance.",
    ),
    col(
        "reservation_subscription_id",
        STRING,
        "A unique identifier that maps a line item with the associated offer. We recommend you use the Reserved Instance ARN as your identifier of joining with Reserved Instance related data.",
    ),
    col(
        "reservation_total_reserved_normalized_units",
        STRING,
        "The total number of reserved normalized units for all instances for a reservation subscription.",
    ),
    col(
        "reservation_total_reserved_units",
        STRING,
        "For Amazon EC2 and Amazon RDS, this is the total number of units reserved for all instances for a reservation subscription.",
    ),
    col(
        "reservation_units_per_reservation",
        STRING,
        "For Amazon EC2 and Amazon RDS, this is the total number of units reserved for a single instance.",
    ),
    col(
        "reservation_unused_amortized_upfront_fee_for_billing_period",
        NUMBER,
        "The amortized portion of the initial upfront fee for all upfront Reserved Instances and partial upfront Reserved Instances that you did not use during the billing period.",
    ),
    col(
        "reservation_unused_normalized_unit_quantity",
        NUMBER,
        "The number of unused normalized units for a size-flexible Regional Reserved Instance that you didn't use during this billing period.",
    ),
    col(
        "reservation_unused_quantity",
        NUMBER,
        "The number of reservation hours that you didn't use during this billing period.",
    ),
    col(
        "reservation_unused_recurring_fee",
        NUMBER,
        "The recurring fees associated with your unused reservation hours for partial upfront and no upfront Reserved Instances.",
    ),
    col(
        "reservation_upfront_value",
        NUMBER,
        "The upfront price paid for your AWS Reserved Instance. For no upfront Reserved Instances, this value is 0.",
    ),
    col(
        "resource_tags",
        MAP,
        "A map column containing key-value pairs of the AWS generated and user-defined tags of the resource of the line item.",
    ),
    col(
        "savings_plan_amortized_upfront_commitment_for_billing_period",
        // The cost and usage dictionary publishes the Savings Plan commitment and rate as
        // TEXT, not as a numeric measure like the other cost columns of this schema.
        STRING,
        "The amount of upfront fee a Savings Plan subscription is costing you for the billing period.",
    ),
    col(
        "savings_plan_end_time",
        STRING,
        "The expiration date for the Savings Plan agreement.",
    ),
    col(
        "savings_plan_instance_type_family",
        STRING,
        "The instance family that is associated with the specified Savings Plan.",
    ),
    col(
        "savings_plan_net_amortized_upfront_commitment_for_billing_period",
        NUMBER,
        "The amortized Savings Plan upfront commitment for the billing period, with discounts applied.",
    ),
    col(
        "savings_plan_net_recurring_commitment_for_billing_period",
        NUMBER,
        "The Savings Plan recurring commitment for the billing period, with discounts applied.",
    ),
    col(
        "savings_plan_net_savings_plan_effective_cost",
        NUMBER,
        "The effective cost for the Savings Plan, with discounts applied.",
    ),
    col(
        "savings_plan_offering_type",
        STRING,
        "Describes the type of Savings Plan purchased.",
    ),
    col(
        "savings_plan_payment_option",
        STRING,
        "The payment option for your Savings Plan.",
    ),
    col(
        "savings_plan_purchase_term",
        STRING,
        "Describes the duration, or term, of the Savings Plan.",
    ),
    col(
        "savings_plan_recurring_commitment_for_billing_period",
        NUMBER,
        "The monthly recurring fee for your Savings Plan subscriptions.",
    ),
    col(
        "savings_plan_region",
        STRING,
        "The AWS Region (geographic area) that hosts your AWS services.",
    ),
    col(
        "savings_plan_savings_plan_a_r_n",
        STRING,
        "The unique Savings Plan identifier.",
    ),
    col(
        "savings_plan_savings_plan_effective_cost",
        NUMBER,
        "The proportion of the Savings Plan monthly commitment amount that is allocated to each usage line.",
    ),
    col(
        "savings_plan_savings_plan_rate",
        STRING,
        "The Savings Plan rate for the usage.",
    ),
    col(
        "savings_plan_start_time",
        STRING,
        "The start date of the Savings Plan agreement.",
    ),
    col(
        "savings_plan_total_commitment_to_date",
        NUMBER,
        "The total amortized upfront commitment and recurring commitment to date, for a Savings Plan.",
    ),
    col(
        "savings_plan_used_commitment",
        NUMBER,
        "The total of the Savings Plan commitment that was used.",
    ),
    col_when(
        "split_line_item_actual_usage",
        NUMBER,
        "The usage for vCPU or memory based on the metric that you selected for allocating costs.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_net_split_cost",
        NUMBER,
        "The effective cost for split cost allocation data, with discounts applied.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_net_unused_cost",
        NUMBER,
        "The effective unused cost for split cost allocation data, with discounts applied.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_parent_resource_id",
        STRING,
        "The resource ID of the parent EC2 instance that is associated with the split cost allocation data.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_public_on_demand_split_cost",
        NUMBER,
        "The split cost for the task, based on public On-Demand Instance rates.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_public_on_demand_unused_cost",
        NUMBER,
        "The unused cost for the task, based on public On-Demand Instance rates.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_reserved_usage",
        NUMBER,
        "The usage for vCPU or memory that you reserved for the task.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_cost",
        NUMBER,
        "The cost of vCPU or memory for the task, based on your usage.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_usage",
        NUMBER,
        "The usage for vCPU or memory that was used by the task.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_usage_ratio",
        NUMBER,
        "The ratio of vCPU or memory that the task used, out of the total usage of the EC2 instance.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_unused_cost",
        NUMBER,
        "The cost of unused vCPU or memory for the EC2 instance.",
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col(
        "tags",
        MAP,
        "A map column containing key-value pairs of the user-defined tags of the line item.",
    ),
];

/// The columns of `COST_OPTIMIZATION_RECOMMENDATIONS`, in the dictionary's order. This
/// dictionary publishes every numeric column — costs, percentages and the lookback day
/// count alike — as `Double`, and the resource's tags as a key-value `Map`.
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
        "The type of action you can take to realize the estimated savings for the recommendation.",
    ),
    col(
        "currency_code",
        STRING,
        "The currency code used for the recommendation.",
    ),
    col(
        "current_resource_details",
        STRING,
        "Details about the current resource, in a JSON string.",
    ),
    col(
        "current_resource_summary",
        STRING,
        "A summary of the current resource configuration.",
    ),
    col(
        "current_resource_type",
        STRING,
        "The type of the current resource.",
    ),
    // Both estimated monthly COST columns report the CURRENT resource's cost — before and
    // after discounts — and it is the estimated monthly SAVINGS columns that report what
    // the recommended resource would change. Attributing the after-discount cost to the
    // recommended resource inverts what the column means.
    col(
        "estimated_monthly_cost_after_discount",
        DOUBLE,
        "The estimated monthly cost of the current resource after discounts. For Reserved Instances and Savings Plans, it refers to the cost for eligible usage.",
    ),
    col(
        "estimated_monthly_cost_before_discount",
        DOUBLE,
        "The estimated monthly cost of the current resource before discounts. For Reserved Instances and Savings Plans, it refers to the cost for eligible usage.",
    ),
    col(
        "estimated_monthly_savings_after_discount",
        DOUBLE,
        "The estimated monthly savings amount for the recommendation after discounts.",
    ),
    col(
        "estimated_monthly_savings_before_discount",
        DOUBLE,
        "The estimated monthly savings amount for the recommendation before discounts.",
    ),
    col(
        "estimated_savings_percentage_after_discount",
        DOUBLE,
        "The estimated savings percentage relative to the total cost over the cost calculation lookback period, after discounts.",
    ),
    col(
        "estimated_savings_percentage_before_discount",
        DOUBLE,
        "The estimated savings percentage relative to the total cost over the cost calculation lookback period, before discounts.",
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
        DOUBLE,
        "The number of days of usage that the recommendation was generated from.",
    ),
    col(
        "recommendation_source",
        STRING,
        "The source of the recommendation.",
    ),
    col(
        "recommended_resource_details",
        STRING,
        "Details about the recommended resource, in a JSON string.",
    ),
    col(
        "recommended_resource_summary",
        STRING,
        "A summary of the recommended resource configuration.",
    ),
    col(
        "recommended_resource_type",
        STRING,
        "The type of the recommended resource.",
    ),
    col(
        "region",
        STRING,
        "The AWS Region of the resource.",
    ),
    col(
        "resource_arn",
        STRING,
        "The Amazon Resource Name (ARN) of the resource.",
    ),
    col(
        "restart_needed",
        BOOLEAN,
        "Whether or not implementing the recommendation requires a restart.",
    ),
    col(
        "rollback_possible",
        BOOLEAN,
        "Whether or not implementing the recommendation can be rolled back.",
    ),
    col(
        "tags",
        // The recommendations dictionary publishes the resource's tags as a key-value
        // column, which is what a consumer casts the column to.
        MAP,
        "The tags of the resource that the recommendation is for.",
    ),
];

/// The standard columns of the FOCUS 1.0 schema, in the dictionary's order, with the text
/// the FOCUS 1.0 specification publishes for each. Shared by the FOCUS 1.0 tables and
/// carried forward — with the revisions in [`FOCUS_1_2_REVISED_DESCRIPTIONS`] — by FOCUS
/// 1.2, which extends this schema rather than replacing it.
const FOCUS_1_0_COLUMNS: &[ColumnSpec] = &[
    col(
        "AvailabilityZone",
        STRING,
        "A provider-assigned identifier for a physically separated and isolated area within a Region that provides high availability and fault tolerance.",
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
        "Represents the currency that a charge was billed in.",
    ),
    col(
        "BillingPeriodEnd",
        TIMESTAMP,
        "The exclusive end date and time of a billing period.",
    ),
    col(
        "BillingPeriodStart",
        TIMESTAMP,
        "The inclusive start date and time of a billing period.",
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
        "Self-contained summary of the charge's purpose and price.",
    ),
    col(
        "ChargeFrequency",
        STRING,
        "Indicates how often a charge will occur.",
    ),
    col(
        "ChargePeriodEnd",
        TIMESTAMP,
        "The exclusive end date and time of a charge period.",
    ),
    col(
        "ChargePeriodStart",
        TIMESTAMP,
        "The inclusive start date and time of a charge period.",
    ),
    col(
        "CommitmentDiscountCategory",
        STRING,
        "Indicates whether the commitment-based discount identified in the CommitmentDiscountId column is based on usage quantity or cost (aka \"spend\").",
    ),
    col(
        "CommitmentDiscountId",
        STRING,
        "The identifier assigned to a commitment-based discount by the provider.",
    ),
    col(
        "CommitmentDiscountName",
        STRING,
        "The display name assigned to a commitment-based discount.",
    ),
    col(
        "CommitmentDiscountStatus",
        STRING,
        "Indicates whether the charge corresponds with the consumption of a commitment-based discount or the unused portion of the committed amount.",
    ),
    col(
        "CommitmentDiscountType",
        STRING,
        "A provider-assigned identifier for the type of commitment-based discount applied to the row.",
    ),
    col(
        "ConsumedQuantity",
        NUMBER,
        "The volume of a given SKU associated with a resource or service used, based on the Consumed Unit.",
    ),
    col(
        "ConsumedUnit",
        STRING,
        "Provider-specified measurement unit indicating how a provider measures usage of a given SKU associated with a resource or service.",
    ),
    col(
        "ContractedCost",
        NUMBER,
        "Cost calculated by multiplying contracted unit price and the corresponding Pricing Quantity.",
    ),
    col(
        "ContractedUnitPrice",
        NUMBER,
        "The agreed-upon unit price for a single Pricing Unit of the associated SKU, inclusive of negotiated discounts, if present, while excluding negotiated commitment-based discounts or any other discounts.",
    ),
    col(
        "EffectiveCost",
        NUMBER,
        "Cost inclusive of the impacts of all reduced rates and discounts, augmented with the amortization of relevant purchases (one-time or recurring) paid to cover future eligible charges.",
    ),
    col(
        "InvoiceIssuerName",
        STRING,
        "The name of the entity responsible for invoicing for the resources or services consumed.",
    ),
    col(
        "ListCost",
        NUMBER,
        "Cost calculated by multiplying the list unit price and the corresponding Pricing Quantity.",
    ),
    col(
        "ListUnitPrice",
        NUMBER,
        "The suggested provider-published unit price for a single Pricing Unit of the associated SKU, exclusive of any discounts.",
    ),
    col(
        "PricingCategory",
        STRING,
        "Describes the pricing model used for a charge at the time of use or purchase.",
    ),
    col(
        "PricingQuantity",
        NUMBER,
        "The volume of a given SKU associated with a resource or service used or purchased, based on the Pricing Unit.",
    ),
    col(
        "PricingUnit",
        STRING,
        "Provider-specified measurement unit for determining unit prices, indicating how the provider rates measured usage and purchase quantities after applying pricing rules like block pricing.",
    ),
    col(
        "ProviderName",
        STRING,
        "The name of the entity that made the resources or services available for purchase.",
    ),
    col(
        "PublisherName",
        STRING,
        "The name of the entity that produced the resources or services that were purchased.",
    ),
    col(
        "RegionId",
        STRING,
        "Provider-assigned identifier for an isolated geographic area where a resource is provisioned or a service is provided.",
    ),
    col(
        "RegionName",
        STRING,
        "The name of an isolated geographic area where a resource is provisioned or a service is provided.",
    ),
    col(
        "ResourceId",
        STRING,
        "Identifier assigned to a resource by the provider.",
    ),
    col(
        "ResourceName",
        STRING,
        "Display name assigned to a resource.",
    ),
    col(
        "ResourceType",
        STRING,
        "The kind of resource for which you are being charged.",
    ),
    col(
        "ServiceCategory",
        STRING,
        "Highest-level classification of a service based on the core function of the service.",
    ),
    col(
        "ServiceName",
        STRING,
        "An offering that can be purchased from a provider (for example, cloud virtual machine, SaaS database, professional services from a systems integrator).",
    ),
    col(
        "SkuId",
        STRING,
        "A unique identifier that defines a provider-supported construct for organizing properties that are common across one or more SKU Prices.",
    ),
    col(
        "SkuPriceId",
        STRING,
        "A unique identifier that defines the unit price used to calculate the charge.",
    ),
    col(
        "SubAccountId",
        STRING,
        "An ID assigned to a grouping of resources or services, often used to manage access and/or cost.",
    ),
    col(
        "SubAccountName",
        STRING,
        "A name assigned to a grouping of resources or services, often used to manage access and/or cost.",
    ),
    col(
        "Tags",
        MAP,
        "The set of tags assigned to a resource or charge, which may include both provider-defined and user-defined tags.",
    ),
];

/// The descriptions the FOCUS 1.2 specification REVISES over FOCUS 1.0's, applied by
/// [`ColumnGroup::Revised`] when the FOCUS 1.2 schema carries those columns forward. The
/// FOCUS 1.0 tables keep the text they were published with, so the same column name
/// carries different published text in the two schemas.
///
/// Two re-wordings run right through the 1.2 revision of the 1.0 columns, so they are
/// applied to every column they touch rather than only to the ones a report cited:
///
/// * a period's limits are published as its BOUNDS ("the exclusive end bound of a billing
///   period") rather than as a date and time, for each of the billing and charge period
///   bounds; and
/// * the discount a commitment buys is a "commitment discount" rather than a
///   "commitment-based discount", for every column of the commitment-discount family and
///   for the contracted unit price, which excludes those discounts by name.
const FOCUS_1_2_REVISED_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "BillingPeriodEnd",
        "The exclusive end bound of a billing period.",
    ),
    (
        "BillingPeriodStart",
        "The inclusive start bound of a billing period.",
    ),
    (
        "ChargeCategory",
        "Represents the highest-level classification of a charge based on the nature of how it is billed.",
    ),
    (
        "ChargeClass",
        "Indicates whether the row represents a correction to a previously invoiced billing period.",
    ),
    (
        "ChargePeriodEnd",
        "The exclusive end bound of a charge period.",
    ),
    (
        "ChargePeriodStart",
        "The inclusive start bound of a charge period.",
    ),
    (
        "CommitmentDiscountCategory",
        "Indicates whether the commitment discount identified in the CommitmentDiscountId column is based on usage quantity or cost (aka \"spend\").",
    ),
    (
        "CommitmentDiscountId",
        "The identifier assigned to a commitment discount by the provider.",
    ),
    (
        "CommitmentDiscountName",
        "The display name assigned to a commitment discount.",
    ),
    (
        "CommitmentDiscountStatus",
        "Indicates whether the charge corresponds with the consumption of a commitment discount or the unused portion of the committed amount.",
    ),
    (
        "CommitmentDiscountType",
        "A provider-assigned identifier for the type of commitment discount applied to the row.",
    ),
    (
        "ContractedUnitPrice",
        "The agreed-upon unit price for a single Pricing Unit of the associated SKU, inclusive of negotiated discounts, if present, while excluding negotiated commitment discounts or any other discounts.",
    ),
    (
        "EffectiveCost",
        "The amortized cost of the charge after applying all reduced rates, discounts, and the applicable portion of relevant, prepaid purchases (one-time or recurring) that covered this charge.",
    ),
];

/// The columns FOCUS 1.2 adds to the FOCUS 1.0 schema, in the dictionary's order. The
/// commitment-discount columns here are 1.2's own, so they are worded the way 1.2 words
/// that family throughout (see [`FOCUS_1_2_REVISED_DESCRIPTIONS`]).
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
        "Indicates whether the charge represents either the consumption of a capacity reservation or when a capacity reservation is unused.",
    ),
    col(
        "CommitmentDiscountQuantity",
        NUMBER,
        "The amount of a commitment discount purchased or accounted for in commitment discount related rows.",
    ),
    col(
        "CommitmentDiscountUnit",
        STRING,
        "The provider-specified measurement unit indicating the quantity of units of a commitment discount purchased or accounted for in commitment discount related rows.",
    ),
    col(
        "InvoiceId",
        STRING,
        "The unique identifier assigned to an invoice by the invoice issuer.",
    ),
    col(
        "PricingCurrency",
        STRING,
        "Represents the currency of the pricing of a charge.",
    ),
    col(
        "PricingCurrencyContractedUnitPrice",
        NUMBER,
        "Represents the contracted unit price for a single pricing unit of the associated SKU, in the pricing currency.",
    ),
    col(
        "PricingCurrencyEffectiveCost",
        NUMBER,
        "Represents the effective cost of a charge, in the pricing currency.",
    ),
    col(
        "PricingCurrencyListUnitPrice",
        NUMBER,
        "Represents the list unit price for a single pricing unit of the associated SKU, in the pricing currency.",
    ),
    col(
        "ServiceSubcategory",
        STRING,
        "Secondary classification of the service category for a service.",
    ),
    col(
        "SkuMeter",
        STRING,
        "A provider-specified description of the metric or event that a SKU measures.",
    ),
    col(
        "SkuPriceDetails",
        MAP,
        "A map column containing the provider-specified details of a SKU price.",
    ),
    col(
        "SubAccountType",
        STRING,
        "A provider-assigned name to identify the type of a sub account.",
    ),
];

/// The `x_`-prefixed provider extensions Amazon Web Services publishes beside the FOCUS
/// 1.0 columns, in the dictionary's order.
const FOCUS_AWS_EXTENSION_COLUMNS: &[ColumnSpec] = &[
    col(
        "x_CostCategories",
        MAP,
        "A map column containing key-value pairs of the cost categories that are applied to the row.",
    ),
    col(
        "x_Discounts",
        MAP,
        "A map column containing key-value pairs of any specific discounts that apply to this row.",
    ),
    col(
        "x_Operation",
        STRING,
        "The specific AWS operation covered by this row. This describes the specific usage of the row.",
    ),
    col(
        "x_ServiceCode",
        STRING,
        "The code of the AWS service.",
    ),
    col(
        "x_UsageType",
        STRING,
        "The usage details of the row.",
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

/// The columns `COST_AND_USAGE_DASHBOARD` publishes, in the dictionary's order: the
/// dimensions and measures the view aggregates the underlying cost and usage line items
/// into. The view is a CURATED schema — it does not re-publish the cost and usage table's
/// own `bill_*`/`line_item_*`/`reservation_*`/`savings_plan_*` columns, because a client
/// selecting from this table selects from these aggregate columns — but it does carry a
/// few of the viewed table's columns over under their own names, and those are named out
/// of the viewed group rather than copied here: see
/// [`COST_AND_USAGE_DASHBOARD_VIEWED_COLUMNS`].
const COST_AND_USAGE_DASHBOARD_COLUMNS: &[ColumnSpec] = &[
    col(
        "billing_period",
        TIMESTAMP,
        "The start date of the billing period that is covered by this report, in UTC. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "usage_date",
        TIMESTAMP,
        "The start date for the line item in UTC, inclusive. The format is YYYY-MM-DDTHH:mm:ssZ.",
    ),
    col(
        "payer_account_id",
        STRING,
        "The account ID of the paying account. For an organization in AWS Organizations, this is the account ID of the management account.",
    ),
    col(
        "payer_account_name",
        STRING,
        "The account name of the paying account. For an organization in AWS Organizations, this is the account name of the management account.",
    ),
    col(
        "linked_account_id",
        STRING,
        "The account ID of the account that used this line item. For organizations, this can be either the management account or a member account.",
    ),
    col(
        "linked_account_name",
        STRING,
        "The account name of the account that used this line item. For organizations, this can be either the management account or a member account.",
    ),
    col(
        "invoice_id",
        STRING,
        "The ID associated with a specific line item. Until the report is final, the InvoiceId is blank.",
    ),
    col(
        "charge_type",
        STRING,
        "The type of charge covered by this line item.",
    ),
    col(
        "charge_category",
        STRING,
        "The category of charge covered by this line item possible values are Running_Usage and non_usage.",
    ),
    col(
        "billing_entity",
        STRING,
        "Helps you identify whether your invoices or transactions are for AWS Marketplace or for purchases of other AWS services.",
    ),
    col(
        "legal_entity",
        STRING,
        "The Seller of Record of a specific product or service.",
    ),
    col(
        "service",
        STRING,
        "The AWS service that the line item applies to.",
    ),
    col(
        "product_code",
        STRING,
        "The code of the product measured.",
    ),
    col(
        "product_name",
        STRING,
        "The full name of the AWS service.",
    ),
    col(
        "product_family",
        STRING,
        "The category for the type of product.",
    ),
    col(
        "product_group",
        STRING,
        "The group of products that the line item applies to.",
    ),
    col(
        "operation",
        STRING,
        "The specific AWS operation covered by this line item. This describes the specific usage of the line item.",
    ),
    col(
        "usage_type",
        STRING,
        "The usage details of the line item.",
    ),
    col(
        "item_description",
        STRING,
        "The description of the line item type.",
    ),
    col(
        "region",
        STRING,
        "The Region that the usage occurred in.",
    ),
    col(
        "availability_zone",
        STRING,
        "The Availability Zone that hosts this line item. For example, us-east-1a or us-east-1b.",
    ),
    col(
        "instance_type",
        STRING,
        "Describes the instance type, size, and family, which define the CPU, networking, and storage capacity of your instance.",
    ),
    col(
        "instance_type_family",
        STRING,
        "The instance family that is associated with the specified usage.",
    ),
    col(
        "current_generation",
        STRING,
        "Indicates whether the instance type is of the current generation.",
    ),
    col(
        "platform",
        STRING,
        "The operating system of the instance.",
    ),
    col(
        "processor",
        STRING,
        "The processor of the instance.",
    ),
    col(
        "processor_features",
        STRING,
        "The features of the processor of the instance.",
    ),
    col(
        "database_engine",
        STRING,
        "The database engine that is running on the instance.",
    ),
    col(
        "tenancy",
        STRING,
        "The tenancy of the instance. For example, shared or dedicated.",
    ),
    col(
        "purchase_option",
        STRING,
        "The purchase option for this line item possible values are SavingsPLan, Reserved, Spot, and OnDemand.",
    ),
    col(
        "usage_quantity",
        NUMBER,
        "The amount of usage that you incurred during the specified time period.",
    ),
    col(
        "unblended_cost",
        NUMBER,
        "The UnblendedRate multiplied by the UsageAmount.",
    ),
    // The commitment a line item benefited from is a "Reservation" in this dictionary's
    // words — the cost and usage dictionary's "Reserved Instance" is that dictionary's.
    col(
        "amortized_cost",
        NUMBER,
        "The upfront and recurring costs of a Reservation or Savings Plan, distributed across the term that they apply to.",
    ),
    col(
        "public_cost",
        NUMBER,
        "The total cost for the line item based on public On-Demand Instance rates.",
    ),
    col(
        "ri_sp_arn",
        STRING,
        "The Amazon Resource Name (ARN) of the Reservation or Savings Plan that this line item benefited from.",
    ),
    col(
        "ri_sp_trueup",
        NUMBER,
        "The true-up charge for the Reservation or Savings Plan that this line item benefited from.",
    ),
    col(
        "ri_sp_upfront_fees",
        NUMBER,
        "The upfront fees for the Reservation or Savings Plan that this line item benefited from.",
    ),
];

/// The columns `COST_AND_USAGE_DASHBOARD` carries over from the cost and usage table
/// UNCHANGED, in the order the dictionary publishes them: the pricing unit the view's
/// usage quantity is measured in, and the two ends of a data transfer. They keep the
/// cost and usage name, so the view reports them exactly as the viewed table does rather
/// than with a second description of the same column — see [`ColumnGroup::View`].
const COST_AND_USAGE_DASHBOARD_VIEWED_COLUMNS: &[&str] = &[
    "pricing_unit",
    "product_from_location",
    "product_to_location",
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
        // A curated VIEW over the cost and usage data: the aggregate dimensions and
        // measures it publishes, plus the columns it carries over from the viewed table
        // under the viewed table's own names.
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
            // The FOCUS 1.0 columns carried forward, at the text the 1.2 specification
            // publishes for them …
            ColumnGroup::Revised {
                of: FOCUS_1_0_COLUMNS,
                revisions: FOCUS_1_2_REVISED_DESCRIPTIONS,
            },
            // … the columns 1.2 adds, and the extensions it still needs.
            ColumnGroup::Own(FOCUS_1_2_ADDED_COLUMNS),
            ColumnGroup::View {
                of: FOCUS_AWS_EXTENSION_COLUMNS,
                columns: FOCUS_1_2_AWS_EXTENSION_COLUMNS,
            },
        ],
    },
];
