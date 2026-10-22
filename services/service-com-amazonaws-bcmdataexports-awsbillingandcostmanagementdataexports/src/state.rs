// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports service.
//!
//! The service's one durable resource is the data export: created by `CreateExport`,
//! read back by `GetExport` / `ListExports` / `ListTagsForResource`, overwritten by
//! `UpdateExport` and removed by `DeleteExport`. Every one of those handlers computes
//! its answer from [`StoredExport`] values held here, so a create is observable by every
//! later reader and a delete is observable by none.
//!
//! The ARN/region/token/timestamp helpers below live beside the store rather than in any
//! one operation file because several operations mint or parse exactly the same values,
//! and two copies of an ARN rule is how a `GetExport` stops finding what `CreateExport`
//! stored.
//!
//! Beside the store there is also the TABLE CATALOG ([`TABLE_CATALOG`]): the tables an
//! account can query, their table properties and their column schemas. That catalog is
//! fixed service data — the same for every account, and nothing a caller does changes it —
//! so it is a static here rather than a field of the state struct: there is nothing to
//! persist and nothing to mutate, and `GetTable` (with `ListTables`, which reports the
//! same entries) must answer the same catalog on every call. It lives beside the store,
//! not inside one operation file, for the same reason the ARN rule does: two operations
//! read it.

use crate::ops::types::{
    Column, Export, ExportStatus, ResourceNotFoundException, ResourceTag, ValidationException,
    ValidationExceptionField,
};
use std::collections::{BTreeMap, HashMap};

/// This service's SigV4 signing name, which is also its ARN namespace (see
/// `OPERATIONS.md`: "sigv4 signing name: bcm-data-exports", "ARN namespace:
/// bcm-data-exports").
pub const SIGNING_NAME: &str = "bcm-data-exports";

/// The region an ARN carries when the request is not signed, so no SigV4 credential
/// scope names one. Matches the region every generated emulator's shared config
/// defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `ResourceType` a `ResourceNotFoundException` reports for an export ARN that
/// names nothing: the Smithy shape the ARN identifies.
pub const EXPORT_RESOURCE_TYPE: &str = "Export";

/// The `ExportStatusCode` reported for an export that is operating normally. One of
/// `ExportStatusCode`'s two modelled values (see
/// `crate::ops::types::EXPORT_STATUS_CODE_VALUES`: `HEALTHY`, `UNHEALTHY`).
pub const EXPORT_STATUS_HEALTHY: &str = "HEALTHY";

/// The most exports this emulator retains, after which `CreateExport` answers the
/// modelled `ServiceQuotaExceededException` (HTTP 402, "You've reached the limit on the
/// number of resources you can create, or exceeded the size of an individual resource").
///
/// AWS enforces a per-account export quota through that same error; the published number
/// is not cited here, so this value is the EMULATOR's choice — set well above any
/// plausible test or demo workload. Its job is to bound the caller-driven growth of
/// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`],
/// which lives for the life of the process: without it, repeated `CreateExport` calls
/// grow the store without limit.
pub const MAX_EXPORTS: usize = 1000;

/// The `ServiceCode` a quota failure reports: this service's endpoint prefix.
pub const SERVICE_CODE: &str = SIGNING_NAME;

/// The quota a `ServiceQuotaExceededException` from `CreateExport` names.
pub const EXPORTS_PER_ACCOUNT_QUOTA_CODE: &str = "ExportsPerAccount";

/// One persisted data export: the submitted definition, the server-owned status, and
/// the tags `CreateExport` associated with it.
#[derive(Debug, Clone)]
pub struct StoredExport {
    /// The server-minted ARN, which is also this export's key in
    /// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`].
    pub arn: String,
    /// The export definition as submitted, with `ExportArn` set to [`Self::arn`].
    pub export: Export,
    /// Server-owned status and timestamps.
    pub status: ExportStatus,
    /// The tags associated with this export, keyed by tag key so a key is unique.
    pub tags: BTreeMap<String, String>,
}

