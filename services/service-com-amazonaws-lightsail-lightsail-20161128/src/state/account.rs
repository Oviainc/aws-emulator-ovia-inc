// SPDX-License-Identifier: CC0-1.0

//! What a release leaves behind, the peering of this Region's Lightsail VPC with
//! the account's default VPC, and the account profile every read of the account
//! itself reports.

use super::*;

/// One reference a resource's removal released: everything the operation log
/// needs to record the release that removal performed.
///
/// A release is a MUTATION of the record that held the reference, so it is
/// logged like any other, under the modelled `OperationType` member for the
/// call that makes the same change by hand — the detach for an attachment, the
/// delete for a record whose reference is required and so cannot be left empty
/// — and located where the record that changed lives.
#[derive(Debug, Clone)]
pub struct ReleasedAttachment {
    /// The `Operation$operationType` this release is logged under, from the
    /// modelled `OperationType` enum.
    pub operation_type: &'static str,
    /// The `Operation$resourceType` of the record the attachment was released
    /// on, from the modelled `ResourceType` enum.
    pub resource_type: &'static str,
    /// That record's own name — the resource the log entry is about.
    pub resource_name: String,
    /// Where that record lives, so the entry is located where the resource is
    /// rather than where the removed resource was.
    pub location: StoredLocation,
}

/// The outcome of removing one instance: the record itself, and every
/// attachment the removal released
/// (see [`ComAmazonawsLightsailLightsail20161128State::remove_instance`]).
#[derive(Debug)]
pub struct RemovedInstance {
    /// The instance record that was removed.
    pub instance: StoredInstance,
    /// One entry per reference the removal released, in the store's own
    /// stable order: disks by name, then static IPs by name, then load
    /// balancers by region and name, then buckets by region and name, then the
    /// alarms that monitored the removed instance by alarm name — every one of
    /// those maps is a `BTreeMap`, so the order is the same on every call.
    pub released_attachments: Vec<ReleasedAttachment>,
}

/// The outcome of removing one managed relational database: the record itself,
/// and every reference another record held to it that the removal released
/// (see
/// [`ComAmazonawsLightsailLightsail20161128State::remove_relational_database`]).
#[derive(Debug)]
pub struct RemovedRelationalDatabase {
    /// The database record that was removed.
    pub database: StoredRelationalDatabase,
    /// One entry per reference the removal released, in the store's own stable
    /// order: the alarms that monitored the removed database, by alarm name
    /// (that map is a `BTreeMap`, so the order is the same on every call).
    pub released_references: Vec<ReleasedAttachment>,
}

/// The outcome of removing one bucket: the record itself, and every reference
/// another record held to it that the removal released
/// (see [`ComAmazonawsLightsailLightsail20161128State::remove_bucket`]).
#[derive(Debug)]
pub struct RemovedBucket {
    /// The bucket record that was removed.
    pub bucket: StoredBucket,
    /// One entry per reference the removal released, in the store's own stable
    /// order: the buckets whose access logs were saved in the removed one, by
    /// region and name (that map is a `BTreeMap`, so the order is the same on
    /// every call).
    pub released_references: Vec<ReleasedAttachment>,
}

// ---------------------------------------------------------------------------
// VPC peering
// ---------------------------------------------------------------------------

/// Where one Region's Lightsail VPC peering is in its lifecycle.
///
/// Lightsail peers a Region's Lightsail VPC with the account's default VPC in
/// that Region on request — `PeerVpc` "peers the Lightsail VPC with the user's
/// default VPC", `UnpeerVpc` "unpeers the Lightsail VPC from the user's default
/// VPC" — and each of those calls answers an operation-log entry rather than the
/// finished connection, so the connection is NOT established (nor torn down) at
/// the instant the call returns: the clock owes it the rest of the walk, like
/// every other non-terminal Lightsail resource in this store.
///
/// `IsVpcPeered` reports one boolean over this walk, and the boolean is about
/// whether a connection EXISTS: a VPC being peered is not peered yet, and one
/// being unpeered still is (see [`VpcPeeringPhase::is_peered`]). The phases
/// themselves are this store's own — the model declares no shape for a Lightsail
/// VPC peering, only the `PeeredVpc` resource type and that one boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpcPeeringPhase {
    /// `PeerVpc` has been called and the connection is being established.
    Peering,
    /// The connection is established: the Lightsail VPC is peered.
    Peered,
    /// `UnpeerVpc` has been called and the connection is being torn down. It is
    /// still up, so the VPC is still peered.
    Unpeering,
    /// The connection is gone. The Lightsail VPC is not peered, and `PeerVpc`
    /// may peer it again.
    Unpeered,
}

impl VpcPeeringPhase {
    /// Whether a Lightsail VPC in this phase IS peered — the one thing
    /// `IsVpcPeered` answers.
    ///
    /// True from the instant the connection is established until the instant it
    /// is gone, so a caller that peers and reads straight back still observes
    /// the transient "not peered yet", exactly as it does against AWS.
    pub fn is_peered(self) -> bool {
        matches!(self, Self::Peered | Self::Unpeering)
    }
}

/// A phase the clock already owes a Region's Lightsail VPC peering.
///
/// Separate from [`DueTransition`] and [`DueNamedState`] because a peering's
/// phase is neither an EC2 `InstanceState` (a code and a name) nor a modelled
/// state name, but this store's own [`VpcPeeringPhase`].
#[derive(Debug, Clone, Copy)]
pub struct DueVpcPeeringPhase {
    /// Epoch milliseconds at which the phase became (or becomes) due.
    pub at_ms: i64,
    /// The phase the peering settles into.
    pub phase: VpcPeeringPhase,
}

