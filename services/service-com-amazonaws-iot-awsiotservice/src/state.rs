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
//! Beside the registry it holds the POLICY registry, keyed by `policyName` — the
//! name is the policy's identity and the `@httpLabel` of `/policies/{policyName}` —
//! where each record carries its numbered document versions and which of them is the
//! default, the THING GROUP registry, keyed by `thingGroupName`
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
//! It also holds the account's CUSTOM AUTHORIZERS, keyed by `authorizerName` for the
//! same reason: the name is the authorizer's identity and the `@httpLabel` of
//! `/authorizer/{authorizerName}`. One record carries everything a reader hands back,
//! so `DescribeAuthorizer` after `CreateAuthorizer` returns the created authorizer
//! member-for-member.
//!
//! It also holds the account's FLEET INDEXING configuration — one record, not a map,
//! because `GetIndexingConfiguration` / `UpdateIndexingConfiguration` name no
//! resource: there is one indexing configuration per account. An account that has
//! never updated it still has one (fleet indexing disabled), which is why the field
//! has a default and the reader never has to create what it reports.
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
    Action, AuthorizerDescription, CommandParameter, CommandPayload, CommandPreprocessor,
    GetCommandOutput, JobExecution, JobExecutionStatusDetails, ThingGroupIndexingConfiguration,
    ThingIndexingConfiguration, TopicRule, TopicRulePayload,
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
    /// Every IoT policy that exists, keyed by `policyName`.
    ///
    /// The name is the policy's identity — the model binds it as the `@httpLabel` of
    /// `/policies/{policyName}` on every single-policy operation — so a `BTreeMap`
    /// keyed by name is the registry itself. `BTreeMap` (not `HashMap`) because
    /// `ListPolicies` is `@paginated` and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_POLICIES`] is the ceiling
    /// `CreatePolicy` refuses past, and [`MAX_POLICY_DOCUMENT_BYTES`] /
    /// [`MAX_POLICY_TAGS`] bound what ONE record may cost.
    pub policies: BTreeMap<String, RegisteredPolicy>,
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
    /// Every custom authorizer that exists, keyed by `authorizerName`.
    ///
    /// The name is the authorizer's identity — the model binds it as the
    /// `@httpLabel` of `/authorizer/{authorizerName}` on every single-authorizer
    /// operation — so a `BTreeMap` keyed by name is the authorizer store itself.
    /// `BTreeMap` (not `HashMap`) because `ListAuthorizers` is `@paginated` and a page
    /// boundary is only meaningful over a stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_AUTHORIZERS`] is the ceiling
    /// `CreateAuthorizer` refuses past.
    pub authorizers: BTreeMap<String, RegisteredAuthorizer>,
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
    /// The account's FLEET INDEXING configuration — one record, read by
    /// `GetIndexingConfiguration` and replaced by `UpdateIndexingConfiguration`.
    ///
    /// It is a plain field and not an `Option`, because an account that has never
    /// updated it still HAS a configuration: fleet indexing disabled
    /// ([`IndexingConfiguration::disabled`]). That is what makes the read a pure
    /// read — it never has to create the record it reports.
    pub indexing_configuration: IndexingConfiguration,
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

/// The ARN of an IoT policy: `arn:aws:iot:<region>:<account>:policy/<policyName>`.
///
/// `policy` is the resource type AWS IoT's policy ARNs use; the namespace and
/// account segments are the same ones [`thing_arn`] documents.
pub fn policy_arn(region: &str, policy_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("policy/{policy_name}"))
}

/// The `policyVersionId` a freshly created policy's only version carries.
///
/// `CreatePolicy` documents it: "The created policy is the default version for the
/// policy. This operation creates a policy version with a version identifier of 1
/// and sets 1 as the policy's default version."
pub const FIRST_POLICY_VERSION_ID: u32 = 1;

/// The shortest policy document AWS IoT accepts, counted EXCLUDING whitespace.
///
/// `CreatePolicy$policyDocument`: "policyDocument must have a minimum length of 1,
/// with a maximum length of 2048, excluding whitespace."
pub const MIN_POLICY_DOCUMENT_LENGTH: usize = 1;

/// The longest policy document AWS IoT accepts, counted EXCLUDING whitespace; the
/// other half of the sentence [`MIN_POLICY_DOCUMENT_LENGTH`] quotes.
///
/// It is NOT the modelled `PolicyDocument` length (0..404600): that bound is what the
/// wire shape admits, while this is the document rule the service itself enforces.
pub const MAX_POLICY_DOCUMENT_LENGTH: usize = 2048;

/// How many policies this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's IoT policies; the exact figure is
/// not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`MAX_JOBS`] and [`MAX_TOPIC_RULES`] exist — and
/// `CreatePolicy` past it refuses with a modelled error rather than growing without
/// bound.
pub const MAX_POLICIES: usize = 1_000;