/// Per-service state for com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports. A handler reaches it through [`service_state`].
///
/// There is ONE store, and no second index beside it: the ARN [`export_arn`] mints is a
/// function of the region and `Export.Name`, so the key of this map already IS the name
/// index, and `CreateExport` refuses a name in use by looking the minted ARN up here. A
/// second map keyed by the bare name would also refuse a name already used in a
/// DIFFERENT region, which the ARN says is a different export.
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState {
    /// The primary store, keyed by the minted export ARN. `BTreeMap` so every list
    /// walks the exports in one stable order and a page token can name a position in it.
    pub exports: BTreeMap<String, StoredExport>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState>)
}

/// The current wall-clock instant, as the modelled `Timestamp` type.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// The region an ARN minted for this request carries: the region of the request's
/// SigV4 credential scope, else [`DEFAULT_REGION`].
///
/// A scope region that could not appear in the modelled `Arn` pattern's region segment
/// (`[-a-z0-9]*`) is not used, so a minted ARN always satisfies the pattern the model
/// declares for it.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region_segment(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` is a legal region segment of the modelled `Arn` pattern
/// `^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`.
fn is_arn_region_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The ARN of the export called `name` in `region`:
/// `arn:aws:bcm-data-exports:<region>:<account>:export/<name>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the one canonical
/// emulator account id rather than a literal.
pub fn export_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("export/{name}"))
}

/// The modelled 404 for an export ARN that names nothing.
pub fn export_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("No export found for the Amazon Resource Name (ARN) {arn}."),
        resource_id: arn.to_string(),
        resource_type: EXPORT_RESOURCE_TYPE.to_string(),
    }
}

/// The status a freshly created export carries.
pub fn fresh_status(at: ::aws_smithy_types::DateTime) -> ExportStatus {
    ExportStatus {
        status_code: Some(EXPORT_STATUS_HEALTHY.to_string()),
        status_reason: None,
        created_at: Some(at),
        last_updated_at: Some(at),
        // No export file has been generated yet: there is nothing to report a
        // last-refresh instant for until an execution delivers one.
        last_refreshed_at: None,
    }
}

/// Bytes of the integrity tag an opaque page token carries ahead of its position.
const PAGE_TOKEN_TAG_LEN: usize = 8;

/// Domain separator for the page-token tag, so a token minted for this service's listings
/// cannot be read as anything else.
const PAGE_TOKEN_DOMAIN: &[u8] = b"bcm-data-exports/page-token/v1\0";

/// Which listing a page token belongs to. A token is bound to its listing as well as to
/// its position, because the two paginated listings page over different key spaces
/// (`ListExports` over export ARNs, `ListTagsForResource` over an export's tag keys): a
/// token handed to the wrong listing names a position in a sequence it was never cut
/// from, and must be refused rather than resumed at some unrelated offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenKind {
    /// `ListExports`, paging over the export ARNs in [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`].
    Exports,
    /// `ListTagsForResource`, paging over one export's tag keys.
    ResourceTags,
}

impl PageTokenKind {
    /// The bytes that distinguish this listing inside the tag's preimage.
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Exports => b"Exports\0",
            Self::ResourceTags => b"ResourceTags\0",
        }
    }
}

/// The integrity tag of `key` for `kind`: the leading [`PAGE_TOKEN_TAG_LEN`] bytes of
/// SHA-256 over the service domain separator, the listing, and the key.
fn page_token_tag(kind: PageTokenKind, key: &str) -> Vec<u8> {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(PAGE_TOKEN_DOMAIN);
    hasher.update(kind.domain());
    hasher.update(key.as_bytes());
    hasher.finalize()[..PAGE_TOKEN_TAG_LEN].to_vec()
}

