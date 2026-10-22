// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-keyspaces-keyspacesservice service.
//!
//! Amazon Keyspaces is a two-level namespace: a region holds keyspaces, and a
//! keyspace holds tables (and user-defined types, which this slice does not yet
//! store). Both levels live in the one [`ComAmazonawsKeyspacesKeyspacesserviceState`]
//! below so every operation reads and writes ONE copy of a resource.
//!
//! Keyed by [`std::collections::BTreeMap`] at both levels, because `ListKeyspaces`
//! and `ListTables` are `@paginated`: a page boundary is only resumable if the
//! iteration order is total and stable, which a hash map's is not.
//!
//! This module also holds what more than one operation needs — ARN construction,
//! the request's region, the modelled defaults, and [`settle`][ComAmazonawsKeyspacesKeyspacesserviceState::settle],
//! which materialises the status transitions the clock already owes — so the
//! per-operation files never carry a second copy of any of it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CapacitySpecification, ClientSideTimestamps, Comment, EncryptionSpecification,
    PointInTimeRecovery, ResourceNotFoundException, SchemaDefinition, Tag, TimeToLive,
};

/// The `cassandra` ARN namespace / SigV4 signing name of Amazon Keyspaces
/// (`awsService.arnNamespace` in this service's Smithy model).
pub const ARN_NAMESPACE: &str = "cassandra";

/// The region an unsigned request is served in. A signed request carries its own
/// region in the SigV4 credential scope; see [`request_region`].
pub const DEFAULT_REGION: &str = "us-east-1";

/// `TableStatus` = `CREATING`: what `CreateTable` puts a new table in.
///
/// The three constants below are members of the model's `TableStatus` value set
/// (`crate::ops::types::TABLE_STATUS_VALUES`).
pub const TABLE_STATUS_CREATING: &str = "CREATING";
/// `TableStatus` = `ACTIVE`: the status a created table settles into.
pub const TABLE_STATUS_ACTIVE: &str = "ACTIVE";
/// `TableStatus` = `DELETING`: what `DeleteTable` puts a table in until the
/// deletion completes.
pub const TABLE_STATUS_DELETING: &str = "DELETING";

/// How long a table stays in a transient status before the clock owes it the
/// next one: `CREATING` -> `ACTIVE` after `CreateTable`, and `DELETING` -> gone
/// after `DeleteTable`.
///
/// CHOSEN, not documented: Amazon Keyspaces publishes no duration for either
/// transition, only that they happen (`CreateTable`: "the status of the table is
/// set to `CREATING`. You can start using a table when the status is `ACTIVE`";
/// `DeleteTable`: "the specified table is in the `DELETING` state until Amazon
/// Keyspaces completes the deletion"). It is deliberately NON-ZERO, so a caller
/// that reads straight after the call still observes the transient status the
/// model documents, and short enough that a test can wait it out.
pub const TRANSIENT_STATUS_SPAN: Duration = Duration::from_millis(500);

/// The throughput mode a table gets when `CreateTable` carries no
/// `capacitySpecification` ("The default is `throughput_mode:PAY_PER_REQUEST`",
/// `CapacitySpecification$throughputMode`).
pub const DEFAULT_THROUGHPUT_MODE: &str = "PAY_PER_REQUEST";

/// The throughput mode that requires both capacity units as input.
pub const PROVISIONED_THROUGHPUT_MODE: &str = "PROVISIONED";

/// The encryption type a table gets when `CreateTable` carries no
/// `encryptionSpecification` ("The default is `type:AWS_OWNED_KMS_KEY`",
/// `EncryptionSpecification$type`).
pub const DEFAULT_ENCRYPTION_TYPE: &str = "AWS_OWNED_KMS_KEY";

/// The encryption type that requires `kmsKeyIdentifier` as input.
pub const CUSTOMER_MANAGED_ENCRYPTION_TYPE: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// Point-in-time recovery is off unless `CreateTable` asks for it.
pub const DEFAULT_POINT_IN_TIME_RECOVERY_STATUS: &str = "DISABLED";