/// How many BYTES of policy document one policy version may keep.
///
/// [`MAX_POLICY_DOCUMENT_LENGTH`] is the documented rule, but it counts only
/// SIGNIFICANT characters, while a record keeps the document verbatim (a reader hands
/// back what the writer sent) — so whitespace alone would decide how much memory one
/// record costs. This is the emulator's own ceiling on that cost: far above any
/// document whose significant length fits the documented 2048, and well under the
/// modelled `PolicyDocument` length (0..404600) that the wire shape admits.
pub const MAX_POLICY_DOCUMENT_BYTES: usize = 32 * 1024;

/// How many tags one policy may carry.
///
/// AWS IoT's tagging documentation publishes the limit as "You can add up to 50 tags
/// to a resource"; `TagList` carries no modelled maximum, so without it one request
/// would choose how large a single record is.
pub const MAX_POLICY_TAGS: usize = 50;

/// Why a string is not an IoT policy document.
///
/// Each variant is a DIFFERENT refusal, so the caller's message can state the rule
/// that was violated rather than one fixed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDocumentRejection {
    /// Its length excluding whitespace is outside
    /// [`MIN_POLICY_DOCUMENT_LENGTH`]..=[`MAX_POLICY_DOCUMENT_LENGTH`]; carries that
    /// length, which the request supplied.
    Length(usize),
    /// It is longer than [`MAX_POLICY_DOCUMENT_BYTES`]; carries its length in bytes,
    /// which the request supplied.
    TooManyBytes(usize),
    /// It is not JSON at all; carries the parser's own complaint.
    NotJson(String),
    /// It is JSON, but not a JSON object, so it cannot be a policy document.
    NotAnObject,
    /// It carries no `Statement` at all.
    MissingStatement,
    /// Its `Statement` list is empty, so the document grants and denies nothing.
    EmptyStatement,
    /// A `Statement` entry is not a JSON object.
    StatementNotAnObject,
    /// A statement carries no `Effect`.
    MissingEffect,
    /// A statement's `Effect` is neither `Allow` nor `Deny`; carries what it was.
    InvalidEffect(String),
    /// A statement names neither `Action` nor `NotAction`.
    MissingAction,
}

impl PolicyDocumentRejection {
    /// What to tell the caller: the rule that was violated, built from the document
    /// that violated it.
    pub fn message(&self) -> String {
        match self {
            Self::Length(length) => format!(
                "policyDocument must have a minimum length of {MIN_POLICY_DOCUMENT_LENGTH}, \
                 with a maximum length of {MAX_POLICY_DOCUMENT_LENGTH}, excluding whitespace; \
                 this one is {length}"
            ),
            Self::TooManyBytes(bytes) => format!(
                "policyDocument must be at most {MAX_POLICY_DOCUMENT_BYTES} bytes long; \
                 this one is {bytes}"
            ),
            Self::NotJson(complaint) => {
                format!("policyDocument is not a valid JSON document: {complaint}")
            }
            Self::NotAnObject => {
                "policyDocument must be a JSON object carrying a Statement".to_string()
            }
            Self::MissingStatement => "policyDocument must carry a Statement".to_string(),
            Self::EmptyStatement => {
                "policyDocument must carry at least one statement in Statement".to_string()
            }
            Self::StatementNotAnObject => {
                "every entry of Statement must be a JSON object".to_string()
            }
            Self::MissingEffect => "every statement must carry an Effect".to_string(),
            Self::InvalidEffect(effect) => format!(
                "a statement's Effect must be either Allow or Deny, not {effect:?}"
            ),
            Self::MissingAction => {
                "every statement must name an Action or a NotAction".to_string()
            }
        }
    }
}

