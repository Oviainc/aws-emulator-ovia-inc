// SPDX-License-Identifier: CC0-1.0

//! The account's PER-VPC VPC Encryption Control configurations: what the store holds, the
//! words the API reports one with, and the clock-owed moves a create, a modify and a delete
//! start.
//!
//! `CreateVpcEncryptionControl` mints one, `ModifyVpcEncryptionControl` writes it,
//! `DeleteVpcEncryptionControl` retires it and `DescribeVpcEncryptionControls` reports it, so
//! all four answer the ONE projection here ([`VpcEncryptionControlRecord::view_at`]) and no
//! two of them can report one configuration differently.
//!
//! ## One configuration per VPC
//!
//! "Creates a VPC Encryption Control configuration for a specified VPC"
//! (`CreateVpcEncryptionControl`) and "the ID of the VPC associated with the encryption
//! control configuration" (`VpcEncryptionControl$VpcId`): a configuration belongs to exactly
//! one VPC, and a VPC that already has one is already associated — so a second create for it
//! is refused ([`vpc_already_has_a_configuration`]) rather than minting a second
//! configuration of the same VPC that two readers could disagree about.
//!
//! ## Where it is stored
//!
//! In the shared engine's own typed state slot, reached through
//! [`require_vpc_encryption_controls`] from the engine every operation of this service is
//! constructed with — never from an engine a handler builds, which would drop what a request
//! wrote. It is a slot of its own rather than another field of
//! [`crate::state::ComAmazonawsEc2Amazonec2State`] because `src/state.rs`, the module that
//! declares that struct, is far past the per-file length ceiling a change to this crate is
//! held to and so cannot take another field. It is still the ONE store of these
//! configurations — nothing else in this crate holds one — so there is no second copy to
//! drift from. The slot is never locked while the service's main store is: each handler
//! finishes with one before it takes the other, so the two have no lock order to invert.
//!
//! ## Transient states are derived, never written by a read
//!
//! A create and a modify record the INSTANT the move each starts falls due; the state words a
//! reader sees are derived from that instant ([`VpcEncryptionControlRecord::state_at`],
//! [`Exclusion::state_at`]), so reporting a configuration writes nothing at all and two
//! reports at one instant answer the same words. Every word reported is a `&'static str` out
//! of a modelled catalog, so a request's own text is never stored or echoed back as a state.
//! A DELETE is the one move with no transient record behind it: it takes the configuration out
//! of the store and answers it [`STATE_DELETING`], after which its id names no configuration
//! at all.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::ops::describe_account_vpc_encryption_control::account_configuration::{
    EXCLUDABLE_RESOURCES, ExcludableResource, Exclusion,
};
use crate::ops::types::{
    Tag, VPC_ENCRYPTION_CONTROL_MODE_VALUES, VpcEncryptionControl, VpcEncryptionControlExclusion,
    VpcEncryptionControlExclusions,
};
use crate::scaffolded::Scaffolded;
use crate::state::{
    MAX_STORED_RECORDS_PER_STORE, TRANSITION_DELAY, id_not_found, invalid_parameter_value,
    require_well_formed_id,
};

/// The id prefix a VPC Encryption Control configuration is minted under.
///
/// `VpcEncryptionControlId` publishes no pattern
/// (`smithy_query(query="shape",shape="VpcEncryptionControlId")`), so the id takes the
/// `<prefix>-<hex>` form every EC2 resource id takes and
/// [`crate::state::ComAmazonawsEc2Amazonec2State::next_id`] mints it, under the prefix EC2
/// names this resource's ids by.
pub const VPC_ENCRYPTION_CONTROL_ID_PREFIX: &str = "vec";

/// The one `ResourceType` a `CreateVpcEncryptionControl` tag specification may name: "the tags
/// to apply to the VPC Encryption Control resource"
/// (`CreateVpcEncryptionControlRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const VPC_ENCRYPTION_CONTROL_TAGGABLE_RESOURCE: &str = "vpc-encryption-control";

