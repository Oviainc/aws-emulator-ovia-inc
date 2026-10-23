// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-memorydb-amazonmemorydb service.
//!
//! One store per emulator, reached from a handler through [`service_state`]. Every
//! modelled shape a handler answers with is BUILT FROM this state (see
//! [`StoredCluster::to_api`] / [`StoredAcl::to_api`] / [`StoredUser::to_api`]), so a
//! describe can only report what a create or update actually persisted — or, for the
//! resources an AWS account already has, what
//! [`ComAmazonawsMemorydbAmazonmemorydbState::ensure_builtins`] materialises.
//!
//! Besides the store itself this module owns the small amount of logic several
//! operations share — the request's region, the ARN/endpoint naming, shard layout,
//! pagination and the clock-owed status settle — so that the per-operation files in
//! `src/ops/` stay one-operation-wide and cannot drift from one another.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    Authentication, AuthenticationMode, Cluster, Endpoint, EngineVersionInfo, Event, Node,
    Parameter, ParameterGroup, SecurityGroupMembership, Shard, Tag, User, ACL,
    ACLPendingChanges,
};

/// The region an ARN or endpoint carries when the request is unsigned, matching the
/// region every generated emulator defaults to (see `engine::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`smithy_query query="service_info"` →
/// `awsService.arnNamespace`).
pub const ARN_NAMESPACE: &str = "memorydb";

/// `CreateCluster$Port`: "The port number on which each of the nodes accepts
/// connections." MemoryDB speaks the Redis OSS / Valkey protocol, whose registered
/// port is 6379, and that is the port a cluster created without `Port` listens on.
pub const DEFAULT_PORT: i32 = 6379;

/// `CreateCluster$NumShards`: "The number of shards the cluster will contain. The
/// default value is 1."
pub const DEFAULT_NUM_SHARDS: i32 = 1;

/// `CreateCluster$NumReplicasPerShard`: "The number of replicas to apply to each
/// shard. The default value is 1."
pub const DEFAULT_NUM_REPLICAS_PER_SHARD: i32 = 1;

/// The most shards one cluster may have (MemoryDB's per-cluster shard quota).
///
/// An UPPER BOUND, not decoration: `NumShards` and `ShardConfiguration$ShardCount`
/// arrive as a bare `i32` the model puts no `@range` on, and they size the stored
/// layout, so every write path refuses a value above this before any layout is built.
/// It is also at most [`TOTAL_SLOTS`], so each shard still owns at least one slot.
pub const MAX_SHARDS_PER_CLUSTER: i32 = 500;

/// The most replicas one shard may have:
/// `CreateCluster$NumReplicasPerShard` — "The maximum is 5."
///
/// The other half of the layout bound above: nodes per cluster can never exceed
/// `MAX_SHARDS_PER_CLUSTER * (MAX_REPLICAS_PER_SHARD + 1)`, so the layout cannot be
/// sized by an unbounded request field.
pub const MAX_REPLICAS_PER_SHARD: i32 = 5;

/// The most clusters, the most ACLs, and the most users this emulator keeps.
///
/// A ceiling THIS EMULATOR chooses (AWS's own cluster/ACL/user quotas are per-account
/// and adjustable) so that one caller cannot grow the store without bound; AWS answers
/// its own exceeded quota with the same modelled faults
/// (`ClusterQuotaForCustomerExceededFault`, `ACLQuotaExceededFault`,
/// `UserQuotaExceededFault`).
pub const MAX_RESOURCES_PER_KIND: usize = 500;

/// The most passwords one user may hold: `Authentication$PasswordCount` — "The number
/// of passwords belonging to the user. The maximum is two."
pub const MAX_PASSWORDS_PER_USER: usize = 2;

/// The most users one ACL may hold, and so the most one request may name in a
/// membership list.
///
/// An UPPER BOUND, not decoration: `UserNameListInput` carries `len 1..` — a minimum
/// with no maximum — so a membership list arrives caller-sized, and it both sizes the
/// stored membership and is one factor of every add/remove comparison. Both write paths
/// of an ACL's membership (`CreateACL`, `UpdateACL`) refuse a longer list, and a
/// resulting membership above this, before any of that work runs, so neither the store
/// nor the comparisons can be grown by a request. AWS answers its own per-account user
/// quota with `UserQuotaExceededFault`; a membership larger than an ACL may hold is a
/// value the service cannot serve, so it is refused with the modelled
/// `InvalidParameterValueException` both operations declare.
pub const MAX_USERS_PER_ACL: usize = 100;

/// The most tags one resource may carry.
///
/// `TagList` is `@length(min: 0, max: 200)` and is the shape `ListTags` reports a
/// resource's whole tag set in, so a resource can never hold more than that many tags;
/// `TagResource` answers a merge that would exceed it with the modelled
/// `TagQuotaPerResourceExceeded`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The shortest password MemoryDB accepts for a user, from the password requirements
/// AWS documents for RBAC users ("Authenticating users with Access Control Lists"):
/// "must be at least 16 characters and no more than 128 characters in length".
pub const MIN_PASSWORD_LEN: usize = 16;

/// The longest password MemoryDB accepts, from the same documented requirement.
pub const MAX_PASSWORD_LEN: usize = 128;

// The Developer Guide's prose also lists characters a password "cannot contain"
// (`,` `"` `/` `@`). The SERVICE does not enforce that list — a `CreateUser` carrying
// any of them succeeds and the user settles with one password — and the model puts no
// `@pattern` on `PasswordListInput`'s members, so `validate_password` enforces only the
// printable-ASCII and length rules above.

/// The longest a MemoryDB user, ACL, cluster or parameter-group name may be.
///
/// The model caps none of them (`UserName` is `len 1..`, the other names are plain
/// `String`), but the service refuses a longer name with its modelled
/// `InvalidParameterValueException`; a name of exactly this length is accepted.
pub const MAX_RESOURCE_NAME_LEN: usize = 40;

/// The smallest page size a MemoryDB listing accepts for `MaxResults`.
///
/// `MaxResults` is an `IntegerOptional` the model puts no `@range` on; the WINDOW comes
/// from the service's own refusal, which names both of its bounds — the recorded
/// `DescribeUsers(MaxResults=0)` answer is `InvalidParameterValueException` "Invalid
/// value for max results. Must be between 1 and 50", and `MaxResults=51` is refused on
/// the same path. So the bound lives here and every listing in this service reads it
/// through [`max_results_rejection`], rather than each one inventing its own.
pub const MIN_MAX_RESULTS: i32 = 1;

/// The largest page size a MemoryDB listing accepts for `MaxResults`, the upper bound
/// of the same recorded refusal as [`MIN_MAX_RESULTS`] ("Must be between 1 and 50"),
/// read through the same [`max_results_rejection`].
pub const MAX_MAX_RESULTS: i32 = 50;

/// The number of hash slots a cluster spreads over its shards. Fixed by the Redis
/// OSS / Valkey cluster protocol MemoryDB implements (slots `0..=16383`).
pub const TOTAL_SLOTS: i32 = 16_384;

/// How long after the call that starts it a cluster or ACL stays in its transient
/// status before the clock owes the terminal one.
///
/// A span THIS EMULATOR chooses (AWS takes minutes): long enough that a caller acting
/// straight after the create still observes `creating`, short enough that a test that
/// waits for the resource is quick. Never zero, and never slept on — the due instant
/// is stored and materialised by whichever later call observes the resource.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How far back a recorded event stays reportable.
///
/// `DescribeEvents` — "you can retrieve up to 14 days' worth of events if necessary":
/// 14 days is the age past which the service holds no event, so an event older than
/// this is dropped from the store rather than kept in a log nothing can report.
pub const EVENT_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// The window `DescribeEvents` reports when the request bounds none itself:
/// "By default, only the events occurring within the last hour are returned".
pub const DEFAULT_EVENT_WINDOW: Duration = Duration::from_secs(60 * 60);

/// The smallest `DescribeEvents$Duration` ("The number of minutes worth of events to
/// retrieve") that names a window at all: a window of zero or fewer minutes selects
/// nothing, so it is a value the listing cannot serve.
pub const MIN_EVENT_DURATION_MINUTES: i32 = 1;

/// The largest `DescribeEvents$Duration` the service can serve: [`EVENT_RETENTION`] in
/// the whole minutes that member is given in (14 days = 20160 minutes), because no
/// event older than that is kept.
pub const MAX_EVENT_DURATION_MINUTES: i32 = 14 * 24 * 60;

/// The most events this store keeps.
///
/// A ceiling THIS EMULATOR chooses, on top of [`EVENT_RETENTION`], so that a caller
/// driving many changes inside the retention window cannot grow the store without
/// bound; the OLDEST events are the ones dropped, so the log a `DescribeEvents` window
/// is most likely to ask about is the one that survives.
pub const MAX_EVENTS_RETAINED: usize = 2_000;

/// The `SourceType` tokens this store records events under, named so the handlers that
/// record one branch on a constant rather than a literal.
///
/// Each is a member of the GENERATED catalog
/// `crate::ops::types::SOURCE_TYPE_VALUES` (`acl`, `cluster`, `node`,
/// `parameter-group`, `subnet-group`, `user`), which is also what `DescribeEvents`
/// validates a requested `SourceType` against, so neither can drift from the model.
/// The catalog's other two tokens are recorded by nothing: a subnet group has no
/// implemented write path in this service (every subnet-group operation still answers
/// 501), and a node has no lifecycle of its own here — nodes exist only as the layout
/// of the cluster whose own events already report the change.
pub mod event_source_type {
    pub const CLUSTER: &str = "cluster";
    pub const ACL: &str = "acl";
    pub const USER: &str = "user";
    pub const PARAMETER_GROUP: &str = "parameter-group";
}

/// What happened to a resource, as the event recording it reports.
///
/// One vocabulary for every source type, so the message an event carries is built by
/// one rule rather than spelled out per operation: a create records
/// [`EventKind::Created`], a change to an existing resource [`EventKind::Modified`] and
/// a delete [`EventKind::Deleted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Created,
    Modified,
    Deleted,
}

impl EventKind {
    /// The past-tense verb this kind reads as in an event message.
    fn verb(self) -> &'static str {
        match self {
            EventKind::Created => "created",
            EventKind::Modified => "modified",
            EventKind::Deleted => "deleted",
        }
    }
}

/// How an event message names the kind of resource it is about, per source type.
///
/// AWS documents no message text for `Event$Message` ("The text of the event"), so the
/// WORDING is this emulator's: the kind of resource and what happened to it, with WHICH
/// resource left to `Event$SourceName` — which is what the service fixes.
fn source_type_label(source_type: &str) -> &str {
    match source_type {
        event_source_type::CLUSTER => "Cluster",
        event_source_type::ACL => "ACL",
        event_source_type::USER => "User",
        event_source_type::PARAMETER_GROUP => "Parameter group",
        other => other,
    }
}

/// An event as this emulator records it: what happened to which resource, and when.
///
/// Recorded by the call that MAKES the change (`CreateUser`, `UpdateACL`,
/// `DeleteCluster`, …) and only read by `DescribeEvents`, so the events a listing
/// reports are the changes this store actually took, never a canned log.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    /// The order this event was recorded in, counting from zero over the store's whole
    /// life. It is the event's identity for pagination: `Date` comes from the wall
    /// clock and two changes can share one, while the sequence cannot, so a page
    /// boundary always names exactly one position in the log.
    pub sequence: u64,
    /// `Event$SourceType`: which kind of resource the event is about, one of
    /// [`event_source_type`].
    pub source_type: String,
    /// `Event$SourceName`: "The identifier for the source of the event" — the resource
    /// name, as the store keys it (folded).
    pub source_name: String,
    /// `Event$Message`: "The text of the event".
    pub message: String,
    /// `Event$Date`: "The date and time when the event occurred", the instant the
    /// change was made.
    pub date: SystemTime,
}

impl StoredEvent {
    /// The modelled `Event` for this stored event.
    pub fn to_api(&self) -> Event {
        Event {
            source_name: Some(self.source_name.clone()),
            source_type: Some(self.source_type.clone()),
            message: Some(self.message.clone()),
            date: Some(::aws_smithy_types::DateTime::from(self.date)),
        }
    }