/// Check `document` against the IoT policy-document rules, naming the first rule it
/// violates.
///
/// An IoT policy document is an IAM-grammar JSON policy: a JSON object whose
/// `Statement` is one statement object or a list of them, each carrying an `Effect`
/// of exactly `Allow` or `Deny` and an `Action` (or `NotAction`). Everything else a
/// statement may carry — `Resource`, `Condition`, `Sid` — is the caller's to shape,
/// so nothing here reads or restricts it. The LENGTH rule is the one
/// `CreatePolicy$policyDocument` documents, counted excluding whitespace; beside it,
/// [`MAX_POLICY_DOCUMENT_BYTES`] bounds what the verbatim document a record keeps may
/// cost, which the documented rule does not.
///
/// Shared by every operation that accepts a policy document, so `CreatePolicy` and
/// `CreatePolicyVersion` cannot drift apart on what they accept.
pub fn validate_policy_document(document: &str) -> Result<(), PolicyDocumentRejection> {
    if document.len() > MAX_POLICY_DOCUMENT_BYTES {
        return Err(PolicyDocumentRejection::TooManyBytes(document.len()));
    }
    let significant = document.chars().filter(|c| !c.is_whitespace()).count();
    if !(MIN_POLICY_DOCUMENT_LENGTH..=MAX_POLICY_DOCUMENT_LENGTH).contains(&significant) {
        return Err(PolicyDocumentRejection::Length(significant));
    }
    let parsed: ::serde_json::Value = ::serde_json::from_str(document)
        .map_err(|error| PolicyDocumentRejection::NotJson(error.to_string()))?;
    let members = parsed
        .as_object()
        .ok_or(PolicyDocumentRejection::NotAnObject)?;
    let statement = members
        .get("Statement")
        .ok_or(PolicyDocumentRejection::MissingStatement)?;
    // The IAM grammar admits both a single statement object and a list of them.
    let statements: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Array(entries) => entries.iter().collect(),
        single => vec![single],
    };
    if statements.is_empty() {
        return Err(PolicyDocumentRejection::EmptyStatement);
    }
    for entry in statements {
        let statement = entry
            .as_object()
            .ok_or(PolicyDocumentRejection::StatementNotAnObject)?;
        let effect = statement
            .get("Effect")
            .ok_or(PolicyDocumentRejection::MissingEffect)?;
        match effect.as_str() {
            Some("Allow" | "Deny") => {}
            _ => {
                return Err(PolicyDocumentRejection::InvalidEffect(effect.to_string()));
            }
        }
        if !statement.contains_key("Action") && !statement.contains_key("NotAction") {
            return Err(PolicyDocumentRejection::MissingAction);
        }
    }
    Ok(())
}

/// One version of a policy's document, as this service persists it.
#[derive(Debug, Clone)]
pub struct PolicyVersion {
    /// The document this version carries, verbatim as the writer supplied it: a
    /// reader hands back what was written, never a reformatted copy.
    pub document: String,
    /// When this version was created.
    pub created_at: SystemTime,
}

/// One IoT policy in the registry, as this service persists it.
///
/// A policy is a NAME plus a set of numbered document versions and a pointer to the
/// one that is in effect, which is what the model describes: `GetPolicy` reports
/// "the policy document of the default version" beside a `defaultVersionId`, and
/// `CreatePolicyVersion` / `SetDefaultPolicyVersion` add versions and re-point that
/// pointer.
#[derive(Debug, Clone)]
pub struct RegisteredPolicy {
    /// The name the policy was created under; also its key in the registry.
    pub policy_name: String,
    /// `arn:aws:iot:<region>:<account>:policy/<policyName>`, minted on create.
    pub policy_arn: String,
    /// Every version that exists, keyed by its numeric `policyVersionId`. Ordered,
    /// so `ListPolicyVersions` reads them oldest-first without sorting.
    pub versions: BTreeMap<u32, PolicyVersion>,
    /// Which version is in effect — the `defaultVersionId` a reader reports and the
    /// version whose document `GetPolicy` hands back.
    pub default_version_id: u32,
    /// When the policy was created — `GetPolicy$creationDate`.
    pub creation_date: SystemTime,
    /// When the policy last changed — `GetPolicy$lastModifiedDate`. Equal to
    /// [`RegisteredPolicy::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
    /// `GetPolicy$generationId`, "The generation ID of the policy": an opaque,
    /// server-minted token identifying the policy's current generation. It is stable
    /// across reads and re-minted by a write, so a caller can tell one generation
    /// from another.
    pub generation_id: String,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags
    /// "are used to manage the resource" rather than describe the policy, so they are
    /// not part of its document or its identity.
    pub tags: BTreeMap<String, Option<String>>,
}

impl RegisteredPolicy {
    /// A newly created policy: the minted identity, `document` as version
    /// [`FIRST_POLICY_VERSION_ID`] and that version set as the default.
    pub fn new(
        policy_name: String,
        policy_arn: String,
        document: String,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            policy_name,
            policy_arn,
            versions: BTreeMap::from([(
                FIRST_POLICY_VERSION_ID,
                PolicyVersion {
                    document,
                    created_at,
                },
            )]),
            default_version_id: FIRST_POLICY_VERSION_ID,
            creation_date: created_at,
            last_modified_date: created_at,
            generation_id: fresh_uuid(),
            tags,
        }
    }

    /// The document of the version that is in effect — what `GetPolicy` reports.
    pub fn default_document(&self) -> &str {
        self.versions
            .get(&self.default_version_id)
            .map_or("", |version| version.document.as_str())
    }

    /// The default version's id in the modelled `PolicyVersionId` form (`^[0-9]+$`).
    pub fn default_version_id(&self) -> String {
        self.default_version_id.to_string()
    }
}