/// The `VpcEncryptionControlState` a configuration is in while the create that minted it is
/// still being made: the enum's `creating` member
/// (`smithy_query(query="shape",shape="VpcEncryptionControlState")`).
pub const STATE_CREATING: &str = "creating";

/// The same enum's `available` member: the configuration is in force, every move asked of it
/// having been made. The one terminal state short of deletion.
pub const STATE_AVAILABLE: &str = "available";

/// The same enum's `enforce-in-progress` member: the move to [`MODE_ENFORCE`] is still being
/// made.
pub const STATE_ENFORCE_IN_PROGRESS: &str = "enforce-in-progress";

/// The same enum's `monitor-in-progress` member: the move to [`MODE_MONITOR`] is still being
/// made.
pub const STATE_MONITOR_IN_PROGRESS: &str = "monitor-in-progress";

/// The same enum's `deleting` member: the delete has been asked for, and this is the last
/// state the configuration it retired is ever reported in.
///
/// The enum's remaining members name a configuration whose move did not succeed
/// (`enforce-failed`, `monitor-failed`, `delete-failed`) or one already gone (`deleted`).
/// None of the four is ever reported: nothing here can fail a move, every move being a change
/// of this store, and a delete takes the configuration OUT of the store, so the id it retired
/// names no configuration from that instant on rather than naming one in `deleted`.
pub const STATE_DELETING: &str = "deleting";

/// The `VpcEncryptionControlMode` a configuration nothing has modified is in: the enum's
/// `monitor` member (`smithy_query(query="shape",shape="VpcEncryptionControlMode")`).
///
/// `CreateVpcEncryptionControl` takes no `Mode`, and the catalog holds only `monitor` and
/// `enforce` — it has no member for "no mode at all", the way the account-level
/// `AccountVpcEncryptionControlMode` has `unmanaged` — so a configuration is in one of the two
/// from the moment it exists. A create that asks for neither gets the one that refuses no
/// traffic: enforcement is what `ModifyVpcEncryptionControl$Mode` is for ("the encryption mode
/// for the VPC Encryption Control configuration").
pub const MODE_MONITOR: &str = "monitor";

/// The same enum's `enforce` member: encryption in transit is enforced for the VPC rather than
/// only reported on.
pub const MODE_ENFORCE: &str = "enforce";

/// EC2's code for a VPC Encryption Control configuration id that names no configuration of
/// the account, in the `Invalid<Kind>.NotFound` form EC2's error-code list gives every
/// unresolvable resource id.
pub const INVALID_VPC_ENCRYPTION_CONTROL_ID_NOT_FOUND: &str =
    "InvalidVpcEncryptionControlId.NotFound";

/// EC2's code for something that is not a VPC Encryption Control configuration id at all, so
/// there is nothing to look up — asked BEFORE the lookup, so a caller can tell "not an id of
/// this kind" from "no such configuration".
pub const INVALID_VPC_ENCRYPTION_CONTROL_ID_MALFORMED: &str =
    "InvalidVpcEncryptionControlId.Malformed";

/// EC2's quota code for one VPC Encryption Control configuration too many, in the
/// `<Resource>LimitExceeded` form EC2 answers every per-resource ceiling with
/// (`InternetGatewayLimitExceeded`, `VolumeLimitExceeded`, `LaunchTemplateLimitExceeded`).
pub const VPC_ENCRYPTION_CONTROL_LIMIT_EXCEEDED: &str = "VpcEncryptionControlLimitExceeded";

/// Refuse a request that named a configuration id this account holds no configuration for:
/// HTTP 400 [`INVALID_VPC_ENCRYPTION_CONTROL_ID_NOT_FOUND`], worded by the one place this
/// crate words a missing resource id ([`id_not_found`]) and naming the id the REQUEST named.
pub fn invalid_vpc_encryption_control_id_not_found<E>(id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_VPC_ENCRYPTION_CONTROL_ID_NOT_FOUND,
        "vpc-encryption-control ID",
        id,
    )
}

