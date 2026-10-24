// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkmanager-networkmanager service.
//!
//! The global-network / site / link lifecycle is the core resource tree of Network
//! Manager: a global network owns sites, a site owns links, and a global network owns
//! devices (each optionally assigned to one of its sites). A global network also owns its
//! CORE network — the Cloud WAN managed network
//! whose segments and edges are a function of the core network policy document it holds.
//! Everything
//! the implemented operations answer is computed from the records below — no handler
//! echoes its input back without storing it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AWSLocation, Bandwidth, CoreNetwork, CoreNetworkEdge, CoreNetworkNetworkFunctionGroup,
    CoreNetworkPolicyError, CoreNetworkPolicyException, CoreNetworkSegment, Device, GlobalNetwork,
    Link, Location, ResourceNotFoundException, Site, Tag, ValidationException,
    ValidationExceptionField,
};

/// Members of `GlobalNetworkState` / `SiteState` / `LinkState` this slice uses. All three
/// enums carry the same four values (`smithy_query shape=GlobalNetworkState,SiteState,LinkState`:
/// AVAILABLE, DELETING, PENDING, UPDATING), mirrored in
/// `ops::types::GLOBAL_NETWORK_STATE_VALUES` / `SITE_STATE_VALUES` / `LINK_STATE_VALUES`.
pub const STATE_PENDING: &str = "PENDING";
/// See [`STATE_PENDING`].
pub const STATE_AVAILABLE: &str = "AVAILABLE";
/// See [`STATE_PENDING`].
pub const STATE_DELETING: &str = "DELETING";

/// The state a freshly created CORE network is in. `CoreNetworkState` is a different enum
/// from the three above — CREATING, UPDATING, AVAILABLE, DELETING (`smithy_query
/// shape=CoreNetworkState`, mirrored in `ops::types::CORE_NETWORK_STATE_VALUES`) — so a
/// core network waits in CREATING where the resources above wait in [`STATE_PENDING`];
/// AVAILABLE and DELETING it shares with them.
pub const STATE_CREATING: &str = "CREATING";

/// The ARN namespace the model declares for this service (`arnNamespace: networkmanager`).
const ARN_NAMESPACE: &str = "networkmanager";

/// The resource kinds this slice mints: both the id prefix (`global-network-<hex>`,
/// `site-<hex>`, `link-<hex>`) and the ARN resource type (`global-network/<id>`,
/// `site/<id>`, `link/<id>`).
pub const GLOBAL_NETWORK_KIND: &str = "global-network";
/// See [`GLOBAL_NETWORK_KIND`].
pub const SITE_KIND: &str = "site";
/// See [`GLOBAL_NETWORK_KIND`].
pub const LINK_KIND: &str = "link";
/// See [`GLOBAL_NETWORK_KIND`].
pub const DEVICE_KIND: &str = "device";
/// See [`GLOBAL_NETWORK_KIND`]. The model constrains this one's id itself:
/// `CoreNetworkId` carries `pattern ^core-network-([0-9a-f]{8,17})$`
/// (`smithy_query shape=CoreNetworkId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
pub const CORE_NETWORK_KIND: &str = "core-network";

/// Network Manager is a global (partition-scoped) service, so its ARNs carry an empty
/// region segment: `arn:aws:networkmanager::<account>:global-network/<id>`.
const ARN_REGION: &str = "";

/// The `ServiceCode` every modelled `ServiceQuotaExceededException` this service raises
/// carries: the model's own `arnNamespace`/signing name for Network Manager.
pub const SERVICE_CODE: &str = ARN_NAMESPACE;

/// The `LimitCode` of the two quotas this slice enforces. The model types `LimitCode` as
/// a free-form string and enumerates no values (`smithy_query
/// shape=ServiceQuotaExceededException`), so each code names its own quota.
pub const LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT: &str = "global-networks-per-account";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_SITES_PER_GLOBAL_NETWORK: &str = "sites-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_LINKS_PER_SITE: &str = "links-per-site";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_DEVICES_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_DEVICES_PER_GLOBAL_NETWORK: &str = "devices-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_CORE_NETWORKS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_CORE_NETWORKS_PER_GLOBAL_NETWORK: &str = "core-networks-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota [`MAX_TAGS_PER_RESOURCE`] bounds.
pub const LIMIT_TAGS_PER_RESOURCE: &str = "tags-per-resource";

/// The message every `ResourceNotFoundException` this service raises carries.
///
/// The model declares the member required but states no text. Two independent recordings
/// of different resources through different operations — `UpdateGlobalNetwork` on a
/// deleted global-network id and `TagResource` on the ARN of a deleted site — both answer
/// the identical string, so the message is a fixed, resource-INDEPENDENT constant: the
/// identifying detail travels in the shape's required `ResourceId` and `ResourceType`,
/// never in the prose. Every not-found refusal of this service is built by
/// [`resource_not_found`], so no handler can reintroduce per-request text.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The modelled `ResourceNotFoundException` (HTTP 404) for the resource `kind` and `id`
/// name: the constant [`RESOURCE_NOT_FOUND_MESSAGE`] plus the identifying detail in
/// `ResourceId` / `ResourceType`.
///
/// `kind` is one of the ARN resource types this service mints ([`GLOBAL_NETWORK_KIND`],
/// [`SITE_KIND`], [`LINK_KIND`]), which is also the `ResourceType` AWS reports.
pub fn resource_not_found(kind: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: id.to_string(),
        resource_type: kind.to_string(),
        context: None,
    }
}

/// The field names this service reports in `ValidationException$Fields[].Name` for the
/// parent reference a create carries.
///
/// Network Manager names the offending REQUEST MEMBER in its own lowerCamelCase field
/// naming, not in the wire (PascalCase) spelling: a recording of `CreateSite` under an
/// unknown global network answers `globalNetworkId` and one of `CreateLink` with an
/// unknown site answers `siteId`. Both are `<kind>Id` for the kind the member references.
pub const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";
/// See [`FIELD_GLOBAL_NETWORK_ID`].
pub const FIELD_SITE_ID: &str = "siteId";

/// How a resource kind of this service reads in prose: the ARN resource type
/// ([`GLOBAL_NETWORK_KIND`] and friends) with its hyphens as spaces and a leading
/// capital, so `global-network` reads "Global network" and `site` reads "Site".
fn kind_label(kind: &str) -> String {
    let spaced = kind.replace('-', " ");
    let mut characters = spaced.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => spaced,
    }
}