    /// This event's position in the log, as the key [`paginate`] pages by.
    ///
    /// The listing reports events NEWEST FIRST, and `paginate` walks keys in ascending
    /// string order, so the key is the sequence COUNTED DOWN from `u64::MAX` and
    /// zero-padded: string order over the keys is newest-first order, and a `NextToken`
    /// names the first event of the next page.
    pub fn page_key(&self) -> String {
        format!("{:020}", u64::MAX - self.sequence)
    }
}

/// One engine version this service offers, as `DescribeEngineVersions` reports it.
///
/// The version is held as its NUMERIC parts rather than as strings, so every value the
/// catalog answers with — `EngineVersion`, `EnginePatchVersion` and the
/// `ParameterGroupFamily` the engine default parameters belong to — is DERIVED from the
/// same three numbers and no two of them can disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineVersion {
    /// `EngineVersionInfo$Engine`: "The name of the engine for which version
    /// information is provided."
    pub engine: &'static str,
    pub major: u32,
    pub minor: u32,
    /// The patch release of `<major>.<minor>` this service runs.
    pub patch: u32,
}

/// The engine versions MemoryDB offers, in catalog order.
///
/// SERVICE metadata, not per-caller state: the list is the same for every account (as a
/// region's availability zones are), so it is a catalog rather than a map in
/// [`ComAmazonawsMemorydbAmazonmemorydbState`]. It lives HERE, beside the store, so that
/// every operation which has to resolve an engine version reads the one list and no two
/// of them can publish different versions.
///
/// Source: the AWS MemoryDB *Supported engine versions* documentation — MemoryDB runs
/// Redis OSS 6.2, 7.0 and 7.1, and Valkey 7.2 and 8.0, each at the patch release below.
pub const ENGINE_VERSIONS: &[EngineVersion] = &[
    EngineVersion { engine: ENGINE_REDIS, major: 6, minor: 2, patch: 6 },
    EngineVersion { engine: ENGINE_REDIS, major: 7, minor: 0, patch: 7 },
    EngineVersion { engine: ENGINE_REDIS, major: 7, minor: 1, patch: 1 },
    EngineVersion { engine: ENGINE_VALKEY, major: 7, minor: 2, patch: 6 },
    EngineVersion { engine: ENGINE_VALKEY, major: 8, minor: 0, patch: 0 },
];

/// `CreateCluster$Engine`: "The name of the engine to be used for the cluster." MemoryDB
/// names its two engines with these lower-case tokens.
pub const ENGINE_REDIS: &str = "redis";
pub const ENGINE_VALKEY: &str = "valkey";

/// The prefix every MemoryDB parameter group family carries, in front of the engine name
/// and its major version (`memorydb_redis7`, `memorydb_valkey8`).
const PARAMETER_GROUP_FAMILY_PREFIX: &str = "memorydb_";

impl EngineVersion {
    /// `EngineVersionInfo$EngineVersion` — "The engine version": `<major>.<minor>`.
    pub fn version(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }

    /// `EngineVersionInfo$EnginePatchVersion` — "The patched engine version":
    /// `<major>.<minor>.<patch>`.
    pub fn patch_version(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// `EngineVersionInfo$ParameterGroupFamily` — "the name of the parameter group
    /// family to which the engine default parameters apply". A MemoryDB family covers
    /// one engine at one MAJOR version, so every minor version of an engine's major
    /// version shares `memorydb_<engine><major>`.
    pub fn parameter_group_family(&self) -> String {
        format!("{PARAMETER_GROUP_FAMILY_PREFIX}{}{}", self.engine, self.major)
    }

    /// Whether this is the DEFAULT version of its parameter group family: the greatest
    /// minor version the catalog publishes for this engine and major version. Default-ness
    /// is a property of the family (the engine defaults a parameter group inherits), not
    /// of one request, so `DescribeEngineVersions$DefaultOnly` narrows to these.
    pub fn is_family_default(&self) -> bool {
        !ENGINE_VERSIONS.iter().any(|other| {
            other.engine == self.engine && other.major == self.major && other.minor > self.minor
        })
    }

    /// This version's position in the catalog listing, for [`paginate`]'s token.
    ///
    /// Zero-padded, so the ordering the key gives is NUMERIC on the version parts: a
    /// future `10.0` sorts after `8.0` rather than before it, which a bare `"10.0"`
    /// string would not.
    pub fn catalog_key(&self) -> String {
        format!("{}/{:05}/{:05}", self.engine, self.major, self.minor)
    }

    /// The modelled `EngineVersionInfo` for this catalog entry.
    pub fn to_api(&self) -> EngineVersionInfo {
        EngineVersionInfo {
            engine: Some(self.engine.to_string()),
            engine_version: Some(self.version()),
            engine_patch_version: Some(self.patch_version()),
            parameter_group_family: Some(self.parameter_group_family()),
        }
    }
}

/// The DOMAIN of one engine default parameter: the values the service accepts for it.
///
/// `Parameter$DataType` ("The parameter's data type") and `Parameter$AllowedValues`
/// ("The valid range of values for the parameter") are both DERIVED from this, and so is
/// the refusal an update answers for a value outside it, so what a describe publishes
/// about a parameter and what an update enforces for it cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterDomain {
    /// A whole number, at least `min` and — when the engine publishes an upper bound —
    /// at most `max`.
    Integer { min: i64, max: Option<i64> },
    /// A whole number from one of several DISJOINT ranges, in the order the engine
    /// publishes them, each `(min, max)` with `None` for an open-ended top.
    ///
    /// Not every numeric setting takes one contiguous range: `timeout` takes 0 (never
    /// close an idle connection) or 20 seconds and up, which the service publishes as
    /// `0,20-`. A single-value range is published as that value alone.
    IntegerRanges(&'static [(i64, Option<i64>)]),
    /// One of a fixed set of tokens, in the order the engine documents them.
    Tokens(&'static [&'static str]),
    /// Free text: a parameter the engine itself parses (a flag string, or a size that
    /// may be written as a percentage), so the service publishes no value set for it.
    Text,
}

impl ParameterDomain {
    /// `Parameter$DataType`, in the two type names MemoryDB reports.
    pub fn data_type(&self) -> &'static str {
        match self {
            Self::Integer { .. } | Self::IntegerRanges(_) => "integer",
            Self::Tokens(_) | Self::Text => "string",
        }
    }

    /// The numeric ranges this domain publishes, empty for a non-numeric one.
    ///
    /// One source for both the published `AllowedValues` and the accept/refuse
    /// decision, so what the service says a parameter takes and what it actually takes
    /// cannot drift apart.
    fn number_ranges(&self) -> Vec<(i64, Option<i64>)> {
        match self {
            Self::Integer { min, max } => vec![(*min, *max)],
            Self::IntegerRanges(ranges) => ranges.to_vec(),
            Self::Tokens(_) | Self::Text => Vec::new(),
        }
    }

    /// `Parameter$AllowedValues`, rendered the way MemoryDB publishes it: `<min>-<max>`
    /// for a number (an open-ended range keeps the trailing hyphen, a single-value one
    /// is just that value, and several ranges are comma-separated) and a
    /// comma-separated list for a token set.
    ///
    /// `None` for free text: the service publishes no value set for a parameter the
    /// engine parses itself, so the member is ABSENT rather than an empty string a
    /// client could read as "no value is allowed".
    pub fn allowed_values(&self) -> Option<String> {
        match self {
            Self::Integer { .. } | Self::IntegerRanges(_) => Some(
                self.number_ranges()
                    .iter()
                    .map(|(min, max)| match max {
                        Some(max) if max == min => format!("{min}"),
                        Some(max) => format!("{min}-{max}"),
                        None => format!("{min}-"),
                    })
                    .collect::<Vec<String>>()
                    .join(","),
            ),
            Self::Tokens(tokens) => Some(tokens.join(",")),
            Self::Text => None,
        }
    }

    /// Whether this domain accepts `value`, or why it does not.
    ///
    /// The message is BUILT FROM the broken constraint — the value the caller sent and
    /// the range or value set this domain PUBLISHES for the parameter (the same string
    /// `DescribeParameters` reports in `Parameter$AllowedValues`) — in the service's
    /// own three wordings: a value that is not a whole number at all, a number outside
    /// the published range, and a token outside the published set.
    pub fn accepts(&self, value: &str) -> Result<(), String> {
        match self {
            Self::Integer { .. } | Self::IntegerRanges(_) => {
                let Ok(number) = value.parse::<i64>() else {
                    return Err(format!("invalid integer value: {value}"));
                };
                let ranges = self.number_ranges();
                if ranges
                    .iter()
                    .any(|(min, max)| number >= *min && max.is_none_or(|max| number <= max))
                {
                    return Ok(());
                }
                Err(format!(
                    "value:{value} is outside of range:{}",
                    self.allowed_values().unwrap_or_default()
                ))
            }
            Self::Tokens(tokens) => {
                if tokens.contains(&value) {
                    Ok(())
                } else {
                    Err(format!(
                        "invalid parameter value, allowed values are:{}",
                        self.allowed_values().unwrap_or_default()
                    ))
                }
            }
            // Parsed by the engine rather than by the control plane, so every value the
            // modelled string constraints already allow reaches it.
            Self::Text => Ok(()),
        }
    }
}

/// One engine default parameter: a setting a parameter group of the right family
/// carries, with the value it holds until an update overrides it.
///
/// SERVICE metadata, not per-caller state, exactly like [`ENGINE_VERSIONS`]: the engine
/// defaults of a family are the same for every account, so the catalog lives beside the
/// store and a parameter group persists only the values a caller CHANGED.
#[derive(Debug, Clone, Copy)]
pub struct EngineParameter {
    /// `Parameter$Name`, as the engine spells it in its own configuration.
    pub name: &'static str,
    /// The value a group of a carrying family reports for this parameter before any
    /// update: `Parameter$Value` of the family's engine defaults.
    pub default_value: &'static str,
    /// `Parameter$Description` — "A description of the parameter".
    pub description: &'static str,
    /// What the parameter accepts, which also publishes its data type and allowed
    /// values.
    pub domain: ParameterDomain,
    /// The earliest family GENERATION (see [`family_generation`]) carrying this
    /// parameter.
    pub since: u32,
    /// The first generation that no longer carries it — the engine renamed or dropped
    /// it there — or `None` when every later generation keeps it.
    pub until: Option<u32>,
}

/// The generation of engine defaults a published engine version's family carries.
///
/// Redis OSS 7.0 renamed the collection-encoding parameters (`*-ziplist-*` →
/// `*-listpack-*`), replaced `lua-time-limit` with `busy-reply-threshold` and added the
/// client-memory and latency-tracking settings; Valkey 7.2 and 8.0 continue that same
/// line. So the families this service publishes fall into two generations: the Redis 6
/// one, and the Redis 7 one every later engine follows.
fn version_generation(version: &EngineVersion) -> u32 {
    if version.engine == ENGINE_REDIS && version.major <= 6 {
        0
    } else {
        1
    }
}

/// The generation of engine defaults the parameter group family `family` carries, or
/// `None` when no published engine version belongs to that family.
///
/// This is how a family's parameters FOLLOW the engine catalog: a family name is
/// resolved against [`ENGINE_VERSIONS`] rather than against a second list of families,
/// so a family carries engine defaults exactly when an engine version publishes it.
pub fn family_generation(family: &str) -> Option<u32> {
    ENGINE_VERSIONS
        .iter()
        .find(|version| version.parameter_group_family() == family)
        .map(version_generation)
}

/// The engine default parameters of one parameter group family, in NAME order (so a
/// listing over them pages stably).
///
/// A family no published engine version names carries no engine defaults, so the
/// listing is empty rather than another family's parameters.
pub fn family_parameters(family: &str) -> Vec<&'static EngineParameter> {
    let Some(generation) = family_generation(family) else {
        return Vec::new();
    };
    let mut parameters: Vec<&'static EngineParameter> = ENGINE_PARAMETERS
        .iter()
        .filter(|parameter| parameter.covers_generation(generation))
        .collect();
    parameters.sort_by_key(|parameter| parameter.name);
    parameters
}

