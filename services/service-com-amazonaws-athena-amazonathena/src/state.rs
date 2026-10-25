// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-athena-amazonathena service.
//!
//! Athena's workgroup and data-catalog registries live here. `CreateWorkGroup` writes a
//! [`WorkGroupRecord`] into [`ComAmazonawsAthenaAmazonathenaState::work_groups`] and
//! `GetWorkGroup` answers out of that same record, so every value a read reports is
//! one some earlier write persisted rather than one derived from the reading request.
//! `CreateDataCatalog` and `GetDataCatalog` stand in the same relation through
//! [`DataCatalogRecord`] and [`ComAmazonawsAthenaAmazonathenaState::data_catalogs`].
//!
//! A workgroup has no lifecycle the clock owes: Athena answers `CreateWorkGroup` with
//! the workgroup already usable, so the record is stored in its terminal state and a
//! read never has to materialise a pending transition. A data catalog has one only when
//! its creation is asynchronous (see [`DataCatalogRecord::status_at`]), and even then the
//! due instant is stored once and every read derives the status from it rather than
//! writing.
//!
//! Athena's TABLE METADATA is not kept here at all: Athena registers no tables of its
//! own, and the data catalog its metadata reads address — [`GLUE_DATA_CATALOG_NAME`], or
//! a registered `GLUE` catalog naming the same catalog id — IS the account's Glue Data
//! Catalog. So those reads project what GLUE persisted ([`to_table_metadata`]) instead of
//! holding a second copy that could drift from the catalog a client writes through.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_glue_awsglue::ops::types::Column as GlueColumn;
use ::service_com_amazonaws_glue_awsglue::state as glue_state;

use crate::ops::types::{
    Column, DataCatalog, EngineVersion, TableMetadata, Tag, WorkGroup, WorkGroupConfiguration,
    CONNECTION_TYPE_VALUES,
};

/// The `WorkGroupState` a freshly created workgroup is in. One of
/// `WORK_GROUP_STATE_VALUES` in [`crate::ops::types`] (the model's `WorkGroupState`
/// enum, whose `WorkGroup$State` documentation reads "The state of the workgroup:
/// ENABLED or DISABLED").
pub const WORK_GROUP_STATE_ENABLED: &str = "ENABLED";
/// The other `WorkGroupState`; see [`WORK_GROUP_STATE_ENABLED`].
pub const WORK_GROUP_STATE_DISABLED: &str = "DISABLED";

/// The engine version that asks Athena to choose one.
///
/// `EngineVersion$SelectedEngineVersion` documents: "Possible values are determined by
/// the output of `ListEngineVersions`, including AUTO. The default is AUTO."
pub const ENGINE_VERSION_AUTO: &str = "AUTO";

/// The engine version this emulator resolves [`ENGINE_VERSION_AUTO`] to.
///
/// `EngineVersion$EffectiveEngineVersion` documents: "If the user requests Auto, the
/// effective engine version is chosen by Athena." Athena's own documentation names
/// Athena engine version 3 as the version new workgroups run on, so that is what this
/// emulator chooses. It is NOT read from the request: a caller that asks for AUTO gets
/// this version whatever else the request says.
pub const DEFAULT_EFFECTIVE_ENGINE_VERSION: &str = "Athena engine version 3";

/// How many tags one Athena resource carries.
///
/// Athena's own documentation states the limit: "You can have up to 50 tags per
/// resource." The model's `TagList` carries no length trait, so the limit is enforced
/// here, and a longer list is refused BEFORE anything walks or stores it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The largest caller-supplied open collection a stored workgroup configuration may
/// hold, in bytes of keys and values (see [`open_configuration_bytes`]).
///
/// This is the EMULATOR's own bound and has NO AWS counterpart: Athena publishes no
/// limit on these members, so the number is chosen generously — far above any plausible
/// Spark property set, engine classification list or CloudWatch log-type map — and
/// exists only so that [`MAX_WORK_GROUPS`] times the size of one record is a bounded
/// product rather than one bounded factor times an unbounded one. Most
/// `WorkGroupConfiguration` members are already bounded by the model (a 1024-byte
/// description, a 128-byte `AdditionalConfiguration`, a 2048-byte `ExecutionRole`); its
/// Spark properties, engine classifications and CloudWatch log types are the open maps
/// and lists the model bounds in neither length nor element count. A configuration past
/// this bound is refused with the operation's own `InvalidRequestException`.
pub const MAX_WORK_GROUP_CONFIGURATION_BYTES: usize = 64 * 1024;