/// A page token naming the key the NEXT page of `kind` starts at: an integrity tag
/// followed by the key, hex-encoded. Opaque — a client cannot read a position out of it
/// and must hand it back verbatim — and self-verifying, so a token this listing never
/// issued is rejected instead of being read as some arbitrary position in it.
fn encode_page_token(kind: PageTokenKind, key: &str) -> String {
    let mut bytes = page_token_tag(kind, key);
    bytes.extend_from_slice(key.as_bytes());
    ::hex::encode(bytes)
}

/// The key a token issued by [`encode_page_token`] for `kind` names, or `None` when this
/// listing never issued it: not hex, too short to carry a tag, not UTF-8, or carrying a
/// tag that does not match the listing and position it claims.
fn decode_page_token(kind: PageTokenKind, token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    if bytes.len() < PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (tag, key) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let key = String::from_utf8(key.to_vec()).ok()?;
    (page_token_tag(kind, &key) == tag).then_some(key)
}

/// One page of `items` (ordered by their keys): at most `max_results` values starting at
/// the key `token` names, plus the token that resumes immediately after the page.
///
/// `None` when `token` was not issued by the `kind` listing — the caller answers that
/// with the modelled `ValidationException`. The returned token is `None` on the last
/// page, so a paginating client stops.
pub fn page<T: Clone>(
    kind: PageTokenKind,
    items: &[(String, T)],
    max_results: Option<i32>,
    token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match token {
        Some(token) if !token.is_empty() => {
            let key = decode_page_token(kind, token)?;
            items.partition_point(|(item_key, _)| item_key.as_str() < key.as_str())
        }
        _ => 0,
    };
    // An absent MaxResults means "no caller-imposed limit". The modelled range (1..300)
    // is already enforced at the boundary, so a present one is at least 1; the floor
    // keeps that true here independently of boundary validation, because a zero-sized
    // page handed out with a resume token would make a paginating client loop forever.
    let limit = max_results.map_or(items.len(), |max| max.max(1) as usize);
    let end = start.saturating_add(limit).min(items.len());
    let values = items[start..end]
        .iter()
        .map(|(_, value)| value.clone())
        .collect();
    let next = items
        .get(end)
        .map(|(key, _)| encode_page_token(kind, key));
    Some((values, next))
}