/// Every parameter group family this service publishes, in catalog order and each one
/// once.
///
/// DERIVED from [`ENGINE_VERSIONS`], exactly as [`family_generation`] resolves a family
/// against the catalog: a family exists precisely when a published engine version names
/// it, so the families this list reports and the families `DescribeEngineVersions`
/// reports cannot disagree. Several versions of one engine's major version share a
/// family ([`EngineVersion::parameter_group_family`]), so the duplicates are dropped.
pub fn parameter_group_families() -> Vec<String> {
    let mut families: Vec<String> = Vec::new();
    for version in ENGINE_VERSIONS {
        let family = version.parameter_group_family();
        if !families.contains(&family) {
            families.push(family);
        }
    }
    families
}

/// The name of the parameter group MemoryDB itself provides for `family`: the
/// [`DEFAULT_PARAMETER_GROUP_PREFIX`] followed by the family name with its underscores
/// written as hyphens (`memorydb_valkey7` → `default.memorydb-valkey7`).
///
/// The family is the only input, so the group's name, its family and the parameters it
/// reports all follow the one engine catalog.
pub fn default_parameter_group_name(family: &str) -> String {
    format!(
        "{DEFAULT_PARAMETER_GROUP_PREFIX}{}",
        family.replace('_', "-")
    )
}

/// `ParameterGroup$Description` of a group MemoryDB itself provides: the service's own
/// wording for the family baseline, so a default group reports a description like every
/// other stored group rather than an absent member.
pub fn default_parameter_group_description(family: &str) -> String {
    format!("Default parameter group for {family}")
}

/// Why a parameter group is not the caller's to change, or `None` when it is.
///
/// The `default.<family>` groups belong to MemoryDB, not to the caller
/// ([`DEFAULT_PARAMETER_GROUP_PREFIX`] is the same rule the tag mutations refuse one
/// with, see [`default_resource_rejection`]): they carry the family's engine defaults
/// and are immutable, so every write path over a parameter group refuses one. The
/// caller maps the refusal onto its own modelled `InvalidParameterValueException`.
pub fn immutable_parameter_group_rejection(name: &str) -> Option<String> {
    name.starts_with(DEFAULT_PARAMETER_GROUP_PREFIX)
        .then(|| "Cannot modify a default parameter group.".to_string())
}

/// The message a `ParameterGroupNotFoundFault` carries for the group `name` the request
/// named.
///
/// Every operation that resolves a parameter group by name answers the absent one
/// through this, so one request's name is reported back in one template whichever
/// operation it was sent to.
pub fn parameter_group_not_found_message(name: &str) -> String {
    format!("ParameterGroup not found: {name}")
}

/// The engine default parameter of one family by name, or `None` when that family
/// carries no parameter with that name.
pub fn family_parameter(family: &str, name: &str) -> Option<&'static EngineParameter> {
    let generation = family_generation(family)?;
    ENGINE_PARAMETERS
        .iter()
        .find(|parameter| parameter.name == name && parameter.covers_generation(generation))
}

impl EngineParameter {
    /// Whether a family of generation `generation` carries this parameter.
    fn covers_generation(&self, generation: u32) -> bool {
        generation >= self.since && self.until.is_none_or(|until| generation < until)
    }

    /// `Parameter$MinimumEngineVersion` — "The earliest engine version to which the
    /// parameter can apply": the lowest published engine version whose family carries
    /// it, as `<major>.<minor>`.
    ///
    /// DERIVED from [`ENGINE_VERSIONS`], so it can never name a version this service
    /// does not offer. `None` only if the catalog published no version at all.
    pub fn minimum_engine_version(&self) -> Option<String> {
        ENGINE_VERSIONS
            .iter()
            .filter(|version| self.covers_generation(version_generation(version)))
            .min_by_key(|version| (version.major, version.minor))
            .map(EngineVersion::version)
    }

    /// The modelled `Parameter` for this setting, carrying `value` — the group's stored
    /// value when an update overrode it, otherwise this engine default.
    pub fn to_api(&self, value: &str) -> Parameter {
        Parameter {
            name: Some(self.name.to_string()),
            value: Some(value.to_string()),
            description: Some(self.description.to_string()),
            data_type: Some(self.domain.data_type().to_string()),
            allowed_values: self.domain.allowed_values(),
            minimum_engine_version: self.minimum_engine_version(),
        }
    }
}

/// The engine default parameters MemoryDB publishes for its parameter group families.
///
/// Source: the AWS MemoryDB User Guide, "Configuring MemoryDB parameter groups" — the
/// per-family default parameter groups (`default.memorydb-redis6`,
/// `default.memorydb-redis7`, `default.memorydb-valkey7`, `default.memorydb-valkey8`),
/// whose settings and defaults are the Redis OSS / Valkey engine defaults of the
/// corresponding engine line. `since`/`until` record which engine line a setting
/// belongs to, which is what makes one family's listing differ from another's.
///
/// Every entry is modifiable: MemoryDB's `Parameter` shape models no `IsModifiable`
/// member, and the settings MemoryDB manages itself (`maxmemory`, `cluster-enabled`,
/// the append-only settings) are not published as parameters at all.
pub const ENGINE_PARAMETERS: &[EngineParameter] = &[
    // ---- Carried by every family -------------------------------------------------
    EngineParameter {
        name: "activedefrag",
        default_value: "no",
        description: "Enabled active memory defragmentation.",
        domain: ParameterDomain::Tokens(YES_NO),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "activerehashing",
        default_value: "yes",
        description: "Whether to actively rehash the main dictionary, which frees \
                      memory faster.",
        domain: ParameterDomain::Tokens(YES_NO),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "active-expire-effort",
        default_value: "1",
        description: "The effort the engine spends expiring keys, from 1 (least) to 10 \
                      (most).",
        domain: ParameterDomain::Integer { min: 1, max: Some(10) },
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "hll-sparse-max-bytes",
        default_value: "3000",
        description: "The size, in bytes, up to which a HyperLogLog is kept in the \
                      sparse representation.",
        domain: ParameterDomain::Integer { min: 0, max: Some(16_000) },
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "lazyfree-lazy-eviction",
        default_value: "no",
        description: "Whether eviction frees memory in a background thread.",
        domain: ParameterDomain::Tokens(YES_NO),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "lazyfree-lazy-expire",
        default_value: "no",
        description: "Whether key expiry frees memory in a background thread.",
        domain: ParameterDomain::Tokens(YES_NO),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "lazyfree-lazy-server-del",
        default_value: "no",
        description: "Whether commands that implicitly delete a key free memory in a \
                      background thread.",
        domain: ParameterDomain::Tokens(YES_NO),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "lazyfree-lazy-user-del",
        default_value: "no",
        description: "Whether a user-issued delete frees memory in a background thread.",
        domain: ParameterDomain::Tokens(YES_NO),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "maxmemory-policy",
        default_value: "noeviction",
        description: "The eviction policy applied when the data on a node reaches its \
                      memory limit.",
        domain: ParameterDomain::Tokens(MAXMEMORY_POLICIES),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "notify-keyspace-events",
        default_value: "",
        description: "The keyspace event classes the engine publishes, as the engine's \
                      flag characters.",
        domain: ParameterDomain::Text,
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "proto-max-bulk-len",
        default_value: "536870912",
        description: "The largest single request or bulk reply, in bytes.",
        domain: ParameterDomain::Integer { min: 1_048_576, max: Some(536_870_912) },
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "set-max-intset-entries",
        default_value: "512",
        description: "The most entries an all-integer set keeps in the memory-efficient \
                      encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "slowlog-log-slower-than",
        default_value: "10000",
        description: "The execution time, in microseconds, above which a command is \
                      recorded in the slow log.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "slowlog-max-len",
        default_value: "128",
        description: "The most entries the slow log keeps.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "timeout",
        default_value: "0",
        description: "The seconds of client idleness after which the engine closes the \
                      connection; 0 never closes it.",
        // Published as `0,20-`: the engine either never closes an idle connection (0)
        // or waits at least 20 seconds, so the seconds between are not a value the
        // parameter takes.
        domain: ParameterDomain::IntegerRanges(&[(0, Some(0)), (20, None)]),
        since: 0,
        until: None,
    },
    EngineParameter {
        name: "tracking-table-max-keys",
        default_value: "1000000",
        description: "The most keys the client-side caching invalidation table tracks.",
        domain: ParameterDomain::Integer { min: 1, max: Some(100_000_000) },
        since: 0,
        until: None,
    },
    // ---- The Redis 6 line only: renamed or replaced in Redis OSS 7.0 -------------
    EngineParameter {
        name: "hash-max-ziplist-entries",
        default_value: "128",
        description: "The most fields a hash keeps in the memory-efficient encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: Some(1),
    },
    EngineParameter {
        name: "hash-max-ziplist-value",
        default_value: "64",
        description: "The longest hash field value, in bytes, kept in the \
                      memory-efficient encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: Some(1),
    },
    EngineParameter {
        name: "list-max-ziplist-size",
        default_value: "128",
        description: "The most entries a list node keeps in the memory-efficient \
                      encoding.",
        domain: ParameterDomain::Integer { min: 1, max: None },
        since: 0,
        until: Some(1),
    },
    EngineParameter {
        name: "lua-time-limit",
        default_value: "5000",
        description: "The milliseconds a script may run before the engine starts \
                      answering BUSY to other clients.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: Some(1),
    },
    EngineParameter {
        name: "zset-max-ziplist-entries",
        default_value: "128",
        description: "The most members a sorted set keeps in the memory-efficient \
                      encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: Some(1),
    },
    EngineParameter {
        name: "zset-max-ziplist-value",
        default_value: "64",
        description: "The longest sorted set member, in bytes, kept in the \
                      memory-efficient encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 0,
        until: Some(1),
    },
    // ---- The Redis 7 line and every engine after it ------------------------------
    EngineParameter {
        name: "busy-reply-threshold",
        default_value: "5000",
        description: "The milliseconds a script may run before the engine starts \
                      answering BUSY to other clients.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "hash-max-listpack-entries",
        default_value: "128",
        description: "The most fields a hash keeps in the memory-efficient encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "hash-max-listpack-value",
        default_value: "64",
        description: "The longest hash field value, in bytes, kept in the \
                      memory-efficient encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "latency-tracking",
        default_value: "yes",
        description: "Whether the engine tracks per-command latency percentiles.",
        domain: ParameterDomain::Tokens(YES_NO),
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "list-max-listpack-size",
        default_value: "128",
        description: "The most entries a list node keeps in the memory-efficient \
                      encoding.",
        domain: ParameterDomain::Integer { min: 1, max: None },
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "maxmemory-clients",
        default_value: "0",
        description: "The memory all client connection buffers together may use, as a \
                      size or a percentage of the node's memory; 0 removes the limit.",
        domain: ParameterDomain::Text,
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "set-max-listpack-entries",
        default_value: "128",
        description: "The most members a set keeps in the memory-efficient encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "set-max-listpack-value",
        default_value: "64",
        description: "The longest set member, in bytes, kept in the memory-efficient \
                      encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "zset-max-listpack-entries",
        default_value: "128",
        description: "The most members a sorted set keeps in the memory-efficient \
                      encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 1,
        until: None,
    },
    EngineParameter {
        name: "zset-max-listpack-value",
        default_value: "64",
        description: "The longest sorted set member, in bytes, kept in the \
                      memory-efficient encoding.",
        domain: ParameterDomain::Integer { min: 0, max: None },
        since: 1,
        until: None,
    },
];

/// The two tokens every boolean engine parameter takes, in the order MemoryDB
/// publishes them in `AllowedValues`.
const YES_NO: &[&str] = &["yes", "no"];

/// The eviction policies `maxmemory-policy` takes, in the order the engine documents
/// them.
const MAXMEMORY_POLICIES: &[&str] = &[
    "volatile-lru",
    "allkeys-lru",
    "volatile-lfu",
    "allkeys-lfu",
    "volatile-random",
    "allkeys-random",
    "volatile-ttl",
    "noeviction",
];

/// The most parameters one `UpdateParameterGroup` call may change:
/// `UpdateParameterGroup$ParameterNameValues` — "A maximum of 20 parameters may be
/// updated per request."
pub const MAX_PARAMETERS_PER_UPDATE: usize = 20;

/// `Cluster$Status`: "The status of the cluster. For example, Available, Updating,
/// Creating." MemoryDB renders them lower-case on the wire.
pub mod cluster_status {
    pub const CREATING: &str = "creating";
    pub const AVAILABLE: &str = "available";
    pub const UPDATING: &str = "updating";
    pub const DELETING: &str = "deleting";
}