/// `PointInTimeRecoveryStatus` = `ENABLED`.
pub const POINT_IN_TIME_RECOVERY_ENABLED: &str = "ENABLED";

/// `rs` (replication strategy) = `SINGLE_REGION`: a keyspace replicated only in
/// the region it was created in, and the strategy a keyspace gets when
/// `CreateKeyspace` carries no `replicationSpecification`.
pub const SINGLE_REGION_STRATEGY: &str = "SINGLE_REGION";

/// `rs` (replication strategy) = `MULTI_REGION`.
pub const MULTI_REGION_STRATEGY: &str = "MULTI_REGION";

/// The most keyspaces this emulator holds, and — with [`MAX_TABLES_PER_KEYSPACE`] — the
/// pair of caps that bound caller-driven growth of the store.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create is the only thing that adds one, so the store needs a maximum entry count:
/// neither a TTL nor an eviction policy is available to it, because `GetTable` must still
/// answer what `CreateTable` created. The two operations that grow the store refuse past
/// these caps with the service's own modelled `ServiceQuotaExceededException` (HTTP 402),
/// which this model declares on exactly the two create operations and on no read. The
/// caps also bound the work [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`] can do
/// while holding the state lock.
///
/// The numbers are the emulator's own bound, not a contract: Amazon Keyspaces publishes
/// both quotas as adjustable, so no single value is the published one; these are the
/// order of magnitude of the defaults and are far above any ordinary workflow.
pub const MAX_KEYSPACES: usize = 256;

/// The most tables one keyspace holds. See [`MAX_KEYSPACES`].
pub const MAX_TABLES_PER_KEYSPACE: usize = 256;

/// The most tags one Amazon Keyspaces resource holds.
///
/// PUBLISHED by the service, not chosen here: the `Tag` shape's own documentation
/// (`smithy_query(query="shape",shape="Tag")`) says "A tag is a key-value pair. You can
/// add up to 50 tags to a single Amazon Keyspaces resource."
///
/// It is a different number from the modelled `TagList` bound (`len 1..60`), which caps
/// what ONE REQUEST may carry: a request well inside the modelled bound can still carry a
/// resource past this quota, and the request boundary cannot see that because it depends
/// on the tags the resource already holds. Such a call is refused with the
/// `ServiceQuotaExceededException` (HTTP 402) that every operation taking tags models.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The prefix reserved for Amazon Web Services' own tag keys.
///
/// From the `Tag` shape's documentation: "Amazon Web Services-assigned tag names and
/// values are automatically assigned the `aws:` prefix, which the user cannot assign."
/// So a caller may neither apply nor remove a key carrying it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// A resource's tags, keyed by `Tag$key`.
///
/// The KEY identifies a tag, not the pair: "Each Amazon Keyspaces resource can only have
/// up to one tag with the same key. If you try to add an existing tag (same key), the
/// existing tag value will be updated to the new value" (`Tag$key`). A
/// [`std::collections::BTreeMap`] for the same reason the keyspace and table maps are
/// one: `ListTagsForResource` is `@paginated`, and a page boundary is only resumable if
/// the iteration order is total and stable.
pub type TagSet = BTreeMap<String, String>;

/// Why a tag a caller supplied was refused. Each variant maps onto the modelled error
/// the operations that take tags declare for it, which is why this is not a bare string:
/// a reserved key is a 400 `ValidationException` and a breached quota a 402
/// `ServiceQuotaExceededException`.
#[derive(Debug, Clone)]
pub enum TagRejection {
    /// A key the caller may not assign ([`RESERVED_TAG_KEY_PREFIX`]).
    ReservedKeyPrefix {
        /// The key as supplied.
        key: String,
    },
    /// The call would carry the resource past [`MAX_TAGS_PER_RESOURCE`].
    QuotaExceeded {
        /// How many tags the resource would have held.
        resulting: usize,
    },
}