/// How many workgroups one account holds in this emulator.
///
/// Athena's published service quota is 1000 workgroups per Region per account; the
/// quota is not carried in the model, so this bound is enforced here and a write past
/// it is refused with the operation's own `InvalidRequestException` rather than
/// growing the emulator's heap without limit.
pub const MAX_WORK_GROUPS: usize = 1000;

/// One workgroup, as this service persists it.
///
/// The configuration is held already normalised (see
/// [`resolved_engine_version`]), so the create path decides the engine version once
/// and every later read reports that same decision.
#[derive(Debug, Clone)]
pub struct WorkGroupRecord {
    /// The workgroup name, which is also its key in the registry.
    pub name: String,
    /// A `WorkGroupState` value: [`WORK_GROUP_STATE_ENABLED`] or
    /// [`WORK_GROUP_STATE_DISABLED`].
    pub state: String,
    /// The description the create call carried, if any.
    pub description: Option<String>,
    /// The workgroup configuration, with the engine version resolved.
    pub configuration: WorkGroupConfiguration,
    /// When the workgroup was created, reported as `WorkGroup$CreationTime`.
    pub created_at: SystemTime,
    /// The tags the create call carried; keys are unique within this list.
    pub tags: Vec<Tag>,
}

impl WorkGroupRecord {
    /// The record as the modelled `WorkGroup` shape a read answers with.
    pub fn to_work_group(&self) -> WorkGroup {
        WorkGroup {
            name: self.name.clone(),
            state: Some(self.state.clone()),
            configuration: Some(self.configuration.clone()),
            description: self.description.clone(),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // Athena fills this only for an IAM Identity Center enabled workgroup,
            // whose Identity Center application this emulator does not create.
            identity_center_application_arn: None,
        }
    }
}

/// The engine version a workgroup runs on, given the one the request selected.
///
/// `EffectiveEngineVersion` is documented "Read only" and "When a request to update the
/// engine version is made by a `CreateWorkGroup` or `UpdateWorkGroup` operation, the
/// `EffectiveEngineVersion` field is ignored" — so whatever the caller put there is
/// dropped, and the effective version is the selected one, or
/// [`DEFAULT_EFFECTIVE_ENGINE_VERSION`] when the selection is
/// [`ENGINE_VERSION_AUTO`] (or absent, AUTO being the documented default).
pub fn resolved_engine_version(requested: Option<&EngineVersion>) -> EngineVersion {
    let selected = requested
        .and_then(|version| version.selected_engine_version.clone())
        .unwrap_or_else(|| ENGINE_VERSION_AUTO.to_string());
    let effective = if selected == ENGINE_VERSION_AUTO {
        DEFAULT_EFFECTIVE_ENGINE_VERSION.to_string()
    } else {
        selected.clone()
    };
    EngineVersion {
        selected_engine_version: Some(selected),
        effective_engine_version: Some(effective),
    }
}

/// Whether any tag in `tags` carries no key.
///
/// `Tag$Key` is 1 to 128 characters, so a present key is already checked at the
/// boundary; the member itself is optional in the model, and a tag with no key at all
/// names nothing to tag. Refused on its own terms, before keys are compared for
/// uniqueness.
pub fn tag_without_key(tags: &[Tag]) -> bool {
    tags.iter()
        .any(|tag| tag.key.as_deref().unwrap_or_default().is_empty())
}

/// The first tag key that appears more than once in `tags`, if any.
///
/// `Tag$Key` documents "Tag keys are case-sensitive and must be unique per resource",
/// so a tag list carrying one key twice describes no resource Athena would accept.
/// Linear in the list length: the keys already seen are held in a set, never rescanned.
pub fn first_duplicate_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(tags.len());
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if !seen.insert(key) {
            return Some(key);
        }
    }
    None
}

/// The bytes of keys and values held in `configuration`'s OPEN collections — the ones
/// the model bounds neither in length nor in element count: the engine configuration's
/// additional configs, Spark properties and classifications, and the CloudWatch logging
/// configuration's log types. Checked against
/// [`MAX_WORK_GROUP_CONFIGURATION_BYTES`] before a configuration is stored.
///
/// Linear in the size of the request it measures, and it allocates nothing.
pub fn open_configuration_bytes(configuration: &WorkGroupConfiguration) -> usize {
    let mut bytes = 0usize;
    if let Some(engine) = configuration.engine_configuration.as_ref() {
        for map in [
            engine.additional_configs.as_ref(),
            engine.spark_properties.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            for (key, value) in map {
                bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
            }
        }
        for classification in engine.classifications.iter().flatten() {
            bytes = bytes
                .saturating_add(classification.name.as_ref().map_or(0, String::len));
            for (key, value) in classification.properties.iter().flatten() {
                bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
            }
        }
    }
    let log_types = configuration
        .monitoring_configuration
        .as_ref()
        .and_then(|monitoring| monitoring.cloud_watch_logging_configuration.as_ref())
        .and_then(|logging| logging.log_types.as_ref());
    for (key, values) in log_types.into_iter().flatten() {
        bytes = bytes.saturating_add(key.len());
        for value in values {
            bytes = bytes.saturating_add(value.len());
        }
    }
    bytes
}

