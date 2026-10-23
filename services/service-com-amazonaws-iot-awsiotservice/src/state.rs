// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iot-awsiotservice service.
//!
//! AWS IoT's primary store is the THING REGISTRY: one record per thing, keyed by
//! `thingName`. The name is the thing's identity — the model binds it as the
//! `@httpLabel` of `/things/{thingName}` on every single-thing operation, and
//! `CreateThing$thingName` documents "You can't change a thing's name after you
//! create it" — so a `BTreeMap` keyed by name is the registry itself, not an index
//! into it. `BTreeMap` (not `HashMap`) because `ListThings` is `@paginated` and a
//! page boundary is only meaningful over a stable order.
//!
//! One record carries everything a reader hands back, so `DescribeThing` after
//! `CreateThing` returns the created thing member-for-member: a reader clones what
//! the writer stored and the two cannot drift.
//!
//! Beside the registry it holds the THING GROUP registry, keyed by `thingGroupName`
//! for the same reason — the name is the group's identity and the `@httpLabel` of
//! `/thing-groups/{thingGroupName}` — the account's JOBS (`CreateJob` keys them by
//! the `jobId` it documents as unique for the account), whose per-thing EXECUTIONS
//! live in the shared store the IoT Jobs data plane serves (see
//! [`job_execution_store`]), and the account's IoT ENDPOINT IDENTIFIERS — the
//! `<identifier>` of `<identifier>.iot.<region>.amazonaws.com`, the account-specific
//! prefix every IoT endpoint address is built from. `DescribeEndpoint` is documented
//! as "returns or creates a unique endpoint specific to the Amazon Web Services
//! account making the call. The first time DescribeEndpoint is called, an endpoint is
//! created. All subsequent calls to DescribeEndpoint return the same endpoint", so the
//! identifier is recorded here on the call that creates it and read back by every
//! later call.
//!
//! It also holds the account's TOPIC RULES, keyed by `ruleName` for the same reason
//! the thing registry is keyed by `thingName`: the name is the rule's identity and
//! the `@httpLabel` of `/rules/{ruleName}`. One record carries everything a reader
//! hands back, so `GetTopicRule` after `CreateTopicRule` returns the created rule
//! member-for-member.
//!
//! Finally it holds the account's COMMANDS, keyed by `commandId` ("A unique
//! identifier for the command", and the `@httpLabel` of `/commands/{commandId}`),
//! whose EXISTENCE the IoT Jobs data plane also needs — `StartCommandExecution`
//! refuses an execution of a command nobody created — so every recorded command's
//! ARN is registered in the shared index as well (see [`command_registry`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use service_common::iot_job_executions::{
    is_modeled_thing_name, JobExecutionRecord, JobExecutionStore,
};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    Action, CommandParameter, CommandPayload, CommandPreprocessor, GetCommandOutput, JobExecution,
    JobExecutionStatusDetails, TopicRule, TopicRulePayload,
};

/// The shared index of which IoT COMMANDS exist, which the IoT Jobs data plane's
/// `StartCommandExecution` validates its `commandArn` against.
///
/// Re-exported here for the same reason the job-execution store is: a command is
/// created by THIS control plane (`CreateCommand`) and read by the data plane, the
/// engine keys state by TYPE, and a store two service crates share has to be declared
/// in a crate they both depend on. See `service_common::iot_commands`.
pub use ::service_common::iot_commands;

/// The configuration of a thing: the members of a `CreateThing` request that
/// describe the thing rather than name it.
///
/// It is a type of its own because `CreateThing` is documented idempotent ON IT:
/// "If this call is made multiple times using the same thing name and
/// configuration, the call will succeed. If this call is made with the same thing
/// name but different configuration a `ResourceAlreadyExistsException` is thrown."
/// Comparing two configurations is therefore a single `==`.
///
/// `AttributePayload$merge` is NOT part of it: the member documents "The `merge`
/// attribute is only valid when calling `UpdateThing` or `UpdateThingGroup`", and a
/// create starts from no attributes at all, so merging into the empty map and
/// replacing it are the same thing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThingConfiguration {
    /// The thing type this thing is associated with, when it has one.
    pub thing_type_name: Option<String>,
    /// The thing's attributes, exactly as the last writer persisted them.
    pub attributes: BTreeMap<String, String>,
    /// The billing group this thing belongs to, when it has one.
    pub billing_group_name: Option<String>,
}

/// One thing in the registry, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredThing {
    /// The name the thing was created under; also its key in the registry.
    pub thing_name: String,
    /// The id minted when the thing was created — stable for the thing's lifetime.
    pub thing_id: String,
    /// `arn:aws:iot:<region>:<account>:thing/<thingName>`, minted on create.
    pub thing_arn: String,
    /// What the thing is configured with; see [`ThingConfiguration`].
    pub configuration: ThingConfiguration,
    /// The registry version of this record.
    pub version: i64,
}