/// Refuse a create for a VPC that already has a configuration: HTTP 400
/// `InvalidParameterValue`, the code EC2 answers a `VpcId` it will not create a second
/// configuration for.
///
/// The sentence is fixed: it states the rule the request broke — one configuration per VPC —
/// and names neither the VPC nor the configuration already in force, because "this VPC" is
/// the one the request itself named.
pub fn vpc_already_has_a_configuration<E>() -> Scaffolded<E> {
    invalid_parameter_value("Vpc Encryption Control resource already exists for this VPC.")
}

/// Refuse a report whose `VpcEncryptionControlIds` names a configuration the account does not
/// hold: HTTP 400 [`INVALID_VPC_ENCRYPTION_CONTROL_ID_NOT_FOUND`].
///
/// A list member's refusal speaks about the LIST rather than about one entry — EC2 states
/// "one or more of the … provided do not exist" for an id list, naming no single id, which is
/// why this sentence carries no part of the request at all.
pub fn vpc_encryption_control_ids_not_found<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_VPC_ENCRYPTION_CONTROL_ID_NOT_FOUND,
        400,
        "One or more of the VpcEncryptionControlIds provided do not exist",
    )
}

/// Is `allowed` the `VpcEncryptionControlMode` catalog — the value set the model declares for
/// the `Mode` a per-VPC configuration is in?
///
/// The boundary's constraint walk reports a refused enum value as the value set it failed
/// rather than as the shape that declared it, so the catalog IS how that type is recognised: a
/// model fact, the same one codegen wrote into [`VPC_ENCRYPTION_CONTROL_MODE_VALUES`]. The
/// comparison is on the SET and not on the order, because the walk reports the members in the
/// order the model declares them while codegen sorts them. Read by [`crate::service`] so a
/// `Mode` outside the catalog is refused with EC2's own code for it rather than with the
/// framework's generic one.
pub fn is_vpc_encryption_control_mode_catalog(allowed: &[String]) -> bool {
    allowed.len() == VPC_ENCRYPTION_CONTROL_MODE_VALUES.len()
        && VPC_ENCRYPTION_CONTROL_MODE_VALUES
            .iter()
            .all(|member| allowed.iter().any(|held| held == member))
}

/// How EC2 states a `Mode` the `VpcEncryptionControlMode` catalog does not hold: the fixed
/// sentence `Mode must be one of: [<members>]`, whose one variable part is the catalog itself.
///
/// ONE sentence for both paths that can refuse it — the boundary walk, through
/// [`crate::service`], and `ModifyVpcEncryptionControl`'s own backstop — so the two cannot
/// word one refusal differently. The members are read off the catalog itself, so a member the
/// model adds later is offered too rather than silently left out.
pub fn vpc_encryption_control_mode_refusal_message() -> String {
    format!(
        "Mode must be one of: [{members}]",
        members = VPC_ENCRYPTION_CONTROL_MODE_VALUES.join(", "),
    )
}

/// Refuse a create the store has no room for: HTTP 400
/// [`VPC_ENCRYPTION_CONTROL_LIMIT_EXCEEDED`].
pub fn vpc_encryption_control_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(VPC_ENCRYPTION_CONTROL_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a configuration id that is not one at all: HTTP 400
/// [`INVALID_VPC_ENCRYPTION_CONTROL_ID_MALFORMED`], asked before any lookup through the one
/// place this crate asks that question ([`require_well_formed_id`]).
pub fn require_well_formed_configuration_id<E>(id: &str) -> Result<(), Scaffolded<E>> {
    require_well_formed_id(
        VPC_ENCRYPTION_CONTROL_ID_PREFIX,
        id,
        INVALID_VPC_ENCRYPTION_CONTROL_ID_MALFORMED,
    )
}

/// The modelled `VpcEncryptionControlMode` member `requested` names, or `None` when it names
/// none.
///
/// The catalog is the generated [`VPC_ENCRYPTION_CONTROL_MODE_VALUES`], so this crate holds no
/// second copy of it; answering a `&'static str` out of that catalog is what keeps request
/// text out of the store.
pub fn vpc_encryption_control_mode(requested: &str) -> Option<&'static str> {
    VPC_ENCRYPTION_CONTROL_MODE_VALUES
        .iter()
        .copied()
        .find(|mode| *mode == requested)
}