impl TagRejection {
    /// What the caller has to fix, naming the member at fault and the rule it broke.
    pub fn message(&self) -> String {
        match self {
            Self::ReservedKeyPrefix { key } => format!(
                "tags contains key '{key}': the '{RESERVED_TAG_KEY_PREFIX}' prefix is reserved for Amazon Web Services and cannot be assigned."
            ),
            Self::QuotaExceeded { resulting } => format!(
                "the resource would hold {resulting} tags, which exceeds the limit of {MAX_TAGS_PER_RESOURCE} tags per resource"
            ),
        }
    }

    /// Whether this refusal is the quota one, so a handler picks
    /// `ServiceQuotaExceededException` over `ValidationException`.
    pub fn is_quota(&self) -> bool {
        matches!(self, Self::QuotaExceeded { .. })
    }
}

/// Merge `incoming` into the tag set `existing`, as `TagResource` does and as a create
/// does onto the empty set.
///
/// A key the set already holds is an UPDATE of its value, not a second tag (`Tag$key`),
/// so it does not count towards [`MAX_TAGS_PER_RESOURCE`] — and neither does a key the
/// request repeats. The resulting size is therefore computed before anything is stored,
/// which is also what makes a refused call leave the tag set exactly as it was.
pub fn merge_tags(existing: &mut TagSet, incoming: &[Tag]) -> Result<(), TagRejection> {
    let mut added: BTreeSet<&str> = BTreeSet::new();
    for tag in incoming {
        if tag.key.starts_with(RESERVED_TAG_KEY_PREFIX) {
            return Err(TagRejection::ReservedKeyPrefix {
                key: tag.key.clone(),
            });
        }
        if !existing.contains_key(&tag.key) {
            added.insert(tag.key.as_str());
        }
    }
    let resulting = existing.len() + added.len();
    if resulting > MAX_TAGS_PER_RESOURCE {
        return Err(TagRejection::QuotaExceeded { resulting });
    }
    for tag in incoming {
        existing.insert(tag.key.clone(), tag.value.clone());
    }
    Ok(())
}

/// The tag set a create operation stores for the `tags` it was given: the same rules as
/// [`merge_tags`], applied to a resource that holds none yet.
pub fn new_tag_set(supplied: Option<&[Tag]>) -> Result<TagSet, TagRejection> {
    let mut tags = TagSet::new();
    merge_tags(&mut tags, supplied.unwrap_or_default())?;
    Ok(tags)
}

/// Which resource an Amazon Keyspaces ARN names: the two resource forms this service
/// mints ([`keyspace_arn`], [`table_arn`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// `arn:<partition>:cassandra:<region>:<account>:/keyspace/<name>/`.
    Keyspace {
        /// The keyspace the ARN names.
        keyspace_name: String,
    },
    /// `arn:<partition>:cassandra:<region>:<account>:/keyspace/<ks>/table/<tbl>`.
    Table {
        /// The owning keyspace.
        keyspace_name: String,
        /// The table the ARN names.
        table_name: String,
    },
}

/// The resource an Amazon Keyspaces ARN names, or `None` when it names none this service
/// has a resource form for.
///
/// The ARN's shape is modelled only as `^arn:(aws[a-zA-Z0-9-]*):cassandra:.+` (`ARN`), so
/// everything after the service segment is parsed here. A keyspace and a table name
/// cannot contain `/` (both are modelled `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`), which is
/// what makes the two forms unambiguous: only the keyspace form ends in `/`.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceRef> {
    let mut segments = arn.splitn(6, ':');
    let scheme = segments.next()?;
    let partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let _account = segments.next()?;
    let resource = segments.next()?;
    if scheme != "arn" || !partition.starts_with("aws") || service != ARN_NAMESPACE {
        return None;
    }
    let rest = resource.strip_prefix("/keyspace/")?;
    match rest.strip_suffix('/') {
        Some(keyspace_name) => (!keyspace_name.is_empty() && !keyspace_name.contains('/')).then(
            || ResourceRef::Keyspace {
                keyspace_name: keyspace_name.to_string(),
            },
        ),
        None => {
            let (keyspace_name, table_name) = rest.split_once("/table/")?;
            (!keyspace_name.is_empty()
                && !keyspace_name.contains('/')
                && !table_name.is_empty()
                && !table_name.contains('/'))
            .then(|| ResourceRef::Table {
                keyspace_name: keyspace_name.to_string(),
                table_name: table_name.to_string(),
            })
        }
    }
}

