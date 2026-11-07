// SPDX-License-Identifier: CC0-1.0

//! The NETWORK slice: the network record, the IP pools and routes a request
//! describes, the refusals an update answers with and the network projections.
//!
//! One file for the whole slice, so the operations that read and write a
//! network cannot drift into several readings of one rule. Moved here unchanged
//! from the state root, which re-exports it (`pub use networks::*;`), so every
//! `crate::state::…` path a handler already uses still resolves.

use super::*;

/// One created Network, as `CreateNetwork` stored it plus the lifecycle state of
/// that creation.
///
/// The members mirror the model's network response shapes (`CreateNetworkResponse`,
/// `DescribeNetworkResponse` and the `DescribeNetworkSummary` a listing reports,
/// which carry the same seven members), minus the one they derive:
/// `AssociatedClusterIds` comes from what else is in the store
/// ([`ComAmazonawsMedialiveMedialiveState::clusters_using_network`]), never from a
/// field written at create time.
#[derive(Debug, Clone)]
pub struct NetworkRecord {
    pub id: String,
    pub arn: String,
    /// The request's `Name`. `Option`, because the model declares the member
    /// optional (`smithy_query(query="operation", operation="CreateNetwork")`: no
    /// `@required`), so a network created without one has none rather than an
    /// empty name.
    pub name: Option<String>,
    /// The CIDR blocks the request reserved, stored as the `IpPool` shape a
    /// response reports, because `IpPoolCreateRequest` and `IpPool` carry the
    /// same one member (`Cidr`).
    pub ip_pools: Vec<IpPool>,
    /// The routes the request named, stored as the `Route` shape a response
    /// reports, for the same reason as [`Self::ip_pools`].
    pub routes: Vec<Route>,
    /// The request's `Tags`. No network response shape carries a `Tags` member
    /// (`smithy_query(query="operation", operation="DescribeNetwork")`), so the
    /// tag operations are what report them: `ListTagsForResource` reads this map
    /// through [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
    /// The `NetworkState` this network is in.
    pub(super) state: String,
    /// The instant the creation — or a later accepted settings change — becomes
    /// due, so `CREATING` and `UPDATING` settle into `ACTIVE`. Stored, never
    /// slept on.
    pub(super) active_due: SystemTime,
    /// The instant an accepted deletion becomes due, so `DELETING` settles into
    /// the terminal `DELETED`. `None` until a delete is accepted, and `None`
    /// again once that deletion has come due — [`Self::deleted`] is what records
    /// it from then on. Stored, never slept on.
    pub(super) deletion_due: Option<SystemTime>,
    /// Whether this network's deletion has come due, which is what puts it in
    /// the terminal `DELETED` state it then keeps.
    pub(super) deleted: bool,
}

impl NetworkRecord {
    /// Whether a delete has been accepted for this network and the clock has not
    /// made that deletion due yet.
    pub fn is_deleting(&self) -> bool {
        self.deletion_due.is_some()
    }

    /// Whether this network's deletion has come due, which is what puts it in
    /// the terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Whether a delete has been accepted for this network, in either half of
    /// the deletion lifecycle — so a repeated `DeleteNetwork` is idempotent
    /// rather than restarting the clock, and a network on its way out no longer
    /// holds the name it was created under
    /// ([`ComAmazonawsMedialiveMedialiveState::network_named`]).
    pub fn is_delete_accepted(&self) -> bool {
        self.is_deleting() || self.is_deleted()
    }

    /// Accept a settings change for this network: it moves into `UPDATING` and
    /// owes `ACTIVE` once [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// `State` is documented "The current state of the Network. Only MediaLive
    /// Anywhere can change the state" and the model's `NetworkState` declares
    /// `UPDATING`, so applying a network's new settings is asynchronous just as
    /// its creation is: the caller polls `DescribeNetwork` for `ACTIVE` again.
    /// The due instant is stored, never slept on.
    pub(super) fn begin_update(&mut self, now: SystemTime) {
        self.state = network_state::UPDATING.to_string();
        self.active_due = now + TRANSIENT_STATE_SPAN;
    }