/// The `version` a freshly registered thing carries.
///
/// `DescribeThing$version` is "The current version of the thing record in the
/// registry", `@required`, and is what a caller passes as `expectedVersion` to
/// `UpdateThing`/`DeleteThing`. A record that exists has had one write (its
/// create), so its version is 1; `UpdateThing` (its own issue) bumps it from here.
pub const FIRST_THING_VERSION: i64 = 1;

impl RegisteredThing {
    /// A newly created thing: the minted identity plus the requested configuration,
    /// at [`FIRST_THING_VERSION`].
    pub fn new(
        thing_name: String,
        thing_id: String,
        thing_arn: String,
        configuration: ThingConfiguration,
    ) -> Self {
        Self {
            thing_name,
            thing_id,
            thing_arn,
            configuration,
            version: FIRST_THING_VERSION,
        }
    }

    /// The attributes as the modelled `Attributes` map a response carries.
    ///
    /// They are persisted ordered (so a paginated reader is stable) and handed back
    /// in the generated `HashMap` the wire shape uses.
    pub fn attributes(&self) -> HashMap<String, String> {
        self.configuration
            .attributes
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    /// The default MQTT client id for this thing.
    ///
    /// `DescribeThing$defaultClientId`: "For a typical device, the thing name is
    /// also used as the default MQTT client ID."
    pub fn default_client_id(&self) -> String {
        self.thing_name.clone()
    }
}

/// The configuration of a thing group: the members of a `CreateThingGroup` request
/// that describe the group rather than name it.
///
/// It is a type of its own for the same reason [`ThingConfiguration`] is:
/// `CreateThingGroup` is documented idempotent ON it — "If the ThingGroup that you
/// create has the exact same attributes as an existing ThingGroup, you will get a
/// 200 success response" — so deciding a repeat create is a single `==`.
///
/// `AttributePayload$merge` is NOT part of it: the member documents "The `merge`
/// attribute is only valid when calling `UpdateThing` or `UpdateThingGroup`", and a
/// create starts from no attributes, so merging into the empty map and replacing it
/// are the same thing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThingGroupConfiguration {
    /// The group this group hangs under in the hierarchy, when it has one. It is
    /// part of the configuration because a group's place in the hierarchy is fixed
    /// at create time: `ThingGroupMetadata$parentGroupName` is metadata of the
    /// record, not something a later call to `CreateThingGroup` may re-point.
    pub parent_group_name: Option<String>,
    /// `ThingGroupProperties$thingGroupDescription`, when the request carried one.
    pub description: Option<String>,
    /// `ThingGroupProperties$attributePayload$attributes`, as the last writer
    /// persisted them. Ordered so a paginated reader over them is stable.
    pub attributes: BTreeMap<String, String>,
}

/// One thing group in the registry, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredThingGroup {
    /// The name the group was created under; also its key in the registry.
    pub thing_group_name: String,
    /// The id minted when the group was created — stable for its lifetime.
    pub thing_group_id: String,
    /// `arn:aws:iot:<region>:<account>:thinggroup/<thingGroupName>`, minted on create.
    pub thing_group_arn: String,
    /// What the group is configured with; see [`ThingGroupConfiguration`].
    pub configuration: ThingGroupConfiguration,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags are
    /// NOT part of [`ThingGroupConfiguration`]: they "are used to manage the
    /// resource" rather than describe the group, so they do not decide whether a
    /// repeat create is the same group.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the group was created — `ThingGroupMetadata$creationDate`, "The UNIX
    /// timestamp of when the thing group was created".
    pub creation_date: SystemTime,
    /// The registry version of this record.
    pub version: i64,
}

/// The `version` a freshly created thing group carries.
///
/// `DescribeThingGroup$version` is `@required` ("The version of the thing group")
/// and is what a caller passes as `expectedVersion` to `UpdateThingGroup`. A record
/// that exists has had one write (its create), so its version is 1 — the same rule
/// [`FIRST_THING_VERSION`] states for a thing.
pub const FIRST_THING_GROUP_VERSION: i64 = 1;

impl RegisteredThingGroup {
    /// A newly created thing group: the minted identity plus the requested
    /// configuration and tags, at [`FIRST_THING_GROUP_VERSION`].
    pub fn new(
        thing_group_name: String,
        thing_group_id: String,
        thing_group_arn: String,
        configuration: ThingGroupConfiguration,
        tags: BTreeMap<String, Option<String>>,
        creation_date: SystemTime,
    ) -> Self {
        Self {
            thing_group_name,
            thing_group_id,
            thing_group_arn,
            configuration,
            tags,
            creation_date,
            version: FIRST_THING_GROUP_VERSION,
        }
    }