/// The modelled `ValidationException` (HTTP 400) a create answers when one of the
/// references its input carries — the parent the new resource must hang off — names no
/// stored resource.
///
/// A create validates its input before anything is minted, so a reference that resolves
/// to nothing is a FIELD failure on the member that carried it rather than a missing
/// resource: `Reason` is `FieldValidationFailed` and `Fields` carries one entry naming
/// that member (see [`FIELD_GLOBAL_NETWORK_ID`]). The prose is the one the recordings
/// show — "<Kind> not found." for the kind the member references, from [`kind_label`] —
/// and is repeated as the field's own message, exactly as recorded.
///
/// `member` is the field name the refusal reports; `kind` is the resource kind the
/// member references ([`GLOBAL_NETWORK_KIND`], [`SITE_KIND`], [`CORE_NETWORK_KIND`]).
pub fn unknown_reference(member: &str, kind: &str) -> ValidationException {
    let message = format!("{} not found.", kind_label(kind));
    ValidationException {
        message: message.clone(),
        reason: Some("FieldValidationFailed".to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// Every value a `ResourceType` member of this service's network-resource operations
/// accepts, in lexicographic order so a paginated listing of them is stable.
///
/// The set is the enumeration in the member's own documentation (`smithy_query
/// query="doc" shape="GetNetworkResourceCountsRequest" member="ResourceType"`): three
/// Direct Connect types (`dxcon`, `dx-gateway`, `dx-vif`), eight Network Manager types
/// (`attachment`, `connect-peer`, `connection`, `core-network`, `device`, `link`,
/// `peering`, `site`) and six Amazon VPC types (`customer-gateway`, `transit-gateway`,
/// `transit-gateway-attachment`, `transit-gateway-connect-peer`,
/// `transit-gateway-route-table`, `vpn-connection`). The modelled `ConstrainedString`
/// carries only a length and a permissive pattern, so a value outside this set is refused
/// here rather than by the schema.
pub const NETWORK_RESOURCE_TYPES: [&str; 17] = [
    "attachment",
    "connect-peer",
    "connection",
    "core-network",
    "customer-gateway",
    "device",
    "dx-gateway",
    "dx-vif",
    "dxcon",
    "link",
    "peering",
    "site",
    "transit-gateway",
    "transit-gateway-attachment",
    "transit-gateway-connect-peer",
    "transit-gateway-route-table",
    "vpn-connection",
];

/// Whether `resource_type` is one of the values [`NETWORK_RESOURCE_TYPES`] lists.
pub fn is_network_resource_type(resource_type: &str) -> bool {
    NETWORK_RESOURCE_TYPES.contains(&resource_type)
}

/// The tag-key prefix AWS reserves for its own tags.
///
/// "Do not use the `aws:` prefix in your tag names or values because it is reserved for
/// AWS use. You can't edit or delete tag names or values with this prefix"
/// (AWS tagging documentation), so a request that asks to SET or REMOVE such a key is
/// refused and nothing is stored. The comparison is case-insensitive, as AWS's own
/// reserved-prefix check is.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The documented maximum length of a tag key, in characters: "Constraints: Maximum
/// length of 128 characters" on `Tag$Key`
/// (`smithy_query query="doc" shape="Tag" member="Key"`) — a constraint the modelled
/// `length 0..10000000` does not express, so it is checked here.
pub const MAX_TAG_KEY_CHARS: usize = 128;
/// The documented maximum length of a tag value, in characters: "Constraints: Maximum
/// length of 256 characters" on `Tag$Value`
/// (`smithy_query query="doc" shape="Tag" member="Value"`).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// How many tags one resource may carry: AWS's published tagging limit of 50 tags per
/// resource, which applies to every taggable resource. Asking for more is refused with
/// the operation's modelled `ServiceQuotaExceededException` (HTTP 402) and stores
/// nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many global networks one account may hold here, and how many sites one global
/// network may hold.
///
/// Network Manager publishes an account-level quota for both, adjustable per account, and
/// the model carries neither number, so these two are CHOSEN: their purpose is that a
/// caller cannot grow the emulator's store without bound. Reaching one is answered with
/// the operation's modelled `ServiceQuotaExceededException` (HTTP 402) and stores nothing,
/// exactly as exceeding a real quota does.
pub const MAX_GLOBAL_NETWORKS: usize = 100;
/// See [`MAX_GLOBAL_NETWORKS`].
pub const MAX_SITES_PER_GLOBAL_NETWORK: usize = 100;
/// How many links one site may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_LINKS_PER_SITE: usize = 100;
/// How many devices one global network may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_DEVICES_PER_GLOBAL_NETWORK: usize = 100;

/// The documented maximum length, in characters, of a device's `Vendor`, `Model` and
/// `SerialNumber`: "Constraints: Maximum length of 128 characters" on each of the three
/// (`smithy_query query="doc" shape="CreateDeviceRequest" member="Vendor"`, the same
/// sentence for `Model` and `SerialNumber`) — a bound the modelled `ConstrainedString`
/// (len 0..256) does not express, so it is checked by the handler instead.
pub const MAX_DEVICE_DESCRIPTOR_CHARS: usize = 128;

/// How many core networks one global network may hold.
///
/// Unlike the three bounds above this one is the SERVICE's number, not a chosen bound:
/// Cloud WAN gives a global network a single core network ("Each global network can have
/// one core network", AWS Cloud WAN documentation), so a second create against the same
/// global network is answered with the operation's modelled
/// `ServiceQuotaExceededException` (HTTP 402) and stores nothing.
pub const MAX_CORE_NETWORKS_PER_GLOBAL_NETWORK: usize = 1;

/// How long after the creating call a `PENDING` resource settles to `AVAILABLE`.
///
/// AWS documents no span for this transition, so the value is CHOSEN; the emulator
/// contract fixes a chosen settle span inside 250–750 ms and forbids zero, so a caller
/// that reads straight after the create still observes the transient `PENDING` state.
/// Nothing sleeps for it: the due instant is stored and compared against the clock.
pub const SETTLE_DELAY: Duration = Duration::from_millis(500);

/// A stored global network. `state` is not a field: it is a function of the clock
/// (see [`GlobalNetworkRecord::state_at`]), so reading one never writes.
#[derive(Debug, Clone)]
pub struct GlobalNetworkRecord {
    pub id: String,
    pub arn: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl GlobalNetworkRecord {
    /// This network's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `GlobalNetwork` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> GlobalNetwork {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `GlobalNetwork` for this record, forcing `state` — used by
    /// `DeleteGlobalNetwork`, which reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> GlobalNetwork {
        GlobalNetwork {
            global_network_id: Some(self.id.clone()),
            global_network_arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored site, back-referencing the global network that owns it.
#[derive(Debug, Clone)]
pub struct SiteRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub description: Option<String>,
    pub location: Option<Location>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl SiteRecord {
    /// This site's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `Site` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Site {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Site` for this record, forcing `state` — used by `DeleteSite`.
    pub fn to_shape_with_state(&self, state: &str) -> Site {
        Site {
            site_id: Some(self.id.clone()),
            site_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            description: self.description.clone(),
            location: self.location.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored link, back-referencing the site (and through it the global network) it
/// belongs to. Like the records above, `state` is a function of the clock.
#[derive(Debug, Clone)]
pub struct LinkRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub site_id: String,
    pub description: Option<String>,
    /// The link's `Type`, a free-form string the model does not enumerate.
    pub link_type: Option<String>,
    /// The upload/download speeds in Mbps, as the request supplied them.
    pub bandwidth: Bandwidth,
    pub provider: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl LinkRecord {
    /// This link's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `Link` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Link {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Link` for this record, forcing `state` — used by `DeleteLink`, which
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Link {
        Link {
            link_id: Some(self.id.clone()),
            link_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            site_id: Some(self.site_id.clone()),
            description: self.description.clone(),
            r#type: self.link_type.clone(),
            bandwidth: Some(self.bandwidth.clone()),
            provider: self.provider.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored device, back-referencing the global network that owns it and, when the
/// create named one, the site it sits at.
///
/// A device hangs off the GLOBAL NETWORK, not off a site: "Creates a new device in a
/// global network" (`smithy_query query="doc" shape="CreateDevice"`) and `SiteId` is
/// optional, so `site_id` is an `Option` where [`LinkRecord::site_id`] is required.
/// Like the records above, `state` is a function of the clock rather than a field.
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    /// The site this device sits at, when the create named one.
    pub site_id: Option<String>,
    /// The Amazon Web Services location (zone / subnet ARN) of the device, if any: "For
    /// an on-premises device, you can omit this parameter"
    /// (`smithy_query query="doc" shape="CreateDeviceRequest" member="AWSLocation"`).
    pub aws_location: Option<AWSLocation>,
    pub description: Option<String>,
    /// The device's `Type`, a free-form string the model does not enumerate.
    pub device_type: Option<String>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub location: Option<Location>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl DeviceRecord {
    /// This device's lifecycle state as of `now`. `DeviceState` carries the same four
    /// values as the three enums above (`smithy_query shape=DeviceState`: PENDING,
    /// AVAILABLE, DELETING, UPDATING), so a device waits in [`STATE_PENDING`] too.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `Device` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Device {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Device` for this record, forcing `state` — for an operation that
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Device {
        Device {
            device_id: Some(self.id.clone()),
            device_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            aws_location: self.aws_location.clone(),
            description: self.description.clone(),
            r#type: self.device_type.clone(),
            vendor: self.vendor.clone(),
            model: self.model.clone(),
            serial_number: self.serial_number.clone(),
            location: self.location.clone(),
            site_id: self.site_id.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored core network: the Cloud WAN managed network a global network owns.
///
/// Its `state` is a function of the clock, like the records above, and its topology
/// (`Segments`, `NetworkFunctionGroups`, `Edges`) is derived from the policy document
/// once, where that document is accepted — nothing about a core network is kept twice and
/// no read re-parses the document.
#[derive(Debug, Clone)]
pub struct CoreNetworkRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The LIVE core network policy document, verbatim as the request supplied it, or
    /// `None` when the core network was created without one.
    pub policy_document: Option<String>,
    /// The topology [`policy_topology`] derived from [`Self::policy_document`] when it was
    /// accepted: what the core network reports once its policy is in force.
    pub topology: PolicyTopology,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second one.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
}

impl CoreNetworkRecord {
    /// This core network's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_CREATING
        }
    }

    /// The modelled `CoreNetwork` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> CoreNetwork {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `CoreNetwork` for this record, forcing `state` — used by a delete,
    /// which reports the record it just removed as `DELETING`.
    ///
    /// A core network's segments and edges exist only once its policy is in force, which
    /// is what the `CREATING` -> `AVAILABLE` transition stands for, so before that instant
    /// the topology is reported empty and afterwards it is the one derived from the stored
    /// policy document.
    pub fn to_shape_with_state(&self, state: &str) -> CoreNetwork {
        let topology = if state == STATE_AVAILABLE {
            ::std::borrow::Cow::Borrowed(&self.topology)
        } else {
            ::std::borrow::Cow::Owned(PolicyTopology::default())
        };
        CoreNetwork {
            global_network_id: Some(self.global_network_id.clone()),
            core_network_id: Some(self.id.clone()),
            core_network_arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            segments: Some(topology.to_segments()),
            network_function_groups: Some(topology.to_network_function_groups()),
            edges: Some(topology.edges.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-networkmanager-networkmanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Global networks by `GlobalNetworkId`. A `BTreeMap`, so listing order is stable
    /// and a pagination token can name a key offset into it.
    pub global_networks: BTreeMap<String, GlobalNetworkRecord>,
    /// Sites by `SiteId`, each carrying its owning `GlobalNetworkId`.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Links by `LinkId`, each carrying the `SiteId` and `GlobalNetworkId` it belongs to.
    pub links: BTreeMap<String, LinkRecord>,
    /// Devices by `DeviceId`, each carrying its owning `GlobalNetworkId` and, when the
    /// create named one, the `SiteId` it sits at.
    pub devices: BTreeMap<String, DeviceRecord>,
    /// Core networks by `CoreNetworkId`, each carrying its owning `GlobalNetworkId`.
    pub core_networks: BTreeMap<String, CoreNetworkRecord>,
    /// Resource policies by the ARN of the resource they are attached to — the one store
    /// `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy` share, holding
    /// the document exactly as it was accepted so a reader reports what was put.
    ///
    /// Keyed by ARN rather than hung off a resource record: none of the three operations
    /// models a `ResourceNotFoundException` (`smithy_query query="operation"
    /// operation="GetResourcePolicy,PutResourcePolicy,DeleteResourcePolicy"`), unlike every
    /// other reader of a resource in this service, so the policy store does not assert
    /// that the ARN names a resource this account holds — only that it is an ARN of this
    /// service ([`parse_resource_arn`]). The store is bounded instead by its own quota:
    /// at most [`MAX_RESOURCE_POLICIES`] ARNs hold a policy, each document at most
    /// [`MAX_RESOURCE_POLICY_CHARS`] characters, so no caller can grow it without end.
    pub resource_policies: BTreeMap<String, String>,
    /// Monotonic source for generated resource-id suffixes, so ids are unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
}

impl ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Mint the next id for `prefix`, e.g. `global-network-00000000000000001`.
    ///
    /// Network Manager ids are `<kind>-<hex>` (the model's own patterns for the kinds
    /// that constrain them read `^core-network-([0-9a-f]{8,17})$`), so the suffix is 17
    /// lowercase hex digits of the monotonic counter.
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        format!("{prefix}-{:017x}", self.id_counter)
    }

    /// The ARN of a resource of this service: `arn:aws:networkmanager::<account>:<kind>/<id>`.
    pub fn resource_arn(kind: &str, id: &str) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("{kind}/{id}"))
    }

    /// How many sites `global_network_id` owns, for the per-global-network quota.
    pub fn site_count_of(&self, global_network_id: &str) -> usize {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .count()
    }

    /// The ids of the sites owned by `global_network_id`, in `SiteId` order.
    pub fn site_ids_of(&self, global_network_id: &str) -> Vec<String> {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .map(|site| site.id.clone())
            .collect()
    }

    /// How many links the global network `global_network_id` holds, across all its sites.
    pub fn link_count_of_network(&self, global_network_id: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.global_network_id == global_network_id)
            .count()
    }

    /// How many devices the global network `global_network_id` holds, for the
    /// per-global-network quota and the network-resource inventory.
    pub fn device_count_of(&self, global_network_id: &str) -> usize {
        self.devices
            .values()
            .filter(|device| device.global_network_id == global_network_id)
            .count()
    }

    /// The ids of the devices assigned to `site_id`, in `DeviceId` order.
    pub fn device_ids_of_site(&self, site_id: &str) -> Vec<String> {
        self.devices
            .values()
            .filter(|device| device.site_id.as_deref() == Some(site_id))
            .map(|device| device.id.clone())
            .collect()
    }

    /// How many core networks `global_network_id` owns, for the per-global-network quota.
    pub fn core_network_count_of(&self, global_network_id: &str) -> usize {
        self.core_networks
            .values()
            .filter(|core_network| core_network.global_network_id == global_network_id)
            .count()
    }

    /// The ids of the core networks owned by `global_network_id`, in `CoreNetworkId` order.
    pub fn core_network_ids_of(&self, global_network_id: &str) -> Vec<String> {
        self.core_networks
            .values()
            .filter(|core_network| core_network.global_network_id == global_network_id)
            .map(|core_network| core_network.id.clone())
            .collect()
    }

    /// How many resources of `resource_type` the global network `global_network_id`
    /// holds, for the network-resource inventory.
    ///
    /// `resource_type` must be one of [`NETWORK_RESOURCE_TYPES`] (the caller refuses
    /// anything else). Only the kinds this service stores can be non-zero; a kind whose
    /// creating operation is still a scaffold stub is held nowhere, so the count reported
    /// for it is the zero the store really holds, not a placeholder.
    pub fn network_resource_count(&self, global_network_id: &str, resource_type: &str) -> i32 {
        let held = match resource_type {
            SITE_KIND => self.site_count_of(global_network_id),
            LINK_KIND => self.link_count_of_network(global_network_id),
            DEVICE_KIND => self.device_count_of(global_network_id),
            CORE_NETWORK_KIND => self.core_network_count_of(global_network_id),
            _ => 0,
        };
        i32::try_from(held).unwrap_or(i32::MAX)
    }

    /// How many links `site_id` holds, for the per-site quota.
    pub fn link_count_of_site(&self, site_id: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .count()
    }

    /// The ids of the links attached to `site_id`, in `LinkId` order.
    pub fn link_ids_of_site(&self, site_id: &str) -> Vec<String> {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .map(|link| link.id.clone())
            .collect()
    }

    /// The stored tag list of the resource `arn` names, or `None` when this service holds
    /// no resource under that ARN.
    ///
    /// Tags live on the resource record itself, so every reader of a resource
    /// (`DescribeGlobalNetworks`, `GetSites`, `GetLinks`, `GetDevices`,
    /// `ListTagsForResource`) answers from the one stored list.
    pub fn tags_of(&self, arn: &str) -> Option<&Vec<Tag>> {
        let parts = parse_resource_arn(arn)?;
        match parts.kind {
            GLOBAL_NETWORK_KIND => self
                .global_networks
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            SITE_KIND => self
                .sites
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            LINK_KIND => self
                .links
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            DEVICE_KIND => self
                .devices
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            CORE_NETWORK_KIND => self
                .core_networks
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            _ => None,
        }
    }

    /// The stored tag list of the resource `arn` names, for writing. See [`Self::tags_of`].
    pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
        let parts = parse_resource_arn(arn)?;
        match parts.kind {
            GLOBAL_NETWORK_KIND => self
                .global_networks
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            SITE_KIND => self
                .sites
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            LINK_KIND => self
                .links
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            DEVICE_KIND => self
                .devices
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            CORE_NETWORK_KIND => self
                .core_networks
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            _ => None,
        }
    }
}

/// The resource an ARN of this service names: the ARN resource type (which is also the
/// resource-id prefix this service mints, see [`GLOBAL_NETWORK_KIND`]) and the id.
#[derive(Debug, Clone, Copy)]
pub struct ArnParts<'a> {
    /// The ARN resource type, e.g. `global-network`.
    pub kind: &'a str,
    /// The resource id inside that type.
    pub id: &'a str,
}

/// Split a Network Manager resource ARN into its kind and id.
///
/// Every ARN this service mints reads `arn:<partition>:networkmanager::<account>:<kind>/<id>`
/// (see [`ComAmazonawsNetworkmanagerNetworkmanagerState::resource_arn`]). Answers `None`
/// when the string is not an ARN of this service in that `<kind>/<id>` form at all — the
/// caller refuses that as a `ValidationException` on the ARN member, which is a different
/// answer from a well-formed ARN naming a resource that does not exist.
///
/// Only the form is judged here, not which account, partition or region the ARN carries:
/// an ARN of this service that names another account's resource, or carries a region this
/// global service does not use, parses and then matches no stored record, so the caller
/// answers it as NOT FOUND rather than as a malformed request.
pub fn parse_resource_arn(arn: &str) -> Option<ArnParts<'_>> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // Partition and account are always present in an ARN; the region segment of this
    // global service is empty, so it is the only one allowed to be.
    if fields.next()?.is_empty() {
        return None;
    }
    if fields.next()? != ARN_NAMESPACE {
        return None;
    }
    let _region = fields.next()?;
    if fields.next()?.is_empty() {
        return None;
    }
    let (kind, id) = fields.next()?.split_once('/')?;
    if kind.is_empty() || id.is_empty() {
        return None;
    }
    Some(ArnParts { kind, id })
}

/// The `Reason` a `ValidationException` of this service carries when one input MEMBER is
/// at fault, and the one it carries when the member's text could not be parsed at all.
/// Both are values the modelled enumeration lists
/// (`ops::types::VALIDATION_EXCEPTION_REASON_VALUES`: `CannotParse`,
/// `FieldValidationFailed`, `Other`, `UnknownOperation`).
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";
/// See [`REASON_FIELD_VALIDATION_FAILED`].
pub const REASON_CANNOT_PARSE: &str = "CannotParse";

/// The input member the resource-policy operations name in a refusal: the ARN of the
/// resource whose policy is addressed, and the policy document itself.
pub const FIELD_RESOURCE_ARN: &str = "ResourceArn";
/// See [`FIELD_RESOURCE_ARN`].
pub const FIELD_POLICY_DOCUMENT: &str = "PolicyDocument";

/// The modelled `ValidationException` (HTTP 400) for a refusal of the single input member
/// `member`, carrying `reason` and one `Fields` entry explaining `message`.
pub fn field_rejection(member: &str, reason: &str, message: String) -> ValidationException {
    ValidationException {
        message: message.clone(),
        reason: Some(reason.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// The modelled `ValidationException` (HTTP 400) an operation answers when the string in
/// its ARN member `member` is not an ARN of this service at all.
///
/// A string that does not parse as `arn:<partition>:networkmanager::<account>:<type>/<id>`
/// (see [`parse_resource_arn`]) names no resource of this service, which is a FIELD
/// failure on the member that carried it — a different answer from a well-formed ARN that
/// names nothing stored.
///
/// The wording is the one the tagging operations of this service already answer for the
/// same input class (`ops/list_tags_for_resource.rs`, `ops/tag_resource.rs`,
/// `ops/untag_resource.rs`), so one malformed ARN reads the same whichever operation it
/// arrived on.
pub fn malformed_resource_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not a networkmanager resource ARN"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: "must read arn:<partition>:networkmanager::<account>:<type>/<id>".to_string(),
        }]),
    }
}

/// How many resource policies this account may hold here, and how long one policy
/// document may be, in characters.
///
/// Both are CHOSEN, like [`MAX_GLOBAL_NETWORKS`], and for the same reason: the
/// `resource_policies` store is keyed by the ARN a request carries, so without a bound a
/// caller could grow this emulator's store without end. AWS publishes an account-level
/// resource-policy quota (adjustable per account) and a per-policy size limit for its
/// resource-based policies, and the model carries neither number — the document is an
/// opaque `SynthesizedJsonResourcePolicyDocument` of length 0..10000000 — so the numbers
/// here are the emulator's own: 20,480 characters is the size AWS's resource-based policy
/// limits are commonly stated at, well inside the modelled maximum.
///
/// Asking to store a policy for one more ARN than [`MAX_RESOURCE_POLICIES`] is refused
/// with `PutResourcePolicy`'s modelled `ServiceQuotaExceededException` (HTTP 402) and
/// stores nothing; replacing the policy of an ARN already held never trips it. A document
/// longer than [`MAX_RESOURCE_POLICY_CHARS`] is refused with the same operation's
/// `ValidationException` (HTTP 400), naming [`FIELD_POLICY_DOCUMENT`].
pub const MAX_RESOURCE_POLICIES: usize = 100;
/// See [`MAX_RESOURCE_POLICIES`].
pub const MAX_RESOURCE_POLICY_CHARS: usize = 20_480;

/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota [`MAX_RESOURCE_POLICIES`] bounds.
pub const LIMIT_RESOURCE_POLICIES_PER_ACCOUNT: &str = "resource-policies-per-account";

/// The message a refusal past the [`MAX_RESOURCE_POLICIES`] quota carries: `resulting` is
/// how many ARNs would hold a policy if the request were accepted.
pub fn too_many_resource_policies_message(resulting: usize) -> String {
    format!(
        "an account may hold resource policies for at most {MAX_RESOURCE_POLICIES} resources, but the request would leave {resulting}"
    )
}

/// The members of a resource policy document this service judges.
///
/// A resource policy is an IAM JSON policy document, so its grammar is IAM's (AWS IAM
/// User Guide, "IAM JSON policy element reference"): the document is a JSON object whose
/// `Statement` is one statement object or a list of them, and every statement names its
/// `Effect`, which is either `Allow` or `Deny`. The model types the document as an opaque
/// string carrying `mediaType: application/json` (`smithy_query
/// shape=SynthesizedJsonResourcePolicyDocument`), so these names come from that grammar
/// rather than from the model.
const POLICY_STATEMENT: &str = "Statement";
/// See [`POLICY_STATEMENT`].
const POLICY_EFFECT: &str = "Effect";
/// See [`POLICY_STATEMENT`]: the two values IAM's `Effect` element allows.
pub const POLICY_EFFECT_VALUES: [&str; 2] = ["Allow", "Deny"];

/// Check the document a `PutResourcePolicy` asks to attach to a resource.
///
/// The document's SIZE is judged first, against [`MAX_RESOURCE_POLICY_CHARS`], so an
/// over-long document is refused before it is parsed. Then its SHAPE, against the IAM
/// policy grammar ([`POLICY_STATEMENT`]): it must parse as JSON, its root must be an
/// object, it must carry at least one statement, and every statement must be an object
/// naming an `Effect` of `Allow` or `Deny`. Whom the statements grant what is the deeper
/// semantics of a policy and is not interpreted here — this service only stores and
/// reports the document it accepted.
///
/// A document that fails any of these is refused with a `ValidationException` (HTTP 400)
/// naming [`FIELD_POLICY_DOCUMENT`] and nothing is stored, so a rejected request leaves
/// the resource's policy exactly as it was.
pub fn validate_resource_policy_document(document: &str) -> Result<(), ValidationException> {
    let reject = |reason: &str, message: String| {
        field_rejection(FIELD_POLICY_DOCUMENT, reason, message)
    };

    let length = document.chars().count();
    if length > MAX_RESOURCE_POLICY_CHARS {
        return Err(reject(
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "a resource policy document must be at most {MAX_RESOURCE_POLICY_CHARS} characters, but {length} were given"
            ),
        ));
    }

    let parsed: ::serde_json::Value = ::serde_json::from_str(document).map_err(|problem| {
        reject(
            REASON_CANNOT_PARSE,
            format!("the resource policy document is not valid JSON: {problem}"),
        )
    })?;
    let root = parsed.as_object().ok_or_else(|| {
        reject(
            REASON_FIELD_VALIDATION_FAILED,
            "the resource policy document must be a JSON object".to_string(),
        )
    })?;

    // IAM allows `Statement` to be a single statement object as well as a list of them.
    let statements: Vec<&::serde_json::Value> = match root.get(POLICY_STATEMENT) {
        Some(::serde_json::Value::Array(entries)) => entries.iter().collect(),
        Some(single @ ::serde_json::Value::Object(_)) => vec![single],
        Some(_) => {
            return Err(reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!("{POLICY_STATEMENT} must be a statement object or a list of them"),
            ));
        }
        None => {
            return Err(reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!("the resource policy document must carry a {POLICY_STATEMENT}"),
            ));
        }
    };
    if statements.is_empty() {
        return Err(reject(
            REASON_FIELD_VALIDATION_FAILED,
            format!("{POLICY_STATEMENT} must carry at least one statement"),
        ));
    }

    for (index, statement) in statements.iter().enumerate() {
        let entry = statement.as_object().ok_or_else(|| {
            reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!("{POLICY_STATEMENT}[{index}] must be a JSON object"),
            )
        })?;
        let effect = entry
            .get(POLICY_EFFECT)
            .and_then(::serde_json::Value::as_str);
        if !effect.is_some_and(|effect| POLICY_EFFECT_VALUES.contains(&effect)) {
            return Err(reject(
                REASON_FIELD_VALIDATION_FAILED,
                format!(
                    "{POLICY_STATEMENT}[{index}].{POLICY_EFFECT} must be one of {}",
                    POLICY_EFFECT_VALUES.join(", ")
                ),
            ));
        }
    }

    Ok(())
}

/// Why a requested tag set or key list was refused: the input member to blame and the
/// reason, which the caller reports as a `ValidationException` (HTTP 400).
#[derive(Debug, Clone)]
pub struct TagRejection {
    /// The input member the refusal names, e.g. `Tags`.
    pub member: &'static str,
    /// Why that member is not acceptable.
    pub message: String,
}

impl TagRejection {
    fn new(member: &'static str, message: String) -> Self {
        Self { member, message }
    }

    /// The modelled `ValidationException` this rejection reports. Every caller lifts the
    /// same exception into its own operation error enum, so the refusal reads the same
    /// whichever operation the tags arrived on.
    pub fn to_exception(&self) -> ValidationException {
        ValidationException {
            message: self.message.clone(),
            reason: Some("FieldValidationFailed".to_string()),
            fields: Some(vec![ValidationExceptionField {
                name: self.member.to_string(),
                message: self.message.clone(),
            }]),
        }
    }
}

/// Why the `Tags` a request asks to store were refused.
#[derive(Debug, Clone)]
pub enum TagProblem {
    /// A tag breaks a rule every tag obeys; the caller answers [`TagRejection::to_exception`].
    Invalid(TagRejection),
    /// The request carries more distinct keys than [`MAX_TAGS_PER_RESOURCE`] (this many),
    /// so the caller answers its modelled `ServiceQuotaExceededException`.
    TooMany(usize),
}

/// The checks every operation that STORES a `Tags` member runs before storing anything:
/// the per-tag rules of [`validate_tags`], then the per-resource tag quota.
///
/// Shared by the creates that carry tags, so one rule change reaches all of them; the
/// caller only lifts the problem into its own error enum.
pub fn check_requested_tags(tags: Option<&Vec<Tag>>) -> Result<(), TagProblem> {
    let tags = tags.map_or(&[][..], Vec::as_slice);
    validate_tags(tags).map_err(TagProblem::Invalid)?;
    let count = tag_count_after_merge(&[], tags);
    if count > MAX_TAGS_PER_RESOURCE {
        return Err(TagProblem::TooMany(count));
    }
    Ok(())
}

/// The message a refusal past the per-resource tag quota carries, shared by every
/// operation that stores tags: `resulting` is how many tags the resource would carry if
/// the request were accepted.
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "a resource may carry at most {MAX_TAGS_PER_RESOURCE} tags, but the request would leave {resulting}"
    )
}