/// The `ResourceNotFoundException` payload for an ARN that names no stored resource.
///
/// The ARN the caller supplied goes in the modelled `resourceArn` member, which is what a
/// client reads to learn WHICH resource was missing.
pub fn arn_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Resource '{resource_arn}' was not found."
        )),
        resource_arn: Some(resource_arn.to_string()),
    }
}

/// Per-service state for com-amazonaws-keyspaces-keyspacesservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKeyspacesKeyspacesserviceState {
    /// Every keyspace in this account, keyed by `keyspaceName`. Bounded by
    /// [`MAX_KEYSPACES`], and each keyspace's tables by [`MAX_TABLES_PER_KEYSPACE`].
    pub keyspaces: BTreeMap<String, Keyspace>,
    /// The earliest instant at which any stored table has a status transition due, or
    /// `None` when none has. [`Self::settle`] returns without looking at a single table
    /// until that instant arrives, so a request pays O(1) for the clock except on the
    /// calls where a transition really is due.
    next_transition_at: Option<SystemTime>,
}

/// One keyspace, with the tables it owns.
#[derive(Debug, Clone)]
pub struct Keyspace {
    /// `keyspaceName`, which is also this map entry's key.
    pub name: String,
    /// The keyspace ARN minted by `CreateKeyspace` ([`keyspace_arn`]).
    pub resource_arn: String,
    /// `replicationSpecification.replicationStrategy`.
    pub replication_strategy: String,
    /// `replicationSpecification.regionList`.
    pub replication_regions: Vec<String>,
    /// This keyspace's tags: what `CreateKeyspace` attached, plus what `TagResource`
    /// added and `UntagResource` removed since.
    pub tags: TagSet,
    /// The keyspace's tables, keyed by `tableName`.
    pub tables: BTreeMap<String, Table>,
}

/// One table: everything `CreateTable` was given, plus what the service derives.
#[derive(Debug, Clone)]
pub struct Table {
    /// The owning keyspace's name.
    pub keyspace_name: String,
    /// `tableName`, which is also this map entry's key.
    pub table_name: String,
    /// The table ARN minted by `CreateTable` ([`table_arn`]).
    pub resource_arn: String,
    /// When `CreateTable` ran.
    pub creation_timestamp: SystemTime,
    /// A `TableStatus` value.
    pub status: String,
    /// When the clock owes this table its next status, or `None` once it holds a
    /// status nothing is pending on. Never slept for: [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`]
    /// compares it against the clock.
    pub status_due_at: Option<SystemTime>,
    /// The schema exactly as supplied.
    pub schema_definition: SchemaDefinition,
    /// The supplied capacity specification, or the modelled default.
    pub capacity_specification: CapacitySpecification,
    /// The supplied encryption specification, or the modelled default.
    pub encryption_specification: EncryptionSpecification,
    /// The supplied point-in-time-recovery setting, or the modelled default.
    pub point_in_time_recovery: PointInTimeRecovery,
    /// The supplied custom TTL setting, if any.
    pub ttl: Option<TimeToLive>,
    /// The supplied default TTL in seconds, if any.
    pub default_time_to_live: Option<i32>,
    /// The supplied comment, if any.
    pub comment: Option<Comment>,
    /// The supplied client-side-timestamps setting, if any.
    pub client_side_timestamps: Option<ClientSideTimestamps>,
    /// This table's tags: what `CreateTable` attached, plus what `TagResource` added and
    /// `UntagResource` removed since.
    pub tags: TagSet,
}

