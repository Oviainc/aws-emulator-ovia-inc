// SPDX-License-Identifier: CC0-1.0

//! The stored integration records — one resource's properties and one table's properties —
//! and the ARNs that address them.

use super::*;

/// How many integration resource property ids this process has minted.
///
/// Not catalog state — the id itself is what gets stored, on the
/// [`IntegrationResourcePropertyEntry`]. This is only a nonce source for
/// [`new_integration_resource_property_id`], so that two creates landing in the same
/// nanosecond are still given different input to mint from.
pub(super) static MINTED_INTEGRATION_RESOURCE_PROPERTY_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh id for an integration resource property record: the name its own ARN carries.
///
/// 32 lowercase hexadecimal characters from [`mint_hex_id`], the width every other id this
/// crate mints has. It is minted ONCE, when the first create for a resource stores its
/// record, and kept on that record — so every later read, and every later create for the
/// same resource, reports the same `ResourcePropertyArn`, and two resources never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_integration_resource_property_id() -> String {
    mint_hex_id(&MINTED_INTEGRATION_RESOURCE_PROPERTY_IDS, CATALOG_ID_WORDS)
}

/// The ARN resource-type segment of an integration resource property record, the way the
/// model spells it: "The format is something like arn:aws:glue:::integrationresourceproperty/*"
/// (`CreateIntegrationResourcePropertyResponse$ResourcePropertyArn`).
pub const INTEGRATION_RESOURCE_PROPERTY_ARN_SEGMENT: &str = "integrationresourceproperty";

/// The ARN of the integration resource property record `property_id`, as served in
/// `region`.
pub fn integration_resource_property_arn(region: &str, property_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{INTEGRATION_RESOURCE_PROPERTY_ARN_SEGMENT}/{property_id}"),
    )
}

/// The Glue resource an integration property call's `ResourceArn` names, or
/// `Err` with the message a refusal carries when the value cannot name one.
///
/// `ResourceArn` is documented as "The connection ARN of the source, or the database ARN of
/// the target", so a value that is not the ARN of a Glue connection or a Glue database
/// names neither and is a bad parameter. The ONE resolution of that member, so the create
/// and every read of a record agree on which values address a resource at all.
///
/// Shared by the integration TABLE property operations, whose `ResourceArn` names the same
/// two legs of an integration: "the same API need to be invoked with the Glue connection ARN
/// as ResourceArn with SourceTableConfig, and the Glue database ARN as ResourceArn with
/// TargetTableConfig respectively" (`CreateIntegrationTableProperties`). A catalog TABLE's
/// own ARN is one of the values that names neither leg.
///
/// Only the KIND and the identity fields are read here; whether this store holds the
/// resource is [`ComAmazonawsGlueAwsglueState::resource_tags`]'s answer, exactly as for a
/// tagging call's `ResourceArn`.
pub fn integration_resource_reference(resource_arn: &str) -> Result<TaggedResource, String> {
    match parse_tagged_resource(resource_arn) {
        Some(resource)
            if matches!(
                resource.kind,
                TaggedResourceKind::Connection | TaggedResourceKind::Database
            ) =>
        {
            Ok(resource)
        }
        _ => Err(format!(
            "ResourceArn {resource_arn:?} is not the ARN of a Glue connection or a Glue database"
        )),
    }
}

/// The processing properties one resource of an integration carries: the record
/// `CreateIntegrationResourceProperty` writes for a Glue connection (the source) or a Glue
/// database (the target).
///
/// Every member of the modelled `IntegrationResourceProperty` shape is held here, plus the
/// create's `Tags`, which that shape does not carry. The id is the one server-set member:
/// it is minted when the first create for a resource stores its record and never changes,
/// so the `ResourcePropertyArn` a client reads is stable across reads and across a later
/// create for the same resource.
#[derive(Debug, Clone)]
pub struct IntegrationResourcePropertyEntry {
    /// The ARN of the resource these properties belong to, which is also this record's key
    /// in [`ComAmazonawsGlueAwsglueState::integration_resource_properties`].
    pub resource_arn: String,
    /// The name this record's own ARN carries ([`integration_resource_property_arn`]),
    /// minted once by [`new_integration_resource_property_id`].
    pub property_id: String,
    /// "The resource properties associated with the integration source", as the last write
    /// that stated them sent them, or `None` while no write has.
    pub source_processing_properties: Option<SourceProcessingProperties>,
    /// "The resource properties associated with the integration target", held the same way.
    pub target_processing_properties: Option<TargetProcessingProperties>,
    /// The create's `Tags`, which the `IntegrationResourceProperty` shape has no member for.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`] gives:
    /// [`TaggedResourceKind`] carries no integration resource property variant.
    pub tags: Vec<Tag>,
}