/// Check a tag set a request asks to APPLY (`TagResource`, and the `Tags` of a create).
///
/// Refuses an empty or over-long key, an over-long value, and any key carrying the
/// AWS-reserved prefix. A key repeated inside one request is NOT an error: tag keys are
/// unique per resource, so the last value for a key wins (see [`merge_tags`]).
pub fn validate_tags(tags: &[Tag]) -> Result<(), TagRejection> {
    const MEMBER: &str = "Tags";
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        check_tag_key(MEMBER, key)?;
        let value = tag.value.as_deref().unwrap_or_default();
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_CHARS {
            return Err(TagRejection::new(
                MEMBER,
                format!(
                    "the value of tag key {key:?} must be at most {MAX_TAG_VALUE_CHARS} characters, but is {length}"
                ),
            ));
        }
    }
    Ok(())
}

/// Check a key list a request asks to REMOVE (`UntagResource`). A key the resource does
/// not carry is not an error — removal is idempotent — but a key AWS reserves cannot be
/// removed any more than it can be set.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), TagRejection> {
    const MEMBER: &str = "TagKeys";
    for key in keys {
        check_tag_key(MEMBER, key)?;
    }
    Ok(())
}

/// The rules every tag key obeys, whichever member carries it.
fn check_tag_key(member: &'static str, key: &str) -> Result<(), TagRejection> {
    if key.is_empty() {
        return Err(TagRejection::new(
            member,
            "a tag key must not be empty".to_string(),
        ));
    }
    let length = key.chars().count();
    if length > MAX_TAG_KEY_CHARS {
        return Err(TagRejection::new(
            member,
            format!("a tag key must be at most {MAX_TAG_KEY_CHARS} characters, but {length} were given"),
        ));
    }
    if key.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
        return Err(TagRejection::new(
            member,
            format!(
                "tag key {key:?} uses the {RESERVED_TAG_PREFIX} prefix, which is reserved for AWS"
            ),
        ));
    }
    Ok(())
}