impl ComAmazonawsKeyspacesKeyspacesserviceState {
    /// Record that some table now has a transition due at `due`, so [`Self::settle`]
    /// knows the earliest instant at which it has to look at the store again.
    pub fn note_transition_due(&mut self, due: SystemTime) {
        self.next_transition_at = Some(match self.next_transition_at {
            Some(existing) if existing <= due => existing,
            _ => due,
        });
    }

    /// Materialise every status transition the clock already owes at `now`: a table whose
    /// `CREATING` span has elapsed becomes `ACTIVE`, and a table whose `DELETING` span
    /// has elapsed is gone.
    ///
    /// Every handler calls this first, including the reporting ones. That is the one write
    /// a report is allowed: the transition was due at `now` whether or not anybody called,
    /// so materialising it changes nothing a later caller would not have seen anyway.
    ///
    /// Nothing is scanned until the earliest due instant arrives, and the scan that does
    /// run is bounded by [`MAX_KEYSPACES`] x [`MAX_TABLES_PER_KEYSPACE`] and runs at most
    /// once per transition that becomes due.
    pub fn settle(&mut self, now: SystemTime) {
        if self.next_transition_at.is_none_or(|next| now < next) {
            return;
        }
        let mut earliest: Option<SystemTime> = None;
        for keyspace in self.keyspaces.values_mut() {
            keyspace.tables.retain(|_, table| {
                let Some(due) = table.status_due_at else {
                    return true;
                };
                if now < due {
                    earliest = Some(match earliest {
                        Some(seen) if seen <= due => seen,
                        _ => due,
                    });
                    return true;
                }
                table.status_due_at = None;
                match table.status.as_str() {
                    TABLE_STATUS_CREATING => {
                        table.status = TABLE_STATUS_ACTIVE.to_string();
                        true
                    }
                    // The deletion the clock owed has completed, so the table is
                    // no longer a resource of this keyspace at all.
                    TABLE_STATUS_DELETING => false,
                    _ => true,
                }
            });
        }
        self.next_transition_at = earliest;
    }

    /// The tags of the resource `resource_arn` names, or `None` when it names no
    /// resource this store holds.
    ///
    /// The ARN is the key, not the name it embeds: a stored resource answers only to the
    /// ARN it was minted with ([`keyspace_arn`], [`table_arn`]), so an ARN for another
    /// partition or another Region reaches nothing here even when a resource of that
    /// name exists.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&TagSet> {
        match parse_resource_arn(resource_arn)? {
            ResourceRef::Keyspace { keyspace_name } => {
                let keyspace = self.keyspaces.get(&keyspace_name)?;
                (keyspace.resource_arn == resource_arn).then_some(&keyspace.tags)
            }
            ResourceRef::Table {
                keyspace_name,
                table_name,
            } => {
                let table = self.keyspaces.get(&keyspace_name)?.tables.get(&table_name)?;
                (table.resource_arn == resource_arn).then_some(&table.tags)
            }
        }
    }

    /// The same lookup, for a tagging operation that writes.
    pub fn resource_tags_mut(&mut self, resource_arn: &str) -> Option<&mut TagSet> {
        match parse_resource_arn(resource_arn)? {
            ResourceRef::Keyspace { keyspace_name } => {
                let keyspace = self.keyspaces.get_mut(&keyspace_name)?;
                (keyspace.resource_arn == resource_arn).then_some(&mut keyspace.tags)
            }
            ResourceRef::Table {
                keyspace_name,
                table_name,
            } => {
                let table = self
                    .keyspaces
                    .get_mut(&keyspace_name)?
                    .tables
                    .get_mut(&table_name)?;
                (table.resource_arn == resource_arn).then_some(&mut table.tags)
            }
        }
    }
}