impl DueVpcPeeringPhase {
    /// The walk to `phase`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, phase: VpcPeeringPhase) -> Self {
        Self {
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            phase,
        }
    }
}

/// The peering of ONE Region's Lightsail VPC with the account's default VPC
/// there.
///
/// Per Region, because Lightsail's API is regional and the Lightsail VPC
/// `PeerVpc` peers is the one in the Region the request is made in: peering in
/// one Region leaves every other Region's Lightsail VPC unpeered.
#[derive(Debug, Clone)]
pub struct StoredVpcPeering {
    /// Where the connection is now.
    pub phase: VpcPeeringPhase,
    /// What the clock owes it next, while it owes it anything.
    pub due: Option<DueVpcPeeringPhase>,
}

impl StoredVpcPeering {
    /// The peering a `PeerVpc` call at `now_ms` starts: the connection is being
    /// established, and the clock owes it [`VpcPeeringPhase::Peered`].
    pub fn peering(now_ms: i64) -> Self {
        Self {
            phase: VpcPeeringPhase::Peering,
            due: Some(DueVpcPeeringPhase::after(now_ms, VpcPeeringPhase::Peered)),
        }
    }

    /// Start the teardown an `UnpeerVpc` call at `now_ms` asks for: the
    /// connection is still up, and the clock owes it
    /// [`VpcPeeringPhase::Unpeered`].
    pub fn start_unpeering(&mut self, now_ms: i64) {
        self.phase = VpcPeeringPhase::Unpeering;
        self.due = Some(DueVpcPeeringPhase::after(
            now_ms,
            VpcPeeringPhase::Unpeered,
        ));
    }

    /// Whether this Region's Lightsail VPC is peered.
    pub fn is_peered(&self) -> bool {
        self.phase.is_peered()
    }
}

// ---------------------------------------------------------------------------
// Account profile
// ---------------------------------------------------------------------------

/// The `ProfileType` of an account that is not enrolled in the Lightsail partner
/// program, from that enum member's own documentation: "`Lightsailor` – The
/// account is not enrolled in the Lightsail partner program."
pub const PROFILE_TYPE_LIGHTSAILOR: &str = "Lightsailor";

/// The `ProfileType` of an account that IS enrolled, from the other member of the
/// same modelled enum: "`LightsailPartner` – The account is enrolled in the
/// Lightsail partner program."
pub const PROFILE_TYPE_LIGHTSAIL_PARTNER: &str = "LightsailPartner";

/// The `PartnerStatus` of a membership whose tier benefits the account has, from
/// that member's own documentation: "`Active` – The membership is active, and the
/// benefits of the current tier are available to the account."
pub const PARTNER_STATUS_ACTIVE: &str = "Active";

/// The `PartnerStatus` of a membership whose tier benefits are withheld, from the
/// other value the same member documents: "`Suspended` – The membership is
/// suspended, and the benefits of the tier are not available to the account."
pub const PARTNER_STATUS_SUSPENDED: &str = "Suspended";

/// The Lightsail partner program membership of this account, held only while the
/// account is enrolled in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPartnerMembership {
    /// When the account was enrolled in the program — the instant
    /// `PartnerInfo$enrolledAt` reports, and a required member of it.
    pub enrolled_at_ms: i64,
    /// The tier of the membership: one of the modelled `TierName` values.
    /// Optional, because `PartnerInfo$tierName` is — a membership that is in no
    /// tier reports none.
    pub tier_name: Option<String>,
    /// The status of the membership: one of the modelled `PartnerStatus` values
    /// ([`PARTNER_STATUS_ACTIVE`], [`PARTNER_STATUS_SUSPENDED`]).
    pub status: String,
}

impl StoredPartnerMembership {
    /// This membership as the modelled `PartnerInfo`.
    pub fn view(&self) -> PartnerInfo {
        PartnerInfo {
            enrolled_at: timestamp(self.enrolled_at_ms),
            tier_name: self.tier_name.clone(),
            status: self.status.clone(),
        }
    }
}

/// The profile of the account this emulator serves: the Lightsail partner program
/// membership it holds, if any.
///
/// The membership is the whole of the record, because the profile TYPE follows
/// from it — an account is a partner exactly while it holds a membership and a
/// Lightsailor otherwise — so the type and the membership can never disagree.
/// Nothing in this service's API enrols an account in the program (enrolment
/// happens outside Lightsail), so a store nothing has seeded holds no
/// membership, which is what makes the profile of a fresh account the
/// not-enrolled one.
#[derive(Debug, Clone, Default)]
pub struct StoredAccountProfile {
    /// The account's partner program membership, and `None` while it is not
    /// enrolled.
    pub partner: Option<StoredPartnerMembership>,
}

impl StoredAccountProfile {
    /// The modelled `ProfileType` of this profile: an enrolled account is a
    /// partner, every other account is a Lightsailor.
    pub fn profile_type(&self) -> &'static str {
        if self.partner.is_some() {
            PROFILE_TYPE_LIGHTSAIL_PARTNER
        } else {
            PROFILE_TYPE_LIGHTSAILOR
        }
    }

    /// The membership as the modelled `PartnerInfo`, which is reported "only for
    /// accounts that have a `profileType` of `LightsailPartner`" — an account
    /// holding no membership has none to report.
    pub fn partner_view(&self) -> Option<PartnerInfo> {
        self.partner.as_ref().map(StoredPartnerMembership::view)
    }
}
