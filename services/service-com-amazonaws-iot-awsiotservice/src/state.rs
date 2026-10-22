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
//! `/thing-groups/{thingGroupName}` — and the account's IoT ENDPOINT IDENTIFIERS — the
//! `<identifier>` of `<identifier>.iot.<region>.amazonaws.com`, the account-specific
//! prefix every IoT endpoint address is built from. `DescribeEndpoint` is documented
//! as "returns or creates a unique endpoint specific to the Amazon Web Services
//! account making the call. The first time DescribeEndpoint is called, an endpoint is
//! created. All subsequent calls to DescribeEndpoint return the same endpoint", so the
//! identifier is recorded here on the call that creates it and read back by every
//! later call.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

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
#[derive(Debug, Default)]
pub struct ComAmazonawsIotAwsiotserviceState {
    /// Every thing that exists, keyed by `thingName`.
    pub things: BTreeMap<String, RegisteredThing>,
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

/// The ARN of a thing group: `arn:aws:iot:<region>:<account>:thinggroup/<thingGroupName>`.
///
/// `thinggroup` (one word, lowercase) is the resource type AWS IoT's thing-group
/// ARNs use, beside `thing/` for a thing; the namespace and account segments are the
/// same ones [`thing_arn`] documents.
pub fn thing_group_arn(region: &str, thing_group_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("thinggroup/{thing_group_name}"))
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
