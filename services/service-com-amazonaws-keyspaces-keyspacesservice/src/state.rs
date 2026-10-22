// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-keyspaces-keyspacesservice service.
//!
//! Amazon Keyspaces is a two-level namespace: a region holds keyspaces, and a
//! keyspace holds tables and user-defined types. Both levels live in the one
//! [`ComAmazonawsKeyspacesKeyspacesserviceState`] below so every operation reads
//! and writes ONE copy of a resource.
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
    AutoScalingSpecification, CapacitySpecification, ClientSideTimestamps, Comment,
    EncryptionSpecification, FieldDefinition, PointInTimeRecovery, ResourceNotFoundException,
    SchemaDefinition, Tag, TimeToLive,
};

/// The `cassandra` ARN namespace / SigV4 signing name of Amazon Keyspaces
/// (`awsService.arnNamespace` in this service's Smithy model).
pub const ARN_NAMESPACE: &str = "cassandra";

/// The region an unsigned request is served in. A signed request carries its own
/// region in the SigV4 credential scope; see [`request_region`].
pub const DEFAULT_REGION: &str = "us-east-1";

/// `KeyspaceStatus` = `CREATING`: what `CreateKeyspace` puts a new keyspace in.
///
/// "`CreateKeyspace` is an asynchronous operation. You can monitor the creation
/// status of the new keyspace by using the `GetKeyspace` operation." Both
/// constants below are members of the model's `KeyspaceStatus` value set
/// (`crate::ops::types::KEYSPACE_STATUS_VALUES`).
pub const KEYSPACE_STATUS_CREATING: &str = "CREATING";
/// `KeyspaceStatus` = `ACTIVE`: the status a created keyspace settles into.
pub const KEYSPACE_STATUS_ACTIVE: &str = "ACTIVE";

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
/// `TableStatus` = `UPDATING`: what `UpdateTable` puts a table in until the
/// setting change completes.
///
/// That `UpdateTable` is asynchronous and leaves the table in this status is what
/// `DeleteTable` documents from the other side: "If a table is either in the
/// `CREATING` or `UPDATING` states, then Amazon Keyspaces returns a
/// `ResourceInUseException`" — only an update can put a table in `UPDATING`.
pub const TABLE_STATUS_UPDATING: &str = "UPDATING";

/// `TypeStatus` = `CREATING`: what `CreateType` puts a new user-defined type in.
///
/// This and [`TYPE_STATUS_ACTIVE`] are members of the model's `TypeStatus` value
/// set (`crate::ops::types::TYPE_STATUS_VALUES`).
pub const TYPE_STATUS_CREATING: &str = "CREATING";
/// `TypeStatus` = `ACTIVE`: the status a created type settles into, and the only
/// one in which the type can be used (`ResourceNotFoundException`: "The resource
/// might not be specified correctly, or its status might not be ACTIVE").
pub const TYPE_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a resource stays in a transient status before the clock owes it the
/// next one: `CREATING` -> `ACTIVE` after `CreateTable`, `CreateKeyspace` or
/// `CreateType`, `UPDATING` -> `ACTIVE` after `UpdateTable`, and `DELETING` ->
/// gone after `DeleteTable`.
///
/// CHOSEN, not documented: Amazon Keyspaces publishes no duration for any of
/// these transitions, only that they happen (`CreateTable`: "the status of the
/// table is set to `CREATING`. You can start using a table when the status is
/// `ACTIVE`"; `CreateKeyspace`: "`CreateKeyspace` is an asynchronous operation.
/// You can monitor the creation status of the new keyspace by using the
/// `GetKeyspace` operation"; `DeleteTable`: "the specified table is in the
/// `DELETING` state until Amazon Keyspaces completes the deletion"; `TypeStatus`
/// models `CREATING` and `GetType` reports it). It is
/// deliberately NON-ZERO, so a caller that reads straight after the call still
/// observes the transient status the model documents, and short enough that a
/// test can wait it out.
pub const TRANSIENT_STATUS_SPAN: Duration = Duration::from_millis(500);