/// `ACL$Status`: "Indicates ACL status. Can be \"creating\", \"active\",
/// \"modifying\", \"deleting\"."
///
/// `DELETING` is TERMINAL for this store, exactly as it is for a user: a deleted ACL is
/// removed outright, so the status only ever appears on the response that reports the
/// deletion, never on an ACL a later call can still read.
pub mod acl_status {
    pub const CREATING: &str = "creating";
    pub const ACTIVE: &str = "active";
    pub const MODIFYING: &str = "modifying";
    pub const DELETING: &str = "deleting";
}

/// `User$Status`: "Indicates the user status. Can be \"active\", \"modifying\" or
/// \"deleting\"."
///
/// `DELETING` is TERMINAL for this store: a deleted user is removed outright, so the
/// status only ever appears on the response that reports the deletion, never on a
/// user a later call can still read.
pub mod user_status {
    pub const ACTIVE: &str = "active";
    pub const MODIFYING: &str = "modifying";
    pub const DELETING: &str = "deleting";
}

/// The two `InputAuthenticationType` tokens a request may ASK for, named so the
/// handlers branch on a constant rather than a literal. The full accepted list stays
/// the GENERATED catalog `crate::ops::types::INPUT_AUTHENTICATION_TYPE_VALUES`, which
/// is what a refusal quotes, so neither can drift from the model.
pub mod input_authentication_type {
    pub const PASSWORD: &str = "password";
    pub const IAM: &str = "iam";
}

/// The user every MemoryDB account already has: MemoryDB creates a user named
/// `default` (an `open-access` member) in every account, so `DescribeUsers` answers
/// with it before any `CreateUser` call ever runs.
pub const DEFAULT_USER_NAME: &str = "default";

/// The access string MemoryDB gives that built-in user: full access to every key
/// (`~*`), every pub/sub channel (`&*`) and every command (`+@all`), in the Redis OSS /
/// Valkey ACL syntax `User$AccessString` carries.
pub const DEFAULT_USER_ACCESS_STRING: &str = "on ~* &* +@all";

/// The ACL every MemoryDB account already has: MemoryDB creates an `open-access` ACL
/// (whose only member is the built-in `default` user) in every account, so
/// `DescribeACLs` answers with it — and `CreateCluster(ACLName="open-access")` resolves
/// — before any `CreateACL` call ever runs.
pub const OPEN_ACCESS_ACL_NAME: &str = "open-access";

/// The pub/sub channel rule a Redis OSS / Valkey ACL falls back to when the access
/// string names none: no channel is allowed. MemoryDB CANONICALISES the access string
/// it was given before storing it, and the canonical form always states the channel
/// permission, so `on ~* +@all` reads back as `on ~* resetchannels +@all` (see
/// [`normalise_access_string`]).
pub const DEFAULT_CHANNEL_RULE: &str = "resetchannels";

/// The engine version a password- or no-password-authenticated user (and an ACL whose
/// members are all of those) reports as `MinimumEngineVersion`: MemoryDB has supported
/// ACL users authenticating with a password since its first engine version.
pub const MIN_ENGINE_VERSION_PASSWORD_AUTH: &str = "6.2";

/// The engine version an IAM-authenticated user — and any ACL with such a member —
/// reports as `MinimumEngineVersion`: MemoryDB added IAM authentication for ACL users
/// with engine version 7.0, so an ACL carrying one cannot be attached to a cluster
/// running an older engine.
pub const MIN_ENGINE_VERSION_IAM_AUTH: &str = "7.0";

/// The description MemoryDB gives a parameter group created without one: a single
/// space, which is what the service reports for `ParameterGroup$Description` when the
/// `CreateParameterGroup` request omits the optional member, rather than no member at
/// all.
pub const DEFAULT_PARAMETER_GROUP_DESCRIPTION: &str = " ";

/// `AuthenticationType` (`password` | `no-password` | `iam`): the built-in user
/// authenticates without a password, so it carries no password count.
pub const NO_PASSWORD_AUTHENTICATION: &str = "no-password";

/// The status a shard, a node and an attached security group report once the cluster
/// carrying them is serving.
const AVAILABLE: &str = "available";

/// `Cluster$SnsTopicStatus` of a cluster that was given a notification topic: the
/// topic is being notified. `UpdateCluster$SnsTopicStatus` lets a caller set another.
pub const SNS_TOPIC_ACTIVE: &str = "active";

/// `AZStatus` (`multiaz` | `singleaz`): a shard with at least one replica is spread
/// over more than one availability zone, a shard without replicas is not.
mod availability_mode {
    pub const MULTI_AZ: &str = "multiaz";
    pub const SINGLE_AZ: &str = "singleaz";
}

/// Per-service state for com-amazonaws-memorydb-amazonmemorydb. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMemorydbAmazonmemorydbState {
    /// Clusters by `ClusterName`. A `BTreeMap`, so every list is in name order and
    /// pagination over it is stable.
    pub clusters: BTreeMap<String, StoredCluster>,
    /// ACLs by `ACLName`.
    pub acls: BTreeMap<String, StoredAcl>,
    /// Users by `UserName`, including the built-in `default` user
    /// [`ComAmazonawsMemorydbAmazonmemorydbState::ensure_builtins`] materialises.
    /// A `BTreeMap`, so every listing is in name order and pagination over it is stable.
    pub users: BTreeMap<String, StoredUser>,
    /// Whether the built-in resources have already been materialised in this store, so
    /// that materialising them is a one-off and never resurrects a deleted one.
    pub builtins_materialised: bool,
    /// Parameter groups by `ParameterGroupName`, in name order like the maps above,
    /// so a listing over them pages stably.
    pub parameter_groups: BTreeMap<String, StoredParameterGroup>,
    /// Tags by resource ARN: the tags of each tagged resource, under the ARN that
    /// resource's create minted. Written by the `Tags` member of a create and by
    /// `TagResource`/`UntagResource`, read by `ListTags` — one map, so no two of them
    /// can report a different tag set for one resource.
    ///
    /// A tag KEY identifies a tag within a resource (a second tag with the same key
    /// replaces the first), so each resource's tags are a `BTreeMap` keyed by it —
    /// which also puts every `TagList` this service answers with in key order. "A tag
    /// with a null Value is permitted" (`Tag`), and such a tag carries the EMPTY value
    /// from then on, so a stored tag always has one.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The events this store has recorded, oldest first.
    ///
    /// Appended to by every call that creates, modifies or deletes a resource (see
    /// [`ComAmazonawsMemorydbAmazonmemorydbState::record_event`]) and read only by
    /// `DescribeEvents`, so the log reports the changes this store actually took. A
    /// `Vec` in record order, which is both the chronological order the listing reports
    /// and the order its pagination keys sort in.
    pub events: Vec<StoredEvent>,
    /// The sequence the next recorded event takes, counting from zero over the store's
    /// whole life. Held separately from `events.len()` so that dropping an event the
    /// retention window has passed never re-uses a pagination key.
    pub next_event_sequence: u64,
}

impl ComAmazonawsMemorydbAmazonmemorydbState {
    /// Materialise every status transition the CLOCK already owes at `now`.
    ///
    /// Called at the top of every handler. This is not a mutation the caller asked
    /// for: each transition was due at `now` whether or not anybody called, so a
    /// reporting operation may apply it and two reads at the same instant agree.
    pub fn settle(&mut self, now: Instant) {
        for cluster in self.clusters.values_mut() {
            if cluster.settles_at.is_some_and(|due| now >= due) {
                cluster.status = cluster_status::AVAILABLE.to_string();
                cluster.settles_at = None;
            }
        }
        for acl in self.acls.values_mut() {
            if acl.settles_at.is_some_and(|due| now >= due) {
                acl.status = acl_status::ACTIVE.to_string();
                acl.settles_at = None;
                // A change the ACL has finished applying is no longer pending: once the
                // ACL is back to `active` its `PendingChanges` carries nothing, which is
                // what makes the member a client can poll say whether the change has
                // settled.
                acl.pending_user_names_to_add.clear();
                acl.pending_user_names_to_remove.clear();
            }
        }
        for user in self.users.values_mut() {
            if user.settles_at.is_some_and(|due| now >= due) {
                user.status = user_status::ACTIVE.to_string();
                user.settles_at = None;
            }
        }
    }

    /// Record one event against the resource a call has just changed.
    ///
    /// Every write path in this service that creates, modifies or deletes a resource
    /// calls this, so the event log is a record of what the store actually took rather
    /// than something `DescribeEvents` makes up: the event carries the resource's
    /// `SourceType` and its stored (folded) name, the instant of the change, and a
    /// message built by one rule for every source type.
    pub fn record_event(&mut self, source_type: &str, source_name: &str, kind: EventKind) {
        let date = SystemTime::now();
        let sequence = self.next_event_sequence;
        self.next_event_sequence = sequence.saturating_add(1);
        self.events.push(StoredEvent {
            sequence,
            source_type: source_type.to_string(),
            source_name: source_name.to_string(),
            message: format!(
                "{} {}",
                source_type_label(source_type),
                kind.verb()
            ),
            date,
        });
        self.forget_unreportable_events(date);
    }

    /// Drop the events no `DescribeEvents` window can report any more: those older than
    /// [`EVENT_RETENTION`], and — once the log is longer than [`MAX_EVENTS_RETAINED`] —
    /// the oldest beyond that ceiling. Called on the RECORDING path, so a read never
    /// changes which events a later caller sees.
    fn forget_unreportable_events(&mut self, now: SystemTime) {
        if let Some(cutoff) = now.checked_sub(EVENT_RETENTION) {
            self.events.retain(|event| event.date >= cutoff);
        }
        if let Some(excess) = self.events.len().checked_sub(MAX_EVENTS_RETAINED) {
            self.events.drain(..excess);
        }
    }

    /// Materialise the resources this account ALREADY HAD before any call: MemoryDB
    /// creates the `default` user and the `open-access` ACL that holds it in every
    /// account, and owns one immutable `default.<family>` parameter group per parameter
    /// group family, so all of them are part of the store's initial contents rather than
    /// something a caller made.
    ///
    /// The engine hands every service an EMPTY store, so the built-ins are written on
    /// first touch instead of by a constructor; the flag makes that a one-off, so a
    /// later delete of a built-in is not undone by the next call. `region` is the
    /// requesting caller's, exactly as every other resource in this store takes the
    /// region of the call that brought it into being.
    pub fn ensure_builtins(&mut self, region: &str) {
        if self.builtins_materialised {
            return;
        }
        self.builtins_materialised = true;
        self.users.insert(
            DEFAULT_USER_NAME.to_string(),
            StoredUser {
                name: DEFAULT_USER_NAME.to_string(),
                status: user_status::ACTIVE.to_string(),
                access_string: DEFAULT_USER_ACCESS_STRING.to_string(),
                authentication_type: NO_PASSWORD_AUTHENTICATION.to_string(),
                password_count: None,
                // The account's own user is ready before any call, so the clock owes
                // it no transition.
                settles_at: None,
                arn: user_arn(region, DEFAULT_USER_NAME),
            },
        );
        self.acls.insert(
            OPEN_ACCESS_ACL_NAME.to_string(),
            StoredAcl {
                name: OPEN_ACCESS_ACL_NAME.to_string(),
                // An ACL the account already had is usable: there is no create for the
                // clock to owe a transition to.
                status: acl_status::ACTIVE.to_string(),
                settles_at: None,
                user_names: vec![DEFAULT_USER_NAME.to_string()],
                // An ACL nobody has asked to change has no change being applied to it.
                pending_user_names_to_add: Vec::new(),
                pending_user_names_to_remove: Vec::new(),
                arn: acl_arn(region, OPEN_ACCESS_ACL_NAME),
            },
        );
        // One default parameter group per parameter group family: MemoryDB provides the
        // family baseline in every account, so a caller can read a family's engine
        // defaults (and compare a custom group against them) without creating anything.
        // The families come from the engine catalog
        // ([`parameter_group_families`]), so the groups that exist here are exactly the
        // families `DescribeEngineVersions` reports.
        for family in parameter_group_families() {
            let name = default_parameter_group_name(&family);
            let description = default_parameter_group_description(&family);
            let arn = parameter_group_arn(region, &name);
            self.parameter_groups.insert(
                name.clone(),
                StoredParameterGroup {
                    name,
                    family,
                    description,
                    arn,
                    // The family baseline overrides nothing: every parameter a default
                    // group reports is its family's engine default, and the group is
                    // immutable, so nothing can ever be stored here.
                    parameter_values: BTreeMap::new(),
                },
            );
        }
    }