/// The ARN of a custom authorizer:
/// `arn:aws:iot:<region>:<account>:authorizer/<authorizerName>`.
///
/// `authorizer/` is the resource type AWS IoT's authorizer ARNs use — it is what
/// `CreateAuthorizer$authorizerArn` / `AuthorizerDescription$authorizerArn` report
/// and what an `iot:DescribeAuthorizer` policy statement names; the namespace and
/// account segments are the ones [`thing_arn`] documents.
pub fn authorizer_arn(region: &str, authorizer_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("authorizer/{authorizer_name}"))
}

/// How many custom authorizers this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's custom authorizers; the exact figure
/// is not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`MAX_TOPIC_RULES`] exists — and `CreateAuthorizer` past it
/// refuses with the modelled `LimitExceededException` rather than growing without
/// bound.
pub const MAX_AUTHORIZERS: usize = 100;

/// How many tags one authorizer may carry: the same documented IoT tagging limit
/// [`MAX_POLICY_TAGS`] records, for the same reason — `TagList` has no modelled
/// maximum, so without a ceiling one request would choose how large a record is.
pub const MAX_AUTHORIZER_TAGS: usize = MAX_POLICY_TAGS;

/// The `status` an authorizer created without one carries.
///
/// `AuthorizerStatus` is `ACTIVE | INACTIVE`
/// (`smithy_query(query="shape",shape="AuthorizerStatus")`) and AWS IoT's custom
/// authentication documentation describes INACTIVE as the status you move an
/// authorizer to in order to stop it being used — so a create that does not ask for
/// INACTIVE yields an authorizer that is in use, which is `ACTIVE`. The record keeps
/// it, and every later read reports it.
pub const DEFAULT_AUTHORIZER_STATUS: &str = "ACTIVE";

/// One custom authorizer in the authorizer store, as this service persists it.
///
/// An authorizer is a NAME plus the Lambda function that authorizes for it and how
/// its token is validated: the name is its identity — the model binds it as the
/// `@httpLabel` of `/authorizer/{authorizerName}` on every single-authorizer
/// operation — and the rest is what `AuthorizerDescription` reports member for
/// member, so a reader clones this record and the writer and the reader cannot
/// drift.
///
/// The optional members are kept as `Option` where ABSENT is distinguishable from a
/// value (`tokenKeyName`, `tokenSigningPublicKeys`: an authorizer that validates no
/// token signature has neither) and resolved to their documented value where it is
/// not (`status`, `signingDisabled`, `enableCachingForHttp`), because a resource that
/// exists has one of each whether the create named it or not.
#[derive(Debug, Clone)]
pub struct RegisteredAuthorizer {
    /// The name the authorizer was created under; also its key in the store.
    pub authorizer_name: String,
    /// `arn:aws:iot:<region>:<account>:authorizer/<authorizerName>`, minted on
    /// create.
    pub authorizer_arn: String,
    /// "The ARN of the authorizer's Lambda function" — `@required` on the create, so
    /// every record has one.
    pub authorizer_function_arn: String,
    /// "The name of the token key used to extract the token from the HTTP headers",
    /// when the create named one.
    pub token_key_name: Option<String>,
    /// "The public keys used to verify the digital signature returned by your custom
    /// authentication service", keyed by key name, when the create carried any.
    /// `BTreeMap` so a reader reports them in one stable order.
    pub token_signing_public_keys: Option<BTreeMap<String, String>>,
    /// The authorizer's status: one of [`DEFAULT_AUTHORIZER_STATUS`]'s enum, resolved
    /// on the write.
    pub status: String,
    /// When the authorizer was created — `AuthorizerDescription$creationDate`, "The
    /// UNIX timestamp of when the authorizer was created".
    pub creation_date: SystemTime,
    /// When the authorizer last changed — `AuthorizerDescription$lastModifiedDate`.
    /// Equal to [`RegisteredAuthorizer::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
    /// "Specifies whether IoT validates the token signature in an authorization
    /// request": a create that does not disable it leaves validation on, so an absent
    /// member is recorded as `false`.
    pub signing_disabled: bool,
    /// "When true, the result from the authorizer's Lambda function is cached for
    /// clients that use persistent HTTP connections. … The default value is false"
    /// (`CreateAuthorizer$enableCachingForHttp`), so an absent member is recorded as
    /// `false`.
    pub enable_caching_for_http: bool,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags "can be
    /// used to manage the custom authorizer" rather than describe it, so they are not
    /// part of `AuthorizerDescription` and not part of its identity.
    pub tags: BTreeMap<String, Option<String>>,
}