/// How many tags the resource would carry once `requested` is merged into `existing`,
/// for the [`MAX_TAGS_PER_RESOURCE`] quota: a key already held is replaced, not added.
pub fn tag_count_after_merge(existing: &[Tag], requested: &[Tag]) -> usize {
    let mut keys: BTreeSet<&str> = existing.iter().filter_map(|tag| tag.key.as_deref()).collect();
    keys.extend(requested.iter().filter_map(|tag| tag.key.as_deref()));
    keys.len()
}

/// Merge `requested` into the resource's stored tags.
///
/// Tag keys are unique per resource, so a key already held keeps its position and takes
/// the requested value and a new key is appended — which is what makes a repeated
/// `TagResource` overwrite rather than duplicate.
pub fn merge_tags(existing: &mut Vec<Tag>, requested: Vec<Tag>) {
    for tag in requested {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
            None => existing.push(tag),
        }
    }
}

/// Remove every tag whose key `keys` names. A key the resource does not carry is ignored,
/// so removing twice answers the same as removing once.
pub fn remove_tags(existing: &mut Vec<Tag>, keys: &[String]) {
    existing.retain(|held| {
        held.key
            .as_deref()
            .is_none_or(|key| !keys.iter().any(|wanted| wanted == key))
    });
}

/// The keys of a core network policy document this service reads.
///
/// A core network policy is a JSON document whose schema AWS publishes (Cloud WAN "Core
/// network policy" documentation): a `version`, a `core-network-configuration` carrying
/// `asn-ranges` and `edge-locations`, a `segments` list and a `network-function-groups`
/// list. The model types the document as an opaque string (`CoreNetworkPolicyDocument`,
/// len 0..10000000, `smithy_query shape=CoreNetworkPolicyDocument`), so these names come
/// from that published schema rather than from the model.
const POLICY_VERSION: &str = "version";
/// See [`POLICY_VERSION`].
const POLICY_CONFIGURATION: &str = "core-network-configuration";
/// See [`POLICY_VERSION`].
const POLICY_ASN_RANGES: &str = "asn-ranges";
/// See [`POLICY_VERSION`].
const POLICY_EDGE_LOCATIONS: &str = "edge-locations";
/// See [`POLICY_VERSION`].
const POLICY_LOCATION: &str = "location";
/// See [`POLICY_VERSION`].
const POLICY_ASN: &str = "asn";
/// See [`POLICY_VERSION`].
const POLICY_INSIDE_CIDR_BLOCKS: &str = "inside-cidr-blocks";
/// See [`POLICY_VERSION`].
const POLICY_SEGMENTS: &str = "segments";
/// See [`POLICY_VERSION`].
const POLICY_NETWORK_FUNCTION_GROUPS: &str = "network-function-groups";
/// See [`POLICY_VERSION`].
const POLICY_NAME: &str = "name";