    /// The group's attributes as the modelled `Attributes` map a response carries.
    pub fn attributes(&self) -> HashMap<String, String> {
        self.configuration
            .attributes
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}

/// Length of an account's IoT endpoint identifier.
///
/// The identifier is one DNS label of lowercase alphanumerics (the AWS IoT
/// Developer Guide's "account-specific prefix", e.g. `a3qexamplesffut`), so 14
/// characters is the length this emulator mints. The exact length is this
/// emulator's choice; what callers depend on is that it is a single, stable,
/// lowercase-alphanumeric label.
pub const ENDPOINT_IDENTIFIER_LENGTH: usize = 14;

/// Upper bound on the number of regions whose endpoint identifier is recorded.
///
/// The region comes from the caller's SigV4 credential scope, so the set of keys is
/// client-influenced and has to be bounded. Exceeding the bound costs nothing
/// observable: an identifier is a pure function of the account and the region (see
/// [`derive_endpoint_identifier`]), so a region whose record did not fit is handed
/// the same identifier every later call derives for it.
pub const MAX_ENDPOINT_REGIONS: usize = 64;

/// The alphabet an endpoint identifier's first character is drawn from.
///
/// A hostname label is read case-insensitively and AWS writes these prefixes in
/// lowercase; starting with a letter keeps the label in the preferred form of
/// RFC 1035 §2.3.1.
const IDENTIFIER_LEADING_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// The alphabet the rest of an endpoint identifier is drawn from.
const IDENTIFIER_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The endpoint identifier for `account_id` in `region`.
///
/// An AWS account's IoT endpoint prefix is permanent and differs per region, so it is
/// derived rather than drawn at random: the SHA-256 of the account id and the region,
/// mapped onto the lowercase-alphanumeric label alphabet. It carries nothing from the
/// request but the region the request was signed for, and — the digest being
/// domain-separated and truncated to one label — two regions (or two accounts) are
/// overwhelmingly unlikely to derive the same label, though the truncation means that
/// is a probability and not a guarantee: nothing downstream treats the label as a
/// unique key.
pub fn derive_endpoint_identifier(account_id: &str, region: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(account_id.as_bytes());
    // A separator, so ("00", "0-x") and ("000", "-x") cannot hash alike.
    hasher.update(b"/iot-endpoint/");
    hasher.update(region.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(ENDPOINT_IDENTIFIER_LENGTH)
        .enumerate()
        .map(|(position, byte)| {
            let alphabet = if position == 0 {
                IDENTIFIER_LEADING_ALPHABET
            } else {
                IDENTIFIER_ALPHABET
            };
            char::from(alphabet[usize::from(*byte) % alphabet.len()])
        })
        .collect()
}

/// Per-service state for com-amazonaws-iot-awsiotservice. A handler reaches it through [`service_state`].
///
/// The registry is NOT partitioned by region: a thing's ARN records the region of
/// the request that created it, but any later call reaches every thing in the store
/// whatever region it is signed for. Partitioning the control plane by region is a
/// separate change; nothing here claims it.
///
/// The JOB EXECUTIONS a job creates do not live here: they are the device-facing
/// records the IoT Jobs data plane serves, so both services reach the one shared
/// store ([`job_execution_store`]) instead of each keeping a copy.
#[derive(Debug, Default)]
pub struct ComAmazonawsIotAwsiotserviceState {
    /// Every thing that exists, keyed by `thingName`.
    pub things: BTreeMap<String, RegisteredThing>,
    /// Every job that exists, keyed by `jobId` — "A job identifier which must be
    /// unique for your account" (`CreateJob$jobId`), which is what makes the id the
    /// key and a repeat create a conflict.
    pub jobs: BTreeMap<String, RegisteredJob>,
    /// Every thing group that exists, keyed by `thingGroupName`.
    ///
    /// The name is the group's identity — the model binds it as the `@httpLabel` of
    /// `/thing-groups/{thingGroupName}` on every single-group operation — so a
    /// `BTreeMap` keyed by name is the registry itself. `BTreeMap` (not `HashMap`)
    /// because `ListThingGroups` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    pub thing_groups: BTreeMap<String, RegisteredThingGroup>,
    /// The names of the thing types that exist.
    ///
    /// `CreateThing` answers `ResourceNotFoundException` for a `thingTypeName` that
    /// is not in here, which is the only reason this set exists yet: `CreateThingType`
    /// / `DeleteThingType` are still scaffolds (their own issues), so the set is empty
    /// until one of them is implemented and populates it.
    pub thing_types: BTreeSet<String>,
    /// The names of the billing groups that exist, read by `CreateThing` for the
    /// same reason as [`ComAmazonawsIotAwsiotserviceState::thing_types`];
    /// `CreateBillingGroup` is still a scaffold and will populate it.
    pub billing_groups: BTreeSet<String>,
    /// The endpoint identifier created for each region this account has called
    /// `DescribeEndpoint` in. The call that finds no record creates one; every later
    /// call in that region answers from the record, which is what makes all of an
    /// account's endpoint addresses share one prefix.
    pub endpoint_identifiers: HashMap<String, String>,
    /// Every topic rule that exists, keyed by `ruleName`.
    ///
    /// The name is the rule's identity — the model binds it as the `@httpLabel` of
    /// `/rules/{ruleName}` on every single-rule operation — so a `BTreeMap` keyed by
    /// name is the rules store itself. `BTreeMap` (not `HashMap`) because
    /// `ListTopicRules` pages over it and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_TOPIC_RULES`] is the
    /// ceiling `CreateTopicRule` refuses past.
    pub topic_rules: BTreeMap<String, RegisteredTopicRule>,
    /// Every command that exists, keyed by `commandId`.
    ///
    /// The id is the command's identity — `CreateCommand$commandId` is "A unique
    /// identifier for the command" and the model binds it as the `@httpLabel` of
    /// `/commands/{commandId}` on every single-command operation — so a `BTreeMap`
    /// keyed by id is the command store itself. `BTreeMap` (not `HashMap`) because
    /// `ListCommands` is `@paginated` and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// The command's EXISTENCE is also needed outside this crate, by the IoT Jobs
    /// data plane's `StartCommandExecution`, so the ARN of every command recorded
    /// here is registered in the shared index as well ([`command_registry`]); this
    /// map is the command's record, that index is the cross-service answer to "does
    /// this command exist?". Both are bounded by
    /// [`iot_commands::MAX_REGISTERED_COMMANDS`].
    pub commands: BTreeMap<String, RegisteredCommand>,
}

impl ComAmazonawsIotAwsiotserviceState {
    /// Mint a `thingId` no thing in the registry is using.
    ///
    /// AWS IoT's `thingId` is a version-4 UUID — the id in every `thing/<name>`
    /// record and in the `iot:ThingId` condition key — so that is what this mints;
    /// the loop re-mints on the (astronomically unlikely) collision, so two
    /// `CreateThing` calls can never be handed the same id.
    pub fn mint_thing_id(&self) -> String {
        loop {
            let candidate = fresh_uuid();
            if !self
                .things
                .values()
                .any(|thing| thing.thing_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// Mint a `thingGroupId` no thing group in the registry is using.
    ///
    /// `ThingGroupId` is modelled `length 1..128, pattern ^[a-zA-Z0-9\-]+$`
    /// (`smithy_query(query="operation",operation="CreateThingGroup")`), which the
    /// canonical hyphenated form of a version-4 UUID satisfies; the loop re-mints on
    /// the (astronomically unlikely) collision, so two `CreateThingGroup` calls can
    /// never be handed the same id.
    pub fn mint_thing_group_id(&self) -> String {
        loop {
            let candidate = fresh_uuid();
            if !self
                .thing_groups
                .values()
                .any(|group| group.thing_group_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// The chain of groups from the hierarchy ROOT down to `parent_group_name`, as
    /// `(name, arn)` pairs — `ThingGroupMetadata$rootToParentThingGroups`.
    ///
    /// Walked from the parent upwards through each record's own
    /// `parent_group_name` and then reversed, so the first entry is the root and the
    /// last is the parent itself. The walk is bounded by the number of groups that
    /// exist, so a record whose parent chain is somehow cyclic terminates instead of
    /// spinning.
    pub fn root_to_parent_thing_groups(&self, parent_group_name: &str) -> Vec<(String, String)> {
        let mut chain = Vec::new();
        let mut cursor = Some(parent_group_name.to_string());
        while let Some(name) = cursor {
            let Some(group) = self.thing_groups.get(&name) else {
                break;
            };
            chain.push((group.thing_group_name.clone(), group.thing_group_arn.clone()));
            if chain.len() >= self.thing_groups.len() {
                break;
            }
            cursor = group.configuration.parent_group_name.clone();
        }
        chain.reverse();
        chain
    }

    /// The endpoint identifier recorded for `region`, if an endpoint has been created
    /// there.
    pub fn endpoint_identifier(&self, region: &str) -> Option<&str> {
        self.endpoint_identifiers.get(region).map(String::as_str)
    }

    /// The endpoint identifier for `region`, creating and recording it on the first
    /// call — the "returns or creates" half of `DescribeEndpoint`.
    pub fn endpoint_identifier_or_create(&mut self, account_id: &str, region: &str) -> String {
        if let Some(created) = self.endpoint_identifiers.get(region) {
            return created.clone();
        }
        let identifier = derive_endpoint_identifier(account_id, region);
        if self.endpoint_identifiers.len() < MAX_ENDPOINT_REGIONS {
            self.endpoint_identifiers
                .insert(region.to_string(), identifier.clone());
        }
        identifier
    }
}

/// A fresh version-4 UUID in the canonical lowercase hyphenated form.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()` rendered
/// canonically) — a crate this service ALREADY depends on. Nothing here is a
/// hand-rolled generator, and the service adds no dependency.
fn fresh_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own
/// shared AWS config names, so an unsigned call and a default-configured client
/// agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the
/// one in its SigV4 credential scope, so that is what an ARN minted while serving
/// it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The ARN of a thing: `arn:aws:iot:<region>:<account>:thing/<thingName>`.
///
/// `iot` is this service's ARN namespace (`smithy_query(query="service_info")`:
/// `awsService.arnNamespace`), and the account segment is the emulator's canonical
/// one, which [`::emulator_server::emulator_arn`] owns.
pub fn thing_arn(region: &str, thing_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("thing/{thing_name}"))
}

/// The ARN of a job: `arn:aws:iot:<region>:<account>:job/<jobId>`, the `jobArn`
/// `CreateJob` answers with.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("job/{job_id}"))
}

/// The resource type an IoT target ARN names, and the name it gives it.
///
/// `CreateJob$targets` is "A list of things and thing groups to which the job should
/// be sent", and both are named by ARN
/// (`arn:<partition>:iot:<region>:<account>:thing/<name>` /
/// `…:thinggroup/<name>`). The ACCOUNT and REGION segments are deliberately not
/// compared: a caller may legitimately name a resource by an ARN minted elsewhere,
/// and this registry is keyed by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTarget {
    /// `thing/<name>`: one device.
    Thing(String),
    /// `thinggroup/<name>`: every device in that group.
    ThingGroup(String),
}

/// Why a `targets` entry is not a job target.
///
/// Each variant is a DIFFERENT refusal, so the caller's message can state the
/// constraint that was violated rather than one fixed ARN-format template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTargetRejection {
    /// Not an `arn:<partition>:iot:<region>:<account>:<type>/<name>` at all.
    NotAnIotResourceArn,
    /// A well-formed IoT ARN naming a resource type a job cannot be sent to; it
    /// carries that type, which the request supplied.
    UnsupportedResourceType(String),
    /// A `thing/` or `thinggroup/` ARN whose NAME violates the model's `ThingName`
    /// (len 1..128, `^[a-zA-Z0-9:_-]+$`); it carries the name, so the refusal can
    /// say which target could never be addressed.
    UnaddressableName(String),
}

/// Parse a `targets` entry, naming the reason it is not a job target.
///
/// The NAME is checked against the model's `ThingName`
/// ([`service_common::iot_job_executions::is_modeled_thing_name`], the same rule the
/// Jobs data plane parses `StartCommandExecution$targetArn` with): a name outside it
/// can never be addressed by a `/things/{thingName}/jobs` request, so an execution
/// stored under it would exist and be unreachable. A thing GROUP's name is checked
/// the same way — `ThingGroupName` carries the same length and character class.
pub fn parse_job_target(target_arn: &str) -> Result<JobTarget, JobTargetRejection> {
    // 6 fields: arn:<partition>:<service>:<region>:<account-id>:<resource>. The
    // resource keeps any colon of its own, so the split is bounded at 6.
    let fields: Vec<&str> = target_arn.splitn(6, ':').collect();
    let ["arn", _partition, "iot", _region, _account, resource] = fields.as_slice() else {
        return Err(JobTargetRejection::NotAnIotResourceArn);
    };
    let Some((resource_type, name)) = resource.split_once('/') else {
        return Err(JobTargetRejection::NotAnIotResourceArn);
    };
    match resource_type {
        "thing" | "thinggroup" if !is_modeled_thing_name(name) => {
            Err(JobTargetRejection::UnaddressableName(name.to_string()))
        }
        "thing" => Ok(JobTarget::Thing(name.to_string())),
        "thinggroup" => Ok(JobTarget::ThingGroup(name.to_string())),
        other => Err(JobTargetRejection::UnsupportedResourceType(
            other.to_string(),
        )),
    }
}

/// How many jobs this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's jobs; the exact figure is not in
/// the model, so this is the emulator's own ceiling on a store a caller can grow,
/// and `CreateJob` past it answers the modelled `LimitExceededException` rather than
/// growing without bound.
pub const MAX_JOBS: usize = 10_000;

/// How many `targets` ONE `CreateJob` may name.
///
/// `JobTargets` is modelled `len 1..` — a minimum and NO maximum — so without a
/// ceiling the request chooses how much work the handler does and how many execution
/// records (each carrying a copy of the job document, up to 32 KiB) it creates. The
/// figure is the emulator's own; what matters is that the fan-out of one request is
/// bounded BEFORE any work proportional to it, and `CreateJob` past it answers the
/// modelled `LimitExceededException`.
pub const MAX_JOB_TARGETS: usize = 100;

/// One job in the control plane, as this service persists it.
///
/// It holds what a job NEEDS to be reconstructed by a reader and to create its
/// executions. The members that shape a ROLLOUT rather than the executions
/// themselves (`jobExecutionsRolloutConfig`, `abortConfig`,
/// `jobExecutionsRetryConfig`, `presignedUrlConfig`, `tags`, …) are NOT kept: nothing
/// stages a rollout here, so storing them would promise a later reader data this
/// record does not carry. `targets` IS kept verbatim, so a reader still reports what
/// the job was pointed at.
#[derive(Debug, Clone)]
pub struct RegisteredJob {
    /// The id the job was created under; also its key in the store.
    pub job_id: String,
    /// `arn:aws:iot:<region>:<account>:job/<jobId>`, minted on create.
    pub job_arn: String,
    /// The description the job was created with, when it has one.
    pub description: Option<String>,
    /// The job document, when the request carried one. `None` for a job created
    /// from `documentSource`: that names an S3 object this emulator does not fetch,
    /// so it reports no document rather than inventing one.
    pub document: Option<String>,
    /// The `documentSource` the job was created with, when it has one.
    pub document_source: Option<String>,
    /// The targets the job was created with, verbatim.
    pub targets: Vec<String>,
    /// `targetSelection` as supplied (`CONTINUOUS` / `SNAPSHOT`), or `None` when the
    /// request left it out.
    pub target_selection: Option<String>,
    /// `timeoutConfig.inProgressTimeoutInMinutes`, carried onto every execution the
    /// job creates.
    pub in_progress_timeout_minutes: Option<i64>,
    /// When the job was created, in epoch seconds.
    pub created_at: i64,
}

/// The ARN of a thing group: `arn:aws:iot:<region>:<account>:thinggroup/<thingGroupName>`.
///
/// `thinggroup` (one word, lowercase) is the resource type AWS IoT's thing-group
/// ARNs use, beside `thing/` for a thing; the namespace and account segments are the
/// same ones [`thing_arn`] documents.
pub fn thing_group_arn(region: &str, thing_group_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("thinggroup/{thing_group_name}"))
}

/// The ARN of a topic rule: `arn:aws:iot:<region>:<account>:rule/<ruleName>`.
///
/// `rule/` is the resource type AWS IoT's rule ARNs use — it is what
/// `GetTopicRule$ruleArn` reports and what an `iot:GetTopicRule` policy statement
/// names; the namespace and account segments are the ones [`thing_arn`] documents.
pub fn topic_rule_arn(region: &str, rule_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("rule/{rule_name}"))
}

/// How many topic rules this emulator keeps for the account.
///
/// AWS publishes a rules-engine service quota on an account's rules; the exact figure
/// is not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`MAX_JOBS`] exists — and `CreateTopicRule` past it refuses
/// with a modelled error rather than growing without bound.
pub const MAX_TOPIC_RULES: usize = 1_000;

/// The `awsIotSqlVersion` a rule created without one carries.
///
/// The AWS IoT SQL reference's "SQL versions" section lists `2015-10-08`,
/// `2016-03-23` and `beta`, and states that `2015-10-08` is the original version and
/// the one used when a rule does not name a version — so a rule whose payload omits
/// the member is recorded at this version rather than at none, and every later read
/// of that rule reports it.
pub const DEFAULT_AWS_IOT_SQL_VERSION: &str = "2015-10-08";

/// One topic rule in the rules store, as this service persists it.
///
/// The record holds the rule payload member-for-member rather than the
/// `TopicRulePayload` the create request carried, because the two are not the same
/// value over the rule's lifetime: `ruleDisabled` is what `DisableTopicRule` /
/// `EnableTopicRule` flip without touching anything else, and the create-time
/// defaults (`ruleDisabled` absent means an enabled rule, `awsIotSqlVersion` absent
/// means [`DEFAULT_AWS_IOT_SQL_VERSION`]) are resolved ONCE, on the write, so every
/// reader of the rule reports the same thing.
#[derive(Debug, Clone)]
pub struct RegisteredTopicRule {
    /// The name the rule was created under; also its key in the store.
    pub rule_name: String,
    /// `arn:aws:iot:<region>:<account>:rule/<ruleName>`, minted on create.
    pub rule_arn: String,
    /// When the rule was created — `TopicRule$createdAt`, "The date and time the
    /// rule was created".
    pub created_at: SystemTime,
    /// `TopicRulePayload$sql`, the `@required` SQL statement the rule queries with.
    pub sql: String,
    /// The description the rule was created with, when it has one.
    pub description: Option<String>,
    /// The `@required` actions the rule runs for a matching message, in the order
    /// the payload listed them.
    pub actions: Vec<Action>,
    /// Whether the rule is disabled. `TopicRulePayload$ruleDisabled` is optional, and
    /// a rule created without it is not disabled.
    pub rule_disabled: bool,
    /// The SQL rules-engine version the rule is evaluated under; see
    /// [`DEFAULT_AWS_IOT_SQL_VERSION`].
    pub aws_iot_sql_version: String,
    /// The action to run when an action fails, when the rule has one.
    pub error_action: Option<Action>,
    /// The `x-amz-tagging` query string the create request carried, verbatim.
    ///
    /// `CreateTopicRule$tags` is a raw `key1=value1&key2=value2` string rather than a
    /// tag list, and no implemented operation reads a rule's tags yet
    /// (`ListTagsForResource` is its own issue), so it is kept exactly as supplied
    /// instead of being parsed into a shape nothing consumes.
    pub tags: Option<String>,
}

impl RegisteredTopicRule {
    /// A newly created rule: the minted identity and creation instant plus the
    /// payload the request carried, with its optional members resolved to what the
    /// rule then has.
    pub fn new(
        rule_name: String,
        rule_arn: String,
        created_at: SystemTime,
        payload: TopicRulePayload,
        tags: Option<String>,
    ) -> Self {
        Self {
            rule_name,
            rule_arn,
            created_at,
            sql: payload.sql,
            description: payload.description,
            actions: payload.actions,
            rule_disabled: payload.rule_disabled.unwrap_or(false),
            aws_iot_sql_version: payload
                .aws_iot_sql_version
                .unwrap_or_else(|| DEFAULT_AWS_IOT_SQL_VERSION.to_string()),
            error_action: payload.error_action,
            tags,
        }
    }

    /// The rule as the modelled `TopicRule` a reader hands back.
    ///
    /// A reader clones what the writer stored, so `GetTopicRule` after
    /// `CreateTopicRule` reports the created rule member-for-member and the two
    /// cannot drift.
    pub fn view(&self) -> TopicRule {
        TopicRule {
            rule_name: Some(self.rule_name.clone()),
            sql: Some(self.sql.clone()),
            description: self.description.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            actions: Some(self.actions.clone()),
            rule_disabled: Some(self.rule_disabled),
            aws_iot_sql_version: Some(self.aws_iot_sql_version.clone()),
            error_action: self.error_action.clone(),
        }
    }
}

/// The ARN of a command: `arn:aws:iot:<region>:<account>:command/<commandId>`.
///
/// `command/` is the resource type AWS IoT's command ARNs use —
/// `CreateCommand$commandArn` documents "The Amazon Resource Number (ARN) of the
/// command. For example, arn:aws:iot:::command/" — and it is the form the IoT Jobs
/// data plane's `StartCommandExecution$commandArn` names a command by; the namespace
/// and account segments are the ones [`thing_arn`] documents.
pub fn command_arn(region: &str, command_id: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("command/{command_id}"))
}

/// One command in the control plane, as this service persists it.
///
/// It holds the create request's members as the command then HAS them, so a reader
/// (`GetCommand`) clones what the writer stored and the two cannot drift. The
/// lifecycle flags are part of the record rather than of the request: a created
/// command is neither deprecated nor being deleted, and `UpdateCommand` /
/// `DeleteCommand` (their own operations) are what move them.
#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    /// The id the command was created under; also its key in the store.
    pub command_id: String,
    /// `arn:aws:iot:<region>:<account>:command/<commandId>`, minted on create.
    pub command_arn: String,
    /// The namespace the command was created in, when the request named one. Stored
    /// as supplied: the model gives `CommandNamespace` no default, so a command
    /// created without one reports none rather than a namespace nobody asked for.
    pub namespace: Option<String>,
    /// `displayName` as supplied, when the request carried one.
    pub display_name: Option<String>,
    /// `description` as supplied, when the request carried one.
    pub description: Option<String>,
    /// The static payload the command was created with, when it has one.
    pub payload: Option<CommandPayload>,
    /// The dynamic command's payload template, when it has one.
    pub payload_template: Option<String>,
    /// How `payloadTemplate` is processed, when the request carried a preprocessor.
    pub preprocessor: Option<CommandPreprocessor>,
    /// The parameters `StartCommandExecution` must supply, in the order the request
    /// listed them. `None` — never an empty list — when the request carried none:
    /// `CommandParameterList` is modelled `len 1..`, so an empty list is not a value
    /// a response may carry.
    pub mandatory_parameters: Option<Vec<CommandParameter>>,
    /// The IAM role the command uses to build its payload, when it has one.
    pub role_arn: Option<String>,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags "are
    /// used as metadata to manage a command" rather than describe it, so they are
    /// kept beside the command instead of inside its configuration.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the command was created — `GetCommand$createdAt`, "The timestamp, when
    /// the command was created".
    pub created_at: SystemTime,
    /// When the command was last written — `GetCommand$lastUpdatedAt`. On a freshly
    /// created command this is the instant it was created: the create IS its last
    /// write.
    pub last_updated_at: SystemTime,
    /// Whether the command has been deprecated (`UpdateCommand$deprecated`). A
    /// created command is not.
    pub deprecated: bool,
    /// Whether the command is being deleted (`GetCommand$pendingDeletion`,
    /// "Indicates whether the command is being deleted"). A created command is not.
    pub pending_deletion: bool,
}

