// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-managedblockchain-taigawebservice service.
//!
//! The service's primary resource is the blockchain **network**, which owns the
//! **members** that joined it and the **nodes** that run on it. Beside the networks sit
//! the **invitations** this account holds — one per approved `InviteAction`, each naming
//! the network its invitee may join — and the account's **accessors**: the token-based
//! access objects `CreateAccessor` mints, which belong to the account rather than to any
//! one network. Every implemented operation answers from the records below: ids, ARNs,
//! endpoints, billing tokens and creation dates are minted once by the call that creates
//! the record and stored, never recomputed on a read.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Accessor, AccessorSummary, Invitation, Member, MemberFabricAttributes, MemberFrameworkAttributes,
    MemberLogPublishingConfiguration, MemberSummary, Network, NetworkEthereumAttributes,
    NetworkFabricAttributes, NetworkFrameworkAttributes, NetworkSummary, Node,
    NodeEthereumAttributes, NodeFabricAttributes, NodeFrameworkAttributes,
    NodeLogPublishingConfiguration, NodeSummary, Proposal, ProposalActions, ProposalSummary,
    ResourceNotFoundException, VotingPolicy,
};

/// The signing name, endpoint prefix and ARN namespace of this service (the model's
/// `aws.auth#sigv4` / `aws.api#service` traits all say `managedblockchain`).
pub const ARN_SERVICE: &str = "managedblockchain";

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a network, member or node stays `CREATING` before the clock owes it
/// `AVAILABLE`.
///
/// Managed Blockchain reports a freshly created resource in the non-terminal
/// `CREATING` status ("The Amazon Web Services account is in the process of creating a
/// node" — `Node$Status`), so the settled status is a transition the clock owes. No AWS
/// documentation fixes how long provisioning takes, so the span is this emulator's own
/// choice; it is deliberately non-zero, so a caller acting straight after the create
/// still observes `CREATING` and the state errors that go with it. The instant it comes
/// due is stored on the record and materialised by [`StoredNetwork::settle`] — nothing
/// sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `com.amazonaws.managedblockchain#Framework` value this service can create a network
/// for. `CreateNetwork` is documented "Applies only to Hyperledger Fabric", and its
/// `@required` `MemberConfiguration` is itself documented "Applies only to Hyperledger
/// Fabric", so an `ETHEREUM` network cannot be created.
pub const FRAMEWORK_HYPERLEDGER_FABRIC: &str = "HYPERLEDGER_FABRIC";

/// The other `com.amazonaws.managedblockchain#Framework` value (the enum is
/// `HYPERLEDGER_FABRIC | ETHEREUM`). No create mints one: the Ethereum networks this
/// service serves are the standing, AWS-owned public ones in [`PUBLIC_NETWORKS`].
pub const FRAMEWORK_ETHEREUM: &str = "ETHEREUM";

/// The Hyperledger Fabric framework versions a network can be created with.
///
/// `FrameworkVersionString` carries only `@length(1, 8)`, so the model does not enumerate
/// the versions; this is the catalog the Amazon Managed Blockchain Hyperledger Fabric
/// Developer Guide publishes for `CreateNetwork`'s `FrameworkVersion` (Hyperledger Fabric
/// 1.2, 1.4 and 2.2). Anything else is a version the service does not support, and the
/// create naming it is refused — see [`framework_version_supported`].
pub const FABRIC_FRAMEWORK_VERSIONS: &[&str] = &["1.2", "1.4", "2.2"];

/// The framework versions a network of `framework` can be created with.
///
/// Only Hyperledger Fabric networks are creatable here (`CreateNetwork` "Applies only to
/// Hyperledger Fabric"), so every other framework supports no create-able version at all.
pub fn supported_framework_versions(framework: &str) -> &'static [&'static str] {
    if framework == FRAMEWORK_HYPERLEDGER_FABRIC {
        FABRIC_FRAMEWORK_VERSIONS
    } else {
        &[]
    }
}

/// Whether `framework_version` is one of the versions `framework` supports.
pub fn framework_version_supported(framework: &str, framework_version: &str) -> bool {
    supported_framework_versions(framework).contains(&framework_version)
}

/// One of AWS's standing, AWS-owned public blockchain networks.
///
/// These are not account resources: no call creates them, every account reads the same
/// ids, and they are already `AVAILABLE` when first read, so the clock owes them no
/// transition. [`ComAmazonawsManagedblockchainTaigawebserviceState::default`]
/// materialises one record per entry, which is what `GetNetwork` resolves.
#[derive(Debug, Clone, Copy)]
pub struct PublicNetwork {
    /// The id every account reads the network under.
    pub id: &'static str,
    /// `Network$Name`.
    pub name: &'static str,
    /// A `Framework` value.
    pub framework: &'static str,
    /// `NetworkEthereumAttributes$ChainId`.
    pub chain_id: &'static str,
    /// `Network$CreationDate`, in seconds since the epoch: the network's own genesis,
    /// which is a fact about the public chain and so is stable across reads and across
    /// runs — never the instant this emulator happened to materialise the record.
    pub genesis_epoch_seconds: u64,
}

/// The AWS-owned public networks this service serves.
///
/// Catalog source: `NetworkEthereumAttributes$ChainId` documents the Ethereum chain ids
/// Managed Blockchain reports ("mainnet = `1`"), and `AccessorNetworkType` names the
/// public Ethereum networks the service exposes (`ETHEREUM_MAINNET`, `ETHEREUM_GOERLI`);
/// the Goerli testnet's published chain id is `5`. The ids and names are the ones AMB
/// Access publishes for those networks.
pub const PUBLIC_NETWORKS: &[PublicNetwork] = &[
    PublicNetwork {
        id: "n-ethereum-mainnet",
        name: "mainnet",
        framework: FRAMEWORK_ETHEREUM,
        chain_id: "1",
        // Ethereum mainnet's first block, 2015-07-30T15:26:28Z.
        genesis_epoch_seconds: 1_438_269_988,
    },
    PublicNetwork {
        id: "n-ethereum-goerli",
        name: "goerli",
        framework: FRAMEWORK_ETHEREUM,
        chain_id: "5",
        // The Goerli testnet's genesis block, 2019-01-30T13:26:31Z.
        genesis_epoch_seconds: 1_548_854_791,
    },
];

/// The non-terminal status a freshly created network, member or node is reported in
/// (`NetworkStatus` / `MemberStatus` / `NodeStatus` all declare it).
pub const STATUS_CREATING: &str = "CREATING";

/// The settled status the clock owes a created network, member or node.
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `ProposalStatus` a freshly created proposal is reported in: "`IN_PROGRESS` - The
/// proposal is active and open for member voting" (`Proposal$Status`). It is NOT a
/// transition the clock owes a settled value — only member votes or the proposal's own
/// expiry close it.
pub const PROPOSAL_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `ProposalStatus` of a proposal whose expiration date passed before the votes that
/// would decide it were cast: "`EXPIRED` - Members didn't cast the number of votes
/// required to determine the proposal outcome before the proposal expired. The specified
/// `ProposalActions` aren't carried out" (`Proposal$Status`).
pub const PROPOSAL_EXPIRED: &str = "EXPIRED";

/// The `ProposalStatus` of a proposal the members approved: "`APPROVED` - The proposal
/// was approved with sufficient `YES` votes among members according to the `VotingPolicy`
/// specified for the `Network`. The specified proposal actions are carried out."
/// (`Proposal$Status`).
pub const PROPOSAL_APPROVED: &str = "APPROVED";

/// The `ProposalStatus` of a proposal the members turned down: "`REJECTED` - The proposal
/// was rejected with insufficient `YES` votes among members according to the
/// `VotingPolicy` specified for the `Network`. The specified `ProposalActions` aren't
/// carried out." (`Proposal$Status`).
pub const PROPOSAL_REJECTED: &str = "REJECTED";

/// The `ProposalStatus` of an approved proposal whose actions could not be carried out:
/// "`ACTION_FAILED` - One or more of the specified `ProposalActions` in a proposal that
/// was approved couldn't be completed because of an error. The `ACTION_FAILED` status
/// occurs even if only one ProposalAction fails and other actions are successful."
/// (`Proposal$Status`).
pub const PROPOSAL_ACTION_FAILED: &str = "ACTION_FAILED";

/// The `ThresholdComparator` under which the `YES` percentage "must be greater than the
/// `ThresholdPercentage` … to be approved".
pub const THRESHOLD_GREATER_THAN: &str = "GREATER_THAN";

/// The `ThresholdComparator` under which the `YES` percentage "must be greater than or
/// equal to the `ThresholdPercentage` to be approved".
pub const THRESHOLD_GREATER_THAN_OR_EQUAL_TO: &str = "GREATER_THAN_OR_EQUAL_TO";

/// The comparison applied when an `ApprovalThresholdPolicy` names no
/// `ThresholdComparator`.
///
/// `ThresholdComparator` is optional and the model fixes no default, so THIS choice is
/// the emulator's own: it is the comparison the documented worked example uses ("a
/// `ThresholdPercentage` value of `50` … along with a `ThresholdComparator` value of
/// `GREATER_THAN`, this indicates that 6 `YES` votes are required"), and it is a member of
/// `ThresholdComparator`, so the rule applied is always a modelled one.
pub const DEFAULT_THRESHOLD_COMPARATOR: &str = THRESHOLD_GREATER_THAN;

/// A `YES` vote on a proposal (`VoteValue` = `YES` | `NO`), counted into
/// `Proposal$YesVoteCount`.
pub const VOTE_YES: &str = "YES";

/// A `NO` vote on a proposal, counted into `Proposal$NoVoteCount`.
pub const VOTE_NO: &str = "NO";

/// The `InvitationStatus` a minted invitation starts in: "`PENDING` - The invitee hasn't
/// created a member to join the network, and the invitation hasn't yet expired"
/// (`Invitation$Status`).
pub const INVITATION_STATUS_PENDING: &str = "PENDING";