/// How many entries of one list a policy document may carry.
///
/// The derived topology is the PRODUCT of the edge-location count and the segment (or
/// network-function-group) count — a segment that restricts itself to no edge location
/// exists in all of them — so a document whose lists are unbounded would expand into a
/// response orders of magnitude larger than the request. These three caps are CHOSEN,
/// like [`MAX_GLOBAL_NETWORKS`]: AWS's own Cloud WAN quotas for edge locations and
/// segments per core network are in the tens, the model carries no number (the document
/// is an opaque `CoreNetworkPolicyDocument`, len 0..10000000), and their purpose here is
/// that one caller cannot make this emulator hold or compute without bound. A document
/// past a cap is refused with `CoreNetworkPolicyException` (HTTP 400) and stored nowhere,
/// so the derived topology is at most
/// `MAX_POLICY_EDGE_LOCATIONS * (MAX_POLICY_SEGMENTS + MAX_POLICY_NETWORK_FUNCTION_GROUPS)`
/// edge-location strings.
pub const MAX_POLICY_EDGE_LOCATIONS: usize = 100;
/// See [`MAX_POLICY_EDGE_LOCATIONS`].
pub const MAX_POLICY_SEGMENTS: usize = 100;
/// See [`MAX_POLICY_EDGE_LOCATIONS`].
pub const MAX_POLICY_NETWORK_FUNCTION_GROUPS: usize = 100;