/// The region this request was signed for — the third field of its SigV4
/// credential scope — or [`DEFAULT_REGION`] when it carries no usable scope.
///
/// Keyspaces ARNs are regional, so every minted ARN is built with this.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A keyspace's ARN: `arn:aws:cassandra:<region>:<account>:/keyspace/<name>/`.
///
/// The trailing `/` is part of the published Keyspaces keyspace-ARN form and is
/// what distinguishes it from a table ARN under the same keyspace.
pub fn keyspace_arn(region: &str, keyspace_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/"),
    )
}

/// A table's ARN: `arn:aws:cassandra:<region>:<account>:/keyspace/<ks>/table/<tbl>`.
pub fn table_arn(region: &str, keyspace_name: &str, table_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/table/{table_name}"),
    )
}

/// The `ResourceNotFoundException` payload for a keyspace that is not in state.
///
/// The ARN of the resource that was looked for goes in the modelled
/// `resourceArn` member, which is what a client reads to learn WHICH resource
/// was missing.
pub fn keyspace_not_found(region: &str, keyspace_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Keyspace '{keyspace_name}' was not found.")),
        resource_arn: Some(keyspace_arn(region, keyspace_name)),
    }
}

/// The `ResourceNotFoundException` payload for a table that is not in state.
pub fn table_not_found(
    region: &str,
    keyspace_name: &str,
    table_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Table '{keyspace_name}.{table_name}' was not found."
        )),
        resource_arn: Some(table_arn(region, keyspace_name, table_name)),
    }
}

/// The capacity specification a table is created with: the caller's, or the
/// modelled default (`PAY_PER_REQUEST`).
///
/// The capacity units belong to `PROVISIONED` and to nothing else: that mode
/// "requires `readCapacityUnits` and `writeCapacityUnits` as input"
/// (`CapacitySpecification$throughputMode`), and no other mode takes them. The
/// model expresses neither half of that, so both are refused here, naming the
/// member the caller has to fix.
pub fn resolve_capacity(
    supplied: Option<CapacitySpecification>,
) -> Result<CapacitySpecification, String> {
    let Some(capacity) = supplied else {
        return Ok(CapacitySpecification {
            throughput_mode: DEFAULT_THROUGHPUT_MODE.to_string(),
            read_capacity_units: None,
            write_capacity_units: None,
        });
    };
    let units = [
        ("readCapacityUnits", capacity.read_capacity_units.is_some()),
        ("writeCapacityUnits", capacity.write_capacity_units.is_some()),
    ];
    if capacity.throughput_mode == PROVISIONED_THROUGHPUT_MODE {
        let missing: Vec<&str> = units
            .iter()
            .filter(|(_, present)| !*present)
            .map(|(name, _)| *name)
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "capacitySpecification with throughputMode '{PROVISIONED_THROUGHPUT_MODE}' requires {}",
                missing.join(" and ")
            ));
        }
    } else {
        let supplied_units: Vec<&str> = units
            .iter()
            .filter(|(_, present)| *present)
            .map(|(name, _)| *name)
            .collect();
        if !supplied_units.is_empty() {
            return Err(format!(
                "capacitySpecification with throughputMode '{}' does not take {}; they are input for '{PROVISIONED_THROUGHPUT_MODE}' only",
                capacity.throughput_mode,
                supplied_units.join(" or ")
            ));
        }
    }
    Ok(capacity)
}

