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
    Accessor, Invitation, Member, MemberFabricAttributes, MemberFrameworkAttributes,
    MemberLogPublishingConfiguration, Network, NetworkFabricAttributes,
    NetworkFrameworkAttributes, NetworkSummary, Node, NodeFabricAttributes,
    NodeFrameworkAttributes, NodeLogPublishingConfiguration, NodeSummary, VotingPolicy,
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

/// The non-terminal status a freshly created network, member or node is reported in
/// (`NetworkStatus` / `MemberStatus` / `NodeStatus` all declare it).
pub const STATUS_CREATING: &str = "CREATING";

/// The settled status the clock owes a created network, member or node.
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `InvitationStatus` a minted invitation starts in: "`PENDING` - The invitee hasn't
/// created a member to join the network, and the invitation hasn't yet expired"
/// (`Invitation$Status`).
pub const INVITATION_STATUS_PENDING: &str = "PENDING";

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

/// `ListInvitations`'s `MaxResults` upper bound, from its `@range(1, 100)`. Also the page
/// size used when a caller names none.
pub const INVITATION_MAX_RESULTS: i32 = 100;

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
    pub framework_version: String,
    /// `NetworkFabricAttributes$Edition`, as the create's `FrameworkConfiguration` set
    /// it; absent when the request configured no framework properties.
    pub fabric_edition: Option<String>,
    /// `NetworkFabricAttributes$OrderingServiceEndpoint`, minted at create time.
    pub ordering_service_endpoint: String,
    pub voting_policy: VotingPolicy,
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
}

impl StoredNetwork {
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
    }

    /// The modelled `Network` for this record, as `GetNetwork` returns it.
    pub fn to_network(&self) -> Network {
        Network {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            framework: Some(self.framework.clone()),
            framework_version: Some(self.framework_version.clone()),
            framework_attributes: Some(NetworkFrameworkAttributes {
                fabric: Some(NetworkFabricAttributes {
                    ordering_service_endpoint: Some(self.ordering_service_endpoint.clone()),
                    edition: self.fabric_edition.clone(),
                }),
                // Only Hyperledger Fabric networks can be created here, so no record
                // ever carries Ethereum attributes.
                ethereum: None,
            }),
            vpc_endpoint_service_name: Some(self.vpc_endpoint_service_name.clone()),
            voting_policy: Some(self.voting_policy.clone()),
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
            framework_version: Some(self.framework_version.clone()),
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
    /// `NodeFabricAttributes$PeerEndpoint`, minted at create time.
    pub peer_endpoint: String,
    /// `NodeFabricAttributes$PeerEventEndpoint`, minted at create time.
    pub peer_event_endpoint: String,
    pub log_publishing_configuration: Option<NodeLogPublishingConfiguration>,
    /// A `StateDBType` value.
    pub state_db: String,
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
    /// The modelled `Node` for this record, as `GetNode` returns it.
    pub fn to_node(&self) -> Node {
        Node {
            network_id: Some(self.network_id.clone()),
            member_id: self.member_id.clone(),
            id: Some(self.id.clone()),
            instance_type: Some(self.instance_type.clone()),
            availability_zone: self.availability_zone.clone(),
            framework_attributes: Some(NodeFrameworkAttributes {
                fabric: Some(NodeFabricAttributes {
                    peer_endpoint: Some(self.peer_endpoint.clone()),
                    peer_event_endpoint: Some(self.peer_event_endpoint.clone()),
                }),
                // Only Hyperledger Fabric networks can be created here, so no node ever
                // carries Ethereum attributes.
                ethereum: None,
            }),
            log_publishing_configuration: self.log_publishing_configuration.clone(),
            state_db: Some(self.state_db.clone()),
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
}

impl StoredAccessor {
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
}

/// Per-service state for com-amazonaws-managedblockchain-taigawebservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsManagedblockchainTaigawebserviceState {
    /// Every created network, keyed by its minted id — which also orders the listings,
    /// so a page cursor keys on the ordering itself.
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

impl ComAmazonawsManagedblockchainTaigawebserviceState {
    /// Materialise every status transition the clock already owes. Called at the top of
    /// each handler, reporting ones included: they may write only a change that was
    /// already due at this instant.
    pub fn settle(&mut self, now: SystemTime) {
        for network in self.networks.values_mut() {
            network.settle(now);
        }
        // An invitation nobody acted on before its `ExpirationDate` is `EXPIRED` — a
        // transition the CLOCK owes, due at that instant whether or not anybody calls.
        for invitation in self.invitations.values_mut() {
            if invitation.status == INVITATION_STATUS_PENDING && now >= invitation.expiration_date {
                invitation.status = INVITATION_STATUS_EXPIRED.to_string();
            }
        }
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
        let hours = self
            .networks
            .get(network_id)?
            .voting_policy
            .approval_threshold_policy
            .as_ref()
            .and_then(|policy| policy.proposal_duration_in_hours)
            .unwrap_or(DEFAULT_PROPOSAL_DURATION_IN_HOURS)
            // `ProposalDurationInt` is `@range(1, 168)`, which the boundary validator
            // already enforced on the create; the clamp keeps the arithmetic below sound
            // whatever a stored record holds.
            .clamp(1, 168) as u64;
        let id = self.mint_invitation_id();
        let invitation = StoredInvitation {
            arn: invitation_arn(region, &id),
            id: id.clone(),
            principal: principal.to_string(),
            network_id: network_id.to_string(),
            status: INVITATION_STATUS_PENDING.to_string(),
            creation_date: now,
            expiration_date: now + Duration::from_secs(hours * 3_600),
        };
        self.invitations.insert(id.clone(), invitation);
        Some(id)
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

/// A stored tag map as the modelled `OutputTagMap`.
fn map_out(map: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
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