/// The tags of an export as the modelled list, ordered by key.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<ResourceTag> {
    tags.iter()
        .map(|(key, value)| ResourceTag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The data-export table catalog
// ---------------------------------------------------------------------------
//
// SOURCE of every name, type, property, valid-value list and default below: the AWS
// Data Exports table dictionary, which `ListTables` mirrors and `GetTable` reports
// ("To view the table properties available for each table, see the Data Exports table
// dictionary or use the ListTables API to get a response of all tables and their
// available properties" — the modelled doc for `CreateExport`'s `Export`, quoted in
// `src/ops/create_export.rs`). It is service data, not account data: see this module's
// header for why it is a static rather than a field of the state struct.

/// The `ValidationExceptionReason` a refusal of one named input member reports. One of
/// the modelled values (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`:
/// `cannotParse`, `fieldValidationFailed`, `other`, `unknownOperation`).
pub const VALIDATION_REASON_FIELD_FAILED: &str = "fieldValidationFailed";

/// The most characters of a caller-supplied value a refusal message echoes.
///
/// `TableName` admits 1024 characters and a table property value 16384 (smithy_query
/// shapes `TableName`, `TableProperties`), while `ValidationException$Message` admits
/// only 1024 — so an echoed value is clipped to keep a refusal built from an outsized
/// one inside the message's own modelled bound.
const MESSAGE_VALUE_CLIP: usize = 128;

/// `value` as a refusal message quotes it: itself when short, else its first
/// [`MESSAGE_VALUE_CLIP`] characters and an ellipsis.
fn clip_for_message(value: &str) -> String {
    if value.chars().count() <= MESSAGE_VALUE_CLIP {
        return value.to_string();
    }
    let head: String = value.chars().take(MESSAGE_VALUE_CLIP).collect();
    format!("{head}...")
}

/// A column's `Type` as the dictionary publishes it.
const STRING: &str = "STRING";
/// A column's `Type` as the dictionary publishes it.
const DOUBLE: &str = "DOUBLE";
/// A column's `Type` as the dictionary publishes it.
const TIMESTAMP: &str = "TIMESTAMP";
/// A column's `Type` as the dictionary publishes it.
const BOOLEAN: &str = "BOOLEAN";
/// A column's `Type` as the dictionary publishes it: the string-keyed maps that carry
/// tags, product attributes, cost categories and discounts.
const STRING_MAP: &str = "MAP<STRING,STRING>";

/// The value a boolean-valued table property takes to switch its columns ON.
pub const PROPERTY_TRUE: &str = "TRUE";
/// The value a boolean-valued table property takes to switch its columns OFF.
pub const PROPERTY_FALSE: &str = "FALSE";

/// The valid values of a boolean-valued table property, in the order the catalog
/// publishes them.
const BOOLEAN_PROPERTY_VALUES: &[&str] = &[PROPERTY_TRUE, PROPERTY_FALSE];

/// Table property: the time granularity of each `COST_AND_USAGE_REPORT` line item.
pub const PROPERTY_TIME_GRANULARITY: &str = "TIME_GRANULARITY";
/// Table property: whether the schema carries the per-resource columns.
pub const PROPERTY_INCLUDE_RESOURCES: &str = "INCLUDE_RESOURCES";
/// Table property: whether the schema carries the split-cost-allocation columns.
pub const PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA: &str =
    "INCLUDE_SPLIT_COST_ALLOCATION_DATA";
/// Table property: whether the schema carries the legacy bundled/total discount columns.
pub const PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY: &str =
    "INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY";
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
    /// The values this property accepts; anything else is refused.
    pub valid_values: &'static [&'static str],
    /// The value the property takes when a request does not name it.
    pub default_value: &'static str,
    /// What the property changes about the table's data or schema.
    pub description: &'static str,
}

/// One column of one table's schema.
#[derive(Debug, Clone, Copy)]
pub struct ColumnSpec {
    /// The column name.
    pub name: &'static str,
    /// The kind of data the column stores, as the dictionary publishes it.
    pub data_type: &'static str,
    /// The `(property, value)` pair that has to hold for this column to be part of the
    /// schema, or `None` for a column every schema of this table carries. This is what
    /// makes a table property "change the data and schema of a table" (the modelled doc
    /// for `GetTableRequest$TableProperties`).
    pub requires: Option<(&'static str, &'static str)>,
}

/// A column present in every schema of its table.
const fn col(name: &'static str, data_type: &'static str) -> ColumnSpec {
    ColumnSpec {
        name,
        data_type,
        requires: None,
    }
}

/// A column present only while `property` holds `value`.
const fn col_when(
    name: &'static str,
    data_type: &'static str,
    property: &'static str,
    value: &'static str,
) -> ColumnSpec {
    ColumnSpec {
        name,
        data_type,
        requires: Some((property, value)),
    }
}

/// One table in the catalog: the name a request names it by, its description, the table
/// properties it publishes, and its columns.
#[derive(Debug, Clone, Copy)]
pub struct TableSpec {
    /// The table name, matched exactly: the catalog publishes each one as a fixed
    /// identifier, so a name differing in case or spelling names no table.
    pub name: &'static str,
    /// What the table holds.
    pub description: &'static str,
    /// The table properties this table publishes, ordered by name.
    pub properties: &'static [TablePropertySpec],
    /// Every column this table can carry, including the ones a table property gates.
    pub columns: &'static [ColumnSpec],
}