    /// Materialise the creation, the settings change or the deletion the clock
    /// has already made due,
    /// answering whether THIS call is the one that settled the deletion. Each is
    /// owed at its stored instant whether or not anybody called, so applying it
    /// in a reporting operation changes nothing a later caller would have seen.
    pub(super) fn settle(&mut self, now: SystemTime) -> bool {
        if self.deletion_due.is_some_and(|due| now >= due) {
            self.deletion_due = None;
            self.deleted = true;
            self.state = network_state::DELETED.to_string();
            return true;
        }
        if (self.state == network_state::CREATING || self.state == network_state::UPDATING)
            && now >= self.active_due
        {
            self.state = network_state::ACTIVE.to_string();
        }
        false
    }
}

/// Why [`ComAmazonawsMedialiveMedialiveState::apply_network_changes`] changed
/// nothing. Each variant is one refusal `UpdateNetwork` answers with, and each
/// leaves the stored network exactly as it was.
#[derive(Debug)]
pub enum NetworkUpdateRefusal {
    /// The `NetworkId` names no network this store has ever held.
    NoSuchNetwork,
    /// The store still holds the network — a delete keeps the record readable —
    /// but that delete has been accepted, so the id names nothing a caller may
    /// still change.
    AlreadyDeleted,
    /// Another live network of the account already carries the new `Name`.
    NameTaken,
}

/// What an `UpdateNetwork` request asks to change.
///
/// Each member is `Some` only when the request carried it — "Include this
/// parameter only if you want to change …" on all three — so an omitted member
/// leaves that setting exactly as it was, and a member carrying an empty list
/// clears it. The two CIDR lists arrive already validated into the shapes the
/// store keeps ([`network_ip_pools`], [`network_routes`]), so a refused list
/// never reaches the store at all. One struct rather than three positional
/// `Option`s on
/// [`ComAmazonawsMedialiveMedialiveState::apply_network_changes`], so a handler
/// cannot pass the pools where the routes belong.
#[derive(Debug, Clone, Default)]
pub struct NetworkChanges {
    pub name: Option<String>,
    pub ip_pools: Option<Vec<IpPool>>,
    pub routes: Option<Vec<Route>>,
}

/// The message a request whose `IpPools` are not all valid IPv4 CIDR blocks is
/// refused with \u2014 the fixed sentence of that rejection class, which states the
/// rule the whole list must meet and interpolates nothing, exactly as
/// [`INVALID_WHITELIST_CIDR_MESSAGE`] does for a security group's rules.
pub const INVALID_IP_POOL_CIDR_MESSAGE: &str = "Every IP pool needs to have a valid IPv4 CIDR.";

/// The message a request whose `Routes` are not all valid is refused with: a
/// route's `Cidr` is "A CIDR block for one Route" and its optional `Gateway` is
/// "The IP address of the Gateway for this route", so a route missing the former
/// or carrying a gateway that is not an IP address fails this one rule.
pub const INVALID_ROUTE_MESSAGE: &str =
    "Every route needs to have a valid IPv4 CIDR and, if any, a valid IPv4 gateway address.";

/// The message a request naming more `IpPools` or `Routes` than the emulator
/// stores on one network is refused with.
pub fn too_many_network_entries_message(member: &str) -> String {
    format!("A network takes at most {MAX_NETWORK_CIDR_ENTRIES} {member} entries.")
}