/// The encryption specification a table is created with: the caller's, or the
/// modelled default (`AWS_OWNED_KMS_KEY`).
///
/// `CUSTOMER_MANAGED_KMS_KEY` "requires the `kms_key_identifier` of the KMS key
/// in Amazon Resource Name (ARN) format as input"
/// (`EncryptionSpecification$type`), which the model cannot express.
pub fn resolve_encryption(
    supplied: Option<EncryptionSpecification>,
) -> Result<EncryptionSpecification, String> {
    let Some(encryption) = supplied else {
        return Ok(EncryptionSpecification {
            r#type: DEFAULT_ENCRYPTION_TYPE.to_string(),
            kms_key_identifier: None,
        });
    };
    if encryption.r#type == CUSTOMER_MANAGED_ENCRYPTION_TYPE
        && encryption.kms_key_identifier.is_none()
    {
        return Err(format!(
            "encryptionSpecification with type '{CUSTOMER_MANAGED_ENCRYPTION_TYPE}' requires kmsKeyIdentifier"
        ));
    }
    Ok(encryption)
}

/// Check that the schema's roles and columns agree: every key and static column
/// is a column the schema declares, and no column is named twice.
///
/// `partitionKeys`, `clusteringKeys` and `staticColumns` each name "the columns
/// ... of the table" (`SchemaDefinition`), so a name that `allColumns` does not
/// declare is not a column of the table. And a column holds ONE role: it is part
/// of the partition key, or of the clustering key, or static, or an ordinary
/// column — the storage engine has no row shape in which a column is two of
/// those, nor one in which `allColumns` declares a name twice. The model
/// expresses only that each list is non-empty, not that the lists agree.
pub fn validate_schema(schema: &SchemaDefinition) -> Result<(), String> {
    let mut declared: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    for column in &schema.all_columns {
        if !declared.insert(column.name.as_str()) {
            return Err(format!(
                "schemaDefinition.allColumns declares column '{}' more than once",
                column.name
            ));
        }
    }
    let roles = schema
        .partition_keys
        .iter()
        .map(|key| ("partitionKeys", key.name.as_str()))
        .chain(
            schema
                .clustering_keys
                .iter()
                .flatten()
                .map(|key| ("clusteringKeys", key.name.as_str())),
        )
        .chain(
            schema
                .static_columns
                .iter()
                .flatten()
                .map(|column| ("staticColumns", column.name.as_str())),
        );
    let mut assigned: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    for (member, name) in roles {
        if !declared.contains(name) {
            return Err(format!(
                "schemaDefinition.{member} names column '{name}', which schemaDefinition.allColumns does not declare"
            ));
        }
        if !assigned.insert(name) {
            return Err(format!(
                "schemaDefinition.{member} names column '{name}', which the schema already gives another role"
            ));
        }
    }
    Ok(())
}

/// The replication strategy and region list a keyspace is created with.
///
/// A `MULTI_REGION` keyspace's "`regionList` requires the current Region and at
/// least one additional Amazon Web Services Region"
/// (`CreateKeyspaceRequest$replicationSpecification`); the model expresses only
/// the list's minimum length, and nothing about the current region or about the
/// list being required at all, so both are checked here. A `SINGLE_REGION`
/// keyspace is replicated in the region it was created in.
pub fn resolve_replication(
    supplied: Option<&crate::ops::types::ReplicationSpecification>,
    region: &str,
) -> Result<(String, Vec<String>), String> {
    let Some(specification) = supplied else {
        return Ok((
            SINGLE_REGION_STRATEGY.to_string(),
            vec![region.to_string()],
        ));
    };
    if specification.replication_strategy != MULTI_REGION_STRATEGY {
        return Ok((
            specification.replication_strategy.clone(),
            specification
                .region_list
                .clone()
                .unwrap_or_else(|| vec![region.to_string()]),
        ));
    }
    let Some(regions) = specification.region_list.clone() else {
        return Err(format!(
            "replicationSpecification with replicationStrategy '{MULTI_REGION_STRATEGY}' requires regionList"
        ));
    };
    if !regions.iter().any(|listed| listed == region) {
        return Err(format!(
            "replicationSpecification.regionList must contain the current Region '{region}'"
        ));
    }
    Ok((specification.replication_strategy.clone(), regions))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKeyspacesKeyspacesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKeyspacesKeyspacesserviceState>)
}