    /// `ACL$MinimumEngineVersion` — "The minimum engine version supported for the ACL":
    /// the greatest minimum its MEMBERS require, because an ACL can only be attached to
    /// a cluster whose engine supports every one of its users.
    ///
    /// DERIVED from the stored users, exactly as `ACL$Clusters` is derived from the
    /// clusters, so the version an ACL reports follows every membership change and
    /// cannot disagree with what its members report.
    pub fn acl_minimum_engine_version(&self, user_names: &[String]) -> &'static str {
        if user_names
            .iter()
            .filter_map(|name| self.users.get(name))
            .any(|user| user.minimum_engine_version() == MIN_ENGINE_VERSION_IAM_AUTH)
        {
            MIN_ENGINE_VERSION_IAM_AUTH
        } else {
            MIN_ENGINE_VERSION_PASSWORD_AUTH
        }
    }

    /// The names of the clusters currently referencing `acl_name`, in name order —
    /// what `ACL$Clusters` ("A list of clusters associated with the ACL") reports.
    pub fn clusters_using_acl(&self, acl_name: &str) -> Vec<String> {
        self.clusters
            .values()
            .filter(|cluster| cluster.acl_name == acl_name)
            .map(|cluster| cluster.name.clone())
            .collect()
    }

    /// The same membership for EVERY ACL, in ONE pass over the clusters.
    ///
    /// A listing uses this rather than [`Self::clusters_using_acl`] per ACL, so the
    /// work of describing N ACLs is linear in the store rather than N scans of it.
    pub fn acl_membership(&self) -> BTreeMap<&str, Vec<String>> {
        let mut membership: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for cluster in self.clusters.values() {
            membership
                .entry(cluster.acl_name.as_str())
                .or_default()
                .push(cluster.name.clone());
        }
        membership
    }

    /// The names of the ACLs whose `UserNames` include each user, in name order —
    /// what `User$ACLNames` ("The names of the Access Control Lists to which the user
    /// belongs") reports. DERIVED from the ACLs, exactly as `ACL$Clusters` is derived
    /// from the clusters, so one membership cannot disagree with the other.
    ///
    /// ONE pass over the ACLs answers EVERY user, so describing N users costs one scan
    /// of the ACLs rather than N; a single-user describe takes its own entry out of
    /// the result rather than scanning again under a second copy of this rule.
    pub fn user_membership(&self) -> BTreeMap<&str, Vec<String>> {
        let mut membership: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for acl in self.acls.values() {
            for user_name in &acl.user_names {
                membership
                    .entry(user_name.as_str())
                    .or_default()
                    .push(acl.name.clone());
            }
        }
        membership
    }

    /// The modelled `TagList` of the resource at `resource_arn`, in key order, or an
    /// empty list for a resource carrying no tags.
    ///
    /// Every `TagList` this service answers with is built here, from the store, so
    /// `ListTags` and the two tag mutations cannot report different tags for one
    /// resource.
    pub fn tag_list(&self, resource_arn: &str) -> Vec<Tag> {
        self.tags
            .get(resource_arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Replace the tags of the resource at `resource_arn`.
    ///
    /// An EMPTY set removes the entry rather than storing an empty one, so a resource
    /// created without tags — including one re-created under a name whose ARN once
    /// carried tags — starts with none of them.
    pub fn set_tags(&mut self, resource_arn: &str, tags: BTreeMap<String, String>) {
        if tags.is_empty() {
            self.tags.remove(resource_arn);
        } else {
            self.tags.insert(resource_arn.to_string(), tags);
        }
    }

    /// Resolve a tagging call's `ResourceArn` to the resource it names.
    ///
    /// Tags belong to a resource, so each of `ListTags`, `TagResource` and
    /// `UntagResource` resolves the ARN against this store before reading or writing a
    /// tag, rather than serving a tag set for an ARN nothing answers for:
    ///
    /// * an ARN that is not a MemoryDB resource ARN at all —
    ///   [`TagTargetRejection::InvalidArn`], the 400 `InvalidARNFault`;
    /// * a well-formed one naming no resource in this store —
    ///   [`TagTargetRejection::NotFound`] carrying the kind whose 404 the operation
    ///   models. A resource's ARN is STORED rather than rebuilt per call, so an ARN
    ///   naming an existing resource in another region or account resolves to nothing
    ///   too.
    pub fn tag_target(&self, resource_arn: &str) -> Result<String, TagTargetRejection> {
        let Some((kind, name)) = parse_resource_arn(resource_arn) else {
            return Err(TagTargetRejection::InvalidArn(format!(
                "{resource_arn} is not a valid MemoryDB resource ARN."
            )));
        };
        let stored = match kind {
            ResourceKind::Cluster => self.clusters.get(name).map(|cluster| cluster.arn.as_str()),
            ResourceKind::Acl => self.acls.get(name).map(|acl| acl.arn.as_str()),
            ResourceKind::User => self.users.get(name).map(|user| user.arn.as_str()),
            ResourceKind::ParameterGroup => self
                .parameter_groups
                .get(name)
                .map(|group| group.arn.as_str()),
            // Snapshots, subnet groups and the multi-Region resources have no store in
            // this service yet (their creates are still scaffolded), so no ARN of those
            // kinds can name a resource that exists — which is exactly what their
            // not-found faults say.
            ResourceKind::Snapshot
            | ResourceKind::SubnetGroup
            | ResourceKind::MultiRegionCluster
            | ResourceKind::MultiRegionParameterGroup => None,
        };
        if stored == Some(resource_arn) {
            return Ok(resource_arn.to_string());
        }
        Err(TagTargetRejection::NotFound(
            kind,
            format!("{} {name} not found.", kind.label()),
        ))
    }

    /// Take `user_name` out of the `UserNames` of every ACL that lists it, answering
    /// the names of those ACLs in name order.
    ///
    /// `DeleteUser` — "The user will be removed from all ACLs and in turn removed from
    /// all clusters" — is what owes this: membership lives ONCE, on the ACL
    /// ([`Self::user_membership`] derives `User$ACLNames` from it), so a user that is
    /// gone has to leave the ACLs rather than linger in a list no user resolves
    /// through. The returned names are the membership the user held UP TO this call,
    /// which is what the delete reports back.
    pub fn remove_user_from_every_acl(&mut self, user_name: &str) -> Vec<String> {
        let mut left_behind = Vec::new();
        for acl in self.acls.values_mut() {
            let before = acl.user_names.len();
            acl.user_names.retain(|name| name != user_name);
            if acl.user_names.len() != before {
                left_behind.push(acl.name.clone());
            }
        }
        left_behind
    }
}

/// A cluster as this emulator persists it: every member `CreateCluster` accepted, plus
/// the values the service itself derives (ARN, endpoint, shard layout, status).
#[derive(Debug, Clone)]
pub struct StoredCluster {
    pub name: String,
    /// The region this cluster was CREATED in, carried in its ARN, its configuration
    /// endpoint and every node endpoint and availability zone. Stored, so a later call
    /// signed for another region reports the cluster where it actually lives instead of
    /// rewriting half of it.
    pub region: String,
    pub description: Option<String>,
    pub status: String,
    /// When the clock owes this cluster `available`, or `None` once it is settled.
    pub settles_at: Option<Instant>,
    pub node_type: String,
    pub number_of_shards: i32,
    pub num_replicas_per_shard: i32,
    pub acl_name: String,
    pub multi_region_cluster_name: Option<String>,
    pub parameter_group_name: Option<String>,
    pub subnet_group_name: Option<String>,
    pub security_group_ids: Vec<String>,
    pub maintenance_window: Option<String>,
    pub snapshot_window: Option<String>,
    pub snapshot_retention_limit: Option<i32>,
    pub sns_topic_arn: Option<String>,
    pub sns_topic_status: Option<String>,
    pub tls_enabled: Option<bool>,
    pub kms_key_id: Option<String>,
    pub engine: Option<String>,
    pub engine_version: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    pub data_tiering: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub port: i32,
    pub endpoint_address: String,
    pub arn: String,
    pub shards: Vec<StoredShard>,
    pub created_at: SystemTime,
}

/// One shard of a stored cluster, laid out once at create time and persisted, so
/// every later call reports the same shards, slots and nodes.
#[derive(Debug, Clone)]
pub struct StoredShard {
    pub name: String,
    pub status: String,
    pub slots: String,
    pub nodes: Vec<StoredNode>,
}

/// One node of a stored shard.
#[derive(Debug, Clone)]
pub struct StoredNode {
    pub name: String,
    pub status: String,
    pub availability_zone: String,
    pub endpoint_address: String,
    pub port: i32,
    pub created_at: SystemTime,
}

/// A parameter group as this emulator persists it: the three members
/// `CreateParameterGroup` accepts, the ARN the service itself mints, and the parameter
/// values a later update changed.
///
/// A parameter group has NO status member in the model (`ParameterGroup` is
/// `Name`/`Family`/`Description`/`ARN`), so there is no transient status to settle and
/// nothing for the clock to owe it.
#[derive(Debug, Clone)]
pub struct StoredParameterGroup {
    pub name: String,
    /// `ParameterGroup$Family`: "The name of the parameter group family that this
    /// parameter group is compatible with." Stored folded to lower case like every
    /// other name in this store — the model publishes no catalog of families, so the
    /// service records the caller's rather than matching it against a guessed list.
    pub family: String,
    /// `ParameterGroup$Description`. NOT optional in the store: a group created
    /// without one gets the service's own default
    /// ([`DEFAULT_PARAMETER_GROUP_DESCRIPTION`]), so every stored group has a
    /// description to report.
    pub description: String,
    pub arn: String,
    /// The values an `UpdateParameterGroup` persisted for this group, by parameter
    /// name. ONLY the overrides: a parameter the group never changed reports its
    /// family's engine default (see [`family_parameters`]), so one stored group is not
    /// a frozen copy of a family's defaults.
    pub parameter_values: BTreeMap<String, String>,
}

/// An ACL as this emulator persists it. `Clusters` is NOT stored: it is derived from
/// the clusters currently referencing the ACL (see
/// [`ComAmazonawsMemorydbAmazonmemorydbState::clusters_using_acl`]).
#[derive(Debug, Clone)]
pub struct StoredAcl {
    pub name: String,
    pub status: String,
    /// When the clock owes this ACL `active`, or `None` once it is settled.
    pub settles_at: Option<Instant>,
    pub user_names: Vec<String>,
    /// The members the change this ACL is still applying ADDS, in name order, or empty
    /// when no change is in flight. Written by the operation that accepts a membership
    /// change and cleared by [`settle`] when the ACL is back to
    /// [`acl_status::ACTIVE`], which is what `ACL$PendingChanges` reports.
    ///
    /// [`settle`]: ComAmazonawsMemorydbAmazonmemorydbState::settle
    pub pending_user_names_to_add: Vec<String>,
    /// The members that same change REMOVES, held and cleared the same way.
    pub pending_user_names_to_remove: Vec<String>,
    pub arn: String,
}

/// A user as this emulator persists it. `ACLNames` is NOT stored: it is derived from
/// the ACLs currently listing the user (see
/// [`ComAmazonawsMemorydbAmazonmemorydbState::user_membership`]).
///
/// The user's PASSWORDS are deliberately absent too: `Authentication$PasswordCount` is
/// the only thing AWS reports about them, so only the count is kept and the secrets
/// themselves are never stored or returned.
#[derive(Debug, Clone)]
pub struct StoredUser {
    pub name: String,
    pub status: String,
    pub access_string: String,
    /// `Authentication$Type`, one of [`AUTHENTICATION_TYPE_VALUES`].
    ///
    /// [`AUTHENTICATION_TYPE_VALUES`]: crate::ops::types::AUTHENTICATION_TYPE_VALUES
    pub authentication_type: String,
    /// `Authentication$PasswordCount`: "The number of passwords belonging to the
    /// user", absent for a user that authenticates without one.
    pub password_count: Option<i32>,
    /// When the clock owes this user [`user_status::ACTIVE`] again, or `None` once it
    /// is settled. Set by the operation that moves the user off `active` (an update
    /// answers with it `modifying`), and materialised by [`settle`].
    ///
    /// [`settle`]: ComAmazonawsMemorydbAmazonmemorydbState::settle
    pub settles_at: Option<Instant>,
    pub arn: String,
}

impl StoredUser {
    /// `User$MinimumEngineVersion` — "The minimum engine version supported for the
    /// user": the earliest engine version whose RBAC supports how this user
    /// authenticates. IAM authentication arrived with
    /// [`MIN_ENGINE_VERSION_IAM_AUTH`]; a password or password-less user works from
    /// [`MIN_ENGINE_VERSION_PASSWORD_AUTH`] on.
    pub fn minimum_engine_version(&self) -> &'static str {
        if self.authentication_type == input_authentication_type::IAM {
            MIN_ENGINE_VERSION_IAM_AUTH
        } else {
            MIN_ENGINE_VERSION_PASSWORD_AUTH
        }
    }

    /// The modelled `User` for this stored user. `acl_names` is the derived membership
    /// list, which only the state as a whole can compute.
    pub fn to_api(&self, acl_names: Vec<String>) -> User {
        User {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            access_string: Some(self.access_string.clone()),
            acl_names: Some(acl_names),
            // DERIVED from how the user authenticates, so it follows the stored
            // authentication type rather than being recorded a second time.
            minimum_engine_version: Some(self.minimum_engine_version().to_string()),
            authentication: Some(Authentication {
                r#type: Some(self.authentication_type.clone()),
                password_count: self.password_count,
            }),
            arn: Some(self.arn.clone()),
        }
    }
}