/// The `IpPools` a request asks for, validated into the pools the store keeps,
/// or the message a list that is not all valid IPv4 CIDR blocks is refused with.
///
/// Takes the CIDRs rather than the request shape, so the one rule serves both
/// operations that write a network's pools (`CreateNetwork` and `UpdateNetwork`)
/// even though the model gives each of
/// them its own request shape (`IpPoolCreateRequest`, `IpPoolUpdateRequest`)
/// carrying the same single member. The whole list is checked before the caller
/// writes anything, so a refused request leaves the store untouched rather than
/// applying a prefix of its pools.
pub fn network_ip_pools(
    requested: impl IntoIterator<Item = Option<String>>,
) -> Result<Vec<IpPool>, String> {
    let mut pools: Vec<IpPool> = Vec::new();
    for cidr in requested {
        if pools.len() >= MAX_NETWORK_CIDR_ENTRIES {
            return Err(too_many_network_entries_message("ipPools"));
        }
        let cidr = cidr.filter(|cidr| is_ipv4_cidr(cidr));
        let Some(cidr) = cidr else {
            return Err(INVALID_IP_POOL_CIDR_MESSAGE.to_string());
        };
        pools.push(IpPool { cidr: Some(cidr) });
    }
    Ok(pools)
}

/// The `Routes` a request asks for, validated into the routes the store keeps,
/// or the message a list carrying an invalid route is refused with. Takes the
/// `(Cidr, Gateway)` pairs for the same reason [`network_ip_pools`] takes the
/// CIDRs.
///
/// `Gateway` is documented "The IP address of the Gateway for this route, if
/// applicable", so a route may omit it; one that names it must name an IP
/// address.
pub fn network_routes(
    requested: impl IntoIterator<Item = (Option<String>, Option<String>)>,
) -> Result<Vec<Route>, String> {
    let mut routes: Vec<Route> = Vec::new();
    for (cidr, gateway) in requested {
        if routes.len() >= MAX_NETWORK_CIDR_ENTRIES {
            return Err(too_many_network_entries_message("routes"));
        }
        let cidr = cidr.filter(|cidr| is_ipv4_cidr(cidr));
        let gateway_is_address = gateway
            .as_deref()
            .is_none_or(|gateway| gateway.parse::<::std::net::Ipv4Addr>().is_ok());
        let Some(cidr) = cidr.filter(|_| gateway_is_address) else {
            return Err(INVALID_ROUTE_MESSAGE.to_string());
        };
        routes.push(Route {
            cidr: Some(cidr),
            gateway,
        });
    }
    Ok(routes)
}

/// Build a new network record from a `CreateNetwork` request's id and ARN.
///
/// The network starts `CREATING` and owes `ACTIVE` once [`TRANSIENT_STATE_SPAN`]
/// has passed: `State` is documented "The current state of the Network. Only
/// MediaLive Anywhere can change the state", so creation is asynchronous and a
/// caller waits on that member. The caller then stores the request's `Name`,
/// `IpPools`, `Routes` and `Tags` on the record.
pub fn new_network(id: String, arn: String, now: SystemTime) -> NetworkRecord {
    NetworkRecord {
        id,
        arn,
        name: None,
        ip_pools: Vec::new(),
        routes: Vec::new(),
        tags: HashMap::new(),
        state: network_state::CREATING.to_string(),
        active_due: now + TRANSIENT_STATE_SPAN,
        deletion_due: None,
        deleted: false,
    }
}