/// The name Athena addresses the account's Glue Data Catalog by.
///
/// Athena's own documentation names it `AwsDataCatalog`, and a data catalog a client
/// registers of its own cannot take that name (`CreateDataCatalog`'s `Name` leaves "the
/// remainder of the length constraint of 256 ... reserved for use by Athena"), so this
/// one name always means the Glue Data Catalog.
pub const GLUE_DATA_CATALOG_NAME: &str = "AwsDataCatalog";

/// Whether `catalog_name` names the Glue Data Catalog ([`GLUE_DATA_CATALOG_NAME`]).
///
/// Matched without regard to ASCII case: Athena addresses this same catalog as
/// `awsdatacatalog` from its query layer, where every identifier is lowercased, so a
/// client that spells it that way addresses the catalog it means.
pub fn is_glue_data_catalog(catalog_name: &str) -> bool {
    catalog_name.eq_ignore_ascii_case(GLUE_DATA_CATALOG_NAME)
}

/// The `CatalogId` the Glue Data Catalog of THIS account is held under in Glue's store:
/// Glue's own default, which is the account id ("If none is provided, the Amazon Web
/// Services account ID is used by default"). Athena's `AwsDataCatalog` is that catalog,
/// so a metadata read resolves it through Glue's own rule rather than restating it.
pub fn glue_data_catalog_id() -> String {
    glue_state::resolve_catalog_id(None)
}

/// The refusal message for a read whose named data catalog this account does not hold.
///
/// NOT recorded: what is pinned is the behaviour — HTTP 400 with the operation's own
/// `InvalidRequestException`, naming a catalog being a problem with the input rather than
/// one the metastore reported — and the sentence names the kind of entity and the
/// requested name, the way this service's other per-resource misses do (`GetWorkGroup`'s
/// "WorkGroup {name} is not found."). ONE sentence for the class, so every operation that
/// answers it words it identically.
pub fn data_catalog_not_found(catalog_name: &str) -> String {
    format!("DataCatalog {catalog_name} is not found.")
}

/// The refusal message for a metadata read whose `DatabaseName` names no database the
/// addressed data catalog holds. The miss is the METASTORE's answer, which is why it is
/// reported as `MetadataException` rather than as a bad request, and the sentence is the
/// Hive-metastore wording for an absent namespace, built from the requested name.
pub fn database_not_found(database_name: &str) -> String {
    format!("Database does not exist: {database_name}")
}

/// The refusal message for a metadata read whose `TableName` names no table the
/// addressed database holds. The per-kind counterpart of [`database_not_found`]: the same
/// metastore answers it, so it is the same `MetadataException` and follows the same
/// template, built from the requested name.
pub fn table_not_found(table_name: &str) -> String {
    format!("Table does not exist: {table_name}")
}

/// The refusal message for a listing whose `Expression` carries more wildcards than the
/// catalog matches against every stored name
/// ([`glue_state::MAX_PATTERN_WILDCARDS`]). Beside the other refusal sentences of this
/// slice, so a reader of the pattern language is told the same thing wherever it is
/// matched.
pub fn too_many_pattern_wildcards(wildcards: usize) -> String {
    format!(
        "Expression carries {wildcards} wildcards, more than the {} this catalog matches",
        glue_state::MAX_PATTERN_WILDCARDS
    )
}

/// One stored Glue table as the `TableMetadata` Athena reports for it.
///
/// Every member is the catalog's, never the reading request's: the name, the instant the
/// catalog recorded the create, the last access time the catalog holds, the table type,
/// the table's columns (its storage descriptor's), its partition keys and its table
/// properties ([`hive_table_properties`]). A member the catalog does not determine for
/// that table stays absent, so a client reads "unknown" only where the catalog knows
/// nothing — a table stored with no storage descriptor has no column list to report,
/// which is not the same as a table whose column list is empty.
///
/// `Parameters` is the ONE member always reported: a Hive table's property set is
/// something the catalog always determines for it — every stored member of the table's
/// storage descriptor contributes one, and a table that names none simply has an empty
/// set — so there is no "unknown" state for it to stand in for.
pub fn to_table_metadata(table: &glue_state::TableEntry) -> TableMetadata {
    TableMetadata {
        name: table.name.clone(),
        create_time: Some(::aws_smithy_types::DateTime::from(table.create_time)),
        last_access_time: table.last_access_time,
        table_type: table.table_type.clone(),
        columns: table
            .storage_descriptor
            .as_ref()
            .and_then(|storage| storage.columns.as_ref())
            .map(|columns| columns.iter().map(to_column).collect()),
        partition_keys: table
            .partition_keys
            .as_ref()
            .map(|keys| keys.iter().map(to_column).collect()),
        parameters: Some(hive_table_properties(table)),
    }
}