/// The `ModifyVpcEncryptionControl` request member that asks about `resource`, spelled as the
/// wire spells it — which is the name a refusal of that member has to carry.
///
/// The per-VPC modify spells each member with the `Exclusion` suffix
/// (`smithy_query(query="operation",operation="ModifyVpcEncryptionControl")`) where the
/// account-level one spells it plain ([`ExcludableResource::request_member`]), so the two
/// spellings live beside their own operation rather than one being taken for the other.
pub fn exclusion_request_member(resource: ExcludableResource) -> &'static str {
    match resource {
        ExcludableResource::InternetGateway => "internetGatewayExclusion",
        ExcludableResource::EgressOnlyInternetGateway => "egressOnlyInternetGatewayExclusion",
        ExcludableResource::NatGateway => "natGatewayExclusion",
        ExcludableResource::VirtualPrivateGateway => "virtualPrivateGatewayExclusion",
        ExcludableResource::VpcPeering => "vpcPeeringExclusion",
        ExcludableResource::Lambda => "lambdaExclusion",
        ExcludableResource::VpcLattice => "vpcLatticeExclusion",
        ExcludableResource::ElasticFileSystem => "elasticFileSystemExclusion",
    }
}

/// Where one configuration is in its lifecycle, and when the move it is making falls due.
///
/// Only a move AWS reports a resource mid-way through is modelled here: a create and a change
/// of mode each answer a transient `VpcEncryptionControlState` and settle one
/// [`TRANSITION_DELAY`] later, the span every clock-owed move of this crate is given. A change
/// of an exclusion is NOT one of them — an exclusion carries its own transient state
/// ([`Exclusion::state_at`]) and the configuration stays in the state it was in. Neither is a
/// DELETE: it takes the configuration out of the store, so there is no record left to report a
/// state for.
#[derive(Debug, Clone, Copy)]
enum Lifecycle {
    /// [`STATE_CREATING`] until the instant the create recorded, [`STATE_AVAILABLE`] from it.
    ///
    /// There is no settled variant beside these two: a configuration whose last move has
    /// fallen due is [`STATE_AVAILABLE`] because that instant has passed, so "in force" is
    /// derived from the clock rather than written by anything.
    Creating { settles_at: Instant },
    /// The transient state of a mode change until the instant the modify recorded,
    /// [`STATE_AVAILABLE`] from it.
    ModeChanging {
        in_progress: &'static str,
        settles_at: Instant,
    },
}

/// ONE stored VPC Encryption Control configuration: the facts a request chose, and the
/// instants the moves it started fall due on.
///
/// The mode is a `&'static str` out of the modelled `VpcEncryptionControlMode` catalog
/// ([`vpc_encryption_control_mode`]), so a request's own text can never be stored here.
#[derive(Debug, Clone)]
pub struct VpcEncryptionControlRecord {
    /// "The ID of the VPC associated with the encryption control configuration"
    /// (`VpcEncryptionControl$VpcId`) — the VPC the create named, which never changes.
    vpc_id: String,
    /// "The ID of the VPC Encryption Control configuration"
    /// (`VpcEncryptionControl$VpcEncryptionControlId`), as this service minted it.
    id: String,
    /// This configuration's `VpcEncryptionControlMode`.
    mode: &'static str,
    /// One entry per [`EXCLUDABLE_RESOURCES`] member, found by [`ExcludableResource::index`].
    exclusions: [Exclusion; EXCLUDABLE_RESOURCES.len()],
    /// Where this configuration is in its lifecycle.
    lifecycle: Lifecycle,
    /// "The tags assigned to the VPC Encryption Control configuration"
    /// (`VpcEncryptionControl$Tags`), as the create's `TagSpecifications` asked for.
    tags: Vec<Tag>,
}