/// The `InvitationStatus` of an invitation whose invitee has begun creating the member it
/// grants: "`ACCEPTING` - The invitee has begun creating a member, and creation hasn't yet
/// completed" (`Invitation$Status`).
pub const INVITATION_STATUS_ACCEPTING: &str = "ACCEPTING";

/// The `InvitationStatus` the clock owes an accepted invitation once the member it granted
/// has finished creating: "`ACCEPTED` - The invitee created a member and joined the network
/// using the `InvitationID`" (`Invitation$Status`).
pub const INVITATION_STATUS_ACCEPTED: &str = "ACCEPTED";

/// The `InvitationStatus` of an invitation its invitee turned down: "`REJECTED` - The
/// invitee rejected the invitation" (`Invitation$Status`).
pub const INVITATION_STATUS_REJECTED: &str = "REJECTED";

/// The `InvitationStatus` the clock owes an invitation nobody acted on: "`EXPIRED` - The
/// invitee neither created a member nor rejected the invitation before the
/// `ExpirationDate`" (`Invitation$Status`).
pub const INVITATION_STATUS_EXPIRED: &str = "EXPIRED";

/// The proposal duration an invitation's expiry falls back on when the inviting network's
/// voting policy names no `ApprovalThresholdPolicy$ProposalDurationInHours`.
///
/// `Invitation$ExpirationDate` is documented as "the `CreationDate` plus the
/// `ProposalDurationInHours` that is specified in the `ProposalThresholdPolicy`", so the
/// span comes from the network's own policy wherever it names one. Nothing in this
/// service's model fixes a default for a policy that omits it, so THIS number is the
/// emulator's own choice; it sits inside `ProposalDurationInt`'s `@range(1, 168)`.
pub const DEFAULT_PROPOSAL_DURATION_IN_HOURS: i32 = 24;

/// `NodeConfiguration$StateDB`'s documented default: "When using an Amazon Managed
/// Blockchain network with Hyperledger Fabric version 1.4 or later, the default is
/// `CouchDB`."
pub const DEFAULT_STATE_DB: &str = "CouchDB";

/// `MemberConfiguration$KmsKeyArn`'s documented default: "Undefined or empty string -
/// By default, use an KMS key that is owned and managed by Amazon Web Services on your
/// behalf", which `Member$KmsKeyArn` reports as this sentinel value.
pub const AWS_OWNED_KMS_KEY: &str = "AWS Owned KMS Key";

/// The status a created accessor is reported in.
///
/// `AccessorStatus` declares only `AVAILABLE | PENDING_DELETION | DELETED` (no creating
/// state at all), so an accessor is ready the moment it is created and the clock owes it
/// no transition — unlike a network, member or node. The literal is the same text
/// [`STATUS_AVAILABLE`] carries, which is why it is defined in terms of it.
pub const ACCESSOR_STATUS_AVAILABLE: &str = STATUS_AVAILABLE;

/// The status a deleted accessor is reported in until its deletion finishes.
///
/// `DeleteAccessor` documents it: "After an accessor is deleted, the status of the
/// accessor changes from AVAILABLE to PENDING_DELETION. An accessor in the
/// PENDING_DELETION state can't be used for new WebSocket requests or HTTP requests."
/// It is one of `AccessorStatus`'s three values.
pub const ACCESSOR_STATUS_PENDING_DELETION: &str = "PENDING_DELETION";

/// The terminal `AccessorStatus`: the accessor's deletion has finished, so neither it
/// nor its billing token can be used any more.
pub const ACCESSOR_STATUS_DELETED: &str = "DELETED";

/// How long a deleted accessor stays `PENDING_DELETION` before the clock owes it
/// `DELETED`.
///
/// `PENDING_DELETION` is a non-terminal status `DeleteAccessor` puts the accessor in, so
/// reaching `DELETED` is a transition the clock owes. No AWS documentation fixes how long
/// it takes (the "up to 2 hours" in `DeleteAccessor`'s prose is how long WebSocket
/// connections opened earlier stay open, not how long the status takes to settle), so the
/// span is this emulator's own choice — the same one [`TRANSITION_SPAN`] makes, and
/// likewise deliberately non-zero, so a caller reading the accessor straight after the
/// delete still observes `PENDING_DELETION`. The instant it comes due is stored on the
/// record and materialised by [`StoredAccessor::settle`] — nothing sleeps.
pub const ACCESSOR_DELETION_SPAN: Duration = TRANSITION_SPAN;

/// The `AccessorNetworkType` an accessor takes when the create names none.
///
/// `CreateAccessor$NetworkType` is optional, and AWS's `CreateAccessor` reference states
/// that the default value of `ETHEREUM_MAINNET_AND_GOERLI` is applied when the
/// `CreateAccessor` action does not set a `networkType`. It is a member of
/// `AccessorNetworkType`, so a stored accessor reports a modelled value either way.
pub const DEFAULT_ACCESSOR_NETWORK_TYPE: &str = "ETHEREUM_MAINNET_AND_GOERLI";

/// How many characters an `AccessorBillingTokenString` has: its `@length(42, 42)` fixes
/// the length exactly.
pub const BILLING_TOKEN_LEN: usize = 42;

/// How many accessors this emulator will hold, for the same reason as [`MAX_NETWORKS`];
/// `CreateAccessor` models `ResourceLimitExceededException` for exceeding the account's
/// quota. The number is the emulator's own ceiling, not a fidelity claim about the
/// published quota.
pub const MAX_ACCESSORS: usize = 1_000;

/// `ListNetworks`'s `MaxResults` upper bound, from its `@range(1, 10)`. Also the page
/// size used when a caller names none.
pub const NETWORK_MAX_RESULTS: i32 = 10;

/// `ListNodes`'s `MaxResults` upper bound, from its `@range(1, 20)`. Also the page size
/// used when a caller names none.
pub const NODE_MAX_RESULTS: i32 = 20;

/// `ListProposals`'s `MaxResults` upper bound, from `ProposalListMaxResults`'s
/// `@range(1, 100)`. Also the page size used when a caller names none.
pub const PROPOSAL_MAX_RESULTS: i32 = 100;

/// `ListMembers`'s `MaxResults` upper bound, from its `@range(1, 20)`. Also the page size
/// used when a caller names none.
pub const MEMBER_MAX_RESULTS: i32 = 20;

/// `ListInvitations`'s `MaxResults` upper bound, from its `@range(1, 100)`. Also the page
/// size used when a caller names none.
pub const INVITATION_MAX_RESULTS: i32 = 100;

/// `ListAccessors`'s `MaxResults` upper bound, from `AccessorListMaxResults`'s
/// `@range(1, 50)`. Also the page size used when a caller names none.
pub const ACCESSOR_MAX_RESULTS: i32 = 50;

/// How many networks this emulator will hold.
///
/// The real service caps networks per account per Region with a service quota, and
/// `CreateNetwork` models `ResourceLimitExceededException` for exceeding it. The number
/// here is NOT a fidelity claim about the published quota: it is the emulator's own
/// ceiling, set generously so no realistic workflow meets it, so that a caller looping
/// on create cannot grow this process's memory without bound.
pub const MAX_NETWORKS: usize = 1_000;

/// How many nodes this emulator will hold on one network, for the same reason as
/// [`MAX_NETWORKS`]; `CreateNode` models `ResourceLimitExceededException` for it.
pub const MAX_NODES_PER_NETWORK: usize = 1_000;

/// How many members this emulator will hold on one network, for the same reason as
/// [`MAX_NETWORKS`]: a member record carries owned strings and a tag map, so the map a
/// create inserts into needs a ceiling of its own or a caller looping on member creates
/// grows this process's memory without bound. Enforced by
/// [`StoredNetwork::insert_member`], the only path that adds a member to a network. The
/// number is NOT a fidelity claim about the published per-network member quota, which the
/// real service refuses with the same `ResourceLimitExceededException` this ceiling
/// answers.
pub const MAX_MEMBERS_PER_NETWORK: usize = 1_000;

/// How many proposals this emulator will hold on one network.
///
/// `CreateProposal` models no limit error (the model declares no
/// `ResourceLimitExceededException` for it, unlike `CreateNetwork` and `CreateNode`), but
/// it does model `ThrottlingException`, whose own doc names the cause: "The most common
/// source of throttling errors is creating resources that exceed your service limit for
/// this resource type." That is the error `CreateProposal` raises at this ceiling. The
/// number is NOT a fidelity claim about any published quota: it is this emulator's own
/// ceiling, set generously so no realistic workflow meets it, so that a caller looping on
/// create cannot grow this process's memory without bound.
pub const MAX_PROPOSALS_PER_NETWORK: usize = 1_000;

/// How many invitations this emulator will hold, for the same reason as [`MAX_NETWORKS`]:
/// an invitation record is minted per approved `InviteAction` and outlives the proposal
/// that minted it, so the store needs a ceiling of its own.
pub const MAX_INVITATIONS: usize = 1_000;

/// How many `ClientRequestToken` records the idempotency ledger keeps.
///
/// The ledger grows by one entry per create with a token the service has not seen, and a
/// token is caller-chosen, so the store needs a ceiling of its own: the resource caps
/// above do not bound it (`DeleteNode` frees a node slot without freeing the token that
/// minted it). At this many records the oldest is evicted — see
/// [`ComAmazonawsManagedblockchainTaigawebserviceState::remember_idempotent`]. The
/// number is the emulator's own, set well above any realistic workflow's token count.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 4_096;

/// The ids a create minted, replayed when the same `ClientRequestToken` comes back.
#[derive(Debug, Clone, Default)]
pub struct CreatedIds {
    pub network_id: Option<String>,
    pub member_id: Option<String>,
    pub node_id: Option<String>,
    pub proposal_id: Option<String>,
    pub accessor_id: Option<String>,
}