impl TableSpec {
    /// This table's property called `name`, if it publishes one.
    pub fn property(&self, name: &str) -> Option<&'static TablePropertySpec> {
        self.properties.iter().find(|property| property.name == name)
    }

    /// The properties in force for a request carrying `requested`: every property this
    /// table publishes, at the requested value where the request names it and at the
    /// published default otherwise ("Each table property has a default value that it
    /// assumes if not specified" — the modelled doc for
    /// `GetTableRequest$TableProperties`).
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
            .map(|property| (property.name.to_string(), property.default_value.to_string()))
            .collect();
        for (name, value) in requested.into_iter().flatten() {
            let property = self
                .property(name)
                .ok_or_else(|| unknown_table_property(self, name))?;
            if !property.valid_values.contains(&value.as_str()) {
                return Err(invalid_table_property_value(property, value));
            }
            effective.insert(name.clone(), value.clone());
        }
        Ok(effective)
    }

    /// This table's schema under `properties`: its columns in catalog order, less the
    /// ones whose gating property does not hold the value they need.
    pub fn schema(&self, properties: &BTreeMap<String, String>) -> Vec<Column> {
        self.columns
            .iter()
            .filter(|column| match column.requires {
                None => true,
                Some((property, value)) => {
                    properties.get(property).map(String::as_str) == Some(value)
                }
            })
            .map(|column| Column {
                name: Some(column.name.to_string()),
                r#type: Some(column.data_type.to_string()),
                // The dictionary's per-column prose is not reproduced in this emulator,
                // and `Column$Description` is an optional member — so it is left absent
                // rather than filled with invented text.
                description: None,
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
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
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
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "TableProperties".to_string(),
            message: format!("This table supports the table properties [{supported}]."),
        }]),
    }
}

/// The modelled 400 for a `TableProperties` value outside the property's valid values.
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
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "TableProperties".to_string(),
            message: format!("{} accepts the values [{valid}].", property.name),
        }]),
    }
}

/// The table properties of `COST_AND_USAGE_REPORT`, ordered by name.
const COST_AND_USAGE_REPORT_PROPERTIES: &[TablePropertySpec] = &[
    TablePropertySpec {
        name: PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: PROPERTY_FALSE,
        description: "Whether the table carries the bundled and total discount columns of the legacy Cost and Usage Report.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_RESOURCES,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: PROPERTY_FALSE,
        description: "Whether the table carries the individual resource ID and resource tags of each line item.",
    },
    TablePropertySpec {
        name: PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: PROPERTY_FALSE,
        description: "Whether the table carries the split cost allocation data of shared resources.",
    },
    TablePropertySpec {
        name: PROPERTY_TIME_GRANULARITY,
        valid_values: &["HOURLY", "DAILY", "MONTHLY"],
        default_value: "HOURLY",
        description: "The time granularity of each line item in the table.",
    },
];