/// The Hive table property that carries where a table's data lives: the stored
/// `StorageDescriptor$Location`.
const LOCATION_PROPERTY: &str = "location";

/// The Hive table property carrying `StorageDescriptor$InputFormat`.
const INPUT_FORMAT_PROPERTY: &str = "inputformat";

/// The Hive table property carrying `StorageDescriptor$OutputFormat`.
const OUTPUT_FORMAT_PROPERTY: &str = "outputformat";

/// The Hive table property carrying the serialization library of the stored
/// `SerDeInfo`.
const SERDE_LIBRARY_PROPERTY: &str = "serde.serialization.lib";

/// The prefix each `SerDeInfo$Parameters` entry is carried under, one Hive table
/// property per entry: `serde.param.<name>`.
const SERDE_PARAMETER_PREFIX: &str = "serde.param.";

/// A stored Data Catalog table's properties, as the `TableMetadata$Parameters`
/// ("a set of custom key/value pairs for table properties") Athena reports for it.
///
/// Athena presents a Data Catalog table as the HIVE table it is, so this is one flat
/// property set rather than the structured definition the catalog holds: the table's own
/// `Parameters`, plus its STORAGE DESCRIPTOR rendered as the Hive properties that
/// describe it — where the data lives, the input and output formats, the serialization
/// library and each of the SerDe's own parameters under
/// [`SERDE_PARAMETER_PREFIX`]. Athena's `TableMetadata` models no storage descriptor of
/// its own, and these properties are how a client reads one back.
///
/// A member the catalog does not hold contributes no property, so the set says exactly
/// what the stored definition carries. A property derived from the storage descriptor
/// wins over a table parameter of the same name: it is what the catalog actually holds
/// for that aspect of the table.
pub fn hive_table_properties(
    table: &glue_state::TableEntry,
) -> ::std::collections::HashMap<String, String> {
    let mut properties = table.parameters.clone().unwrap_or_default();
    let Some(storage) = table.storage_descriptor.as_ref() else {
        return properties;
    };
    for (property, value) in [
        (LOCATION_PROPERTY, storage.location.as_ref()),
        (INPUT_FORMAT_PROPERTY, storage.input_format.as_ref()),
        (OUTPUT_FORMAT_PROPERTY, storage.output_format.as_ref()),
    ] {
        if let Some(value) = value {
            properties.insert(property.to_string(), value.clone());
        }
    }
    let Some(serde) = storage.serde_info.as_ref() else {
        return properties;
    };
    if let Some(library) = serde.serialization_library.as_ref() {
        properties.insert(SERDE_LIBRARY_PROPERTY.to_string(), library.clone());
    }
    for (name, value) in serde.parameters.iter().flatten() {
        properties.insert(format!("{SERDE_PARAMETER_PREFIX}{name}"), value.clone());
    }
    properties
}

/// One column of a stored Glue table as Athena's own `Column` shape.
///
/// Athena's column carries the three members it models — the name, the data type and the
/// comment — and the Data Catalog's per-column `Parameters`, which Athena's shape does
/// not model, are left behind rather than folded into another member.
fn to_column(column: &GlueColumn) -> Column {
    Column {
        name: column.name.clone(),
        r#type: column.r#type.clone(),
        comment: column.comment.clone(),
    }
}

/// Where the metadata of the data catalog a read names comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataSource {
    /// A Glue Data Catalog this emulator holds, under this `CatalogId` — the account's
    /// own, which a registered `GLUE` catalog may name a second time.
    GlueDataCatalog(String),
    /// A registered catalog whose metadata an external metastore or a connector function
    /// serves, carrying that catalog's `DataCatalogType`.
    ExternalMetastore(String),
    /// A registered `GLUE` catalog naming the Data Catalog of ANOTHER account, carrying
    /// that `CatalogId`. The obstacle is the catalog it names rather than an external
    /// metastore: this emulator holds the Data Catalog of its own account only.
    ForeignGlueCatalog(String),
    /// No data catalog of that name: neither the Glue Data Catalog nor one this account
    /// registered.
    Unregistered,
}

