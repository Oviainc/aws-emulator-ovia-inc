// SPDX-License-Identifier: CC0-1.0

//! The account's VPC Encryption Control configuration: what the account holds, the words
//! the API reports it with, and the clock-owed moves a modify starts.
//!
//! `DescribeAccountVpcEncryptionControl` reports this configuration and
//! `ModifyAccountVpcEncryptionControl` writes it, so BOTH answer the one projection here
//! ([`AccountConfiguration::view_at`]) and neither can report the account two ways.
//!
//! ## One configuration for the account, not one per Region
//!
//! "Describes the account-level VPC Encryption Control configuration for your account"
//! (`DescribeAccountVpcEncryptionControl`) names no Region, so the configuration is a fact
//! of the ACCOUNT: a request signed for any Region reports and modifies the same one, the
//! way the account's Capacity Manager settings
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::capacity_manager_at`]) are the account's
//! rather than a Region's.
//!
//! ## Where it is stored
//!
//! In the shared engine's own typed state slot, reached through
//! [`require_account_configuration`] from the engine every operation of this service is
//! constructed with — never from an engine a handler builds, which would drop what a
//! request wrote. It is a slot of its own rather than another field of
//! [`crate::state::ComAmazonawsEc2Amazonec2State`] because `src/state.rs`, the module that
//! declares that struct, is far past the per-file length ceiling a change to this crate is
//! held to and so cannot take another field. It is still the ONE store of this
//! configuration — nothing else in this crate holds it — so there is no second copy of it
//! to drift from.
//!
//! ## Transient states are derived, never written by a read
//!
//! A modify records the INSTANT its transitions fall due; the state words a reader sees are
//! derived from that instant ([`AccountConfiguration::view_at`]), so reporting the
//! configuration writes nothing at all and two reports at one instant answer the same
//! words. Every word reported is a `&'static str` out of a modelled catalog, so a request's
//! own text is never stored or echoed back as a state.

use std::time::{Instant, SystemTime};

use crate::ops::types::{
    ACCOUNT_VPC_ENCRYPTION_CONTROL_MODE_VALUES, AccountVpcEncryptionControl,
    AccountVpcEncryptionControlExclusions, VPC_ENCRYPTION_CONTROL_EXCLUSION_STATE_INPUT_VALUES,
};
use crate::scaffolded::Scaffolded;
use crate::state::{MANAGED_BY_ACCOUNT, TRANSITION_DELAY, system_time_to_date_time};

/// Encryption neither enforced nor monitored: the `AccountVpcEncryptionControlMode` enum's
/// `unmanaged` member (`smithy_query(query="shape",shape="AccountVpcEncryptionControlMode")`),
/// and where an account that has never modified this configuration sits.
pub const MODE_UNMANAGED: &str = "unmanaged";

/// The configuration of an account nothing has modified: the
/// `AccountVpcEncryptionControlState` enum's `default-state` member
/// (`smithy_query(query="shape",shape="AccountVpcEncryptionControlState")`).
pub const STATE_DEFAULT: &str = "default-state";

/// The same enum's `transitions-in-progress` member: the state a modify leaves the
/// configuration in while the moves it asked for are still being made.
pub const STATE_TRANSITIONS_IN_PROGRESS: &str = "transitions-in-progress";

/// The same enum's `transitions-successful` member: every move the last modify asked for
/// has been made.
///
/// Its remaining two members, `transitions-partially-successful` and `transitions-failed`,
/// name a configuration whose moves did not all succeed. Nothing here can fail one — each
/// move is a change of this store — so no configuration is ever reported in either.
pub const STATE_TRANSITIONS_SUCCESSFUL: &str = "transitions-successful";

/// The `VpcEncryptionControlExclusionStateInput` member that ASKS for an exclusion:
/// "specifies whether to exclude …" (`ModifyAccountVpcEncryptionControlRequest`), whose
/// catalog is `enable` and `disable`
/// (`smithy_query(query="shape",shape="VpcEncryptionControlExclusionStateInput")`).
pub const EXCLUSION_STATE_INPUT_ENABLE: &str = "enable";