/// The idempotency key of one `@idempotencyToken` call: the token is scoped to the
/// operation that minted it, so reusing a network token on `CreateNode` is a new call.
pub fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}/{token}")
}

/// One stored network, with the members that belong to it and the nodes that run on it.
#[derive(Debug, Clone)]
pub struct StoredNetwork {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub framework: String,
    /// `Network$FrameworkVersion`, as the create named it. Absent on an AWS-owned public
    /// network: no request named one, and which client version the service reports for a
    /// public chain is not documented, so reporting one would invent a value.
    pub framework_version: Option<String>,
    /// `NetworkFabricAttributes$Edition`, as the create's `FrameworkConfiguration` set
    /// it; absent when the request configured no framework properties.
    pub fabric_edition: Option<String>,
    /// `NetworkFabricAttributes$OrderingServiceEndpoint`, minted at create time; `None`
    /// on a network of a framework that has no ordering service.
    pub ordering_service_endpoint: Option<String>,
    /// `NetworkEthereumAttributes$ChainId`, on an Ethereum network only.
    pub ethereum_chain_id: Option<String>,
    /// Whether this is one of AWS's standing, AWS-owned public networks
    /// ([`PUBLIC_NETWORKS`]) rather than a network this account created.
    pub aws_owned: bool,
    /// `Network$VotingPolicy`, as the create named it. `None` on a network of a framework
    /// that has no member voting (`VotingPolicy` "Applies only to Hyperledger Fabric").
    pub voting_policy: Option<VotingPolicy>,
    /// A `NetworkStatus` value.
    pub status: String,
    /// When `CREATING` -> `AVAILABLE` falls due. See [`TRANSITION_SPAN`].
    pub available_at: SystemTime,
    pub creation_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub arn: String,
    pub vpc_endpoint_service_name: String,
    /// The members of this network, keyed by member id.
    pub members: BTreeMap<String, StoredMember>,
    /// The nodes on this network, keyed by node id.
    pub nodes: BTreeMap<String, StoredNode>,
    /// The proposals made on this network, keyed by proposal id — which also orders the
    /// listings, so a page cursor keys on the ordering itself.
    pub proposals: BTreeMap<String, StoredProposal>,
}

/// A member could not be added: the network already holds [`MAX_MEMBERS_PER_NETWORK`] of
/// them. A handler answers it with the modelled `ResourceLimitExceededException`.
#[derive(Debug, Clone, Copy)]
pub struct MemberLimitExceeded;

impl StoredNetwork {
    /// Add `member` to this network, refusing once the network already holds
    /// [`MAX_MEMBERS_PER_NETWORK`] of them.
    ///
    /// Every path that adds a member goes through here — `CreateNetwork`'s first member
    /// today, and whatever later implements `CreateMember` — so the member map cannot
    /// grow without bound however many creates a caller loops on. Replacing a member id
    /// the network already holds is not growth, so it is always allowed.
    pub fn insert_member(&mut self, member: StoredMember) -> Result<(), MemberLimitExceeded> {
        if !self.members.contains_key(&member.id) && self.members.len() >= MAX_MEMBERS_PER_NETWORK
        {
            return Err(MemberLimitExceeded);
        }
        self.members.insert(member.id.clone(), member);
        Ok(())
    }

    /// Materialise the status transitions the clock already owes this network and
    /// everything on it.
    ///
    /// This is the ONLY write a reporting operation makes: each change was owed at this
    /// instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_CREATING && now >= self.available_at {
            self.status = STATUS_AVAILABLE.to_string();
        }
        for member in self.members.values_mut() {
            if member.status == STATUS_CREATING && now >= member.available_at {
                member.status = STATUS_AVAILABLE.to_string();
            }
        }
        for node in self.nodes.values_mut() {
            if node.status == STATUS_CREATING && now >= node.available_at {
                node.status = STATUS_AVAILABLE.to_string();
            }
        }
        for proposal in self.proposals.values_mut() {
            proposal.settle(now);
        }
    }

    /// Whether the current account participates in this network, which is what
    /// `ListNetworks` reports ("the networks in which the current Amazon Web Services
    /// account participates").
    ///
    /// Every network the account created counts; one of AWS's standing public networks
    /// counts only once the account holds a member or a node on it, because until then
    /// the account merely *can read* it.
    pub fn account_participates(&self) -> bool {
        !self.aws_owned || !self.members.is_empty() || !self.nodes.is_empty()
    }

    /// The modelled `NetworkFrameworkAttributes` for this record: the framework's own
    /// attributes, and nothing for a framework this record carries none for.
    fn to_framework_attributes(&self) -> NetworkFrameworkAttributes {
        NetworkFrameworkAttributes {
            fabric: (self.ordering_service_endpoint.is_some() || self.fabric_edition.is_some())
                .then(|| NetworkFabricAttributes {
                    ordering_service_endpoint: self.ordering_service_endpoint.clone(),
                    edition: self.fabric_edition.clone(),
                }),
            ethereum: self
                .ethereum_chain_id
                .clone()
                .map(|chain_id| NetworkEthereumAttributes {
                    chain_id: Some(chain_id),
                }),
        }
    }

    /// The modelled `Network` for this record, as `GetNetwork` returns it.
    pub fn to_network(&self) -> Network {
        Network {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            framework: Some(self.framework.clone()),
            framework_version: self.framework_version.clone(),
            framework_attributes: Some(self.to_framework_attributes()),
            vpc_endpoint_service_name: Some(self.vpc_endpoint_service_name.clone()),
            voting_policy: self.voting_policy.clone(),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            tags: Some(map_out(&self.tags)),
            arn: Some(self.arn.clone()),
        }
    }

    /// The modelled `NetworkSummary` for this record, as `ListNetworks` returns it.
    pub fn to_summary(&self) -> NetworkSummary {
        NetworkSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            framework: Some(self.framework.clone()),
            framework_version: self.framework_version.clone(),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            arn: Some(self.arn.clone()),
        }
    }
}

/// One stored member of a network: the first one comes from `CreateNetwork`'s
/// `MemberConfiguration`.
#[derive(Debug, Clone)]
pub struct StoredMember {
    pub id: String,
    pub network_id: String,
    pub name: String,
    pub description: Option<String>,
    /// `MemberFabricAttributes$AdminUsername`, as `MemberFabricConfiguration` set it.
    /// The admin password is never stored: nothing may report it back.
    pub admin_username: Option<String>,
    /// `MemberFabricAttributes$CaEndpoint`, minted at create time.
    pub ca_endpoint: String,
    pub log_publishing_configuration: Option<MemberLogPublishingConfiguration>,
    /// A `MemberStatus` value.
    pub status: String,
    /// When `CREATING` -> `AVAILABLE` falls due. See [`TRANSITION_SPAN`].
    pub available_at: SystemTime,
    pub creation_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub arn: String,
    /// The key the member encrypts at rest with, which its nodes inherit.
    pub kms_key_arn: String,
}

impl StoredMember {
    /// The modelled `Member` for this record, as `GetMember` returns it.
    pub fn to_member(&self) -> Member {
        Member {
            network_id: Some(self.network_id.clone()),
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            framework_attributes: Some(MemberFrameworkAttributes {
                fabric: Some(MemberFabricAttributes {
                    admin_username: self.admin_username.clone(),
                    ca_endpoint: Some(self.ca_endpoint.clone()),
                }),
            }),
            log_publishing_configuration: self.log_publishing_configuration.clone(),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            tags: Some(map_out(&self.tags)),
            arn: Some(self.arn.clone()),
            kms_key_arn: Some(self.kms_key_arn.clone()),
        }
    }

    /// Whether this member belongs to the account under emulation
    /// (`MemberSummary$IsOwned`: "whether the member is owned by your Amazon Web Services
    /// account or a different Amazon Web Services account").
    ///
    /// Read off the account segment of the member's STORED ARN, which the call that
    /// created the member minted once: that ARN is this emulator's record of which
    /// account holds the member, so a member ever stored for another account reports
    /// `false` without this needing to change.
    pub fn is_owned(&self) -> bool {
        arn_account_id(&self.arn) == Some(::engine::emulator_account_id())
    }

    /// The modelled `MemberSummary` for this record, as `ListMembers` returns it.
    pub fn to_summary(&self) -> MemberSummary {
        MemberSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            is_owned: Some(self.is_owned()),
            arn: Some(self.arn.clone()),
        }
    }
}

/// The endpoints a node exposes, which differ by the framework of the network it runs on:
/// a Hyperledger Fabric node is reached through its peer endpoints
/// (`NodeFabricAttributes`), an Ethereum node through its JSON-RPC endpoints
/// (`NodeEthereumAttributes`). Both are minted once, at create time, and stored.
#[derive(Debug, Clone)]
pub enum StoredNodeEndpoints {
    /// `NodeFabricAttributes$PeerEndpoint` and `$PeerEventEndpoint`.
    Fabric {
        peer_endpoint: String,
        peer_event_endpoint: String,
    },
    /// `NodeEthereumAttributes$HttpEndpoint` ("the endpoint on which the Ethereum node
    /// listens to run Ethereum API methods over HTTP connections") and
    /// `$WebSocketEndpoint`.
    Ethereum {
        http_endpoint: String,
        web_socket_endpoint: String,
    },
}

/// One stored node of a network.
#[derive(Debug, Clone)]
pub struct StoredNode {
    pub id: String,
    pub network_id: String,
    /// The member that owns the node; `None` only on a framework that has no members.
    pub member_id: Option<String>,
    pub instance_type: String,
    /// The Availability Zone the create named, or `None` when it named none: which zone
    /// the service would have picked is EC2 state this emulator does not hold.
    pub availability_zone: Option<String>,
    /// The endpoints this node is reached on, minted at create time for the framework of
    /// the network it runs on.
    pub endpoints: StoredNodeEndpoints,
    pub log_publishing_configuration: Option<NodeLogPublishingConfiguration>,
    /// A `StateDBType` value; `None` on a framework that has no state database
    /// (`NodeConfiguration$StateDB` "Applies only to Hyperledger Fabric").
    pub state_db: Option<String>,
    /// A `NodeStatus` value.
    pub status: String,
    /// When `CREATING` -> `AVAILABLE` falls due. See [`TRANSITION_SPAN`].
    pub available_at: SystemTime,
    pub creation_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub arn: String,
    /// Inherited from the owning member: "The node inherits this parameter from the
    /// member that it belongs to" (`Node$KmsKeyArn`).
    pub kms_key_arn: Option<String>,
}