/// Which metadata source `catalog_name` addresses, given the `catalogs` this account
/// registered.
///
/// [`GLUE_DATA_CATALOG_NAME`] is the account's Glue Data Catalog. A catalog a client
/// registered is resolved from its OWN stored record: a `GLUE` catalog reads the Glue Data
/// Catalog the `catalog-id` parameter its type documents names ([`PARAMETER_CATALOG_ID`])
/// — so one naming the catalog this emulator holds is that same catalog under a second
/// Athena name, while one naming another account's names a Data Catalog this emulator
/// does not hold ([`MetadataSource::ForeignGlueCatalog`]); a `HIVE`, `LAMBDA` or
/// `FEDERATED` catalog reads through an external Hive metastore or a connector function,
/// which this emulator does not host.
pub fn metadata_source(
    catalogs: &BTreeMap<String, DataCatalogRecord>,
    catalog_name: &str,
) -> MetadataSource {
    let glue_catalog_id = glue_data_catalog_id();
    if is_glue_data_catalog(catalog_name) {
        return MetadataSource::GlueDataCatalog(glue_catalog_id);
    }
    let Some(record) = catalogs.get(catalog_name) else {
        return MetadataSource::Unregistered;
    };
    if record.catalog_type != DATA_CATALOG_TYPE_GLUE {
        return MetadataSource::ExternalMetastore(record.catalog_type.clone());
    }
    match record.parameters.get(PARAMETER_CATALOG_ID) {
        Some(catalog_id) if *catalog_id == glue_catalog_id => {
            MetadataSource::GlueDataCatalog(glue_catalog_id)
        }
        // A GLUE catalog carries that parameter (its type requires it), and one naming
        // any other id names a Data Catalog of another account.
        Some(catalog_id) => MetadataSource::ForeignGlueCatalog(catalog_id.clone()),
        None => MetadataSource::ExternalMetastore(record.catalog_type.clone()),
    }
}

/// The refusal message for a metadata read of a registered catalog whose metastore this
/// emulator does not host.
///
/// Athena reads such a catalog by calling that catalog's own metastore or connector
/// function, so a metastore that answers nothing is what `MetadataException` reports; the
/// sentence names the catalog the request addressed and the type that decides where its
/// metadata lives.
pub fn metastore_unreachable(catalog_name: &str, catalog_type: &str) -> String {
    format!(
        "Catalog {catalog_name} is a {catalog_type} data catalog, whose metadata is served \
         by a metastore this emulator does not host."
    )
}

/// The refusal message for a metadata read of a registered `GLUE` catalog that names the
/// Data Catalog of another account.
///
/// The counterpart of [`metastore_unreachable`] for the other obstacle: the metastore is
/// the Glue Data Catalog, and the catalog this one names is one this emulator does not
/// hold, so the sentence names the catalog the request addressed and the `CatalogId` it
/// points at.
pub fn foreign_glue_catalog(catalog_name: &str, catalog_id: &str) -> String {
    format!(
        "Catalog {catalog_name} names the Data Catalog {catalog_id}, which is not a Data \
         Catalog this account holds."
    )
}

/// The `DataCatalogType` a federated catalog carries — the one type whose creation the
/// `DataCatalog$Status` documentation calls asynchronous. One of
/// `DATA_CATALOG_TYPE_VALUES` in [`crate::ops::types`].
pub const DATA_CATALOG_TYPE_FEDERATED: &str = "FEDERATED";
/// The `DataCatalogType` of an Glue Data Catalog; see [`DATA_CATALOG_TYPE_FEDERATED`].
pub const DATA_CATALOG_TYPE_GLUE: &str = "GLUE";
/// The `DataCatalogType` of an external Apache Hive metastore; see
/// [`DATA_CATALOG_TYPE_FEDERATED`].
pub const DATA_CATALOG_TYPE_HIVE: &str = "HIVE";
/// The `DataCatalogType` of a Lambda-backed catalog; see
/// [`DATA_CATALOG_TYPE_FEDERATED`].
pub const DATA_CATALOG_TYPE_LAMBDA: &str = "LAMBDA";

/// The `DataCatalogStatus` of a catalog whose creation is still running.
///
/// `DataCatalog$Status` documents "`CREATE_IN_PROGRESS`: Federated data catalog creation
/// in progress." One of `DATA_CATALOG_STATUS_VALUES` in [`crate::ops::types`].
pub const DATA_CATALOG_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// The `DataCatalogStatus` of a created catalog: `DataCatalog$Status` documents
/// "`CREATE_COMPLETE`: Data catalog creation complete."
pub const DATA_CATALOG_STATUS_CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long a federated catalog stays [`DATA_CATALOG_STATUS_CREATE_IN_PROGRESS`] before
/// it is [`DATA_CATALOG_STATUS_CREATE_COMPLETE`].
///
/// `DataCatalog$Status` documents that the `FEDERATED` type "is created asynchronously",
/// so the catalog is handed back in a non-terminal status and the completion is a
/// transition the CLOCK owes. AWS publishes no duration for it (it builds a stack, a
/// Lambda function and a connection), so this span is the emulator's own choice: short
/// enough that a caller is not kept waiting, long enough that a caller reading straight
/// back still observes the in-progress status.
pub const FEDERATED_DATA_CATALOG_CREATION: Duration = Duration::from_millis(500);