/// `ThroughputMode` = `PAY_PER_REQUEST`: the on-demand mode, which takes no capacity
/// units.
pub const PAY_PER_REQUEST_THROUGHPUT_MODE: &str = "PAY_PER_REQUEST";

/// The throughput mode a table gets when `CreateTable` carries no
/// `capacitySpecification` ("The default is `throughput_mode:PAY_PER_REQUEST`",
/// `CapacitySpecification$throughputMode`).
pub const DEFAULT_THROUGHPUT_MODE: &str = PAY_PER_REQUEST_THROUGHPUT_MODE;

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

/// The lowest `targetValue` a target tracking auto scaling policy may be given:
/// "You define `targetValue` as a percentage. A `double` between 20 and 90"
/// (`TargetTrackingScalingPolicyConfiguration$targetValue`).
pub const MIN_TARGET_VALUE: f64 = 20.0;

/// The highest `targetValue` of the same published range. See [`MIN_TARGET_VALUE`].
pub const MAX_TARGET_VALUE: f64 = 90.0;

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
/// answer what `CreateTable` created. Every operation that grows the store refuses past
/// these caps with the service's own modelled `ServiceQuotaExceededException` (HTTP 402),
/// which this model declares on the two create operations, on `UpdateTable` and on no
/// read. The caps also bound the work
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`] can do while holding the state
/// lock.
///
/// The numbers are the emulator's own bound, not a contract: Amazon Keyspaces publishes
/// these quotas as adjustable, so no single value is the published one; these are the
/// order of magnitude of the defaults and are far above any ordinary workflow.
pub const MAX_KEYSPACES: usize = 256;

/// The most tables one keyspace holds. See [`MAX_KEYSPACES`].
pub const MAX_TABLES_PER_KEYSPACE: usize = 256;

/// The most user-defined types one keyspace holds. See [`MAX_KEYSPACES`]: `CreateType`
/// is another operation that grows the store, and it refuses past this cap with the
/// `ServiceQuotaExceededException` the model declares for it.
pub const MAX_TYPES_PER_KEYSPACE: usize = 256;

/// The most fields one user-defined type holds, and with [`MAX_TYPES_PER_KEYSPACE`] the
/// pair that bounds everything `GetType` derives from a keyspace's types: the
/// [`nesting_depth`] walk, [`Keyspace::direct_parent_types`] and
/// [`Keyspace::direct_referring_tables`] each cost at most one pass over this many
/// fields per stored type.
///
/// `FieldList` is modelled `len 1..` with NO maximum, so without a cap here one request
/// could store a field list of any size and every later read would pay for it. Past the
/// cap `CreateType` answers the `ServiceQuotaExceededException` (HTTP 402) it models.
///
/// Like [`MAX_KEYSPACES`], the number is the emulator's own bound rather than a
/// contract: Amazon Keyspaces publishes a fields-per-UDT quota as adjustable, so no
/// single value is the published one, and this is far above any ordinary type.
pub const MAX_FIELDS_PER_TYPE: usize = 128;

/// The Cassandra native (scalar) data type names, which a field's `type` may name
/// without that naming a user-defined type.
///
/// Source: the CQL native types, as linked from `FieldDefinition$type` ("Any supported
/// Cassandra data type, including collections and other user-defined types that are
/// contained in the same keyspace" -> Cassandra data type support, Amazon Keyspaces
/// Developer Guide). The list is deliberately the WHOLE CQL set rather than only the
/// subset Keyspaces serves: its one job is to tell an identifier that names a built-in
/// type apart from one that names a UDT, in [`referenced_type_names`].
pub const CQL_NATIVE_TYPES: &[&str] = &[
    "ascii",
    "bigint",
    "blob",
    "boolean",
    "counter",
    "date",
    "decimal",
    "double",
    "duration",
    "float",
    "inet",
    "int",
    "smallint",
    "text",
    "time",
    "timestamp",
    "timeuuid",
    "tinyint",
    "uuid",
    "varchar",
    "varint",
];

/// The CQL type constructors: an identifier that names one of these introduces a
/// parameterised type (`list<int>`, `map<text, address>`, `frozen<address>`,
/// `tuple<int, text>`, `vector<float, 3>`) rather than naming a user-defined type. Same
/// source as [`CQL_NATIVE_TYPES`].
pub const CQL_TYPE_CONSTRUCTORS: &[&str] = &["frozen", "list", "map", "set", "tuple", "vector"];

/// The most columns one table's schema holds. See [`MAX_KEYSPACES`].
///
/// This is the third caller-driven growth dimension of the store, and the only one a
/// table can grow along after it exists: `addColumns` is modelled `len 1..`, with no
/// maximum, and `UpdateTable` appends to a schema that never shrinks. Both the create
/// and the update refuse past this cap, so neither one oversized request nor a run of
/// updates can grow a stored schema without bound. It also caps the duplicate-name scan
/// an `addColumns` update runs while holding the state lock.
pub const MAX_COLUMNS_PER_TABLE: usize = 1024;

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
    /// [`MAX_KEYSPACES`], each keyspace's tables by [`MAX_TABLES_PER_KEYSPACE`] and its
    /// types by [`MAX_TYPES_PER_KEYSPACE`], and each table's schema by
    /// [`MAX_COLUMNS_PER_TABLE`].
    pub keyspaces: BTreeMap<String, Keyspace>,
    /// The earliest instant at which any stored table or type has a status transition
    /// due, or `None` when none has. [`Self::settle`] returns without looking at a single
    /// resource until that instant arrives, so a request pays O(1) for the clock except on
    /// the calls where a transition really is due.
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
    /// A `KeyspaceStatus` value: the keyspace's own status, which `GetKeyspace`
    /// reports per replication Region.
    pub status: String,
    /// When the clock owes this keyspace its next status, or `None` once it holds
    /// a status nothing is pending on. Never slept for: [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`]
    /// compares it against the clock.
    pub status_due_at: Option<SystemTime>,
    /// The keyspace's tables, keyed by `tableName`.
    pub tables: BTreeMap<String, Table>,
    /// The keyspace's user-defined types, keyed by the FORMATTED `typeName`
    /// ([`format_type_name`]) — the name every other operation takes, and the name a
    /// field or column refers to the type by.
    pub types: BTreeMap<String, UserDefinedType>,
}