impl StoredNode {
    /// The modelled `NodeFrameworkAttributes` for this record: the attributes of the
    /// framework whose endpoints it was minted with, and nothing for the other.
    fn to_framework_attributes(&self) -> NodeFrameworkAttributes {
        match &self.endpoints {
            StoredNodeEndpoints::Fabric {
                peer_endpoint,
                peer_event_endpoint,
            } => NodeFrameworkAttributes {
                fabric: Some(NodeFabricAttributes {
                    peer_endpoint: Some(peer_endpoint.clone()),
                    peer_event_endpoint: Some(peer_event_endpoint.clone()),
                }),
                ethereum: None,
            },
            StoredNodeEndpoints::Ethereum {
                http_endpoint,
                web_socket_endpoint,
            } => NodeFrameworkAttributes {
                fabric: None,
                ethereum: Some(NodeEthereumAttributes {
                    http_endpoint: Some(http_endpoint.clone()),
                    web_socket_endpoint: Some(web_socket_endpoint.clone()),
                }),
            },
        }
    }

    /// The modelled `Node` for this record, as `GetNode` returns it.
    pub fn to_node(&self) -> Node {
        Node {
            network_id: Some(self.network_id.clone()),
            member_id: self.member_id.clone(),
            id: Some(self.id.clone()),
            instance_type: Some(self.instance_type.clone()),
            availability_zone: self.availability_zone.clone(),
            framework_attributes: Some(self.to_framework_attributes()),
            log_publishing_configuration: self.log_publishing_configuration.clone(),
            state_db: self.state_db.clone(),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            tags: Some(map_out(&self.tags)),
            arn: Some(self.arn.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
        }
    }

    /// The modelled `NodeSummary` for this record, as `ListNodes` returns it.
    pub fn to_summary(&self) -> NodeSummary {
        NodeSummary {
            id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            availability_zone: self.availability_zone.clone(),
            instance_type: Some(self.instance_type.clone()),
            arn: Some(self.arn.clone()),
        }
    }
}

/// One stored proposal on a network: a change to the network that the network's members
/// vote on, as `CreateProposal` recorded it.
#[derive(Debug, Clone)]
pub struct StoredProposal {
    pub id: String,
    pub network_id: String,
    pub description: Option<String>,
    /// "The actions to perform on the network if the proposal is `APPROVED`"
    /// (`Proposal$Actions`), stored exactly as the create named them.
    pub actions: ProposalActions,
    /// The member that created the proposal; its name is read from the member record, so
    /// a rename is reflected rather than frozen into the proposal.
    pub proposed_by_member_id: String,
    /// A `ProposalStatus` value.
    pub status: String,
    pub creation_date: SystemTime,
    /// "This is the `CreationDate` plus the `ProposalDurationInHours` that is specified in
    /// the `ProposalThresholdPolicy`" (`Proposal$ExpirationDate`); `None` when the
    /// network's voting policy names no duration, which fixes no expiry to report.
    pub expiration_date: Option<SystemTime>,
    /// The `VoteValue` each member cast, keyed by the voting member's id. The vote counts
    /// are COUNTED from this map rather than kept as separate totals, so no counter can
    /// drift from the votes actually cast.
    pub votes: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub arn: String,
}

impl StoredProposal {
    /// Materialise the expiry the clock already owes this proposal.
    ///
    /// Like [`StoredNetwork::settle`] this is the only write a reporting operation makes:
    /// the change was owed at this instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) {
        if self.status == PROPOSAL_IN_PROGRESS
            && self.expiration_date.is_some_and(|expires| now >= expires)
        {
            self.status = PROPOSAL_EXPIRED.to_string();
        }
    }

    /// How many members cast `vote` on this proposal.
    pub fn vote_count(&self, vote: &str) -> i32 {
        i32::try_from(self.votes.values().filter(|cast| *cast == vote).count()).unwrap_or(i32::MAX)
    }

    /// Whether this proposal is still "active and open for member voting"
    /// (`Proposal$Status`'s `IN_PROGRESS`) — the only status under which a vote can be
    /// cast on it. Every other status is terminal: approved, rejected, expired, or
    /// approved with actions that failed.
    pub fn is_open(&self) -> bool {
        self.status == PROPOSAL_IN_PROGRESS
    }

    /// The modelled `Proposal` for this record, as `GetProposal` returns it.
    ///
    /// `network` is the network this proposal belongs to: the proposing member's name and
    /// the outstanding-vote count are read from its current membership, which is what
    /// `Proposal$OutstandingVoteCount` describes — "the number of members minus the sum of
    /// `YES` votes and `NO` votes".
    pub fn to_proposal(&self, network: &StoredNetwork) -> Proposal {
        let members = i32::try_from(network.members.len()).unwrap_or(i32::MAX);
        let cast = i32::try_from(self.votes.len()).unwrap_or(i32::MAX);
        Proposal {
            proposal_id: Some(self.id.clone()),
            network_id: Some(self.network_id.clone()),
            description: self.description.clone(),
            actions: Some(self.actions.clone()),
            proposed_by_member_id: Some(self.proposed_by_member_id.clone()),
            proposed_by_member_name: network
                .members
                .get(&self.proposed_by_member_id)
                .map(|member| member.name.clone()),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            expiration_date: self.expiration_date.map(::aws_smithy_types::DateTime::from),
            yes_vote_count: Some(self.vote_count(VOTE_YES)),
            no_vote_count: Some(self.vote_count(VOTE_NO)),
            outstanding_vote_count: Some((members - cast).max(0)),
            tags: Some(map_out(&self.tags)),
            arn: Some(self.arn.clone()),
        }
    }

    /// The modelled `ProposalSummary` for this record, as `ListProposals` returns it.
    ///
    /// `network` is the network this proposal belongs to, for the same reason as in
    /// [`Self::to_proposal`]: the proposing member's name is read from its current
    /// membership rather than frozen into the proposal.
    pub fn to_summary(&self, network: &StoredNetwork) -> ProposalSummary {
        ProposalSummary {
            proposal_id: Some(self.id.clone()),
            description: self.description.clone(),
            proposed_by_member_id: Some(self.proposed_by_member_id.clone()),
            proposed_by_member_name: network
                .members
                .get(&self.proposed_by_member_id)
                .map(|member| member.name.clone()),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            expiration_date: self.expiration_date.map(::aws_smithy_types::DateTime::from),
            arn: Some(self.arn.clone()),
        }
    }
}

/// One stored accessor: the token-based access object `CreateAccessor` mints.
///
/// An accessor belongs to the account rather than to a network — `Accessor` names no
/// network id — so the records live beside [`StoredNetwork`], not inside one.
#[derive(Debug, Clone)]
pub struct StoredAccessor {
    pub id: String,
    /// An `AccessorType` value, as the create named it.
    pub accessor_type: String,
    /// `Accessor$BillingToken`, minted at create time and stored, so every read of the
    /// accessor reports the same token.
    pub billing_token: String,
    /// An `AccessorStatus` value.
    pub status: String,
    pub creation_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub arn: String,
    /// An `AccessorNetworkType` value: what the create named, or
    /// [`DEFAULT_ACCESSOR_NETWORK_TYPE`] when it named none.
    pub network_type: String,
    /// When `PENDING_DELETION` -> `DELETED` falls due. `None` until `DeleteAccessor`
    /// starts the deletion. See [`ACCESSOR_DELETION_SPAN`].
    pub deleted_at: Option<SystemTime>,
}

impl StoredAccessor {
    /// Start this accessor's deletion, as `DeleteAccessor` does: "the status of the
    /// accessor changes from AVAILABLE to PENDING_DELETION", and `DELETED` falls due
    /// [`ACCESSOR_DELETION_SPAN`] later.
    ///
    /// An accessor already `PENDING_DELETION` or `DELETED` has nothing left to start, so
    /// a repeated delete leaves the status and the due instant exactly as the first one
    /// set them.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.status != ACCESSOR_STATUS_AVAILABLE {
            return;
        }
        self.status = ACCESSOR_STATUS_PENDING_DELETION.to_string();
        self.deleted_at = Some(now + ACCESSOR_DELETION_SPAN);
    }

    /// Materialise the status transition the clock already owes this accessor: a deletion
    /// that has run its course reports `DELETED`.
    ///
    /// Like [`StoredNetwork::settle`] this is the only write a reporting operation makes:
    /// the change was owed at this instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) {
        if self.status == ACCESSOR_STATUS_PENDING_DELETION
            && self.deleted_at.is_some_and(|deleted_at| now >= deleted_at)
        {
            self.status = ACCESSOR_STATUS_DELETED.to_string();
        }
    }

    /// The modelled `Accessor` for this record, as `GetAccessor` returns it.
    pub fn to_accessor(&self) -> Accessor {
        Accessor {
            id: Some(self.id.clone()),
            r#type: Some(self.accessor_type.clone()),
            billing_token: Some(self.billing_token.clone()),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            arn: Some(self.arn.clone()),
            tags: Some(map_out(&self.tags)),
            network_type: Some(self.network_type.clone()),
        }
    }

    /// The modelled `AccessorSummary` for this record, as `ListAccessors` reports it.
    ///
    /// Projected from the SAME stored record [`Self::to_accessor`] answers from, so a
    /// listing and a `GetAccessor` never disagree; `AccessorSummary` declares no
    /// `BillingToken` or `Tags`, so the summary carries the members the model lists and
    /// no more.
    pub fn to_summary(&self) -> AccessorSummary {
        AccessorSummary {
            id: Some(self.id.clone()),
            r#type: Some(self.accessor_type.clone()),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            arn: Some(self.arn.clone()),
            network_type: Some(self.network_type.clone()),
        }
    }
}