/// A resource type being taken out of enforcement: the
/// `VpcEncryptionControlExclusionState` enum's `enabling` member
/// (`smithy_query(query="shape",shape="VpcEncryptionControlExclusionState")`).
pub const EXCLUSION_ENABLING: &str = "enabling";

/// The same enum's `enabled` member: the exclusion is in force, so this resource type is
/// out of account-level encryption enforcement.
pub const EXCLUSION_ENABLED: &str = "enabled";

/// The same enum's `disabling` member: an exclusion being taken back.
pub const EXCLUSION_DISABLING: &str = "disabling";

/// The same enum's `disabled` member: no exclusion, so this resource type is inside
/// enforcement. Where every resource type of an unmodified account sits.
pub const EXCLUSION_DISABLED: &str = "disabled";

/// One resource type the account-level configuration can exclude from encryption
/// enforcement.
///
/// The catalog is the member list of `AccountVpcEncryptionControlExclusions`
/// (`smithy_query(query="shape",shape="AccountVpcEncryptionControlExclusions")`), which is
/// also — member for member — what `ModifyAccountVpcEncryptionControl` takes, so one value
/// here names both the request member a modify reads and the result member a report
/// writes.
///
/// `VpcEncryptionControlExclusions` — what a PER-VPC configuration reports — declares the
/// same eight resource types (`smithy_query(query="shape",shape="VpcEncryptionControlExclusions")`),
/// so the per-VPC surface reads this one catalog too and only spells the REQUEST member
/// differently ([`crate::ops::create_vpc_encryption_control::vpc_encryption_controls`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcludableResource {
    /// `AccountVpcEncryptionControlExclusions$InternetGateway`.
    InternetGateway,
    /// `AccountVpcEncryptionControlExclusions$EgressOnlyInternetGateway`.
    EgressOnlyInternetGateway,
    /// `AccountVpcEncryptionControlExclusions$NatGateway`.
    NatGateway,
    /// `AccountVpcEncryptionControlExclusions$VirtualPrivateGateway`.
    VirtualPrivateGateway,
    /// `AccountVpcEncryptionControlExclusions$VpcPeering`.
    VpcPeering,
    /// `AccountVpcEncryptionControlExclusions$Lambda`.
    Lambda,
    /// `AccountVpcEncryptionControlExclusions$VpcLattice`.
    VpcLattice,
    /// `AccountVpcEncryptionControlExclusions$ElasticFileSystem`.
    ElasticFileSystem,
}

/// Every excludable resource type, in the order `AccountVpcEncryptionControlExclusions`
/// declares its members — the one order this crate reads them in, so a modify and a report
/// cannot disagree about which member is which.
pub const EXCLUDABLE_RESOURCES: [ExcludableResource; 8] = [
    ExcludableResource::InternetGateway,
    ExcludableResource::EgressOnlyInternetGateway,
    ExcludableResource::NatGateway,
    ExcludableResource::VirtualPrivateGateway,
    ExcludableResource::VpcPeering,
    ExcludableResource::Lambda,
    ExcludableResource::VpcLattice,
    ExcludableResource::ElasticFileSystem,
];

impl ExcludableResource {
    /// This resource type's place in [`EXCLUDABLE_RESOURCES`], which is how a stored
    /// exclusion is found for it.
    pub const fn index(self) -> usize {
        match self {
            Self::InternetGateway => 0,
            Self::EgressOnlyInternetGateway => 1,
            Self::NatGateway => 2,
            Self::VirtualPrivateGateway => 3,
            Self::VpcPeering => 4,
            Self::Lambda => 5,
            Self::VpcLattice => 6,
            Self::ElasticFileSystem => 7,
        }
    }