impl Keyspace {
    /// The `replicationRegions` a report of this keyspace carries: the stored region
    /// list for a `MULTI_REGION` keyspace, and nothing for any other strategy.
    ///
    /// "If the `replicationStrategy` of the keyspace is `MULTI_REGION`, a list of
    /// replication Regions is returned" (`KeyspaceSummary$replicationRegions`), and the
    /// member's modelled minimum length is 2 — which a single-Region keyspace's own
    /// region, on its own, could never satisfy.
    pub fn reported_replication_regions(&self) -> Option<Vec<String>> {
        (self.replication_strategy == MULTI_REGION_STRATEGY)
            .then(|| self.replication_regions.clone())
    }
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
    /// The supplied capacity specification, or the modelled default, as last set by
    /// `CreateTable` or `UpdateTable`.
    pub capacity_specification: CapacitySpecification,
    /// The supplied auto scaling settings, if any — "The optional auto scaling settings
    /// for a table in provisioned capacity mode" — reported back by
    /// `GetTableAutoScalingSettings`.
    pub auto_scaling_specification: Option<AutoScalingSpecification>,
    /// When an `UpdateTable` last moved this table's throughput mode to
    /// `PAY_PER_REQUEST`, or `None` while no update ever has
    /// (`CapacitySpecificationSummary$lastUpdateToPayPerRequestTimestamp`). An update
    /// that re-sets the mode the table is already in changed no throughput capacity, so
    /// it leaves this where it is.
    pub last_update_to_pay_per_request: Option<SystemTime>,
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

/// One user-defined type: the fields `CreateType` was given, plus what the service
/// derives. Everything `GetType` reports about how the type RELATES to other
/// resources (`directParentTypes`, `directReferringTables`, `maxNestingDepth`) is
/// derived from the keyspace on read, so no copy of it can go stale.
#[derive(Debug, Clone)]
pub struct UserDefinedType {
    /// The owning keyspace's name.
    pub keyspace_name: String,
    /// The FORMATTED type name ([`format_type_name`]), which is also this map entry's
    /// key and what `CreateType` answers with.
    pub type_name: String,
    /// The field definitions exactly as supplied, in the order they were supplied.
    pub field_definitions: Vec<FieldDefinition>,
    /// When `CreateType` ran, which is the last instant the type's definition
    /// changed: nothing in this service's surface alters a type's fields.
    pub last_modified_timestamp: SystemTime,
    /// A `TypeStatus` value.
    pub status: String,
    /// When the clock owes this type its next status, or `None` once it holds a
    /// status nothing is pending on. Never slept for; see
    /// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`].
    pub status_due_at: Option<SystemTime>,
}

impl Keyspace {
    /// The types of this keyspace that use `type_name` in one of their fields —
    /// `GetType`'s `directParentTypes` ("The types that use this type").
    ///
    /// Derived, never stored: a parent is any type one of whose field types refers to
    /// this one, which is exactly what [`referenced_type_names`] extracts. One pass over
    /// the keyspace's types, so at most [`MAX_TYPES_PER_KEYSPACE`] x
    /// [`MAX_FIELDS_PER_TYPE`] field types are scanned.
    pub fn direct_parent_types(&self, type_name: &str) -> Vec<String> {
        self.types
            .values()
            .filter(|candidate| {
                candidate.type_name != type_name
                    && candidate.field_definitions.iter().any(|field| {
                        referenced_type_names(&field.r#type)
                            .iter()
                            .any(|referenced| referenced == type_name)
                    })
            })
            .map(|candidate| candidate.type_name.clone())
            .collect()
    }

    /// The tables of this keyspace that use `type_name` in one of their columns —
    /// `GetType`'s `directReferringTables` ("The tables that use this type").
    ///
    /// Derived the same way, in one pass over the keyspace's tables.
    pub fn direct_referring_tables(&self, type_name: &str) -> Vec<String> {
        self.tables
            .values()
            .filter(|table| {
                table.schema_definition.all_columns.iter().any(|column| {
                    referenced_type_names(&column.r#type)
                        .iter()
                        .any(|referenced| referenced == type_name)
                })
            })
            .map(|table| table.table_name.clone())
            .collect()
    }
}

/// `GetType`'s `maxNestingDepth` ("The level of nesting implemented for this type"):
/// `0` for a type none of whose fields refers to another user-defined type, and
/// otherwise one more than the deepest type it refers to.
///
/// Derived from the keyspace's types on read, so it is `Depth`'s modelled default of
/// `0` exactly for a flat type.
///
/// BOUNDED, and bounded by the stored data rather than by the shape of the type graph:
/// the walk is iterative (no recursion to blow a stack) and MEMOISED in `depths`, so
/// every reachable type is expanded at most once however many times it is referred to
/// — a diamond or a repeated reference (`map<a, a>`) costs one expansion, not two. The
/// whole derivation is therefore at most [`MAX_TYPES_PER_KEYSPACE`] expansions of at
/// most [`MAX_FIELDS_PER_TYPE`] fields each: the same bounded class of work as
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`], and linear in what the
/// keyspace holds. A reference can only name a type that already exists (`CreateType`
/// refuses any other), so the relation is acyclic; `expanding` keeps the walk finite
/// even if a future operation ever changes that, by taking a reference back into the
/// chain currently being expanded as contributing no depth.
pub fn nesting_depth(types: &BTreeMap<String, UserDefinedType>, type_name: &str) -> i32 {
    /// What the walk still owes: a type whose references are not known yet, or one
    /// whose depth follows once its references have theirs.
    enum Step {
        Expand(String),
        Resolve(String, Vec<String>),
    }
    let mut depths: BTreeMap<String, i32> = BTreeMap::new();
    let mut expanding: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![Step::Expand(type_name.to_string())];
    while let Some(step) = stack.pop() {
        match step {
            Step::Expand(name) => {
                if depths.contains_key(&name) {
                    continue;
                }
                let references: Vec<String> = types
                    .get(&name)
                    .map(|udt| {
                        udt.field_definitions
                            .iter()
                            .flat_map(|field| referenced_type_names(&field.r#type))
                            .collect()
                    })
                    .unwrap_or_default();
                let pending: Vec<String> = references
                    .iter()
                    .filter(|reference| {
                        !depths.contains_key(*reference) && !expanding.contains(*reference)
                    })
                    .cloned()
                    .collect();
                expanding.insert(name.clone());
                // The resolve goes UNDER the references, so each of them has its depth
                // in `depths` by the time this type's own depth is taken.
                stack.push(Step::Resolve(name, references));
                stack.extend(pending.into_iter().map(Step::Expand));
            }
            Step::Resolve(name, references) => {
                let deepest = references
                    .iter()
                    .map(|reference| 1 + depths.get(reference).copied().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                expanding.remove(&name);
                depths.insert(name, deepest);
            }
        }
    }
    depths.get(type_name).copied().unwrap_or(0)
}

/// What makes two identifiers the same one, for a comparison that is not a lookup of a
/// stored name: an unquoted identifier is case-folded (Amazon Keyspaces "converts upper
/// case characters automatically into lower case characters"), a double-quoted one
/// stands for itself. So `Id` and `id` are one field name, and `"Id"` is another.
///
/// This is what [`format_type_name`] does to a name it accepts, minus the refusal, so a
/// caller that only needs the identity of a name it is not storing (a duplicate check)
/// does not have to validate the form twice.
pub fn identifier_identity(supplied: &str) -> String {
    if supplied.len() > 1 && supplied.starts_with('"') && supplied.ends_with('"') {
        supplied.to_string()
    } else {
        supplied.to_ascii_lowercase()
    }
}

/// The formatted form of a `typeName`, or why the name cannot be one.
///
/// `CreateType$typeName`: "UDT names must contain 48 characters or less, must begin
/// with an alphabetic character, and can only contain alpha-numeric characters and
/// underscores. Amazon Keyspaces converts upper case characters automatically into
/// lower case characters. Alternatively, you can declare a UDT name in double quotes.
/// When declaring a UDT name inside double quotes, Amazon Keyspaces preserves upper
/// casing and allows special characters. You can also use double quotes as part of the
/// name when you create the UDT, but you must escape each double quote character with
/// an additional double quote character."
///
/// So there are two forms, and the formatted name is the one every other operation
/// takes (`GetType$typeName`: "if the name of the type was created without double
/// quotes, Amazon Keyspaces saved the name in lower-case characters. If the name was
/// created in double quotes, you must use double quotes to specify the type name"):
///
/// * unquoted — alphabetic first character, then alphanumerics and underscores,
///   case-folded to lower case;
/// * double-quoted — preserved verbatim, quotes included, with every interior double
///   quote doubled.
///
/// The model constrains only the length (`TypeName`, `len 1..48`), so the form is
/// checked here and a name that is neither is refused with the operation's
/// `ValidationException`.
pub fn format_type_name(supplied: &str) -> Result<String, String> {
    if let Some(rest) = supplied.strip_prefix('"') {
        let Some(inner) = rest.strip_suffix('"') else {
            return Err(format!(
                "typeName '{supplied}' opens a double-quoted name that is never closed"
            ));
        };
        if inner.is_empty() {
            return Err("typeName '\"\"' is a double-quoted name with no characters".to_string());
        }
        let mut characters = inner.chars();
        while let Some(character) = characters.next() {
            if character == '"' && characters.next() != Some('"') {
                return Err(format!(
                    "typeName '{supplied}' uses a double quote inside a double-quoted name \
                     without escaping it with an additional double quote character"
                ));
            }
        }
        return Ok(supplied.to_string());
    }
    let mut characters = supplied.chars();
    let first = characters.next().unwrap_or_default();
    if !first.is_ascii_alphabetic() || !characters.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "typeName '{supplied}' must begin with an alphabetic character and contain only \
             alpha-numeric characters and underscores, or be declared in double quotes"
        ));
    }
    Ok(supplied.to_ascii_lowercase())
}

/// The user-defined types a CQL type expression refers to, in the order they appear.
///
/// A field's or column's type is "any supported Cassandra data type, including
/// collections and other user-defined types that are contained in the same keyspace"
/// (`FieldDefinition$type`), so the UDTs it refers to are the identifiers in it that
/// name neither a native type ([`CQL_NATIVE_TYPES`]) nor a type constructor
/// ([`CQL_TYPE_CONSTRUCTORS`]): `frozen<list<address>>` refers to `address`, and
/// `map<text, int>` to nothing.
///
/// Each identifier is returned in its FORMATTED form, so it compares equal to a key of
/// [`Keyspace::types`]: an unquoted one case-folded, a double-quoted one verbatim with
/// its quotes. A token that cannot be an unquoted identifier at all (one that does not
/// begin with an alphabetic character, such as the `3` of `vector<float, 3>`) is not a
/// reference. The CQL textual type grammar has no maintained Rust crate (the Cassandra
/// crates parse the binary protocol, not this grammar), so the identifiers are scanned
/// here; this extracts references and does not claim to validate the grammar.
pub fn referenced_type_names(type_expression: &str) -> Vec<String> {
    let mut referenced = Vec::new();
    let characters: Vec<char> = type_expression.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '"' {
            // A quoted identifier runs to the next unescaped quote; `""` is one
            // escaped quote inside it, exactly as `format_type_name` accepts.
            let start = index;
            index += 1;
            while index < characters.len() {
                if characters[index] == '"' {
                    if characters.get(index + 1) == Some(&'"') {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                index += 1;
            }
            referenced.push(characters[start..index].iter().collect());
        } else if character.is_ascii_alphanumeric() || character == '_' {
            let start = index;
            while index < characters.len()
                && (characters[index].is_ascii_alphanumeric() || characters[index] == '_')
            {
                index += 1;
            }
            let identifier: String = characters[start..index]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase();
            // An unquoted identifier "must begin with an alphabetic character"
            // ([`format_type_name`]), so a token that does not — the `3` of a
            // parameterised type such as `vector<float, 3>` — names no type and is not a
            // reference to one.
            if identifier.starts_with(|first: char| first.is_ascii_alphabetic())
                && !CQL_NATIVE_TYPES.contains(&identifier.as_str())
                && !CQL_TYPE_CONSTRUCTORS.contains(&identifier.as_str())
            {
                referenced.push(identifier);
            }
        } else {
            index += 1;
        }
    }
    referenced
}

impl ComAmazonawsKeyspacesKeyspacesserviceState {
    /// Record that some table or type now has a transition due at `due`, so
    /// [`Self::settle`] knows the earliest instant at which it has to look at the store
    /// again.
    pub fn note_transition_due(&mut self, due: SystemTime) {
        self.next_transition_at = earlier_of(self.next_transition_at, due);
    }

    /// Materialise every status transition the clock already owes at `now`: a keyspace
    /// whose `CREATING` span has elapsed becomes `ACTIVE`, a table whose `CREATING` or
    /// `UPDATING` span has elapsed becomes `ACTIVE`, a type whose `CREATING` span has
    /// elapsed becomes `ACTIVE`, and a table whose
    /// `DELETING` span has elapsed is gone.
    ///
    /// Every handler calls this first, including the reporting ones. That is the one write
    /// a report is allowed: the transition was due at `now` whether or not anybody called,
    /// so materialising it changes nothing a later caller would not have seen anyway.
    ///
    /// Nothing is scanned until the earliest due instant arrives, and the scan that does
    /// run is bounded by [`MAX_KEYSPACES`] x ([`MAX_TABLES_PER_KEYSPACE`] +
    /// [`MAX_TYPES_PER_KEYSPACE`]) and runs at most once per transition that becomes due.
    pub fn settle(&mut self, now: SystemTime) {
        if self.next_transition_at.is_none_or(|next| now < next) {
            return;
        }
        let mut earliest: Option<SystemTime> = None;
        for keyspace in self.keyspaces.values_mut() {
            match keyspace.status_due_at {
                // Still pending: nothing to materialise, but `settle` has to look at this
                // keyspace again once its span elapses.
                Some(due) if now < due => earliest = earlier_of(earliest, due),
                Some(_) => {
                    keyspace.status_due_at = None;
                    if keyspace.status == KEYSPACE_STATUS_CREATING {
                        keyspace.status = KEYSPACE_STATUS_ACTIVE.to_string();
                    }
                }
                None => {}
            }
            keyspace.tables.retain(|_, table| {
                let Some(due) = table.status_due_at else {
                    return true;
                };
                if now < due {
                    earliest = earlier_of(earliest, due);
                    return true;
                }
                table.status_due_at = None;
                match table.status.as_str() {
                    // The creation, or the setting change, the clock owed has
                    // completed, so the table is usable again.
                    TABLE_STATUS_CREATING | TABLE_STATUS_UPDATING => {
                        table.status = TABLE_STATUS_ACTIVE.to_string();
                        true
                    }
                    // The deletion the clock owed has completed, so the table is
                    // no longer a resource of this keyspace at all.
                    TABLE_STATUS_DELETING => false,
                    _ => true,
                }
            });
            // A type's only transition in this slice is the one `CreateType` starts:
            // `CREATING` -> `ACTIVE`. `DELETING` belongs to `DeleteType`, which is
            // still a scaffold, so nothing here can put a type in it.
            for udt in keyspace.types.values_mut() {
                let Some(due) = udt.status_due_at else {
                    continue;
                };
                if now < due {
                    earliest = earlier_of(earliest, due);
                    continue;
                }
                udt.status_due_at = None;
                if udt.status == TYPE_STATUS_CREATING {
                    udt.status = TYPE_STATUS_ACTIVE.to_string();
                }
            }
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

/// The earlier of an instant already seen (if any) and `due`: how both
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::note_transition_due`] and
/// [`ComAmazonawsKeyspacesKeyspacesserviceState::settle`] keep the single earliest instant at
/// which the store has a transition to materialise.
fn earlier_of(seen: Option<SystemTime>, due: SystemTime) -> Option<SystemTime> {
    Some(seen.map_or(due, |existing| existing.min(due)))
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

/// A user-defined type's ARN:
/// `arn:aws:cassandra:<region>:<account>:/keyspace/<ks>/type/<type>`.
///
/// The `type` resource segment is the published Keyspaces form for a UDT (the
/// resource an IAM policy names to grant `cassandra:Create` on a type). No operation
/// of this service returns it — `CreateType` and `GetType` both answer with the
/// KEYSPACE ARN — so its only use is naming the resource a
/// `ResourceNotFoundException` looked for.
pub fn type_arn(region: &str, keyspace_name: &str, type_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/type/{type_name}"),
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
///
/// The message is the Cassandra-style form Amazon Keyspaces builds from the request for a
/// table lookup — the bare `tableName`, unqualified and unquoted — which is what
/// Cassandra-compatible tooling keys off. A keyspace lookup keeps its own form
/// ([`keyspace_not_found`]).
pub fn table_not_found(
    region: &str,
    keyspace_name: &str,
    table_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("unconfigured table {table_name}")),
        resource_arn: Some(table_arn(region, keyspace_name, table_name)),
    }
}

/// The `ResourceNotFoundException` payload for a user-defined type that is not in
/// state, or that is in state in a status other than `ACTIVE` ("The operation tried to
/// access a keyspace, table, or type that doesn't exist. The resource might not be
/// specified correctly, or its status might not be ACTIVE").
///
/// The message is the request-derived Cassandra-style form Amazon Keyspaces answers an
/// unresolvable UDT name with, `UDT <name> does not exist` — the form a recording of
/// the real service shows for a column whose type names no type of the keyspace.
pub fn type_not_found(
    region: &str,
    keyspace_name: &str,
    type_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("UDT {type_name} does not exist")),
        resource_arn: Some(type_arn(region, keyspace_name, type_name)),
    }
}