/// One invitation an approved `InviteAction` minted: "An invitation to an Amazon Web
/// Services account to create a member and join the network."
#[derive(Debug, Clone)]
pub struct StoredInvitation {
    pub id: String,
    /// The account the `InviteAction` named (`InviteAction$Principal`). `ListInvitations`
    /// reports "all invitations for the current Amazon Web Services account", so this is
    /// what a listing selects on.
    pub principal: String,
    /// The network the invitee may join; its [`StoredNetwork::to_summary`] is what
    /// `Invitation$NetworkSummary` reports.
    pub network_id: String,
    /// An `InvitationStatus` value.
    pub status: String,
    pub creation_date: SystemTime,
    /// `Invitation$ExpirationDate`: "This is the `CreationDate` plus the
    /// `ProposalDurationInHours` that is specified in the `ProposalThresholdPolicy`."
    /// When it passes, a still-`PENDING` invitation is owed `EXPIRED` — see
    /// [`ComAmazonawsManagedblockchainTaigawebserviceState::settle`].
    pub expiration_date: SystemTime,
    /// When `ACCEPTING` -> `ACCEPTED` falls due: the instant the member the invitee
    /// created with this invitation finishes creating. `None` until `CreateMember`
    /// accepts it.
    pub accepted_at: Option<SystemTime>,
    pub arn: String,
}

impl StoredInvitation {
    /// The modelled `Invitation` for this record, as `ListInvitations` returns it.
    ///
    /// `network` is the stored network this invitation is onto, which the caller looks up
    /// by [`Self::network_id`]; `None` when nothing holds it any more, in which case there
    /// is no summary to report.
    pub fn to_invitation(&self, network: Option<&StoredNetwork>) -> Invitation {
        Invitation {
            invitation_id: Some(self.id.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            expiration_date: Some(::aws_smithy_types::DateTime::from(self.expiration_date)),
            status: Some(self.status.clone()),
            network_summary: network.map(StoredNetwork::to_summary),
            arn: Some(self.arn.clone()),
        }
    }

    /// Materialise the status transitions the clock already owes this invitation: one
    /// nobody acted on expires on its `ExpirationDate`, and one whose member has finished
    /// creating is `ACCEPTED`.
    fn settle(&mut self, now: SystemTime) {
        if self.status == INVITATION_STATUS_PENDING && now >= self.expiration_date {
            self.status = INVITATION_STATUS_EXPIRED.to_string();
        }
        if self.status == INVITATION_STATUS_ACCEPTING
            && self.accepted_at.is_some_and(|accepted_at| now >= accepted_at)
        {
            self.status = INVITATION_STATUS_ACCEPTED.to_string();
        }
    }

    /// Whether this invitation can still be used to create a member: only a `PENDING` one
    /// can, since after its `ExpirationDate` "the invitee can no longer create a member
    /// and join the network using this `InvitationId`", and a rejected or already accepted
    /// invitation is spent.
    pub fn is_usable(&self) -> bool {
        self.status == INVITATION_STATUS_PENDING
    }
}

/// Per-service state for com-amazonaws-managedblockchain-taigawebservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsManagedblockchainTaigawebserviceState {
    /// Every network this store holds — the ones this account created and AWS's standing
    /// public ones — keyed by id, which also orders the listings, so a page cursor keys
    /// on the ordering itself.
    pub networks: BTreeMap<String, StoredNetwork>,
    /// Every created accessor, keyed by its minted id — which also orders the listings,
    /// so a page cursor keys on the ordering itself.
    pub accessors: BTreeMap<String, StoredAccessor>,
    /// Every invitation this account holds, keyed by its minted id — which, as for the
    /// networks above, is also the listing order a page cursor keys on.
    pub invitations: BTreeMap<String, StoredInvitation>,
    /// The ids each `ClientRequestToken` already minted, so a repeated token replays its
    /// original result instead of creating a second resource.
    ///
    /// Private, together with [`Self::idempotency_order`], because the two are one
    /// bounded store: every read and write goes through [`Self::replay_idempotent`],
    /// [`Self::remember_idempotent`] and [`Self::forget_node_token`], which keep the
    /// count at or under [`MAX_IDEMPOTENCY_RECORDS`] and keep the two in step.
    idempotency: BTreeMap<String, CreatedIds>,
    /// The keys of `idempotency` in insertion order, oldest first: the eviction order
    /// when the ledger is at its ceiling.
    idempotency_order: ::std::collections::VecDeque<String>,
    /// Advanced once per minted id, so no id is ever a function of the request.
    id_seed: u64,
}

impl Default for ComAmazonawsManagedblockchainTaigawebserviceState {
    /// A fresh store already holds AWS's standing, AWS-owned public networks.
    ///
    /// They are not account resources and no call creates them: "AMB Access exposes
    /// standing public blockchain networks that every account can read", under the same
    /// ids in every account. Materialising them here — rather than on first read — is
    /// what keeps every read of them a pure read.
    ///
    /// Their ARNs and VPC endpoint service names name [`DEFAULT_REGION`]: nothing minted
    /// them for a request, so there is no credential scope to read a Region from.
    fn default() -> Self {
        let mut state = Self {
            networks: BTreeMap::new(),
            accessors: BTreeMap::new(),
            invitations: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            idempotency_order: ::std::collections::VecDeque::new(),
            id_seed: 0,
        };
        for public in PUBLIC_NETWORKS {
            let genesis = SystemTime::UNIX_EPOCH + Duration::from_secs(public.genesis_epoch_seconds);
            state.networks.insert(
                public.id.to_string(),
                StoredNetwork {
                    id: public.id.to_string(),
                    name: public.name.to_string(),
                    description: None,
                    framework: public.framework.to_string(),
                    framework_version: None,
                    fabric_edition: None,
                    ordering_service_endpoint: None,
                    ethereum_chain_id: Some(public.chain_id.to_string()),
                    aws_owned: true,
                    voting_policy: None,
                    // A public network is standing and ready: AWS reports it `AVAILABLE`,
                    // so the clock owes it no transition.
                    status: STATUS_AVAILABLE.to_string(),
                    available_at: genesis,
                    creation_date: genesis,
                    tags: BTreeMap::new(),
                    arn: network_arn(DEFAULT_REGION, public.id),
                    vpc_endpoint_service_name: vpc_endpoint_service_name(DEFAULT_REGION, public.id),
                    members: BTreeMap::new(),
                    nodes: BTreeMap::new(),
                    proposals: BTreeMap::new(),
                },
            );
        }
        state
    }
}

impl ComAmazonawsManagedblockchainTaigawebserviceState {
    /// Materialise every status transition the clock already owes. Called at the top of
    /// each handler, reporting ones included: they may write only a change that was
    /// already due at this instant.
    pub fn settle(&mut self, now: SystemTime) {
        for network in self.networks.values_mut() {
            network.settle(now);
        }
        // An invitation nobody acted on before its `ExpirationDate` is `EXPIRED`, and one
        // whose member has finished creating is `ACCEPTED` — transitions the CLOCK owes,
        // due at that instant whether or not anybody calls.
        for invitation in self.invitations.values_mut() {
            invitation.settle(now);
        }
        // An accessor whose deletion has run its course is `DELETED` — again a
        // transition the CLOCK owes, due at that instant whether or not anybody calls.
        for accessor in self.accessors.values_mut() {
            accessor.settle(now);
        }
    }

    /// The tags held by the resource `arn` names, or `None` when no stored resource
    /// carries that ARN.
    ///
    /// Resolution is by the ARN each record was minted with and STORED, never by
    /// rebuilding one from the request: an ARN naming another account's or another
    /// Region's resource therefore names nothing this store holds, which is the same
    /// answer as naming a resource that was never created. Every taggable resource is
    /// reachable here — the account's accessors, and each network with the members, nodes
    /// and proposals that belong to it — so one tag set is read and written through this
    /// one lookup rather than a copy per operation.
    pub fn tags_for_resource(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(accessor) = self.accessors.values().find(|accessor| accessor.arn == arn) {
            return Some(&accessor.tags);
        }
        for network in self.networks.values() {
            if network.arn == arn {
                return Some(&network.tags);
            }
            if let Some(member) = network.members.values().find(|member| member.arn == arn) {
                return Some(&member.tags);
            }
            if let Some(node) = network.nodes.values().find(|node| node.arn == arn) {
                return Some(&node.tags);
            }
            if let Some(proposal) = network
                .proposals
                .values()
                .find(|proposal| proposal.arn == arn)
            {
                return Some(&proposal.tags);
            }
        }
        None
    }