/// What ONE `ModifyVpcEncryptionControl` request asks for: the mode it names, and per resource
/// type whether it asks for that resource to be excluded.
///
/// A member the request leaves out is `None` and leaves what the configuration holds alone —
/// "modifies the VPC Encryption Control configuration" changes what the request speaks about
/// and nothing else.
#[derive(Debug, Clone, Default)]
pub struct ConfigurationPatch {
    /// The `VpcEncryptionControlMode` the request names, out of the modelled catalog.
    pub mode: Option<&'static str>,
    /// Per [`EXCLUDABLE_RESOURCES`] member, whether the request asks to exclude it (`enable`)
    /// or to stop excluding it (`disable`).
    pub exclusions: [Option<bool>; EXCLUDABLE_RESOURCES.len()],
}

impl VpcEncryptionControlRecord {
    /// The `VpcEncryptionControlId` this configuration was minted under.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The VPC this configuration belongs to.
    pub fn vpc_id(&self) -> &str {
        &self.vpc_id
    }

    /// The `VpcEncryptionControlState` as of `now`.
    ///
    /// Pure: the transient words come from the due instants the last writer recorded, so
    /// nothing is written and two reports at one instant are identical.
    pub fn state_at(&self, now: Instant) -> &'static str {
        match self.lifecycle {
            Lifecycle::Creating { settles_at } if now < settles_at => STATE_CREATING,
            Lifecycle::ModeChanging {
                in_progress,
                settles_at,
            } if now < settles_at => in_progress,
            Lifecycle::Creating { .. } | Lifecycle::ModeChanging { .. } => STATE_AVAILABLE,
        }
    }

    /// This configuration as of `now`: the one projection every operation that reports it
    /// answers from.
    pub fn view_at(&self, now: Instant) -> VpcEncryptionControl {
        self.view_in_state(self.state_at(now), now)
    }

    /// The same projection with the `VpcEncryptionControlState` stated rather than derived,
    /// for the one report of a configuration the store no longer holds: what a delete answers
    /// about the configuration it just took out ([`STATE_DELETING`]).
    fn view_in_state(&self, state: &str, now: Instant) -> VpcEncryptionControl {
        VpcEncryptionControl {
            vpc_id: Some(self.vpc_id.clone()),
            vpc_encryption_control_id: Some(self.id.clone()),
            mode: Some(self.mode.to_string()),
            state: Some(state.to_string()),
            // "A message providing additional information about the encryption control state"
            // (`VpcEncryptionControl$StateMessage`): every state this store reaches is a state
            // of the lifecycle itself, with nothing additional to say about it — no failure
            // state is reachable, nothing here being able to fail a move.
            state_message: None,
            resource_exclusions: Some(VpcEncryptionControlExclusions {
                internet_gateway: Some(self.exclusion_at(ExcludableResource::InternetGateway, now)),
                egress_only_internet_gateway: Some(
                    self.exclusion_at(ExcludableResource::EgressOnlyInternetGateway, now),
                ),
                nat_gateway: Some(self.exclusion_at(ExcludableResource::NatGateway, now)),
                virtual_private_gateway: Some(
                    self.exclusion_at(ExcludableResource::VirtualPrivateGateway, now),
                ),
                vpc_peering: Some(self.exclusion_at(ExcludableResource::VpcPeering, now)),
                lambda: Some(self.exclusion_at(ExcludableResource::Lambda, now)),
                vpc_lattice: Some(self.exclusion_at(ExcludableResource::VpcLattice, now)),
                elastic_file_system: Some(
                    self.exclusion_at(ExcludableResource::ElasticFileSystem, now),
                ),
            }),
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled exclusion of `resource` as of `now`, through the one reading of the
    /// `VpcEncryptionControlExclusionState` catalog this surface shares
    /// ([`Exclusion::state_at`]).
    fn exclusion_at(
        &self,
        resource: ExcludableResource,
        now: Instant,
    ) -> VpcEncryptionControlExclusion {
        VpcEncryptionControlExclusion {
            state: Some(self.exclusions[resource.index()].state_at(now).to_string()),
            // The same reading as `VpcEncryptionControl$StateMessage` above: an exclusion
            // reaches only the states of its own move, so there is nothing additional to say.
            state_message: None,
        }
    }

    /// Apply `patch` and answer the configuration as it then stands as of `now`.
    ///
    /// Only what `patch` speaks about is touched, and only a value that DIFFERS from what the
    /// configuration holds is a change: a modify that asks for what is already in force
    /// transitions nothing, so it starts no move — the configuration it answers is the one
    /// that was already there.
    ///
    /// A change of mode starts the move it owes — the configuration goes to that mode's
    /// in-progress state, falling due one [`TRANSITION_DELAY`] out — and a change of an
    /// exclusion starts that exclusion's own. The instants are STORED; nothing sleeps for
    /// them.
    pub fn apply(&mut self, patch: &ConfigurationPatch, now: Instant) -> VpcEncryptionControl {
        if let Some(mode) = patch.mode {
            if self.mode != mode {
                self.mode = mode;
                self.lifecycle = Lifecycle::ModeChanging {
                    in_progress: mode_in_progress_state(mode),
                    settles_at: now + TRANSITION_DELAY,
                };
            }
        }
        for resource in EXCLUDABLE_RESOURCES {
            let Some(excluded) = patch.exclusions[resource.index()] else {
                continue;
            };
            let exclusion = &mut self.exclusions[resource.index()];
            if exclusion.is_excluded() != excluded {
                *exclusion = Exclusion::moving_to(excluded, now + TRANSITION_DELAY);
            }
        }
        self.view_at(now)
    }
}

/// The `VpcEncryptionControlState` a move to `mode` is reported in while it is being made.
///
/// One state per mode, as the catalog pairs them: `enforce-in-progress` for `enforce` and
/// `monitor-in-progress` for `monitor`
/// (`smithy_query(query="shape",shape="VpcEncryptionControlState")`).
fn mode_in_progress_state(mode: &str) -> &'static str {
    if mode == MODE_ENFORCE {
        STATE_ENFORCE_IN_PROGRESS
    } else {
        STATE_MONITOR_IN_PROGRESS
    }
}