/// How many characters a data catalog name may use.
///
/// `CreateDataCatalogInput$Name` documents: "The catalog name must be unique for the
/// Amazon Web Services account and can use a maximum of 127 alphanumeric, underscore, at
/// sign, or hyphen characters. The remainder of the length constraint of 256 is reserved
/// for use by Athena." The model bounds `CatalogNameString` at 256, so this tighter
/// documented bound is enforced here.
pub const MAX_DATA_CATALOG_NAME_CHARS: usize = 127;

/// How many characters a FEDERATED data catalog name may use.
///
/// `CreateDataCatalogInput$Name` documents: "since we use it to create other resources
/// that allow less characters and we prepend a prefix to it, the actual catalog name
/// limit for `FEDERATED` catalog is 64 - 23 = 41 characters."
pub const MAX_FEDERATED_DATA_CATALOG_NAME_CHARS: usize = 41;

/// The characters a data catalog name may use beside the alphanumeric ones, from the
/// `CreateDataCatalogInput$Name` documentation quoted on
/// [`MAX_DATA_CATALOG_NAME_CHARS`]: underscore, at sign and hyphen.
pub const DATA_CATALOG_NAME_PUNCTUATION: [char; 3] = ['_', '@', '-'];

/// How many data catalogs one account holds in this emulator.
///
/// This is the EMULATOR's own bound and has no counterpart in the model: it is chosen
/// far above any plausible set of registered catalogs and exists only so that the
/// registry cannot grow without limit. A write past it is refused with the operation's
/// own `InvalidRequestException`.
pub const MAX_DATA_CATALOGS: usize = 1000;

/// The largest `Parameters` map a stored data catalog may hold, in bytes of keys and
/// values (see [`parameters_bytes`]).
///
/// Another bound with no AWS counterpart: the model bounds one `ParametersMap` value at
/// 51200 bytes and one key at 255, but not how many entries the map holds, so this is
/// what keeps [`MAX_DATA_CATALOGS`] times the size of one record a bounded product. It
/// is well above the handful of entries a catalog of any type documents, including a
/// federated catalog's escaped-JSON connection properties.
pub const MAX_DATA_CATALOG_PARAMETERS_BYTES: usize = 128 * 1024;

/// The `Parameters` key a HIVE or LAMBDA catalog names its metadata function with.
///
/// Every key below is from the `DataCatalog$Parameters` documentation, which gives the
/// syntax per catalog type.
pub const PARAMETER_METADATA_FUNCTION: &str = "metadata-function";
/// The `Parameters` key a LAMBDA catalog names its record function with.
pub const PARAMETER_RECORD_FUNCTION: &str = "record-function";
/// The `Parameters` key a LAMBDA catalog names its composite function with.
pub const PARAMETER_FUNCTION: &str = "function";
/// The `Parameters` key a GLUE catalog names its catalog id with.
pub const PARAMETER_CATALOG_ID: &str = "catalog-id";
/// The `Parameters` key a FEDERATED catalog reuses an existing Glue connection with.
pub const PARAMETER_CONNECTION_ARN: &str = "connection-arn";
/// The `Parameters` key a FEDERATED catalog states a new connection's type with.
pub const PARAMETER_CONNECTION_TYPE: &str = "connection-type";

/// One data catalog, as this service persists it.
///
/// The status is NOT stored: it follows from [`create_complete_at`](Self::create_complete_at),
/// so a read derives it from the clock instead of a reader having to move it.
#[derive(Debug, Clone)]
pub struct DataCatalogRecord {
    /// The catalog name, which is also its key in the registry.
    pub name: String,
    /// A `DataCatalogType` value: one of [`DATA_CATALOG_TYPE_FEDERATED`],
    /// [`DATA_CATALOG_TYPE_GLUE`], [`DATA_CATALOG_TYPE_HIVE`] or
    /// [`DATA_CATALOG_TYPE_LAMBDA`].
    pub catalog_type: String,
    /// The description the create call carried, if any.
    pub description: Option<String>,
    /// The `Parameters` the create call carried. A `BTreeMap` so the stored map has one
    /// stable order whatever order the request listed it in.
    pub parameters: BTreeMap<String, String>,
    /// The `ConnectionType` of a federated catalog that states one, read once from
    /// [`PARAMETER_CONNECTION_TYPE`] at create time.
    pub connection_type: Option<String>,
    /// When this catalog's creation is complete. For a synchronously created type this
    /// is the instant of the create call itself; for a federated one it is
    /// [`FEDERATED_DATA_CATALOG_CREATION`] after it.
    pub create_complete_at: SystemTime,
    /// The tags the create call carried; keys are unique within this list.
    pub tags: Vec<Tag>,
}