    /// The tag set of the resource `arn` names, to write: the same resolution
    /// [`Self::tags_for_resource`] reads through, so a tag written here is the one a read
    /// of that resource reports.
    pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(accessor) = self
            .accessors
            .values_mut()
            .find(|accessor| accessor.arn == arn)
        {
            return Some(&mut accessor.tags);
        }
        for network in self.networks.values_mut() {
            if network.arn == arn {
                return Some(&mut network.tags);
            }
            if let Some(member) = network
                .members
                .values_mut()
                .find(|member| member.arn == arn)
            {
                return Some(&mut member.tags);
            }
            if let Some(node) = network.nodes.values_mut().find(|node| node.arn == arn) {
                return Some(&mut node.tags);
            }
            if let Some(proposal) = network
                .proposals
                .values_mut()
                .find(|proposal| proposal.arn == arn)
            {
                return Some(&mut proposal.tags);
            }
        }
        None
    }

    /// What the create under this idempotency key already minted, if the ledger still
    /// holds it.
    pub fn replay_idempotent(&self, key: &str) -> Option<CreatedIds> {
        self.idempotency.get(key).cloned()
    }

    /// Record the ids a create minted under its idempotency key.
    ///
    /// The ledger is a BOUNDED store: at [`MAX_IDEMPOTENCY_RECORDS`] entries the oldest
    /// record is evicted to make room, so a caller looping on creates with fresh
    /// caller-chosen tokens cannot grow this process's memory without limit. An evicted
    /// token is simply no longer remembered — replaying it creates a new resource, which
    /// is the same answer the real service gives once a token has aged out.
    pub fn remember_idempotent(&mut self, key: String, created: CreatedIds) {
        if let ::std::collections::btree_map::Entry::Occupied(mut existing) =
            self.idempotency.entry(key.clone())
        {
            // Already recorded (and so already in the eviction queue): overwrite in
            // place rather than queueing the key twice.
            existing.insert(created);
            return;
        }
        while self.idempotency.len() >= MAX_IDEMPOTENCY_RECORDS {
            let Some(oldest) = self.idempotency_order.pop_front() else {
                break;
            };
            self.idempotency.remove(&oldest);
        }
        self.idempotency_order.push_back(key.clone());
        self.idempotency.insert(key, created);
    }

    /// Forget the record of the create that minted `node_id`.
    ///
    /// Called when the node is deleted: its data "is lost and cannot be recovered", so
    /// replaying that token could only hand back an id nothing holds. It also keeps the
    /// ledger from outliving the resources it describes, so a create/delete loop frees
    /// the token record along with the node.
    pub fn forget_node_token(&mut self, node_id: &str) {
        let stale: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| created.node_id.as_deref() == Some(node_id))
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            self.idempotency.remove(&key);
            self.idempotency_order.retain(|queued| queued != &key);
        }
    }

    /// Mint a fresh resource id: `prefix`, a hyphen, then 26 characters of the base-32
    /// alphabet, the shape every Managed Blockchain id takes (`n-` for a network, `m-`
    /// for a member, `nd-` for a node, `i-` for an invitation), and within
    /// `ResourceIdString`'s `@length(1, 32)`.
    ///
    /// Drawn from a counter the state advances (seeded from the clock on first use),
    /// never from the request, so two identical creates mint two different ids.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        let high = self.advance();
        let low = self.advance();
        let mut id = String::from(prefix);
        id.push('-');
        id.push_str(&base32_13(high));
        id.push_str(&base32_13(low));
        id
    }

    /// Mint a network id no stored network holds.
    pub fn mint_network_id(&mut self) -> String {
        loop {
            let id = self.mint_id("n");
            if !self.networks.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint a proposal id no proposal on `network_id` holds.
    pub fn mint_proposal_id(&mut self, network_id: &str) -> String {
        loop {
            let id = self.mint_id("p");
            let taken = self
                .networks
                .get(network_id)
                .is_some_and(|network| network.proposals.contains_key(&id));
            if !taken {
                return id;
            }
        }
    }

    /// Mint an accessor id no stored accessor holds (`ac-` is the prefix Managed
    /// Blockchain writes an accessor id with).
    pub fn mint_accessor_id(&mut self) -> String {
        loop {
            let id = self.mint_id("ac");
            if !self.accessors.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an invitation id no stored invitation holds.
    pub fn mint_invitation_id(&mut self) -> String {
        loop {
            let id = self.mint_id("i");
            if !self.invitations.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint a billing token no stored accessor holds: exactly [`BILLING_TOKEN_LEN`]
    /// characters, as `AccessorBillingTokenString`'s `@length(42, 42)` requires.
    ///
    /// Drawn from the same counter the ids come from, never from the request, so two
    /// accessors never share a token and no token is a function of the caller's input.
    pub fn mint_billing_token(&mut self) -> String {
        loop {
            let mut token = String::with_capacity(BILLING_TOKEN_LEN);
            while token.len() < BILLING_TOKEN_LEN {
                token.push_str(&base32_13(self.advance()));
            }
            // Every character is one ASCII byte of the base-32 alphabet, so this cuts
            // the token to exactly `BILLING_TOKEN_LEN` characters.
            token.truncate(BILLING_TOKEN_LEN);
            if !self
                .accessors
                .values()
                .any(|accessor| accessor.billing_token == token)
            {
                return token;
            }
        }
    }

    /// Advance the id counter and hand back its new value. Seeded from the clock on
    /// first use, so no minted value is ever a function of the request.
    fn advance(&mut self) -> u64 {
        if self.id_seed == 0 {
            self.id_seed = now_nanos();
        }
        self.id_seed = splitmix64(self.id_seed);
        self.id_seed
    }
    /// Record the invitation an `InviteAction` owes `principal` once its proposal is
    /// approved: "The `InviteAction` is carried out when a `Proposal` is `APPROVED`"
    /// (`InviteAction`). This is where an invitation enters the store, and the only
    /// place: `ListInvitations` and `RejectInvitation` report and settle what is here.
    ///
    /// The record expires at `now` plus the inviting network's
    /// `ApprovalThresholdPolicy$ProposalDurationInHours` (defaulting to
    /// [`DEFAULT_PROPOSAL_DURATION_IN_HOURS`]), as `Invitation$ExpirationDate` documents.
    ///
    /// Answers the minted invitation id, or `None` when `network_id` names no stored
    /// network (there is nothing to be invited onto) or the store is already holding
    /// [`MAX_INVITATIONS`] records.
    pub fn record_invitation(
        &mut self,
        network_id: &str,
        principal: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<String> {
        if self.invitations.len() >= MAX_INVITATIONS {
            return None;
        }
        let duration = self
            .networks
            .get(network_id)?
            .voting_policy
            .as_ref()
            .and_then(proposal_duration)
            // Both `Invitation$ExpirationDate` and `Proposal$ExpirationDate` are the
            // creation date plus the policy's `ProposalDurationInHours`, so both read it
            // through the one helper. Where they part is a policy that names NO duration:
            // an invitation must still report an expiry, so it falls back to
            // [`DEFAULT_PROPOSAL_DURATION_IN_HOURS`], while `Proposal$ExpirationDate` is
            // optional and is simply not reported (see `proposal_duration`).
            .unwrap_or_else(|| {
                Duration::from_secs(DEFAULT_PROPOSAL_DURATION_IN_HOURS as u64 * 3_600)
            });
        let id = self.mint_invitation_id();
        let invitation = StoredInvitation {
            arn: invitation_arn(region, &id),
            id: id.clone(),
            principal: principal.to_string(),
            network_id: network_id.to_string(),
            status: INVITATION_STATUS_PENDING.to_string(),
            creation_date: now,
            expiration_date: now + duration,
            accepted_at: None,
        };
        self.invitations.insert(id.clone(), invitation);
        Some(id)
    }

    /// Carry out the actions of a proposal that has just been approved: "The specified
    /// proposal actions are carried out" (`Proposal$Status`'s `APPROVED`).
    ///
    /// Every `InviteAction` records the invitation it owes its principal, and every
    /// `RemoveAction` deletes the member it names together with that member's resources.
    ///
    /// Answers whether EVERY action was carried out: `false` when any one of them could
    /// not be, which is what `ACTION_FAILED` reports — "one or more of the specified
    /// `ProposalActions` … couldn't be completed because of an error. The `ACTION_FAILED`
    /// status occurs even if only one ProposalAction fails and other actions are
    /// successful", so the remaining actions are still attempted.
    pub fn carry_out_proposal_actions(
        &mut self,
        network_id: &str,
        actions: &ProposalActions,
        region: &str,
        now: SystemTime,
    ) -> bool {
        let mut carried_out = true;
        for invite in actions.invitations.iter().flatten() {
            if self
                .record_invitation(network_id, &invite.principal, region, now)
                .is_none()
            {
                carried_out = false;
            }
        }
        for removal in actions.removals.iter().flatten() {
            if !self.remove_member(network_id, &removal.member_id) {
                carried_out = false;
            }
        }
        carried_out
    }

    /// Remove `member_id` from `network_id`: "The member and all associated resources are
    /// deleted from the network" (`RemoveAction`), so the member's nodes go with it, and
    /// the idempotency record of each node's create goes with the node (the same reason
    /// [`Self::forget_node_token`] is called when a node is deleted outright).
    ///
    /// Answers whether the member was there to remove: `false` when the network or the
    /// member names nothing stored, which is an action that could not be carried out.
    pub fn remove_member(&mut self, network_id: &str, member_id: &str) -> bool {
        let Some(network) = self.networks.get_mut(network_id) else {
            return false;
        };
        if network.members.remove(member_id).is_none() {
            return false;
        }
        let orphaned: Vec<String> = network
            .nodes
            .iter()
            .filter(|(_, node)| node.member_id.as_deref() == Some(member_id))
            .map(|(id, _)| id.clone())
            .collect();
        for node_id in &orphaned {
            network.nodes.remove(node_id);
        }
        for node_id in &orphaned {
            self.forget_node_token(node_id);
        }
        true
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsManagedblockchainTaigawebserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsManagedblockchainTaigawebserviceState>)
}

/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a network: `arn:aws:managedblockchain:<region>:<account>:networks/<id>`,
/// which satisfies `ArnString`'s pattern `^arn:.+:.+:.+:.+:.+$`.
pub fn network_arn(region: &str, network_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("networks/{network_id}"))
}

/// The ARN of a member: `arn:aws:managedblockchain:<region>:<account>:members/<id>`.
pub fn member_arn(region: &str, member_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("members/{member_id}"))
}

/// The ARN of an accessor:
/// `arn:aws:managedblockchain:<region>:<account>:accessors/<id>`.
pub fn accessor_arn(region: &str, accessor_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("accessors/{accessor_id}"))
}

/// The ARN of a node: `arn:aws:managedblockchain:<region>:<account>:nodes/<id>`.
pub fn node_arn(region: &str, node_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("nodes/{node_id}"))
}

/// The ARN of a proposal:
/// `arn:aws:managedblockchain:<region>:<account>:proposals/<id>`, the same
/// `<resource-type>/<id>` shape as the network, member and node ARNs above, and within
/// `ArnString`'s pattern `^arn:.+:.+:.+:.+:.+$`.
pub fn proposal_arn(region: &str, proposal_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("proposals/{proposal_id}"))
}

/// How long a proposal on a network with this voting policy stays open — and, through
/// [`ComAmazonawsManagedblockchainTaigawebserviceState::record_invitation`], how long an
/// invitation minted on it stays `PENDING`: both
/// `Proposal$ExpirationDate` and `Invitation$ExpirationDate` are documented as "the
/// `CreationDate` plus the `ProposalDurationInHours` that is specified in the
/// `ProposalThresholdPolicy`".
///
/// `ApprovalThresholdPolicy$ProposalDurationInHours` is "the duration from the time that a
/// proposal is created until it expires", so the value the network was created with is
/// kept exactly: this emulator chooses nothing here. `None` when the policy names no
/// duration, which fixes no expiry — `Proposal$ExpirationDate` then goes unreported, while
/// an invitation, whose expiry is not optional, falls back to
/// [`DEFAULT_PROPOSAL_DURATION_IN_HOURS`].
pub fn proposal_duration(policy: &VotingPolicy) -> Option<Duration> {
    let hours = policy
        .approval_threshold_policy
        .as_ref()
        .and_then(|threshold| threshold.proposal_duration_in_hours)?
        // `ProposalDurationInt` is `@range(1, 168)`, which the boundary validator already
        // enforced on the create; the clamp keeps the arithmetic sound whatever a stored
        // record holds.
        .clamp(1, 168);
    u64::try_from(hours)
        .ok()
        .map(|hours| Duration::from_secs(hours * 60 * 60))
}

/// How many `YES` votes approve a proposal on a network with this voting policy, when the
/// network holds `members` members.
///
/// `ApprovalThresholdPolicy$ThresholdPercentage` is "the percentage of votes among all
/// members that must be `YES` for a proposal to be approved", and
/// `$ThresholdComparator` "determines whether the vote percentage must be greater than
/// the `ThresholdPercentage` or must be greater than or equal to the
/// `ThresholdPercentage` to be approved" — so the count is the smallest number of `YES`
/// votes whose share of the membership satisfies that comparison. This reproduces the
/// documented worked example exactly: 50% `GREATER_THAN` on a network with 10 members
/// requires 6 `YES` votes.
///
/// `None` when the policy names no threshold (either no `ApprovalThresholdPolicy` at all
/// or no `ThresholdPercentage` in it): nothing then fixes how many `YES` votes approve a
/// proposal, so no number of votes decides it.
///
/// Never below one: the percentage is of the votes "that must be `YES`", so approval
/// takes a `YES` vote however low the threshold is set (`ThresholdPercentageInt` is
/// `@range(0, 100)`, so a policy may name 0%).
pub fn required_yes_votes(policy: Option<&VotingPolicy>, members: usize) -> Option<i64> {
    let threshold = policy?.approval_threshold_policy.as_ref()?;
    // `ThresholdPercentageInt` is `@range(0, 100)`, which the boundary validator already
    // enforced on the create; the clamp keeps the arithmetic sound whatever a stored
    // record holds.
    let percentage = i64::from(threshold.threshold_percentage?.clamp(0, 100));
    let members = i64::try_from(members).unwrap_or(i64::MAX);
    let comparator = threshold
        .threshold_comparator
        .as_deref()
        .unwrap_or(DEFAULT_THRESHOLD_COMPARATOR);
    // `votes * 100` against `percentage * members` keeps the comparison in integers, so
    // no rounding of a percentage can approve a proposal a point short of the threshold.
    // The membership is bounded by `MAX_MEMBERS_PER_NETWORK`, which
    // `StoredNetwork::insert_member` enforces, but the product is saturated here as well
    // so this public helper's arithmetic does not rest on its callers' bounds.
    let share = percentage.saturating_mul(members);
    let required = if comparator == THRESHOLD_GREATER_THAN_OR_EQUAL_TO {
        // The smallest `votes` with `votes * 100 >= share`.
        share.div_euclid(100) + i64::from(share.rem_euclid(100) != 0)
    } else {
        // `GREATER_THAN` (and the default): the smallest `votes` with `votes * 100 >
        // share`.
        share.div_euclid(100) + 1
    };
    Some(required.max(1))
}

/// The `ProposalStatus` the votes cast so far decide for a proposal on a network with
/// this voting policy, or `None` while they decide nothing and the proposal stays
/// "active and open for member voting".
///
/// The rule has two halves, both from `ApprovalThresholdPolicy$ProposalDurationInHours`:
/// "if members cast neither the required number of `YES` votes to approve the proposal
/// nor the number of `NO` votes required to reject it before the duration expires, the
/// proposal is `EXPIRED`".
///
/// * approved once the `YES` votes reach [`required_yes_votes`];
/// * rejected once `NO` votes have been cast and leave too few members un-voted for that
///   count to be reached — "rejected with insufficient `YES` votes among members according
///   to the `VotingPolicy`" (`Proposal$Status`). Rejection is what the `NO` votes do, so a
///   threshold no number of votes could reach (100% under `GREATER_THAN`, say) is not
///   rejected by a `YES` vote: with no `NO` vote cast it stays open, and the clock expires
///   it on its `ExpirationDate`, which is exactly the case
///   `ProposalDurationInHours` describes.
/// * otherwise undecided, which is what leaves the proposal open to its `ExpirationDate`.
pub fn proposal_outcome(
    policy: Option<&VotingPolicy>,
    members: usize,
    yes_votes: i32,
    no_votes: i32,
) -> Option<&'static str> {
    let required = required_yes_votes(policy, members)?;
    let members = i64::try_from(members).unwrap_or(i64::MAX);
    let yes = i64::from(yes_votes);
    let no = i64::from(no_votes);
    if yes >= required {
        return Some(PROPOSAL_APPROVED);
    }
    // Every member that voted `NO` is a member that cannot vote `YES`, so the most `YES`
    // votes this proposal can still gather is the membership minus the `NO` votes. It is
    // the `NO` votes that reject a proposal ("the number of `NO` votes required to reject
    // it"), so a threshold that was out of reach before anybody voted `NO` leaves the
    // proposal open to its `ExpirationDate` instead.
    if no > 0 && members - no < required {
        return Some(PROPOSAL_REJECTED);
    }
    None
}