/// The account's VPC Encryption Control configurations, keyed by `VpcEncryptionControlId`.
///
/// A [`BTreeMap`] for the reason every other store of this crate is one: it gives the describe
/// path one stable order and lets a pagination token name the id a page ends at.
#[derive(Debug, Default)]
pub struct VpcEncryptionControls {
    configurations: BTreeMap<String, VpcEncryptionControlRecord>,
}

impl VpcEncryptionControls {
    /// The configuration that GOVERNS `vpc_id`, or `None` when that VPC has none.
    ///
    /// This is what a create asks before minting, and what decides whether a VPC may be given
    /// one. A delete takes its configuration out of the store, so the VPC it belonged to has
    /// none from that instant on.
    pub fn governing(&self, vpc_id: &str) -> Option<&VpcEncryptionControlRecord> {
        self.configurations
            .values()
            .find(|record| record.vpc_id == vpc_id)
    }

    /// The configuration `id` names, or `None` when the account holds none under it.
    pub fn get(&self, id: &str) -> Option<&VpcEncryptionControlRecord> {
        self.configurations.get(id)
    }

    /// The same configuration, to write.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut VpcEncryptionControlRecord> {
        self.configurations.get_mut(id)
    }

    /// Every stored configuration, in `VpcEncryptionControlId` order — the order a describe
    /// pages in, which the store itself establishes.
    pub fn ordered(&self) -> impl Iterator<Item = &VpcEncryptionControlRecord> {
        self.configurations.values()
    }

    /// Whether another configuration fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`], the ceiling every other store of this crate holds):
    /// a create asks first so it can refuse with [`vpc_encryption_control_limit_exceeded`]
    /// instead of overshooting.
    ///
    /// The bytes one record retains are bounded already: everything on it but the tags is
    /// derived by this crate out of ids it minted, and the tags were held to the documented
    /// count and per-tag sizes by [`crate::state::tags_from_specifications`] before the create
    /// got here.
    pub fn has_capacity(&self) -> bool {
        self.configurations.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a configuration of `vpc_id` under `id` carrying `tags`, and answer it as it
    /// stands at `now`.
    ///
    /// The ONE writer of a new configuration, so what a later report answers is always what
    /// this left here. A fresh configuration is in [`MODE_MONITOR`] with every resource type
    /// inside enforcement and nothing excluded, and is reported [`STATE_CREATING`] until the
    /// instant recorded here — one [`TRANSITION_DELAY`] out — from which it is
    /// [`STATE_AVAILABLE`].
    pub fn insert(
        &mut self,
        id: String,
        vpc_id: String,
        tags: Vec<Tag>,
        now: Instant,
    ) -> VpcEncryptionControl {
        let record = VpcEncryptionControlRecord {
            vpc_id,
            id: id.clone(),
            mode: MODE_MONITOR,
            exclusions: [Exclusion::default(); EXCLUDABLE_RESOURCES.len()],
            lifecycle: Lifecycle::Creating {
                settles_at: now + TRANSITION_DELAY,
            },
            tags,
        };
        let view = record.view_at(now);
        self.configurations.insert(id, record);
        view
    }

    /// The configuration `id` names, refusing a request this store cannot resolve: a
    /// malformed id first and an unknown one second — the order EC2 asks the two questions in,
    /// so "not an id of this kind" is never reported as "no such configuration".
    pub fn require<E>(&self, id: &str) -> Result<&VpcEncryptionControlRecord, Scaffolded<E>> {
        require_well_formed_configuration_id(id)?;
        self.get(id)
            .ok_or_else(|| invalid_vpc_encryption_control_id_not_found(id))
    }

    /// The same configuration, to WRITE, refused the same way.
    pub fn require_mut<E>(
        &mut self,
        id: &str,
    ) -> Result<&mut VpcEncryptionControlRecord, Scaffolded<E>> {
        require_well_formed_configuration_id(id)?;
        self.get_mut(id)
            .ok_or_else(|| invalid_vpc_encryption_control_id_not_found(id))
    }

    /// Take the configuration `id` names OUT of the store and answer it in [`STATE_DELETING`]
    /// as of `now`: "information about the deleted VPC Encryption Control configuration"
    /// (`DeleteVpcEncryptionControlResult$VpcEncryptionControl`).
    ///
    /// The ONE retirement of a configuration, refused the same way a modify's lookup is: the
    /// id names no configuration from this instant on, so a second delete of it, and every
    /// later read, is answered the same as an id this service never minted — and the VPC it
    /// belonged to may be given a new configuration at once.
    pub fn delete<E>(
        &mut self,
        id: &str,
        now: Instant,
    ) -> Result<VpcEncryptionControl, Scaffolded<E>> {
        self.require::<E>(id)?;
        let record = self
            .configurations
            .remove(id)
            .ok_or_else(|| invalid_vpc_encryption_control_id_not_found(id))?;
        Ok(record.view_in_state(STATE_DELETING, now))
    }
}

/// This store's slot in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answers the server error every handler answers a service mounted on an engine that holds no
/// state with — never a fresh engine, whose writes nothing else would ever see.
pub fn require_vpc_encryption_controls<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
) -> Result<::engine::ServiceState<VpcEncryptionControls>, Scaffolded<E>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<VpcEncryptionControls>)
        .ok_or_else(|| {
            Scaffolded::internal_error("the EC2 emulator is mounted on an engine that holds no state")
        })
}