impl DataCatalogRecord {
    /// The `DataCatalogStatus` this catalog is in at `now`.
    ///
    /// `DataCatalog$Status` documents the `LAMBDA`, `GLUE` and `HIVE` types as created
    /// synchronously and the `FEDERATED` type as created asynchronously, so a catalog is
    /// [`DATA_CATALOG_STATUS_CREATE_IN_PROGRESS`] until its completion is due and
    /// [`DATA_CATALOG_STATUS_CREATE_COMPLETE`] from then on. Derived, never written: two
    /// reads at the same instant report the same status.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.create_complete_at {
            DATA_CATALOG_STATUS_CREATE_IN_PROGRESS
        } else {
            DATA_CATALOG_STATUS_CREATE_COMPLETE
        }
    }

    /// The record as the modelled `DataCatalog` shape a read answers with, as of `now`.
    pub fn to_data_catalog(&self, now: SystemTime) -> DataCatalog {
        DataCatalog {
            name: self.name.clone(),
            description: self.description.clone(),
            r#type: self.catalog_type.clone(),
            parameters: if self.parameters.is_empty() {
                None
            } else {
                Some(
                    self.parameters
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
            },
            status: Some(self.status_at(now).to_string()),
            connection_type: self.connection_type.clone(),
            // `DataCatalog$Error` is the "text of the error that occurred during data
            // catalog creation or deletion"; nothing here fails, so there is none.
            error: None,
        }
    }
}

/// When a catalog of `catalog_type` created at `now` is complete.
///
/// `DataCatalog$Status` documents the `LAMBDA`, `GLUE` and `HIVE` types as created
/// synchronously — complete as the create call returns — and the `FEDERATED` type as
/// created asynchronously, which is due [`FEDERATED_DATA_CATALOG_CREATION`] later. The
/// addition is checked, so a clock far enough in the future to overflow yields a catalog
/// that is already complete rather than a panic.
pub fn create_complete_at(catalog_type: &str, now: SystemTime) -> SystemTime {
    if catalog_type == DATA_CATALOG_TYPE_FEDERATED {
        now.checked_add(FEDERATED_DATA_CATALOG_CREATION).unwrap_or(now)
    } else {
        now
    }
}