/// The columns of `COST_AND_USAGE_REPORT` (Cost and Usage Report 2.0), in the
/// dictionary's order. The gated ones are exactly the columns a table property adds.
const COST_AND_USAGE_REPORT_COLUMNS: &[ColumnSpec] = &[
    col("bill_bill_type", STRING),
    col("bill_billing_entity", STRING),
    col("bill_billing_period_end_date", TIMESTAMP),
    col("bill_billing_period_start_date", TIMESTAMP),
    col("bill_invoice_id", STRING),
    col("bill_invoicing_entity", STRING),
    col("bill_payer_account_id", STRING),
    col("bill_payer_account_name", STRING),
    col("cost_category", STRING_MAP),
    col("discount", STRING_MAP),
    col_when(
        "discount_bundled_discount",
        DOUBLE,
        PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY,
        PROPERTY_TRUE,
    ),
    col_when(
        "discount_total_discount",
        DOUBLE,
        PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY,
        PROPERTY_TRUE,
    ),
    col("identity_line_item_id", STRING),
    col("identity_time_interval", STRING),
    col("line_item_availability_zone", STRING),
    col("line_item_blended_cost", DOUBLE),
    col("line_item_blended_rate", STRING),
    col("line_item_currency_code", STRING),
    col("line_item_legal_entity", STRING),
    col("line_item_line_item_description", STRING),
    col("line_item_line_item_type", STRING),
    col("line_item_net_unblended_cost", DOUBLE),
    col("line_item_net_unblended_rate", STRING),
    col("line_item_normalization_factor", DOUBLE),
    col("line_item_normalized_usage_amount", DOUBLE),
    col("line_item_operation", STRING),
    col("line_item_product_code", STRING),
    col_when(
        "line_item_resource_id",
        STRING,
        PROPERTY_INCLUDE_RESOURCES,
        PROPERTY_TRUE,
    ),
    col("line_item_tax_type", STRING),
    col("line_item_unblended_cost", DOUBLE),
    col("line_item_unblended_rate", STRING),
    col("line_item_usage_account_id", STRING),
    col("line_item_usage_account_name", STRING),
    col("line_item_usage_amount", DOUBLE),
    col("line_item_usage_end_date", TIMESTAMP),
    col("line_item_usage_start_date", TIMESTAMP),
    col("line_item_usage_type", STRING),
    col("pricing_currency", STRING),
    col("pricing_lease_contract_length", STRING),
    col("pricing_offering_class", STRING),
    col("pricing_public_on_demand_cost", DOUBLE),
    col("pricing_public_on_demand_rate", STRING),
    col("pricing_purchase_option", STRING),
    col("pricing_rate_code", STRING),
    col("pricing_rate_id", STRING),
    col("pricing_term", STRING),
    col("pricing_unit", STRING),
    col("product", STRING_MAP),
    col("product_comment", STRING),
    col("product_fee_code", STRING),
    col("product_fee_description", STRING),
    col("product_from_location", STRING),
    col("product_from_location_type", STRING),
    col("product_from_region_code", STRING),
    col("product_instance_family", STRING),
    col("product_instance_type", STRING),
    col("product_instancesku", STRING),
    col("product_location", STRING),
    col("product_location_type", STRING),
    col("product_operation", STRING),
    col("product_pricing_unit", STRING),
    col("product_product_family", STRING),
    col("product_region_code", STRING),
    col("product_servicecode", STRING),
    col("product_sku", STRING),
    col("product_to_location", STRING),
    col("product_to_location_type", STRING),
    col("product_to_region_code", STRING),
    col("product_usagetype", STRING),
    col("reservation_amortized_upfront_cost_for_usage", DOUBLE),
    col("reservation_amortized_upfront_fee_for_billing_period", DOUBLE),
    col("reservation_availability_zone", STRING),
    col("reservation_effective_cost", DOUBLE),
    col("reservation_end_time", STRING),
    col("reservation_modification_status", STRING),
    col("reservation_net_amortized_upfront_cost_for_usage", DOUBLE),
    col(
        "reservation_net_amortized_upfront_fee_for_billing_period",
        DOUBLE,
    ),
    col("reservation_net_effective_cost", DOUBLE),
    col("reservation_net_recurring_fee_for_usage", DOUBLE),
    col(
        "reservation_net_unused_amortized_upfront_fee_for_billing_period",
        DOUBLE,
    ),
    col("reservation_net_unused_recurring_fee", DOUBLE),
    col("reservation_net_upfront_value", DOUBLE),
    col("reservation_normalized_units_per_reservation", STRING),
    col("reservation_number_of_reservations", STRING),
    col("reservation_recurring_fee_for_usage", DOUBLE),
    col("reservation_reservation_a_r_n", STRING),
    col("reservation_start_time", STRING),
    col("reservation_subscription_id", STRING),
    col("reservation_total_reserved_normalized_units", STRING),
    col("reservation_total_reserved_units", STRING),
    col("reservation_units_per_reservation", STRING),
    col(
        "reservation_unused_amortized_upfront_fee_for_billing_period",
        DOUBLE,
    ),
    col("reservation_unused_normalized_unit_quantity", DOUBLE),
    col("reservation_unused_quantity", DOUBLE),
    col("reservation_unused_recurring_fee", DOUBLE),
    col("reservation_upfront_value", DOUBLE),
    col_when(
        "resource_tags",
        STRING_MAP,
        PROPERTY_INCLUDE_RESOURCES,
        PROPERTY_TRUE,
    ),
    col(
        "savings_plan_amortized_upfront_commitment_for_billing_period",
        DOUBLE,
    ),
    col("savings_plan_end_time", STRING),
    col("savings_plan_instance_type_family", STRING),
    col(
        "savings_plan_net_amortized_upfront_commitment_for_billing_period",
        DOUBLE,
    ),
    col(
        "savings_plan_net_recurring_commitment_for_billing_period",
        DOUBLE,
    ),
    col("savings_plan_net_savings_plan_effective_cost", DOUBLE),
    col("savings_plan_offering_type", STRING),
    col("savings_plan_payment_option", STRING),
    col("savings_plan_purchase_term", STRING),
    col(
        "savings_plan_recurring_commitment_for_billing_period",
        DOUBLE,
    ),
    col("savings_plan_region", STRING),
    col("savings_plan_savings_plan_a_r_n", STRING),
    col("savings_plan_savings_plan_effective_cost", DOUBLE),
    col("savings_plan_savings_plan_rate", DOUBLE),
    col("savings_plan_start_time", STRING),
    col("savings_plan_total_commitment_to_date", DOUBLE),
    col("savings_plan_used_commitment", DOUBLE),
    col_when(
        "split_line_item_actual_usage",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_net_split_cost",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_net_unused_cost",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_parent_resource_id",
        STRING,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_public_on_demand_split_cost",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_public_on_demand_unused_cost",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_reserved_usage",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_cost",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_usage",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_split_usage_ratio",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
    col_when(
        "split_line_item_unused_cost",
        DOUBLE,
        PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
        PROPERTY_TRUE,
    ),
];

/// The table properties of `COST_OPTIMIZATION_RECOMMENDATIONS`.
const COST_OPTIMIZATION_RECOMMENDATIONS_PROPERTIES: &[TablePropertySpec] =
    &[TablePropertySpec {
        name: PROPERTY_INCLUDE_ALL_RECOMMENDATIONS,
        valid_values: BOOLEAN_PROPERTY_VALUES,
        default_value: PROPERTY_FALSE,
        description: "Whether the table reports every recommendation for a resource, rather than only the recommendation with the highest estimated savings.",
    }];

/// The columns of `COST_OPTIMIZATION_RECOMMENDATIONS`, in the dictionary's order.
const COST_OPTIMIZATION_RECOMMENDATIONS_COLUMNS: &[ColumnSpec] = &[
    col("account_id", STRING),
    col("action_type", STRING),
    col("currency_code", STRING),
    col("current_resource_details", STRING),
    col("current_resource_summary", STRING),
    col("current_resource_type", STRING),
    col("estimated_monthly_cost_after_discount", DOUBLE),
    col("estimated_monthly_cost_before_discount", DOUBLE),
    col("estimated_monthly_savings_after_discount", DOUBLE),
    col("estimated_monthly_savings_before_discount", DOUBLE),
    col("estimated_savings_percentage_after_discount", DOUBLE),
    col("estimated_savings_percentage_before_discount", DOUBLE),
    col("implementation_effort", STRING),
    col("last_refresh_timestamp", TIMESTAMP),
    col("recommendation_id", STRING),
    col("recommendation_source", STRING),
    col("recommended_resource_details", STRING),
    col("recommended_resource_summary", STRING),
    col("recommended_resource_type", STRING),
    col("region", STRING),
    col("resource_arn", STRING),
    col("restart_needed", BOOLEAN),
    col("rollback_possible", BOOLEAN),
    col("tags", STRING_MAP),
];

/// The columns of `FOCUS_1_0_AWS`, in the dictionary's order: the FOCUS 1.0 columns plus
/// the `x_`-prefixed provider extensions AWS publishes beside them.
const FOCUS_1_0_AWS_COLUMNS: &[ColumnSpec] = &[
    col("AvailabilityZone", STRING),
    col("BilledCost", DOUBLE),
    col("BillingAccountId", STRING),
    col("BillingAccountName", STRING),
    col("BillingCurrency", STRING),
    col("BillingPeriodEnd", TIMESTAMP),
    col("BillingPeriodStart", TIMESTAMP),
    col("ChargeCategory", STRING),
    col("ChargeClass", STRING),
    col("ChargeDescription", STRING),
    col("ChargeFrequency", STRING),
    col("ChargePeriodEnd", TIMESTAMP),
    col("ChargePeriodStart", TIMESTAMP),
    col("CommitmentDiscountCategory", STRING),
    col("CommitmentDiscountId", STRING),
    col("CommitmentDiscountName", STRING),
    col("CommitmentDiscountStatus", STRING),
    col("CommitmentDiscountType", STRING),
    col("ConsumedQuantity", DOUBLE),
    col("ConsumedUnit", STRING),
    col("ContractedCost", DOUBLE),
    col("ContractedUnitPrice", DOUBLE),
    col("EffectiveCost", DOUBLE),
    col("InvoiceIssuerName", STRING),
    col("ListCost", DOUBLE),
    col("ListUnitPrice", DOUBLE),
    col("PricingCategory", STRING),
    col("PricingQuantity", DOUBLE),
    col("PricingUnit", STRING),
    col("ProviderName", STRING),
    col("PublisherName", STRING),
    col("RegionId", STRING),
    col("RegionName", STRING),
    col("ResourceId", STRING),
    col("ResourceName", STRING),
    col("ResourceType", STRING),
    col("ServiceCategory", STRING),
    col("ServiceName", STRING),
    col("SkuId", STRING),
    col("SkuPriceId", STRING),
    col("SubAccountId", STRING),
    col("SubAccountName", STRING),
    col("Tags", STRING_MAP),
    col("x_CostCategories", STRING_MAP),
    col("x_Discounts", STRING_MAP),
    col("x_Operation", STRING),
    col("x_ServiceCode", STRING),
    col("x_UsageType", STRING),
];

/// Every table data exports publishes, ordered by name — the one catalog `GetTable`
/// reports an entry of and `ListTables` lists.
pub const TABLE_CATALOG: &[TableSpec] = &[
    TableSpec {
        name: "COST_AND_USAGE_REPORT",
        description: "The Cost and Usage Report table contains the most detailed set of cost and usage data available for an account, with one row per line item.",
        properties: COST_AND_USAGE_REPORT_PROPERTIES,
        columns: COST_AND_USAGE_REPORT_COLUMNS,
    },
    TableSpec {
        name: "COST_OPTIMIZATION_RECOMMENDATIONS",
        description: "The Cost Optimization Recommendations table contains the cost optimization recommendations available for an account, with one row per recommendation.",
        properties: COST_OPTIMIZATION_RECOMMENDATIONS_PROPERTIES,
        columns: COST_OPTIMIZATION_RECOMMENDATIONS_COLUMNS,
    },
    TableSpec {
        name: "FOCUS_1_0_AWS",
        description: "The FOCUS 1.0 table contains cost and usage data in the FinOps Open Cost and Usage Specification 1.0 schema, with the Amazon Web Services provider extensions.",
        // The catalog publishes no table properties for this table, so every request
        // for it reports the same schema.
        properties: &[],
        columns: FOCUS_1_0_AWS_COLUMNS,
    },
];