impl StoredCluster {
    /// The modelled `Cluster` for this stored cluster.
    ///
    /// `DescribeClusters$ShowShardDetails` is "An optional flag that can be included
    /// in the request to retrieve information about the individual shard(s)", so
    /// `Shards` is populated only when the caller asked for it.
    pub fn to_api(&self, show_shard_details: bool) -> Cluster {
        Cluster {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            status: Some(self.status.clone()),
            pending_updates: None,
            multi_region_cluster_name: self.multi_region_cluster_name.clone(),
            number_of_shards: Some(self.number_of_shards),
            shards: show_shard_details
                .then(|| self.shards.iter().map(StoredShard::to_api).collect()),
            availability_mode: Some(
                if self.num_replicas_per_shard > 0 {
                    availability_mode::MULTI_AZ
                } else {
                    availability_mode::SINGLE_AZ
                }
                .to_string(),
            ),
            cluster_endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: self.port,
            }),
            node_type: Some(self.node_type.clone()),
            engine: self.engine.clone(),
            engine_version: self.engine_version.clone(),
            engine_patch_version: None,
            parameter_group_name: self.parameter_group_name.clone(),
            // Left ABSENT deliberately: `ParameterGroupStatus` has its own vocabulary
            // (a parameter group is in sync with the cluster, or being applied to it),
            // and nothing tracks the application of a parameter group to a cluster —
            // the operations that would (`UpdateParameterGroup`, `UpdateCluster`'s
            // `ParameterGroupName`) do not record it. Reporting the cluster's own
            // status here would be a plausible-looking token for state nothing tracks.
            parameter_group_status: None,
            security_groups: Some(
                self.security_group_ids
                    .iter()
                    .map(|id| SecurityGroupMembership {
                        security_group_id: Some(id.clone()),
                        status: Some(AVAILABLE.to_string()),
                    })
                    .collect(),
            ),
            subnet_group_name: self.subnet_group_name.clone(),
            tls_enabled: self.tls_enabled,
            kms_key_id: self.kms_key_id.clone(),
            arn: Some(self.arn.clone()),
            sns_topic_arn: self.sns_topic_arn.clone(),
            sns_topic_status: self.sns_topic_status.clone(),
            snapshot_retention_limit: self.snapshot_retention_limit,
            maintenance_window: self.maintenance_window.clone(),
            snapshot_window: self.snapshot_window.clone(),
            acl_name: Some(self.acl_name.clone()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            // `DataTieringStatus` is an enum of the strings "true"/"false", not a
            // boolean, so the stored flag renders as its wire token.
            data_tiering: self
                .data_tiering
                .map(|enabled| if enabled { "true" } else { "false" }.to_string()),
            network_type: self.network_type.clone(),
            ip_discovery: self.ip_discovery.clone(),
        }
    }
}

impl StoredShard {
    /// The modelled `Shard` for this stored shard.
    pub fn to_api(&self) -> Shard {
        Shard {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            slots: Some(self.slots.clone()),
            number_of_nodes: Some(i32::try_from(self.nodes.len()).unwrap_or(i32::MAX)),
            nodes: Some(self.nodes.iter().map(StoredNode::to_api).collect()),
        }
    }
}

impl StoredNode {
    /// The modelled `Node` for this stored node.
    pub fn to_api(&self) -> Node {
        Node {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            availability_zone: Some(self.availability_zone.clone()),
            create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: self.port,
            }),
        }
    }
}

impl StoredParameterGroup {
    /// The parameters this group reports, keyed by name for [`paginate`] and in name
    /// order: its FAMILY's engine defaults, each carrying the value this group
    /// persisted for it when an update changed it.
    ///
    /// So the listing of a group follows the family it was created with, and a value a
    /// caller never set is the family's default rather than a stored copy of it.
    pub fn parameters(&self) -> Vec<(String, Parameter)> {
        family_parameters(&self.family)
            .into_iter()
            .map(|parameter| {
                let value = self
                    .parameter_values
                    .get(parameter.name)
                    .map_or(parameter.default_value, String::as_str);
                (parameter.name.to_string(), parameter.to_api(value))
            })
            .collect()
    }

    /// The modelled `ParameterGroup` for this stored parameter group.
    pub fn to_api(&self) -> ParameterGroup {
        ParameterGroup {
            name: Some(self.name.clone()),
            family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            arn: Some(self.arn.clone()),
        }
    }
}

impl StoredAcl {
    /// `ACL$PendingChanges` — "A list of updates being applied to the ACL": the members
    /// the change this ACL is still applying adds and removes, or `None` once there is
    /// no change in flight.
    ///
    /// Each list is ABSENT rather than empty when that side of the change names nobody,
    /// so a caller reading `PendingChanges` sees only what is actually being applied;
    /// the whole member is absent when the ACL has settled, which is how a client that
    /// polls it learns the change is complete.
    pub fn pending_changes(&self) -> Option<ACLPendingChanges> {
        if self.pending_user_names_to_add.is_empty()
            && self.pending_user_names_to_remove.is_empty()
        {
            return None;
        }
        Some(ACLPendingChanges {
            user_names_to_add: (!self.pending_user_names_to_add.is_empty())
                .then(|| self.pending_user_names_to_add.clone()),
            user_names_to_remove: (!self.pending_user_names_to_remove.is_empty())
                .then(|| self.pending_user_names_to_remove.clone()),
        })
    }

    /// The modelled `ACL` for this stored ACL. `clusters` is the derived membership
    /// list and `minimum_engine_version` the version its members require, both of
    /// which only the state as a whole can compute
    /// ([`ComAmazonawsMemorydbAmazonmemorydbState::clusters_using_acl`] /
    /// [`ComAmazonawsMemorydbAmazonmemorydbState::acl_minimum_engine_version`]).
    pub fn to_api(&self, clusters: Vec<String>, minimum_engine_version: &str) -> ACL {
        ACL {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            user_names: Some(self.user_names.clone()),
            minimum_engine_version: Some(minimum_engine_version.to_string()),
            // BUILT FROM the stored pending lists, so the update that recorded the
            // change and every later read report the same in-flight change — and it
            // disappears once the clock has settled the ACL.
            pending_changes: self.pending_changes(),
            clusters: Some(clusters),
            arn: Some(self.arn.clone()),
        }
    }
}

/// Why an `AuthenticationMode` cannot be accepted, in the two classes MemoryDB models
/// separately: a member whose VALUE breaks a rule, and members that cannot be
/// COMBINED. The caller maps each onto the modelled error its own operation declares
/// (`InvalidParameterValueException` / `InvalidParameterCombinationException`), so one
/// rule serves every operation that takes an `AuthenticationMode`.
#[derive(Debug, Clone)]
pub enum AuthenticationModeRejection {
    /// A member's value is outside what the service accepts.
    Value(String),
    /// The members supplied cannot hold together.
    Combination(String),
}

/// The `Authentication` a stored user gets from the `AuthenticationMode` a request
/// supplies, or why the mode is refused.
///
/// The documented password requirements (AWS, "Authenticating users with Access
/// Control Lists") are enforced here rather than per operation, because `CreateUser`
/// and `UpdateUser` take the same member and so must refuse the same values:
///
/// * `Type` names an `InputAuthenticationType` (`password` or `iam`);
/// * `password` requires between 1 and [`MAX_PASSWORDS_PER_USER`] passwords, `iam`
///   accepts none at all;
/// * each password is printable ASCII and [`MIN_PASSWORD_LEN`]..=[`MAX_PASSWORD_LEN`]
///   characters long. The Developer Guide's narrower character list is NOT enforced:
///   the service accepts a password carrying any of it.
pub fn authentication_from_mode(
    mode: &AuthenticationMode,
) -> Result<(String, Option<i32>), AuthenticationModeRejection> {
    let passwords = mode.passwords.as_deref().unwrap_or(&[]);
    let mode_type = mode.r#type.as_deref().unwrap_or_default();
    match mode_type {
        input_authentication_type::PASSWORD => {
            if passwords.is_empty() {
                // The service's own wording for the combination it cannot act on: the
                // authentication type the request ASKED for, and what that type needs.
                return Err(AuthenticationModeRejection::Combination(format!(
                    "A user with Authentication Mode: {}, must have at least one \
                     password",
                    input_authentication_type::PASSWORD
                )));
            }
            if passwords.len() > MAX_PASSWORDS_PER_USER {
                // The service's own wording for the password quota, built from the
                // bound `Authentication$PasswordCount` documents, so a caller reads
                // which limit it broke and what the limit is.
                return Err(AuthenticationModeRejection::Value(format!(
                    "Maximum number of passwords allowed in this service is \
                     {MAX_PASSWORDS_PER_USER}."
                )));
            }
            for password in passwords {
                validate_password(password)?;
            }
            // At most `MAX_PASSWORDS_PER_USER` (2) — anything larger was refused just
            // above — so the conversion is exact. The fallback is unreachable and is
            // there only to keep the handler panic-free.
            let count = i32::try_from(passwords.len()).unwrap_or(i32::MAX);
            Ok((mode_type.to_string(), Some(count)))
        }
        input_authentication_type::IAM => {
            if !passwords.is_empty() {
                // The same rule from the other side: the refusal names the member that
                // does not belong with the authentication type the request asked for.
                return Err(AuthenticationModeRejection::Combination(format!(
                    "Password field is not allowed with authentication type: {}",
                    input_authentication_type::IAM
                )));
            }
            // An IAM-authenticated user holds no passwords, so there is no count to
            // report for it.
            Ok((mode_type.to_string(), None))
        }
        other => Err(AuthenticationModeRejection::Value(format!(
            "AuthenticationMode.Type must be one of {} — got {other:?}.",
            // The GENERATED catalog, so the refusal cannot name a different set from
            // the one the model declares.
            crate::ops::types::INPUT_AUTHENTICATION_TYPE_VALUES.join(", ")
        ))),
    }
}

/// One password against the documented requirements. The message names the broken
/// rule and never repeats the password itself.
fn validate_password(password: &str) -> Result<(), AuthenticationModeRejection> {
    if !(MIN_PASSWORD_LEN..=MAX_PASSWORD_LEN).contains(&password.chars().count()) {
        // The refusal names the member being refused and the documented bound, in the
        // service's own wording: `<member> length must be between <min>-<max>
        // characters.`
        return Err(AuthenticationModeRejection::Value(format!(
            "Passwords length must be between {MIN_PASSWORD_LEN}-{MAX_PASSWORD_LEN} \
             characters."
        )));
    }
    // "Must be only printable ASCII characters": the ASCII graphic range plus the
    // space, so a control character or any non-ASCII character is refused.
    if !password
        .chars()
        .all(|character| character.is_ascii_graphic() || character == ' ')
    {
        return Err(AuthenticationModeRejection::Value(
            "A user password must contain only printable ASCII characters.".to_string(),
        ));
    }
    // The Developer Guide's character exclusion (`,` `"` `/` `@`) is deliberately NOT
    // enforced: the service accepts a password carrying any of them, and the model puts
    // no `@pattern` on `PasswordListInput`'s members, so length and the printable-ASCII
    // rule above are the whole of it.
    Ok(())
}