/// Why `name` is not a usable data catalog name for `catalog_type`, if it is not.
///
/// Enforces the two documented bounds quoted on [`MAX_DATA_CATALOG_NAME_CHARS`] and
/// [`MAX_FEDERATED_DATA_CATALOG_NAME_CHARS`] plus the documented character set, none of
/// which the model's `CatalogNameString` carries.
pub fn data_catalog_name_error(name: &str, catalog_type: &str) -> Option<String> {
    let limit = if catalog_type == DATA_CATALOG_TYPE_FEDERATED {
        MAX_FEDERATED_DATA_CATALOG_NAME_CHARS
    } else {
        MAX_DATA_CATALOG_NAME_CHARS
    };
    let length = name.chars().count();
    if length > limit {
        return Some(format!(
            "Name is {length} characters long, more than the {limit} characters a \
             {catalog_type} data catalog name can use."
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|ch| !ch.is_ascii_alphanumeric() && !DATA_CATALOG_NAME_PUNCTUATION.contains(ch))
    {
        return Some(format!(
            "Name contains the character '{bad}'; a data catalog name can use \
             alphanumeric, underscore, at sign, or hyphen characters."
        ));
    }
    None
}

/// Why `parameters` does not describe a catalog of `catalog_type`, if it does not.
///
/// `DataCatalog$Parameters` is "a mapping whose values depend on the catalog type" and
/// states, per type, which keys are required and which sets are mutually exclusive; a
/// map that satisfies none of those sets names no catalog Athena could register. A type
/// the documentation states no requirement for constrains nothing here.
pub fn data_catalog_parameters_error(
    catalog_type: &str,
    parameters: &HashMap<String, String>,
) -> Option<String> {
    let has = |key: &str| parameters.contains_key(key);
    match catalog_type {
        // "For the HIVE data catalog type ... The metadata-function parameter is
        // required."
        DATA_CATALOG_TYPE_HIVE if !has(PARAMETER_METADATA_FUNCTION) => Some(format!(
            "Parameters for a {DATA_CATALOG_TYPE_HIVE} data catalog must contain \
             {PARAMETER_METADATA_FUNCTION}."
        )),
        // "For the LAMBDA data catalog type, use one of the following sets of required
        // parameters, but not both": metadata-function with record-function, or
        // function on its own.
        DATA_CATALOG_TYPE_LAMBDA => {
            let composite = has(PARAMETER_FUNCTION);
            let split = has(PARAMETER_METADATA_FUNCTION) && has(PARAMETER_RECORD_FUNCTION);
            let partial_split = has(PARAMETER_METADATA_FUNCTION) || has(PARAMETER_RECORD_FUNCTION);
            if composite && partial_split {
                Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_LAMBDA} data catalog must contain \
                     either {PARAMETER_FUNCTION} or {PARAMETER_METADATA_FUNCTION} with \
                     {PARAMETER_RECORD_FUNCTION}, but not both."
                ))
            } else if composite || split {
                None
            } else {
                Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_LAMBDA} data catalog must contain \
                     either {PARAMETER_FUNCTION} or both {PARAMETER_METADATA_FUNCTION} and \
                     {PARAMETER_RECORD_FUNCTION}."
                ))
            }
        }
        // "The GLUE type takes a catalog ID parameter and is required."
        DATA_CATALOG_TYPE_GLUE if !has(PARAMETER_CATALOG_ID) => Some(format!(
            "Parameters for a {DATA_CATALOG_TYPE_GLUE} data catalog must contain \
             {PARAMETER_CATALOG_ID}."
        )),
        // "The FEDERATED data catalog type uses one of the following parameters, but not
        // both": connection-arn, or connection-type (with connection-properties).
        DATA_CATALOG_TYPE_FEDERATED => {
            match (has(PARAMETER_CONNECTION_ARN), has(PARAMETER_CONNECTION_TYPE)) {
                (true, true) => Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_FEDERATED} data catalog must contain \
                     either {PARAMETER_CONNECTION_ARN} or {PARAMETER_CONNECTION_TYPE}, but \
                     not both."
                )),
                (false, false) => Some(format!(
                    "Parameters for a {DATA_CATALOG_TYPE_FEDERATED} data catalog must contain \
                     either {PARAMETER_CONNECTION_ARN} or {PARAMETER_CONNECTION_TYPE}."
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The `ConnectionType` a federated catalog's `Parameters` state, and whether it is one
/// the model's `ConnectionType` enum holds.
///
/// `DataCatalog$ConnectionType` is that enum and `DataCatalog$Parameters` writes the
/// parameter as `connection-type:MYSQL|REDSHIFT|....`, so a value outside the enum is
/// one no read could report. `Ok(None)` means the parameter is absent, which is what a
/// catalog reusing an existing connection, or one of any other type, looks like.
pub fn federated_connection_type(
    catalog_type: &str,
    parameters: &HashMap<String, String>,
) -> Result<Option<String>, String> {
    if catalog_type != DATA_CATALOG_TYPE_FEDERATED {
        return Ok(None);
    }
    match parameters.get(PARAMETER_CONNECTION_TYPE) {
        None => Ok(None),
        Some(value) if CONNECTION_TYPE_VALUES.contains(&value.as_str()) => Ok(Some(value.clone())),
        Some(value) => Err(format!(
            "Parameters names the connection type '{value}', which is not a supported \
             connection type. Supported connection types are {}.",
            CONNECTION_TYPE_VALUES.join(", ")
        )),
    }
}

/// The bytes of keys and values held in a `ParametersMap`, checked against
/// [`MAX_DATA_CATALOG_PARAMETERS_BYTES`] before the map is stored.
///
/// Linear in the size of the map it measures, and it allocates nothing.
pub fn parameters_bytes(parameters: &HashMap<String, String>) -> usize {
    parameters
        .iter()
        .fold(0usize, |bytes, (key, value)| {
            bytes.saturating_add(key.len()).saturating_add(value.len())
        })
}

/// Per-service state for com-amazonaws-athena-amazonathena. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAthenaAmazonathenaState {
    /// Every workgroup this account holds, keyed by its name. A `BTreeMap` so a
    /// listing walks the registry in one stable order.
    pub work_groups: BTreeMap<String, WorkGroupRecord>,
    /// Every data catalog this account holds, keyed by its name — the key the
    /// `CreateDataCatalogInput$Name` documentation calls unique per account. A
    /// `BTreeMap` for the same reason as [`Self::work_groups`].
    pub data_catalogs: BTreeMap<String, DataCatalogRecord>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAthenaAmazonathenaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAthenaAmazonathenaState>)
}