    /// The request member a modify reads this resource type's choice from, spelled as the
    /// wire spells it — which is the name a refusal of that member has to carry.
    pub const fn request_member(self) -> &'static str {
        match self {
            Self::InternetGateway => "internetGateway",
            Self::EgressOnlyInternetGateway => "egressOnlyInternetGateway",
            Self::NatGateway => "natGateway",
            Self::VirtualPrivateGateway => "virtualPrivateGateway",
            Self::VpcPeering => "vpcPeering",
            Self::Lambda => "lambda",
            Self::VpcLattice => "vpcLattice",
            Self::ElasticFileSystem => "elasticFileSystem",
        }
    }
}

/// What a store holds about ONE resource type: whether it is excluded, and when the move
/// to that choice settles.
///
/// Shared with the PER-VPC configurations
/// ([`crate::ops::create_vpc_encryption_control::vpc_encryption_controls`]) rather than
/// copied there: a resource type's exclusion is reported through the same modelled enum at
/// both levels (`VpcEncryptionControlExclusion$State`, whose catalog is
/// `smithy_query(query="shape",shape="VpcEncryptionControlExclusionState")`), so one state
/// machine answers both and the two cannot word one move differently.
#[derive(Debug, Clone, Copy, Default)]
pub struct Exclusion {
    /// Whether this resource type is excluded from encryption enforcement. `false` — inside
    /// enforcement — is where an unmodified account's every resource type sits, and where a
    /// freshly created per-VPC configuration's every resource type sits.
    excluded: bool,
    /// When the move to [`Self::excluded`] falls due, and `None` for a choice nothing has
    /// moved at all.
    ///
    /// The due instant is KEPT once it has passed rather than cleared: whether the move has
    /// settled is derived by comparing it with the instant a report is made
    /// ([`Self::state_at`]), which is what lets a report write nothing.
    settles_at: Option<Instant>,
}

impl Exclusion {
    /// Whether this resource type is excluded, whatever the move to that choice has reached.
    pub const fn is_excluded(self) -> bool {
        self.excluded
    }

    /// The exclusion as a move to `excluded` that settles at `settles_at` — what a modify
    /// that CHANGED the choice leaves behind, the instant being stored rather than slept
    /// for.
    pub const fn moving_to(excluded: bool, settles_at: Instant) -> Self {
        Self {
            excluded,
            settles_at: Some(settles_at),
        }
    }

    /// The `VpcEncryptionControlExclusionState` of this resource type as of `now`: the
    /// settled word once the move to the stored choice has fallen due, and the transitional
    /// one until then.
    pub fn state_at(self, now: Instant) -> &'static str {
        let settled = self.settles_at.is_none_or(|due| now >= due);
        match (self.excluded, settled) {
            (true, true) => EXCLUSION_ENABLED,
            (true, false) => EXCLUSION_ENABLING,
            (false, true) => EXCLUSION_DISABLED,
            (false, false) => EXCLUSION_DISABLING,
        }
    }
}

/// The account's VPC Encryption Control configuration.
///
/// The mode is a `&'static str` out of the modelled `AccountVpcEncryptionControlMode`
/// catalog ([`account_vpc_encryption_control_mode`]), so a request's own text can never be
/// stored here.
#[derive(Debug, Clone)]
pub struct AccountConfiguration {
    /// The account's `AccountVpcEncryptionControlMode`.
    mode: &'static str,
    /// One entry per [`EXCLUDABLE_RESOURCES`] member, found by
    /// [`ExcludableResource::index`].
    exclusions: [Exclusion; EXCLUDABLE_RESOURCES.len()],
    /// When the transitions the last modify started fall due, and `None` while no modify
    /// has started any — which is what tells an account in [`STATE_DEFAULT`] from one whose
    /// transitions have all been made.
    transitions_complete_at: Option<Instant>,
    /// "The date and time when the account-level VPC Encryption Control configuration was
    /// last updated" (`AccountVpcEncryptionControl$LastUpdateTimestamp`): the wall clock of
    /// the modify that last changed something, and `None` for a configuration nothing has
    /// changed — there is then no update to date.
    last_update: Option<SystemTime>,
}