/// The ARN of an invitation
/// (`arn:aws:managedblockchain:<region>:<account>:invitations/<id>`), which satisfies
/// `ArnString`'s pattern `^arn:.+:.+:.+:.+:.+$`.
pub fn invitation_arn(region: &str, invitation_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("invitations/{invitation_id}"),
    )
}

/// How this service names the resource type of a network in the prose of a failure.
pub const RESOURCE_NETWORK: &str = "Network";

/// How this service names the resource type of a member in the prose of a failure.
pub const RESOURCE_MEMBER: &str = "Member";

/// How this service names the resource type of a node in the prose of a failure.
pub const RESOURCE_NODE: &str = "Node";

/// How this service names the resource type of an invitation in the prose of a failure.
pub const RESOURCE_INVITATION: &str = "Invitation";

/// How this service names the resource type of a proposal in the prose of a failure.
pub const RESOURCE_PROPOSAL: &str = "Proposal";

/// How this service names the resource type of an accessor in the prose of a failure.
pub const RESOURCE_ACCESSOR: &str = "Accessor";

/// The `ResourceNotFoundException` for a resource the request named and this service does
/// not hold: `Network n-… not found.`, `Node nd-… not found.`
///
/// `ResourceNotFoundException$ResourceName` targets `ArnString` — `@length(1, 1011)` and
/// `@pattern ^arn:.+:.+:.+:.+:.+$` — so the member can only ever carry an ARN. A lookup
/// that found nothing has no stored ARN to name, and a bare resource id cannot satisfy
/// that pattern (a client that parses the member for its region, account and resource id
/// would fail on one, and a model-driven validator rejects it outright), so the member is
/// OMITTED here. The identification travels in `Message` instead, which names the type
/// and the id the request asked for — both read off the request, so the sentence always
/// names the resource the caller actually asked about.
///
/// This is the helper for a resource the request identifies BY ID, which every operation
/// here but the three tagging ones does. One that identifies it by ARN
/// (`TagResource`/`UntagResource`/`ListTagsForResource` take a `ResourceArn`) does have
/// an ARN to name, so a handler of one of those sets `ResourceName` to that ARN instead
/// of calling this.
pub fn not_found(resource_type: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("{resource_type} {id} not found.")),
        resource_name: None,
    }
}

/// The `ResourceNotFoundException` for a resource the request named BY ARN and this
/// service does not hold.
///
/// The counterpart of [`not_found`] for the three tagging operations, which identify
/// their resource by `ResourceArn`: here there IS an ARN to put in
/// `ResourceNotFoundException$ResourceName` (`ArnString`, `@pattern
/// ^arn:.+:.+:.+:.+:.+$`), namely the one the request sent, which the modelled pattern
/// already held it to. `resource_type` is how this service names the type the ARN spells
/// (see [`taggable_resource_type`]).
pub fn not_found_arn(resource_type: &str, arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("{resource_type} {arn} not found.")),
        resource_name: Some(arn.to_string()),
    }
}