/// The capacity specification a table is created or updated with: the caller's,
/// or the modelled default (`PAY_PER_REQUEST`) when a create omits it.
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

/// The encryption specification a table is created or updated with: the
/// caller's, or the modelled default (`AWS_OWNED_KMS_KEY`) when a create omits
/// it.
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

/// The auto scaling settings a table is created with: the caller's, checked against what
/// the service documents for them, or `None`.
///
/// Auto scaling is "The optional auto scaling settings for a table in provisioned capacity
/// mode" (`CreateTableRequest$autoScalingSpecification`), so it belongs to
/// `throughputMode:PROVISIONED` and to nothing else. For each capacity it is turned on for
/// ("This optional parameter enables auto scaling for the table if set to `false`"),
/// "you must specify the following parameters": `minimumUnits`, `maximumUnits` and
/// `scalingPolicy`, whose target tracking configuration defines `targetValue` as "a
/// percentage. A `double` between 20 and 90". The model states none of that, only that the
/// unit members are `long`s with a minimum of 1.
pub fn resolve_auto_scaling(
    supplied: Option<AutoScalingSpecification>,
    capacity: &CapacitySpecification,
) -> Result<Option<AutoScalingSpecification>, String> {
    let Some(specification) = supplied else {
        return Ok(None);
    };
    if capacity.throughput_mode != PROVISIONED_THROUGHPUT_MODE {
        return Err(format!(
            "autoScalingSpecification is the auto scaling setting of a table in capacity mode '{PROVISIONED_THROUGHPUT_MODE}'; \
             capacitySpecification.throughputMode is '{}'",
            capacity.throughput_mode
        ));
    }
    for (member, settings) in [
        (
            "autoScalingSpecification.readCapacityAutoScaling",
            specification.read_capacity_auto_scaling.as_ref(),
        ),
        (
            "autoScalingSpecification.writeCapacityAutoScaling",
            specification.write_capacity_auto_scaling.as_ref(),
        ),
    ] {
        let Some(settings) = settings else {
            continue;
        };
        if settings.auto_scaling_disabled {
            continue;
        }
        let target_value = settings
            .scaling_policy
            .as_ref()
            .and_then(|policy| policy.target_tracking_scaling_policy_configuration.as_ref())
            .map(|configuration| configuration.target_value);
        let (Some(minimum), Some(maximum), Some(target_value)) =
            (settings.minimum_units, settings.maximum_units, target_value)
        else {
            return Err(format!(
                "{member} with autoScalingDisabled false requires minimumUnits, maximumUnits and \
                 scalingPolicy.targetTrackingScalingPolicyConfiguration.targetValue"
            ));
        };
        if minimum > maximum {
            return Err(format!(
                "{member}.minimumUnits ({minimum}) must not exceed maximumUnits ({maximum})"
            ));
        }
        if !(MIN_TARGET_VALUE..=MAX_TARGET_VALUE).contains(&target_value) {
            return Err(format!(
                "{member}.scalingPolicy.targetTrackingScalingPolicyConfiguration.targetValue \
                 ({target_value}) must be between {MIN_TARGET_VALUE} and {MAX_TARGET_VALUE}"
            ));
        }
    }
    Ok(Some(specification))
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