impl Default for AccountConfiguration {
    /// Where an account that has never called `ModifyAccountVpcEncryptionControl` sits:
    /// [`MODE_UNMANAGED`], no resource type excluded, no transition owed and no update to
    /// report a timestamp for — which is the configuration AWS reports in
    /// [`STATE_DEFAULT`].
    fn default() -> Self {
        Self {
            mode: MODE_UNMANAGED,
            exclusions: [Exclusion::default(); EXCLUDABLE_RESOURCES.len()],
            transitions_complete_at: None,
            last_update: None,
        }
    }
}

/// What ONE `ModifyAccountVpcEncryptionControl` request asks for: the mode it names, and
/// per resource type whether it asks for that resource to be excluded.
///
/// A member the request leaves out is `None` and leaves what the account holds alone —
/// "modifies the account-level VPC Encryption Control configuration" changes what the
/// request speaks about and nothing else.
#[derive(Debug, Clone, Default)]
pub struct ConfigurationPatch {
    /// The `AccountVpcEncryptionControlMode` the request names, out of the modelled catalog.
    pub mode: Option<&'static str>,
    /// Per [`EXCLUDABLE_RESOURCES`] member, whether the request asks to exclude it
    /// (`enable`) or to stop excluding it (`disable`).
    pub exclusions: [Option<bool>; EXCLUDABLE_RESOURCES.len()],
}

impl AccountConfiguration {
    /// The configuration as of `now`: the one projection every operation that reports it
    /// answers from.
    ///
    /// Pure: the transient words come from the due instants the last modify recorded, so
    /// nothing is written and two reports at one instant are identical.
    pub fn view_at(&self, now: Instant) -> AccountVpcEncryptionControl {
        AccountVpcEncryptionControl {
            state: Some(self.state_at(now).to_string()),
            mode: Some(self.mode.to_string()),
            exclusions: Some(AccountVpcEncryptionControlExclusions {
                internet_gateway: Some(
                    self.exclusion_at(ExcludableResource::InternetGateway, now)
                        .to_string(),
                ),
                egress_only_internet_gateway: Some(
                    self.exclusion_at(ExcludableResource::EgressOnlyInternetGateway, now)
                        .to_string(),
                ),
                nat_gateway: Some(
                    self.exclusion_at(ExcludableResource::NatGateway, now)
                        .to_string(),
                ),
                virtual_private_gateway: Some(
                    self.exclusion_at(ExcludableResource::VirtualPrivateGateway, now)
                        .to_string(),
                ),
                vpc_peering: Some(
                    self.exclusion_at(ExcludableResource::VpcPeering, now)
                        .to_string(),
                ),
                lambda: Some(self.exclusion_at(ExcludableResource::Lambda, now).to_string()),
                vpc_lattice: Some(
                    self.exclusion_at(ExcludableResource::VpcLattice, now)
                        .to_string(),
                ),
                elastic_file_system: Some(
                    self.exclusion_at(ExcludableResource::ElasticFileSystem, now)
                        .to_string(),
                ),
            }),
            // "The entity that manages the account-level VPC Encryption Control
            // configuration" (`AccountVpcEncryptionControl$ManagedBy`): the `ManagedBy`
            // enum's `account` member, the account itself being the only writer of this
            // configuration this emulator has — nothing here applies a declarative policy.
            managed_by: Some(MANAGED_BY_ACCOUNT.to_string()),
            last_update_timestamp: self.last_update.map(system_time_to_date_time),
        }
    }