impl IntegrationResourcePropertyEntry {
    /// The record a first `CreateIntegrationResourceProperty` for `resource_arn` stores:
    /// the properties the request carried, under a freshly minted id.
    pub fn from_create_input(input: &CreateIntegrationResourcePropertyInput) -> Self {
        Self {
            resource_arn: input.resource_arn.clone(),
            property_id: new_integration_resource_property_id(),
            source_processing_properties: input.source_processing_properties.clone(),
            target_processing_properties: input.target_processing_properties.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Apply a create for a resource this store ALREADY holds a record for: each member the
    /// request states replaces the stored one, and a member it does not state is left as
    /// stored.
    ///
    /// The source and the target properties of one integration are written by separate
    /// calls — "To set both source and target properties the same API needs to be invoked
    /// with the Glue connection ARN as ResourceArn with SourceProcessingProperties and the
    /// Glue database ARN as ResourceArn with TargetProcessingProperties respectively" — so a
    /// write that states one side says nothing about the other and must not erase it. The id
    /// is not touched: the record is the same resource's, so its ARN stays what the first
    /// create minted.
    pub fn apply_create(&mut self, input: &CreateIntegrationResourcePropertyInput) {
        if input.source_processing_properties.is_some() {
            self.source_processing_properties = input.source_processing_properties.clone();
        }
        if input.target_processing_properties.is_some() {
            self.target_processing_properties = input.target_processing_properties.clone();
        }
        if let Some(tags) = input.tags.clone() {
            self.tags = tags;
        }
    }

    /// This record's own ARN, as served in `region`.
    pub fn arn(&self, region: &str) -> String {
        integration_resource_property_arn(region, &self.property_id)
    }
}

/// The ARN resource-type segment of a Data Catalog table, whose resource portion carries
/// the table's database as well as its own name: `table/<database>/<table>`.
///
/// Not in [`TAGGED_RESOURCE_TYPES`] because a table carries no tag set of its own
/// (`CreateTable` models no `Tags` member), but it IS a value an integration table
/// property's `ResourceArn` may carry: "the provided ARN should be the ARN of the target
/// table in the Glue Data Catalog" (`CreateIntegrationTablePropertiesRequest$ResourceArn`).
pub const TABLE_ARN_SEGMENT: &str = "table";

/// The Glue resource an integration table property call's `ResourceArn` names.
///
/// Either the LEG of an integration whose table properties the call writes — the Glue
/// connection of the source or the Glue database of the target ("the same API need to be
/// invoked with the Glue connection ARN as ResourceArn with SourceTableConfig, and the Glue
/// database ARN as ResourceArn with TargetTableConfig respectively") — or a replicated
/// table's own ARN, which `CreateIntegrationTableProperties` also takes ("the provided ARN
/// should be the ARN of the target table in the Glue Data Catalog").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationTableResource {
    /// A Glue connection or a Glue database, resolved the way every call that names one by
    /// ARN resolves it ([`parse_tagged_resource`]).
    Tagged(TaggedResource),
    /// A Data Catalog table, whose ARN names its database as well as itself.
    Table {
        /// Field 5 of the ARN, which for a catalog resource is the `CatalogId` it lives
        /// under.
        account_id: String,
        /// The folded database name, the way the catalog keys it.
        database: String,
        /// The folded table name, the same way.
        table: String,
    },
}

impl IntegrationTableResource {
    /// Whether this resource is a LEG of an integration, which is what a read of a table
    /// property record addresses.
    ///
    /// Recorded behaviour: a create against a replicated table's own ARN is accepted, and
    /// `GetIntegrationTableProperties` for that same ARN is then refused — the read resolves
    /// its `ResourceArn` to the connection or the database an integration runs between, and a
    /// catalog table is neither.
    pub fn is_integration_leg(&self) -> bool {
        matches!(self, Self::Tagged(_))
    }
}

/// The Glue resource `resource_arn` names, or `Err` with the message a refusal carries when
/// the value can name no resource an integration table property could belong to.
///
/// The ONE resolution of that member, so the create and every read of a record agree on
/// which values address a resource at all. Only the KIND and the identity fields are read
/// here; whether this store holds the resource is
/// [`ComAmazonawsGlueAwsglueState::holds_integration_table_resource`]'s answer, exactly as a
/// tagging call's `ResourceArn` splits those two questions.
pub fn integration_table_reference(resource_arn: &str) -> Result<IntegrationTableResource, String> {
    if let Some(resource) = parse_tagged_resource(resource_arn) {
        if matches!(
            resource.kind,
            TaggedResourceKind::Connection | TaggedResourceKind::Database
        ) {
            return Ok(IntegrationTableResource::Tagged(resource));
        }
    }
    if let Some(parsed) = parse_glue_arn(resource_arn) {
        if let Some((database, table)) = parsed
            .resource_of_type(TABLE_ARN_SEGMENT)
            .and_then(|rest| rest.split_once('/'))
        {
            if !database.is_empty() && !table.is_empty() && !table.contains('/') {
                return Ok(IntegrationTableResource::Table {
                    account_id: parsed.account_id.clone(),
                    database: fold_name(database),
                    table: fold_name(table),
                });
            }
        }
    }
    Err(format!(
        "ResourceArn {resource_arn:?} is not the ARN of a Glue connection, a Glue database or a \
         Glue Data Catalog table"
    ))
}

/// The `SourceTableConfig` members the service does not process yet, named as the model
/// spells them.
///
/// Source: the three members the model documents as "Currently unsupported" — `Fields` ("A
/// list of fields used for column-level filtering. Currently unsupported."),
/// `FilterPredicate` ("A condition clause used for row-level filtering. Currently
/// unsupported.") and `RecordUpdateField` ("Incremental pull timestamp-based field.
/// Currently unsupported."). `PrimaryKey` is NOT one of them: it is "Currently supported
/// specifically for SAP EntityOf entities upon request".
pub const UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS: [&str; 3] =
    ["Fields", "FilterPredicate", "RecordUpdateField"];

/// Whether a source table configuration states any member of
/// [`UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS`], which is what makes a write of it a request
/// for processing the source leg does not do.
///
/// The ONE reading of that rule, so the create and any later write of a record refuse the
/// same configurations.
pub fn states_unsupported_source_table_properties(config: &SourceTableConfig) -> bool {
    config.fields.is_some()
        || config.filter_predicate.is_some()
        || config.record_update_field.is_some()
}

/// The override properties ONE replicated table carries on one leg of an integration: the
/// record `CreateIntegrationTableProperties` writes for a `ResourceArn` and a `TableName`.
///
/// Both modelled sides are held here — the source leg's filtering and partitioning
/// properties and the target leg's — because the two are written by separate calls against
/// separate `ResourceArn`s and read back together by one `GetIntegrationTableProperties`.
/// The record carries no server-set member: the model gives integration table properties no
/// id, ARN, status or timestamp of their own.
#[derive(Debug, Clone)]
pub struct IntegrationTablePropertiesEntry {
    /// The ARN of the integration leg these properties belong to, the first half of this
    /// record's key in
    /// [`ComAmazonawsGlueAwsglueState::integration_table_properties`].
    pub resource_arn: String,
    /// "The name of the table to be replicated", the second half of that key.
    pub table_name: String,
    /// "A structure for the source table configuration", as the last write that stated it
    /// sent it, or `None` while no write has.
    pub source_table_config: Option<SourceTableConfig>,
    /// "A structure for the target table configuration", held the same way.
    pub target_table_config: Option<TargetTableConfig>,
}

impl IntegrationTablePropertiesEntry {
    /// The record a first `CreateIntegrationTableProperties` for a pair stores: the
    /// configuration the request carried, for the resource and table it named.
    pub fn from_create_input(input: &CreateIntegrationTablePropertiesInput) -> Self {
        Self {
            resource_arn: input.resource_arn.clone(),
            table_name: input.table_name.clone(),
            source_table_config: input.source_table_config.clone(),
            target_table_config: input.target_table_config.clone(),
        }
    }

    /// Apply a create for a pair this store ALREADY holds a record for: the side the
    /// request states replaces the stored one, and a side it does not state is left as
    /// stored.
    ///
    /// The source and the target configuration of one table are written by separate calls —
    /// each against the ARN of its own leg — so a write that states one side says nothing
    /// about the other and must not erase it.
    pub fn apply_create(&mut self, input: &CreateIntegrationTablePropertiesInput) {
        if input.source_table_config.is_some() {
            self.source_table_config = input.source_table_config.clone();
        }
        if input.target_table_config.is_some() {
            self.target_table_config = input.target_table_config.clone();
        }
    }
}