/// Accept a delete for `network`: it moves into `DELETING` and owes the terminal
/// `DELETED` once [`TRANSIENT_STATE_SPAN`] has passed.
///
/// Deleting a network is asynchronous — the model answers `DeleteNetwork` with
/// `202 Accepted` and `State` is documented "The current state of the Network.
/// Only MediaLive Anywhere can change the state" — so the caller polls
/// `DescribeNetwork` for that terminal state. The record then stays in the
/// account in it, so the readers go on reporting the network there rather than
/// losing it.
///
/// Idempotent: a delete already accepted keeps the instant it was accepted at,
/// so a repeated call neither restarts the clock nor revives a deleted network.
pub fn begin_delete_network(network: &mut NetworkRecord, now: SystemTime) {
    if network.is_delete_accepted() {
        return;
    }
    network.deletion_due = Some(now + TRANSIENT_STATE_SPAN);
    network.state = network_state::DELETING.to_string();
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Whether the network store is full, so `CreateNetwork` must refuse rather
    /// than mint another network.
    pub fn networks_are_full(&self) -> bool {
        self.networks.len() >= MAX_NETWORKS
    }

    /// The next network id, as AWS shapes MediaLive resource ids: a numeric
    /// string, which `Id` documents as "the resource-id portion of the ARN".
    pub fn mint_network_id(&mut self) -> String {
        (FIRST_NETWORK_ID + self.mints.next(|mints| &mut mints.network)).to_string()
    }

    /// Record `token` as having created `network_id`.
    pub fn record_network_token(&mut self, token: String, network_id: &str) {
        self.network_tokens.record(token, network_id, MAX_NETWORKS);
    }

    /// The network a stored `RequestId` created, if that network still exists.
    pub fn network_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.network_tokens.created(request_id)?;
        self.networks.get(id).map(|network| network.id.as_str())
    }

    /// The id of the network whose `Name` is `name`, if the store holds one.
    ///
    /// `CreateNetwork`'s `Name` is documented "Specify a name that is unique in
    /// the AWS account… Names are case-sensitive", so the lookup is exact and
    /// account-wide (not per region), and it is what makes a second create under
    /// a name already taken the modelled `ConflictException` rather than a second
    /// network.
    ///
    /// A network whose delete this service has accepted is skipped: it is on its
    /// way out of the account, so the name it was created under is free for the
    /// next create even though its record stays readable.
    pub fn network_named(&self, name: &str) -> Option<&str> {
        self.networks
            .values()
            .find(|network| {
                network.name.as_deref() == Some(name) && !network.is_delete_accepted()
            })
            .map(|network| network.id.as_str())
    }

    /// The network `id` names while it is still LIVE: one no accepted delete has
    /// taken out of the account.
    ///
    /// The lookup a request REFERENCING a network resolves through — an
    /// `InterfaceMapping`'s `NetworkId`, "the ID of the network that you want to
    /// connect to the specified logicalInterfaceName" — because a network whose
    /// delete this service has accepted is on its way out of the account and is
    /// not one a new resource may take a dependency on, even though its record
    /// stays readable in that state.
    ///
    /// NOT the lookup a mutation OF the network goes through: those resolve the
    /// id across the whole [`Self::networks`] store and answer a tombstone with
    /// the already-deleted refusal of their own.
    pub fn live_network(&self, id: &str) -> Option<&NetworkRecord> {
        self.networks
            .get(id)
            .filter(|network| !network.is_delete_accepted())
    }

    /// Apply an `UpdateNetwork` request's changes to the network `id` names, or
    /// refuse.
    ///
    /// Every refusal is checked BEFORE the first write, so a refused update
    /// leaves the stored network exactly as it was. A member the request omitted
    /// is not changed ("Include this parameter only if you want to change …" on
    /// `Name`, `IpPools` and `Routes`), and neither is a member no update request
    /// carries at all: the id, the ARN, the tags and the derived
    /// `AssociatedClusterIds` are not something this operation can move.
    ///
    /// The id is resolved through the LIVE record, like every other mutation of
    /// this service: a delete keeps the record readable, but an id whose delete
    /// has been accepted names nothing a caller may still change. The documented
    /// account-wide name uniqueness is the create's own rule, resolved through
    /// the same lookup ([`Self::network_named`]) so a rename cannot store a name
    /// a create could not, while re-sending the network's own name is not a
    /// clash with itself.
    ///
    /// An accepted change puts the network in `UPDATING` and leaves the clock
    /// owing it `ACTIVE` ([`NetworkRecord::begin_update`]), because `State` is
    /// "The current state of the Network. Only MediaLive Anywhere can change the
    /// state": the settings are applied to the stored record at once and the
    /// lifecycle member a caller polls follows the clock.
    pub fn apply_network_changes(
        &mut self,
        id: &str,
        changes: NetworkChanges,
        now: SystemTime,
    ) -> Result<(), NetworkUpdateRefusal> {
        let Some(network) = self.networks.get(id) else {
            return Err(NetworkUpdateRefusal::NoSuchNetwork);
        };
        if network.is_delete_accepted() {
            return Err(NetworkUpdateRefusal::AlreadyDeleted);
        }
        // Decided while the borrow is still shared, so the write below needs one
        // lookup rather than a second fallible one.
        if changes
            .name
            .as_deref()
            .and_then(|name| self.network_named(name))
            .is_some_and(|holder| holder != id)
        {
            return Err(NetworkUpdateRefusal::NameTaken);
        }

        let Some(network) = self.networks.get_mut(id) else {
            return Err(NetworkUpdateRefusal::NoSuchNetwork);
        };
        if let Some(name) = changes.name {
            network.name = Some(name);
        }
        if let Some(ip_pools) = changes.ip_pools {
            network.ip_pools = ip_pools;
        }
        if let Some(routes) = changes.routes {
            network.routes = routes;
        }
        network.begin_update(now);
        Ok(())
    }

    /// The ids of the clusters currently attached to the network `network_id` —
    /// what `AssociatedClusterIds` reports, and what `DeleteNetwork` refuses a
    /// delete on when it is not empty. Derived from the cluster store on
    /// every read, so it tracks every later cluster create rather than being
    /// recorded at create time.
    ///
    /// The association the store knows is the one a cluster's own request
    /// carries: an `InterfaceMapping`'s `NetworkId`, "The ID of the network that
    /// you want to connect to the specified logicalInterfaceName"
    /// (`smithy_query(query="shape", shape="InterfaceMapping")`). A cluster that
    /// maps several of its interfaces onto the same network is attached to it
    /// once, and `DefaultRoute` names an interface rather than a network, so it
    /// attaches nothing.
    ///
    /// BOUNDED work, on both factors: the store holds at most [`MAX_CLUSTERS`]
    /// clusters (enforced in [`Self::insert_cluster`]) and each carries at most
    /// [`MAX_CLUSTER_INTERFACE_MAPPINGS`] mappings (`CreateCluster` refuses a
    /// longer list), so one call scans at most the product of the two and a
    /// listing — itself capped at `MAX_LIST_PAGE_SIZE` networks per page — calls
    /// it once per reported network.
    ///
    /// Reported in `BTreeMap` key order, which is the order the cluster ids were
    /// minted in because they are minted from a counter at a fixed seven-digit
    /// width (see `FIRST_CLUSTER_ID`), so the keys sort numerically.
    pub fn clusters_using_network(&self, network_id: &str) -> Vec<String> {
        self.clusters
            .values()
            // A cluster whose delete this service has ACCEPTED is no longer
            // *currently using* anything, so it leaves this derived association
            // exactly as a deleted channel leaves a cluster's own `ChannelIds`;
            // its record stays readable in its deletion states.
            .filter(|cluster| !cluster.is_delete_accepted())
            .filter(|cluster| {
                cluster
                    .network_settings
                    .as_ref()
                    .and_then(|settings| settings.interface_mappings.as_ref())
                    .is_some_and(|mappings| {
                        mappings
                            .iter()
                            .any(|mapping| mapping.network_id.as_deref() == Some(network_id))
                    })
            })
            .map(|cluster| cluster.id.clone())
            .collect()
    }

    /// The `DescribeNetworkSummary` projection of one stored network, or `None`
    /// when no network has that id.
    ///
    /// The ONE projection every operation that reports a network
    /// (`CreateNetwork`, `DescribeNetwork`, `UpdateNetwork`, `DeleteNetwork`,
    /// `ListNetworks`)
    /// answers from, so none
    /// of them can drift from the others: the summary shape a listing reports
    /// carries exactly the members the flat describe output does
    /// (`smithy_query(query="shape", shape="DescribeNetworkSummary")`), and
    /// [`network_description_output`] projects it onto those outputs.
    pub fn network_description(&self, id: &str) -> Option<DescribeNetworkSummary> {
        let network = self.networks.get(id)?;
        Some(DescribeNetworkSummary {
            arn: Some(network.arn.clone()),
            associated_cluster_ids: Some(self.clusters_using_network(id)),
            id: Some(network.id.clone()),
            ip_pools: Some(network.ip_pools.clone()),
            name: network.name.clone(),
            routes: Some(network.routes.clone()),
            state: Some(network.state.clone()),
        })
    }
}