    /// The `AccountVpcEncryptionControlState` as of `now`.
    ///
    /// An account no modify has touched is in [`STATE_DEFAULT`]; one whose modify's moves
    /// are still owed is in [`STATE_TRANSITIONS_IN_PROGRESS`] until the instant that modify
    /// recorded, and in [`STATE_TRANSITIONS_SUCCESSFUL`] from it.
    fn state_at(&self, now: Instant) -> &'static str {
        match self.transitions_complete_at {
            None => STATE_DEFAULT,
            Some(due) if now < due => STATE_TRANSITIONS_IN_PROGRESS,
            Some(_) => STATE_TRANSITIONS_SUCCESSFUL,
        }
    }

    /// The `VpcEncryptionControlExclusionState` of `resource` as of `now`, through the one
    /// reading of that enum every level of this surface shares
    /// ([`Exclusion::state_at`]).
    fn exclusion_at(&self, resource: ExcludableResource, now: Instant) -> &'static str {
        self.exclusions[resource.index()].state_at(now)
    }

    /// Apply `patch` and answer the configuration as it then stands as of `now`.
    ///
    /// The ONE writer, so what a later report answers is always what this left here. Only
    /// what `patch` speaks about is touched, and only a value that DIFFERS from what the
    /// account holds is a change: a modify that asks for the configuration already in force
    /// transitions nothing, so it starts no move and dates no update — the configuration it
    /// answers is the one that was already there.
    ///
    /// A change starts the moves it owes: the resource types whose choice changed go to
    /// their transitional state and the configuration to
    /// [`STATE_TRANSITIONS_IN_PROGRESS`], both falling due one [`TRANSITION_DELAY`] out —
    /// the span every clock-owed move of this crate is given. The instant is STORED;
    /// nothing sleeps for it.
    pub fn apply(
        &mut self,
        patch: &ConfigurationPatch,
        now: Instant,
        wall_clock: SystemTime,
    ) -> AccountVpcEncryptionControl {
        let mut changed = false;
        if let Some(mode) = patch.mode {
            if self.mode != mode {
                self.mode = mode;
                changed = true;
            }
        }
        for resource in EXCLUDABLE_RESOURCES {
            let Some(excluded) = patch.exclusions[resource.index()] else {
                continue;
            };
            let exclusion = &mut self.exclusions[resource.index()];
            if exclusion.is_excluded() != excluded {
                *exclusion = Exclusion::moving_to(excluded, now + TRANSITION_DELAY);
                changed = true;
            }
        }
        if changed {
            self.transitions_complete_at = Some(now + TRANSITION_DELAY);
            self.last_update = Some(wall_clock);
        }
        self.view_at(now)
    }
}

/// The modelled `AccountVpcEncryptionControlMode` member `requested` names, or `None` when
/// it names none.
///
/// The catalog is the generated
/// [`crate::ops::types::ACCOUNT_VPC_ENCRYPTION_CONTROL_MODE_VALUES`], so this crate holds no
/// second copy of it; answering a `&'static str` out of that catalog is what keeps request
/// text out of the store.
pub fn account_vpc_encryption_control_mode(requested: &str) -> Option<&'static str> {
    ACCOUNT_VPC_ENCRYPTION_CONTROL_MODE_VALUES
        .iter()
        .copied()
        .find(|mode| *mode == requested)
}

/// Whether `requested` — a `VpcEncryptionControlExclusionStateInput` — asks for the
/// exclusion to be in force, or `None` when it is not a member of that catalog.
///
/// "Specifies whether to exclude … from account-level encryption enforcement"
/// (`ModifyAccountVpcEncryptionControlRequest`), whose catalog is `enable` and `disable`
/// ([`crate::ops::types::VPC_ENCRYPTION_CONTROL_EXCLUSION_STATE_INPUT_VALUES`]):
/// [`EXCLUSION_STATE_INPUT_ENABLE`] asks for the exclusion, `disable` asks for it to be
/// taken back.
pub fn exclusion_state_input(requested: &str) -> Option<bool> {
    VPC_ENCRYPTION_CONTROL_EXCLUSION_STATE_INPUT_VALUES
        .iter()
        .copied()
        .find(|value| *value == requested)
        .map(|value| value == EXCLUSION_STATE_INPUT_ENABLE)
}

/// This configuration's store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// Answers the server error every handler answers a service mounted on an engine that
/// holds no state with — never a fresh engine, whose writes nothing else would ever see.
pub fn require_account_configuration<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
) -> Result<::engine::ServiceState<AccountConfiguration>, Scaffolded<E>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<AccountConfiguration>)
        .ok_or_else(|| {
            Scaffolded::internal_error(
                "the EC2 emulator is mounted on an engine that holds no state",
            )
        })
}