/// How long one value inside a policy document may be, in characters.
///
/// These two are the MODEL's own numbers, not chosen: the shapes the derived topology is
/// reported through constrain them. An edge location is reported as
/// `CoreNetworkEdge$EdgeLocation`, an `ExternalRegionCode` of length 1..63, and a segment
/// or group name as a `ConstrainedString` of length 0..256 (`smithy_query
/// shape=CoreNetworkEdge,CoreNetworkSegment`). Capping them is also what bounds the
/// derived topology in BYTES, not merely in entries: at most
/// `MAX_POLICY_EDGE_LOCATIONS * MAX_POLICY_LOCATION_CHARS` characters of location text,
/// expanded into at most `MAX_POLICY_SEGMENTS + MAX_POLICY_NETWORK_FUNCTION_GROUPS`
/// entries, however large the document that carried them.
pub const MAX_POLICY_LOCATION_CHARS: usize = 63;
/// See [`MAX_POLICY_LOCATION_CHARS`].
pub const MAX_POLICY_NAME_CHARS: usize = 256;

/// The ASN domain an `asn-ranges` entry has to lie inside: a BGP AS number is a 32-bit
/// number and AS 0 is reserved, so the assignable numbers are 1..=4294967295. A range
/// reaching outside it assigns nothing (see [`parse_asn_range`]).
const ASN_MIN: i64 = 1;
/// See [`ASN_MIN`].
const ASN_MAX: i64 = 4_294_967_295;

/// The `ErrorCode` each policy refusal carries. The model types
/// `CoreNetworkPolicyError$ErrorCode` as a free-form string and enumerates no values
/// (`smithy_query shape=CoreNetworkPolicyError`), so each code names its own rule and the
/// identifying detail travels in the shape's `Path` (the JSON path of the offending node).
pub const POLICY_ERROR_INVALID_JSON: &str = "INVALID_JSON";
/// See [`POLICY_ERROR_INVALID_JSON`].
pub const POLICY_ERROR_MISSING_FIELD: &str = "MISSING_REQUIRED_FIELD";
/// See [`POLICY_ERROR_INVALID_JSON`].
pub const POLICY_ERROR_INVALID_TYPE: &str = "INVALID_TYPE";
/// See [`POLICY_ERROR_INVALID_JSON`]; raised past one of the [`MAX_POLICY_EDGE_LOCATIONS`]
/// list caps.
pub const POLICY_ERROR_TOO_MANY_ENTRIES: &str = "TOO_MANY_ENTRIES";
/// See [`POLICY_ERROR_INVALID_JSON`]; raised for a value outside the length the shape it
/// is reported through allows ([`MAX_POLICY_LOCATION_CHARS`],
/// [`MAX_POLICY_NAME_CHARS`]).
pub const POLICY_ERROR_INVALID_VALUE: &str = "INVALID_VALUE";

/// One entry of a `CoreNetworkPolicyException`'s `Errors` list.
fn policy_error(code: &str, message: String, path: &str) -> CoreNetworkPolicyError {
    CoreNetworkPolicyError {
        error_code: code.to_string(),
        message,
        path: Some(path.to_string()),
    }
}