/// Why a membership list this long cannot be served, or `None` when it can.
///
/// Read by BOTH write paths of an ACL's membership before they do any per-name work, so
/// one request cannot size the stored membership or the comparisons against it.
pub fn member_count_rejection(member: &str, count: usize) -> Option<String> {
    (count > MAX_USERS_PER_ACL).then(|| {
        format!("{member} must not name more than {MAX_USERS_PER_ACL} users.")
    })
}

/// A MemoryDB resource name as the service STORES it: folded to lower case.
///
/// User, ACL, cluster and parameter-group names (and a parameter group's family) are
/// case-insensitive identifiers — the mixed-case and the lower-case spelling are one
/// resource — so every write path folds the name on the way into state and every
/// lookup folds the name it was given, and the two cannot disagree about which
/// resource a request named.
pub fn fold_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Why `name` cannot name a MemoryDB resource of this kind, or `None` when it can.
///
/// The service enforces a TIGHTER rule than the model's `^[a-zA-Z][a-zA-Z0-9\-]*$`:
/// at most [`MAX_RESOURCE_NAME_LEN`] characters, beginning with a letter, only ASCII
/// letters, digits and hyphens, and neither a trailing hyphen nor two consecutive
/// hyphens. Both refusals name the identifier the way the SERVICE names it
/// ([`ResourceKind::name_label`]) rather than the wire member the request carried it
/// in, which is how a caller learns which rule it broke; the caller maps the refusal
/// onto its own modelled `InvalidParameterValueException`.
pub fn resource_name_rejection(kind: ResourceKind, name: &str) -> Option<String> {
    let label = kind.name_label();
    if name.chars().count() > MAX_RESOURCE_NAME_LEN {
        return Some(format!(
            "{label} length must be less than {MAX_RESOURCE_NAME_LEN}."
        ));
    }
    let well_formed = name.starts_with(|first: char| first.is_ascii_alphabetic())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        && !name.ends_with('-')
        && !name.contains("--");
    (!well_formed).then(|| {
        format!(
            "{label} must begin with a letter; must contain only ASCII letters, digits, \
             and hyphens; and must not end with a hyphen or contain two consecutive \
             hyphens."
        )
    })
}

/// The message a create's already-exists refusal carries, built from the kind of
/// resource and the FOLDED name the request named.
///
/// Each kind has the service's OWN template for it, which is what a caller keying on
/// the refusal reads: an ACL's also says what to do about it, and a parameter group's
/// names no group at all.
pub fn already_exists_message(kind: ResourceKind, name: &str) -> String {
    match kind {
        ResourceKind::Acl => {
            format!("ACL {name} already exists. Please retry with another name.")
        }
        ResourceKind::ParameterGroup | ResourceKind::MultiRegionParameterGroup => {
            "Parameter Group already exists.".to_string()
        }
        _ => format!("{} {name} already exists.", kind.label()),
    }
}

/// What a request asks of a resource, in the word the service's refusal uses for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    /// An update of the resource (`UpdateUser`, `UpdateACL`, …).
    Modify,
    /// A delete of the resource (`DeleteUser`, `DeleteACL`, …).
    Delete,
}

impl Mutation {
    /// How a refusal names what was asked.
    fn participle(self) -> &'static str {
        match self {
            Self::Modify => "modified",
            Self::Delete => "deleted",
        }
    }
}

/// Why a resource MemoryDB itself provides is not the caller's to change, or `None`
/// when the request names a resource the caller owns.
///
/// The built-ins are the ones [`ComAmazonawsMemorydbAmazonmemorydbState::ensure_builtins`]
/// materialises — the `default` user and the `open-access` ACL — and EVERY write path
/// over one reads this, so a caller is refused with one template whichever operation it
/// sent: the resource named by the name the service gives it, and the action refused.
/// The caller maps the refusal onto its own modelled `InvalidParameterValueException`.
pub fn builtin_resource_rejection(
    kind: ResourceKind,
    name: &str,
    mutation: Mutation,
) -> Option<String> {
    let built_in = match kind {
        ResourceKind::User => name == DEFAULT_USER_NAME,
        ResourceKind::Acl => name == OPEN_ACCESS_ACL_NAME,
        // The `default.<family>` parameter groups are built-in too, but the service
        // refuses a write to one with its own wording, which names no group — see
        // [`immutable_parameter_group_rejection`].
        _ => false,
    };
    built_in.then(|| {
        format!(
            "{name} {} can't be {}.",
            kind.noun(),
            mutation.participle()
        )
    })
}

/// Why a listing cannot serve this `MaxResults`, or `None` when it can.
///
/// EVERY paginated listing in this service reads the page-size window through here, so
/// one request's page size is refused with the same rule and the same message whichever
/// listing it was sent to. The caller maps the refusal onto its own modelled
/// `InvalidParameterValueException`.
pub fn max_results_rejection(max_results: Option<i32>) -> Option<String> {
    let max = max_results?;
    (!(MIN_MAX_RESULTS..=MAX_MAX_RESULTS).contains(&max)).then(|| {
        format!(
            "Invalid value for max results. Must be between {MIN_MAX_RESULTS} and \
             {MAX_MAX_RESULTS}"
        )
    })
}

/// The canonical form of an access string, which is what the service stores and
/// reports for `User$AccessString`.
///
/// `AccessString` is a Redis OSS / Valkey ACL rule list, and MemoryDB canonicalises it
/// before storing it: the rules are rendered one space apart, and the pub/sub channel
/// permission is always stated — a string that names no channel rule gets the engine's
/// own default, [`DEFAULT_CHANNEL_RULE`], in the position the canonical order puts it
/// (after the key patterns, before the command rules). So `on ~* +@all` is stored and
/// reported as `on ~* resetchannels +@all`, while a string that already names its
/// channels (`on ~* &* +@all`) keeps them.
pub fn normalise_access_string(access_string: &str) -> String {
    let mut rules: Vec<&str> = access_string.split_whitespace().collect();
    let names_channels = rules.iter().any(|rule| is_channel_rule(rule));
    if !names_channels {
        // The canonical order states the channel permission after the key patterns and
        // before the command rules, so the default goes in front of the first command
        // rule — or at the end when the string carries none.
        let position = rules
            .iter()
            .position(|rule| rule.starts_with('+') || rule.starts_with('-'))
            .unwrap_or(rules.len());
        rules.insert(position, DEFAULT_CHANNEL_RULE);
    }
    rules.join(" ")
}

/// Whether one ACL rule states the pub/sub channel permission: a channel pattern
/// (`&<pattern>`) or one of the two keywords that set the whole permission at once.
fn is_channel_rule(rule: &str) -> bool {
    rule.starts_with('&')
        || rule.eq_ignore_ascii_case(DEFAULT_CHANNEL_RULE)
        || rule.eq_ignore_ascii_case("allchannels")
}

/// Why no engine version this service runs matches `engine_version` under `engine`, or
/// `None` when one does.
///
/// A MemoryDB engine version belongs to ONE engine ([`ENGINE_VERSIONS`] is the catalog
/// `DescribeEngineVersions` publishes), so naming an engine together with a version
/// another engine offers is a combination that selects nothing — which is why the
/// caller answers it with its modelled `InvalidParameterCombinationException`. A
/// request that names no engine is checked against every engine's versions, because any
/// of them could be the one it meant.
///
/// The version a cluster is created or updated with is the `<major>.<minor>` form
/// `EngineVersionInfo$EngineVersion` publishes, NOT the `<major>.<minor>.<patch>` form
/// of `EngineVersionInfo$EnginePatchVersion`: the patch release is the service's to
/// choose, so a patch version is not a value a cluster can ask for and is refused here
/// like any other version no engine publishes.
pub fn engine_version_rejection(engine: Option<&str>, engine_version: Option<&str>) -> Option<String> {
    let version = engine_version?;
    let offered = ENGINE_VERSIONS.iter().any(|candidate| {
        engine.is_none_or(|engine| engine == candidate.engine) && version == candidate.version()
    });
    (!offered).then(|| format!("Invalid engine version {version} specified for the cluster"))
}

/// Why `engine` names no engine this service runs, or `None` when it names one.
///
/// The model types `Engine` as a plain string, but the engines MemoryDB runs are the
/// ones [`ENGINE_VERSIONS`] publishes, so anything else is a value the service cannot
/// honour and the caller refuses it with its modelled
/// `InvalidParameterValueException`.
pub fn engine_rejection(engine: Option<&str>) -> Option<String> {
    let engine = engine?;
    let offered = ENGINE_VERSIONS
        .iter()
        .any(|candidate| candidate.engine == engine);
    (!offered).then(|| format!("Invalid engine name {engine} specified for the cluster"))
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope. Every ARN and endpoint a handler mints goes through here,
/// so a resource's region is the caller's rather than a constant.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:memorydb:<region>:<account>:cluster/<name>`.
pub fn cluster_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("cluster/{name}"))
}

/// `arn:aws:memorydb:<region>:<account>:acl/<name>`.
pub fn acl_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("acl/{name}"))
}

/// `arn:aws:memorydb:<region>:<account>:user/<name>`.
pub fn user_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("user/{name}"))
}

/// `arn:aws:memorydb:<region>:<account>:parametergroup/<name>`.
pub fn parameter_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("parametergroup/{name}"))
}

/// The per-cluster label AWS puts between the cluster name and the service domain in
/// an endpoint. AWS generates it randomly; this emulator derives it from the cluster's
/// name and region so the address a create returns is the address every later describe
/// returns, in this process and the next.
fn endpoint_suffix(region: &str, cluster_name: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{region}/{cluster_name}").as_bytes());
    hex::encode(&digest[..3])
}

/// The cluster's configuration endpoint host:
/// `<name>.<suffix>.clustercfg.memorydb.<region>.amazonaws.com`.
pub fn cluster_endpoint_address(region: &str, cluster_name: &str) -> String {
    let suffix = endpoint_suffix(region, cluster_name);
    format!("{cluster_name}.{suffix}.clustercfg.memorydb.{region}.amazonaws.com")
}

/// A node's own endpoint host:
/// `<node>.<suffix>.<cluster>.memorydb.<region>.amazonaws.com`.
fn node_endpoint_address(region: &str, cluster_name: &str, node_name: &str) -> String {
    let suffix = endpoint_suffix(region, cluster_name);
    format!("{node_name}.{suffix}.{cluster_name}.memorydb.{region}.amazonaws.com")
}