impl RegisteredCommand {
    /// A newly created command: the minted identity and creation instant plus the
    /// members the request carried, with the lifecycle flags at what a created
    /// command has.
    pub fn new(
        command_id: String,
        command_arn: String,
        created_at: SystemTime,
        input: &crate::ops::types::CreateCommandInput,
    ) -> Self {
        Self {
            command_id,
            command_arn,
            namespace: input.namespace.clone(),
            display_name: input.display_name.clone(),
            description: input.description.clone(),
            payload: input.payload.clone(),
            payload_template: input.payload_template.clone(),
            preprocessor: input.preprocessor.clone(),
            mandatory_parameters: input.mandatory_parameters.clone(),
            role_arn: input.role_arn.clone(),
            tags: input
                .tags
                .iter()
                .flatten()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect(),
            created_at,
            last_updated_at: created_at,
            deprecated: false,
            pending_deletion: false,
        }
    }

    /// The command as the modelled `GetCommandResponse` a reader hands back.
    ///
    /// A reader clones what the writer stored, so `GetCommand` after `CreateCommand`
    /// reports the created command member-for-member and the two cannot drift.
    pub fn view(&self) -> GetCommandOutput {
        GetCommandOutput {
            command_id: Some(self.command_id.clone()),
            command_arn: Some(self.command_arn.clone()),
            namespace: self.namespace.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            mandatory_parameters: self.mandatory_parameters.clone(),
            payload: self.payload.clone(),
            payload_template: self.payload_template.clone(),
            preprocessor: self.preprocessor.clone(),
            role_arn: self.role_arn.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            deprecated: Some(self.deprecated),
            pending_deletion: Some(self.pending_deletion),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotAwsiotserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotAwsiotserviceState>)
}

/// The SHARED IoT job-execution store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A job execution is created here (a job targets a thing) and served by the IoT Jobs
/// data plane (the device polls, starts and reports it), so it is one store under one
/// shared type — `service_common::iot_job_executions::JobExecutionStore` — and not a
/// second copy inside [`ComAmazonawsIotAwsiotserviceState`]: the engine keys state by
/// TYPE, so a copy of its own here would be invisible to the device.
///
/// LOCK ORDER, for an operation that writes BOTH stores: take
/// [`service_state`] first and this one second (`CreateJob` and `DeleteJob` both do).
/// The data-plane handlers take only this one, so that order has no inversion — keep
/// it if another writer of both is added.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn job_execution_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<JobExecutionStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<JobExecutionStore>)
}