/// Check the structure of a policy document a request asks to store.
///
/// Only the document's SHAPE is judged here, which is what every operation that accepts a
/// policy document has to agree on: it must parse as JSON, its root must be an object,
/// the `version` and `core-network-configuration` the schema requires must be present and
/// of the right type, and a `segments` list, when given, must be a list of objects that
/// each name themselves, and no list may carry more entries than its cap allows, so a
/// stored document expands into a bounded topology. The deeper semantics of the document
/// (which segments may share routes, which attachments map where) belong to the operations
/// that act on a policy.
///
/// Every problem found is reported, not just the first, which is why the answer is a list:
/// `CoreNetworkPolicyException` carries one entry per offending node.
pub fn validate_policy_document(document: &str) -> Result<(), Vec<CoreNetworkPolicyError>> {
    let parsed: ::serde_json::Value = match ::serde_json::from_str(document) {
        Ok(parsed) => parsed,
        Err(problem) => {
            return Err(vec![policy_error(
                POLICY_ERROR_INVALID_JSON,
                format!("the policy document is not valid JSON: {problem}"),
                "$",
            )]);
        }
    };
    let Some(root) = parsed.as_object() else {
        return Err(vec![policy_error(
            POLICY_ERROR_INVALID_TYPE,
            "the policy document must be a JSON object".to_string(),
            "$",
        )]);
    };

    let mut errors = Vec::new();
    for (key, wanted, holds) in [
        (
            POLICY_VERSION,
            "a string",
            ::serde_json::Value::is_string as fn(&::serde_json::Value) -> bool,
        ),
        (
            POLICY_CONFIGURATION,
            "an object",
            ::serde_json::Value::is_object as fn(&::serde_json::Value) -> bool,
        ),
    ] {
        match root.get(key) {
            None => errors.push(policy_error(
                POLICY_ERROR_MISSING_FIELD,
                format!("the policy document must carry {key:?}"),
                &format!("$.{key}"),
            )),
            Some(value) if !holds(value) => errors.push(policy_error(
                POLICY_ERROR_INVALID_TYPE,
                format!("{key:?} must be {wanted}"),
                &format!("$.{key}"),
            )),
            Some(_) => {}
        }
    }

    match root.get(POLICY_SEGMENTS) {
        None => {}
        Some(::serde_json::Value::Array(segments)) => {
            // `.take` bounds how many problems one document can be reported for: a list
            // longer than its cap is refused by the cap check below, so there is nothing
            // to gain from walking the rest of it.
            for (index, segment) in segments.iter().enumerate().take(MAX_POLICY_SEGMENTS) {
                if segment
                    .get(POLICY_NAME)
                    .and_then(::serde_json::Value::as_str)
                    .is_none()
                {
                    errors.push(policy_error(
                        POLICY_ERROR_MISSING_FIELD,
                        format!("every entry of {POLICY_SEGMENTS:?} must carry a {POLICY_NAME:?} string"),
                        &format!("$.{POLICY_SEGMENTS}[{index}].{POLICY_NAME}"),
                    ));
                }
            }
        }
        Some(_) => errors.push(policy_error(
            POLICY_ERROR_INVALID_TYPE,
            format!("{POLICY_SEGMENTS:?} must be an array"),
            &format!("$.{POLICY_SEGMENTS}"),
        )),
    }

    // The caps that bound the derived topology. Each is checked on the list's own length,
    // so a document past one is refused before anything is stored or expanded.
    let edge_locations = root
        .get(POLICY_CONFIGURATION)
        .and_then(::serde_json::Value::as_object)
        .and_then(|configuration| configuration.get(POLICY_EDGE_LOCATIONS))
        .and_then(::serde_json::Value::as_array)
        .map_or(0, Vec::len);
    for (path, held, cap) in [
        (
            format!("$.{POLICY_CONFIGURATION}.{POLICY_EDGE_LOCATIONS}"),
            edge_locations,
            MAX_POLICY_EDGE_LOCATIONS,
        ),
        (
            format!("$.{POLICY_SEGMENTS}"),
            root.get(POLICY_SEGMENTS)
                .and_then(::serde_json::Value::as_array)
                .map_or(0, Vec::len),
            MAX_POLICY_SEGMENTS,
        ),
        (
            format!("$.{POLICY_NETWORK_FUNCTION_GROUPS}"),
            root.get(POLICY_NETWORK_FUNCTION_GROUPS)
                .and_then(::serde_json::Value::as_array)
                .map_or(0, Vec::len),
            MAX_POLICY_NETWORK_FUNCTION_GROUPS,
        ),
    ] {
        if held > cap {
            errors.push(policy_error(
                POLICY_ERROR_TOO_MANY_ENTRIES,
                format!("{path} carries {held} entries, but at most {cap} are allowed"),
                &path,
            ));
        }
    }

    // The per-value lengths the shapes this topology is reported through allow. Capping
    // them is what bounds the derived topology in BYTES as well as in entries.
    for (index, entry) in root
        .get(POLICY_CONFIGURATION)
        .and_then(::serde_json::Value::as_object)
        .and_then(|configuration| configuration.get(POLICY_EDGE_LOCATIONS))
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .enumerate()
        .take(MAX_POLICY_EDGE_LOCATIONS)
    {
        let path =
            format!("$.{POLICY_CONFIGURATION}.{POLICY_EDGE_LOCATIONS}[{index}].{POLICY_LOCATION}");
        match entry
            .get(POLICY_LOCATION)
            .and_then(::serde_json::Value::as_str)
        {
            None => errors.push(policy_error(
                POLICY_ERROR_MISSING_FIELD,
                format!(
                    "every entry of {POLICY_EDGE_LOCATIONS:?} must carry a {POLICY_LOCATION:?} string"
                ),
                &path,
            )),
            Some(location) => {
                let length = location.chars().count();
                if length == 0 || length > MAX_POLICY_LOCATION_CHARS {
                    errors.push(policy_error(
                        POLICY_ERROR_INVALID_VALUE,
                        format!(
                            "a {POLICY_LOCATION:?} must be 1 to {MAX_POLICY_LOCATION_CHARS} characters, but {length} were given"
                        ),
                        &path,
                    ));
                }
            }
        }
    }

    for (key, cap) in [
        (POLICY_SEGMENTS, MAX_POLICY_SEGMENTS),
        (
            POLICY_NETWORK_FUNCTION_GROUPS,
            MAX_POLICY_NETWORK_FUNCTION_GROUPS,
        ),
    ] {
        for (index, entry) in root
            .get(key)
            .and_then(::serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .enumerate()
            .take(cap)
        {
            let Some(name) = entry.get(POLICY_NAME).and_then(::serde_json::Value::as_str) else {
                continue;
            };
            let length = name.chars().count();
            if length > MAX_POLICY_NAME_CHARS {
                errors.push(policy_error(
                    POLICY_ERROR_INVALID_VALUE,
                    format!(
                        "a {POLICY_NAME:?} must be at most {MAX_POLICY_NAME_CHARS} characters, but {length} were given"
                    ),
                    &format!("$.{key}[{index}].{POLICY_NAME}"),
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The modelled `CoreNetworkPolicyException` (HTTP 400) reporting the problems
/// [`validate_policy_document`] found, so every operation that takes a policy document
/// refuses a bad one the same way.
pub fn policy_rejection(errors: Vec<CoreNetworkPolicyError>) -> CoreNetworkPolicyException {
    CoreNetworkPolicyException {
        message: format!(
            "the core network policy document is not valid: {} problem(s) found",
            errors.len()
        ),
        errors: Some(errors),
    }
}

/// What a `CoreNetwork` reports about the policy in force on it, kept in the COMPACT form
/// the document describes: a segment or group that restricts itself to no edge location is
/// held as "every edge location" rather than as a copy of the whole location list, so the
/// stored topology is linear in the document it came from and only a response expands it.
#[derive(Debug, Clone, Default)]
pub struct PolicyTopology {
    /// One entry per `core-network-configuration.edge-locations[]` of the document.
    pub edges: Vec<CoreNetworkEdge>,
    /// One entry per named `segments[]` of the document.
    pub segments: Vec<NamedGroup>,
    /// One entry per named `network-function-groups[]` of the document.
    pub network_function_groups: Vec<NamedGroup>,
}

/// One named `segments[]` / `network-function-groups[]` entry of a policy document.
#[derive(Debug, Clone)]
pub struct NamedGroup {
    /// The entry's `name`.
    pub name: String,
    /// The edge locations the entry restricts itself to, or `None` for "every edge
    /// location this core network has" — the case a stored copy would duplicate.
    pub edge_locations: Option<Vec<String>>,
}

impl PolicyTopology {
    /// The edge locations this core network has, in the order the document configures
    /// them: what an entry restricting itself to none is present in.
    fn everywhere(&self) -> Vec<String> {
        self.edges
            .iter()
            .filter_map(|edge| edge.edge_location.clone())
            .collect()
    }

    /// The modelled `Segments` of this topology.
    pub fn to_segments(&self) -> Vec<CoreNetworkSegment> {
        let everywhere = self.everywhere();
        self.segments
            .iter()
            .map(|group| CoreNetworkSegment {
                name: Some(group.name.clone()),
                edge_locations: Some(
                    group
                        .edge_locations
                        .clone()
                        .unwrap_or_else(|| everywhere.clone()),
                ),
                // Which segments a segment shares routes with is decided by the
                // document's segment actions, which no operation of this slice acts on,
                // so the member is absent rather than guessed.
                shared_segments: None,
            })
            .collect()
    }

    /// The modelled `NetworkFunctionGroups` of this topology.
    pub fn to_network_function_groups(&self) -> Vec<CoreNetworkNetworkFunctionGroup> {
        let everywhere = self.everywhere();
        self.network_function_groups
            .iter()
            .map(|group| CoreNetworkNetworkFunctionGroup {
                name: Some(group.name.clone()),
                edge_locations: Some(
                    group
                        .edge_locations
                        .clone()
                        .unwrap_or_else(|| everywhere.clone()),
                ),
                // As with `shared_segments` above: a group's send-via / send-to segments
                // come from the document's segment actions, which this slice does not act
                // on.
                segments: None,
            })
            .collect()
    }
}

/// Derive the reported topology of a core network from its policy document.
///
/// The rules are the published policy schema's own: an edge exists in each location the
/// `core-network-configuration` lists, carrying that entry's `asn` — or, where the entry
/// names none, the next ASN of the configuration's `asn-ranges` that no entry claims, as
/// Cloud WAN assigns one from the ranges — and its `inside-cidr-blocks`; a segment exists
/// for each named entry of `segments`, present in the edge locations it restricts itself
/// to or, where it names none, in every edge location of the core network; and the same
/// location rule holds for each named entry of `network-function-groups`.
///
/// A document this service cannot parse describes no topology, so it answers the empty
/// one rather than guessing: a document is only stored once
/// [`validate_policy_document`] has accepted it, which also caps every list this walks,
/// so the work and the result are bounded by those caps.
///
/// Called ONCE, where the document is accepted, and the answer is stored on the record
/// beside it: a read of a core network reports the derived topology and never re-parses
/// the document.
pub fn policy_topology(document: &str) -> PolicyTopology {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return PolicyTopology::default();
    };
    let Some(root) = parsed.as_object() else {
        return PolicyTopology::default();
    };
    let configuration = root
        .get(POLICY_CONFIGURATION)
        .and_then(::serde_json::Value::as_object);
    let declared = configuration
        .and_then(|configuration| configuration.get(POLICY_EDGE_LOCATIONS))
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let ranges = configuration
        .and_then(|configuration| configuration.get(POLICY_ASN_RANGES))
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    let mut pool = AsnPool::new(ranges, declared);
    let edges: Vec<CoreNetworkEdge> = declared
        .iter()
        .filter_map(|entry| {
            let location = entry
                .get(POLICY_LOCATION)
                .and_then(::serde_json::Value::as_str)?;
            let asn = entry
                .get(POLICY_ASN)
                .and_then(::serde_json::Value::as_i64)
                .or_else(|| pool.take());
            Some(CoreNetworkEdge {
                edge_location: Some(location.to_string()),
                asn,
                inside_cidr_blocks: entry
                    .get(POLICY_INSIDE_CIDR_BLOCKS)
                    .and_then(::serde_json::Value::as_array)
                    .map(|blocks| strings_of(blocks)),
            })
        })
        .collect();

    let named = |key: &str| -> Vec<NamedGroup> {
        root.get(key)
            .and_then(::serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|entry| {
                let name = entry
                    .get(POLICY_NAME)
                    .and_then(::serde_json::Value::as_str)?;
                Some(NamedGroup {
                    name: name.to_string(),
                    // An entry naming no edge locations is held as "every edge location",
                    // not as a copy of the list: the expansion happens where a response
                    // is built, so nothing quadratic is retained.
                    edge_locations: entry
                        .get(POLICY_EDGE_LOCATIONS)
                        .and_then(::serde_json::Value::as_array)
                        .map(|listed| strings_of(listed)),
                })
            })
            .collect()
    };

    PolicyTopology {
        segments: named(POLICY_SEGMENTS),
        network_function_groups: named(POLICY_NETWORK_FUNCTION_GROUPS),
        edges,
    }
}

/// The strings of a JSON array, skipping any entry that is not one.
fn strings_of(values: &[::serde_json::Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(::serde_json::Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Hands out the ASNs a policy document's `asn-ranges` makes available, in ascending
/// order, skipping every ASN an edge location claims explicitly.
///
/// A range is written `"<low>-<high>"` (inclusive) or as a single ASN, and a range can
/// span millions of numbers, so the candidates are walked with a cursor rather than
/// materialised.
#[derive(Debug)]
struct AsnPool {
    /// The inclusive `(low, high)` ranges, in the order the document lists them.
    ranges: Vec<(i64, i64)>,
    /// How far into `ranges` the walk has got: the index and the next candidate in it.
    cursor: usize,
    /// The next candidate inside `ranges[cursor]`.
    next: i64,
    /// The ASNs edge locations claim explicitly, which an assignment skips.
    claimed: BTreeSet<i64>,
}

impl AsnPool {
    /// The pool `ranges` describes, with every ASN the `edge-locations` entries claim
    /// explicitly already taken.
    fn new(ranges: &[::serde_json::Value], edge_locations: &[::serde_json::Value]) -> Self {
        let ranges: Vec<(i64, i64)> = ranges.iter().filter_map(parse_asn_range).collect();
        let claimed = edge_locations
            .iter()
            .filter_map(|entry| {
                entry
                    .get(POLICY_ASN)
                    .and_then(::serde_json::Value::as_i64)
            })
            .collect();
        let next = ranges.first().map_or(0, |(low, _)| *low);
        Self {
            ranges,
            cursor: 0,
            next,
            claimed,
        }
    }

    /// The next free ASN, or `None` once the ranges are exhausted (or none were given).
    fn take(&mut self) -> Option<i64> {
        while let Some((low, high)) = self.ranges.get(self.cursor).copied() {
            let candidate = self.next.max(low);
            if candidate > high {
                self.advance();
                continue;
            }
            match candidate.checked_add(1) {
                Some(next) => self.next = next,
                // The candidate is the largest number there is, so this range holds
                // nothing after it: the walk moves on to the next range whether or not
                // the candidate itself is free. Advancing unconditionally is what keeps
                // the walk strictly monotonic — a saturating increment would re-derive
                // this same candidate forever once it is claimed.
                None => self.advance(),
            }
            if self.claimed.insert(candidate) {
                return Some(candidate);
            }
        }
        None
    }

    /// Move the walk on to the next range.
    fn advance(&mut self) {
        self.cursor = self.cursor.saturating_add(1);
        self.next = self.ranges.get(self.cursor).map_or(0, |(low, _)| *low);
    }
}

/// One `asn-ranges` entry: `"64512-64555"`, `"64512"` or a bare number.
///
/// A range that is empty, or that reaches outside the [`ASN_MIN`]..=[`ASN_MAX`] domain a
/// BGP AS number lives in, names no assignable ASN and is answered `None` — an entry no
/// assignment can be made from, rather than one that is quietly reinterpreted.
fn parse_asn_range(value: &::serde_json::Value) -> Option<(i64, i64)> {
    let (low, high) = if let Some(single) = value.as_i64() {
        (single, single)
    } else {
        let text = value.as_str()?.trim();
        match text.split_once('-') {
            Some((low, high)) => (low.trim().parse::<i64>().ok()?, high.trim().parse::<i64>().ok()?),
            None => {
                let single = text.parse::<i64>().ok()?;
                (single, single)
            }
        }
    };
    (low <= high && low >= ASN_MIN && high <= ASN_MAX).then_some((low, high))
}

/// Whether two tag sets hold the same keys with the same values, whatever order they
/// arrived in. Used by the idempotency check of a create: a retry that carries the same
/// `ClientToken` must carry the same tags to be the same request. `Tag` is a generated
/// shape with no `PartialEq`, so the comparison goes through the key/value pairs.
pub fn same_tags(left: &[Tag], right: &[Tag]) -> bool {
    let pairs = |tags: &[Tag]| -> BTreeMap<String, String> {
        tags.iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect()
    };
    pairs(left) == pairs(right)
}

/// `SystemTime` as a Smithy timestamp (epoch seconds, sub-second part kept).
fn epoch_seconds(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// One page of a paginated listing.
pub struct Page<T> {
    /// The items on this page, in key order.
    pub items: Vec<T>,
    /// The token that resumes after the last item, or `None` when this is the last page.
    pub next_token: Option<String>,
}

/// The prefix every pagination token this service mints carries, so a token minted by
/// something else (or a truncated one) is recognised as unparseable.
const PAGE_TOKEN_PREFIX: &str = "networkmanager:";

/// Encode the continuation token that resumes AFTER `last_key`.
///
/// The token is opaque to the caller: hex of a prefixed key offset into the relevant
/// `BTreeMap`, which is why the listing map is ordered. Hex, so the token survives the
/// query string a `NextToken` travels back in (`@httpQuery("nextToken")`) unescaped,
/// whatever the key holds.
fn encode_page_token(last_key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{last_key}"))
}

/// Decode a continuation token minted by [`encode_page_token`], or `None` when the token
/// is not one this service produced — the caller answers that with a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// Take one page of `entries` (which must be sorted by key) starting after `after`.
///
/// `max_results` is the caller's page size; absent means "every remaining item". A
/// `next_token` is emitted only when items remain after this page, so a client that
/// follows tokens sees every item exactly once.
pub fn paginate<T>(
    entries: Vec<(String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    let start = match after {
        Some(key) => entries
            .iter()
            .position(|(candidate, _)| candidate.as_str() > key)
            .unwrap_or(entries.len()),
        None => 0,
    };
    let limit = max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(usize::MAX)
        // A page is never empty while items remain: `MaxResults` is modelled `1..500`, so
        // 0 cannot arrive, and clamping keeps the token below from skipping an item.
        .max(1);
    let end = start.saturating_add(limit).min(entries.len());
    let next_token = if end < entries.len() {
        entries
            .get(end.saturating_sub(1))
            .map(|(key, _)| encode_page_token(key))
    } else {
        None
    };
    let items = entries
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|(_, item)| item)
        .collect();
    Page { items, next_token }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkmanagerNetworkmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkmanagerNetworkmanagerState>)
}