/// How many tags one resource can carry.
///
/// "A resource can have up to 50 tags. If you try to create more than 50 tags for a
/// resource, your request fails and returns an error" (`TagResource`), which is also what
/// `InputTagMap`'s `@length(0, 50)` lets one request carry. `OutputTagMap`'s
/// `@length(0, 200)` bounds the map a READ reports, not how many tags a resource may
/// hold, so a write is held to the documented 50.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The resource types this service tags: the `<type>` of an ARN's `<type>/<id>` resource
/// part, paired with how a failure names that type.
///
/// These are exactly the shapes whose model declares a `Tags` member — `Network`,
/// `Member`, `Node`, `Proposal` and `Accessor` — so they are the resources a tag set
/// exists for. `Invitation` declares none, so no ARN of one names a tag set.
pub const TAGGABLE_RESOURCE_TYPES: &[(&str, &str)] = &[
    ("accessors", RESOURCE_ACCESSOR),
    ("members", RESOURCE_MEMBER),
    ("networks", RESOURCE_NETWORK),
    ("nodes", RESOURCE_NODE),
    ("proposals", RESOURCE_PROPOSAL),
];

/// How this service names the type of resource `arn` identifies, when `arn` is an ARN in
/// this service's own namespace that names one of the resource types it tags
/// ([`TAGGABLE_RESOURCE_TYPES`]).
///
/// `None` for anything else — another service's ARN, or one of this service's whose
/// resource part names a type that holds no tag set: no resource this service could ever
/// tag is named by such an ARN, whatever the store holds, so the request asks for
/// something this service cannot do rather than for a resource it has lost.
pub fn taggable_resource_type(arn: &str) -> Option<&'static str> {
    // `arn:<partition>:<service>:<region>:<account>:<resource>` — the resource part is
    // taken whole (it may itself contain colons) and only its `<type>/<id>` head is read.
    //
    // The partition, Region and account are deliberately NOT judged here: this answers
    // only whether the ARN names a KIND of resource this service tags. Whether it names
    // one this store holds is then decided by matching the ARN a create minted and
    // stored, so an ARN of another partition, Region or account is a resource that is
    // not found rather than a request this service cannot understand.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, id) = fields.next()?.split_once('/')?;
    if id.is_empty() {
        return None;
    }
    TAGGABLE_RESOURCE_TYPES
        .iter()
        .find(|(segment, _)| *segment == resource_type)
        .map(|(_, named)| *named)
}

/// How this service words a tagging request whose `ResourceArn` names nothing it could
/// ever tag ([`taggable_resource_type`] answered `None`): the prose of the
/// `InvalidRequestException` every operation of this service models for a request it
/// cannot accept, naming the ARN the request sent and the resource types that can carry
/// tags.
pub fn not_a_taggable_arn(arn: &str) -> String {
    let types: Vec<&str> = TAGGABLE_RESOURCE_TYPES
        .iter()
        .map(|(segment, _)| *segment)
        .collect();
    format!(
        "{arn} is not the ARN of a taggable {ARN_SERVICE} resource. Expected \
         arn:<partition>:{ARN_SERVICE}:<region>:<account>:<type>/<id>, where <type> is one \
         of: {}.",
        types.join(", ")
    )
}

/// The VPC endpoint service name members use to reach a network's resources
/// (`Network$VpcEndpointServiceName`), shaped as Managed Blockchain publishes it:
/// `com.amazonaws.<region>.managedblockchain.<NetworkId>`.
pub fn vpc_endpoint_service_name(region: &str, network_id: &str) -> String {
    format!("com.amazonaws.{region}.{ARN_SERVICE}.{network_id}")
}

/// A network's ordering-service endpoint, shaped as Managed Blockchain publishes it for
/// Hyperledger Fabric: `orderer.<NetworkId>.managedblockchain.<region>.amazonaws.com`
/// on the ordering-service port. Derived from the stored id, so it is stable across
/// reads and differs between two networks.
pub fn ordering_service_endpoint(region: &str, network_id: &str) -> String {
    format!("orderer.{network_id}.{ARN_SERVICE}.{region}.amazonaws.com:30001")
}

/// A member's certificate-authority endpoint (`MemberFabricAttributes$CaEndpoint`),
/// shaped `ca.<MemberId>.<NetworkId>.managedblockchain.<region>.amazonaws.com` on the
/// CA port.
pub fn ca_endpoint(region: &str, network_id: &str, member_id: &str) -> String {
    format!("ca.{member_id}.{network_id}.{ARN_SERVICE}.{region}.amazonaws.com:30002")
}

/// A node's peer endpoint and peer event endpoint (`NodeFabricAttributes`), shaped
/// `<NodeId>.<MemberId>.<NetworkId>.managedblockchain.<region>.amazonaws.com` on the
/// peer and peer-event ports.
pub fn peer_endpoints(
    region: &str,
    network_id: &str,
    member_id: Option<&str>,
    node_id: &str,
) -> (String, String) {
    let host = match member_id {
        Some(member_id) => {
            format!("{node_id}.{member_id}.{network_id}.{ARN_SERVICE}.{region}.amazonaws.com")
        }
        None => format!("{node_id}.{network_id}.{ARN_SERVICE}.{region}.amazonaws.com"),
    };
    (format!("{host}:30003"), format!("{host}:30004"))
}

/// The endpoints a node on an Ethereum network is reached on
/// (`NodeEthereumAttributes$HttpEndpoint` and `$WebSocketEndpoint`), shaped as AMB Access
/// publishes them: the JSON-RPC endpoint over HTTPS and the same node over WebSocket.
/// Derived from the stored node id, so they are stable across reads and differ between two
/// nodes.
pub fn ethereum_endpoints(region: &str, node_id: &str) -> (String, String) {
    (
        format!("https://{node_id}.ethereum.{ARN_SERVICE}.{region}.amazonaws.com"),
        format!("wss://{node_id}.wss.ethereum.{ARN_SERVICE}.{region}.amazonaws.com"),
    )
}

/// The endpoints a node on `framework` is reached on, for the node and member ids its
/// create minted: the framework decides which `NodeFrameworkAttributes` the node reports.
pub fn node_endpoints(
    framework: &str,
    region: &str,
    network_id: &str,
    member_id: Option<&str>,
    node_id: &str,
) -> StoredNodeEndpoints {
    if framework == FRAMEWORK_HYPERLEDGER_FABRIC {
        let (peer_endpoint, peer_event_endpoint) =
            peer_endpoints(region, network_id, member_id, node_id);
        StoredNodeEndpoints::Fabric {
            peer_endpoint,
            peer_event_endpoint,
        }
    } else {
        let (http_endpoint, web_socket_endpoint) = ethereum_endpoints(region, node_id);
        StoredNodeEndpoints::Ethereum {
            http_endpoint,
            web_socket_endpoint,
        }
    }
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `NextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Prefix the cursor plaintext carries, so a token this service never minted cannot
/// decode into a usable position.
const CURSOR_PREFIX: &str = "managedblockchain-after:";

/// Encode the resume cursor that follows the item keyed `key`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(0, 128)` for every id this service mints. The cursor names a POSITION in the
/// id ordering rather than an offset, so resuming never repeats or skips an item when one
/// is created or deleted between pages.
///
/// It is self-describing, so a listing validates a token by decoding it and never has to
/// record the tokens it issued — which matters because a listing is a reporting
/// operation: remembering an issued token would make a read mutate state.
pub fn encode_cursor(key: &str) -> String {
    ::hex::encode(format!("{CURSOR_PREFIX}{key}"))
}

/// Decode a cursor minted by [`encode_cursor`]; `None` starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<Option<String>, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    if token.is_empty() {
        // `NextToken` is `@length(0, 128)`, so the empty string is a legal value; it
        // names no position, which is the start.
        return Ok(None);
    }
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix(CURSOR_PREFIX)
        .map(|key| Some(key.to_string()))
        .ok_or(MalformedCursor)
}

/// Take the page of `ordered` (sorted by key) that follows `after`, at most
/// `max_results` long, defaulting to and capped at `limit_max`.
///
/// `MaxResults` carries a `@range`, so the boundary validator has already refused
/// anything outside it; an absent one takes a full page.
pub fn page_of<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
    limit_max: i32,
) -> Page<T> {
    let limit = max_results.unwrap_or(limit_max).clamp(1, limit_max) as usize;
    let mut items = Vec::new();
    let mut last_key = String::new();
    let mut more_follow = false;
    for (key, item) in ordered
        .into_iter()
        .filter(|(key, _)| after.is_none_or(|after| key.as_str() > after))
    {
        if items.len() == limit {
            // One item beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips an item.
            more_follow = true;
            break;
        }
        last_key = key;
        items.push(item);
    }
    Page {
        items,
        next_token: more_follow.then(|| encode_cursor(&last_key)),
    }
}

/// The `StateDB` a node takes when the create names none: `NodeConfiguration$StateDB`
/// documents `CouchDB` as the default.
pub fn default_state_db() -> String {
    DEFAULT_STATE_DB.to_string()
}

/// The account segment of an ARN (`arn:<partition>:<service>:<region>:<account>:<rest>`),
/// or `None` when the string carries no non-empty one.
fn arn_account_id(arn: &str) -> Option<&str> {
    arn.split(':').nth(4).filter(|account| !account.is_empty())
}

/// A stored tag map as the modelled `OutputTagMap`.
///
/// `pub` because `ListTagsForResource` answers with a resource's tag map itself, where
/// every other read reaches it as one member of a larger shape.
pub fn map_out(map: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// An input tag map as a stored one.
pub fn map_in(map: Option<std::collections::HashMap<String, String>>) -> BTreeMap<String, String> {
    map.unwrap_or_default().into_iter().collect()
}

/// Wall-clock nanoseconds since the epoch, used only to seed the id counter.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never
/// revisits a value before the counter wraps.
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as 13 characters of the base-32 alphabet Managed Blockchain ids are written
/// in (RFC 4648: `A`-`Z` and `2`-`7`).
fn base32_13(value: u64) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    (0..13)
        .map(|i| {
            let shift = 5 * (12 - i);
            let digit = (value >> shift) & 0x1f;
            char::from(ALPHABET[digit as usize])
        })
        .collect()
}