/// Lay out `number_of_shards` shards, each with one primary plus
/// `num_replicas_per_shard` replicas, over the cluster's hash slots.
///
/// Called once, by the operation that changes the shard count; the result is persisted
/// so no two calls can compute a different layout for the same cluster.
///
/// BOTH factors are CLAMPED to [`MAX_SHARDS_PER_CLUSTER`] / [`MAX_REPLICAS_PER_SHARD`]
/// here as well as refused by the write paths, so neither this function's single
/// allocation nor its loop can ever be sized by an out-of-range request field, however
/// it is reached. The clamped product is at most
/// `MAX_SHARDS_PER_CLUSTER * (MAX_REPLICAS_PER_SHARD + 1)` nodes, which no i32
/// arithmetic below can overflow. The `debug_assert!`s say what the write paths already
/// guarantee, so a future caller that relies on the clamp instead fails loudly in a
/// debug build rather than being silently corrected.
pub fn build_shards(
    region: &str,
    cluster_name: &str,
    number_of_shards: i32,
    num_replicas_per_shard: i32,
    port: i32,
    created_at: SystemTime,
) -> Vec<StoredShard> {
    debug_assert!(
        (1..=MAX_SHARDS_PER_CLUSTER).contains(&number_of_shards),
        "number_of_shards {number_of_shards} must be refused by the caller, not clamped"
    );
    debug_assert!(
        (0..=MAX_REPLICAS_PER_SHARD).contains(&num_replicas_per_shard),
        "num_replicas_per_shard {num_replicas_per_shard} must be refused by the caller, \
         not clamped"
    );
    let shard_count = number_of_shards.clamp(1, MAX_SHARDS_PER_CLUSTER);
    let replicas = num_replicas_per_shard.clamp(0, MAX_REPLICAS_PER_SHARD);
    let base = TOTAL_SLOTS / shard_count;
    let remainder = TOTAL_SLOTS % shard_count;
    let mut next_slot = 0;
    let mut shards = Vec::with_capacity(shard_count as usize);
    for shard_index in 0..shard_count {
        // The first `remainder` shards carry one extra slot, so the shards together
        // cover `0..=16383` exactly once however many there are. `shard_count` is at
        // most TOTAL_SLOTS, so `base` is at least 1 and no shard is empty.
        let width = base + i32::from(shard_index < remainder);
        let first_slot = next_slot;
        let last_slot = first_slot + width - 1;
        next_slot = last_slot + 1;
        let shard_name = format!("{:04}", shard_index + 1);
        let nodes = (0..=replicas)
            .map(|node_index| {
                let node_name = format!("{cluster_name}-{shard_name}-{:03}", node_index + 1);
                StoredNode {
                    endpoint_address: node_endpoint_address(region, cluster_name, &node_name),
                    name: node_name,
                    status: AVAILABLE.to_string(),
                    // Nodes of one shard are spread over the region's zones, the
                    // primary and each replica landing in a different one.
                    availability_zone: format!(
                        "{region}{}",
                        char::from(b'a' + u8::try_from(node_index % 3).unwrap_or(0))
                    ),
                    port,
                    created_at,
                }
            })
            .collect();
        shards.push(StoredShard {
            name: shard_name,
            status: AVAILABLE.to_string(),
            slots: format!("{first_slot}-{last_slot}"),
            nodes,
        });
    }
    shards
}

/// One page of a name-ordered listing, plus the token that resumes after it.
///
/// `@paginated` on this service's listings names `MaxResults` as the page size and
/// `NextToken` as both tokens. The token is the name the NEXT page starts at, so
/// resuming means skipping every name that sorts before the token.
///
/// `None` means the token names no position in this listing — every name sorts before
/// it, so it was not minted for this listing (or everything from its target on was
/// deleted between pages). The caller answers that with its modelled invalid-parameter
/// error rather than serving a silently empty page: a token this emulator minted always
/// names a resource that existed when the page was served, so a token past the end is
/// reported rather than quietly treated as the end of the listing.
///
/// `limit` is an already-validated page size (the callers refuse a `MaxResults` below
/// 1 with a modelled 400 before converting), and it only ever caps a `take` over an
/// existing list, so it never sizes an allocation.
pub fn paginate<T>(
    entries: Vec<(String, T)>,
    limit: Option<usize>,
    next_token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match next_token {
        Some(token) => entries.iter().position(|(name, _)| name.as_str() >= token)?,
        None => 0,
    };
    let mut remaining = entries.into_iter().skip(start);
    let page: Vec<(String, T)> = match limit {
        Some(limit) => remaining.by_ref().take(limit).collect(),
        None => remaining.by_ref().collect(),
    };
    let next = remaining.next().map(|(name, _)| name);
    Some((page.into_iter().map(|(_, item)| item).collect(), next))
}

/// The already-validated page size of a listing request: `None` for "every remaining
/// item". Only reached once the caller has refused a `MaxResults` below 1.
pub fn page_limit(max_results: Option<i32>) -> Option<usize> {
    max_results.and_then(|max| usize::try_from(max).ok())
}

/// The name prefix of the parameter groups MemoryDB itself creates in every account
/// (`default.memorydb-redis7`, `default.memorydb-valkey7`, …).
///
/// A caller neither created those groups nor owns them, so they are not a caller's to
/// tag — see [`default_resource_rejection`].
pub const DEFAULT_PARAMETER_GROUP_PREFIX: &str = "default.";

/// The refusal for a tag mutation aimed at a resource MemoryDB itself provides, or
/// `None` when the ARN names a resource a caller owns.
///
/// Tags are a caller's own metadata on a caller's own resource; the service's DEFAULT
/// resources — the `default.<family>` parameter groups every account starts with —
/// belong to MemoryDB, and a tag mutation on one is refused instead of written. An ARN
/// this service cannot parse at all is not refused here: it is the ARN itself that is
/// wrong, which the caller is told instead.
pub fn default_resource_rejection(resource_arn: &str) -> Option<String> {
    let (kind, name) = parse_resource_arn(resource_arn)?;
    let is_default = match kind {
        ResourceKind::ParameterGroup | ResourceKind::MultiRegionParameterGroup => {
            name.starts_with(DEFAULT_PARAMETER_GROUP_PREFIX)
        }
        _ => false,
    };
    is_default.then(|| "Tagging on default resources is not supported".to_string())
}

/// The tag-key prefix AWS reserves for itself.
///
/// From AWS's tagging rules (and "Tagging your MemoryDB resources"): "Do not use the
/// `aws:` prefix in your tag names or values because it is reserved for AWS use. You
/// can't edit or delete tag names or values with this prefix." So a CALLER's tag key
/// may not carry it, in any letter case, and neither a create nor a tag mutation
/// accepts one.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What is wrong with a caller-supplied tag key, or `None` when the caller may use it.
///
/// Two rules, both holding wherever a request carries tag keys — a create's `Tags`,
/// `TagResource`'s `Tags` and `UntagResource`'s `TagKeys`:
///
/// * `Tag$Key` "may not be null", so a tag with no usable key names no tag at all;
/// * the `aws:` prefix is reserved ([`RESERVED_TAG_KEY_PREFIX`]), so a caller can
///   neither write nor remove a key that carries it.
pub fn tag_key_rejection(key: &str) -> Option<String> {
    if key.is_empty() {
        return Some("A tag key may not be null or empty.".to_string());
    }
    if key
        .to_ascii_lowercase()
        .starts_with(RESERVED_TAG_KEY_PREFIX)
    {
        return Some(format!(
            "Tag keys cannot start with the reserved prefix {RESERVED_TAG_KEY_PREFIX:?}."
        ));
    }
    None
}

/// The first thing wrong with a request's `Tags` member, or `None` when every tag in it
/// is one a caller may write. A request carrying no `Tags` at all breaks no rule.
pub fn tags_rejection(tags: Option<&Vec<Tag>>) -> Option<String> {
    tags?
        .iter()
        .find_map(|tag| tag_key_rejection(tag.key.as_deref().unwrap_or_default()))
}

/// The tags of a request's `Tags` member, for the by-ARN tag store.
///
/// Keyed by tag key, so a request repeating a key leaves ONE tag on the resource (the
/// last value it gave) instead of two a later read would have to choose between. "A tag
/// with a null Value is permitted" (`Tag`) and carries the EMPTY value once stored, so
/// every tag this service reports has a `Value`. A tag whose `Key` the request omitted
/// lands under the empty key, which the tag mutations refuse up front (`Tag$Key` "may
/// not be null").
pub fn tag_map(tags: Option<&Vec<Tag>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The kind of resource a MemoryDB ARN names, which is what decides WHICH not-found
/// fault a tagging call answers with for an ARN this store does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Cluster,
    Acl,
    User,
    ParameterGroup,
    Snapshot,
    SubnetGroup,
    MultiRegionCluster,
    MultiRegionParameterGroup,
}

impl ResourceKind {
    /// How a message names this kind of resource.
    pub fn label(self) -> &'static str {
        match self {
            Self::Cluster => "Cluster",
            Self::Acl => "ACL",
            Self::User => "User",
            Self::ParameterGroup => "Parameter group",
            Self::Snapshot => "Snapshot",
            Self::SubnetGroup => "Subnet group",
            Self::MultiRegionCluster => "Multi-Region cluster",
            Self::MultiRegionParameterGroup => "Multi-Region parameter group",
        }
    }

    /// How a message names this kind of resource mid-sentence, after the resource's own
    /// name (`"<name> user can't be modified."`): the same word as [`label`], lower
    /// case except where the service's word for the kind is an acronym.
    ///
    /// [`label`]: ResourceKind::label
    pub fn noun(self) -> &'static str {
        match self {
            Self::Acl => "ACL",
            Self::Cluster => "cluster",
            Self::User => "user",
            Self::ParameterGroup => "parameter group",
            Self::Snapshot => "snapshot",
            Self::SubnetGroup => "subnet group",
            Self::MultiRegionCluster => "Multi-Region cluster",
            Self::MultiRegionParameterGroup => "Multi-Region parameter group",
        }
    }

    /// How the service names this kind of resource's IDENTIFIER when it refuses one:
    /// a user is identified by its "User id", every other kind by its name.
    ///
    /// This is the service's own vocabulary rather than the wire member a request
    /// carried the name in (`UserName`, `ACLName`, …), so a refusal built from it says
    /// which rule of the service the name broke — see [`resource_name_rejection`].
    pub fn name_label(self) -> &'static str {
        match self {
            Self::User => "User id",
            Self::Acl => "ACL name",
            Self::Cluster => "Cluster name",
            Self::ParameterGroup => "Parameter group name",
            Self::Snapshot => "Snapshot name",
            Self::SubnetGroup => "Subnet group name",
            Self::MultiRegionCluster => "Multi-Region cluster name",
            Self::MultiRegionParameterGroup => "Multi-Region parameter group name",
        }
    }
}

/// The resource-type segment of each kind of MemoryDB ARN.
///
/// The first four are the segments this service's own ARN builders mint
/// ([`cluster_arn`], [`acl_arn`], [`user_arn`], [`parameter_group_arn`]); the rest are
/// the remaining resource types whose absence `ListTags`/`TagResource`/`UntagResource`
/// each model a not-found fault for. A segment that is in neither list names no
/// taggable MemoryDB resource, so an ARN carrying it is refused as invalid.
const RESOURCE_TYPE_SEGMENTS: &[(&str, ResourceKind)] = &[
    ("cluster", ResourceKind::Cluster),
    ("acl", ResourceKind::Acl),
    ("user", ResourceKind::User),
    ("parametergroup", ResourceKind::ParameterGroup),
    ("snapshot", ResourceKind::Snapshot),
    ("subnetgroup", ResourceKind::SubnetGroup),
    ("multiregioncluster", ResourceKind::MultiRegionCluster),
    (
        "multiregionparametergroup",
        ResourceKind::MultiRegionParameterGroup,
    ),
];

/// The kind and name a MemoryDB resource ARN carries, or `None` when it is not one.
///
/// The shape is the one every ARN this service mints has (see [`cluster_arn`]):
/// `arn:<partition>:memorydb:<region>:<account>:<type>/<name>`, with a non-empty
/// partition, region and account, a resource type from [`RESOURCE_TYPE_SEGMENTS`] and a
/// non-empty resource name. The service segment must be this service's
/// ([`ARN_NAMESPACE`]): another service's ARN names no MemoryDB resource.
pub fn parse_resource_arn(resource_arn: &str) -> Option<(ResourceKind, &str)> {
    let mut fields = resource_arn.splitn(6, ':');
    let prefix = fields.next()?;
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if prefix != "arn"
        || partition.is_empty()
        || service != ARN_NAMESPACE
        || region.is_empty()
        || account.is_empty()
    {
        return None;
    }
    let (resource_type, name) = resource.split_once('/')?;
    if name.is_empty() {
        return None;
    }
    let (_, kind) = RESOURCE_TYPE_SEGMENTS
        .iter()
        .find(|(segment, _)| *segment == resource_type)?;
    Some((*kind, name))
}

/// Why a tagging call's `ResourceArn` names no resource of this service
/// (see [`ComAmazonawsMemorydbAmazonmemorydbState::tag_target`]).
///
/// Carries the MESSAGE and, for a resolved kind, the kind — not a modelled error: each
/// of the three tagging operations declares its own error enum, so each lifts this into
/// its own `InvalidARNFault` / per-kind not-found variant.
#[derive(Debug, Clone)]
pub enum TagTargetRejection {
    /// The ARN is not a MemoryDB resource ARN at all: the 400 `InvalidARNFault`.
    InvalidArn(String),
    /// The ARN is well formed but names no resource in this store: that kind's 404.
    NotFound(ResourceKind, String),
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMemorydbAmazonmemorydbState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMemorydbAmazonmemorydbState>)
}