/// A stored job execution as the modeled `JobExecution` this CONTROL plane reports.
///
/// The shared store holds the execution; this is the only place that projects it onto
/// the shapes THIS model declares, so every reader of a job execution here (today
/// `DescribeJobExecution`) reports one value for one record.
///
/// `thingArn` is "The ARN of the thing on which the job execution is running", built
/// from the thing NAME the execution is keyed by and the region the request is signed
/// for ([`thing_arn`]) — a job may target a thing that is not in the registry, so the
/// ARN comes from the execution and not from a registry lookup. `statusDetails` is
/// reported only while the execution carries a name/value pair, matching the data
/// plane's projection of the same record. `approximateSecondsBeforeTimedOut` is "the
/// estimated number of seconds that remain before the job execution status will be
/// changed to TIMED_OUT … This value will not be included if the job execution has
/// reached a terminal status", which is exactly what
/// [`JobExecutionRecord::approximate_seconds_before_timed_out`] answers.
///
/// `forceCanceled` is "true if the job execution was canceled with the optional force
/// parameter set to true", and nothing in this emulator force-cancels an execution yet
/// (`CancelJobExecution` is still a scaffold, its own issue), so there is no stored
/// force-cancel to report and the member is left out rather than reported `false`.
pub fn to_job_execution(record: &JobExecutionRecord, region: &str, now: i64) -> JobExecution {
    JobExecution {
        job_id: Some(record.job_id.clone()),
        status: Some(record.status.clone()),
        force_canceled: None,
        status_details: (!record.status_details.is_empty()).then(|| JobExecutionStatusDetails {
            details_map: Some(record.status_details_map()),
        }),
        thing_arn: Some(thing_arn(region, &record.thing_name)),
        queued_at: Some(::aws_smithy_types::DateTime::from_secs(record.queued_at)),
        started_at: record
            .started_at
            .map(::aws_smithy_types::DateTime::from_secs),
        last_updated_at: Some(::aws_smithy_types::DateTime::from_secs(
            record.last_updated_at,
        )),
        execution_number: Some(record.execution_number),
        version_number: record.version_number,
        approximate_seconds_before_timed_out: record.approximate_seconds_before_timed_out(now),
    }
}

/// The SHARED index of which IoT commands exist, in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// A command is created here (`CreateCommand`) and read by the IoT Jobs data plane
/// (`StartCommandExecution` refuses an execution of a command that does not exist), so
/// it is ONE index under one shared type —
/// `service_common::iot_commands::IotCommandRegistry` — and not a second copy inside
/// [`ComAmazonawsIotAwsiotserviceState`]: the engine keys state by TYPE, so a copy of
/// its own here would be invisible to the data plane.
///
/// LOCK ORDER, for an operation that writes BOTH stores: take [`service_state`] first
/// and this one second (`CreateCommand` does). The data plane takes its own store and
/// then this one, so that order has no inversion — keep it if another writer of both
/// is added.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn command_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<iot_commands::IotCommandRegistry>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<iot_commands::IotCommandRegistry>)
}