impl RegisteredAuthorizer {
    /// A newly created authorizer: the minted identity plus the create request's
    /// members, with each optional one resolved as its field documents.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        authorizer_name: String,
        authorizer_arn: String,
        authorizer_function_arn: String,
        token_key_name: Option<String>,
        token_signing_public_keys: Option<BTreeMap<String, String>>,
        status: Option<String>,
        signing_disabled: Option<bool>,
        enable_caching_for_http: Option<bool>,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            authorizer_name,
            authorizer_arn,
            authorizer_function_arn,
            token_key_name,
            token_signing_public_keys,
            status: status.unwrap_or_else(|| DEFAULT_AUTHORIZER_STATUS.to_string()),
            creation_date: created_at,
            last_modified_date: created_at,
            signing_disabled: signing_disabled.unwrap_or(false),
            enable_caching_for_http: enable_caching_for_http.unwrap_or(false),
            tags,
        }
    }

    /// This record as the `AuthorizerDescription` every reader of one authorizer
    /// reports: one function, so two readers cannot describe the same record
    /// differently.
    pub fn description(&self) -> AuthorizerDescription {
        AuthorizerDescription {
            authorizer_name: Some(self.authorizer_name.clone()),
            authorizer_arn: Some(self.authorizer_arn.clone()),
            authorizer_function_arn: Some(self.authorizer_function_arn.clone()),
            token_key_name: self.token_key_name.clone(),
            token_signing_public_keys: self.token_signing_public_keys.as_ref().map(|keys| {
                keys.iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            }),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            signing_disabled: Some(self.signing_disabled),
            enable_caching_for_http: Some(self.enable_caching_for_http),
        }
    }
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
    /// The topic filter the rule's SQL statement selects from — what
    /// `TopicRuleListItem$topicPattern` reports — resolved ONCE, on the write, by
    /// [`topic_pattern_of_sql`], so every listing of the rule reports the same
    /// pattern and none of them re-reads the statement.
    ///
    /// `None` for a statement that names no topic filter: the pattern is then
    /// unknown, and a reader reports the member as absent rather than inventing one.
    pub topic_pattern: Option<String>,
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
        let payload_sql = payload.sql;
        Self {
            rule_name,
            rule_arn,
            created_at,
            description: payload.description,
            actions: payload.actions,
            rule_disabled: payload.rule_disabled.unwrap_or(false),
            aws_iot_sql_version: payload
                .aws_iot_sql_version
                .unwrap_or_else(|| DEFAULT_AWS_IOT_SQL_VERSION.to_string()),
            topic_pattern: topic_pattern_of_sql(&payload_sql),
            sql: payload_sql,
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

/// The most characters of a rule's SQL statement [`topic_pattern_of_sql`] reads.
///
/// `TopicRulePayload$sql` carries NO modelled length constraint
/// (`smithy_query(query="shape",shape="TopicRulePayload")`), so the statement a caller
/// stores is bounded only by the request body, while the topic filter it names is in
/// the statement's FIRST clause — reading past a generous prefix of it can never find
/// one. The value is the EMULATOR's choice: the AWS IoT SQL reference's statements are
/// a few hundred characters, so 64 Ki characters is orders of magnitude above any real
/// rule and keeps this pass's work and its single allocation finite whatever a caller
/// stored.
pub const MAX_SQL_SCAN_CHARACTERS: usize = 64 * 1024;

/// The topic filter a rule's SQL statement selects from — what
/// `TopicRuleListItem$topicPattern` ("The pattern for the topic names that apply")
/// reports for that rule.
///
/// The AWS IoT SQL reference gives a rule's statement as `SELECT <attributes> FROM
/// <topic filter> [WHERE <condition>]`, with the topic filter written as a
/// single-quoted string (`SELECT * FROM 'a/b/+'`), so the pattern is the quoted
/// literal the statement's first `FROM` names, whatever the clause's casing, spacing
/// or comments.
///
/// The statement is LEXED rather than matched out of its text: this is a single
/// left-to-right pass that knows where whitespace, a `--` line comment, a `/* */`
/// block comment, a bare word and a single-quoted literal (with `''` for a quote
/// inside one) begin and end, and it answers with the first literal the first `FROM`
/// names. It is written here, and not delegated to a SQL parser crate, because the
/// clause is AWS IoT's OWN dialect: standard SQL names a RELATION after `FROM`, never
/// a string literal, so no SQL grammar parses this clause and a parser crate could
/// only ever lend its lexer to it. The pass is iterative, reads at most
/// [`MAX_SQL_SCAN_CHARACTERS`] of the statement and allocates only the literal it
/// returns, so a caller-supplied statement can neither recurse nor grow its work
/// without bound.
///
/// `None` when the statement has no `FROM` followed by a quoted topic filter inside
/// that prefix (an unquoted relation, an unterminated literal, no `FROM` at all): the
/// pattern is then unknown, and a reader reports the member as absent rather than
/// inventing one.
pub fn topic_pattern_of_sql(sql: &str) -> Option<String> {
    let characters: Vec<char> = sql.chars().take(MAX_SQL_SCAN_CHARACTERS).collect();
    let mut index = 0;
    let mut after_from = false;
    while index < characters.len() {
        let character = characters[index];
        match character {
            // Whitespace separates the clause's parts and carries nothing.
            _ if character.is_whitespace() => index += 1,
            // `-- …` comments out the rest of the line.
            '-' if characters.get(index + 1) == Some(&'-') => {
                while index < characters.len() && characters[index] != '\n' {
                    index += 1;
                }
            }
            // `/* … */` comments out everything up to its close.
            '/' if characters.get(index + 1) == Some(&'*') => {
                index += 2;
                while index < characters.len()
                    && !(characters[index] == '*' && characters.get(index + 1) == Some(&'/'))
                {
                    index += 1;
                }
                index = index.saturating_add(2).min(characters.len());
            }
            // A single-quoted literal. After `FROM` it IS the topic filter; anywhere
            // else (a selected constant, a `WHERE` comparand) it is skipped.
            '\'' => {
                let mut literal = String::new();
                index += 1;
                loop {
                    match characters.get(index) {
                        // A doubled quote is one quote inside the literal.
                        Some('\'') if characters.get(index + 1) == Some(&'\'') => {
                            literal.push('\'');
                            index += 2;
                        }
                        Some('\'') => {
                            index += 1;
                            break;
                        }
                        Some(other) => {
                            literal.push(*other);
                            index += 1;
                        }
                        // An unterminated literal names no topic filter.
                        None => return None,
                    }
                }
                if after_from {
                    return Some(literal);
                }
            }
            // A bare word. `FROM` is the one this looks for, whatever its casing; a
            // word AFTER it is an unquoted relation, which names no topic filter.
            _ if is_word_character(character) => {
                let start = index;
                while index < characters.len() && is_word_character(characters[index]) {
                    index += 1;
                }
                if after_from {
                    return None;
                }
                after_from = characters[start..index]
                    .iter()
                    .collect::<String>()
                    .eq_ignore_ascii_case("FROM");
            }
            // Anything else — an operator, a bracket, a comma, a quoted identifier.
            // Before `FROM` it is part of the select clause; after it, it is not a
            // quoted topic filter.
            _ => {
                if after_from {
                    return None;
                }
                index += 1;
            }
        }
    }
    None
}

/// Whether `character` can appear in a bare word of an AWS IoT SQL statement: the
/// keywords, function names and attribute names a statement writes unquoted.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '.' | '$')
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

/// The wire token every fleet-indexing mode uses for "this kind of indexing is
/// disabled".
///
/// `ThingIndexingMode`, `ThingConnectivityIndexingMode`,
/// `DeviceDefenderIndexingMode`, `NamedShadowIndexingMode` and
/// `ThingGroupIndexingMode` each declare it and each document it as "OFF - ...
/// indexing is disabled"
/// (`smithy_query(query="doc",shape="ThingIndexingConfiguration",member="thingIndexingMode")`).
pub const INDEXING_MODE_OFF: &str = "OFF";

/// How many custom fields one indexing configuration may carry, per section.
///
/// `Fields` carries no modelled length and the list is one a CALLER supplies, so
/// this is the emulator's own ceiling on it — the same reason [`MAX_TOPIC_RULES`]
/// exists — and `UpdateIndexingConfiguration` past it refuses with a modelled error
/// rather than recording a configuration of unbounded size.
pub const MAX_CUSTOM_FIELDS: usize = 100;

/// The account's fleet-indexing configuration, as this service persists it.
///
/// One record for the account, holding the two configuration sections the model
/// declares — `GetIndexingConfiguration` reports both, `UpdateIndexingConfiguration`
/// writes the ones its request supplies — so the reader hands back exactly what the
/// last writer stored and the two cannot drift.
///
/// Like the rest of this store it is NOT partitioned by region (see
/// [`ComAmazonawsIotAwsiotserviceState`]): one account, one configuration.
#[derive(Debug, Clone)]
pub struct IndexingConfiguration {
    /// `ThingIndexingConfiguration` — "Thing indexing configuration".
    pub thing: ThingIndexingConfiguration,
    /// `ThingGroupIndexingConfiguration` — "Thing group indexing configuration".
    pub thing_group: ThingGroupIndexingConfiguration,
}

impl IndexingConfiguration {
    /// The configuration of an account that has never updated it: every kind of
    /// indexing at its documented [`INDEXING_MODE_OFF`] value, no custom fields and
    /// no filter.
    ///
    /// `thingIndexingMode` and `thingGroupIndexingMode` are `@required` members of
    /// their sections, so the disabled configuration states them explicitly rather
    /// than leaving either section's mode unset.
    pub fn disabled() -> Self {
        Self {
            thing: ThingIndexingConfiguration {
                thing_indexing_mode: INDEXING_MODE_OFF.to_string(),
                thing_connectivity_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                device_defender_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                named_shadow_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                managed_fields: None,
                custom_fields: None,
                filter: None,
            },
            thing_group: ThingGroupIndexingConfiguration {
                thing_group_indexing_mode: INDEXING_MODE_OFF.to_string(),
                managed_fields: None,
                custom_fields: None,
            },
        }
    }

    /// Record `supplied` as the account's THING indexing configuration.
    ///
    /// A supplied section replaces the stored one in full — the request carries the
    /// whole `ThingIndexingConfiguration`, not a patch of it — so an optional mode
    /// the request leaves out is recorded at [`INDEXING_MODE_OFF`] rather than
    /// keeping the value some earlier call set. `managedFields` is dropped: "You
    /// can't modify managed fields by updating fleet indexing configuration"
    /// (`smithy_query(query="doc",shape="ThingIndexingConfiguration",member="managedFields")`),
    /// so the caller's list is not what the service indexes.
    pub fn set_thing(&mut self, supplied: ThingIndexingConfiguration) {
        self.thing = ThingIndexingConfiguration {
            thing_indexing_mode: supplied.thing_indexing_mode,
            thing_connectivity_indexing_mode: Some(
                supplied
                    .thing_connectivity_indexing_mode
                    .unwrap_or_else(|| INDEXING_MODE_OFF.to_string()),
            ),
            device_defender_indexing_mode: Some(
                supplied
                    .device_defender_indexing_mode
                    .unwrap_or_else(|| INDEXING_MODE_OFF.to_string()),
            ),
            named_shadow_indexing_mode: Some(
                supplied
                    .named_shadow_indexing_mode
                    .unwrap_or_else(|| INDEXING_MODE_OFF.to_string()),
            ),
            managed_fields: None,
            custom_fields: supplied.custom_fields,
            filter: supplied.filter,
        };
    }

    /// Record `supplied` as the account's THING GROUP indexing configuration, under
    /// the same replacement and `managedFields` rules as
    /// [`IndexingConfiguration::set_thing`].
    pub fn set_thing_group(&mut self, supplied: ThingGroupIndexingConfiguration) {
        self.thing_group = ThingGroupIndexingConfiguration {
            thing_group_indexing_mode: supplied.thing_group_indexing_mode,
            managed_fields: None,
            custom_fields: supplied.custom_fields,
        };
    }
}

/// A store with no recorded update reports the disabled configuration, so that is
/// this record's default.
impl Default for IndexingConfiguration {
    fn default() -> Self {
        Self::disabled()
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

/// The `CommandExecutionStatus` a stored execution's `JobExecutionStatus` is reported
/// as, or `None` for a stored status no `CommandExecutionStatus` value names.
///
/// A COMMAND execution and a JOB execution are one record in the shared store (the
/// data plane's `StartCommandExecution` enqueues the command one, and from then on the
/// same device-facing operations move it), but the two models name the lifecycle
/// differently: `JobExecutionStatus` admits `QUEUED` and `CommandExecutionStatus` does
/// not, declaring `CREATED` instead
/// (`smithy_query(query="shape",shape="CommandExecutionStatus")`: `CREATED`,
/// `IN_PROGRESS`, `SUCCEEDED`, `FAILED`, `REJECTED`, `TIMED_OUT`) — an execution that
/// has been created for its device and not yet taken up. The other five values are
/// spelled the same in both models, so they are reported unchanged.
///
/// `None` is for a stored status this model cannot express (`CANCELED`, `REMOVED` —
/// job-level statuses a command execution does not reach, since those moves are made
/// by operations that resolve a `jobId` of the control plane's job registry, and a
/// command execution is keyed by a minted `executionId` that is not one). A reader
/// reports no command execution for such a record rather than inventing a status the
/// model does not declare.
pub fn command_execution_status(stored_status: &str) -> Option<&'static str> {
    match stored_status {
        ::service_common::iot_job_executions::STATUS_QUEUED => Some("CREATED"),
        ::service_common::iot_job_executions::STATUS_IN_PROGRESS => Some("IN_PROGRESS"),
        "SUCCEEDED" => Some("SUCCEEDED"),
        "FAILED" => Some("FAILED"),
        ::service_common::iot_job_executions::STATUS_REJECTED => Some("REJECTED"),
        ::service_common::iot_job_executions::STATUS_TIMED_OUT => Some("TIMED_OUT"),
        _ => None,
    }
}

/// When a command execution STARTED, in epoch seconds.
///
/// `CommandExecutionSummary$startedAt` is "The date and time at which the command
/// started executing on the target device", and the operation that brings a command
/// execution into existence is `iot-jobs-data:StartCommandExecution` — the call that
/// STARTS it on that device. So an execution has a start instant from the moment it is
/// created: the instant the DEVICE reported taking the work up when it has reported
/// one, and otherwise the instant the start call started it. The instant therefore only
/// ever moves forward, and never from a read.
///
/// `completedAt` has no such floor ([`command_execution_completed_at`]): an execution
/// that has not reached a status it never leaves has not completed, so there is no
/// instant to report for it.
pub fn command_execution_started_at(record: &JobExecutionRecord) -> i64 {
    record.started_at.unwrap_or(record.queued_at)
}

/// When a command execution COMPLETED, in epoch seconds, or `None` while it has not.
///
/// `CommandExecutionSummary$completedAt` is "The date and time at which the command
/// completed executing on the target device", so it exists exactly for an execution
/// that has reached a status it never leaves, and it is the instant of that last
/// write: the device's terminal update stamps `last_updated_at`, and an execution the
/// clock timed out is stamped at its DEADLINE
/// (`JobExecutionRecord::time_out_if_due`), which is when it completed rather than
/// when it was next read.
pub fn command_execution_completed_at(record: &JobExecutionRecord) -> Option<i64> {
    record.is_terminal().then_some(record.last_updated_at)
}

/// A stored command execution as the modelled `CommandExecutionSummary` this service
/// reports, or `None` when the record is not one a reader may report.
///
/// `None` for a record that is not a command execution at all (it carries no
/// `commandArn`, so it was created by `CreateJob` and belongs to the job-execution
/// readers) and for one whose stored status this model cannot name
/// ([`command_execution_status`]).
///
/// Every member is read off the stored record: `executionId` is the id the data plane
/// minted and keyed the record by, `targetArn` is "The Amazon Resource Name (ARN) of
/// the target device for which the command is being executed" built from the thing
/// NAME the execution is keyed by and the region the request is signed for
/// ([`thing_arn`]), `createdAt` is when the execution was created for the device,
/// `startedAt` is when it started on that device ([`command_execution_started_at`])
/// and `completedAt` only once it finished ([`command_execution_completed_at`]).
pub fn to_command_execution_summary(
    record: &JobExecutionRecord,
    region: &str,
) -> Option<crate::ops::types::CommandExecutionSummary> {
    let command_arn = record.command_arn.clone()?;
    let status = command_execution_status(&record.status)?;
    Some(crate::ops::types::CommandExecutionSummary {
        command_arn: Some(command_arn),
        execution_id: Some(record.job_id.clone()),
        target_arn: Some(thing_arn(region, &record.thing_name)),
        status: Some(status.to_string()),
        created_at: Some(::aws_smithy_types::DateTime::from_secs(record.queued_at)),
        started_at: Some(::aws_smithy_types::DateTime::from_secs(
            command_execution_started_at(record),
        )),
        completed_at: command_execution_completed_at(record)
            .map(::aws_smithy_types::DateTime::from_secs),
    })
}

/// The instant, in epoch seconds, that a `StringDateTime` names, or `None` for text
/// that names none.
///
/// The members that carry one here are `TimeFilter$after` / `TimeFilter$before`, whose
/// documentation states the form as "the format yyyy-MM-dd'T'HH:mm"
/// (`smithy_query(query="doc",shape="TimeFilter",member="after")`) — a CIVIL date and
/// time with no zone and no seconds, which no modelled `timestamp` reader accepts. So
/// the text is normalised here instead: a value with no zone suffix is read as UTC,
/// one carrying any of the zone forms RFC 3339 admits is shifted by that offset
/// through the ONE shared reader (`service_common::rfc3339::split_utc_offset`), and a
/// civil part given to minute precision is completed to the second. The calendar
/// conversion itself (month lengths, leap years) still comes from the library.
///
/// TOTAL over arbitrary text: the value comes straight off the wire, so every form
/// this does not recognise answers `None` for the caller to refuse as its modelled
/// client error.
pub fn parse_string_date_time(value: &str) -> Option<i64> {
    let (civil, offset_seconds) = ::service_common::rfc3339::split_utc_offset(value)
        // No zone suffix at all is the documented form, which is UTC.
        .unwrap_or((value, 0));
    // `yyyy-MM-ddTHH:mm` carries no seconds field; the reader below requires one.
    let completed = if civil.len() == MINUTE_PRECISION_LENGTH {
        format!("{civil}:00")
    } else {
        civil.to_string()
    };
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{completed}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    parsed.secs().checked_sub(offset_seconds)
}

/// How many characters a civil date-time given to MINUTE precision has
/// (`yyyy-MM-ddTHH:mm`), the form `TimeFilter`'s members document.
const MINUTE_PRECISION_LENGTH: usize = "yyyy-MM-ddTHH:mm".len();

