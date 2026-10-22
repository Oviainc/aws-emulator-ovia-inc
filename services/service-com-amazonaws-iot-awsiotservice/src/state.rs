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

use std::collections::{BTreeMap, BTreeSet, HashMap};

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
