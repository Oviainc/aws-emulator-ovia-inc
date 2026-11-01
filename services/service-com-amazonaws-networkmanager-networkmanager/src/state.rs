// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkmanager-networkmanager service.
//!
//! The global-network / site / link lifecycle is the core resource tree of Network
//! Manager: a global network owns sites, a site owns links, and a global network owns
//! devices (each optionally assigned to one of its sites). A device and a link that sit at
//! the same site can be tied together by a LINK ASSOCIATION, which has no id of its own
//! and is therefore keyed by that pair. A device can likewise be tied to a CUSTOMER
//! GATEWAY of Amazon VPC — and, when the request names one, to a link of that device — by a
//! CUSTOMER GATEWAY ASSOCIATION, which has no id either and is keyed by the global network
//! plus the gateway's ARN. Two devices of one global network can instead be
//! tied together by a CONNECTION, which unlike that association IS a resource of its own,
//! with an id, an ARN and tags. A global network also owns its
//! CORE network — the Cloud WAN managed network
//! whose segments and edges are a function of the core network policy document it holds.
//! A core network edge can PEER with a resource of another service (a transit gateway),
//! and a core network in turn owns its ATTACHMENTS: each joins one resource to the edge
//! locations that policy configures — one of them for every kind but a Direct Connect
//! gateway attachment, which spans several — and a Connect attachment is carried over
//! another attachment (its transport) rather than over a resource of its own, and a
//! transit gateway route table attachment over a PEERING — the core network's own
//! connection to a transit gateway, which is a resource of its own with an id, an ARN and
//! tags, and which fixes the core network and the edge location the attachment over it
//! takes. A Connect
//! attachment in turn carries CONNECT PEERS, each a BGP peering between one edge of the
//! core network and one appliance outside AWS, and each a resource of its own with an id,
//! an ARN and tags. A Connect peer can in turn be ASSOCIATED with one device of the global
//! network that owns its core network, optionally over one of that device's links: like a
//! link association that tie has no id of its own and is keyed by the peer it ties. The
//! policy's
//! `attachment-policies` rules decide what becomes of each attachment created on the core
//! network — which segment or network function group it joins, and whether its attachment
//! request has to be ACCEPTED first, which is what holds it in
//! `PENDING_ATTACHMENT_ACCEPTANCE` until `AcceptAttachment` grants that acceptance. A
//! Connect attachment in turn owns the CONNECT PEERS built on it: each is one BGP peering
//! between that attachment's edge of the core network and an appliance.
//! Beside that tree a global network holds the telemetry-bearing connections registered
//! with it (a Connect peer of either kind, a VPN connection), whose session health is what
//! `GetNetworkTelemetry` reports.
//! A global network also holds the TRANSIT GATEWAYS registered with it: each registration
//! is the tie between the global network and one transit gateway of Amazon VPC, with no id
//! of its own, so it is keyed by that pair like a customer gateway association. A CONNECT
//! PEER of such a gateway can in turn be tied to one device of that global network, and
//! optionally to one of that device's links, by a TRANSIT GATEWAY CONNECT PEER ASSOCIATION,
//! which has no id either and is keyed by the global network plus the peer's own ARN; that
//! tie is what registers the peering with the global network's telemetry.
//! A global network also holds the ROUTE ANALYSES started on it: each is a Route Analyzer
//! job that analysed the routing path between two attachments against what the global
//! network held when it was started, and holds the paths it found for `GetRouteAnalysis`
//! to read back.
//! Any resource of a global network's network-resource inventory can in turn carry
//! METADATA — the key/value map `UpdateNetworkResourceMetadata` stores for it and
//! `GetNetworkResources` reports beside its definition — held under that resource's own ARN.
//! Everything
//! the implemented operations answer is computed from the records below — no handler
//! echoes its input back without storing it.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// The two shapes a core network change event is reported through, and the two a change of
// a change set is reported through, imported on their own so this slice's shape list below
// stays exactly as the rest of the service shares it.
use crate::ops::types::{CoreNetworkChange, CoreNetworkChangeValues};
use crate::ops::types::{CoreNetworkChangeEvent, CoreNetworkChangeEventValues};
// The shapes one change reports a segment's service insertion actions through, imported
// beside them for the same reason.
use crate::ops::types::{NetworkFunctionGroup, ServiceInsertionAction, Via};
use crate::ops::types::{
    AWSLocation, AccountStatus, Attachment, AttachmentRoutingPolicyAssociationSummary, Bandwidth,
    ConnectAttachment, ConnectAttachmentOptions,
    ConnectPeer, ConnectPeerAssociation, ConnectPeerBgpConfiguration, ConnectPeerConfiguration,
    ConnectPeerSummary,
    ConflictException,
    Connection, ConnectionHealth, CoreNetwork, CoreNetworkEdge, CoreNetworkNetworkFunctionGroup,
    CustomerGatewayAssociation,
    CoreNetworkPolicy, CoreNetworkPolicyError, CoreNetworkPolicyException, CoreNetworkPolicyVersion,
    CoreNetworkSegment,
    Device, DirectConnectGatewayAttachment, GlobalNetwork, Link, LinkAssociation, Location,
    NetworkResource, NetworkTelemetry, OrganizationStatus, Peering, Relationship,
    ResourceNotFoundException, RouteAnalysis, RouteAnalysisCompletion,
    RouteAnalysisEndpointOptions, RouteAnalysisEndpointOptionsSpecification, RouteAnalysisPath,
    ServiceInsertionSegments, Site, SiteToSiteVpnAttachment, Tag,
    TransitGatewayConnectPeerAssociation, TransitGatewayPeering,
    TransitGatewayRegistration, TransitGatewayRegistrationStateReason,
    TransitGatewayRouteTableAttachment, ValidationException,
    ValidationExceptionField, VpcAttachment, VpcOptions,
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
/// See [`STATE_PENDING`]. The state a resource of one of those kinds is in between the
/// update that reconfigures it and the instant that reconfiguration is applied.
pub const STATE_UPDATING: &str = "UPDATING";

/// The state a resource whose CREATION is still in flight is in, where the resources
/// above wait in [`STATE_PENDING`]: a value of `CoreNetworkState`, `PeeringState`,
/// `ConnectPeerState` and `AttachmentState` (`smithy_query
/// shape=CoreNetworkState,PeeringState,ConnectPeerState,AttachmentState`, mirrored in
/// `ops::types::CORE_NETWORK_STATE_VALUES` and the three beside it), which carry CREATING
/// where `GlobalNetworkState` and its two siblings carry PENDING; AVAILABLE and DELETING
/// they share with them.
///
/// A peering, a Connect peer and an attachment are each answered in it by the call that
/// creates them and wait in it for the transition the clock owes. A CORE NETWORK is not:
/// its create answers it already [`STATE_AVAILABLE`] ([`CoreNetworkRecord::state_at`]), so
/// nothing of this service reports a core network in this state.
pub const STATE_CREATING: &str = "CREATING";

/// The state an ATTACHMENT waits in while the `attachment-policies` rule of the core
/// network policy that claimed it asks for the attachment request to be accepted. One of
/// `AttachmentState`'s own values (`smithy_query shape=AttachmentState`, mirrored in
/// `ops::types::ATTACHMENT_STATE_VALUES`).
///
/// Unlike [`STATE_CREATING`] this is NOT a wait on the clock: no span is owed on it, and
/// only an acceptance (`AcceptAttachment`, [`AttachmentRecord::accept_at`]) moves an
/// attachment out of it — after which the ordinary `CREATING` -> `AVAILABLE` transition
/// the clock owes begins.
pub const STATE_PENDING_ATTACHMENT_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";

/// The TERMINAL state of an attachment whose attachment request was turned down
/// (`RejectAttachment`, [`AttachmentRecord::reject_at`]). One of `AttachmentState`'s own
/// values (`smithy_query shape=AttachmentState`, mirrored in
/// `ops::types::ATTACHMENT_STATE_VALUES`).
///
/// Like [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`] this is not a wait on the clock, and unlike
/// it nothing moves an attachment out of it: the request was never granted, so the
/// attachment is never created and no transition is ever owed on it. The record stays
/// stored and readable in this state until `DeleteAttachment` removes it.
pub const STATE_REJECTED: &str = "REJECTED";

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
/// See [`GLOBAL_NETWORK_KIND`]. A connection hangs off the global network and ties two of
/// its devices together ("Creates a connection between two devices", `smithy_query
/// query="doc" shape="CreateConnection"`), so one name serves the id prefix
/// (`connection-<hex>`), the ARN resource type (`connection/<id>`) and the `connection`
/// entry of [`NETWORK_RESOURCE_TYPES`].
pub const CONNECTION_KIND: &str = "connection";
/// The resource kind a link ASSOCIATION reports in a refusal that names it.
///
/// Unlike the kinds above this one is not an ARN resource type and no id is minted for
/// it: a link association has no ARN and no id of its own — "Describes the association
/// between a device and a link" (`smithy_query shape=LinkAssociation`) is the
/// device/link pair plus its state — so the pair is its identity
/// ([`link_association_key`]) and this name is only how a not-found refusal spells the
/// resource type.
pub const LINK_ASSOCIATION_KIND: &str = "link-association";
/// The resource kind a CUSTOMER GATEWAY association reports in a refusal that names it.
///
/// Like [`LINK_ASSOCIATION_KIND`] this is not an ARN resource type and no id is minted for
/// it: "Describes the association between a customer gateway, a device, and a link"
/// (`smithy_query shape=CustomerGatewayAssociation`) carries no id, so the global network
/// plus the customer gateway ARN is its identity
/// ([`customer_gateway_association_key`]) and this name is only how a not-found refusal
/// spells the resource type.
pub const CUSTOMER_GATEWAY_ASSOCIATION_KIND: &str = "customer-gateway-association";
/// The resource kind a TRANSIT GATEWAY REGISTRATION reports in a refusal that names it.
///
/// Like [`CUSTOMER_GATEWAY_ASSOCIATION_KIND`] this is not an ARN resource type and no id is
/// minted for it: "Describes the registration of a transit gateway to a global network"
/// (`smithy_query shape=TransitGatewayRegistration`) carries no id, so the global network
/// plus the transit gateway's ARN is its identity
/// ([`transit_gateway_registration_key`]) and this name is only how a not-found refusal
/// spells the resource type.
pub const TRANSIT_GATEWAY_REGISTRATION_KIND: &str = "transit-gateway-registration";
/// The resource kind a TRANSIT GATEWAY CONNECT PEER association reports in a refusal that
/// names it.
///
/// Like [`TRANSIT_GATEWAY_REGISTRATION_KIND`] this is not an ARN resource type and no id is
/// minted for it: "Describes a transit gateway Connect peer association" (`smithy_query
/// shape=TransitGatewayConnectPeerAssociation`) carries no id, so the global network plus
/// the peer's own ARN is its identity
/// ([`transit_gateway_connect_peer_association_key`]) and this name is only how a
/// not-found refusal spells the resource type.
pub const TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND: &str =
    "transit-gateway-connect-peer-association";
/// The resource kind a CORE NETWORK CONNECT PEER association reports in a refusal that
/// names it.
///
/// Like [`TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND`] this is not an ARN resource type
/// and no id is minted for it: "Describes a core network Connect peer association"
/// (`smithy_query shape=ConnectPeerAssociation`) carries no id of its own, so the PEER is
/// its identity ([`ConnectPeerAssociationRecord`]) and this name is only how a not-found
/// refusal spells the resource type of the association, as against the
/// [`CONNECT_PEER_KIND`] of the peer itself.
pub const CONNECT_PEER_ASSOCIATION_KIND: &str = "connect-peer-association";
/// See [`GLOBAL_NETWORK_KIND`]. The model constrains this one's id itself:
/// `CoreNetworkId` carries `pattern ^core-network-([0-9a-f]{8,17})$`
/// (`smithy_query shape=CoreNetworkId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
pub const CORE_NETWORK_KIND: &str = "core-network";
/// See [`GLOBAL_NETWORK_KIND`]. The model constrains this one's id itself too:
/// `PeeringId` carries `pattern ^peering-([0-9a-f]{8,17})$` (`smithy_query
/// shape=PeeringId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy. It is also the
/// `peering` entry of [`NETWORK_RESOURCE_TYPES`].
pub const PEERING_KIND: &str = "peering";
/// The resource kind a refusal names when the resource addressed is ONE policy of a core
/// network rather than the core network itself. A policy version is not a resource this
/// service mints an id or an ARN for — the model identifies it by its owning
/// `CoreNetworkId` plus an `Integer` `PolicyVersionId` (`smithy_query
/// operation=GetCoreNetworkPolicy,DeleteCoreNetworkPolicyVersion`) — so the pair travels
/// in the refusal's `ResourceId` ([`policy_version_resource_id`]) and this kind in its
/// `ResourceType`.
pub const CORE_NETWORK_POLICY_KIND: &str = "core-network-policy";
/// See [`GLOBAL_NETWORK_KIND`]. The model constrains an attachment's id too:
/// `AttachmentId` carries `pattern ^attachment-([0-9a-f]{8,17})$` (`smithy_query
/// shape=AttachmentId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy. It is also the
/// `attachment` entry of [`NETWORK_RESOURCE_TYPES`], so one name serves the id prefix,
/// the ARN resource type and the network-resource inventory.
pub const ATTACHMENT_KIND: &str = "attachment";
/// The resource kind a refusal names when the resource addressed is one PREFIX LIST
/// ASSOCIATION of a core network.
///
/// Like [`LINK_ASSOCIATION_KIND`] this is not an ARN resource type and no id is minted for
/// it: the modelled `PrefixListAssociation` is the core network / prefix list pair plus the
/// alias, with no id, ARN or state of its own (`smithy_query shape=PrefixListAssociation`),
/// so the pair travels in a refusal's `ResourceId` ([`prefix_list_association_key`]) and
/// this kind in its `ResourceType`.
pub const PREFIX_LIST_ASSOCIATION_KIND: &str = "prefix-list-association";
/// See [`GLOBAL_NETWORK_KIND`]. A ROUTE ANALYSIS is a job of its global network rather
/// than a resource with an ARN — the modelled `RouteAnalysis` carries only a
/// `RouteAnalysisId` (`smithy_query shape=RouteAnalysis`) — so this name serves the id
/// prefix (`route-analysis-<hex>`) and the `ResourceType` a not-found refusal reports.
pub const ROUTE_ANALYSIS_KIND: &str = "route-analysis";

/// The one value `PeeringType` enumerates (`smithy_query shape=PeeringType`, mirrored in
/// `ops::types::PEERING_TYPE_VALUES`): "The type of peering. This will be
/// `TRANSIT_GATEWAY`" (`smithy_query query="doc" shape="Peering" member="PeeringType"`).
pub const PEERING_TYPE_TRANSIT_GATEWAY: &str = "TRANSIT_GATEWAY";

/// The id prefix of the peering attachment a transit gateway peering leaves on the
/// transit gateway side: `TransitGatewayPeeringAttachmentId` carries `pattern
/// ^tgw-attach-([0-9a-f]{8,17})$` (`smithy_query shape=TransitGatewayPeering`), which the
/// 17 hex digits of [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
///
/// Unlike the kinds above this one is not an ARN resource type of this service: the
/// attachment it names belongs to the peered transit gateway, an Amazon VPC resource.
pub const TRANSIT_GATEWAY_PEERING_ATTACHMENT_KIND: &str = "tgw-attach";

/// See [`GLOBAL_NETWORK_KIND`]. A Connect peer hangs off a core network CONNECT attachment
/// — "Creates a core network Connect peer for a specified core network connect attachment
/// between a core network and an appliance" (`smithy_query query="doc"
/// shape="CreateConnectPeer"`) — and is a resource of its own, with an id the model
/// constrains (`ConnectPeerId` carries `pattern ^connect-peer-([0-9a-f]{8,17})$`,
/// `smithy_query shape=ConnectPeerId`), an ARN and tags. The name is the same string
/// [`CONNECT_PEER_TYPE`] holds, so one name serves the id prefix (`connect-peer-<hex>`),
/// the ARN resource type (`connect-peer/<id>`) and the `connect-peer` entry of
/// [`NETWORK_RESOURCE_TYPES`].
pub const CONNECT_PEER_KIND: &str = CONNECT_PEER_TYPE;

/// The state an attachment is in while a CHANGE to one that already exists is in flight,
/// as against the [`STATE_CREATING`] of one whose creation is.
///
/// `AttachmentState` carries both (`smithy_query shape=AttachmentState`, mirrored in
/// `ops::types::ATTACHMENT_STATE_VALUES`), so an accepted `UpdateVpcAttachment` reports
/// `UPDATING` until the transition the clock owes falls due and [`STATE_AVAILABLE`] after
/// it. See [`AttachmentRecord::state_at`].
pub const ATTACHMENT_STATE_UPDATING: &str = "UPDATING";

/// The `AttachmentType`s this service mints, the five the model enumerates
/// (CONNECT, DIRECT_CONNECT_GATEWAY, SITE_TO_SITE_VPN, TRANSIT_GATEWAY_ROUTE_TABLE, VPC:
/// `smithy_query shape=AttachmentType`, mirrored in `ops::types::ATTACHMENT_TYPE_VALUES`).
pub const ATTACHMENT_TYPE_VPC: &str = "VPC";
/// See [`ATTACHMENT_TYPE_VPC`].
pub const ATTACHMENT_TYPE_CONNECT: &str = "CONNECT";
/// See [`ATTACHMENT_TYPE_VPC`].
pub const ATTACHMENT_TYPE_SITE_TO_SITE_VPN: &str = "SITE_TO_SITE_VPN";
/// See [`ATTACHMENT_TYPE_VPC`]. The one kind that spans SEVERAL edge locations of its
/// core network rather than sitting at one (see
/// [`AttachmentDetail::DirectConnectGateway`]).
pub const ATTACHMENT_TYPE_DIRECT_CONNECT_GATEWAY: &str = "DIRECT_CONNECT_GATEWAY";
/// See [`ATTACHMENT_TYPE_VPC`]. The one kind created over a PEERING of its core network
/// rather than over a resource reached directly (see
/// [`AttachmentDetail::TransitGatewayRouteTable`]).
pub const ATTACHMENT_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE: &str = "TRANSIT_GATEWAY_ROUTE_TABLE";

/// The tunnel protocols a Connect attachment's options may ask for, the two the model
/// enumerates (GRE, NO_ENCAP: `smithy_query shape=TunnelProtocol`, mirrored in
/// `ops::types::TUNNEL_PROTOCOL_VALUES`), and therefore the two a Connect peer can inherit
/// from the attachment it is carried over.
///
/// They are named here because the protocol decides which of a Connect peer create's
/// members apply at all ([`check_protocol_scoped_member`]).
pub const TUNNEL_PROTOCOL_GRE: &str = "GRE";
/// See [`TUNNEL_PROTOCOL_GRE`].
pub const TUNNEL_PROTOCOL_NO_ENCAP: &str = "NO_ENCAP";

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
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_CONNECTIONS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_CONNECTIONS_PER_GLOBAL_NETWORK: &str = "connections-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK`] bounds.
pub const LIMIT_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK: &str =
    "prefix-list-associations-per-core-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK: &str =
    "customer-gateway-associations-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota
/// [`MAX_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK`] bounds.
pub const LIMIT_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK: &str =
    "transit-gateway-connect-peer-associations-per-global-network";
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
///
/// `id` is the identifying detail the operation reports: the value the request addressed
/// the resource by, except where AWS reports the NAME of the member that addressed it
/// instead, which is what `ops/get_core_network.rs` passes [`FIELD_CORE_NETWORK_ID`] for.
pub fn resource_not_found(kind: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: id.to_string(),
        resource_type: kind.to_string(),
        context: None,
    }
}

/// The field names this service reports in `ValidationException$Fields[].Name` for the
/// reference a request carries — the parent a create hangs its new resource off, or the
/// resource a reporting operation is addressed through.
///
/// Network Manager names the offending REQUEST MEMBER in its own lowerCamelCase field
/// naming, not in the wire (PascalCase) spelling: a recording of `CreateSite` under an
/// unknown global network answers `globalNetworkId` and one of `CreateLink` with an
/// unknown site answers `siteId`. Both are `<kind>Id` for the kind the member references.
pub const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";
/// See [`FIELD_GLOBAL_NETWORK_ID`].
pub const FIELD_SITE_ID: &str = "siteId";
/// See [`FIELD_GLOBAL_NETWORK_ID`]: the same lowerCamelCase spelling of the request
/// member, for the references and values an attachment or peering create carries.
pub const FIELD_CORE_NETWORK_ID: &str = "coreNetworkId";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_TRANSPORT_ATTACHMENT_ID: &str = "transportAttachmentId";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_EDGE_LOCATION: &str = "edgeLocation";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_VPC_ARN: &str = "vpcArn";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_SUBNET_ARNS: &str = "subnetArns";
/// See [`FIELD_CORE_NETWORK_ID`]: the two lists an update of a VPC attachment carries.
pub const FIELD_ADD_SUBNET_ARNS: &str = "addSubnetArns";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_REMOVE_SUBNET_ARNS: &str = "removeSubnetArns";
/// See [`FIELD_CORE_NETWORK_ID`].
pub const FIELD_VPN_CONNECTION_ARN: &str = "vpnConnectionArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the member carrying the ARN of the customer gateway a
/// global network's customer gateway association names.
pub const FIELD_CUSTOMER_GATEWAY_ARN: &str = "customerGatewayArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the member carrying the ARN of the managed prefix list a
/// core network's prefix-list association names.
pub const FIELD_PREFIX_LIST_ARN: &str = "prefixListArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the member carrying the several edge locations a Direct
/// Connect gateway attachment spans.
pub const FIELD_EDGE_LOCATIONS: &str = "edgeLocations";
/// See [`FIELD_GLOBAL_NETWORK_ID`]: the same lowerCamelCase spelling of the request
/// member, for the device and link references a connection carries.
pub const FIELD_DEVICE_ID: &str = "deviceId";
/// See [`FIELD_DEVICE_ID`].
pub const FIELD_CONNECTED_DEVICE_ID: &str = "connectedDeviceId";
/// See [`FIELD_DEVICE_ID`].
pub const FIELD_LINK_ID: &str = "linkId";
/// See [`FIELD_DEVICE_ID`].
pub const FIELD_CONNECTED_LINK_ID: &str = "connectedLinkId";
/// See [`FIELD_CORE_NETWORK_ID`]: the same lowerCamelCase spelling of the request member,
/// for the references and values a peering and a transit gateway route table attachment
/// carry.
pub const FIELD_PEERING_ID: &str = "peeringId";
/// See [`FIELD_PEERING_ID`].
pub const FIELD_TRANSIT_GATEWAY_ARN: &str = "transitGatewayArn";
/// See [`FIELD_PEERING_ID`].
pub const FIELD_TRANSIT_GATEWAY_ROUTE_TABLE_ARN: &str = "transitGatewayRouteTableArn";
/// See [`FIELD_PEERING_ID`]: the member carrying the ARN of the transit gateway Connect
/// peer a global network's Connect peer association names.
pub const FIELD_TRANSIT_GATEWAY_CONNECT_PEER_ARN: &str = "transitGatewayConnectPeerArn";
/// See [`FIELD_CORE_NETWORK_ID`]: the references and values a Connect peer create carries
/// — the Connect attachment the peer is carried over, the inside IP addresses it peers
/// over, and the subnet it reaches its appliance through.
pub const FIELD_CONNECT_ATTACHMENT_ID: &str = "connectAttachmentId";
/// See [`FIELD_CONNECT_ATTACHMENT_ID`].
pub const FIELD_INSIDE_CIDR_BLOCKS: &str = "insideCidrBlocks";
/// See [`FIELD_CONNECT_ATTACHMENT_ID`].
pub const FIELD_SUBNET_ARN: &str = "subnetArn";
/// See [`FIELD_CONNECT_ATTACHMENT_ID`]: the two ends of the tunnel a Connect peer's BGP
/// session runs over — the appliance's address and the core network's own.
pub const FIELD_PEER_ADDRESS: &str = "peerAddress";
/// See [`FIELD_PEER_ADDRESS`].
pub const FIELD_CORE_NETWORK_ADDRESS: &str = "coreNetworkAddress";
/// See [`FIELD_GLOBAL_NETWORK_ID`]: the same lowerCamelCase spelling of the request
/// member, for the version id a put of a core network policy asserts the history ends at.
pub const FIELD_LATEST_VERSION_ID: &str = "latestVersionId";
/// See [`FIELD_CORE_NETWORK_ID`]: the segment a listing of one segment's routing
/// information addresses, and the next-hop filter map that listing narrows itself with.
pub const FIELD_SEGMENT_NAME: &str = "segmentName";
/// See [`FIELD_SEGMENT_NAME`].
pub const FIELD_NEXT_HOP_FILTERS: &str = "nextHopFilters";
/// See [`FIELD_SEGMENT_NAME`]: the four lists a listing of routing information matches a
/// route's BGP attributes against.
pub const FIELD_LOCAL_PREFERENCE_MATCHES: &str = "localPreferenceMatches";
/// See [`FIELD_LOCAL_PREFERENCE_MATCHES`].
pub const FIELD_MED_MATCHES: &str = "medMatches";
/// See [`FIELD_LOCAL_PREFERENCE_MATCHES`].
pub const FIELD_EXACT_AS_PATH_MATCHES: &str = "exactAsPathMatches";
/// See [`FIELD_LOCAL_PREFERENCE_MATCHES`].
pub const FIELD_COMMUNITY_MATCHES: &str = "communityMatches";

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

/// What an OPTIONAL reference member of a request names: the id it carries, or `None`
/// when it names no resource at all.
///
/// Every id shape of this service is modelled `len 0..50` over `^[\s\S]*$`
/// (`smithy_query query="shape" shape="SiteId"`), so the empty string satisfies the
/// constraint walk and reaches the handler; an ARN-shaped reference member reaches it the
/// same way when its own pattern spells the empty string as an alternative of its own
/// (`^arn:…$|^$`, `smithy_query query="shape" shape="SubnetArn"`). On a member that is
/// OPTIONAL the empty string
/// is the ABSENCE of the reference — the spelling the service documents for giving no
/// value ("To remove information for any of the parameters, specify an empty string",
/// `smithy_query query="doc" shape="UpdateDevice"`) — and not a reference to a resource
/// whose id is the empty string, which nothing can ever be stored under. So it resolves
/// nothing, is never refused as unresolvable, and is held as none, exactly as the omitted
/// member is: that keeps every stored record's references either naming a resource of the
/// same global network or naming nothing at all.
///
/// A REQUIRED reference is not passed through here: it must name a resource, so the empty
/// string stays the unresolvable reference its own refusal reports.
///
/// A listing that SELECTS on an optional reference member reads it through here too, so one
/// member has one reading whether a request writes it or filters by it: the empty string
/// names no resource, so it narrows the listing by nothing and the listing answers exactly
/// what the omitted member asks for. Two kinds of member are NOT read through here: a
/// DESCRIPTIVE one — a link's type or provider, a Region, a state — is not a reference and
/// its writers store the empty string verbatim, so a record can carry it and a filter naming
/// it keeps matching it literally; and one whose modelled shape does not admit the empty
/// string at all (an id pattern, a minimum length) never reaches a handler empty, because the
/// constraint walk refuses it first.
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}

/// What an UPDATE writes for an OPTIONAL reference member: the outer `Option` says whether
/// the request spoke about the member at all, the inner one what the member names.
///
/// `None` is the member the request does not carry, which leaves the stored value where it
/// is. `Some(inner)` is a member the request supplies, and `inner` is what it names under
/// the one reading this service gives these members ([`optional_reference`]): `Some(id)` for
/// an id to resolve, `None` for the empty string, which names no resource and is therefore
/// written as none rather than stored verbatim. So an update that empties a reference leaves
/// the record in the state a create with the same spelling leaves it in, and no stored
/// reference ever names an id nothing can be stored under.
pub fn requested_reference(supplied: Option<&str>) -> Option<Option<&str>> {
    supplied.map(|id| optional_reference(Some(id)))
}

/// The modelled `ValidationException` (HTTP 400) an operation answers when a reference
/// its input carries — the parent a new resource must hang off, or the resource a
/// reporting operation is addressed through — names no stored resource.
///
/// A create validates its input before anything is minted, so a reference that resolves
/// to nothing is a FIELD failure on the member that carried it rather than a missing
/// resource: `Reason` is `FieldValidationFailed` and `Fields` carries one entry naming
/// that member (see [`FIELD_GLOBAL_NETWORK_ID`]). The prose is the one the recordings
/// show — "<Kind> not found." for the kind the member references, from [`kind_label`] —
/// and is repeated as the field's own message, exactly as recorded.
///
/// This is the crate's SINGLE construction for an unresolvable reference refused as a
/// validation failure, so one service reports one field name and one message for the same
/// member whichever operation refused it. An operation that MODELS
/// `ResourceNotFoundException` answers the absent resource with that 404 instead (see
/// [`resource_not_found`]); one that does not — a reporting operation whose only client
/// error is the `ValidationException` — refuses the reference through here.
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

/// How a resource kind of this service reads where AWS names it as the SUBJECT of a
/// refusal: every word of the ARN resource type capitalised, so `global-network` reads
/// "Global Network" and `core-network` reads "Core Network".
fn kind_title(kind: &str) -> String {
    kind.split('-')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// The `Reason` a refusal carries when the request's own fields all parse and satisfy
/// their constraints, yet the request is invalid against stored state — the modelled
/// catch-all of `ValidationExceptionReason` (UnknownOperation, CannotParse,
/// FieldValidationFailed, Other: `smithy_query shape=ValidationExceptionReason`).
pub const VALIDATION_REASON_OTHER: &str = "Other";

/// The modelled `ValidationException` (HTTP 400) a DELETE answers while the resource it
/// names still owns dependent resources the caller has to remove first, or `None` when
/// nothing blocks the delete (every count is zero).
///
/// Network Manager documents the emptying order each delete requires ("You must first
/// delete all global network objects (devices, links, and sites), deregister all transit
/// gateways, and delete any core networks", `smithy_query query="doc"
/// shape="DeleteGlobalNetwork"`; "The site cannot be associated with any device or link",
/// `shape="DeleteSite"`) and refuses the delete until it holds. That refusal is a
/// VALIDATION failure of the request against the stored resource tree — HTTP 400,
/// `ValidationException` — and NOT a `ConflictException` (409): a dependency refusal
/// stands until the caller deletes the children, so an SDK waiter or retry loop must see a
/// terminal 400, while 409 is this service's concurrent-state conflict (an idempotency
/// token reused for a different request, a resource being modified) which a retry can
/// clear. Recorded behaviour confirms it for the core-network dependent: the delete of a
/// global network that still owns one answers HTTP 400 with code `ValidationException`,
/// and succeeds once the core network is gone.
///
/// The prose names the blocking kinds the same way the recorded message does — the parent
/// in [`kind_title`], the dependents as their ARN resource types read as words — and the
/// identifying detail stays out of it: no `Fields` entry, because no input field is at
/// fault, and `Reason` is [`VALIDATION_REASON_OTHER`].
///
/// `parent_kind` is the kind being deleted ([`GLOBAL_NETWORK_KIND`], [`SITE_KIND`], …) and
/// `dependents` pairs each child kind with how many of it the parent still holds, in the
/// order the message should list them; a zero count never blocks and is never named.
pub fn dependents_block_delete(
    parent_kind: &str,
    dependents: &[(&str, usize)],
) -> Option<ValidationException> {
    let blocking: Vec<String> = dependents
        .iter()
        .filter(|(_, held)| *held > 0)
        .map(|(kind, _)| kind.replace('-', " "))
        .collect();
    let listed = match blocking.split_last() {
        None => return None,
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    };
    Some(ValidationException {
        message: format!(
            "{} cannot be deleted due to an existing {listed} associated with it.",
            kind_title(parent_kind)
        ),
        reason: Some(VALIDATION_REASON_OTHER.to_string()),
        fields: None,
    })
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
    ATTACHMENT_KIND,
    "connect-peer",
    "connection",
    "core-network",
    CUSTOMER_GATEWAY_KIND,
    "device",
    "dx-gateway",
    "dx-vif",
    "dxcon",
    "link",
    PEERING_KIND,
    "site",
    TRANSIT_GATEWAY_TYPE,
    TRANSIT_GATEWAY_ATTACHMENT_TYPE,
    "transit-gateway-connect-peer",
    TRANSIT_GATEWAY_ROUTE_TABLE_TYPE,
    "vpn-connection",
];

/// The network-resource type of a TRANSIT GATEWAY, and of one of its route tables: Amazon
/// VPC resources, which is also the resource type their ARNs carry
/// (`arn:<partition>:ec2:<region>:<account>:transit-gateway/tgw-<hex>`) and what
/// [`ec2_resource_region`] is asked for when a request names one.
pub const TRANSIT_GATEWAY_TYPE: &str = "transit-gateway";
/// See [`TRANSIT_GATEWAY_TYPE`].
pub const TRANSIT_GATEWAY_ROUTE_TABLE_TYPE: &str = "transit-gateway-route-table";

/// The network-resource type of an attachment on a TRANSIT GATEWAY registered with a
/// global network — an Amazon VPC resource, unlike the Cloud WAN attachments this service
/// mints itself ([`ATTACHMENT_KIND`]).
///
/// One of [`NETWORK_RESOURCE_TYPES`], and the only kind a ROUTE ANALYSIS can run a routing
/// path through: the Route Analyzer analyses the path between two transit gateway
/// attachments ([`RouteAnalysisEndpointOptions`]), so resolving an endpoint means finding
/// the global network's network resource of this type with the ARN the endpoint names (see
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::analyze_route_path`]).
pub const TRANSIT_GATEWAY_ATTACHMENT_TYPE: &str = "transit-gateway-attachment";

/// Whether `resource_type` is one of the values [`NETWORK_RESOURCE_TYPES`] lists.
pub fn is_network_resource_type(resource_type: &str) -> bool {
    NETWORK_RESOURCE_TYPES.contains(&resource_type)
}

/// The three resource types TELEMETRY is reported for, each also one of
/// [`NETWORK_RESOURCE_TYPES`].
///
/// Telemetry covers a far narrower set than the network-resource inventory does: only the
/// CONNECTIONS whose session the service measures. The set is the enumeration in the
/// member's own documentation (`smithy_query query="doc"
/// shape="GetNetworkTelemetryRequest" member="ResourceType"`: `connect-peer`,
/// `transit-gateway-connect-peer`, `vpn-connection`) — a Cloud WAN Connect peer, a transit
/// gateway Connect peer, and a Site-to-Site VPN connection. The modelled
/// `ConstrainedString` carries only a length and a permissive pattern, so a value outside
/// this set is refused by the handler rather than by the schema.
///
/// [`CONNECT_PEER_TYPE`] doubles as the id prefix and the ARN resource type of the Cloud
/// WAN Connect PEER this slice mints, the way [`GLOBAL_NETWORK_KIND`] does for a global
/// network: the model constrains that id itself with `pattern
/// ^connect-peer-([0-9a-f]{8,17})$` (`smithy_query shape=ConnectPeerId`), which the 17 hex
/// digits of [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
pub const CONNECT_PEER_TYPE: &str = "connect-peer";
/// See [`CONNECT_PEER_TYPE`].
pub const TRANSIT_GATEWAY_CONNECT_PEER_TYPE: &str = "transit-gateway-connect-peer";
/// See [`CONNECT_PEER_TYPE`].
pub const VPN_CONNECTION_TYPE: &str = "vpn-connection";

/// The telemetry resource types ([`CONNECT_PEER_TYPE`] and friends) in lexicographic
/// order, so anything that walks them is stable.
pub const TELEMETRY_RESOURCE_TYPES: [&str; 3] = [
    CONNECT_PEER_TYPE,
    TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
    VPN_CONNECTION_TYPE,
];

/// Whether `resource_type` is one of the values [`TELEMETRY_RESOURCE_TYPES`] lists.
pub fn is_telemetry_resource_type(resource_type: &str) -> bool {
    TELEMETRY_RESOURCE_TYPES.contains(&resource_type)
}

/// The input member every operation that selects by resource type names in a refusal of the
/// value it carried: `ResourceType`, as the model spells the member.
pub const FIELD_RESOURCE_TYPE: &str = "ResourceType";

/// The modelled `ValidationException` (HTTP 400) for a `ResourceType` outside the set the
/// operation reports on: `scope` says, in the member's own terms, what those values are the
/// types of and `supported` lists them in the order they are declared.
///
/// The ONE construction of that refusal for this service, reached through the two checks
/// below: an operation's own predicate decides which set applies, but the refusal — its
/// `Reason`, its `Message` and the single `Fields` entry [`field_rejection`] gives the
/// member — is built here, so one rule on one member cannot answer two different bodies
/// depending on which operation refused it.
fn unsupported_resource_type(
    resource_type: &str,
    scope: &str,
    supported: &[&str],
) -> ValidationException {
    field_rejection(
        FIELD_RESOURCE_TYPE,
        REASON_FIELD_VALIDATION_FAILED,
        format!(
            "ResourceType {resource_type:?} is not one of {scope}: {}",
            supported.join(", ")
        ),
    )
}

/// Check one `ResourceType` member against the network-resource inventory's own set.
///
/// Answers `Ok(())` when the member is absent (no filter: every type is reported) or carries
/// one of [`NETWORK_RESOURCE_TYPES`], otherwise the modelled `ValidationException`
/// (HTTP 400) naming the member — a value outside that set names no resource kind this
/// service holds, so it is refused on the member rather than answered with an empty page or
/// a zero count. Shared by every operation that reports out of that inventory, so the set,
/// the predicate and the refusal cannot drift apart between them.
pub fn check_network_resource_type(resource_type: Option<&str>) -> Result<(), ValidationException> {
    match resource_type {
        Some(resource_type) if !is_network_resource_type(resource_type) => Err(
            unsupported_resource_type(
                resource_type,
                "the supported network resource types",
                &NETWORK_RESOURCE_TYPES,
            ),
        ),
        _ => Ok(()),
    }
}

/// The sibling of [`check_network_resource_type`] for the narrower set TELEMETRY is reported
/// for ([`TELEMETRY_RESOURCE_TYPES`]): the same rule on the same member over the set that
/// operation covers, refused through the same construction.
pub fn check_telemetry_resource_type(
    resource_type: Option<&str>,
) -> Result<(), ValidationException> {
    match resource_type {
        Some(resource_type) if !is_telemetry_resource_type(resource_type) => Err(
            unsupported_resource_type(
                resource_type,
                "the resource types telemetry is reported for",
                &TELEMETRY_RESOURCE_TYPES,
            ),
        ),
        _ => Ok(()),
    }
}

/// The two values `ConnectionHealth$Type` carries (`smithy_query shape=ConnectionType`,
/// mirrored in `ops::types::CONNECTION_TYPE_VALUES`: BGP, IPSEC) — the PROTOCOL whose
/// session the reported health belongs to, which is a function of the resource kind: a
/// Connect peer of either kind peers over BGP (its configuration is a list of
/// `BgpConfigurations`, `smithy_query shape=ConnectPeerConfiguration`), while a
/// Site-to-Site VPN connection is an IPsec tunnel.
pub const CONNECTION_TYPE_BGP: &str = "BGP";
/// See [`CONNECTION_TYPE_BGP`].
pub const CONNECTION_TYPE_IPSEC: &str = "IPSEC";

/// The two values `ConnectionHealth$Status` carries (`smithy_query shape=ConnectionStatus`,
/// mirrored in `ops::types::CONNECTION_STATUS_VALUES`: DOWN, UP).
pub const CONNECTION_STATUS_UP: &str = "UP";
/// See [`CONNECTION_STATUS_UP`].
pub const CONNECTION_STATUS_DOWN: &str = "DOWN";

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
/// How many connections one global network may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_CONNECTIONS_PER_GLOBAL_NETWORK: usize = 100;

/// The documented maximum length, in characters, of a device's `Vendor`, `Model` and
/// `SerialNumber`: "Constraints: Maximum length of 128 characters" on each of the three
/// (`smithy_query query="doc" shape="CreateDeviceRequest" member="Vendor"`, the same
/// sentence for `Model` and `SerialNumber`, and for the same three members of
/// `UpdateDeviceRequest`) — a bound the modelled `ConstrainedString` (len 0..256) does not
/// express, so it is checked by [`device_descriptor_problem`] instead.
pub const MAX_DEVICE_DESCRIPTOR_CHARS: usize = 128;

/// Check one free-form device descriptor against the constraint
/// [`MAX_DEVICE_DESCRIPTOR_CHARS`] documents.
///
/// Answers `None` when `value` satisfies it, otherwise the member-level reason the refusal
/// reports. Shared by every operation that takes those descriptors, so a value a create
/// refuses an update refuses too — the same seam [`link_descriptor_problem`] gives a link.
pub fn device_descriptor_problem(value: &str) -> Option<String> {
    let max = MAX_DEVICE_DESCRIPTOR_CHARS;
    let length = value.chars().count();
    (length > max).then(|| format!("must be at most {max} characters, but is {length}"))
}

/// The documented maximum length, in characters, of a link's `Type` and `Provider`, and
/// the characters the documentation forbids in them: "Constraints: Maximum length of 128
/// characters. Cannot include the following characters: | \ ^" (`smithy_query query="doc"
/// shape="CreateLinkRequest" member="Provider"`, the same sentence for `Type` and for
/// both members of `UpdateLinkRequest`) — neither of which the modelled
/// `ConstrainedString` (len 0..256, pattern `^[\s\S]*$`) expresses, so they are checked
/// by [`link_descriptor_problem`] instead.
pub const MAX_LINK_DESCRIPTOR_CHARS: usize = 128;
/// See [`MAX_LINK_DESCRIPTOR_CHARS`].
pub const FORBIDDEN_LINK_DESCRIPTOR_CHARS: [char; 3] = ['|', '\\', '^'];

/// Check one free-form link descriptor against the two constraints
/// [`MAX_LINK_DESCRIPTOR_CHARS`] documents.
///
/// Answers `None` when `value` satisfies both, otherwise the member-level reason the
/// refusal reports. Shared by every operation that takes those descriptors, so a value a
/// create refuses an update refuses too.
pub fn link_descriptor_problem(value: &str) -> Option<String> {
    let max = MAX_LINK_DESCRIPTOR_CHARS;
    let length = value.chars().count();
    if length > max {
        return Some(format!("must be at most {max} characters, but is {length}"));
    }
    value
        .chars()
        .find(|candidate| FORBIDDEN_LINK_DESCRIPTOR_CHARS.contains(candidate))
        .map(|found| {
            format!("must not include any of the characters | \\ ^, but includes {found:?}")
        })
}

/// How many core networks one global network may hold.
///
/// Unlike the three bounds above this one is the SERVICE's number, not a chosen bound:
/// Cloud WAN gives a global network a single core network ("Each global network can have
/// one core network", AWS Cloud WAN documentation), so a second create against the same
/// global network is answered with the operation's modelled
/// `ServiceQuotaExceededException` (HTTP 402) and stores nothing.
pub const MAX_CORE_NETWORKS_PER_GLOBAL_NETWORK: usize = 1;

/// How many attachments one core network may hold here.
///
/// CHOSEN, like [`MAX_SITES_PER_GLOBAL_NETWORK`]: Cloud WAN publishes a per-account
/// adjustable quota for attachments per core network and the model carries no number, so
/// this bound exists only so that a repeated create cannot grow the store without end.
/// No attachment create models a `ServiceQuotaExceededException` (`smithy_query
/// query="operation"
/// operation="CreateVpcAttachment,CreateConnectAttachment,CreateSiteToSiteVpnAttachment"`),
/// so reaching it is refused with the modelled `ValidationException` (HTTP 400) carrying
/// [`VALIDATION_REASON_OTHER`] — the request is invalid against stored state, no single
/// member is at fault — and stores nothing.
pub const MAX_ATTACHMENTS_PER_CORE_NETWORK: usize = 100;

/// How many peerings one core network may hold here.
///
/// CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`], and refused the same way:
/// `CreateTransitGatewayPeering` models no `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="CreateTransitGatewayPeering"`), so a create past this cap
/// answers the modelled `ValidationException` (HTTP 400) carrying
/// [`VALIDATION_REASON_OTHER`] and stores nothing.
pub const MAX_PEERINGS_PER_CORE_NETWORK: usize = 100;

/// How many managed prefix lists one core network may be associated with here.
///
/// CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`]: Cloud WAN publishes no number for it
/// and the model carries none, so this bound exists only so that repeated creates cannot
/// grow the store without end. `CreateCoreNetworkPrefixListAssociation` DOES model a
/// `ServiceQuotaExceededException` (`smithy_query query="operation"
/// operation="CreateCoreNetworkPrefixListAssociation"`), so reaching it is answered with
/// that modelled error (HTTP 402) and stores nothing.
pub const MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK: usize = 100;

/// How many customer gateways one global network may be associated with here.
///
/// CHOSEN, like [`MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK`] and for the same reason:
/// no published number bounds it and the model carries none, so this bound exists only so
/// that repeated associates — each naming a customer gateway of Amazon VPC, of which this
/// service holds no inventory — cannot grow the store without end.
/// `AssociateCustomerGateway` DOES model a `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="AssociateCustomerGateway"`), so reaching it is answered
/// with that modelled error (HTTP 402) and stores nothing.
pub const MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK: usize = 100;

/// How many transit gateways one global network may hold a registration for here.
///
/// CHOSEN, like [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] and for the same
/// reason: no published number bounds it and the model carries none, so this bound exists
/// only so that repeated registers — each naming a transit gateway of Amazon VPC, of which
/// this service holds no inventory — cannot grow the store without end.
/// `RegisterTransitGateway` models no `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="RegisterTransitGateway"`), so reaching this bound is
/// refused with the modelled `ValidationException` (HTTP 400) naming the member that asked,
/// and stores nothing.
pub const MAX_TRANSIT_GATEWAY_REGISTRATIONS_PER_GLOBAL_NETWORK: usize = 100;

/// How many transit gateway Connect peers one global network may hold an association for
/// here.
///
/// CHOSEN, like [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] and for the same
/// reason: no published number bounds it and the model carries none, so this bound exists
/// only so that repeated associates — each naming a Connect peer of a transit gateway of
/// Amazon VPC, of which this service holds no inventory — cannot grow the store without
/// end. `AssociateTransitGatewayConnectPeer` DOES model a `ServiceQuotaExceededException`
/// (`smithy_query query="operation" operation="AssociateTransitGatewayConnectPeer"`), so
/// reaching it is answered with that modelled error (HTTP 402) and stores nothing.
pub const MAX_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK: usize = 100;

/// How many Connect peers one Connect attachment may hold here, and how many inside CIDR
/// blocks one Connect peer may peer over.
///
/// Both are CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`] and for the same reason: the
/// model carries neither number — `ConstrainedStringList`, which `InsideCidrBlocks`
/// targets, constrains its member string only (`smithy_query shape=ConstrainedStringList`)
/// — and `CreateConnectPeer` models no `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="CreateConnectPeer"`), so their only purpose is that a
/// repeated create cannot grow the store, or one peer's list, without end. Reaching either
/// is refused with the modelled `ValidationException` (HTTP 400) naming the member that
/// asked, and stores nothing.
pub const MAX_CONNECT_PEERS_PER_ATTACHMENT: usize = 100;
/// See [`MAX_CONNECT_PEERS_PER_ATTACHMENT`].
pub const MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER: usize = 10;

/// Check the `InsideCidrBlocks` list a Connect peer create carries.
///
/// The LENGTH is judged first, against [`MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER`], so no
/// per-entry work is spent on a list already too long to be accepted. Then no block may
/// repeat: a peer peers over a block once or not at all, so a repeated entry describes
/// nothing further. What the blocks CONTAIN is not judged here — "The inside IP addresses
/// used for BGP peering" (`smithy_query query="doc" shape="CreateConnectPeerRequest"
/// member="InsideCidrBlocks"`) is an opaque `ConstrainedString` in the model — so the
/// stored list is the one the request carried.
pub fn check_inside_cidr_blocks(
    member: &str,
    requested: &[String],
) -> Result<(), ValidationException> {
    if requested.len() > MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER {
        return Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!(
                "a Connect peer peers over at most {MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER} inside CIDR blocks, but {} were named",
                requested.len()
            ),
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for block in requested {
        if !seen.insert(block.as_str()) {
            return Err(field_rejection(
                member,
                VALIDATION_REASON_OTHER,
                format!("inside CIDR block {block} is named more than once"),
            ));
        }
    }
    Ok(())
}

/// The two IP address families, spelled the way this service's documentation spells them
/// ("must be the same IP address family (IPv4 or IPv6)", `smithy_query query="doc"
/// shape="CreateConnectPeer"`). The model declares no enumeration of them, so these are the
/// words a refusal names a family by.
pub const IP_FAMILY_V4: &str = "IPv4";
/// See [`IP_FAMILY_V4`].
pub const IP_FAMILY_V6: &str = "IPv6";

/// Which IP address family `address` is in, or `None` when the value is no IP address at
/// all.
///
/// Every address member of this service targets `IPAddress`, whose modelled constraints are
/// a length and the permissive `^[\s\S]*$` (`smithy_query shape=IPAddress`), so the
/// pre-handler constraint walk cannot tell an address from any other string and what a
/// value IS has to be read here. The reading is the textual form of an IPv4 or an IPv6
/// address and nothing else: no port, no prefix length, no surrounding space.
pub fn ip_address_family(address: &str) -> Option<&'static str> {
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => Some(IP_FAMILY_V4),
        Ok(IpAddr::V6(_)) => Some(IP_FAMILY_V6),
        Err(_) => None,
    }
}

/// Check the PAIR of tunnel addresses a Connect peer create carries: each has to be an IP
/// address, and the two ends of one peering have to be in the same address family — "The
/// peer address and transit gateway address must be the same IP address family (IPv4 or
/// IPv6)" (`smithy_query query="doc" shape="CreateConnectPeer"`), the core network's end
/// being the transit gateway address that sentence names (`ConnectPeerConfiguration`
/// reports it as `CoreNetworkAddress`, "The IP address of a core network").
///
/// A BGP session runs between two addresses of ONE family, so a pair that straddles the two
/// describes a peering that cannot exist and is refused before anything is stored. The value
/// that is at fault is a MEMBER's, so the refusal is a field failure
/// ([`REASON_FIELD_VALIDATION_FAILED`]) naming the member that carried it; a mismatch is
/// blamed on the core network's end, since the appliance's end is the required member every
/// peering has and the core network's end is the one that has to agree with it.
///
/// `core_network_address` is `None` for a request naming no core network address at all,
/// which carries no second family and so can never straddle one.
pub fn check_tunnel_addresses(
    peer_member: &str,
    peer_address: &str,
    core_member: &str,
    core_network_address: Option<&str>,
) -> Result<(), ValidationException> {
    let not_an_address = |member: &str, value: &str| {
        field_rejection(
            member,
            REASON_FIELD_VALIDATION_FAILED,
            format!("{value} is not an {IP_FAMILY_V4} or {IP_FAMILY_V6} address"),
        )
    };
    let peer_family =
        ip_address_family(peer_address).ok_or_else(|| not_an_address(peer_member, peer_address))?;
    let Some(core_address) = core_network_address else {
        return Ok(());
    };
    let core_family = ip_address_family(core_address)
        .ok_or_else(|| not_an_address(core_member, core_address))?;
    if core_family != peer_family {
        return Err(field_rejection(
            core_member,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "core network address {core_address} is {core_family} and peer address \
                 {peer_address} is {peer_family}; the peer address and the core network \
                 address must be the same IP address family ({IP_FAMILY_V4} or \
                 {IP_FAMILY_V6})"
            ),
        ));
    }
    Ok(())
}

/// Check a request member the model scopes to ONE tunnel protocol against the protocol the
/// peering actually runs.
///
/// A Connect peer takes its protocol from the attachment it is carried over, and the model
/// scopes two of the create's members to a single protocol each: "This only applies only
/// when the protocol is `GRE`" on `CoreNetworkAddress` and "...when the protocol is
/// NO_ENCAP" on `SubnetArn` (`smithy_query query="doc" shape="CreateConnectPeerRequest"
/// member="CoreNetworkAddress"`, `member="SubnetArn"`). A member supplied against the OTHER
/// protocol describes configuration that peering has no place for — a tunnel address on a
/// peering that builds no tunnel, a subnet on one that reaches its appliance through a
/// tunnel — so it is refused with the modelled `ValidationException` (HTTP 400) naming the
/// member that carried it and nothing is stored. That is what keeps a stored peer from
/// contradicting the `Protocol` its own configuration reports.
///
/// The refusal is against STORED state rather than against the value — the member parses
/// and satisfies its constraints, and only the attachment's protocol makes it inapplicable
/// — so it carries [`VALIDATION_REASON_OTHER`], and it is judged after the member's own
/// value has been.
///
/// `supplied` says whether the request named the member at all (a member whose spelling
/// names nothing, like the empty `SubnetArn`, is not supplied); `applies_to` is the protocol
/// the model scopes it to; `protocol` is the attachment's, which is `None` where its options
/// named none — then no stated protocol excludes anything and the member stands.
pub fn check_protocol_scoped_member(
    member: &str,
    supplied: bool,
    applies_to: &str,
    connect_attachment_id: &str,
    protocol: Option<&str>,
) -> Result<(), ValidationException> {
    match protocol {
        Some(running) if supplied && running != applies_to => Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!(
                "{member} only applies when the protocol is {applies_to}, but Connect \
                 attachment {connect_attachment_id} carries protocol {running}"
            ),
        )),
        _ => Ok(()),
    }
}


///
/// CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`]: every `StartRouteAnalysis` records
/// one more analysis a later `GetRouteAnalysis` can read back, and neither the model nor
/// the published quotas state a number, so this bound exists only so that repeated calls
/// cannot grow the store without end. `StartRouteAnalysis` models no quota error
/// (`smithy_query query="operation" operation="StartRouteAnalysis"`), so a start past this
/// cap is refused with its modelled `ValidationException` (HTTP 400) carrying
/// [`VALIDATION_REASON_OTHER`] and records nothing.
pub const MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK: usize = 100;

/// How long after the creating call a `PENDING` resource settles to `AVAILABLE`.
///
/// AWS documents no span for this transition, so the value is CHOSEN; the emulator
/// contract fixes a chosen settle span inside 250–750 ms and forbids zero, so a caller
/// that reads straight after the create still observes the transient `PENDING` state.
/// Nothing sleeps for it: the due instant is stored and compared against the clock.
pub const SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The `PolicyVersionId` the first version of a core network's policy takes — the one a
/// create's document becomes ([`CoreNetworkRecord::add_policy_version`] mints from
/// [`CoreNetworkRecord::policy_version_counter`], which starts at zero).
pub const FIRST_POLICY_VERSION: i32 = 1;

/// How many policy versions one core network may hold.
///
/// A CHOSEN bound: a policy version is immutable ("Creates a new, immutable version of a
/// core network policy", `smithy_query query="doc" shape="PutCoreNetworkPolicy"`), so a
/// put never replaces one and the history would otherwise grow without end, and neither
/// the model nor the published quotas state a number. `PutCoreNetworkPolicy` models no
/// quota error at all (`smithy_query operation=PutCoreNetworkPolicy`), so a put past this
/// cap is refused with its modelled `ValidationException` (HTTP 400,
/// [`VALIDATION_REASON_OTHER`]) and stores nothing; deleting a version frees room for
/// another put, though never its version id ([`CoreNetworkRecord::policy_version_counter`]).
pub const MAX_POLICY_VERSIONS_PER_CORE_NETWORK: usize = 100;

/// The members of `CoreNetworkPolicyAlias` (`smithy_query shape=CoreNetworkPolicyAlias`:
/// LIVE, LATEST — mirrored in `ops::types::CORE_NETWORK_POLICY_ALIAS_VALUES`): "Whether a
/// core network policy is the current LIVE policy or the most recently submitted policy".
pub const POLICY_ALIAS_LIVE: &str = "LIVE";
/// See [`POLICY_ALIAS_LIVE`].
pub const POLICY_ALIAS_LATEST: &str = "LATEST";

/// The members of `ChangeSetState` a stored policy version reports (`smithy_query
/// shape=ChangeSetState`: PENDING_GENERATION, FAILED_GENERATION, READY_TO_EXECUTE,
/// EXECUTING, EXECUTION_SUCCEEDED, OUT_OF_DATE — mirrored in
/// `ops::types::CHANGE_SET_STATE_VALUES`).
///
/// A version whose change set has not been executed waits in
/// [`CHANGE_SET_PENDING_GENERATION`] until the span the clock owes elapses and then reads
/// [`CHANGE_SET_READY_TO_EXECUTE`]; one whose document is being put in force reads
/// [`CHANGE_SET_EXECUTING`] and then [`CHANGE_SET_EXECUTION_SUCCEEDED`]
/// ([`CoreNetworkPolicyRecord::change_set_state_at`]). An unexecuted version whose change
/// set was generated against a document that is no longer in force — because another
/// version has been put in force since — reads [`CHANGE_SET_OUT_OF_DATE`] instead and can
/// no longer be executed ([`CoreNetworkRecord::execute_policy_version`]), since the
/// differences it describes are no longer the ones executing it would apply.
/// `FAILED_GENERATION` is unreachable here because a document that does not pass
/// [`validate_policy_document`] is refused synchronously with `CoreNetworkPolicyException`
/// and never stored.
pub const CHANGE_SET_PENDING_GENERATION: &str = "PENDING_GENERATION";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_READY_TO_EXECUTE: &str = "READY_TO_EXECUTE";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_EXECUTING: &str = "EXECUTING";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_EXECUTION_SUCCEEDED: &str = "EXECUTION_SUCCEEDED";
/// See [`CHANGE_SET_PENDING_GENERATION`].
pub const CHANGE_SET_OUT_OF_DATE: &str = "OUT_OF_DATE";

/// The members of `ChangeStatus` one change of a stored policy version's change set
/// reports (`smithy_query shape=ChangeStatus`: NOT_STARTED, IN_PROGRESS, COMPLETE, FAILED
/// — mirrored in `ops::types::CHANGE_STATUS_VALUES`).
///
/// A change waits in [`CHANGE_STATUS_NOT_STARTED`] while its version's change set has not
/// been executed, reads [`CHANGE_STATUS_IN_PROGRESS`] while that execution is under way
/// and [`CHANGE_STATUS_COMPLETE`] once it has succeeded — the same clock the version's own
/// `ChangeSetState` follows ([`CoreNetworkPolicyRecord::change_set_state_at`]). `FAILED` is
/// unreachable here because a document that does not pass [`validate_policy_document`] is
/// refused synchronously and never stored, so no stored change set can fail.
pub const CHANGE_STATUS_NOT_STARTED: &str = "NOT_STARTED";
/// See [`CHANGE_STATUS_NOT_STARTED`].
pub const CHANGE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`CHANGE_STATUS_NOT_STARTED`].
pub const CHANGE_STATUS_COMPLETE: &str = "COMPLETE";

/// The members of `ChangeAction` one change reports (`smithy_query shape=ChangeAction`:
/// ADD, MODIFY, REMOVE — mirrored in `ops::types::CHANGE_ACTION_VALUES`): what the change
/// does to the object it names, which follows from which of the two compared policies
/// configures that object ([`policy_changes`]).
pub const CHANGE_ACTION_ADD: &str = "ADD";
/// See [`CHANGE_ACTION_ADD`].
pub const CHANGE_ACTION_MODIFY: &str = "MODIFY";
/// See [`CHANGE_ACTION_ADD`].
pub const CHANGE_ACTION_REMOVE: &str = "REMOVE";

/// The members of `ChangeType` the difference between two policy documents produces
/// (`smithy_query shape=ChangeType`, mirrored in `ops::types::CHANGE_TYPE_VALUES`): the
/// kinds of object a policy document configures and [`policy_topology`] therefore derives
/// — an edge, a segment at one edge location, a network function group at one edge
/// location, one static route a segment holds at one edge location, and the
/// `attachment-policies` block as a whole.
///
/// The members this list leaves out are the ones no difference between two STORED documents
/// can be seen in here. `ATTACHMENT_MAPPING`, the route-propagation kind and the
/// routing-policy kinds describe objects that are not part of a document at all. The
/// remaining configuration kinds (`CORE_NETWORK_CONFIGURATION`, `SEGMENTS_CONFIGURATION`,
/// `SEGMENT_ACTIONS_CONFIGURATION`) name blocks a document does carry, but every object
/// those blocks configure is reported under the kind that names that object — the edge, the
/// segment with what the `segment-actions` acting on it declare, the group, the static route
/// — so reporting the block as well would report one difference twice.
/// `attachment-policies` is the one block reported as a block, because it configures no
/// object of its own ([`policy_changes`]).
pub const CHANGE_TYPE_CORE_NETWORK_EDGE: &str = "CORE_NETWORK_EDGE";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`].
pub const CHANGE_TYPE_CORE_NETWORK_SEGMENT: &str = "CORE_NETWORK_SEGMENT";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`].
pub const CHANGE_TYPE_NETWORK_FUNCTION_GROUP: &str = "NETWORK_FUNCTION_GROUP";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`]: the kind of ONE static route a `create-route`
/// entry of `segment-actions` declares for a segment, which is reached through the
/// attachment its destination names.
pub const CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC: &str = "ATTACHMENT_ROUTE_STATIC";
/// See [`CHANGE_TYPE_CORE_NETWORK_EDGE`].
pub const CHANGE_TYPE_ATTACHMENT_POLICIES_CONFIGURATION: &str =
    "ATTACHMENT_POLICIES_CONFIGURATION";

/// How this service names ONE policy version of a core network in the required
/// `ResourceId` of a modelled refusal: `<CoreNetworkId>/<PolicyVersionId>`.
///
/// A version id is an `Integer` that is unique only inside its core network, so neither
/// half identifies the addressed policy on its own.
pub fn policy_version_resource_id(core_network_id: &str, version_id: i32) -> String {
    format!("{core_network_id}/{version_id}")
}

/// How this service names the policy a request addressed by ALIAS rather than by version
/// id: `<CoreNetworkId>/<Alias>`.
///
/// A request that selects a policy by alias names no version id, and a core network may
/// hold no version for that alias to resolve to, so this keeps a refusal's `ResourceId`
/// reading `<CoreNetworkId>/<selector>` whichever selector the request used, beside the
/// same `ResourceType` ([`CORE_NETWORK_POLICY_KIND`]).
pub fn policy_alias_resource_id(core_network_id: &str, alias: &str) -> String {
    format!("{core_network_id}/{alias}")
}

/// How a paginated listing of a core network's policy versions keys ONE version.
///
/// A version id is an `Integer` (`smithy_query shape=CoreNetworkPolicyVersion
/// member=PolicyVersionId`) while [`paginate`] resumes a page after a STRING key, so the
/// id is zero-padded to the widest a positive one can be: that keeps the lexicographic
/// order the page token is compared in the numeric order of the history, so version 10
/// never sorts before version 2.
pub fn policy_version_page_key(version_id: i32) -> String {
    format!("{version_id:010}")
}

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
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the global network and `None` while no update
    /// has ever touched it. See [`GlobalNetworkRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl GlobalNetworkRecord {
    /// This network's lifecycle state as of `now`.
    ///
    /// Both transitions are a function of the clock, so reading one never writes: a global
    /// network is `UPDATING` from an update until that reconfiguration is applied,
    /// `PENDING` until the create settles, and `AVAILABLE` once neither is outstanding. An
    /// update OUTSTANDING is what it reports even when the create has not settled yet,
    /// since that update is the later of the two requests it is still working on — the rule
    /// [`LinkRecord::state_at`] states for every record of this service whose state enum
    /// carries `UPDATING`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
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
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the site and `None` while no update has ever
    /// touched it. See [`SiteRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl SiteRecord {
    /// This site's lifecycle state as of `now`, by the rule [`LinkRecord::state_at`]
    /// states: `UPDATING` while an update is outstanding, `PENDING` until the create
    /// settles, and `AVAILABLE` once neither is.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
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
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the link and `None` while no update has ever
    /// touched it. See [`LinkRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl LinkRecord {
    /// This link's lifecycle state as of `now`.
    ///
    /// Both transitions are a function of the clock, so reading one never writes: a link
    /// is `UPDATING` from an update until that reconfiguration is applied, `PENDING` until
    /// the create settles, and `AVAILABLE` once neither is outstanding. An update
    /// OUTSTANDING is what the link reports even when the create has not settled yet,
    /// since that update is the later of the two requests the link is still working on.
    ///
    /// This is the rule EVERY record of this service whose state enum carries `UPDATING`
    /// follows ([`GlobalNetworkRecord::state_at`], [`SiteRecord::state_at`],
    /// [`DeviceRecord::state_at`], [`ConnectionRecord::state_at`],
    /// [`CoreNetworkRecord::state_at`], [`AttachmentRecord::state_at`]): an update of such
    /// a resource is not applied instantly, so the resource reports the non-terminal state
    /// its reconfiguration is outstanding in until the transition the clock owes falls due.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
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
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the device and `None` while no update has ever
    /// touched it. See [`DeviceRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl DeviceRecord {
    /// This device's lifecycle state as of `now`. `DeviceState` carries the same four
    /// values as the three enums above (`smithy_query shape=DeviceState`: PENDING,
    /// AVAILABLE, DELETING, UPDATING), so a device waits in [`STATE_PENDING`] too and
    /// reports [`STATE_UPDATING`] while a reconfiguration is outstanding, exactly as
    /// [`LinkRecord::state_at`] reports a link's.
    ///
    /// Both transitions are a function of the clock, so reading a device never writes, and
    /// an outstanding update is what it reports even when the create has not settled yet:
    /// that update is the later of the two requests it is still working on.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
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

/// A stored core network policy VERSION: one immutable document a core network holds.
///
/// "Creates a new, immutable version of a core network policy" (`smithy_query
/// query="doc" shape="PutCoreNetworkPolicy"`), so a stored version is never edited: a
/// later put mints the next version beside it. The document lives HERE and nowhere else
/// — a core network's LIVE document is the one of the version
/// [`CoreNetworkRecord::live_policy_version`] names — so no two records of this service
/// hold a copy of the same policy.
#[derive(Debug, Clone)]
pub struct CoreNetworkPolicyRecord {
    /// The core network this version belongs to, carried so a read of the version reports
    /// it without the reader having to pass it back in.
    pub core_network_id: String,
    pub version_id: i32,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the change-set transition the clock owes this version becomes due:
    /// `PENDING_GENERATION` -> `READY_TO_EXECUTE` while it waits to be executed, or
    /// `EXECUTING` -> `EXECUTION_SUCCEEDED` while its document is being put in force.
    pub settled_at: SystemTime,
    /// The policy document, verbatim as the request supplied it.
    pub document: String,
    /// The topology [`policy_topology`] derived from [`Self::document`], derived ONCE where
    /// that document was accepted — before the store was locked, by the request that
    /// carried it — and kept here beside it.
    ///
    /// A version's document never changes, so neither does this: it is what the core
    /// network reports while this version is the LIVE one
    /// ([`CoreNetworkRecord::topology`]), the submitted side of this version's own change
    /// set, and what a restore of this version carries to the version it mints. Nothing
    /// re-parses the document to get it, so no operation of this service parses a policy
    /// document while holding the store lock. What it holds is bounded by the caps
    /// [`validate_policy_document`] put on the document before it was stored — the topology
    /// is linear in the document it came from — and a version is itself bounded by
    /// [`MAX_POLICY_VERSIONS_PER_CORE_NETWORK`].
    pub topology: PolicyTopology,
    /// The `ClientToken` of the `PutCoreNetworkPolicy` that minted this version, kept so a
    /// retry of that request answers this version instead of minting a second one.
    pub client_token: Option<String>,
    /// The instant the execution of this version's change set began — which is what puts
    /// its document in force — and `None` for a version whose change set has not been
    /// executed. The version a create carries is executed by the create itself ("A
    /// subsequent change set is created showing the differences between the LIVE policy and
    /// the submitted policy", `smithy_query query="doc" shape="PutCoreNetworkPolicy"` — a
    /// put only PREPARES one); the change set a put or a restore prepared is executed by
    /// `ExecuteCoreNetworkChangeSet` ([`CoreNetworkRecord::execute_policy_version`]).
    pub executed_at: Option<SystemTime>,
    /// Whether this version's change set describes differences from a document that is no
    /// longer in force, which is what makes it [`CHANGE_SET_OUT_OF_DATE`].
    ///
    /// A change set is generated ONCE, against the document in force at that moment, so
    /// putting ANOTHER version in force leaves every change set still waiting to be
    /// executed describing a comparison that no longer holds — it is the stored flag rather
    /// than a function of the clock because what invalidates it is that later execution,
    /// not the passing of time. An executed version is unaffected: its change set has
    /// already been applied.
    pub out_of_date: bool,
    /// The changes this version's change set carries: the difference between the policy
    /// that was in force when this version was stored and this version's own document
    /// ([`policy_changes`]).
    ///
    /// Computed ONCE, where the version is stored, because that is when the change set is
    /// generated — "a subsequent change set is created showing the differences between the
    /// LIVE policy and the submitted policy" (`smithy_query query="doc"
    /// shape="PutCoreNetworkPolicy"`) — so a later put that moves the LIVE policy does not
    /// rewrite the change set of a version already stored, and a read of these changes
    /// never re-parses a document. The list is bounded by the caps
    /// [`validate_policy_document`] puts on the two documents compared, and a version is
    /// itself bounded by [`MAX_POLICY_VERSIONS_PER_CORE_NETWORK`].
    pub changes: Vec<PolicyChange>,
}

impl CoreNetworkPolicyRecord {
    /// Whether this version's change set has been executed, which is what puts its
    /// document in force.
    pub fn executed(&self) -> bool {
        self.executed_at.is_some()
    }

    /// This version's `ChangeSetState` as of `now` — a function of the clock and of the
    /// stored record, so reading a version never writes. See
    /// [`CHANGE_SET_PENDING_GENERATION`].
    pub fn change_set_state_at(&self, now: SystemTime) -> &'static str {
        // A change set superseded by another version's execution can never be executed, so
        // it reports that instead of the state the clock would otherwise give it; only an
        // unexecuted one can be superseded.
        if !self.executed() && self.out_of_date {
            return CHANGE_SET_OUT_OF_DATE;
        }
        match (self.executed(), now >= self.settled_at) {
            (false, false) => CHANGE_SET_PENDING_GENERATION,
            (false, true) => CHANGE_SET_READY_TO_EXECUTE,
            (true, false) => CHANGE_SET_EXECUTING,
            (true, true) => CHANGE_SET_EXECUTION_SUCCEEDED,
        }
    }

    /// The change events of this version's change set as of `now`, each keyed by its
    /// `IdentifierPath` — in that key's order, which is the order
    /// [`policy_changes`] records the changes in.
    ///
    /// One event per change the change set carries, reporting the status that change's
    /// execution is in as of `now` and the instant it last entered that status: the version
    /// was stored (nothing executed yet), the execution began, or the execution succeeded.
    /// Both are functions of stored instants and the clock, so reading them writes nothing
    /// and two reads of an execution that has settled report the same events.
    pub fn change_events_at(&self, now: SystemTime) -> Vec<(String, CoreNetworkChangeEvent)> {
        let (status, event_time) = match self.executed_at {
            Some(_) if now >= self.settled_at => (CHANGE_STATUS_COMPLETE, self.settled_at),
            Some(started_at) => (CHANGE_STATUS_IN_PROGRESS, started_at),
            None => (CHANGE_STATUS_NOT_STARTED, self.created_at),
        };
        self.changes
            .iter()
            .map(|change| {
                (
                    change.identifier_path(),
                    change.to_change_event(status, event_time),
                )
            })
            .collect()
    }

    /// The changes of this version's change set, each keyed by its `IdentifierPath` — in
    /// that key's order, which is the order [`policy_changes`] records the changes in.
    ///
    /// The SAME stored changes [`Self::change_events_at`] reports as the events of this
    /// version's execution, reported instead as the change set itself: what each change
    /// does to the object it names, with the values that object held under the policy this
    /// version was submitted against and the ones this version's document gives it. Unlike
    /// an event, a change carries no status and no instant, so this answer is independent
    /// of the clock: the change set is a property of the version, not of its execution.
    pub fn change_set(&self) -> Vec<(String, CoreNetworkChange)> {
        self.changes
            .iter()
            .map(|change| (change.identifier_path(), change.to_core_network_change()))
            .collect()
    }

    /// The modelled `CoreNetworkPolicy` for this version, as of `now`, carrying the
    /// `alias` its core network gives it ([`CoreNetworkRecord::policy_alias_of`]).
    pub fn to_shape(&self, alias: Option<&str>, now: SystemTime) -> CoreNetworkPolicy {
        CoreNetworkPolicy {
            core_network_id: Some(self.core_network_id.clone()),
            policy_version_id: Some(self.version_id),
            alias: alias.map(str::to_string),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            change_set_state: Some(self.change_set_state_at(now).to_string()),
            // `PolicyErrors` "describes any errors in a core network policy": a stored
            // version passed [`validate_policy_document`], so it carries none and the
            // member is absent rather than an empty list.
            policy_errors: None,
            policy_document: Some(self.document.clone()),
        }
    }

    /// The modelled `CoreNetworkPolicyVersion` for this version, as of `now`, carrying the
    /// `alias` its core network gives it ([`CoreNetworkRecord::policy_alias_of`]).
    ///
    /// The SUMMARY of the version a listing reports: the same members as
    /// [`Self::to_shape`] without the document and its errors (`smithy_query
    /// shape=CoreNetworkPolicyVersion`), so a listing and a read of one version agree on
    /// everything they both report.
    pub fn to_version_shape(
        &self,
        alias: Option<&str>,
        now: SystemTime,
    ) -> CoreNetworkPolicyVersion {
        CoreNetworkPolicyVersion {
            core_network_id: Some(self.core_network_id.clone()),
            policy_version_id: Some(self.version_id),
            alias: alias.map(str::to_string),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            change_set_state: Some(self.change_set_state_at(now).to_string()),
        }
    }
}

/// A stored link association: the tie between one device and one link of the same global
/// network and the same site.
///
/// The association has no id and no ARN of its own — the modelled `LinkAssociation` is
/// the device/link pair plus its state (`smithy_query shape=LinkAssociation`) — so the
/// pair is its identity and the store keys it by [`link_association_key`]. Like the
/// records above, its state is a function of the clock rather than a field, so reading
/// one never writes.
#[derive(Debug, Clone)]
pub struct LinkAssociationRecord {
    pub global_network_id: String,
    pub device_id: String,
    pub link_id: String,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl LinkAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `LinkAssociationState` carries PENDING, AVAILABLE, DELETING and DELETED
    /// (`smithy_query shape=LinkAssociationState`, mirrored in
    /// `ops::types::LINK_ASSOCIATION_STATE_VALUES`); the first three are spelled exactly
    /// as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`], so this slice
    /// reuses those constants.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `LinkAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> LinkAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `LinkAssociation` for this record, forcing `state` — used by
    /// `DisassociateLink`, which reports the association it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> LinkAssociation {
        LinkAssociation {
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: Some(self.link_id.clone()),
            link_association_state: Some(state.to_string()),
        }
    }
}

/// The key the association store holds a device/link pair under: the device id, then the
/// link id, so the map orders associations by device and then by link and a pagination
/// token can name an offset into it.
///
/// An association is only ever stored for a device and a link this service minted ids for
/// ([`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`]), each `<kind>-<17 hex>`,
/// so no two pairs can produce the same key. The same string identifies the association
/// in a not-found refusal, where no single resource id would.
pub fn link_association_key(device_id: &str, link_id: &str) -> String {
    format!("{device_id}/{link_id}")
}

/// A stored customer gateway association: the tie between one customer gateway of Amazon
/// VPC and one device — and optionally one link — of a global network.
///
/// Like [`LinkAssociationRecord`] this is NOT a resource of its own: the modelled
/// `CustomerGatewayAssociation` carries no id and no ARN of its own (`smithy_query
/// shape=CustomerGatewayAssociation`), so the global network plus the customer gateway ARN
/// is its identity and the store keys it by [`customer_gateway_association_key`]. Its
/// state is a function of the clock rather than a field, so reading one never writes.
#[derive(Debug, Clone)]
pub struct CustomerGatewayAssociationRecord {
    pub global_network_id: String,
    /// The ARN of the customer gateway, as the request carried it: the gateway belongs to
    /// Amazon VPC, so this service stores the reference and holds no record of the gateway
    /// itself (see [`parse_customer_gateway_arn`]).
    pub customer_gateway_arn: String,
    pub device_id: String,
    /// The link the association names, or `None` when the associate did not name one —
    /// `LinkId` is the one optional member of the request (`smithy_query
    /// query="operation" operation="AssociateCustomerGateway"`).
    pub link_id: Option<String>,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl CustomerGatewayAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `CustomerGatewayAssociationState` carries PENDING, AVAILABLE, DELETING and DELETED
    /// (`smithy_query shape=CustomerGatewayAssociationState`, mirrored in
    /// `ops::types::CUSTOMER_GATEWAY_ASSOCIATION_STATE_VALUES`); the first three are
    /// spelled exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`],
    /// so this slice reuses those constants.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `CustomerGatewayAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> CustomerGatewayAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `CustomerGatewayAssociation` for this record, forcing `state` — used by
    /// `DisassociateCustomerGateway`, which reports the association it just removed as
    /// `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> CustomerGatewayAssociation {
        CustomerGatewayAssociation {
            customer_gateway_arn: Some(self.customer_gateway_arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: self.link_id.clone(),
            state: Some(state.to_string()),
        }
    }
}

/// The key the store holds a customer gateway association under: the global network it was
/// made in, then the ARN of the customer gateway it ties to one of that network's devices.
///
/// The ARN alone is not the key: the association is addressed THROUGH the global network in
/// every one of the three operations' URIs (`smithy_query query="operation"
/// operation="AssociateCustomerGateway,DisassociateCustomerGateway,GetCustomerGatewayAssociations"`),
/// so a pair made in a DIFFERENT global network is not addressable here — exactly as
/// [`prefix_list_association_key`] scopes a prefix list to its core network. The same
/// string identifies the association in a not-found refusal, where no single resource id
/// would.
pub fn customer_gateway_association_key(global_network_id: &str, customer_gateway_arn: &str) -> String {
    format!("{global_network_id}/{customer_gateway_arn}")
}

/// A stored transit gateway registration: the tie between one transit gateway of Amazon VPC
/// and one global network.
///
/// Like [`CustomerGatewayAssociationRecord`] this is NOT a resource of its own: the modelled
/// `TransitGatewayRegistration` carries the global network, the gateway's ARN and a state
/// reason, with no id and no ARN of its own (`smithy_query
/// shape=TransitGatewayRegistration`), so the global network plus the gateway's ARN is its
/// identity and the store keys it by [`transit_gateway_registration_key`]. Its state is a
/// function of the clock rather than a field, so reading one never writes.
#[derive(Debug, Clone)]
pub struct TransitGatewayRegistrationRecord {
    pub global_network_id: String,
    /// The ARN of the transit gateway, as the request carried it: the gateway belongs to
    /// Amazon VPC, so this service stores the reference and holds no record of the gateway
    /// itself (see [`ec2_resource_region`]).
    pub transit_gateway_arn: String,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl TransitGatewayRegistrationRecord {
    /// This registration's state as of `now`.
    ///
    /// `TransitGatewayRegistrationState` carries PENDING, AVAILABLE, DELETING, DELETED and
    /// FAILED (`smithy_query shape=TransitGatewayRegistrationState`, mirrored in
    /// `ops::types::TRANSIT_GATEWAY_REGISTRATION_STATE_VALUES`); the first three are spelled
    /// exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`], so this
    /// slice reuses those constants. A stored registration is one of the first two: nothing
    /// here fails a registration, and a deregistered one is removed rather than kept.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `TransitGatewayRegistration` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> TransitGatewayRegistration {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `TransitGatewayRegistration` for this record, forcing `state` — used by
    /// `DeregisterTransitGateway`, which reports the registration it just removed as
    /// `DELETING`.
    ///
    /// `State$Message` is left absent: "The message for the state reason" (`smithy_query
    /// query="doc" shape="TransitGatewayRegistrationStateReason" member="Message"`) is what
    /// a service reporting WHY a registration reached its state fills in, and the two states
    /// a registration of this store reaches are the ordinary ones the code already names.
    pub fn to_shape_with_state(&self, state: &str) -> TransitGatewayRegistration {
        TransitGatewayRegistration {
            global_network_id: Some(self.global_network_id.clone()),
            transit_gateway_arn: Some(self.transit_gateway_arn.clone()),
            state: Some(TransitGatewayRegistrationStateReason {
                code: Some(state.to_string()),
                message: None,
            }),
        }
    }
}

/// The key the store holds a transit gateway registration under: the global network it was
/// made in, then the ARN of the transit gateway registered with it.
///
/// Like [`customer_gateway_association_key`], the ARN alone is not the key: the registration
/// is addressed THROUGH the global network in every one of the three operations' URIs
/// (`smithy_query query="operation"
/// operation="RegisterTransitGateway,DeregisterTransitGateway,GetTransitGatewayRegistrations"`).
/// The same string identifies the registration in a not-found refusal, where no single
/// resource id would.
pub fn transit_gateway_registration_key(
    global_network_id: &str,
    transit_gateway_arn: &str,
) -> String {
    format!("{global_network_id}/{transit_gateway_arn}")
}

/// A stored transit gateway Connect peer association: the tie between one Connect peer of a
/// transit gateway of Amazon VPC and one DEVICE of a global network, optionally over one of
/// that device's links.
///
/// "Associates a transit gateway Connect peer with a device, and optionally, with a link."
/// (`smithy_query query="doc" shape="AssociateTransitGatewayConnectPeer"`) \u2014 like
/// [`CustomerGatewayAssociationRecord`] the tie carries no id and no ARN of its own (the
/// modelled `TransitGatewayConnectPeerAssociation` is the peer's ARN, the global network,
/// the device, the link and a state: `smithy_query
/// shape=TransitGatewayConnectPeerAssociation`), so the global network plus the peer's ARN
/// is its identity and the store keys it by
/// [`transit_gateway_connect_peer_association_key`] \u2014 which is exactly what
/// `DisassociateTransitGatewayConnectPeer` addresses, naming those two and nothing else
/// (`smithy_query query="operation" operation="DisassociateTransitGatewayConnectPeer"`).
///
/// Like the records above, its state is a function of the clock rather than a field, so
/// reading one never writes.
#[derive(Debug, Clone)]
pub struct TransitGatewayConnectPeerAssociationRecord {
    pub global_network_id: String,
    /// The ARN of the transit gateway Connect peer, as the request carried it: the peer
    /// belongs to Amazon VPC, so this service stores the reference and holds no record of
    /// the peer itself (see [`ec2_resource_region`]).
    pub transit_gateway_connect_peer_arn: String,
    pub device_id: String,
    /// The link the device reaches this peering over, or `None` where the association named
    /// none \u2014 `LinkId` is the one optional member of the request.
    pub link_id: Option<String>,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl TransitGatewayConnectPeerAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `TransitGatewayConnectPeerAssociationState` carries PENDING, AVAILABLE, DELETING and
    /// DELETED (`smithy_query shape=TransitGatewayConnectPeerAssociationState`, mirrored in
    /// `ops::types::TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_STATE_VALUES`); the first three
    /// are spelled exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`],
    /// so this slice reuses those constants, as [`CustomerGatewayAssociationRecord`] does.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `TransitGatewayConnectPeerAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> TransitGatewayConnectPeerAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `TransitGatewayConnectPeerAssociation` for this record, forcing `state`
    /// \u2014 used by `DisassociateTransitGatewayConnectPeer`, which reports the association it
    /// just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> TransitGatewayConnectPeerAssociation {
        TransitGatewayConnectPeerAssociation {
            transit_gateway_connect_peer_arn: Some(self.transit_gateway_connect_peer_arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: self.link_id.clone(),
            state: Some(state.to_string()),
        }
    }
}

/// The key the store holds a transit gateway Connect peer association under: the global
/// network it was made in, then the ARN of the Connect peer tied to one of that network's
/// devices.
///
/// Like [`customer_gateway_association_key`], the ARN alone is not the key: the association
/// is addressed THROUGH the global network in every one of the three operations' URIs
/// (`smithy_query query="operation"
/// operation="AssociateTransitGatewayConnectPeer,DisassociateTransitGatewayConnectPeer,GetTransitGatewayConnectPeerAssociations"`),
/// so a tie made in a DIFFERENT global network is not addressable here. The same string
/// identifies the association in a not-found refusal, where no single resource id would.
pub fn transit_gateway_connect_peer_association_key(
    global_network_id: &str,
    transit_gateway_connect_peer_arn: &str,
) -> String {
    format!("{global_network_id}/{transit_gateway_connect_peer_arn}")
}

/// A stored connection: the tie between two devices of one global network.
///
/// "Creates a connection between two devices" (`smithy_query query="doc"
/// shape="CreateConnection"`), so a connection hangs off the GLOBAL NETWORK and names two
/// of its devices; the two link members are optional, each naming the link that device
/// reaches the connection over. Unlike a link association this one IS a resource: it has
/// an id, an ARN and tags of its own. Like every record above, its state is a function of
/// the clock rather than a field, so reading one never writes.
#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    /// The first device in the connection.
    pub device_id: String,
    /// The second device in the connection.
    pub connected_device_id: String,
    /// The link the first device reaches the connection over, when the request named one.
    pub link_id: Option<String>,
    /// The link the second device reaches the connection over, when the request named one.
    pub connected_link_id: Option<String>,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the connection and `None` while no update has
    /// ever touched it. See [`ConnectionRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl ConnectionRecord {
    /// This connection's lifecycle state as of `now`. `ConnectionState` carries the same
    /// four values as the enums above (`smithy_query shape=ConnectionState`: PENDING,
    /// AVAILABLE, DELETING, UPDATING, mirrored in `ops::types::CONNECTION_STATE_VALUES`),
    /// so a connection waits in [`STATE_PENDING`] too and reports [`STATE_UPDATING`] while
    /// a reconfiguration is outstanding, by the rule [`LinkRecord::state_at`] states.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `Connection` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Connection {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Connection` for this record, forcing `state` — for an operation that
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Connection {
        Connection {
            connection_id: Some(self.id.clone()),
            connection_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            connected_device_id: Some(self.connected_device_id.clone()),
            link_id: self.link_id.clone(),
            connected_link_id: self.connected_link_id.clone(),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored core network: the Cloud WAN managed network a global network owns.
///
/// Its `state` is a function of the clock, like the records above, and its topology
/// (`Segments`, `NetworkFunctionGroups`, `Edges`) is the one derived from the LIVE policy
/// version's document where that document was accepted
/// ([`CoreNetworkPolicyRecord::topology`], reached through [`Self::topology`]) — nothing
/// about a core network is kept twice and no read re-parses the document.
#[derive(Debug, Clone)]
pub struct CoreNetworkRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// Every policy version this core network holds, by `PolicyVersionId`. The documents
    /// live in these records alone, so a read of a version reports what was put, and the
    /// LIVE document is the document of the version [`Self::live_policy_version`] names.
    pub policy_versions: BTreeMap<i32, CoreNetworkPolicyRecord>,
    /// The version whose document is in force (the one aliased `LIVE`), or `None` for a
    /// core network created without a policy document.
    pub live_policy_version: Option<i32>,
    /// The highest `PolicyVersionId` ever minted here. A version is immutable, so its id
    /// is never reused: the next put takes this plus one even when the version that held
    /// it has since been deleted.
    pub policy_version_counter: i32,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured this core network — a description the caller
    /// supplied, or the execution that put a policy version in force — and `None` while no
    /// update has ever touched it. See [`CoreNetworkRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    /// Whether the CREATE of this core network carried a policy document, which is what
    /// makes its first version the one [`Self::created_policy_document`] answers.
    pub created_with_policy_document: bool,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second one.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
}

impl CoreNetworkRecord {
    /// The document the CREATE of this core network carried: its first policy version,
    /// `None` for a core network created without a document.
    ///
    /// The version is the FIRST one of a core network the create gave a document to
    /// ([`Self::created_with_policy_document`]), so a put that mints version 1 on a core
    /// network created without a document does not make this answer the put's document
    /// — not even once that version's change set has been executed. This, and not the LIVE
    /// document, is what a retried create compares itself against: the LIVE document moves
    /// to whichever version is executed next, while the document a create carried is the
    /// one it is idempotent in.
    pub fn created_policy_document(&self) -> Option<&str> {
        if !self.created_with_policy_document {
            return None;
        }
        self.policy_versions
            .get(&FIRST_POLICY_VERSION)
            .map(|version| version.document.as_str())
    }

    /// The highest `PolicyVersionId` this core network still holds, or `None` while it
    /// holds no version at all.
    pub fn latest_policy_version(&self) -> Option<i32> {
        self.policy_versions.keys().next_back().copied()
    }

    /// The alias of the version `version_id`: `LIVE` for the version in force, `LATEST`
    /// for the newest version held when that is not the one in force, and none for every
    /// other version — the model allows a policy a single alias
    /// ([`POLICY_ALIAS_LIVE`]), and the policy in force is the one a caller means by it.
    pub fn policy_alias_of(&self, version_id: i32) -> Option<&'static str> {
        if self.live_policy_version == Some(version_id) {
            Some(POLICY_ALIAS_LIVE)
        } else if self.latest_policy_version() == Some(version_id) {
            Some(POLICY_ALIAS_LATEST)
        } else {
            None
        }
    }

    /// The modelled `CoreNetworkPolicy` for the version `version_id` as of `now`, or
    /// `None` when this core network holds no such version.
    pub fn policy_shape(&self, version_id: i32, now: SystemTime) -> Option<CoreNetworkPolicy> {
        let alias = self.policy_alias_of(version_id);
        self.policy_versions
            .get(&version_id)
            .map(|version| version.to_shape(alias, now))
    }

    /// Every policy version this core network holds as of `now`, each keyed by
    /// [`policy_version_page_key`] so [`paginate`] can page over them.
    ///
    /// In the order of the HISTORY — the order of the key the versions are stored under,
    /// oldest version id first — which is the key order every paginated listing of this
    /// service reports in and the order its page tokens resume after.
    pub fn policy_version_entries(
        &self,
        now: SystemTime,
    ) -> Vec<(String, CoreNetworkPolicyVersion)> {
        self.policy_versions
            .values()
            .map(|version| {
                (
                    policy_version_page_key(version.version_id),
                    version.to_version_shape(self.policy_alias_of(version.version_id), now),
                )
            })
            .collect()
    }

    /// The topology of the policy in force on this core network: the one derived from the
    /// LIVE version's document where that document was accepted
    /// ([`CoreNetworkPolicyRecord::topology`]), or the topology of no policy at all
    /// ([`NO_POLICY_IN_FORCE`]) while no version is in force — a core network created
    /// without a policy document, until a version's change set is executed.
    ///
    /// Derived data is kept beside the document it comes from and nowhere else, so this is
    /// a view of the LIVE version rather than a second copy: putting another version in
    /// force moves which version answers here and copies nothing.
    pub fn topology(&self) -> &PolicyTopology {
        self.live_policy_version
            .and_then(|live| self.policy_versions.get(&live))
            .map_or(&NO_POLICY_IN_FORCE, |live| &live.topology)
    }

    /// Store `document`, whose derived topology is `topology`, as this core network's next
    /// policy version and answer that version's modelled `CoreNetworkPolicy`.
    ///
    /// `topology` is the answer [`policy_topology`] gives for `document`, derived by the
    /// caller where the document was ACCEPTED and before the store was locked, so
    /// appending a version parses no policy document: the work done here is a comparison
    /// of two topologies both already derived and both bounded by
    /// [`validate_policy_document`].
    ///
    /// `executed` says whether this version's change set is being executed, which is what
    /// puts its document in force: an executed version becomes the LIVE one. A freshly
    /// minted version always carries the highest id in the history, so its alias follows
    /// from `executed` alone — `LIVE` for the version just put in force, `LATEST`
    /// otherwise — and the answer is built from the record itself rather than looked up
    /// again.
    pub fn add_policy_version(
        &mut self,
        document: String,
        topology: PolicyTopology,
        description: Option<String>,
        client_token: Option<String>,
        now: SystemTime,
        executed: bool,
    ) -> CoreNetworkPolicy {
        let version_id = self.policy_version_counter.saturating_add(1);
        self.policy_version_counter = version_id;
        // The change set generated with the version: the difference between the policy in
        // force right now and the one being stored.
        let changes = policy_changes(self.topology(), &topology);
        let record = CoreNetworkPolicyRecord {
            core_network_id: self.id.clone(),
            version_id,
            description,
            created_at: now,
            settled_at: now + SETTLE_DELAY,
            document,
            topology,
            client_token,
            executed_at: executed.then_some(now),
            // Freshly generated against the document in force a moment ago, so it is the
            // comparison that holds right now; a later execution of ANOTHER version is
            // what supersedes it.
            out_of_date: false,
            changes,
        };
        let alias = if executed {
            POLICY_ALIAS_LIVE
        } else {
            POLICY_ALIAS_LATEST
        };
        let shape = record.to_shape(Some(alias), now);
        self.policy_versions.insert(version_id, record);
        if executed {
            self.live_policy_version = Some(version_id);
        }
        shape
    }

    /// Put the policy version `version_id`'s document in force: what executing that
    /// version's change set does. A no-op for a version this core network does not hold,
    /// which a caller refuses before reaching here.
    ///
    /// Executing a change set makes its version the LIVE one — and with it the topology
    /// its document describes the one the core network reports ([`Self::topology`]) —
    /// starts the `EXECUTING` ->
    /// `EXECUTION_SUCCEEDED` span the clock then owes that change set (and with it the
    /// `IN_PROGRESS` -> `COMPLETE` span of each of its changes,
    /// [`CoreNetworkPolicyRecord::change_events_at`]) and the `UPDATING` -> `AVAILABLE`
    /// span it owes the core network being deployed to. Every OTHER change set still
    /// waiting to be executed described its differences from the document this one
    /// replaces, so none of them is a comparison that still holds: they are superseded
    /// ([`CoreNetworkPolicyRecord::out_of_date`]).
    pub fn execute_policy_version(&mut self, version_id: i32, now: SystemTime) {
        let Some(version) = self.policy_versions.get_mut(&version_id) else {
            return;
        };
        version.executed_at = Some(now);
        version.settled_at = now + SETTLE_DELAY;
        for (id, superseded) in self.policy_versions.iter_mut() {
            if *id != version_id && superseded.executed_at.is_none() {
                superseded.out_of_date = true;
            }
        }
        // The topology this core network reports follows from WHICH version is live: it was
        // derived where that version's document was accepted, so putting the document in
        // force re-parses nothing here.
        self.live_policy_version = Some(version_id);
        self.updating_until = Some(now + SETTLE_DELAY);
    }

    /// This core network's lifecycle state as of `now`.
    ///
    /// The transition is a function of the clock, so reading one never writes: a core
    /// network is `UPDATING` while a reconfiguration of it is still outstanding — a policy
    /// version being put in force, or an update of its own members — and `AVAILABLE` once
    /// none is, which is the state it is created in. `CreateCoreNetwork` answers the core
    /// network ALREADY `AVAILABLE`, so nothing is owed on its creation and the modelled
    /// `CREATING` ([`STATE_CREATING`]) is a state no core network of this service passes
    /// through.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `CoreNetwork` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> CoreNetwork {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `CoreNetwork` for this record, forcing `state` — used by a delete,
    /// which reports the record it just removed as `DELETING`.
    ///
    /// A core network's segments and edges are the ones the policy in force configures, so
    /// a record being REMOVED — the one state a stored core network never reports itself in
    /// — carries no topology, and every other state carries the one derived from the stored
    /// policy document. A policy being deployed over it (`UPDATING`) does not take the
    /// topology away: a policy IS in force there, the one that execution put in force.
    pub fn to_shape_with_state(&self, state: &str) -> CoreNetwork {
        let topology = if state == STATE_DELETING {
            &NO_POLICY_IN_FORCE
        } else {
            self.topology()
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

/// A stored association between a core network and a managed prefix list: the tie
/// `CreateCoreNetworkPrefixListAssociation` makes "for routing control" (`smithy_query
/// query="doc" shape="CreateCoreNetworkPrefixListAssociation"`).
///
/// Like [`LinkAssociationRecord`] this is NOT a resource of its own: the modelled
/// `PrefixListAssociation` is the core network id, the prefix list ARN and the alias
/// (`smithy_query shape=PrefixListAssociation`), with no id, ARN or state member, so the
/// core network / prefix list pair is its identity and the store keys it by
/// [`prefix_list_association_key`]. Carrying no state member, it has nothing that is a
/// function of the clock and no transition is owed on it.
#[derive(Debug, Clone)]
pub struct PrefixListAssociationRecord {
    /// The core network the association was made on: the one whose prefix-list
    /// associations list it and whose delete removes it.
    pub core_network_id: String,
    /// The ARN of the associated prefix list, exactly as accepted. A managed prefix list is
    /// an Amazon VPC resource, so this service mints nothing for it and stores no record of
    /// the list itself — [`parse_prefix_list_arn`] judges only the form of the ARN.
    pub prefix_list_arn: String,
    /// The alias the create carried: "An optional alias for the prefix list association"
    /// (`smithy_query query="doc" shape="CreateCoreNetworkPrefixListAssociationRequest"
    /// member="PrefixListAlias"`). A required member whose modelled length starts at zero,
    /// so an empty alias is one a caller may ask for and is stored as asked.
    pub prefix_list_alias: String,
    /// The `@idempotencyToken` the create carried, when it carried a non-empty one, so a
    /// retry of the same request answers this association instead of storing a second one.
    pub client_token: Option<String>,
}

/// The key the association store holds a core network / prefix list pair under: the core
/// network id, then the ARN of the prefix list, so the map orders associations by core
/// network and then by prefix list and a pagination token can name an offset into it.
///
/// A core network id is `core-network-<17 hex>`
/// ([`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`], which the modelled
/// `CoreNetworkId` pattern constrains) and carries no `/`, so no two pairs can produce the
/// same key. The same string identifies the association in a not-found refusal, where no
/// single resource id would.
pub fn prefix_list_association_key(core_network_id: &str, prefix_list_arn: &str) -> String {
    format!("{core_network_id}/{prefix_list_arn}")
}

/// How many subnet ARNs one VPC attachment may hold.
///
/// Cloud WAN attaches a VPC through its subnets, one per Availability Zone, and no Region
/// offers more than a handful, so — like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`] — this bound
/// is CHOSEN: the model carries no number and neither the create nor the update of a VPC
/// attachment models a quota error (`smithy_query query="operation"
/// operation="CreateVpcAttachment,UpdateVpcAttachment"`), so its only purpose is that no
/// caller can grow one attachment's subnet list without end. A request that would push the
/// list past it is refused with the modelled `ValidationException` naming the member that
/// asked, and stores nothing.
pub const MAX_SUBNETS_PER_VPC_ATTACHMENT: usize = 50;

/// Refuse a subnet list that would leave a VPC attachment holding more than `limit`
/// subnets.
///
/// `length` is how many the attachment would be left holding, so this is called on the
/// list the attachment WOULD hold (a create's `SubnetArns`, the list an update composes) —
/// never on a removal, which cannot grow it. `limit` is
/// [`MAX_SUBNETS_PER_VPC_ATTACHMENT`], except where an attachment already holds more than
/// that: an update may then still change it, but never grow it (see
/// `UpdateVpcAttachment`), so the limit applied is the one the message reports.
pub fn check_subnet_count(
    member: &str,
    length: usize,
    limit: usize,
) -> Result<(), ValidationException> {
    if length > limit {
        return Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!("a VPC attachment holds at most {limit} subnets, but the request would leave {length}"),
        ));
    }
    Ok(())
}

/// Check a subnet-ARN list a request carries for a VPC attachment.
///
/// The LENGTH is judged FIRST, before any walk of the list: `SubnetArnList` carries no
/// modelled `@length` (`smithy_query shape=SubnetArnList` constrains the member string
/// only), so a caller chooses how long it is, and no per-entry work is done on a list that
/// is already too long to be accepted. `limit` is [`MAX_SUBNETS_PER_VPC_ATTACHMENT`],
/// except where an attachment already holds more than that: an update may then still
/// change it, but never grow it (see `UpdateVpcAttachment`).
///
/// Then every entry must name a subnet the VPC could hold — one in the VPC's own Region,
/// which is the attachment's edge location — and no entry may repeat, since a subnet is
/// attached once or not at all. The modelled `SubnetArn` admits the EMPTY string as well
/// (`^arn:…:subnet\/subnet-[0-9a-f]{8,17}$|^$`, `smithy_query shape=SubnetArn`), which
/// names no subnet and so names no Region; it is refused by the same check. Membership is
/// tested through a set built once, so a list at the bound costs O(n log n).
///
/// What the attachment would be LEFT holding is the separate rule
/// [`check_subnet_count`]: this one judges the list as the request carried it.
///
/// Shared by `CreateVpcAttachment` and `UpdateVpcAttachment`, so ONE function owns the
/// rule and the two cannot judge a subnet list differently; each passes the member of its
/// own request that carried the list.
pub fn check_subnet_list(
    member: &str,
    edge_location: &str,
    requested: &[String],
    limit: usize,
) -> Result<(), ValidationException> {
    if requested.len() > limit {
        return Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!(
                "at most {limit} subnet ARNs may be named, but {} were",
                requested.len()
            ),
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for subnet_arn in requested {
        if arn_region(subnet_arn) != Some(edge_location) {
            return Err(field_rejection(
                member,
                VALIDATION_REASON_OTHER,
                format!(
                    "subnet {subnet_arn} is not in {edge_location}, the Region of the VPC it must belong to"
                ),
            ));
        }
        if !seen.insert(subnet_arn.as_str()) {
            return Err(field_rejection(
                member,
                VALIDATION_REASON_OTHER,
                format!("subnet {subnet_arn} is named more than once"),
            ));
        }
    }
    Ok(())
}

/// How many edge locations one Direct Connect gateway attachment may span.
///
/// CHOSEN, like [`MAX_SUBNETS_PER_VPC_ATTACHMENT`] and for the same reason: the model
/// carries no number — `ExternalRegionCodeList` constrains its member string only
/// (`smithy_query shape=ExternalRegionCodeList`) — and neither the create nor the update
/// of this attachment kind models a quota error (`smithy_query query="operation"
/// operation="CreateDirectConnectGatewayAttachment,UpdateDirectConnectGatewayAttachment"`),
/// so its only purpose is that no caller can grow one attachment's list without end. The
/// number is generous against what a list can usefully name: an attachment spans EDGES of
/// its core network, a core network configures at most one edge per Region, and no entry
/// may repeat ([`check_edge_location_list`]), so a longer list could name no further edge.
pub const MAX_EDGE_LOCATIONS_PER_ATTACHMENT: usize = 50;

/// Check ONE edge location a record is about to be placed at against `topology`, the policy
/// topology of the core network it hangs off (`None` where that core network holds none,
/// which configures no edge at all).
///
/// A record that sits at a core network EDGE sits at an edge the policy IN FORCE configures
/// ([`PolicyTopology::has_edge_location`]), and which edges those are moves with the
/// document in force: a version put in force after a resource was created can configure no
/// edge where an earlier one did. So the location is judged HERE, at the moment of the
/// call, whether the request named it or the handler took it from a resource created
/// earlier — otherwise the stored record would claim an edge the same core network reports
/// none of. `member` is the request member that asked: the one carrying the location, or
/// the reference the handler read it from.
///
/// ONE function owns that rule for every create and update of this service that places a
/// record at a single edge, and [`check_edge_location_list`] below applies it per entry of
/// a list, so no two of them can judge a location differently.
pub fn check_edge_location(
    member: &str,
    core_network_id: &str,
    topology: Option<&PolicyTopology>,
    location: &str,
) -> Result<(), ValidationException> {
    if topology.is_some_and(|topology| topology.has_edge_location(location)) {
        Ok(())
    } else {
        Err(no_such_edge(member, core_network_id, location))
    }
}

/// Check an `EdgeLocations` list a request carries for a Direct Connect gateway attachment
/// against `topology`, the policy topology of the core network the attachment belongs to
/// (`None` where that core network holds none, which configures no edge at all).
///
/// The LENGTH is judged FIRST, before anything walks the list, so no per-entry work — and
/// no retention — is spent on a list already too long to be accepted
/// ([`MAX_EDGE_LOCATIONS_PER_ATTACHMENT`]). Then the list must name at least one location,
/// since an attachment joins its resource to at least one edge; every location it names
/// must be one the policy in force configures an edge for, which is
/// [`check_edge_location`] per entry; and no location may repeat, since an attachment
/// spans an edge once or not at all. Membership is tested through a set built once, so a
/// list at the bound costs O(n log n).
///
/// Shared by `CreateDirectConnectGatewayAttachment` and
/// `UpdateDirectConnectGatewayAttachment` — the create's list and the list an update
/// OVERWRITES it with are the same thing — so ONE function owns the rule and the two
/// cannot judge a list differently. Every refusal names [`FIELD_EDGE_LOCATIONS`], the
/// member both requests carry it in.
pub fn check_edge_location_list(
    core_network_id: &str,
    topology: Option<&PolicyTopology>,
    requested: &[String],
) -> Result<(), ValidationException> {
    if requested.len() > MAX_EDGE_LOCATIONS_PER_ATTACHMENT {
        return Err(field_rejection(
            FIELD_EDGE_LOCATIONS,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "at most {MAX_EDGE_LOCATIONS_PER_ATTACHMENT} edge locations may be named, but {} were",
                requested.len()
            ),
        ));
    }
    if requested.is_empty() {
        return Err(field_rejection(
            FIELD_EDGE_LOCATIONS,
            REASON_FIELD_VALIDATION_FAILED,
            "a Direct Connect gateway attachment must name at least one edge location"
                .to_string(),
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for location in requested {
        check_edge_location(FIELD_EDGE_LOCATIONS, core_network_id, topology, location)?;
        if !seen.insert(location.as_str()) {
            return Err(field_rejection(
                FIELD_EDGE_LOCATIONS,
                VALIDATION_REASON_OTHER,
                format!("edge location {location} is named more than once"),
            ));
        }
    }
    Ok(())
}

/// A telemetry-bearing network resource of one global network: a connection registered
/// with the global network whose session health `GetNetworkTelemetry` reports.///
/// Its `resource_type` is one of [`TELEMETRY_RESOURCE_TYPES`] — nothing else has a session
/// to measure — and its identity is its ARN, which is an ARN of the service that OWNS the
/// connection (Amazon VPC for a transit gateway Connect peer or a VPN connection, this
/// service for a Cloud WAN Connect peer), not one this service mints. So the ARN is stored
/// and `ResourceId` and `AccountId` are DERIVED from it ([`arn_resource_id`],
/// [`arn_account_id`]) rather than kept a second time.
///
/// The Region is stored, because it cannot be derived: a Cloud WAN Connect peer's ARN
/// carries an EMPTY region segment (this service is partition-scoped, see `ARN_REGION`)
/// while its telemetry still reports the edge location the peer lives at.
///
/// Like every other record in this slice, the health this record reports is a function of
/// the clock rather than a field ([`Self::health_at`]), so reading telemetry never writes.
#[derive(Debug, Clone)]
pub struct TelemetryResourceRecord {
    /// The global network the connection is registered with: the one whose
    /// `GetNetworkTelemetry` reports it.
    pub global_network_id: String,
    /// One of [`TELEMETRY_RESOURCE_TYPES`].
    pub resource_type: String,
    /// The ARN of the connection itself, which is also this record's identity.
    pub resource_arn: String,
    /// The Amazon Web Services Region the connection lives in.
    pub aws_region: String,
    /// The core network the connection belongs to, for a Cloud WAN Connect peer; `None`
    /// for a connection reached through a registered gateway instead.
    pub core_network_id: Option<String>,
    /// The ARN of the gateway the connection was registered through (a transit gateway or
    /// a customer gateway), for a connection on the gateway side; `None` for a Cloud WAN
    /// Connect peer.
    pub registered_gateway_arn: Option<String>,
    /// The address of the connection's far end, when the registration recorded one.
    pub address: Option<String>,
    /// The instant the connection was registered with the global network: the instant the
    /// `DOWN` health this record starts out with took effect.
    pub registered_at: SystemTime,
    /// The instant the session the registration started comes up — the transition the
    /// clock owes, [`SETTLE_DELAY`] after the registering call — and so the instant the
    /// `UP` health takes effect.
    pub connected_at: SystemTime,
}

impl TelemetryResourceRecord {
    /// The protocol whose session this resource's health belongs to: IPsec for a VPN
    /// connection, BGP for a Connect peer of either kind. See [`CONNECTION_TYPE_BGP`].
    pub fn connection_type(&self) -> &'static str {
        match self.resource_type.as_str() {
            VPN_CONNECTION_TYPE => CONNECTION_TYPE_IPSEC,
            // `connect-peer` and `transit-gateway-connect-peer`: both peer over BGP.
            _ => CONNECTION_TYPE_BGP,
        }
    }

    /// Whether the session is up as of `now`: the registration starts it, and it comes up
    /// at [`Self::connected_at`] — the transition the clock owes, never a stored flag, so
    /// a caller that reads straight after the registration still sees it `DOWN`.
    pub fn is_up_at(&self, now: SystemTime) -> bool {
        now >= self.connected_at
    }

    /// The modelled `ConnectionHealth` for this record as of `now`.
    ///
    /// `Timestamp` is "The time the status was last updated"
    /// (`smithy_query query="doc" shape="ConnectionHealth" member="Timestamp"`), which is
    /// the instant the status being reported took effect: the registration while the
    /// session is still down, the instant it came up afterwards. Both are stored instants,
    /// so repeated reads of one record report the identical health.
    pub fn health_at(&self, now: SystemTime) -> ConnectionHealth {
        let up = self.is_up_at(now);
        let status = if up {
            CONNECTION_STATUS_UP
        } else {
            CONNECTION_STATUS_DOWN
        };
        let since = if up { self.connected_at } else { self.registered_at };
        ConnectionHealth {
            r#type: Some(self.connection_type().to_string()),
            status: Some(status.to_string()),
            timestamp: Some(epoch_seconds(since)),
        }
    }

    /// The modelled `NetworkTelemetry` for this record as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> NetworkTelemetry {
        NetworkTelemetry {
            registered_gateway_arn: self.registered_gateway_arn.clone(),
            core_network_id: self.core_network_id.clone(),
            aws_region: Some(self.aws_region.clone()),
            account_id: arn_account_id(&self.resource_arn).map(str::to_string),
            resource_type: Some(self.resource_type.clone()),
            resource_id: Some(arn_resource_id(&self.resource_arn).to_string()),
            resource_arn: Some(self.resource_arn.clone()),
            address: self.address.clone(),
            health: Some(self.health_at(now)),
        }
    }
}

/// A stored peering: what joins a core network, at ONE of its edge locations, to a
/// resource of another service — a transit gateway, the one kind of peering the model
/// carries ([`PEERING_TYPE_TRANSIT_GATEWAY`]).
///
/// A peering is a resource of its own, with an id, an ARN and tags, and it is what a
/// transit gateway route table attachment is created OVER
/// ([`AttachmentDetail::TransitGatewayRouteTable`]): the attachment takes its core network
/// and its edge location from the peering rather than from its own request, so the two can
/// never disagree about which core network the route table is reaching.
///
/// Like every record here, `state` is not a field but a function of what is stored and the
/// clock ([`Self::state_at`]), so reporting a peering never writes.
#[derive(Debug, Clone)]
pub struct PeeringRecord {
    pub id: String,
    pub arn: String,
    pub core_network_id: String,
    /// The ARN of [`Self::core_network_id`], kept so a read never has to resolve the
    /// parent to report `Peering$CoreNetworkArn`.
    pub core_network_arn: String,
    /// One of `ops::types::PEERING_TYPE_VALUES`, which carries the single value
    /// [`PEERING_TYPE_TRANSIT_GATEWAY`].
    pub peering_type: &'static str,
    /// The edge location the peering sits at: one of the locations the core network's
    /// policy configures ([`PolicyTopology::has_edge_location`]), which is the Region the
    /// peered resource is in.
    pub edge_location: String,
    /// The ARN of the resource peered with — the transit gateway for a transit gateway
    /// peering, which is also the `TransitGatewayArn` that shape reports.
    pub resource_arn: String,
    /// The id of the peering attachment this peering leaves on the peered transit gateway
    /// ([`TRANSIT_GATEWAY_PEERING_ATTACHMENT_KIND`]): a resource of the peered gateway, so
    /// it is minted here rather than taken from any request member.
    pub transit_gateway_peering_attachment_id: String,
    pub created_at: SystemTime,
    /// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second peering.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
}

impl PeeringRecord {
    /// This peering's lifecycle state as of `now`.
    ///
    /// `PeeringState` carries CREATING, FAILED, AVAILABLE and DELETING (`smithy_query
    /// shape=PeeringState`, mirrored in `ops::types::PEERING_STATE_VALUES`), two of which
    /// are the ones the clock decides: a peering under way waits in [`STATE_CREATING`]
    /// until the transition the clock owes falls due and is [`STATE_AVAILABLE`] after it.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_CREATING
        }
    }

    /// The modelled `Peering` envelope of this record, as of `now`.
    pub fn to_peering(&self, now: SystemTime) -> Peering {
        self.to_peering_with_state(self.state_at(now))
    }

    /// The modelled `Peering` envelope of this record in `state`: what a DELETE reports for
    /// the record it has just removed, which is no longer stored and so has no state the
    /// clock could compute. The same envelope every other read of a peering answers, so a
    /// delete reports the peering it removed in full rather than a shell carrying only an id.
    pub fn to_peering_with_state(&self, state: &str) -> Peering {
        Peering {
            core_network_id: Some(self.core_network_id.clone()),
            core_network_arn: Some(self.core_network_arn.clone()),
            peering_id: Some(self.id.clone()),
            owner_account_id: Some(::engine::emulator_account_id().to_string()),
            peering_type: Some(self.peering_type.to_string()),
            state: Some(state.to_string()),
            edge_location: Some(self.edge_location.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            tags: Some(self.tags.clone()),
            created_at: Some(epoch_seconds(self.created_at)),
            // No modification of this peering has failed: the list is empty rather than
            // carrying an error nothing produced.
            last_modification_errors: Some(Vec::new()),
        }
    }

    /// The modelled `TransitGatewayPeering` for this record as of `now`: the envelope
    /// above plus the two members only a transit gateway peering carries (`smithy_query
    /// shape=TransitGatewayPeering`).
    ///
    /// `TransitGatewayArn` is [`Self::resource_arn`]: the resource a transit gateway
    /// peering peers with IS the transit gateway, so it is not stored twice.
    pub fn to_transit_gateway_peering(&self, now: SystemTime) -> TransitGatewayPeering {
        TransitGatewayPeering {
            peering: Some(self.to_peering(now)),
            transit_gateway_arn: Some(self.resource_arn.clone()),
            transit_gateway_peering_attachment_id: Some(
                self.transit_gateway_peering_attachment_id.clone(),
            ),
        }
    }
}

/// A stored attachment: what joins one resource to ONE edge location of a core network.
///
/// Every attachment kind shares the modelled `Attachment` envelope (`smithy_query
/// shape=Attachment`) — the core network it belongs to, its type, its state, its edge
/// location, the ARN of the resource it attaches and its tags — and each kind adds its own
/// members on top, which is what [`AttachmentDetail`] holds. One record and one map
/// therefore serve every kind: `GetConnectAttachment`, `GetVpcAttachment` and
/// `GetSiteToSiteVpnAttachment` read the same store the quota, the inventory count and the
/// core-network delete refusal are computed from, so no two operations can disagree about
/// which attachments exist.
///
/// Like the records above, `state` is not a field but a function of what is stored and the
/// clock ([`Self::state_at`]), so reporting an attachment never writes. The one thing the
/// clock does NOT decide is the acceptance an `attachment-policies` rule can ask for: an
/// attachment waiting for one stays in [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`] until
/// `AcceptAttachment` grants it ([`Self::accept_at`]) or `RejectAttachment` turns it down
/// ([`Self::reject_at`]), which leaves it [`STATE_REJECTED`] for good.
#[derive(Debug, Clone)]
pub struct AttachmentRecord {
    pub id: String,
    pub arn: String,
    pub core_network_id: String,
    /// The ARN of [`Self::core_network_id`], kept so a read never has to resolve the
    /// parent to report `Attachment$CoreNetworkArn`.
    pub core_network_arn: String,
    /// One of `ops::types::ATTACHMENT_TYPE_VALUES` — [`ATTACHMENT_TYPE_VPC`],
    /// [`ATTACHMENT_TYPE_CONNECT`] or [`ATTACHMENT_TYPE_SITE_TO_SITE_VPN`] for the three
    /// kinds this slice mints.
    pub attachment_type: &'static str,
    /// The edge location the attachment sits at: one of the locations the core network's
    /// policy configures ([`PolicyTopology::has_edge_location`]). For a kind that spans
    /// SEVERAL of them this is the primary of those locations and is not reported on its
    /// own (see [`AttachmentDetail::DirectConnectGateway`]).
    pub edge_location: String,
    /// The ARN of the resource this attachment attaches: the VPC for a VPC attachment, the
    /// VPN connection for a site-to-site VPN attachment, and for a Connect attachment the
    /// transport attachment it is carried over, which is the resource it is built on.
    pub resource_arn: String,
    /// The `RoutingPolicyLabel` the create carried, stored because the request supplied
    /// it. The modelled `Attachment` has no member for it (`smithy_query shape=Attachment`)
    /// — it is reported by `ListAttachmentRoutingPolicyAssociations`
    /// ([`Self::to_routing_policy_association`]) — so it is held here rather than dropped.
    pub routing_policy_label: Option<String>,
    /// The normalised idempotency token of the `PutAttachmentRoutingPolicyLabel` call that
    /// applied the label above, kept so a retry of that call answers it again instead of
    /// writing a second time and so the same token asking for another label is refused
    /// ([`replay_or_conflict_by`]). `None` for a label that came from the create, for one
    /// applied by a call carrying no token, and for an attachment carrying no label — none
    /// of which a token names.
    pub routing_policy_label_client_token: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due, or
    /// `None` while the attachment is still waiting for the acceptance the core network
    /// policy asked for: nothing is being created until the request is accepted, so no
    /// transition is owed on it until then (see [`Self::accept_at`]).
    pub available_at: Option<SystemTime>,
    /// When the acceptance the policy asked for was granted — the instant the attachment
    /// last changed before the transition above falls due. `None` for an attachment no rule
    /// asked acceptance for, which never waited for one.
    pub accepted_at: Option<SystemTime>,
    /// When the attachment request was TURNED DOWN, or `None` for one nothing rejected.
    /// Terminal: an attachment holding an instant here is [`STATE_REJECTED`] from that
    /// instant on, is never created, and has no transition owed on it
    /// ([`Self::reject_at`]).
    pub rejected_at: Option<SystemTime>,
    /// The instant a CHANGE to this attachment was last requested — what
    /// `UpdateVpcAttachment` records — or `None` for one nothing has changed since it was
    /// created. What [`Self::state_at`] tells a creation in flight from a change in flight
    /// by, and what [`Self::updated_at`] reports until the transition falls due.
    pub changed_at: Option<SystemTime>,
    /// The `attachment-policies` rule of the core network policy in force that claimed this
    /// attachment, and what that rule put the attachment in: the
    /// `Attachment$AttachmentPolicyRuleNumber`, `Attachment$SegmentName` and
    /// `Attachment$NetworkFunctionGroupName` a read of it reports. All three are absent
    /// where no rule of that policy claims the attachment.
    pub attachment_policy_rule_number: Option<i32>,
    /// See [`Self::attachment_policy_rule_number`].
    pub segment_name: Option<String>,
    /// See [`Self::attachment_policy_rule_number`].
    pub network_function_group_name: Option<String>,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second attachment.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
    /// The members only this attachment's own kind has.
    pub detail: AttachmentDetail,
}

/// What one attachment kind adds to the shared [`AttachmentRecord`] envelope: the members
/// its own wrapper shape carries beside `Attachment` (`smithy_query
/// shape=VpcAttachment,ConnectAttachment,SiteToSiteVpnAttachment,TransitGatewayRouteTableAttachment,DirectConnectGatewayAttachment`),
/// plus the one envelope member only a single kind fills
/// ([`Self::DirectConnectGateway`]).
#[derive(Debug, Clone)]
pub enum AttachmentDetail {
    /// A VPC attachment: the subnets it reaches the VPC through, in the order the request
    /// listed them, and the VPC options it was created with.
    Vpc {
        subnet_arns: Vec<String>,
        options: Option<VpcOptions>,
    },
    /// A core network Connect attachment: the attachment carrying it and its tunnel
    /// options.
    Connect {
        transport_attachment_id: String,
        options: ConnectAttachmentOptions,
    },
    /// A site-to-site VPN attachment. It carries NO member of its own: beside
    /// `Attachment`, `SiteToSiteVpnAttachment` holds only `VpnConnectionArn`
    /// (`smithy_query shape=SiteToSiteVpnAttachment`), and that is the ARN of the resource
    /// this attachment attaches — [`AttachmentRecord::resource_arn`] — so storing it here
    /// would be a second copy of one value.
    SiteToSiteVpn,
    /// A Direct Connect gateway attachment: the edge locations it spans, in the order the
    /// request listed them.
    ///
    /// `DirectConnectGatewayArn` is [`AttachmentRecord::resource_arn`], as
    /// `VpnConnectionArn` is for the kind above, so it is not stored twice. What IS stored
    /// here is the one envelope member no other kind fills: this is the only kind that
    /// sits at SEVERAL edges of its core network, which the envelope reports as
    /// `Attachment$EdgeLocations` instead of `Attachment$EdgeLocation` ("The edge
    /// locations that the Direct Connect gateway is associated with. This is returned only
    /// for Direct Connect gateway attachments. All other attachment types retrun
    /// EdgeLocation", `smithy_query query="doc" shape="Attachment"
    /// member="EdgeLocations"`). [`AttachmentRecord::edge_location`] carries the first of
    /// them — the attachment's PRIMARY edge, the one an `attachment-policies` `region`
    /// condition is evaluated against ([`primary_edge_location`]) — and is never reported
    /// for this kind.
    DirectConnectGateway { edge_locations: Vec<String> },
    /// A transit gateway route table attachment: the transit gateway PEERING it was
    /// created over ([`PeeringRecord`]).
    ///
    /// `TransitGatewayRouteTableArn` is [`AttachmentRecord::resource_arn`], as
    /// `VpnConnectionArn` is for the site-to-site VPN kind, so it is not stored twice.
    /// What IS stored here is the peering, because this is the one kind whose edge
    /// location and core network come from a peering rather than from the attached
    /// resource: the route table is reached THROUGH that peering.
    TransitGatewayRouteTable { peering_id: String },
}

/// The PRIMARY edge of an attachment spanning `edge_locations`: the first of them, or
/// `None` where the list is empty — which no stored attachment has, since an attachment
/// joins a resource to at least one edge of its core network.
///
/// One place decides it, so the edge a multi-edge attachment carries as
/// [`AttachmentRecord::edge_location`] cannot drift between the create and a later update
/// of its locations.
pub fn primary_edge_location(edge_locations: &[String]) -> Option<&str> {
    edge_locations.first().map(String::as_str)
}

impl AttachmentRecord {
    /// This attachment's lifecycle state as of `now`.
    ///
    /// `AttachmentState` is a wider enum than the resource states above (REJECTED,
    /// PENDING_ATTACHMENT_ACCEPTANCE, CREATING, FAILED, AVAILABLE, UPDATING,
    /// PENDING_NETWORK_UPDATE, PENDING_TAG_ACCEPTANCE, DELETING: `smithy_query
    /// shape=AttachmentState`, mirrored in `ops::types::ATTACHMENT_STATE_VALUES`), and it
    /// shares two of them with [`CoreNetworkRecord::state_at`]: an attachment under way
    /// waits in [`STATE_CREATING`] until the transition the clock owes falls due and is
    /// [`STATE_AVAILABLE`] after it. An attachment the policy in force asked acceptance for
    /// waits in [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`] INSTEAD, and the clock never moves
    /// it: only [`Self::accept_at`] does, after which the span above starts. A CHANGE to an
    /// attachment that is already there owes the same transition from
    /// [`ATTACHMENT_STATE_UPDATING`], which is why the state before the due instant depends
    /// on whether anything has changed the attachment since it was created
    /// ([`Self::changed_at`]).
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.rejected_at.is_some() {
            return STATE_REJECTED;
        }
        match self.available_at {
            None => STATE_PENDING_ATTACHMENT_ACCEPTANCE,
            Some(available_at) if now >= available_at => STATE_AVAILABLE,
            Some(_) if self.changed_at.is_some() => ATTACHMENT_STATE_UPDATING,
            Some(_) => STATE_CREATING,
        }
    }

    /// Whether this attachment is still waiting for the acceptance its core network policy
    /// asked for — the one thing `AcceptAttachment` and `RejectAttachment` can act on. An
    /// attachment already rejected waits for nothing: that decision was taken.
    pub fn awaits_acceptance(&self) -> bool {
        self.available_at.is_none() && self.rejected_at.is_none()
    }

    /// Whether anything was ever CREATED for this attachment — whether it is the attachment
    /// it describes rather than a request for one.
    ///
    /// An attachment still waiting for the acceptance its core network policy asked for has
    /// nothing created yet ([`Self::available_at`]), and one whose request was turned down
    /// never will have ([`Self::rejected_at`]); every other stored attachment is either
    /// coming into existence on the transition the clock owes it or already there, so it is
    /// something another resource can be built on ([`check_attachment_is_created`]).
    pub fn is_created(&self) -> bool {
        self.available_at.is_some() && self.rejected_at.is_none()
    }

    /// Grant the acceptance this attachment is waiting for, at `now`: the attachment starts
    /// being created, so the `CREATING` -> `AVAILABLE` transition the clock owes becomes due
    /// a [`SETTLE_DELAY`] later. Answers `false`, writing nothing, when the attachment was
    /// not waiting for one.
    pub fn accept_at(&mut self, now: SystemTime) -> bool {
        if !self.awaits_acceptance() {
            return false;
        }
        self.accepted_at = Some(now);
        self.available_at = Some(now + SETTLE_DELAY);
        true
    }

    /// Turn down the attachment request this attachment is waiting for, at `now`: it is
    /// never created, so no transition is owed on it and it is [`STATE_REJECTED`] from here
    /// on. Answers `false`, writing nothing, when the attachment was not waiting for an
    /// acceptance — the mirror of [`Self::accept_at`], so the two decisions are taken on
    /// exactly the same attachments and neither can overtake the other.
    pub fn reject_at(&mut self, now: SystemTime) -> bool {
        if !self.awaits_acceptance() {
            return false;
        }
        self.rejected_at = Some(now);
        true
    }

    /// When this attachment last changed, as of `now`: the create, the acceptance that
    /// started it, the rejection that turned its request down, the change an update
    /// requested, or the transition to `AVAILABLE` once that has fallen due. DERIVED from
    /// the stored instants, so a read reports a current `UpdatedAt` without writing one.
    pub fn updated_at(&self, now: SystemTime) -> SystemTime {
        match self.available_at {
            Some(available_at) if now >= available_at => available_at,
            _ => self
                .rejected_at
                .or(self.changed_at)
                .or(self.accepted_at)
                .unwrap_or(self.created_at),
        }
    }

    /// Overwrite the edge locations a Direct Connect gateway attachment spans, at `now`,
    /// keeping the first of them as the attachment's primary edge
    /// ([`primary_edge_location`]).
    ///
    /// The attachment has CHANGED, so — exactly as an accepted `UpdateVpcAttachment`
    /// records it — it is [`ATTACHMENT_STATE_UPDATING`] until the transition the clock owes
    /// falls due a [`SETTLE_DELAY`] later and [`STATE_AVAILABLE`] after it. Answers
    /// `false`, writing NOTHING, for an empty list, for a record of a kind that sits at one
    /// edge rather than spanning several, and for a list the record already holds — a
    /// change that asks for what is stored is no change, so no transition is owed on it.
    pub fn set_edge_locations(&mut self, requested: Vec<String>, now: SystemTime) -> bool {
        let Some(primary) = primary_edge_location(&requested).map(str::to_string) else {
            return false;
        };
        match &mut self.detail {
            AttachmentDetail::DirectConnectGateway { edge_locations } => {
                if *edge_locations == requested {
                    return false;
                }
                *edge_locations = requested;
                self.edge_location = primary;
                self.changed_at = Some(now);
                self.available_at = Some(now + SETTLE_DELAY);
                true
            }
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => false,
        }
    }

    /// The ONE edge location this attachment sits at, or `None` for a kind that spans
    /// several of them instead.
    ///
    /// ONE place decides it, so every answer that names the single Region an attachment
    /// lives in — the `Attachment$EdgeLocation` of its own reader
    /// ([`Self::to_attachment_at`]) and the `NetworkResource$AwsRegion` of the global
    /// network's inventory ([`ComAmazonawsNetworkmanagerNetworkmanagerState::network_resources_of`])
    /// — reports the same one. A Direct Connect gateway attachment is the kind that spans
    /// several edges and reports them as `Attachment$EdgeLocations` instead ("This is
    /// returned only for Direct Connect gateway attachments. All other attachment types
    /// retrun EdgeLocation", `smithy_query query="doc" shape="Attachment"
    /// member="EdgeLocations"`), so it has no single edge to name.
    pub fn sole_edge_location(&self) -> Option<&str> {
        match &self.detail {
            AttachmentDetail::DirectConnectGateway { .. } => None,
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => {
                Some(self.edge_location.as_str())
            }
        }
    }

    /// Whether this attachment sits at the edge location `location`.
    ///
    /// Every kind but one sits at ONE edge ([`Self::sole_edge_location`]), so that single
    /// edge decides it; a Direct Connect gateway attachment spans SEVERAL and sits at every
    /// one of them, so any of them decides it. That is the same split
    /// [`Self::to_attachment_at`] reports as `EdgeLocation` / `EdgeLocations`, so a listing
    /// selected on a location answers the attachments whose reported edges hold it and no
    /// other.
    pub fn sits_at(&self, location: &str) -> bool {
        match &self.detail {
            AttachmentDetail::DirectConnectGateway { edge_locations } => {
                edge_locations.iter().any(|edge| edge == location)
            }
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => {
                self.sole_edge_location() == Some(location)
            }
        }
    }

    /// The telemetry resource type of the connection this attachment attaches — one of
    /// [`TELEMETRY_RESOURCE_TYPES`] — or `None` when what it attaches is not a connection
    /// whose session this service measures.
    ///
    /// A site-to-site VPN attachment attaches a VPN CONNECTION ([`VPN_CONNECTION_TYPE`],
    /// reported with the IPsec session health [`CONNECTION_TYPE_IPSEC`]), so tying one to a
    /// core network is what makes that connection's session known to the global network
    /// owning the core network ([`ComAmazonawsNetworkmanagerNetworkmanagerState::register_attachment_telemetry`]).
    /// What the other kinds attach is a VPC, a Direct Connect gateway, a transit gateway
    /// route table or another attachment: resources of the inventory rather than sessions,
    /// none of them a type `GetNetworkTelemetry` reports on. A Connect attachment carries
    /// BGP sessions, but each is a CONNECT PEER of its own, registered by the associate that
    /// ties that peer to the global network rather than by the attachment carrying it.
    pub fn telemetry_resource_type(&self) -> Option<&'static str> {
        match &self.detail {
            AttachmentDetail::SiteToSiteVpn => Some(VPN_CONNECTION_TYPE),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `Attachment` envelope of this record, as of `now`.
    ///
    /// `EdgeLocations` is absent because "this is returned only for Direct Connect gateway
    /// attachments. All other attachment types retrun EdgeLocation" (`smithy_query
    /// query="doc" shape="Attachment" member="EdgeLocations"`).
    pub fn to_attachment(&self, now: SystemTime) -> Attachment {
        self.to_attachment_at(self.state_at(now), self.updated_at(now))
    }

    /// The modelled `Attachment` envelope of the record a delete at `now` just removed:
    /// [`STATE_DELETING`], which the clock no longer decides for it, and `now` as the
    /// instant it last changed — the delete IS that change.
    ///
    /// The sibling `to_shape_with_state` of the records above forces only the state because
    /// their shapes carry no `UpdatedAt` member at all (`GlobalNetwork`, `Site`, `Link`,
    /// `Device`, `Connection`, `CoreNetwork` each carry `CreatedAt` only); `Attachment` is
    /// the one shape of this service that reports when the resource last changed
    /// (`smithy_query shape=Attachment`), so it is the one delete that has an instant to
    /// report.
    pub fn to_deleted_attachment(&self, now: SystemTime) -> Attachment {
        self.to_attachment_at(STATE_DELETING, now)
    }

    /// The shared body of the two above: the envelope with `state` and `updated_at`
    /// supplied, so neither spelling of a read can disagree with the other about the rest.
    fn to_attachment_at(&self, state: &str, updated_at: SystemTime) -> Attachment {
        Attachment {
            core_network_id: Some(self.core_network_id.clone()),
            core_network_arn: Some(self.core_network_arn.clone()),
            attachment_id: Some(self.id.clone()),
            owner_account_id: Some(::engine::emulator_account_id().to_string()),
            attachment_type: Some(self.attachment_type.to_string()),
            state: Some(state.to_string()),
            // Exactly ONE of the two edge members is reported, and which one is a function
            // of the kind ([`Self::sole_edge_location`]). Neither is a copy of the other:
            // the absent member stays absent.
            edge_location: self.sole_edge_location().map(str::to_string),
            edge_locations: match &self.detail {
                AttachmentDetail::DirectConnectGateway { edge_locations } => {
                    Some(edge_locations.clone())
                }
                AttachmentDetail::Vpc { .. }
                | AttachmentDetail::Connect { .. }
                | AttachmentDetail::SiteToSiteVpn
                | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
            },
            resource_arn: Some(self.resource_arn.clone()),
            // Which `attachment-policies` rule of the core network policy claimed this
            // attachment, and the segment or network function group that rule put it in,
            // were decided where the attachment was created, from the policy in force on
            // its core network, and are reported from the record rather than re-derived.
            attachment_policy_rule_number: self.attachment_policy_rule_number,
            segment_name: self.segment_name.clone(),
            network_function_group_name: self.network_function_group_name.clone(),
            tags: Some(self.tags.clone()),
            // A proposed change exists only while a tag or segment change is awaiting
            // acceptance, which nothing here can start.
            proposed_segment_change: None,
            proposed_network_function_group_change: None,
            created_at: Some(epoch_seconds(self.created_at)),
            updated_at: Some(epoch_seconds(updated_at)),
            // No modification of this attachment has failed: the list is empty rather than
            // carrying an error nothing produced.
            last_modification_errors: Some(Vec::new()),
        }
    }

    /// The modelled `ConnectAttachment` for this record as of `now`, or `None` when the
    /// record is not a Connect attachment — which is how `GetConnectAttachment` answers
    /// the id of an attachment of another kind as not found.
    pub fn to_connect_attachment(&self, now: SystemTime) -> Option<ConnectAttachment> {
        match &self.detail {
            AttachmentDetail::Connect {
                transport_attachment_id,
                options,
            } => Some(ConnectAttachment {
                attachment: Some(self.to_attachment(now)),
                transport_attachment_id: Some(transport_attachment_id.clone()),
                options: Some(options.clone()),
            }),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `VpcAttachment` for this record as of `now`, or `None` when the record
    /// is not a VPC attachment. See [`Self::to_connect_attachment`].
    pub fn to_vpc_attachment(&self, now: SystemTime) -> Option<VpcAttachment> {
        match &self.detail {
            AttachmentDetail::Vpc {
                subnet_arns,
                options,
            } => Some(VpcAttachment {
                attachment: Some(self.to_attachment(now)),
                subnet_arns: Some(subnet_arns.clone()),
                options: options.clone(),
            }),
            AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `SiteToSiteVpnAttachment` for this record as of `now`, or `None` when
    /// the record is not a site-to-site VPN attachment. See
    /// [`Self::to_connect_attachment`].
    ///
    /// `VpnConnectionArn` is [`Self::resource_arn`]: the resource a site-to-site VPN
    /// attachment attaches IS the VPN connection.
    pub fn to_site_to_site_vpn_attachment(
        &self,
        now: SystemTime,
    ) -> Option<SiteToSiteVpnAttachment> {
        match &self.detail {
            AttachmentDetail::SiteToSiteVpn => Some(SiteToSiteVpnAttachment {
                attachment: Some(self.to_attachment(now)),
                vpn_connection_arn: Some(self.resource_arn.clone()),
            }),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `DirectConnectGatewayAttachment` for this record as of `now`, or `None`
    /// when the record is not a Direct Connect gateway attachment. See
    /// [`Self::to_connect_attachment`].
    ///
    /// `DirectConnectGatewayArn` is [`Self::resource_arn`]: the resource this attachment
    /// attaches IS the Direct Connect gateway.
    pub fn to_direct_connect_gateway_attachment(
        &self,
        now: SystemTime,
    ) -> Option<DirectConnectGatewayAttachment> {
        match &self.detail {
            AttachmentDetail::DirectConnectGateway { .. } => Some(DirectConnectGatewayAttachment {
                attachment: Some(self.to_attachment(now)),
                direct_connect_gateway_arn: Some(self.resource_arn.clone()),
            }),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `TransitGatewayRouteTableAttachment` for this record as of `now`, or
    /// `None` when the record is not a transit gateway route table attachment. See
    /// [`Self::to_connect_attachment`].
    ///
    /// `TransitGatewayRouteTableArn` is [`Self::resource_arn`]: the resource this
    /// attachment attaches IS the transit gateway route table.
    pub fn to_transit_gateway_route_table_attachment(
        &self,
        now: SystemTime,
    ) -> Option<TransitGatewayRouteTableAttachment> {
        match &self.detail {
            AttachmentDetail::TransitGatewayRouteTable { peering_id } => {
                Some(TransitGatewayRouteTableAttachment {
                    attachment: Some(self.to_attachment(now)),
                    peering_id: Some(peering_id.clone()),
                    transit_gateway_route_table_arn: Some(self.resource_arn.clone()),
                })
            }
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::DirectConnectGateway { .. } => None,
        }
    }

    /// The modelled `AttachmentRoutingPolicyAssociationSummary` of this record, or `None`
    /// when the attachment carries no routing policy label: the label is what ties an
    /// attachment to the routing policies of its core network, so an attachment without one
    /// has no routing policy association to report.
    pub fn to_routing_policy_association(&self) -> Option<AttachmentRoutingPolicyAssociationSummary> {
        let label = self.routing_policy_label.clone()?;
        Some(AttachmentRoutingPolicyAssociationSummary {
            attachment_id: Some(self.id.clone()),
            routing_policy_label: Some(label),
            // The routing policies an attachment's label matches are declared by the core
            // network policy document, which this slice parses for its edges, segments,
            // network function groups and attachment policies ([`policy_topology`]) and for
            // nothing else: no operation of this service creates, names or changes a routing
            // policy, so there is no stored policy for a label to match. Both members are
            // therefore absent rather than carrying names nothing declared.
            pending_routing_policies: None,
            associated_routing_policies: None,
        })
    }
}

/// Check the attachment a request wants to build a resource ON — the transport a Connect
/// attachment is carried over, the Connect attachment a Connect peer runs inside — against
/// what it is: a resource is built on an attachment that EXISTS, and an attachment whose
/// request nothing has granted yet, or that was turned down, is not one
/// ([`AttachmentRecord::is_created`]).
///
/// The refusal is the modelled `ValidationException` (HTTP 400) naming the member that
/// carried the reference, exactly as the other refusals of that reference do: the request
/// names an attachment that cannot carry anything, which is a fault of that member's value
/// against stored state ([`VALIDATION_REASON_OTHER`]), and the state the attachment is in
/// is reported so the caller knows whether an acceptance is owed on it or the decision was
/// already taken against it.
///
/// ONE function owns the rule, so no two creates can disagree about which attachments can
/// be built on. `member` is the request member carrying the reference and `built` names the
/// resource the request asked for, as the message reads it.
pub fn check_attachment_is_created(
    member: &str,
    attachment: &AttachmentRecord,
    built: &str,
    now: SystemTime,
) -> Result<(), ValidationException> {
    if attachment.is_created() {
        return Ok(());
    }
    Err(field_rejection(
        member,
        VALIDATION_REASON_OTHER,
        format!(
            "attachment {} is {}; {built} is built on an attachment that exists",
            attachment.id,
            attachment.state_at(now)
        ),
    ))
}

/// A stored core network Connect peer: the BGP peering one CONNECT attachment carries to
/// an appliance outside AWS.
///
/// A peer belongs to ONE Connect attachment, and therefore to that attachment's core
/// network and to the single edge location the attachment sits at: those three are read
/// from the attachment where the peer is created and kept here, so a peer reports the edge
/// its tunnel really lands on rather than one a request named. The tunnel PROTOCOL is the
/// attachment's too — a Connect attachment "is a GRE-based tunnel attachment"
/// (`smithy_query query="doc" shape="CreateConnectAttachment"`) unless its options ask for
/// `NO_ENCAP` — which is why `CreateConnectPeer` takes no protocol of its own.
///
/// Like the records above, `state` is not a field but a function of what is stored and the
/// clock ([`Self::state_at`]), so reporting a peer never writes.
#[derive(Debug, Clone)]
pub struct ConnectPeerRecord {
    pub id: String,
    pub arn: String,
    /// The Connect attachment this peer is carried over.
    pub connect_attachment_id: String,
    /// The core network and edge location of that attachment. Kept here so a read never
    /// has to resolve the attachment to report `ConnectPeer$CoreNetworkId` and
    /// `ConnectPeer$EdgeLocation`.
    pub core_network_id: String,
    /// See [`Self::core_network_id`].
    pub edge_location: String,
    /// The tunnel protocol of that attachment — one of `ops::types::TUNNEL_PROTOCOL_VALUES`
    /// — or `None` where its options named none, which is what the peer's configuration
    /// reports as `ConnectPeerConfiguration$Protocol`.
    pub protocol: Option<String>,
    /// The addresses the tunnel's two ends carry: the core network side, which only a GRE
    /// peering needs ("This only applies only when the protocol is GRE", `smithy_query
    /// query="doc" shape="CreateConnectPeerRequest" member="CoreNetworkAddress"`), and the
    /// appliance side, which every peering requires.
    ///
    /// A peering whose [`Self::protocol`] is another one carries NONE: the create that
    /// named it against that protocol is refused rather than stored
    /// ([`check_protocol_scoped_member`]), so this is held only where it applies.
    pub core_network_address: Option<String>,
    /// See [`Self::core_network_address`].
    pub peer_address: String,
    /// The inside IP addresses the two ends run BGP over, as the request listed them,
    /// bounded by [`MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER`].
    pub inside_cidr_blocks: Vec<String>,
    /// The two ends' AS numbers: the core network's is the ASN its policy assigns the edge
    /// this peer lands on ([`PolicyTopology::edge_asn`]), and the appliance's is the one the
    /// request's `BgpOptions` named. Either is absent where the policy assigns the edge none
    /// or the request named none.
    pub core_network_asn: Option<i64>,
    /// See [`Self::core_network_asn`].
    pub peer_asn: Option<i64>,
    /// The subnet the peer reaches its appliance through, which only a `NO_ENCAP` peering
    /// needs (`smithy_query query="doc" shape="CreateConnectPeerRequest"
    /// member="SubnetArn"`), or `None` where the request named none — the modelled pattern
    /// admits the empty string, which names no subnet and is held as none.
    ///
    /// Like [`Self::core_network_address`], a peering whose [`Self::protocol`] is another
    /// one carries none ([`check_protocol_scoped_member`]).
    pub subnet_arn: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second peer.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
}

impl ConnectPeerRecord {
    /// This peer's lifecycle state as of `now`.
    ///
    /// `ConnectPeerState` enumerates CREATING, FAILED, AVAILABLE and DELETING
    /// (`smithy_query shape=ConnectPeerState`, mirrored in
    /// `ops::types::CONNECT_PEER_STATE_VALUES`), and shares the two the clock decides with
    /// [`CoreNetworkRecord::state_at`]: a peer being created waits in [`STATE_CREATING`]
    /// until the transition the clock owes falls due and is [`STATE_AVAILABLE`] after it.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_CREATING
        }
    }

    /// The modelled `ConnectPeerConfiguration` of this peer: the two tunnel addresses, the
    /// inside CIDR blocks, the protocol it inherits from its attachment, and the BGP
    /// session that configuration describes.
    ///
    /// ONE BGP configuration, because one peer joins one core network edge to one
    /// appliance: its two AS numbers and its two addresses are exactly the stored ends of
    /// this peering, so nothing here is invented.
    pub fn configuration(&self) -> ConnectPeerConfiguration {
        ConnectPeerConfiguration {
            core_network_address: self.core_network_address.clone(),
            peer_address: Some(self.peer_address.clone()),
            inside_cidr_blocks: Some(self.inside_cidr_blocks.clone()),
            protocol: self.protocol.clone(),
            bgp_configurations: Some(vec![ConnectPeerBgpConfiguration {
                core_network_asn: self.core_network_asn,
                peer_asn: self.peer_asn,
                core_network_address: self.core_network_address.clone(),
                peer_address: Some(self.peer_address.clone()),
            }]),
        }
    }

    /// The modelled `ConnectPeer` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> ConnectPeer {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `ConnectPeer` for this record in `state`: what a DELETE reports for the
    /// record it has just removed, which is no longer stored and so has no state the clock
    /// could compute. The same shape every other read of a peer answers, so a delete
    /// reports the configuration the peer was stored with rather than an empty shell.
    pub fn to_shape_with_state(&self, state: &str) -> ConnectPeer {
        ConnectPeer {
            core_network_id: Some(self.core_network_id.clone()),
            connect_attachment_id: Some(self.connect_attachment_id.clone()),
            connect_peer_id: Some(self.id.clone()),
            edge_location: Some(self.edge_location.clone()),
            state: Some(state.to_string()),
            created_at: Some(epoch_seconds(self.created_at)),
            configuration: Some(self.configuration()),
            tags: Some(self.tags.clone()),
            subnet_arn: self.subnet_arn.clone(),
            // "Describes the error associated with the attachment request" (`smithy_query
            // shape=ConnectPeer`): a peer this service accepted and stored has no failed
            // modification to report, so the member is absent rather than an empty list of
            // errors nothing produced.
            last_modification_errors: None,
        }
    }

    /// The modelled `ConnectPeerSummary` for this record, as of `now`: the same stored
    /// record `ListConnectPeers` reports, without the configuration only a read of the one
    /// peer carries (`smithy_query shape=ConnectPeerSummary,ConnectPeer`).
    pub fn to_summary(&self, now: SystemTime) -> ConnectPeerSummary {
        ConnectPeerSummary {
            core_network_id: Some(self.core_network_id.clone()),
            connect_attachment_id: Some(self.connect_attachment_id.clone()),
            connect_peer_id: Some(self.id.clone()),
            edge_location: Some(self.edge_location.clone()),
            connect_peer_state: Some(self.state_at(now).to_string()),
            created_at: Some(epoch_seconds(self.created_at)),
            tags: Some(self.tags.clone()),
            subnet_arn: self.subnet_arn.clone(),
        }
    }
}

/// A stored Connect peer association: the tie between one core network Connect peer and
/// one DEVICE of a global network, optionally over one of that device's links.
///
/// "Associates a core network Connect peer with a device and optionally, with a link."
/// (`smithy_query query="doc" shape="AssociateConnectPeer"`) — like a link association the
/// tie carries no id and no ARN of its own (the modelled `ConnectPeerAssociation` is the
/// peer, the global network, the device, the link and a state, `smithy_query
/// shape=ConnectPeerAssociation`), so the PEER is its identity: one peer reaches its
/// appliance over one device, and the store keys the association by `ConnectPeerId`. That
/// is also what `DisassociateConnectPeer` addresses, which names the peer and nothing else
/// (`smithy_query query="operation" operation="DisassociateConnectPeer"`).
///
/// Like the records above, its state is a function of the clock rather than a field, so
/// reading one never writes.
#[derive(Debug, Clone)]
pub struct ConnectPeerAssociationRecord {
    pub connect_peer_id: String,
    pub global_network_id: String,
    pub device_id: String,
    /// The link the device reaches this peering over, or `None` where the association
    /// named none — the link is optional on the request that makes the association.
    pub link_id: Option<String>,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl ConnectPeerAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `ConnectPeerAssociationState` carries PENDING, AVAILABLE, DELETING and DELETED
    /// (`smithy_query shape=ConnectPeerAssociationState`, mirrored in
    /// `ops::types::CONNECT_PEER_ASSOCIATION_STATE_VALUES`); the first three are spelled
    /// exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`], so this
    /// slice reuses those constants, as [`LinkAssociationRecord::state_at`] does.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `ConnectPeerAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> ConnectPeerAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `ConnectPeerAssociation` for this record, forcing `state` — used by
    /// `DisassociateConnectPeer`, which reports the association it just removed as
    /// `DELETING`, exactly as [`TransitGatewayConnectPeerAssociationRecord::to_shape_with_state`]
    /// serves its own disassociate.
    pub fn to_shape_with_state(&self, state: &str) -> ConnectPeerAssociation {
        ConnectPeerAssociation {
            connect_peer_id: Some(self.connect_peer_id.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: self.link_id.clone(),
            state: Some(state.to_string()),
        }
    }
}

/// The two `Action` values `StartOrganizationServiceAccessUpdate` accepts, in the order
/// its documentation lists them.
///
/// The model types `Action` as a free-form string of length 0..50 and enumerates nothing
/// (`smithy_query shape=Action`); the catalog is the member's own documentation — "The
/// action to take for the update request. This can be either ENABLE or DISABLE"
/// (`smithy_query query="doc" shape="StartOrganizationServiceAccessUpdateRequest"
/// member="Action"`) — so a value outside it is refused by the handler rather than by the
/// schema.
pub const SERVICE_ACCESS_ACTIONS: [&str; 2] = ["ENABLE", "DISABLE"];

/// The input member a refused `StartOrganizationServiceAccessUpdate` names in
/// `ValidationException$Fields[].Name`, and the one a listing names for a token it did not
/// mint.
pub const FIELD_ACTION: &str = "Action";
/// See [`FIELD_ACTION`].
pub const FIELD_NEXT_TOKEN: &str = "NextToken";

/// The two values `OrganizationAwsServiceAccessStatus` carries: "The status of the
/// organization's AWS service access. This will be ENABLED or DISABLED" (`smithy_query
/// query="doc" shape="OrganizationStatus" member="OrganizationAwsServiceAccessStatus"`).
/// The modelled string enumerates nothing, so the pair lives here.
pub const SERVICE_ACCESS_ENABLED: &str = "ENABLED";
/// See [`SERVICE_ACCESS_ENABLED`].
pub const SERVICE_ACCESS_DISABLED: &str = "DISABLED";

/// The two values `SLRDeploymentStatus` carries: "The status of the SLR deployment for the
/// account. This will be either SUCCEEDED or IN_PROGRESS" (`smithy_query query="doc"
/// shape="OrganizationStatus" member="SLRDeploymentStatus"`). A deployment is IN_PROGRESS
/// until the span [`SETTLE_DELAY`] the clock owes has passed, then SUCCEEDED.
pub const SLR_DEPLOYMENT_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`SLR_DEPLOYMENT_IN_PROGRESS`].
pub const SLR_DEPLOYMENT_SUCCEEDED: &str = "SUCCEEDED";

/// The id prefix of an Amazon Web Services Organization. The model constrains the id
/// itself: `OrganizationId` carries `pattern ^o-([0-9a-f]{8,17})$` (`smithy_query
/// shape=OrganizationId`), which the 17 hex digits of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`] satisfy.
pub const ORGANIZATION_KIND: &str = "o";

/// This account's Network Manager service-access record for its Amazon Web Services
/// Organization: whether the service has organization-wide access, and the state of the
/// service-linked-role (SLR) deployment that enabling it started.
///
/// There is at most ONE such record, because this emulator presents exactly one account
/// ([`::engine::emulator_account_id`]) and an account belongs to at most one organization.
/// `SLRDeploymentStatus` is not a stored field: it is a function of the clock
/// ([`OrganizationServiceAccessRecord::slr_deployment_status_at`]), so reading one never
/// writes.
#[derive(Debug, Clone)]
pub struct OrganizationServiceAccessRecord {
    /// The minted `o-<hex>` id of the organization the record belongs to. Minted once and
    /// kept across later enable/disable updates: the organization does not change when
    /// service access to it does.
    pub organization_id: String,
    /// [`SERVICE_ACCESS_ENABLED`] or [`SERVICE_ACCESS_DISABLED`].
    pub service_access_status: String,
    /// The accounts the SLR deployment in effect covers, in account-id order.
    ///
    /// The organization's membership is not knowable from inside this service — the
    /// Organizations emulator's own `ListAccounts` is a scaffold stub, and this emulator
    /// presents exactly one account — so an enabled organization's deployment covers that
    /// single (management) account. EMPTY while service access is disabled: no SLR is
    /// deployed then, so there is no deployment status to report for the organization or
    /// for any account in it.
    pub deployed_accounts: Vec<String>,
    /// The instant the SLR deployment the enabling call started becomes due, i.e. when
    /// `IN_PROGRESS` becomes `SUCCEEDED`.
    pub deployed_at: SystemTime,
}

impl OrganizationServiceAccessRecord {
    /// The SLR deployment status of every account this deployment covers, as of `now`, or
    /// `None` while no SLR is deployed at all (see [`Self::deployed_accounts`]).
    ///
    /// One due instant covers the whole deployment, so this is also the organization-wide
    /// aggregate: `IN_PROGRESS` until it passes, `SUCCEEDED` after.
    pub fn slr_deployment_status_at(&self, now: SystemTime) -> Option<&'static str> {
        if self.deployed_accounts.is_empty() {
            return None;
        }
        Some(if now >= self.deployed_at {
            SLR_DEPLOYMENT_SUCCEEDED
        } else {
            SLR_DEPLOYMENT_IN_PROGRESS
        })
    }

    /// One modelled `AccountStatus` per account the deployment covers, keyed by account id
    /// so [`paginate`] can page over them in a stable order.
    pub fn account_statuses_at(&self, now: SystemTime) -> Vec<(String, AccountStatus)> {
        let status = self.slr_deployment_status_at(now);
        self.deployed_accounts
            .iter()
            .map(|account_id| {
                (
                    account_id.clone(),
                    AccountStatus {
                        account_id: Some(account_id.clone()),
                        slr_deployment_status: status.map(str::to_string),
                    },
                )
            })
            .collect()
    }

    /// The modelled `OrganizationStatus` for this record as of `now`, carrying
    /// `account_status_list` — the page of accounts the caller asked for, which is why the
    /// list is passed in rather than computed here.
    pub fn to_shape(
        &self,
        now: SystemTime,
        account_status_list: Vec<AccountStatus>,
    ) -> OrganizationStatus {
        OrganizationStatus {
            organization_id: Some(self.organization_id.clone()),
            organization_aws_service_access_status: Some(self.service_access_status.clone()),
            slr_deployment_status: self.slr_deployment_status_at(now).map(str::to_string),
            account_status_list: Some(account_status_list),
        }
    }
}

/// The members of `RouteAnalysisStatus` a route analysis of this service reports
/// (`smithy_query shape=RouteAnalysisStatus`: RUNNING, COMPLETED, FAILED — mirrored in
/// `ops::types::ROUTE_ANALYSIS_STATUS_VALUES`).
///
/// An analysis starts `RUNNING` and reaches `COMPLETED` when the transition the clock owes
/// falls due ([`RouteAnalysisRecord::status_at`]). `FAILED` is the analysis itself failing
/// rather than finding no route — a path that does not reach the destination COMPLETES,
/// carrying [`ROUTE_ANALYSIS_NOT_CONNECTED`] — so nothing here reports it.
pub const ROUTE_ANALYSIS_RUNNING: &str = "RUNNING";
/// See [`ROUTE_ANALYSIS_RUNNING`].
pub const ROUTE_ANALYSIS_COMPLETED: &str = "COMPLETED";

/// The members of `RouteAnalysisCompletionResultCode` (`smithy_query
/// shape=RouteAnalysisCompletionResultCode`: CONNECTED, NOT_CONNECTED — mirrored in
/// `ops::types::ROUTE_ANALYSIS_COMPLETION_RESULT_CODE_VALUES`): whether the analysed path
/// reached its destination. A `NOT_CONNECTED` path also carries the reason code for where
/// it stopped ("The reason code. Available only if a connection is not found.",
/// `smithy_query query="doc" shape="RouteAnalysisCompletion" member="ReasonCode"`).
pub const ROUTE_ANALYSIS_CONNECTED: &str = "CONNECTED";
/// See [`ROUTE_ANALYSIS_CONNECTED`].
pub const ROUTE_ANALYSIS_NOT_CONNECTED: &str = "NOT_CONNECTED";

/// The members of `RouteAnalysisCompletionReasonCode` a path of this service completes
/// with, each documented on the member itself (`smithy_query query="doc"
/// shape="RouteAnalysisCompletion" member="ReasonCode"`, mirrored in
/// `ops::types::ROUTE_ANALYSIS_COMPLETION_REASON_CODE_VALUES`):
///
/// * this one — the request named no attachment ARN for the endpoint the path runs
///   TOWARDS, so there is no destination to analyse a path to;
/// * [`REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND`] — "Cannot find an attachment": the
///   ARN an endpoint named is not an attachment the global network holds;
/// * [`REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY`] — "Found an attachment,
///   but not to the correct transit gateway": the ARN names an attachment of the global
///   network that is not one on a transit gateway;
/// * [`REASON_ROUTE_NOT_FOUND`] — "Cannot find a route table with the destination CIDR
///   block": both endpoints are attachments on transit gateways of the global network, and
///   no route table of them carries the destination.
pub const REASON_NO_DESTINATION_ARN_PROVIDED: &str = "NO_DESTINATION_ARN_PROVIDED";
/// See [`REASON_NO_DESTINATION_ARN_PROVIDED`].
pub const REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND: &str =
    "TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND";
/// See [`REASON_NO_DESTINATION_ARN_PROVIDED`].
pub const REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY: &str =
    "TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY";
/// See [`REASON_NO_DESTINATION_ARN_PROVIDED`].
pub const REASON_ROUTE_NOT_FOUND: &str = "ROUTE_NOT_FOUND";

/// The request members a route analysis's two endpoints come from, which is how a reason
/// context names the endpoint whose attachment ARN a path could not get past
/// ([`RouteEndpoint::reason_context`]).
pub const ENDPOINT_SOURCE: &str = "Source";
/// See [`ENDPOINT_SOURCE`].
pub const ENDPOINT_DESTINATION: &str = "Destination";

/// The reason-context key naming the `TransitGatewayAttachmentArn` of the endpoint
/// [`ENDPOINT_SOURCE`] / [`ENDPOINT_DESTINATION`] names.
///
/// `ReasonContext` is "Additional information about the path. Available only if a
/// connection is not found." (`smithy_query query="doc" shape="RouteAnalysisCompletion"
/// member="ReasonContext"`) — an unconstrained map — so the key names the request member
/// whose value the analysis could not resolve, and the value is that ARN.
pub fn reason_context_key(endpoint_member: &str) -> String {
    format!("{endpoint_member}.TransitGatewayAttachmentArn")
}

/// What the global network's network-resource inventory holds for the
/// `TransitGatewayAttachmentArn` one endpoint of a route analysis names.
///
/// A routing path runs between two attachments on transit gateways registered with the
/// global network, so this is what decides whether the path can start at an endpoint at
/// all, and the reason code it completes with when it cannot.
#[derive(Debug, Clone)]
enum RouteEndpointResolution {
    /// The endpoint named no attachment ARN at all.
    Unspecified,
    /// The ARN names no network resource of this global network.
    Unknown,
    /// The ARN names an attachment on a transit gateway registered with this global
    /// network ([`TRANSIT_GATEWAY_ATTACHMENT_TYPE`]), carrying the ARN of the gateway it
    /// was registered through where the inventory records one.
    TransitGatewayAttachment { transit_gateway_arn: Option<String> },
    /// The ARN names a network resource of this global network of some OTHER type,
    /// carrying that `NetworkResource$ResourceType`.
    OtherResource { resource_type: String },
}

/// One endpoint of a route analysis: the request member it came from, what it named, and
/// what the global network knows about the attachment ARN it named.
#[derive(Debug, Clone)]
struct RouteEndpoint {
    /// [`ENDPOINT_SOURCE`] or [`ENDPOINT_DESTINATION`].
    member: &'static str,
    /// The `TransitGatewayAttachmentArn` the endpoint named, if any.
    attachment_arn: Option<String>,
    /// The `IpAddress` the endpoint named, if any. The analysis carries it through to the
    /// endpoint options it reports; no route table of this global network holds a CIDR to
    /// match it against (see [`REASON_ROUTE_NOT_FOUND`]).
    ip_address: Option<String>,
    /// See [`RouteEndpointResolution`].
    resolution: RouteEndpointResolution,
}

impl RouteEndpoint {
    /// The modelled endpoint options this endpoint reports: the ARN and the IP address the
    /// request named, plus the transit gateway the attachment belongs to — which is
    /// RESOLVED from the inventory, not supplied by the caller, and is therefore absent
    /// for an endpoint the global network holds no such attachment for.
    fn options(&self) -> RouteAnalysisEndpointOptions {
        RouteAnalysisEndpointOptions {
            transit_gateway_attachment_arn: self.attachment_arn.clone(),
            transit_gateway_arn: match &self.resolution {
                RouteEndpointResolution::TransitGatewayAttachment {
                    transit_gateway_arn,
                } => transit_gateway_arn.clone(),
                _ => None,
            },
            ip_address: self.ip_address.clone(),
        }
    }

    /// The reason code a path stops with at this endpoint, or `None` for one a path can run
    /// through — an attachment on a transit gateway of this global network.
    fn reason_code(&self) -> Option<&'static str> {
        match &self.resolution {
            RouteEndpointResolution::TransitGatewayAttachment { .. } => None,
            // An attachment of the global network that is not one on a transit gateway: a
            // Cloud WAN attachment ([`ATTACHMENT_KIND`]) is an attachment, so the reason is
            // the "found an attachment, but not to the correct transit gateway" one rather
            // than "cannot find an attachment".
            RouteEndpointResolution::OtherResource { resource_type }
                if resource_type == ATTACHMENT_KIND =>
            {
                Some(REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_IN_TRANSIT_GATEWAY)
            }
            _ => Some(REASON_TRANSIT_GATEWAY_ATTACHMENT_NOT_FOUND),
        }
    }

    /// The reason context a path carries when it stopped at this endpoint: the request
    /// member and the ARN the analysis could not resolve, or `None` for an endpoint that
    /// named no ARN at all — there is nothing further to report about it.
    fn reason_context(&self) -> Option<std::collections::HashMap<String, String>> {
        let arn = self.attachment_arn.clone()?;
        Some(std::collections::HashMap::from([(
            reason_context_key(self.member),
            arn,
        )]))
    }
}

/// A stored route analysis: the Route Analyzer job one `StartRouteAnalysis` records on its
/// global network, which `GetRouteAnalysis` reads back.
///
/// The paths are computed ONCE, by the call that starts the analysis, out of what the
/// global network held at that instant — an analysis is a snapshot of the routing it
/// found, so a resource created afterwards cannot change the answer a completed analysis
/// reports. What the CLOCK owes is the `RUNNING` -> `COMPLETED` transition
/// ([`Self::completed_at`]), so `Status` is a function of the clock rather than a stored
/// field and reading an analysis never writes.
#[derive(Debug, Clone)]
pub struct RouteAnalysisRecord {
    /// The minted `route-analysis-<hex>` id, this record's identity.
    pub id: String,
    /// The global network the analysis was started on: the only one it can be read back
    /// through.
    pub global_network_id: String,
    /// "The time that the analysis started" (`smithy_query query="doc"
    /// shape="RouteAnalysis" member="StartTimestamp"`).
    pub started_at: SystemTime,
    /// The instant the `RUNNING` -> `COMPLETED` transition the clock owes falls due:
    /// [`SETTLE_DELAY`] after the call that started the analysis, so a caller that reads
    /// straight afterwards still sees it `RUNNING`.
    pub completed_at: SystemTime,
    /// The source as the analysis ran it: the ARN and IP address the request named, plus
    /// the transit gateway resolved for that attachment.
    pub source: RouteAnalysisEndpointOptions,
    /// The destination as the analysis ran it; see [`Self::source`].
    pub destination: RouteAnalysisEndpointOptions,
    pub include_return_path: bool,
    pub use_middleboxes: bool,
    /// The forward path, as the analysis completed it.
    pub forward_path: RouteAnalysisPath,
    /// The return path, or `None` where none was analysed: "The return path is not
    /// analyzed if the forward path analysis does not succeed" (`smithy_query query="doc"
    /// shape="RouteAnalysis" member="IncludeReturnPath"`), and none is analysed at all
    /// unless the request asked for one.
    pub return_path: Option<RouteAnalysisPath>,
}

impl RouteAnalysisRecord {
    /// This analysis's status as of `now`: [`ROUTE_ANALYSIS_RUNNING`] until the transition
    /// the clock owes falls due, [`ROUTE_ANALYSIS_COMPLETED`] after it.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.completed_at {
            ROUTE_ANALYSIS_COMPLETED
        } else {
            ROUTE_ANALYSIS_RUNNING
        }
    }

    /// The modelled `RouteAnalysis` for this record as of `now`.
    ///
    /// The paths are "The status of the analysis at completion" (`smithy_query query="doc"
    /// shape="RouteAnalysisPath" member="CompletionStatus"`), so they are reported only
    /// once the analysis has completed; while it is still `RUNNING` there is no completion
    /// to report. Both are already stored, so no read computes them a second time.
    pub fn to_shape(&self, now: SystemTime) -> RouteAnalysis {
        let completed = self.status_at(now) == ROUTE_ANALYSIS_COMPLETED;
        RouteAnalysis {
            global_network_id: Some(self.global_network_id.clone()),
            // "The ID of the AWS account that created the route analysis": this emulator
            // presents exactly one account, which is the one every call runs as.
            owner_account_id: Some(::engine::emulator_account_id().to_string()),
            route_analysis_id: Some(self.id.clone()),
            start_timestamp: Some(epoch_seconds(self.started_at)),
            status: Some(self.status_at(now).to_string()),
            source: Some(self.source.clone()),
            destination: Some(self.destination.clone()),
            include_return_path: self.include_return_path,
            use_middleboxes: self.use_middleboxes,
            forward_path: completed.then(|| self.forward_path.clone()),
            return_path: completed.then(|| self.return_path.clone()).flatten(),
        }
    }
}

/// The path `from` -> `to`, as the analysis completes it.
///
/// A routing path runs from one attachment on a transit gateway of the global network to
/// another, so the completion is decided by what the global network holds for the two
/// endpoints ([`RouteEndpoint::reason_code`]), in the order the path depends on them:
///
/// 1. the endpoint the path runs TOWARDS must name an attachment ARN at all, or there is
///    no destination to analyse towards ([`REASON_NO_DESTINATION_ARN_PROVIDED`]);
/// 2. the endpoint it starts FROM must be an attachment on a transit gateway of this
///    global network, then the one it runs towards must be too;
/// 3. with both of them attachments on transit gateways of this global network, the path
///    needs a route table of those gateways carrying the destination. A transit gateway's
///    route tables are Amazon VPC's, learned through the gateway's registration with the
///    global network, and the inventory holds none, so no route table carries it
///    ([`REASON_ROUTE_NOT_FOUND`]).
///
/// The path carries no component: nothing was traversed, since the analysis stopped at an
/// endpoint or at the missing route table. `UseMiddleboxes` asks for "the location of
/// middlebox appliances in the route analysis" (`smithy_query query="doc"
/// shape="RouteAnalysis" member="UseMiddleboxes"`), which is a property of the components
/// of a path, so it changes nothing about a path that has none.
fn analyze_route_path(from: &RouteEndpoint, to: &RouteEndpoint) -> RouteAnalysisPath {
    let (reason_code, reason_context) = if to.attachment_arn.is_none() {
        (REASON_NO_DESTINATION_ARN_PROVIDED, None)
    } else if let Some(reason) = from.reason_code() {
        (reason, from.reason_context())
    } else if let Some(reason) = to.reason_code() {
        (reason, to.reason_context())
    } else {
        (REASON_ROUTE_NOT_FOUND, to.reason_context())
    };
    RouteAnalysisPath {
        completion_status: Some(RouteAnalysisCompletion {
            result_code: Some(ROUTE_ANALYSIS_NOT_CONNECTED.to_string()),
            reason_code: Some(reason_code.to_string()),
            reason_context,
        }),
        path: Some(Vec::new()),
    }
}

/// Whether `path` reached its destination.
fn route_path_connected(path: &RouteAnalysisPath) -> bool {
    path.completion_status
        .as_ref()
        .and_then(|completion| completion.result_code.as_deref())
        == Some(ROUTE_ANALYSIS_CONNECTED)
}

/// The key the telemetry store holds one registration under: the global network, then the
/// resource ARN, so the map groups a global network's connections into one contiguous run
/// ordered by ARN and a pagination token can name an offset into it.
///
/// The pair is the identity: one connection is registered with one global network, and the
/// same ARN may not be registered with it twice.
pub fn telemetry_key(global_network_id: &str, resource_arn: &str) -> String {
    format!("{global_network_id}/{resource_arn}")
}

/// The id of the resource an ARN names: the text after the last `/` of its resource
/// segment, or the whole segment for an ARN that carries no `/` at all.
///
/// Unlike [`parse_resource_arn`] this judges nothing about the ARN and accepts one of ANY
/// service, because a telemetry resource is owned by whichever service runs the connection
/// (`arn:aws:ec2:<region>:<account>:vpn-connection/<id>` for a VPN connection,
/// `arn:aws:networkmanager::<account>:connect-peer/<id>` for a Cloud WAN Connect peer).
pub fn arn_resource_id(arn: &str) -> &str {
    let resource = arn.rsplit_once(':').map_or(arn, |(_, tail)| tail);
    resource.rsplit_once('/').map_or(resource, |(_, id)| id)
}

/// The TYPE of the resource an ARN names: the text of its resource segment ahead of the
/// first `/`, or `None` for an ARN whose resource segment carries no `/` at all and so
/// names no type of its own.
///
/// The companion of [`arn_resource_id`], and judges as little as it does: the ARN may
/// belong to any service, because the resource an attachment attaches is owned by
/// whichever service runs it (`arn:aws:ec2:<region>:<account>:vpc/<id>` for a VPC,
/// `arn:aws:networkmanager::<account>:attachment/<id>` for the transport attachment a
/// Connect attachment is carried over). So a next hop reports the kind and the id of the
/// resource it reaches as that resource's own ARN spells them, rather than a second copy
/// kept beside it.
pub fn arn_resource_kind(arn: &str) -> Option<&str> {
    let resource = arn.rsplit_once(':').map_or(arn, |(_, tail)| tail);
    let (kind, id) = resource.split_once('/')?;
    (!kind.is_empty() && !id.is_empty()).then_some(kind)
}

/// The account id an ARN carries (its fifth colon-separated field), or `None` when the
/// string has no such field or carries it empty.
///
/// This is the account that OWNS the resource, which is what a telemetry entry reports in
/// `AccountId` — not the account this emulator runs as, since a connection may be owned by
/// another account of the organization.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    let account = arn.split(':').nth(4)?;
    (!account.is_empty()).then_some(account)
}

/// The Region an ARN carries (its fourth colon-separated field), or `None` when the string
/// has no such field or carries it empty — which is what every ARN of this global service
/// carries, and what an ARN of a global resource of another service carries too.
///
/// The companion of [`arn_account_id`], for the same reason: the Region a resource of
/// ANOTHER service lives in is spelled by its own ARN, so a reader comparing Regions reads
/// it from there rather than from a second copy. A VPC attachment's edge location is not a
/// request member either: it is the Region the VPC is in, which only the VPC's ARN names,
/// and the same holds for a site-to-site VPN attachment and the VPN connection it attaches.
/// The modelled patterns for `VpcArn`, `SubnetArn` and `VpnConnectionArn` all allow an
/// EMPTY Region segment
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$`, `smithy_query
/// shape=VpcArn`), so an ARN that names no Region answers `None` here and is refused by the
/// caller rather than stored with an edge location no edge has.
pub fn arn_region(arn: &str) -> Option<&str> {
    let region = arn.split(':').nth(3)?;
    (!region.is_empty()).then_some(region)
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
    /// Link associations by [`link_association_key`], each tying one stored device to one
    /// stored link of the same site.
    ///
    /// No quota of its own bounds this map: an association exists only for a device and a
    /// link that are both stored and sit at the same site, so the per-global-network
    /// device quota and the per-site link quota already bound how many pairs can exist.
    pub link_associations: BTreeMap<String, LinkAssociationRecord>,
    /// Customer gateway associations by [`customer_gateway_association_key`], each tying a
    /// customer gateway of Amazon VPC to one stored device, and optionally one stored link,
    /// of the global network the key names.
    ///
    /// Bounded per global network by
    /// [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] — unlike the link
    /// associations above, the subject is a gateway of another service rather than a stored
    /// resource, so no existing quota bounds how many distinct ARNs a caller can name — and
    /// only ever holding a pair once.
    pub customer_gateway_associations: BTreeMap<String, CustomerGatewayAssociationRecord>,
    /// Transit gateway registrations by [`transit_gateway_registration_key`], each tying a
    /// transit gateway of Amazon VPC to the global network the key names.
    ///
    /// Bounded per global network by
    /// [`MAX_TRANSIT_GATEWAY_REGISTRATIONS_PER_GLOBAL_NETWORK`], for the reason the customer
    /// gateway associations above are: the subject is a gateway of another service rather
    /// than a stored resource, so no existing quota bounds how many distinct ARNs a caller
    /// can name — and only ever holding a pair once.
    pub transit_gateway_registrations: BTreeMap<String, TransitGatewayRegistrationRecord>,
    /// Transit gateway Connect peer associations by
    /// [`transit_gateway_connect_peer_association_key`], each tying a Connect peer of a
    /// transit gateway of Amazon VPC to one stored device, and optionally one stored link,
    /// of the global network the key names.
    ///
    /// Bounded per global network by
    /// [`MAX_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK`], for the reason
    /// the customer gateway associations above are: the subject is a resource of another
    /// service rather than a stored one, so no existing quota bounds how many distinct ARNs
    /// a caller can name — and only ever holding a pair once.
    pub transit_gateway_connect_peer_associations:
        BTreeMap<String, TransitGatewayConnectPeerAssociationRecord>,
    /// Connections by `ConnectionId`, each carrying the `GlobalNetworkId` that owns it and
    /// the two devices of that global network it ties together.
    pub connections: BTreeMap<String, ConnectionRecord>,
    /// Core networks by `CoreNetworkId`, each carrying its owning `GlobalNetworkId`.
    pub core_networks: BTreeMap<String, CoreNetworkRecord>,
    /// Peerings by `PeeringId`, each carrying the `CoreNetworkId` it joins to a resource
    /// of another service ([`PeeringRecord`]). A `BTreeMap`, so the store has one stable
    /// order like every other map here.
    ///
    /// Bounded by [`MAX_PEERINGS_PER_CORE_NETWORK`] per core network, so repeated creates
    /// cannot grow it without end.
    pub peerings: BTreeMap<String, PeeringRecord>,
    /// Attachments by `AttachmentId`, of every kind, each carrying the `CoreNetworkId` it
    /// belongs to. A `BTreeMap`, so listing order is stable and a pagination token can
    /// name a key offset into it.
    ///
    /// One map for every attachment kind: the kinds share the modelled `Attachment`
    /// envelope and differ only in [`AttachmentDetail`], and the quota, the
    /// network-resource inventory and the core-network delete refusal all count
    /// attachments without caring which kind each is.
    pub attachments: BTreeMap<String, AttachmentRecord>,
    /// Prefix list associations by [`prefix_list_association_key`], each tying one stored
    /// core network to one managed prefix list of Amazon VPC. A `BTreeMap`, so listing
    /// order is stable and a pagination token can name a key offset into it.
    ///
    /// Bounded per core network by [`MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK`],
    /// and only ever holding a pair once, so no caller can grow it without end.
    pub prefix_list_associations: BTreeMap<String, PrefixListAssociationRecord>,
    /// Connect peers by `ConnectPeerId`, each carrying the Connect attachment it is carried
    /// over and that attachment's core network and edge location. A `BTreeMap`, so listing
    /// order is stable and a pagination token can name a key offset into it.
    ///
    /// Bounded by [`MAX_CONNECT_PEERS_PER_ATTACHMENT`] per attachment, on top of the
    /// per-core-network attachment quota, so no caller can grow the map without end.
    pub connect_peers: BTreeMap<String, ConnectPeerRecord>,
    /// Connect peer associations by `ConnectPeerId` — the tie each peer has to one device
    /// of a global network ([`ConnectPeerAssociationRecord`]). A `BTreeMap`, so listing
    /// order is the stable peer-id order and a pagination token can name a key offset
    /// into it.
    ///
    /// No quota of its own bounds this map: it holds at most one entry per stored Connect
    /// peer, which [`MAX_CONNECT_PEERS_PER_ATTACHMENT`] and the per-core-network
    /// attachment quota already bound.
    pub connect_peer_associations: BTreeMap<String, ConnectPeerAssociationRecord>,
    /// Resource policies by the ARN of the resource they are attached to — the one store
    /// `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy` share, holding
    /// the document exactly as it was accepted so a reader reports what was put.
    ///
    /// Keyed by ARN rather than hung off a resource record: none of the three operations
    /// models a `ResourceNotFoundException` (`smithy_query query="operation"
    /// operation="GetResourcePolicy,PutResourcePolicy,DeleteResourcePolicy"`), unlike every
    /// other reader of a resource in this service, so the policy store does not assert
    /// that the ARN names a resource this account holds — only that it names a resource
    /// type a policy may be attached to ([`parse_policy_attachable_arn`]), which is a
    /// resource of ANOTHER service. The store is bounded instead by its own quota:
    /// at most [`MAX_RESOURCE_POLICIES`] ARNs hold a policy, each document at most
    /// [`MAX_RESOURCE_POLICY_CHARS`] characters, so no caller can grow it without end.
    pub resource_policies: BTreeMap<String, String>,
    /// Telemetry-bearing connections by [`telemetry_key`]: the registrations
    /// `GetNetworkTelemetry` reports the session health of.
    ///
    /// A connection is registered here by the operation that ties it to the global network:
    /// `AssociateConnectPeer` for a core network Connect peer and
    /// `AssociateTransitGatewayConnectPeer` for a transit gateway one, and the matching
    /// disassociate removes it again. Registering a transit gateway adds nothing here:
    /// the connections a registered gateway carries are VPN connections of Amazon VPC, of
    /// which this service holds no inventory, so an emulator nobody has registered a
    /// connection with reports the empty telemetry it really holds rather than a
    /// placeholder. No quota of its own bounds the map: a
    /// registration exists only for a stored global network, which the per-account global
    /// network quota bounds, and only once per resource ARN.
    pub telemetry_resources: BTreeMap<String, TelemetryResourceRecord>,
    /// Route analyses by `RouteAnalysisId`, each carrying the global network it was
    /// started on and the paths the analysis found there ([`RouteAnalysisRecord`]). A
    /// `BTreeMap`, so the store has one stable order like every other map here.
    ///
    /// Bounded by [`MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK`] per global network, so
    /// repeated `StartRouteAnalysis` calls cannot grow it without end: an analysis is
    /// addressed only through the global network it was started on, so deleting that global
    /// network takes its analyses with it ([`Self::untie_global_network`]) and the bound
    /// keeps holding across repeated create/delete rounds.
    pub route_analyses: BTreeMap<String, RouteAnalysisRecord>,
    /// The metadata one network resource of a global network's inventory carries, by the
    /// ARN of that resource — the one store `UpdateNetworkResourceMetadata` writes and
    /// [`Self::network_resources_of`] reports in `NetworkResource$Metadata`.
    ///
    /// Keyed by ARN alone rather than by the global network and the ARN together: a network
    /// resource of this inventory belongs to exactly one global network, so its ARN already
    /// identifies the pair the operation addresses, which the write path checks before it
    /// stores anything. An entry holds the WHOLE metadata of its resource, so a later write
    /// replaces it rather than growing it, and a resource nobody has written metadata for
    /// has no entry at all — which is what makes `Metadata` absent rather than empty for it.
    ///
    /// Bounded by the resources that exist: at most one entry per stored resource, which the
    /// per-global-network and per-core-network quotas bound, and the write path drops the
    /// entry of a resource that has since been deleted.
    pub network_resource_metadata: BTreeMap<String, BTreeMap<String, String>>,
    /// This account's organization service-access record, or `None` while no
    /// `StartOrganizationServiceAccessUpdate` has ever been called: this service then
    /// knows of no organization, so `ListOrganizationServiceAccessStatus` reports no
    /// `OrganizationStatus` at all. One record, not a map — see
    /// [`OrganizationServiceAccessRecord`].
    pub organization_service_access: Option<OrganizationServiceAccessRecord>,
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

    /// Apply an organization service-access update to the stored record and answer the
    /// record it leaves behind.
    ///
    /// `enable` is the `ENABLE` / `DISABLE` the request's `Action` named (see
    /// [`SERVICE_ACCESS_ACTIONS`]).
    ///
    /// * `ENABLE` on an organization whose access is already `ENABLED` changes NOTHING —
    ///   not the organization id, and not the SLR deployment already in flight — so a
    ///   client that repeats the call does not restart the deployment it is waiting for.
    /// * `ENABLE` otherwise records `ENABLED` and starts a fresh SLR deployment, due
    ///   [`SETTLE_DELAY`] from `now`, covering the accounts of the organization.
    /// * `DISABLE` records `DISABLED` and drops the deployment: with no SLR deployed there
    ///   is no deployment status to report. The organization id is kept, so re-enabling
    ///   reports the same organization.
    pub fn update_organization_service_access(
        &mut self,
        enable: bool,
        now: SystemTime,
    ) -> OrganizationServiceAccessRecord {
        let held = self.organization_service_access.clone();
        let organization_id = match &held {
            Some(record) => record.organization_id.clone(),
            None => self.next_id(ORGANIZATION_KIND),
        };
        let record = match held {
            Some(record)
                if enable && record.service_access_status == SERVICE_ACCESS_ENABLED =>
            {
                record
            }
            _ if enable => OrganizationServiceAccessRecord {
                organization_id,
                service_access_status: SERVICE_ACCESS_ENABLED.to_string(),
                deployed_accounts: vec![::engine::emulator_account_id().to_string()],
                deployed_at: now + SETTLE_DELAY,
            },
            _ => OrganizationServiceAccessRecord {
                organization_id,
                service_access_status: SERVICE_ACCESS_DISABLED.to_string(),
                deployed_accounts: Vec::new(),
                deployed_at: now,
            },
        };
        self.organization_service_access = Some(record.clone());
        record
    }

    /// The ARN of a resource of this service that stands on its own — one whose identity
    /// needs no parent to be unique: `arn:aws:networkmanager::<account>:<kind>/<id>`.
    ///
    /// That is the form for the kinds this service identifies account-wide: a global
    /// network ([`GLOBAL_NETWORK_KIND`]), a core network ([`CORE_NETWORK_KIND`]) and an
    /// attachment ([`ATTACHMENT_KIND`]). A resource that exists only INSIDE a global
    /// network is named by [`Self::scoped_resource_arn`] instead.
    pub fn resource_arn(kind: &str, id: &str) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("{kind}/{id}"))
    }

    /// The ARN of a resource of this service that exists only inside a SCOPE — for every
    /// such kind here, the global network that owns it:
    /// `arn:aws:networkmanager::<account>:<kind>/<scope-id>/<id>`.
    ///
    /// The owner's id is part of the name because the resource has no identity outside it:
    /// a site ([`SITE_KIND`]), a link ([`LINK_KIND`]), a device ([`DEVICE_KIND`]) and a
    /// connection ([`CONNECTION_KIND`]) are each addressed through the global network that
    /// holds them, so their ARN names that global network ahead of their own id — which
    /// stays the LAST path segment, and is what [`parse_resource_arn`] reads back as the id
    /// a lookup keys on.
    pub fn scoped_resource_arn(kind: &str, scope_id: &str, id: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            ARN_REGION,
            &format!("{kind}/{scope_id}/{id}"),
        )
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

    /// How many connections the global network `global_network_id` holds, for the
    /// per-global-network quota and the network-resource inventory.
    pub fn connection_count_of(&self, global_network_id: &str) -> usize {
        self.connections
            .values()
            .filter(|connection| connection.global_network_id == global_network_id)
            .count()
    }

    /// Every stored record of `global_network_id` that still NAMES the link `link_id`,
    /// paired with its kind in the order a refusal should list them — what
    /// [`dependents_block_delete`] turns into the dependency `ValidationException` of
    /// `DeleteLink`.
    ///
    /// A delete may not leave a stored record pointing at a resource no read can resolve
    /// any more, so what blocks it is EVERY reference to the link this store holds, whether
    /// the referring record requires one ([`LinkAssociationRecord`]) or only carries the
    /// link the request supplied ([`CustomerGatewayAssociationRecord`],
    /// [`ConnectionRecord`], [`TransitGatewayConnectPeerAssociationRecord`],
    /// [`ConnectPeerAssociationRecord`]) — a connection holds two such members, either of
    /// which names a link of the same global network. The two kinds the operation's own
    /// documentation names ("You must first disassociate the link from any devices and
    /// customer gateways", `smithy_query query="doc" shape="DeleteLink"`) come first.
    pub fn link_dependents(
        &self,
        global_network_id: &str,
        link_id: &str,
    ) -> Vec<(&'static str, usize)> {
        let names_link = |candidate: &Option<String>| candidate.as_deref() == Some(link_id);
        vec![
            (
                LINK_ASSOCIATION_KIND,
                self.link_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.link_id == link_id
                    })
                    .count(),
            ),
            (
                CUSTOMER_GATEWAY_ASSOCIATION_KIND,
                self.customer_gateway_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && names_link(&association.link_id)
                    })
                    .count(),
            ),
            (
                CONNECTION_KIND,
                self.connections
                    .values()
                    .filter(|connection| {
                        connection.global_network_id == global_network_id
                            && (names_link(&connection.link_id)
                                || names_link(&connection.connected_link_id))
                    })
                    .count(),
            ),
            (
                TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND,
                self.transit_gateway_connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && names_link(&association.link_id)
                    })
                    .count(),
            ),
            (
                CONNECT_PEER_ASSOCIATION_KIND,
                self.connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && names_link(&association.link_id)
                    })
                    .count(),
            ),
        ]
    }

    /// Every stored record of `global_network_id` that still NAMES the device `device_id`,
    /// paired with its kind in the order a refusal should list them — the
    /// [`link_dependents`](Self::link_dependents) of a device, for `DeleteDevice`.
    ///
    /// The same rule holds here: the four ties that each run over one device of the global
    /// network, and the connections whose two device members name it — "Creates a
    /// connection between two devices" (`smithy_query query="doc"
    /// shape="CreateConnection"`), which is a reference needing no association at all. The
    /// kinds the operation's own documentation names ("You must first disassociate the
    /// device from any links and customer gateways", `smithy_query query="doc"
    /// shape="DeleteDevice"`) come first.
    pub fn device_dependents(
        &self,
        global_network_id: &str,
        device_id: &str,
    ) -> Vec<(&'static str, usize)> {
        vec![
            (
                LINK_ASSOCIATION_KIND,
                self.link_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
            (
                CUSTOMER_GATEWAY_ASSOCIATION_KIND,
                self.customer_gateway_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
            (
                CONNECTION_KIND,
                self.connections
                    .values()
                    .filter(|connection| {
                        connection.global_network_id == global_network_id
                            && (connection.device_id == device_id
                                || connection.connected_device_id == device_id)
                    })
                    .count(),
            ),
            (
                TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND,
                self.transit_gateway_connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
            (
                CONNECT_PEER_ASSOCIATION_KIND,
                self.connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
        ]
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

    /// How many attachments `core_network_id` holds, across every attachment kind, for the
    /// per-core-network quota and the core-network delete refusal.
    pub fn attachment_count_of_core_network(&self, core_network_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| attachment.core_network_id == core_network_id)
            .count()
    }

    /// The attachment `attachment_id` names WITHIN the core network `core_network_id`, for
    /// the operations that address a stored attachment THROUGH its core network and mint
    /// nothing — the routing policy label ones.
    ///
    /// Both ids address resources that have to be stored already, so an id naming nothing
    /// stored is the service's not-found refusal rather than the field failure a create
    /// gives for a reference it would hang a new resource off. The resource the request
    /// reaches the attachment THROUGH is checked first, the rule every operation of this
    /// service that addresses a child of a core network follows. The attachment is then
    /// resolved within that core network: one id space holds every attachment kind and a
    /// label applies to any of them, so the kind is never checked — but an attachment
    /// stored under a DIFFERENT core network is not an attachment of the one addressed, so
    /// it is as absent here as an id nothing was ever stored for.
    pub fn attachment_of_core_network_mut(
        &mut self,
        core_network_id: &str,
        attachment_id: &str,
    ) -> Result<&mut AttachmentRecord, ResourceNotFoundException> {
        self.require_attachment_of_core_network(core_network_id, attachment_id)?;
        self.attachments
            .get_mut(attachment_id)
            .ok_or_else(|| resource_not_found(ATTACHMENT_KIND, attachment_id))
    }

    /// The same resolution as [`Self::attachment_of_core_network_mut`] without handing the
    /// record back: `Err` names whichever of the two resources the request addresses the
    /// store does not hold.
    ///
    /// For the point in such a call where the refusal is already owed but the record is not
    /// needed yet: a tokened write resolves the resources it addresses before it resolves
    /// its idempotency token, and that resolution walks the whole attachment map, across
    /// which no borrow of one record can be held.
    pub fn require_attachment_of_core_network(
        &self,
        core_network_id: &str,
        attachment_id: &str,
    ) -> Result<(), ResourceNotFoundException> {
        if !self.core_networks.contains_key(core_network_id) {
            return Err(resource_not_found(CORE_NETWORK_KIND, core_network_id));
        }
        if !self
            .attachments
            .get(attachment_id)
            .is_some_and(|record| record.core_network_id == core_network_id)
        {
            return Err(resource_not_found(ATTACHMENT_KIND, attachment_id));
        }
        Ok(())
    }

    /// How many peerings `core_network_id` holds, for the per-core-network quota and the
    /// core-network delete refusal: a peering hangs off exactly one core network, so
    /// deleting that core network while it holds one would orphan the peering.
    pub fn peering_count_of_core_network(&self, core_network_id: &str) -> usize {
        self.peerings
            .values()
            .filter(|peering| peering.core_network_id == core_network_id)
            .count()
    }

    /// How many customer gateways `global_network_id` is associated with, for the
    /// per-global-network quota.
    pub fn customer_gateway_association_count_of(&self, global_network_id: &str) -> usize {
        self.customer_gateway_associations
            .values()
            .filter(|association| association.global_network_id == global_network_id)
            .count()
    }

    /// How many transit gateways `global_network_id` holds a registration for, for the
    /// per-global-network quota.
    pub fn transit_gateway_registration_count_of(&self, global_network_id: &str) -> usize {
        self.transit_gateway_registrations
            .values()
            .filter(|registration| registration.global_network_id == global_network_id)
            .count()
    }

    /// How many transit gateway Connect peers `global_network_id` is associated with, for
    /// the per-global-network quota.
    pub fn transit_gateway_connect_peer_association_count_of(
        &self,
        global_network_id: &str,
    ) -> usize {
        self.transit_gateway_connect_peer_associations
            .values()
            .filter(|association| association.global_network_id == global_network_id)
            .count()
    }

    /// Whether `global_network_id` holds a transit gateway registration for a gateway that
    /// could carry a Connect peer living in `region` and owned by `account`.
    ///
    /// "You can only associate transit gateway Connect peers that have been created on a
    /// transit gateway that's registered in your global network." (`smithy_query
    /// query="doc" shape="AssociateTransitGatewayConnectPeer"`). WHICH gateway a given
    /// Connect peer was created on is Amazon VPC's record, which this service holds no
    /// inventory of, and the peer's own ARN does not name it; what the ARN does fix is the
    /// Region and the owning account, and a Connect peer always lives in the Region and
    /// account of the transit gateway it was created on. So the registrations that could
    /// carry the peer are those of a gateway in that same Region and account, and a global
    /// network holding none of them holds no gateway the peer can have been created on.
    pub fn carries_transit_gateway_in(
        &self,
        global_network_id: &str,
        region: &str,
        account: Option<&str>,
    ) -> bool {
        self.carries_transit_gateway_in_besides(global_network_id, region, account, None)
    }

    /// [`Self::carries_transit_gateway_in`] with the registration held under `besides`
    /// left out: what the global network would still carry once that registration is gone.
    fn carries_transit_gateway_in_besides(
        &self,
        global_network_id: &str,
        region: &str,
        account: Option<&str>,
        besides: Option<&str>,
    ) -> bool {
        self.transit_gateway_registrations
            .iter()
            .filter(|(key, registration)| {
                registration.global_network_id == global_network_id
                    && Some(key.as_str()) != besides
            })
            .any(|(_, registration)| {
                ec2_resource_region(&registration.transit_gateway_arn, TRANSIT_GATEWAY_TYPE)
                    == Some(region)
                    && arn_account_id(&registration.transit_gateway_arn) == account
            })
    }

    /// How many stored transit gateway Connect peer associations of `global_network_id`
    /// would be left standing on no registration at all once the registration held under
    /// `registration_key` is gone — the count the deregister's dependency refusal is built
    /// from ([`dependents_block_delete`]).
    ///
    /// An association of this kind can only be made while the global network carries a
    /// registration of a gateway the peer could have been created on
    /// ([`Self::carries_transit_gateway_in`]), and it keeps standing on that precondition
    /// for as long as it is stored, so removing the LAST registration that satisfies it
    /// would leave the tie in exactly the state the associate refuses to create. Another
    /// registration in the peer's Region and account still satisfies it, which is why the
    /// question is asked of the registrations that would remain rather than of the one
    /// being removed.
    pub fn transit_gateway_connect_peer_associations_standing_on(
        &self,
        global_network_id: &str,
        registration_key: &str,
    ) -> usize {
        self.transit_gateway_connect_peer_associations
            .values()
            .filter(|association| association.global_network_id == global_network_id)
            .filter(|association| {
                ec2_resource_region(
                    &association.transit_gateway_connect_peer_arn,
                    TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
                )
                .is_some_and(|region| {
                    !self.carries_transit_gateway_in_besides(
                        global_network_id,
                        region,
                        arn_account_id(&association.transit_gateway_connect_peer_arn),
                        Some(registration_key),
                    )
                })
            })
            .count()
    }

    /// Whether `global_network_id` carries, in `region`, something a VPN connection landing
    /// there can be attached to: a registered transit gateway in that Region, or a core
    /// network of this global network whose policy IN FORCE configures an edge there.
    ///
    /// "You can only associate customer gateways that are connected to a VPN attachment on a
    /// transit gateway or core network registered in your global network." (`smithy_query
    /// query="doc" shape="AssociateCustomerGateway"`). WHICH VPN connection a given customer
    /// gateway is connected to is Amazon VPC's record, which this service holds no inventory
    /// of, and the gateway's own ARN does not name it; what that ARN does fix is the Region
    /// the gateway lives in, and a VPN connection is a Regional resource, so the transit
    /// gateway it attaches to and the core network EDGE a site-to-site VPN attachment of it
    /// sits at ([`PolicyTopology::has_edge_location`], which is also what
    /// [`check_edge_location`] judges such an attachment against) are both in that same
    /// Region. So the carriers that could reach the gateway are a registration of a gateway
    /// in its Region and a core network edge configured there, and a global network holding
    /// neither holds nothing the gateway can be connected to a VPN attachment on.
    ///
    /// The owning ACCOUNT is not compared, unlike [`Self::carries_transit_gateway_in`]: a
    /// Connect peer is created ON the registered gateway and so belongs to the same account
    /// as it, while a VPN connection belongs to the account that owns the customer gateway
    /// and reaches a transit gateway or a core network edge the global network's own account
    /// owns (`RegisterTransitGateway` holds every registration to that account), which is a
    /// different account whenever the customer gateway is another account's.
    pub fn carries_vpn_attachment_in(&self, global_network_id: &str, region: &str) -> bool {
        let registered = self
            .transit_gateway_registrations
            .values()
            .filter(|registration| registration.global_network_id == global_network_id)
            .any(|registration| {
                ec2_resource_region(&registration.transit_gateway_arn, TRANSIT_GATEWAY_TYPE)
                    == Some(region)
            });
        registered
            || self
                .core_networks
                .values()
                .filter(|core_network| core_network.global_network_id == global_network_id)
                .any(|core_network| core_network.topology().has_edge_location(region))
    }

    /// How many managed prefix lists `core_network_id` is associated with, for the
    /// per-core-network quota.
    pub fn prefix_list_association_count_of(&self, core_network_id: &str) -> usize {
        self.prefix_list_associations
            .values()
            .filter(|association| association.core_network_id == core_network_id)
            .count()
    }

    /// How many Connect attachments are carried over the attachment `attachment_id` as
    /// their transport, for the attachment delete refusal.
    ///
    /// A Connect attachment is built on another attachment rather than on a resource of its
    /// own ([`AttachmentDetail::Connect::transport_attachment_id`]), so that transport is a
    /// dependent of exactly the kind every other delete of this service refuses to orphan.
    pub fn connect_attachment_count_over_transport(&self, attachment_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| match &attachment.detail {
                AttachmentDetail::Connect {
                    transport_attachment_id,
                    ..
                } => transport_attachment_id == attachment_id,
                AttachmentDetail::Vpc { .. }
                | AttachmentDetail::SiteToSiteVpn
                | AttachmentDetail::DirectConnectGateway { .. }
                | AttachmentDetail::TransitGatewayRouteTable { .. } => false,
            })
            .count()
    }

    /// How many attachments are created over the peering `peering_id`, for the peering
    /// delete refusal.
    ///
    /// A transit gateway route table attachment takes its core network and its edge location
    /// from the peering it is created over
    /// ([`AttachmentDetail::TransitGatewayRouteTable::peering_id`]) rather than from a
    /// resource of its own, so that peering is a dependent of exactly the kind every other
    /// delete of this service refuses to orphan.
    pub fn attachment_count_over_peering(&self, peering_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| match &attachment.detail {
                AttachmentDetail::TransitGatewayRouteTable {
                    peering_id: over, ..
                } => over == peering_id,
                AttachmentDetail::Connect { .. }
                | AttachmentDetail::Vpc { .. }
                | AttachmentDetail::SiteToSiteVpn
                | AttachmentDetail::DirectConnectGateway { .. } => false,
            })
            .count()
    }

    /// How many Connect peers are carried over the Connect attachment `attachment_id`, for
    /// the per-attachment quota [`MAX_CONNECT_PEERS_PER_ATTACHMENT`] and the attachment
    /// delete refusal: a peer hangs off exactly one Connect attachment, so deleting that
    /// attachment while it carries one would orphan the peer.
    pub fn connect_peer_count_of_attachment(&self, attachment_id: &str) -> usize {
        self.connect_peers
            .values()
            .filter(|peer| peer.connect_attachment_id == attachment_id)
            .count()
    }

    /// How many Connect peers the global network `global_network_id` holds, for the
    /// network-resource inventory: a peer hangs off a Connect attachment's core network,
    /// which hangs off a global network, so the count is resolved through that chain rather
    /// than stored twice.
    pub fn connect_peer_count_of_global_network(&self, global_network_id: &str) -> usize {
        self.connect_peers
            .values()
            .filter(|peer| self.holds_core_network(global_network_id, &peer.core_network_id))
            .count()
    }

    /// Whether the global network `global_network_id` owns the core network
    /// `core_network_id`: the chain every resource hanging off a core network is resolved to
    /// its global network through.
    fn holds_core_network(&self, global_network_id: &str, core_network_id: &str) -> bool {
        self.core_networks
            .get(core_network_id)
            .is_some_and(|core_network| core_network.global_network_id == global_network_id)
    }

    /// How many attachments the global network `global_network_id` holds, for the
    /// network-resource inventory: an attachment hangs off a core network, which hangs off
    /// a global network, so the count is resolved through that chain rather than stored
    /// twice.
    pub fn attachment_count_of_global_network(&self, global_network_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| {
                self.holds_core_network(global_network_id, &attachment.core_network_id)
            })
            .count()
    }

    /// How many peerings the global network `global_network_id` holds, for the
    /// network-resource inventory: a peering hangs off a core network, which hangs off a
    /// global network, so the count is resolved through that chain rather than stored
    /// twice.
    pub fn peering_count_of_global_network(&self, global_network_id: &str) -> usize {
        self.peerings_of_global_network(global_network_id).count()
    }

    /// The peerings the global network `global_network_id` holds, in `PeeringId` order.
    /// See [`Self::peering_count_of_global_network`] for the chain they are resolved
    /// through.
    fn peerings_of_global_network<'a>(
        &'a self,
        global_network_id: &'a str,
    ) -> impl Iterator<Item = &'a PeeringRecord> {
        self.peerings.values().filter(move |peering| {
            self.core_networks
                .get(&peering.core_network_id)
                .is_some_and(|core_network| core_network.global_network_id == global_network_id)
        })
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
            CONNECTION_KIND => self.connection_count_of(global_network_id),
            CORE_NETWORK_KIND => self.core_network_count_of(global_network_id),
            PEERING_KIND => self.peering_count_of_global_network(global_network_id),
            ATTACHMENT_KIND => self.attachment_count_of_global_network(global_network_id),
            CONNECT_PEER_KIND => self.connect_peer_count_of_global_network(global_network_id),
            _ => 0,
        };
        i32::try_from(held).unwrap_or(i32::MAX)
    }

    /// Every network resource the global network `global_network_id` holds, as of `now`,
    /// keyed by [`network_resource_key`] so the listing is ordered and a page token can
    /// name an offset into it.
    ///
    /// The inventory is the same one [`Self::network_resource_count`] counts, branch for
    /// branch — attachments, Connect peers, connections, core networks, devices, links and
    /// sites — so the count of a type and the resources listed for it can never disagree. A
    /// kind whose creating operation is still a scaffold stub is held nowhere and therefore
    /// contributes nothing, not a placeholder. A connection registered with a global
    /// network only for its telemetry lives in a different store
    /// ([`Self::telemetry_resources`]) that [`Self::network_resource_count`] does not count
    /// either, so the two agree on those registrations as well: a connection's session is
    /// reported by `GetNetworkTelemetry` alone, while the resource the registration hangs
    /// off — the Connect peer, or the attachment that attached the VPN connection — is what
    /// this inventory lists. A
    /// transit gateway registered with the global network
    /// ([`Self::transit_gateway_registrations`]) is a resource of Amazon VPC rather than one
    /// this service holds, so it is not part of this inventory either.
    pub fn network_resources_of(
        &self,
        global_network_id: &str,
        now: SystemTime,
    ) -> Vec<(String, NetworkResource)> {
        let mut resources: BTreeMap<String, NetworkResource> = BTreeMap::new();
        for record in self.attachments.values().filter(|attachment| {
            self.holds_core_network(global_network_id, &attachment.core_network_id)
        }) {
            // An attachment's Describe reader is the one for its OWN kind, so the
            // definition is rendered from that kind's shape — the same shape
            // `GetVpcAttachment` / `GetConnectAttachment` / `GetSiteToSiteVpnAttachment` /
            // `GetDirectConnectGatewayAttachment` answer. Each conversion answers `None`
            // for a record of another kind, so exactly one of them builds a shape here.
            let definition = if let Some(vpc) = record.to_vpc_attachment(now) {
                resource_definition(&VpcAttachment::SCHEMA, &vpc)
            } else if let Some(connect) = record.to_connect_attachment(now) {
                resource_definition(&ConnectAttachment::SCHEMA, &connect)
            } else if let Some(vpn) = record.to_site_to_site_vpn_attachment(now) {
                resource_definition(&SiteToSiteVpnAttachment::SCHEMA, &vpn)
            } else if let Some(gateway) = record.to_direct_connect_gateway_attachment(now) {
                resource_definition(&DirectConnectGatewayAttachment::SCHEMA, &gateway)
            } else if let Some(route_table) = record.to_transit_gateway_route_table_attachment(now)
            {
                resource_definition(&TransitGatewayRouteTableAttachment::SCHEMA, &route_table)
            } else {
                None
            };
            resources.insert(
                network_resource_key(ATTACHMENT_KIND, &record.id),
                NetworkResource {
                    // An attachment belongs to the core network it joins a resource to.
                    core_network_id: Some(record.core_network_id.clone()),
                    // The attachment lives at the edge it was created at, which its own
                    // reader reports as the Region of that edge
                    // ([`AttachmentRecord::sole_edge_location`]).
                    aws_region: record.sole_edge_location().map(str::to_string),
                    definition,
                    ..network_resource(ATTACHMENT_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self.peerings_of_global_network(global_network_id) {
            resources.insert(
                network_resource_key(PEERING_KIND, &record.id),
                NetworkResource {
                    // A peering belongs to the core network it joins to another resource.
                    core_network_id: Some(record.core_network_id.clone()),
                    // A peering joins ONE edge of its core network to the peer, and its
                    // own reader reports that edge as `Peering$EdgeLocation`.
                    aws_region: Some(record.edge_location.clone()),
                    definition: resource_definition(&Peering::SCHEMA, &record.to_peering(now)),
                    ..network_resource(PEERING_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .connect_peers
            .values()
            .filter(|peer| self.holds_core_network(global_network_id, &peer.core_network_id))
        {
            resources.insert(
                network_resource_key(CONNECT_PEER_KIND, &record.id),
                NetworkResource {
                    // A Connect peer belongs to the core network of the Connect attachment
                    // it is carried over.
                    core_network_id: Some(record.core_network_id.clone()),
                    // A Connect peer lands on the edge of that attachment, which its own
                    // reader reports as `ConnectPeer$EdgeLocation`.
                    aws_region: Some(record.edge_location.clone()),
                    definition: resource_definition(&ConnectPeer::SCHEMA, &record.to_shape(now)),
                    ..network_resource(CONNECT_PEER_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .connections
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(CONNECTION_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Connection::SCHEMA, &record.to_shape(now)),
                    ..network_resource(CONNECTION_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .core_networks
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(CORE_NETWORK_KIND, &record.id),
                NetworkResource {
                    // A core network IS a core network's resource, so this is the one
                    // kind of this service whose `CoreNetworkId` is its own id.
                    core_network_id: Some(record.id.clone()),
                    definition: resource_definition(&CoreNetwork::SCHEMA, &record.to_shape(now)),
                    ..network_resource(CORE_NETWORK_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .devices
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(DEVICE_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Device::SCHEMA, &record.to_shape(now)),
                    ..network_resource(DEVICE_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .links
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(LINK_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Link::SCHEMA, &record.to_shape(now)),
                    ..network_resource(LINK_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .sites
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(SITE_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Site::SCHEMA, &record.to_shape(now)),
                    ..network_resource(SITE_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        // "The resource metadata" (`smithy_query shape=NetworkResource`) is what
        // `UpdateNetworkResourceMetadata` stores for one resource ARN, so every resource of
        // this inventory reports the map last stored under its own ARN, whatever its kind,
        // and a resource nobody has stored metadata for reports none.
        for resource in resources.values_mut() {
            if let Some(stored) = resource
                .resource_arn
                .as_deref()
                .and_then(|arn| self.network_resource_metadata.get(arn))
            {
                resource.metadata = Some(metadata_shape(stored));
            }
        }
        resources.into_iter().collect()
    }

    /// Store `metadata` as the WHOLE metadata of the network resource `resource_arn` names
    /// inside the global network `global_network_id`, answering the map that resource then
    /// carries, or `None` when that global network holds no such network resource.
    ///
    /// The resource is resolved through the global network's own inventory
    /// ([`Self::network_resources_of`]), so exactly the resources a caller can SEE in that
    /// global network can be given metadata, and one naming another global network's
    /// resource — or none at all — is refused by the caller rather than stored.
    ///
    /// The map REPLACES whatever the resource carried: the operation's input carries the
    /// resource metadata in full, so a key the request leaves out is a key the resource no
    /// longer has.
    ///
    /// Writing also drops the entries of resources that have since been deleted, so the
    /// store holds metadata only for resources that still exist (see
    /// [`Self::network_resource_metadata`]).
    pub fn put_network_resource_metadata(
        &mut self,
        global_network_id: &str,
        resource_arn: &str,
        metadata: BTreeMap<String, String>,
        now: SystemTime,
    ) -> Option<BTreeMap<String, String>> {
        if !self
            .network_resources_of(global_network_id, now)
            .iter()
            .any(|(_, resource)| resource.resource_arn.as_deref() == Some(resource_arn))
        {
            return None;
        }
        let deleted: Vec<String> = self
            .network_resource_metadata
            .keys()
            .filter(|arn| self.tags_of(arn.as_str()).is_none())
            .cloned()
            .collect();
        for arn in deleted {
            self.network_resource_metadata.remove(&arn);
        }
        self.network_resource_metadata
            .insert(resource_arn.to_string(), metadata);
        self.network_resource_metadata.get(resource_arn).cloned()
    }

    /// Every relationship the global network `global_network_id` holds, keyed by
    /// [`relationship_key`] so the listing is ordered and a page token can name an offset
    /// into it.
    ///
    /// A relationship is one TIE the store holds between two resources named by ARN: the
    /// parent that owns a resource of this global network's inventory
    /// ([`Self::network_resources_of`]), the resource an attachment or a peering joins to
    /// it, and each resource an association ties it to. `From` is the resource the tie is
    /// recorded on — the child, the attaching resource, or, for an association that is no
    /// resource of its own, the DEVICE (and the LINK, where the association names one) of
    /// this global network it ties — and `To` is the resource at the other end. Every ARN
    /// comes from a stored record, so a tie whose other end belongs to another service (a
    /// customer gateway, a VPC, a transit gateway, a managed prefix list) is reported under
    /// the ARN that service's resource carries, and a pair two ties both name is held once.
    ///
    /// Two things deliberately contribute no pair:
    ///
    /// * A transit gateway registration ties a gateway to the GLOBAL NETWORK itself, which
    ///   is the scope this listing is taken over rather than a resource inside it.
    /// * The subnets a VPC attachment reaches its VPC through, and the subnet a device's
    ///   AWS location names, say HOW and WHERE a resource is reached rather than naming
    ///   another resource the tie joins — the resource an attachment attaches is the one
    ///   [`AttachmentRecord::resource_arn`] names.
    pub fn network_resource_relationships(
        &self,
        global_network_id: &str,
    ) -> Vec<(String, Relationship)> {
        /// Hold the pair `from` -> `to`. Keyed by the pair, so two stored ties that name
        /// the same pair leave one relationship rather than two.
        fn tie(pairs: &mut BTreeMap<String, Relationship>, from: &str, to: &str) {
            pairs.insert(
                relationship_key(from, to),
                Relationship {
                    from: Some(from.to_string()),
                    to: Some(to.to_string()),
                },
            );
        }

        // Each resolver answers the stored ARN of a resource of THIS global network, and
        // `None` for an id it holds none for — an id of another global network included, so
        // a tie never reaches out of the scope the listing is taken over.
        let site_arn = |id: &str| {
            self.sites
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let link_arn = |id: &str| {
            self.links
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let device_arn = |id: &str| {
            self.devices
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let core_network_arn = |id: &str| {
            self.core_networks
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let attachment_arn = |id: &str| {
            self.attachments
                .get(id)
                .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
                .map(|record| record.arn.clone())
        };
        let peering_arn = |id: &str| {
            self.peerings
                .get(id)
                .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
                .map(|record| record.arn.clone())
        };
        let connect_peer_arn = |id: &str| {
            self.connect_peers
                .get(id)
                .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
                .map(|record| record.arn.clone())
        };

        let mut pairs: BTreeMap<String, Relationship> = BTreeMap::new();

        // A link belongs to a site, and a device sits at one where its create named one.
        for record in self
            .links
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(site) = site_arn(&record.site_id) {
                tie(&mut pairs, &record.arn, &site);
            }
        }
        for record in self
            .devices
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(site) = record.site_id.as_deref().and_then(&site_arn) {
                tie(&mut pairs, &record.arn, &site);
            }
        }

        // A connection joins two devices of the global network, each over the link the
        // create named for it.
        for record in self
            .connections
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            for device in [&record.device_id, &record.connected_device_id] {
                if let Some(arn) = device_arn(device) {
                    tie(&mut pairs, &record.arn, &arn);
                }
            }
            for link in [record.link_id.as_deref(), record.connected_link_id.as_deref()]
                .into_iter()
                .flatten()
            {
                if let Some(arn) = link_arn(link) {
                    tie(&mut pairs, &record.arn, &arn);
                }
            }
        }

        // An attachment belongs to its core network and attaches one resource; a transit
        // gateway route table attachment reaches that route table THROUGH a peering, which
        // is the one kind whose detail names a further resource of its own.
        for record in self.attachments.values().filter(|record| {
            self.holds_core_network(global_network_id, &record.core_network_id)
        }) {
            tie(&mut pairs, &record.arn, &record.core_network_arn);
            tie(&mut pairs, &record.arn, &record.resource_arn);
            if let AttachmentDetail::TransitGatewayRouteTable { peering_id } = &record.detail {
                if let Some(arn) = peering_arn(peering_id) {
                    tie(&mut pairs, &record.arn, &arn);
                }
            }
        }

        // A peering joins its core network to the resource it peers with.
        for record in self.peerings_of_global_network(global_network_id) {
            tie(&mut pairs, &record.arn, &record.core_network_arn);
            tie(&mut pairs, &record.arn, &record.resource_arn);
        }

        // A Connect peer is carried over its Connect attachment.
        for record in self
            .connect_peers
            .values()
            .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
        {
            if let Some(arn) = attachment_arn(&record.connect_attachment_id) {
                tie(&mut pairs, &record.arn, &arn);
            }
        }

        // A core network is tied to each managed prefix list associated with it.
        for record in self.prefix_list_associations.values() {
            if let Some(arn) = core_network_arn(&record.core_network_id) {
                tie(&mut pairs, &arn, &record.prefix_list_arn);
            }
        }

        // Every remaining tie is one an association holds between a resource and a DEVICE
        // of this global network, optionally over one of that device's LINKS: the tie is
        // recorded on each of those two, so each is a `From` of the resource it reaches.
        let mut device_ties: Vec<(&str, Option<&str>, String)> = Vec::new();
        for record in self
            .link_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(arn) = link_arn(&record.link_id) {
                device_ties.push((&record.device_id, None, arn));
            }
        }
        for record in self
            .customer_gateway_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            device_ties.push((
                &record.device_id,
                record.link_id.as_deref(),
                record.customer_gateway_arn.clone(),
            ));
        }
        for record in self
            .connect_peer_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(arn) = connect_peer_arn(&record.connect_peer_id) {
                device_ties.push((&record.device_id, record.link_id.as_deref(), arn));
            }
        }
        for record in self
            .transit_gateway_connect_peer_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            device_ties.push((
                &record.device_id,
                record.link_id.as_deref(),
                record.transit_gateway_connect_peer_arn.clone(),
            ));
        }
        for (device_id, link_id, to) in device_ties {
            if let Some(from) = device_arn(device_id) {
                tie(&mut pairs, &from, &to);
            }
            if let Some(from) = link_id.and_then(&link_arn) {
                tie(&mut pairs, &from, &to);
            }
        }

        pairs.into_iter().collect()
    }

    /// Register `record` as a telemetry-bearing connection of its global network,
    /// replacing any registration that global network already holds for the same resource
    /// ARN (see [`telemetry_key`]).
    pub fn register_telemetry_resource(&mut self, record: TelemetryResourceRecord) {
        self.telemetry_resources.insert(
            telemetry_key(&record.global_network_id, &record.resource_arn),
            record,
        );
    }

    /// Register the connection the attachment `attachment_id` attaches as a
    /// telemetry-bearing connection of the global network that owns its core network, at
    /// `now`, answering whether a registration was made.
    ///
    /// The call that ties a connection into a global network is what registers its session
    /// ([`Self::register_telemetry_resource`], the same point the two Connect peer
    /// associates register theirs), so an attachment that attaches a connection whose
    /// session this service measures ([`AttachmentRecord::telemetry_resource_type`]) is
    /// registered by the call that starts attaching it: its create, or the acceptance that
    /// releases one the policy held for one. Nothing is registered for an attachment that
    /// attaches no such connection, for one still waiting for that acceptance or already
    /// rejected (nothing is created for it, so there is no session to measure:
    /// [`AttachmentRecord::is_created`]), or for one whose core network is no longer stored.
    ///
    /// The session starts DOWN and comes up when the attachment carrying it reaches
    /// `AVAILABLE` ([`AttachmentRecord::available_at`]) — the transition the clock already
    /// owes that attachment, so the session the registration starts needs no span of its
    /// own and no handler sleeps for it. Every member of the registration is read off the
    /// attachment: the connection's own ARN identifies it, the attachment's edge location is
    /// the Region the session lands in, and its core network is the one the connection
    /// reaches. `address` is left unset because a VPN connection's far end is Amazon VPC's
    /// record, which this service holds no inventory of.
    pub fn register_attachment_telemetry(&mut self, attachment_id: &str, now: SystemTime) -> bool {
        let Some(attachment) = self.attachments.get(attachment_id) else {
            return false;
        };
        let Some(resource_type) = attachment.telemetry_resource_type() else {
            return false;
        };
        let Some(connected_at) = attachment.available_at.filter(|_| attachment.is_created()) else {
            return false;
        };
        let Some(global_network_id) = self
            .core_networks
            .get(&attachment.core_network_id)
            .map(|core_network| core_network.global_network_id.clone())
        else {
            return false;
        };
        let record = TelemetryResourceRecord {
            global_network_id,
            resource_type: resource_type.to_string(),
            resource_arn: attachment.resource_arn.clone(),
            aws_region: attachment.edge_location.clone(),
            core_network_id: Some(attachment.core_network_id.clone()),
            // The connection reaches a core network edge rather than a gateway registered
            // with the global network, so no registration carries it.
            registered_gateway_arn: None,
            address: None,
            registered_at: now,
            connected_at,
        };
        self.register_telemetry_resource(record);
        true
    }

    /// Drop the registration the attachment `attachment` made for the connection it
    /// attaches, answering whether one was dropped.
    ///
    /// The counterpart of [`Self::register_attachment_telemetry`]: the call that unties the
    /// connection from the global network — the delete of the attachment that attached it —
    /// is what deregisters it, so a global network reports no session for a connection it
    /// no longer holds. Takes the record rather than its id, so the delete can untie an
    /// attachment it has already removed from the store.
    pub fn deregister_attachment_telemetry(&mut self, attachment: &AttachmentRecord) -> bool {
        if attachment.telemetry_resource_type().is_none() {
            return false;
        }
        let Some(global_network_id) = self
            .core_networks
            .get(&attachment.core_network_id)
            .map(|core_network| core_network.global_network_id.clone())
        else {
            return false;
        };
        self.deregister_telemetry_resource(&global_network_id, &attachment.resource_arn)
            .is_some()
    }

    /// Drop the registration `global_network_id` holds for the connection `resource_arn`,
    /// answering the record that was dropped, or `None` where it held none.
    ///
    /// The counterpart of [`Self::register_telemetry_resource`]: the operation that unties
    /// a connection from its global network is what deregisters it, so a global network
    /// stops reporting the session health of a peering it no longer holds.
    pub fn deregister_telemetry_resource(
        &mut self,
        global_network_id: &str,
        resource_arn: &str,
    ) -> Option<TelemetryResourceRecord> {
        self.telemetry_resources
            .remove(&telemetry_key(global_network_id, resource_arn))
    }

    /// Drop every record the store holds that exists only as a TIE to the Connect peer
    /// `connect_peer_id` — the association keyed by that peer
    /// ([`Self::connect_peer_associations`]) and the telemetry registration that
    /// association made under the peer's ARN in the global network it tied the peer to —
    /// answering the association that was dropped, or `None` where the peer held none.
    ///
    /// Removing the TIE (`DisassociateConnectPeer`) and removing the PEER
    /// (`DeleteConnectPeer`) both untie the peer through here, so the two records can never
    /// come apart: no reader of the ties reports an association for a peer that is gone,
    /// and no registration is left behind with nothing able to remove it.
    ///
    /// The ARN the registration is keyed by is computed from the id — the one name every
    /// Connect peer carries ([`Self::resource_arn`] over [`CONNECT_PEER_KIND`], which is
    /// how the create mints it) — rather than read out of the peer record, so untying works
    /// whether or not that record is still stored.
    pub fn untie_connect_peer(
        &mut self,
        connect_peer_id: &str,
    ) -> Option<ConnectPeerAssociationRecord> {
        let removed = self.connect_peer_associations.remove(connect_peer_id)?;
        self.deregister_telemetry_resource(
            &removed.global_network_id,
            &Self::resource_arn(CONNECT_PEER_KIND, connect_peer_id),
        );
        Some(removed)
    }

    /// Drop every record the store holds that exists only as a record OF the global network
    /// `global_network_id`, answering how many were dropped.
    ///
    /// Today that is its route analyses ([`Self::route_analyses`]): an analysis carries no
    /// identity outside the global network it was started on
    /// ([`RouteAnalysisRecord::global_network_id`], the only one `GetRouteAnalysis` resolves
    /// it through) and this service models no operation that removes one, so the analyses go
    /// with the global network the way the ties a core network holds go with the core
    /// network. Were they left behind, each would be keyed to a global network no call can
    /// address: unreadable, unremovable, and no longer counted by the per-global-network
    /// bound on the map.
    ///
    /// Unlike a dependent the caller can remove itself — a child resource, or a transit
    /// gateway registration — these cannot block the delete: there is no call that would
    /// empty them first.
    pub fn untie_global_network(&mut self, global_network_id: &str) -> usize {
        let held = self.route_analysis_count_of(global_network_id);
        self.route_analyses
            .retain(|_, analysis| analysis.global_network_id != global_network_id);
        held
    }

    /// How many route analyses the global network `global_network_id` holds, for the
    /// per-global-network quota ([`MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK`]).
    pub fn route_analysis_count_of(&self, global_network_id: &str) -> usize {
        self.route_analyses
            .values()
            .filter(|analysis| analysis.global_network_id == global_network_id)
            .count()
    }

    /// What the global network `global_network_id` holds for one endpoint of a route
    /// analysis: the `member` the endpoint came from ([`ENDPOINT_SOURCE`] /
    /// [`ENDPOINT_DESTINATION`]) and the ARN and IP address it named.
    ///
    /// The endpoint's attachment ARN is looked up in the global network's one
    /// network-resource inventory ([`Self::network_resources_of`]) — the same inventory
    /// `GetNetworkResources` reports and `GetNetworkResourceCounts` counts — so a route
    /// analysis and that inventory cannot disagree about which attachments the global
    /// network holds. An attachment on a transit gateway is a resource of Amazon VPC, of
    /// which this service holds no inventory — registering the gateway with the global
    /// network records the registration and no attachment of it — so no such ARN resolves to
    /// one and the analysis reports the "cannot find an attachment" it really found rather
    /// than a placeholder.
    fn resolve_route_endpoint(
        &self,
        global_network_id: &str,
        member: &'static str,
        spec: &RouteAnalysisEndpointOptionsSpecification,
        now: SystemTime,
    ) -> RouteEndpoint {
        let resolution = match spec.transit_gateway_attachment_arn.as_deref() {
            None => RouteEndpointResolution::Unspecified,
            Some(arn) => {
                let held = self
                    .network_resources_of(global_network_id, now)
                    .into_iter()
                    .map(|(_, resource)| resource)
                    .find(|resource| resource.resource_arn.as_deref() == Some(arn));
                match held {
                    None => RouteEndpointResolution::Unknown,
                    Some(resource) => {
                        let resource_type = resource.resource_type.unwrap_or_default();
                        if resource_type == TRANSIT_GATEWAY_ATTACHMENT_TYPE {
                            RouteEndpointResolution::TransitGatewayAttachment {
                                // The gateway the attachment was registered through, which
                                // is the transit gateway it belongs to.
                                transit_gateway_arn: resource.registered_gateway_arn,
                            }
                        } else {
                            RouteEndpointResolution::OtherResource { resource_type }
                        }
                    }
                }
            }
        };
        RouteEndpoint {
            member,
            attachment_arn: spec.transit_gateway_attachment_arn.clone(),
            ip_address: spec.ip_address.clone(),
            resolution,
        }
    }

    /// Start a route analysis on the global network `global_network_id` and answer the
    /// record it leaves behind.
    ///
    /// The analysis is run HERE, by the starting call, against what the global network
    /// holds at `now` ([`analyze_route_path`]): the paths are stored with the record, so
    /// every later read of the analysis reports the routing it found and nothing created
    /// afterwards changes it. The record is the whole answer — `StartRouteAnalysis` and
    /// `GetRouteAnalysis` both report it through [`RouteAnalysisRecord::to_shape`] — so the
    /// two cannot disagree about one analysis.
    pub fn start_route_analysis(
        &mut self,
        global_network_id: &str,
        source: &RouteAnalysisEndpointOptionsSpecification,
        destination: &RouteAnalysisEndpointOptionsSpecification,
        include_return_path: bool,
        use_middleboxes: bool,
        now: SystemTime,
    ) -> RouteAnalysisRecord {
        let source_endpoint =
            self.resolve_route_endpoint(global_network_id, ENDPOINT_SOURCE, source, now);
        let destination_endpoint =
            self.resolve_route_endpoint(global_network_id, ENDPOINT_DESTINATION, destination, now);
        let forward_path = analyze_route_path(&source_endpoint, &destination_endpoint);
        // "The return path is not analyzed if the forward path analysis does not succeed"
        // (`smithy_query query="doc" shape="RouteAnalysis" member="IncludeReturnPath"`), so
        // a forward path that did not reach the destination leaves the return path absent
        // even where the request asked for one.
        let return_path = (include_return_path && route_path_connected(&forward_path))
            .then(|| analyze_route_path(&destination_endpoint, &source_endpoint));
        let id = self.next_id(ROUTE_ANALYSIS_KIND);
        let record = RouteAnalysisRecord {
            id: id.clone(),
            global_network_id: global_network_id.to_string(),
            started_at: now,
            completed_at: now + SETTLE_DELAY,
            source: source_endpoint.options(),
            destination: destination_endpoint.options(),
            include_return_path,
            use_middleboxes,
            forward_path,
            return_path,
        };
        self.route_analyses.insert(id, record.clone());
        record
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
}

/// The ARN -> stored-tag-list dispatch of this service, stated ONCE: every taggable kind
/// paired with the map whose records carry that kind's tags.
///
/// Both halves of the dispatch are generated from this one mapping — the reader
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::tags_of`] behind
/// `ListTagsForResource` (and behind the "does this ARN still name a stored resource"
/// probe of [`ComAmazonawsNetworkmanagerNetworkmanagerState::put_network_resource_metadata`])
/// and the writer [`ComAmazonawsNetworkmanagerNetworkmanagerState::tags_of_mut`] behind
/// `TagResource` / `UntagResource` — so the two cannot answer different records for the
/// same ARN: a kind is added, and the identity re-check of a scoped ARN changed, in one
/// place for both.
macro_rules! taggable_records {
    ($($kind:ident => $records:ident),+ $(,)?) => {
        impl ComAmazonawsNetworkmanagerNetworkmanagerState {
            /// The stored tag list of the resource `arn` names, or `None` when this service
            /// holds no resource under that ARN.
            ///
            /// Tags live on the resource record itself, so every reader of a resource
            /// (`DescribeGlobalNetworks`, `GetSites`, `GetLinks`, `GetDevices`,
            /// `ListTagsForResource`) answers from the one stored list. The record's own ARN
            /// is re-checked against `arn`, so an ARN carrying a stored id under the wrong
            /// parent names no resource.
            pub fn tags_of(&self, arn: &str) -> Option<&Vec<Tag>> {
                let parts = parse_resource_arn(arn)?;
                match parts.kind {
                    $(
                        $kind => self
                            .$records
                            .get(parts.id)
                            .filter(|record| record.arn == arn)
                            .map(|record| &record.tags),
                    )+
                    _ => None,
                }
            }

            /// The stored tag list of the resource `arn` names, for writing: the SAME
            /// mapping [`Self::tags_of`] reads through, so a write and a read resolve the
            /// same record for the same ARN.
            pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
                let parts = parse_resource_arn(arn)?;
                match parts.kind {
                    $(
                        $kind => self
                            .$records
                            .get_mut(parts.id)
                            .filter(|record| record.arn == arn)
                            .map(|record| &mut record.tags),
                    )+
                    _ => None,
                }
            }
        }
    };
}

taggable_records!(
    GLOBAL_NETWORK_KIND => global_networks,
    SITE_KIND => sites,
    LINK_KIND => links,
    DEVICE_KIND => devices,
    CONNECTION_KIND => connections,
    CORE_NETWORK_KIND => core_networks,
    ATTACHMENT_KIND => attachments,
    PEERING_KIND => peerings,
    CONNECT_PEER_KIND => connect_peers,
);

/// The key one network resource is listed under: its resource TYPE, then its id.
///
/// A network-resource listing spans several kinds at once, and the ids of two kinds are
/// drawn from different prefixes, so the type leads the key: the listing is then ordered
/// by type and then by id, every key is unique, and a page token (which names a key
/// offset, see [`paginate`]) can resume inside it.
pub fn network_resource_key(resource_type: &str, resource_id: &str) -> String {
    format!("{resource_type}/{resource_id}")
}

/// The key one relationship is listed under: the ARN it leads FROM, then the ARN it names.
///
/// A relationship has no identity beyond the ordered pair of ARNs it joins, so the pair is
/// its key: the listing is then ordered by the resource a relationship leads from and then
/// by the one it reaches, every key is unique, a pair reached from two stored ties is held
/// once, and a page token (which names a key offset, see [`paginate`]) can resume inside
/// it. The separator is one no ARN carries, so no two pairs can produce the same key.
pub fn relationship_key(from: &str, to: &str) -> String {
    format!("{from}|{to}")
}

/// How many entries the metadata map of one network resource may carry.
///
/// CHOSEN, like [`MAX_RESOURCE_POLICIES`], and for the same reason: an entry of
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::network_resource_metadata`] holds the
/// map a request carried, so without a bound one request could grow this emulator's store
/// without end. AWS publishes no per-resource limit for this map, and the model bounds only
/// each key and each value inside it (a `ConstrainedString` of 0 to 256 characters,
/// `smithy_query shape=NetworkResourceMetadataMap`), so the number here is the emulator's
/// own: the same one as this service's per-resource key/value quota for tags
/// ([`MAX_TAGS_PER_RESOURCE`]), the closest limit AWS does publish.
///
/// A map carrying more entries is refused with `UpdateNetworkResourceMetadata`'s modelled
/// `ValidationException` (HTTP 400) naming [`FIELD_METADATA`], and stores nothing.
pub const MAX_METADATA_ENTRIES: usize = MAX_TAGS_PER_RESOURCE;

/// Check a requested metadata map of `given` entries against [`MAX_METADATA_ENTRIES`].
pub fn check_metadata_entries(given: usize) -> Result<(), ValidationException> {
    if given > MAX_METADATA_ENTRIES {
        return Err(field_rejection(
            FIELD_METADATA,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "a network resource's metadata may carry at most {MAX_METADATA_ENTRIES} entries, but {given} were given"
            ),
        ));
    }
    Ok(())
}

/// The stored metadata of one network resource as the modelled `NetworkResourceMetadataMap`
/// is carried on the wire.
///
/// The store keeps the map ordered by key, like every other map of this state, while the
/// modelled shape is a plain map whose order the protocol does not fix, so this is the one
/// place the two representations meet.
pub fn metadata_shape(
    stored: &BTreeMap<String, String>,
) -> ::std::collections::HashMap<String, String> {
    stored
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The members every network resource of this service carries, whatever its kind: the
/// account that holds it, its type, id and ARN, its tags, and the instant the answer read
/// its definition. The caller fills `Definition` (and, where the kind has one,
/// `CoreNetworkId` and `AwsRegion`) from the record itself.
///
/// `AwsRegion` defaults to absent here and is filled in by the caller for every kind whose
/// own reader names the single Region the resource lives in — the edge location of an
/// attachment, a peering or a Connect peer, all typed `ExternalRegionCode` exactly as
/// `NetworkResource$AwsRegion` is (`smithy_query shape=NetworkResource,Attachment`). A kind
/// whose reader names no single Region keeps the default: the resource is held under the
/// global network itself rather than at one edge (a site, a link, a device, a connection),
/// or it spans several Regions at once and reports them as a LIST (a core network's
/// `Edges`, a Direct Connect gateway attachment's `EdgeLocations`), which a single-valued
/// member cannot carry.
///
/// One member is deliberately left absent, because nothing in the store holds it:
///
/// * `RegisteredGatewayArn` — only a resource reached THROUGH a registered gateway carries
///   one, and every resource of this inventory is one this service holds in its own right.
///   Registering a transit gateway with a global network records that registration alone:
///   the resources reached through the gateway belong to Amazon VPC, of which this service
///   holds no inventory. Associating a customer gateway with a device does not register one
///   either: it ties a gateway the global network already holds to a device of it.
///
/// `Metadata` defaults to absent here too and is filled in by
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::network_resources_of`] for a resource
/// whose ARN the metadata store holds an entry for, whatever its kind.
fn network_resource(
    resource_type: &str,
    resource_id: &str,
    resource_arn: &str,
    tags: &[Tag],
    now: SystemTime,
) -> NetworkResource {
    NetworkResource {
        registered_gateway_arn: None,
        core_network_id: None,
        aws_region: None,
        account_id: Some(::engine::emulator_account_id().to_string()),
        resource_type: Some(resource_type.to_string()),
        resource_id: Some(resource_id.to_string()),
        resource_arn: Some(resource_arn.to_string()),
        definition: None,
        // "The time that the resource definition was retrieved"
        // (`smithy_query shape=NetworkResource`): the definition below is computed from
        // the record as this call reads it, so the instant is this call's.
        definition_timestamp: Some(epoch_seconds(now)),
        tags: Some(tags.to_vec()),
        metadata: None,
    }
}

/// The JSON a `NetworkResource$Definition` carries for one stored resource: the resource's
/// own Describe output, rendered by THIS SERVICE'S OWN restJson1 codec from the very shape
/// that resource's reader answers.
///
/// "Information about the resource, in JSON format. Network Manager gets this information
/// by describing the resource using its Describe API call"
/// (`smithy_query shape=NetworkResource`), so a definition is exactly what `GetSites` /
/// `GetLinks` / `GetDevices` / `GetCoreNetwork` / `GetVpcAttachment` /
/// `GetConnectAttachment` put on the wire for that resource. Rendering through the codec
/// (`JsonCodec::rest_json`, the one the protocol itself serialises responses with) rather
/// than mapping member by member is what keeps the two readers of one resource in step:
/// the member spelling, the `@jsonName` overrides and the wire form of every timestamp are
/// the protocol's, so a model change cannot drift the definition away from the reader.
///
/// The resource's tags appear here as well as in the enclosing `NetworkResource$Tags`,
/// because the definition is the Describe output as it stands and both come from the one
/// stored tag list.
///
/// A value the codec cannot render answers `None`, so the member is absent rather than
/// carrying a definition this service made up.
fn resource_definition(
    schema: &::aws_smithy_schema::Schema,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Option<String> {
    use ::aws_smithy_schema::codec::Codec;
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonCodec::rest_json().create_serializer();
    if let Err(error) = serializer.write_struct(schema, value) {
        ::tracing::warn!(
            %error,
            "the JSON codec could not render a stored resource, so its network-resource definition is absent"
        );
        return None;
    }
    Some(serializer.into_string())
}

/// The resource an ARN of this service names: the ARN resource type (which is also the
/// resource-id prefix this service mints, see [`GLOBAL_NETWORK_KIND`]), the id, and —
/// for a kind that exists only inside a parent — the id of that parent.
#[derive(Debug, Clone, Copy)]
pub struct ArnParts<'a> {
    /// The ARN resource type, e.g. `global-network`.
    pub kind: &'a str,
    /// The resource id inside that type: the LAST path segment of the ARN.
    pub id: &'a str,
    /// The id the ARN names ahead of `id`, for a resource whose name carries its owner
    /// (see [`ComAmazonawsNetworkmanagerNetworkmanagerState::scoped_resource_arn`]);
    /// `None` for an ARN whose resource path is just `<kind>/<id>`.
    pub scope: Option<&'a str>,
}

/// Split a Network Manager resource ARN into its kind and id.
///
/// Every ARN this service mints reads
/// `arn:<partition>:networkmanager::<account>:<kind>/<id>`, or
/// `arn:<partition>:networkmanager::<account>:<kind>/<owner-id>/<id>` for a kind that
/// exists only inside an owning resource (see
/// [`ComAmazonawsNetworkmanagerNetworkmanagerState::scoped_resource_arn`]). The resource's
/// own id is the LAST path segment either way, which is the id a lookup keys on and the
/// `ResourceId` a refusal reports. Answers `None` when the string is not an ARN of this
/// service in one of those forms at all — the caller refuses that as a
/// `ValidationException` on the ARN member, which is a different answer from a well-formed
/// ARN naming a resource that does not exist.
///
/// Only the form is judged here, not which account, partition or region the ARN carries,
/// nor whether the owner it names really owns the resource: an ARN of this service that
/// names another account's resource, carries a region this global service does not use, or
/// names the wrong owner, parses and then matches no stored record, so the caller answers
/// it as NOT FOUND rather than as a malformed request.
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
    let (kind, path) = fields.next()?.split_once('/')?;
    // The resource's own id is the last path segment; anything ahead of it names the
    // resource that owns it.
    let (scope, id) = match path.split_once('/') {
        Some((scope, id)) => (Some(scope), id),
        None => (None, path),
    };
    if kind.is_empty() || id.is_empty() || scope.is_some_and(str::is_empty) || id.contains('/') {
        return None;
    }
    Some(ArnParts { kind, id, scope })
}

/// The `Reason` a `ValidationException` of this service carries when one input MEMBER is
/// at fault — whatever is wrong with the value it carried — and the one it carries when a
/// DOCUMENT this service has to read was not readable as the format it is written in (a
/// policy document that is no JSON: [`validate_resource_policy_document`]). A member whose
/// value this service rejects is the first of the two, not the second, however the value
/// failed. Both are values the modelled enumeration lists
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
/// The input member every operation that stores tags names in a refusal of them: the
/// `Tags` list itself, as [`validate_tags`] and [`tag_problem_rejection`] both report it.
pub const FIELD_TAGS: &str = "Tags";
/// See [`FIELD_RESOURCE_ARN`]: the member carrying the resource metadata an update of one
/// network resource's metadata stores, as [`check_metadata_entries`] reports it.
pub const FIELD_METADATA: &str = "Metadata";

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

/// How many ids ONE id-list filter of a reporting operation may name.
///
/// The bound is the one the member's OWN documentation publishes, so it reaches exactly
/// the id-list filters whose documentation carries the sentence that publishes it: "The IDs
/// of one or more global networks. The maximum is 10." on
/// `DescribeGlobalNetworksRequest$GlobalNetworkIds`, and the same sentence on
/// `GetSitesRequest$SiteIds` ("One or more site IDs. The maximum is 10."),
/// `GetLinksRequest$LinkIds`, `GetDevicesRequest$DeviceIds`,
/// `GetTransitGatewayRegistrationsRequest$TransitGatewayArns` and
/// `GetCustomerGatewayAssociationsRequest$CustomerGatewayArns` ("One or more customer
/// gateway Amazon Resource Names (ARNs). The maximum is 10.": `smithy_query query="doc"
/// shape="<Request>" member="<Member>"`). The modelled list shapes do NOT express it as
/// `@length` — `smithy_query
/// shape=GlobalNetworkIdList,SiteIdList,LinkIdList,DeviceIdList,CustomerGatewayArnList`
/// carries a length on the member STRING only — so the pre-handler constraint walk
/// cannot enforce it and [`check_filter_ids`] does, exactly as the device and link
/// descriptor maxima ([`MAX_DEVICE_DESCRIPTOR_CHARS`]) are enforced in handler code.
///
/// The id-list filters whose documentation publishes NO maximum do not take this bound and
/// get no such refusal invented for them — `GetConnectionsRequest$ConnectionIds` ("One or
/// more connection IDs."), `GetConnectPeerAssociationsRequest$ConnectPeerIds` and
/// `GetTransitGatewayConnectPeerAssociationsRequest$TransitGatewayConnectPeerArns`: each of
/// those listings serves a filter of any length: each selects by ONE pass over the records
/// it holds, testing them against the ids the request named, so what the selection costs is
/// bounded by what the store holds rather than by the length of the caller's list.
pub const MAX_FILTER_IDS: usize = 10;

/// Check one id-list filter against [`MAX_FILTER_IDS`].
///
/// Answers `Ok(())` when the filter names at most that many ids (an absent filter is an
/// empty one, which always passes), otherwise the modelled `ValidationException`
/// (HTTP 400) naming `member` — the input member that carried the list — so a request past
/// the documented maximum is refused before any listing is computed. Shared by every
/// reporting operation whose id-list filter member PUBLISHES that maximum (see
/// [`MAX_FILTER_IDS`] for which do and which do not), so one rule reaches all of them.
pub fn check_filter_ids(member: &str, ids: &[String]) -> Result<(), ValidationException> {
    check_filter_list(member, ids.len(), "name", "ids")
}

/// Check one VALUE-match filter of a reporting operation against [`MAX_FILTER_IDS`].
///
/// The sibling of [`check_filter_ids`] for a filter that selects on a VALUE rather than on
/// a resource id: the match lists of a listing of routing information, and each value list
/// of its next-hop filter map. Those lists are modelled as `ConstrainedStringList` /
/// `FilterValues`, which constrain the member STRING only and carry no length of their own
/// (`smithy_query shape=ConstrainedStringList,FilterValues`), so — exactly as for the id
/// lists above — the pre-handler constraint walk cannot bound them and the handler does.
///
/// The bound MATTERS beyond tidiness: selecting a listing compares every entry it holds
/// against every value a filter carries, so the work is the product of the two, and the
/// count is what caps the caller's factor of it — a value-match filter selects by COMPARING
/// values rather than by looking an id up, so there is no keyed lookup to serve it with.
/// It is the same number this service allows a documented id-list filter — but unlike that
/// one it is CHOSEN rather than documented: no match list of this service carries a
/// published maximum, so a request real AWS would plausibly accept is refused here past it.
pub fn check_filter_values(member: &str, values: &[String]) -> Result<(), ValidationException> {
    check_filter_list(member, values.len(), "carry", "values")
}

/// The shared body of the two above: ONE statement of the bound and of the refusal a list
/// past it carries, so an id-list filter and a value-match filter cannot drift apart in
/// either. `verb` and `things` are how the refusal reads about the list at fault ("name …
/// ids", "carry … values"); `given` is how long that list was.
fn check_filter_list(
    member: &str,
    given: usize,
    verb: &str,
    things: &str,
) -> Result<(), ValidationException> {
    if given > MAX_FILTER_IDS {
        return Err(field_rejection(
            member,
            REASON_FIELD_VALIDATION_FAILED,
            format!(
                "{member} may {verb} at most {MAX_FILTER_IDS} {things}, but {given} were given"
            ),
        ));
    }
    Ok(())
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

/// Whether every character of `text` is a lowercase hex digit — the alphabet every id the
/// cross-service ARN patterns of this model spell out is drawn from (`[0-9a-f]` in
/// `DirectConnectGatewayArn`'s and `VpcArn`'s patterns).
fn is_lowercase_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// What the Region segment of a cross-service ARN may carry, which is the one thing the
/// modelled patterns of the two forms below disagree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnRegion {
    /// The segment must be EMPTY, as `DirectConnectGatewayArn`'s pattern has it
    /// (`:directconnect::`).
    Absent,
    /// The segment may carry any Region, or none, as the EC2 patterns have it
    /// (`:ec2:[^:]{0,63}:`).
    Any,
}

/// Split the ARN of a resource of ANOTHER service into its kind and id, or `None` when the
/// string is not one in the form asked for.
///
/// Every cross-service ARN this model types spells one skeleton —
/// `arn:<partition>:<namespace>:<region>:<account>:<kind>/<id>`, with the partition 1 to 63
/// characters and the Region and account at most 63
/// (`^arn:[^:]{1,63}:directconnect::[^:]{0,63}:dx-gateway\/…$`, `smithy_query
/// shape=DirectConnectGatewayArn`; `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/…$`,
/// `smithy_query shape=VpcArn`) — so ONE walk serves every such form and two of them cannot
/// drift apart. `region` is what that form's pattern says about the Region segment, `kind`
/// the resource type it names and `is_id` the shape of the id inside it.
///
/// Which account, partition or Region the ARN carries is not judged beyond those widths,
/// exactly as [`parse_resource_arn`] does not judge them for an ARN of this service: the
/// resource belongs to another service, so whether it EXISTS cannot be known here.
fn parse_foreign_arn<'a>(
    arn: &'a str,
    namespace: &str,
    region: ArnRegion,
    kind: &str,
    is_id: fn(&str) -> bool,
) -> Option<ArnParts<'a>> {
    /// The widest partition, Region and account those patterns admit.
    const MAX_SEGMENT: usize = 63;

    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if partition.is_empty() || partition.len() > MAX_SEGMENT {
        return None;
    }
    if fields.next()? != namespace {
        return None;
    }
    let found_region = fields.next()?;
    if found_region.len() > MAX_SEGMENT || (region == ArnRegion::Absent && !found_region.is_empty())
    {
        return None;
    }
    if fields.next()?.len() > MAX_SEGMENT {
        return None;
    }
    let (found_kind, id) = fields.next()?.split_once('/')?;
    if found_kind != kind || !is_id(id) {
        return None;
    }
    // A resource of another service is identified account-wide here: none of the forms this
    // walk serves names an owning resource ahead of the id, so the resource path is just
    // `<kind>/<id>` and the scope [`ArnParts`] carries is `None`.
    Some(ArnParts {
        kind: found_kind,
        id,
        scope: None,
    })
}

/// The ARN namespace of the resources of ANOTHER service this service's Cloud WAN
/// operations take as INPUT: a transit gateway, one of its route tables and a managed
/// prefix list are all Amazon VPC (EC2) resources, so their ARNs read
/// `arn:<partition>:ec2:<region>:<account>:<type>/<id>` — the form the members' own
/// documentation gives as an example ("arn:aws:ec2:us-west-2:123456789012:transit-gateway-route-table/tgw-rtb-9876543210123456",
/// `smithy_query query="doc" shape="CreateTransitGatewayRouteTableAttachmentRequest"
/// member="TransitGatewayRouteTableArn"`). This service mints nothing for them.
const EC2_NAMESPACE: &str = "ec2";
/// The ARN resource type of a managed prefix list, and the prefix of its id — the `pl-` of
/// `prefix-list/pl-0123456789abcdef1`.
pub const PREFIX_LIST_KIND: &str = "prefix-list";
/// See [`PREFIX_LIST_KIND`].
const PREFIX_LIST_ID_PREFIX: &str = "pl-";

/// Whether `id` reads as an EC2 resource id of the type `prefix` names: that prefix plus
/// the 8 to 17 lowercase hex digits every EC2 resource id the model DOES type carries
/// (`vpc-[0-9a-f]{8,17}` in `VpcArn`'s pattern, `vpn-[0-9a-f]{8,17}` in
/// `VpnConnectionArn`'s).
fn is_ec2_hex_id(id: &str, prefix: &str) -> bool {
    /// The width an EC2 resource id's hex suffix may have, from those patterns.
    const WIDTHS: ::std::ops::RangeInclusive<usize> = 8..=17;

    match id.strip_prefix(prefix) {
        Some(digits) => WIDTHS.contains(&digits.len()) && is_lowercase_hex(digits),
        None => false,
    }
}

/// Whether `id` reads as a managed prefix list id (see [`is_ec2_hex_id`]).
fn is_prefix_list_id(id: &str) -> bool {
    is_ec2_hex_id(id, PREFIX_LIST_ID_PREFIX)
}

/// Split a managed prefix list ARN into its kind and id, or `None` when the string is not
/// one at all — the caller refuses that as a `ValidationException` on the ARN member
/// ([`malformed_prefix_list_arn`]).
///
/// `PrefixListArn` is the one cross-service ARN this model leaves free-form (len 0..500,
/// pattern `^[\s\S]*$`: `smithy_query shape=PrefixListArn`), so the form is judged here
/// rather than by the pre-handler constraint walk. Every EC2 ARN the model DOES type spells
/// the skeleton [`parse_foreign_arn`] walks — `VpcArn` carries
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$` (`smithy_query
/// shape=VpcArn`) — and a prefix list ARN is that skeleton with the `prefix-list` resource
/// type and a `pl-` id ([`is_prefix_list_id`]).
///
/// Whether the prefix list EXISTS is not judged: it belongs to Amazon VPC, which this
/// service holds no record of, so a well-formed ARN is stored as the association's subject.
pub fn parse_prefix_list_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_foreign_arn(
        arn,
        EC2_NAMESPACE,
        ArnRegion::Any,
        PREFIX_LIST_KIND,
        is_prefix_list_id,
    )
}

/// The modelled `ValidationException` (HTTP 400) a prefix-list association operation
/// answers when the string in its member `member` is not a managed prefix list ARN at all
/// (see [`parse_prefix_list_arn`]).
///
/// Like [`malformed_resource_arn`]: a string that names no prefix list is a FIELD failure
/// on the member that carried it, a different answer from a well-formed ARN that names no
/// association this core network holds.
pub fn malformed_prefix_list_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not a managed prefix list ARN"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{EC2_NAMESPACE}:<region>:<account>:{PREFIX_LIST_KIND}/{PREFIX_LIST_ID_PREFIX}<8 to 17 hex digits>"
            ),
        }]),
    }
}

/// The ARN resource type of a CUSTOMER GATEWAY, and the prefix of its id — the `cgw-` of
/// `customer-gateway/cgw-0123456789abcdef1`. An Amazon VPC (EC2) resource, like the managed
/// prefix list above, and one of [`NETWORK_RESOURCE_TYPES`].
pub const CUSTOMER_GATEWAY_KIND: &str = "customer-gateway";
/// See [`CUSTOMER_GATEWAY_KIND`].
const CUSTOMER_GATEWAY_ID_PREFIX: &str = "cgw-";

/// Whether `id` reads as a customer gateway id (see [`is_ec2_hex_id`]).
fn is_customer_gateway_id(id: &str) -> bool {
    is_ec2_hex_id(id, CUSTOMER_GATEWAY_ID_PREFIX)
}

/// Split a customer gateway ARN into its kind and id, or `None` when the string is not one
/// at all — the caller refuses that as a `ValidationException` on the ARN member
/// ([`malformed_customer_gateway_arn`]).
///
/// `CustomerGatewayArn` is modelled free-form (len 0..500, pattern `^[\s\S]*$`:
/// `smithy_query shape=CustomerGatewayArn`), like `PrefixListArn`, so the form is judged
/// here rather than by the pre-handler constraint walk. A customer gateway is an Amazon VPC
/// resource, so its ARN is the EC2 skeleton [`parse_foreign_arn`] walks — the one
/// `VpnConnectionArn` spells out, `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`
/// (`smithy_query shape=VpnConnectionArn`) — with the `customer-gateway` resource type and
/// a `cgw-` id.
///
/// Whether the gateway EXISTS is not judged: it belongs to Amazon VPC, which this service
/// holds no record of, so a well-formed ARN is stored as the association's subject.
pub fn parse_customer_gateway_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_foreign_arn(
        arn,
        EC2_NAMESPACE,
        ArnRegion::Any,
        CUSTOMER_GATEWAY_KIND,
        is_customer_gateway_id,
    )
}

/// The modelled `ValidationException` (HTTP 400) a customer gateway association operation
/// answers when the string in its member `member` is not a customer gateway ARN at all
/// (see [`parse_customer_gateway_arn`]).
///
/// Like [`malformed_prefix_list_arn`]: a string that names no customer gateway is a FIELD
/// failure on the member that carried it, a different answer from a well-formed ARN that
/// names no association this global network holds.
pub fn malformed_customer_gateway_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not a customer gateway ARN"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{EC2_NAMESPACE}:<region>:<account>:{CUSTOMER_GATEWAY_KIND}/{CUSTOMER_GATEWAY_ID_PREFIX}<8 to 17 hex digits>"
            ),
        }]),
    }
}

/// The ARN namespace and resource type of the resource a resource policy of this service
/// is attached to: a Direct Connect gateway.
///
/// A resource policy is attached to a resource that can CARRY one, and no resource Network
/// Manager itself owns can: a recording of `PutResourcePolicy` against a core network — the
/// central resource of this service — is refused for every well-formed policy document.
/// The model agrees that the subject of these three operations is not a resource of this
/// service: `TagResource` and `ListTagsForResource` carry the very same `ResourceArn`
/// member and model `ResourceNotFoundException` (HTTP 404) for an ARN naming nothing,
/// while `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy` model no
/// not-found failure at all (`smithy_query query="operation"
/// operation="PutResourcePolicy,TagResource"`), so they never resolve the ARN against this
/// service's resources.
///
/// The one resource of ANOTHER service this API surface joins to a core network — across
/// accounts, so it needs a grant from its own owner — is the Direct Connect gateway
/// `CreateDirectConnectGatewayAttachment` takes, and its ARN is the one cross-service ARN
/// the model types: `DirectConnectGatewayArn` carries
/// `pattern ^arn:[^:]{1,63}:directconnect::[^:]{0,63}:dx-gateway\/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}$`
/// (`smithy_query shape=DirectConnectGatewayArn`). That form is what
/// [`parse_direct_connect_gateway_arn`] accepts, and an ARN of any other resource type is
/// refused by `PutResourcePolicy` with [`resource_not_policy_attachable`].
const DIRECT_CONNECT_NAMESPACE: &str = "directconnect";
/// See [`DIRECT_CONNECT_NAMESPACE`].
pub const DIRECT_CONNECT_GATEWAY_KIND: &str = "dx-gateway";

/// Whether `id` reads as a Direct Connect gateway id: the five lowercase-hex groups of
/// 8-4-4-4-12 digits the modelled `DirectConnectGatewayArn` pattern spells out (see
/// [`DIRECT_CONNECT_NAMESPACE`]).
fn is_direct_connect_gateway_id(id: &str) -> bool {
    /// The widths of the groups, in order, from that pattern.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

    let mut groups = id.split('-');
    for width in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !is_lowercase_hex(group) {
            return false;
        }
    }
    groups.next().is_none()
}

/// Split a Direct Connect gateway ARN into its kind and id, or `None` when the string is
/// not one in the form the model types (see [`DIRECT_CONNECT_NAMESPACE`]).
///
/// The region segment must be EMPTY and the account segment may be, exactly as that
/// pattern has them; which account owns the gateway is not judged here, since the point of
/// a resource policy is that the gateway belongs to another account.
pub fn parse_direct_connect_gateway_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_foreign_arn(
        arn,
        DIRECT_CONNECT_NAMESPACE,
        ArnRegion::Absent,
        DIRECT_CONNECT_GATEWAY_KIND,
        is_direct_connect_gateway_id,
    )
}

/// The resource an ARN names when that resource may CARRY a resource policy, or `None`
/// when it may not — see [`DIRECT_CONNECT_NAMESPACE`] for which resource types may.
///
/// `PutResourcePolicy` attaches a policy only to such a resource; every other ARN, this
/// service's own resources included, is refused with [`resource_not_policy_attachable`].
/// This is the ONE place that class is decided, so a second attachable resource type —
/// should one be recorded as carrying a policy — is added here as another arm and reaches
/// all three operations at once.
pub fn parse_policy_attachable_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_direct_connect_gateway_arn(arn)
}

/// The resource an ARN names when the resource-policy operations ADDRESS it at all: one of
/// this service's own resources (which carries no policy, so it is reported as holding
/// none) or a policy-attachable resource of another service.
///
/// `GetResourcePolicy` and `DeleteResourcePolicy` answer for either — a resource that
/// cannot carry a policy simply holds none — so only a string that names neither is
/// refused, with [`malformed_policy_resource_arn`].
pub fn parse_policy_resource_arn(arn: &str) -> Option<ArnParts<'_>> {
    parse_resource_arn(arn).or_else(|| parse_policy_attachable_arn(arn))
}

/// The modelled `ValidationException` (HTTP 400) the resource-policy operations answer when
/// the string in `member` names no resource they address at all (see
/// [`parse_policy_resource_arn`]).
///
/// Like [`malformed_resource_arn`], but naming BOTH forms these three operations read: an
/// ARN of this service, and the policy-attachable form of another service's resource — a
/// caller whose gateway ARN is slightly wrong is told the form it missed rather than the
/// one it was not reaching for.
pub fn malformed_policy_resource_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not an ARN of a resource a resource policy is read or written for"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{ARN_NAMESPACE}::<account>:<type>/<id>, or arn:<partition>:{DIRECT_CONNECT_NAMESPACE}::<account>:{DIRECT_CONNECT_GATEWAY_KIND}/<id> for a resource that can carry a policy"
            ),
        }]),
    }
}

/// The message a refusal of a resource that cannot carry a resource policy carries.
///
/// A fixed, request-INDEPENDENT constant, like [`RESOURCE_NOT_FOUND_MESSAGE`] and for the
/// same reason: a recording of `PutResourcePolicy` against a resource of this service
/// answers the identical string for every well-formed policy document put at it — the
/// document's principal, its actions and whether the resource already holds a policy of
/// its own change nothing — so the wording carries no per-request detail. The offending
/// member travels in `Fields` instead, where [`resource_not_policy_attachable`] names it.
///
/// What this refusal PINS is the HTTP 400, the `ValidationException` code and the
/// `FieldValidationFailed` reason; the wording is this service's own text, so changing it
/// is not a behaviour change.
pub const INVALID_RESOURCE_POLICY_MESSAGE: &str = "Invalid policy.";

/// The modelled `ValidationException` (HTTP 400) `PutResourcePolicy` answers when the ARN
/// in `member` names a well-formed resource that cannot carry a resource policy — this
/// service's own resources among them (see [`DIRECT_CONNECT_NAMESPACE`]).
///
/// A FIELD failure on the member that carried the ARN, like [`malformed_resource_arn`]:
/// nothing is stored, so the resource keeps reporting no policy afterwards.
pub fn resource_not_policy_attachable(member: &str) -> ValidationException {
    ValidationException {
        message: INVALID_RESOURCE_POLICY_MESSAGE.to_string(),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "must read arn:<partition>:{DIRECT_CONNECT_NAMESPACE}::<account>:{DIRECT_CONNECT_GATEWAY_KIND}/<id>"
            ),
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

/// The modelled `ValidationException` (HTTP 400) a [`TagProblem`] becomes on an operation
/// that models NO `ServiceQuotaExceededException`.
///
/// The attachment creates are such operations (`smithy_query query="operation"
/// operation="CreateVpcAttachment,CreateConnectAttachment"` models AccessDenied, Conflict,
/// InternalServer, ResourceNotFound, Throttling and Validation only), so a request past
/// the per-resource tag quota is refused through the one error class they do model,
/// carrying [`VALIDATION_REASON_OTHER`] — the `Tags` member satisfies every modelled
/// constraint, it is the resulting tag COUNT that the request cannot have.
pub fn tag_problem_rejection(problem: TagProblem) -> ValidationException {
    match problem {
        TagProblem::Invalid(rejection) => rejection.to_exception(),
        TagProblem::TooMany(resulting) => field_rejection(
            FIELD_TAGS,
            VALIDATION_REASON_OTHER,
            too_many_tags_message(resulting),
        ),
    }
}

/// The message a refusal past the per-resource tag quota carries, shared by every
/// operation that stores tags: `resulting` is how many tags the resource would carry if
/// the request were accepted.
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "a resource may carry at most {MAX_TAGS_PER_RESOURCE} tags, but the request would leave {resulting}"
    )
}

/// The modelled `ValidationException` (HTTP 400) a create answers when the core network it
/// must hang off configures no edge in the Region the resource it names lives in.
///
/// A peering and an attachment both sit at a core network EDGE, and a core network's edges
/// are the ones its policy document configures
/// ([`PolicyTopology::has_edge_location`]), so a request naming a Region that document
/// does not configure is refused rather than attached to an edge that does not exist. The
/// `Reason` is [`VALIDATION_REASON_OTHER`]: the member satisfies every modelled
/// constraint, it is the stored policy that configures no such edge.
pub fn no_such_edge(
    member: &str,
    core_network_id: &str,
    edge_location: &str,
) -> ValidationException {
    field_rejection(
        member,
        VALIDATION_REASON_OTHER,
        format!("core network {core_network_id} configures no edge location in {edge_location}"),
    )
}

/// The modelled `ValidationException` (HTTP 400) a request answers when the core network it
/// addresses declares no segment of the name it carries.
///
/// The sibling of [`no_such_edge`] for the other half of a core network's topology: a
/// segment of a core network is one its policy document declares
/// ([`PolicyTopology::has_segment`]), so a request naming any other segment addresses
/// nothing. The `Reason` is [`VALIDATION_REASON_OTHER`] for the same reason: the member
/// satisfies every modelled constraint, it is the stored policy that declares no such
/// segment.
pub fn no_such_segment(
    member: &str,
    core_network_id: &str,
    segment_name: &str,
) -> ValidationException {
    field_rejection(
        member,
        VALIDATION_REASON_OTHER,
        format!("core network {core_network_id} declares no segment {segment_name}"),
    )
}

/// Check a tag set a request asks to APPLY (`TagResource`, and the `Tags` of a create).
///
/// Refuses an empty or over-long key, an over-long value, and any key carrying the
/// AWS-reserved prefix. A key repeated inside one request is NOT an error: tag keys are
/// unique per resource, so the last value for a key wins — through [`merge_tags`] on the
/// `TagResource` path and through [`tags_to_store`] for the `Tags` of a create. Every
/// entry is checked, including one a repeat later replaces, so a request carrying a tag
/// that breaks a rule is refused whether or not that entry would survive the merge.
pub fn validate_tags(tags: &[Tag]) -> Result<(), TagRejection> {
    const MEMBER: &str = FIELD_TAGS;
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

/// The tag list a resource is STORED with for the `Tags` a create carried.
///
/// A resource's tags are a key-to-value map, so a key one request names more than once is
/// stored ONCE: the first occurrence keeps its position and the last value given for that
/// key wins — the very merge [`merge_tags`] performs for a `TagResource`, and the one
/// [`tag_count_after_merge`] already grades the create's quota by. Without it a create
/// could store two entries for one key, which no later write could repair.
pub fn tags_to_store(requested: Option<Vec<Tag>>) -> Vec<Tag> {
    let mut stored = Vec::new();
    merge_tags(&mut stored, requested.unwrap_or_default());
    stored
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
/// See [`POLICY_VERSION`]: the keys of an `attachment-policies[]` rule, the part of the
/// document that decides what happens to an attachment created on the core network. A rule
/// carries a `rule-number`, the `conditions` an attachment is matched against (combined by
/// its `condition-logic`) and the `action` taken on the attachment it claims.
const POLICY_ATTACHMENT_POLICIES: &str = "attachment-policies";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_RULE_NUMBER: &str = "rule-number";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_CONDITION_LOGIC: &str = "condition-logic";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_CONDITIONS: &str = "conditions";
/// See [`POLICY_ATTACHMENT_POLICIES`].
const POLICY_ACTION: &str = "action";
/// See [`POLICY_ATTACHMENT_POLICIES`]: the keys of one `conditions[]` entry.
const POLICY_TYPE: &str = "type";
/// See [`POLICY_TYPE`].
const POLICY_OPERATOR: &str = "operator";
/// See [`POLICY_TYPE`].
const POLICY_KEY: &str = "key";
/// See [`POLICY_TYPE`].
const POLICY_VALUE: &str = "value";
/// See [`POLICY_ATTACHMENT_POLICIES`]: the keys of a rule's `action`.
const POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE: &str = "require-attachment-acceptance";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_ASSOCIATION_METHOD: &str = "association-method";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_SEGMENT: &str = "segment";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_TAG_VALUE_OF_KEY: &str = "tag-value-of-key";
/// See [`POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE`].
const POLICY_ADD_TO_NETWORK_FUNCTION_GROUP: &str = "add-to-network-function-group";

/// See [`POLICY_VERSION`]: the keys of a `segment-actions[]` entry, the part of the document
/// that says what one segment's routes reach. Every entry names the `action` it takes and
/// the `segment` it acts on; a `share` action lists the segments that segment's routes are
/// shared with in `share-with`, and a service-insertion action names the network function
/// groups the segment's traffic goes through under `via.network-function-groups`.
const POLICY_SEGMENT_ACTIONS: &str = "segment-actions";
/// See [`POLICY_SEGMENT_ACTIONS`].
const POLICY_SHARE_WITH: &str = "share-with";
/// See [`POLICY_SEGMENT_ACTIONS`]: the key of the object form of `share-with`, which shares
/// with every other segment but the ones it lists.
const POLICY_EXCEPT: &str = "except";
/// See [`POLICY_SEGMENT_ACTIONS`].
const POLICY_VIA: &str = "via";
/// See [`POLICY_SEGMENT_ACTIONS`]: the keys of a `create-route` entry, which declares the
/// STATIC routes of the segment it acts on — the prefixes those routes carry, and the
/// attachments of the segment they are reached through (or the string
/// [`POLICY_DESTINATION_BLACKHOLE`], which reaches nothing).
const POLICY_DESTINATION_CIDR_BLOCKS: &str = "destination-cidr-blocks";
/// See [`POLICY_DESTINATION_CIDR_BLOCKS`].
const POLICY_DESTINATIONS: &str = "destinations";
/// See [`POLICY_DESTINATION_CIDR_BLOCKS`]: the `destinations` entry that names no
/// attachment, so the route it declares is reached through nothing.
pub const POLICY_DESTINATION_BLACKHOLE: &str = "blackhole";

/// The `segment-actions[].action` values this slice derives the reported topology from:
/// `share` decides which segments a segment's routes reach, and the two service-insertion
/// actions — the values the model enumerates as `SegmentActionServiceInsertion`, mirrored
/// in `ops::types::SEGMENT_ACTION_SERVICE_INSERTION_VALUES` — decide which segments a
/// network function group serves. Every other action of the published schema says nothing
/// about either and is passed over.
const POLICY_ACTION_SHARE: &str = "share";
/// See [`POLICY_ACTION_SHARE`].
const POLICY_ACTION_SEND_VIA: &str = "send-via";
/// See [`POLICY_ACTION_SHARE`].
const POLICY_ACTION_SEND_TO: &str = "send-to";
/// See [`POLICY_ACTION_SHARE`]: `create-route` declares the static routes a segment holds,
/// which is what the routing information of that segment reports beside what its BGP
/// peerings carry ([`PolicyTopology::static_routes_of`]).
const POLICY_ACTION_CREATE_ROUTE: &str = "create-route";

/// The `share-with` spelling that means every other segment of the core network.
const POLICY_SHARE_WITH_ALL: &str = "*";

/// The `condition-logic` value that makes a rule claim an attachment ANY one of its
/// conditions holds for; the other published value, `and`, is the default and is what every
/// other string (and an absent member) means here.
const POLICY_LOGIC_OR: &str = "or";

/// The `association-method` values a rule's action may carry: `constant` names the segment
/// in `segment`, `tag` takes the segment name from the value of the attachment's
/// `tag-value-of-key` tag (Cloud WAN "Core network policy" documentation).
const POLICY_ASSOCIATION_CONSTANT: &str = "constant";
/// See [`POLICY_ASSOCIATION_CONSTANT`].
const POLICY_ASSOCIATION_TAG: &str = "tag";

/// The `conditions[].type` values an attachment-policy rule matches an attachment against,
/// as the published policy schema lists them: `any` claims every attachment, and the rest
/// each compare one property of the attachment with the condition's `value` under its
/// `operator` — the account that owns it, the Region of its edge location, its
/// `AttachmentType`, the ARN of the resource it attaches, or one of its tags.
const CONDITION_ANY: &str = "any";
/// See [`CONDITION_ANY`].
const CONDITION_ACCOUNT_ID: &str = "account-id";
/// See [`CONDITION_ANY`].
const CONDITION_REGION: &str = "region";
/// See [`CONDITION_ANY`].
const CONDITION_ATTACHMENT_TYPE: &str = "attachment-type";
/// See [`CONDITION_ANY`].
const CONDITION_RESOURCE_ID: &str = "resource-id";
/// See [`CONDITION_ANY`].
const CONDITION_TAG_EXISTS: &str = "tag-exists";
/// See [`CONDITION_ANY`].
const CONDITION_TAG_VALUE: &str = "tag-value";

/// The `conditions[].operator` values the schema lists, each comparing the attachment's
/// property with the condition's `value`.
const OPERATOR_EQUALS: &str = "equals";
/// See [`OPERATOR_EQUALS`].
const OPERATOR_NOT_EQUALS: &str = "not-equals";
/// See [`OPERATOR_EQUALS`].
const OPERATOR_CONTAINS: &str = "contains";
/// See [`OPERATOR_EQUALS`].
const OPERATOR_BEGINS_WITH: &str = "begins-with";

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
/// See [`MAX_POLICY_EDGE_LOCATIONS`]: how many `attachment-policies` rules one document may
/// carry, and how many conditions one of those rules may carry. CHOSEN for the same reason
/// — every rule of the policy in force is walked, in order, for each attachment created, so
/// the work one create costs is bounded by their product.
pub const MAX_POLICY_ATTACHMENT_POLICIES: usize = 100;
/// See [`MAX_POLICY_ATTACHMENT_POLICIES`].
pub const MAX_POLICY_CONDITIONS_PER_RULE: usize = 20;
/// See [`MAX_POLICY_EDGE_LOCATIONS`]: how many `segment-actions` entries one document may
/// carry. CHOSEN for the same reason — every entry is walked to derive which segments a
/// segment's routes reach and which segments a network function group serves, so a document
/// past this cap is refused rather than silently applied in part. The name lists INSIDE one
/// entry are capped at [`MAX_POLICY_SEGMENTS`] /
/// [`MAX_POLICY_NETWORK_FUNCTION_GROUPS`] — the counts of the things they may name — so one
/// entry's kept names, and the work of reporting them, are bounded by those caps too.
pub const MAX_POLICY_SEGMENT_ACTIONS: usize = 100;

/// See [`MAX_POLICY_EDGE_LOCATIONS`]: how many prefixes one `create-route` entry may
/// declare, and how many destinations it may name. CHOSEN for the same reason — the routes
/// one segment holds are the PRODUCT of the three counts, and every one of them is
/// materialised to report that segment's routing information, so the work one listing costs
/// and the answer it can grow to are bounded by
/// `MAX_POLICY_SEGMENT_ACTIONS * MAX_POLICY_ROUTE_PREFIXES * MAX_POLICY_ROUTE_DESTINATIONS`
/// routes rather than by the document's length. The numbers are generous against what an
/// entry can usefully name: a destination names one attachment of the core network, or the
/// blackhole, and a prefix is reached once per destination however often the document
/// repeats it. AWS publishes no quota these follow, so a document real Cloud WAN would
/// accept is refused here past either number — the same trade every other policy cap of
/// this slice makes ([`MAX_POLICY_EDGE_LOCATIONS`]).
pub const MAX_POLICY_ROUTE_PREFIXES: usize = 50;
/// See [`MAX_POLICY_ROUTE_PREFIXES`].
pub const MAX_POLICY_ROUTE_DESTINATIONS: usize = 10;

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

/// Check one edge-location string of a policy document, at the JSON path `path`: a location
/// is reported through `ExternalRegionCode` (len 1..63, `smithy_query
/// shape=ExternalRegionCode`), so one outside that length is refused. The same rule holds
/// wherever a document names a location — the configuration's own `edge-locations`, and the
/// list a segment or network function group restricts itself to — so both walks use this.
fn check_policy_location(path: &str, location: &str, errors: &mut Vec<CoreNetworkPolicyError>) {
    let length = location.chars().count();
    if length == 0 || length > MAX_POLICY_LOCATION_CHARS {
        errors.push(policy_error(
            POLICY_ERROR_INVALID_VALUE,
            format!(
                "a {POLICY_LOCATION:?} must be 1 to {MAX_POLICY_LOCATION_CHARS} characters, but {length} were given"
            ),
            path,
        ));
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
            Some(location) => check_policy_location(&path, location, &mut errors),
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
            // An entry may restrict itself to some of the core network's edge locations,
            // and THAT list is what the entry is expanded over — once per edge location it
            // names, in the derived topology and in the change set of the policy version.
            // So it is capped exactly like the configuration's own edge-location list, in
            // entries and in characters: without the cap a document carrying a bounded
            // number of entries could still name an unbounded number of locations in each
            // and expand into an unbounded topology.
            let path = format!("$.{key}[{index}].{POLICY_EDGE_LOCATIONS}");
            let locations = entry
                .get(POLICY_EDGE_LOCATIONS)
                .and_then(::serde_json::Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            if locations.len() > MAX_POLICY_EDGE_LOCATIONS {
                errors.push(policy_error(
                    POLICY_ERROR_TOO_MANY_ENTRIES,
                    format!(
                        "{path} carries {} entries, but at most {MAX_POLICY_EDGE_LOCATIONS} are allowed",
                        locations.len()
                    ),
                    &path,
                ));
            }
            // Enumerated BEFORE the entries are read, so the index a refusal reports is the
            // entry's own position in the document, and an entry that is not a string is
            // reported rather than passed over — the same order the configuration's own
            // edge-location walk above uses.
            for (position, location) in locations.iter().enumerate().take(MAX_POLICY_EDGE_LOCATIONS)
            {
                let path = format!("{path}[{position}]");
                let Some(location) = location.as_str() else {
                    errors.push(policy_error(
                        POLICY_ERROR_INVALID_TYPE,
                        format!(
                            "every entry of {POLICY_EDGE_LOCATIONS:?} must be a {POLICY_LOCATION:?} string"
                        ),
                        &path,
                    ));
                    continue;
                };
                check_policy_location(&path, location, &mut errors);
            }
        }
    }

    // `attachment-policies` decides what becomes of every attachment created on this core
    // network, so a document carrying one is judged like the lists above: it has to be an
    // array, each rule has to carry the `rule-number` that orders it and the `action` that
    // says what it does, and both the rule count and each rule's condition count are capped
    // so the walk one attachment create costs stays bounded.
    match root.get(POLICY_ATTACHMENT_POLICIES) {
        None => {}
        Some(::serde_json::Value::Array(rules)) => {
            if rules.len() > MAX_POLICY_ATTACHMENT_POLICIES {
                errors.push(policy_error(
                    POLICY_ERROR_TOO_MANY_ENTRIES,
                    format!(
                        "$.{POLICY_ATTACHMENT_POLICIES} carries {} entries, but at most {MAX_POLICY_ATTACHMENT_POLICIES} are allowed",
                        rules.len()
                    ),
                    &format!("$.{POLICY_ATTACHMENT_POLICIES}"),
                ));
            }
            for (index, rule) in rules
                .iter()
                .enumerate()
                .take(MAX_POLICY_ATTACHMENT_POLICIES)
            {
                if rule
                    .get(POLICY_RULE_NUMBER)
                    .and_then(::serde_json::Value::as_i64)
                    .is_none()
                {
                    errors.push(policy_error(
                        POLICY_ERROR_MISSING_FIELD,
                        format!("every entry of {POLICY_ATTACHMENT_POLICIES:?} must carry a {POLICY_RULE_NUMBER:?} number"),
                        &format!("$.{POLICY_ATTACHMENT_POLICIES}[{index}].{POLICY_RULE_NUMBER}"),
                    ));
                }
                if rule
                    .get(POLICY_ACTION)
                    .and_then(::serde_json::Value::as_object)
                    .is_none()
                {
                    errors.push(policy_error(
                        POLICY_ERROR_MISSING_FIELD,
                        format!("every entry of {POLICY_ATTACHMENT_POLICIES:?} must carry an {POLICY_ACTION:?} object"),
                        &format!("$.{POLICY_ATTACHMENT_POLICIES}[{index}].{POLICY_ACTION}"),
                    ));
                }
                let conditions = rule
                    .get(POLICY_CONDITIONS)
                    .and_then(::serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                if conditions > MAX_POLICY_CONDITIONS_PER_RULE {
                    errors.push(policy_error(
                        POLICY_ERROR_TOO_MANY_ENTRIES,
                        format!(
                            "a rule's {POLICY_CONDITIONS:?} carries {conditions} entries, but at most {MAX_POLICY_CONDITIONS_PER_RULE} are allowed"
                        ),
                        &format!("$.{POLICY_ATTACHMENT_POLICIES}[{index}].{POLICY_CONDITIONS}"),
                    ));
                }
            }
        }
        Some(_) => errors.push(policy_error(
            POLICY_ERROR_INVALID_TYPE,
            format!("{POLICY_ATTACHMENT_POLICIES:?} must be an array"),
            &format!("$.{POLICY_ATTACHMENT_POLICIES}"),
        )),
    }

    // `segment-actions` is the part of the document that says what each segment's routes
    // reach, which the reported topology is derived from, so a document carrying one is
    // judged like the lists above: it has to be an array, and both its entry count and the
    // name lists inside one entry are capped so that derivation stays bounded — a name list
    // longer than the segment or group list it may name can only be repeating names.
    match root.get(POLICY_SEGMENT_ACTIONS) {
        None => {}
        Some(::serde_json::Value::Array(actions)) => {
            if actions.len() > MAX_POLICY_SEGMENT_ACTIONS {
                errors.push(policy_error(
                    POLICY_ERROR_TOO_MANY_ENTRIES,
                    format!(
                        "$.{POLICY_SEGMENT_ACTIONS} carries {} entries, but at most {MAX_POLICY_SEGMENT_ACTIONS} are allowed",
                        actions.len()
                    ),
                    &format!("$.{POLICY_SEGMENT_ACTIONS}"),
                ));
            }
            for (index, entry) in actions.iter().enumerate().take(MAX_POLICY_SEGMENT_ACTIONS) {
                let share_with = entry.get(POLICY_SHARE_WITH);
                for (listed, path, cap) in [
                    (
                        share_with.and_then(::serde_json::Value::as_array),
                        format!("$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_SHARE_WITH}"),
                        MAX_POLICY_SEGMENTS,
                    ),
                    (
                        share_with
                            .and_then(::serde_json::Value::as_object)
                            .and_then(|with| with.get(POLICY_EXCEPT))
                            .and_then(::serde_json::Value::as_array),
                        format!(
                            "$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_SHARE_WITH}.{POLICY_EXCEPT}"
                        ),
                        MAX_POLICY_SEGMENTS,
                    ),
                    (
                        entry
                            .get(POLICY_VIA)
                            .and_then(::serde_json::Value::as_object)
                            .and_then(|via| via.get(POLICY_NETWORK_FUNCTION_GROUPS))
                            .and_then(::serde_json::Value::as_array),
                        format!(
                            "$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_VIA}.{POLICY_NETWORK_FUNCTION_GROUPS}"
                        ),
                        MAX_POLICY_NETWORK_FUNCTION_GROUPS,
                    ),
                    (
                        entry
                            .get(POLICY_DESTINATION_CIDR_BLOCKS)
                            .and_then(::serde_json::Value::as_array),
                        format!(
                            "$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_DESTINATION_CIDR_BLOCKS}"
                        ),
                        MAX_POLICY_ROUTE_PREFIXES,
                    ),
                    (
                        entry
                            .get(POLICY_DESTINATIONS)
                            .and_then(::serde_json::Value::as_array),
                        format!("$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_DESTINATIONS}"),
                        MAX_POLICY_ROUTE_DESTINATIONS,
                    ),
                ] {
                    let held = listed.map_or(0, Vec::len);
                    if held > cap {
                        errors.push(policy_error(
                            POLICY_ERROR_TOO_MANY_ENTRIES,
                            format!("{path} carries {held} entries, but at most {cap} are allowed"),
                            &path,
                        ));
                    }
                }
            }
        }
        Some(_) => errors.push(policy_error(
            POLICY_ERROR_INVALID_TYPE,
            format!("{POLICY_SEGMENT_ACTIONS:?} must be an array"),
            &format!("$.{POLICY_SEGMENT_ACTIONS}"),
        )),
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
    /// One entry per `attachment-policies[]` rule of the document, in the order the rules
    /// are evaluated: ascending `rule-number`.
    pub attachment_policies: Vec<AttachmentPolicyRule>,
    /// One entry per `segment-actions[]` entry of action `share` the document carries, in
    /// the order it lists them: which segment shares its routes, and which segments they
    /// are shared with.
    pub shares: Vec<SegmentShare>,
    /// One entry per `segment-actions[]` entry of action `send-via` / `send-to` the
    /// document carries, in the order it lists them: which segment's traffic is served by
    /// which network function groups.
    pub service_insertions: Vec<ServiceInsertion>,
    /// One entry per `segment-actions[]` entry of action `create-route` the document
    /// carries, in the order it lists them: the static routes it declares for the segment
    /// it acts on.
    pub static_routes: Vec<SegmentStaticRoute>,
}

/// The topology of NO policy: what a core network with no policy version in force reports,
/// and what one being deleted reports ([`CoreNetworkRecord::to_shape_with_state`]).
///
/// A constant rather than a value built per read, so answering either costs nothing and
/// [`CoreNetworkRecord::topology`] can hand a reference back.
pub static NO_POLICY_IN_FORCE: PolicyTopology = PolicyTopology {
    edges: Vec::new(),
    segments: Vec::new(),
    network_function_groups: Vec::new(),
    attachment_policies: Vec::new(),
    shares: Vec::new(),
    service_insertions: Vec::new(),
    static_routes: Vec::new(),
};

/// One `segment-actions[]` entry of action `create-route`: the STATIC routes a policy
/// document declares for one segment.
///
/// A static route is not a function of anything attached: the document states the prefix
/// and what it is reached through, so the routes a segment holds follow from the policy in
/// force on the core network and from nothing else. Kept as the document spells them — a
/// destination is resolved against the stored attachments where the routing information is
/// reported, so a destination naming an attachment that has since gone reaches nothing
/// rather than being reported as a route to it.
#[derive(Debug, Clone)]
pub struct SegmentStaticRoute {
    /// The entry's `segment`: the segment whose route table holds these routes. Always a
    /// segment the document declares, since an entry naming any other segment is not kept.
    pub segment: String,
    /// The entry's `destination-cidr-blocks`: the prefix each route it declares carries, in
    /// the order the document lists them, bounded by [`MAX_POLICY_ROUTE_PREFIXES`].
    pub destination_cidr_blocks: Vec<String>,
    /// The entry's `destinations`: what those prefixes are reached through — the id of an
    /// attachment of the segment, or [`POLICY_DESTINATION_BLACKHOLE`] for a route that
    /// reaches nothing — bounded by [`MAX_POLICY_ROUTE_DESTINATIONS`].
    pub destinations: Vec<String>,
}

/// One `segment-actions[]` entry of action `share`: the segment whose routes are shared,
/// and how the entry names the segments they are shared with.
#[derive(Debug, Clone)]
pub struct SegmentShare {
    /// The entry's `segment`: the segment that shares its routes. Always a segment the
    /// document declares, since an entry naming any other segment is not kept.
    pub segment: String,
    /// The entry's `share-with`.
    pub with: ShareWith,
}

/// How a `share` entry names the segments the sharing segment's routes are shared with:
/// either the names it lists, or every other declared segment save the ones it excepts.
///
/// Both forms hold their names as a SET of declared segment names, so what one entry
/// retains is bounded by [`MAX_POLICY_SEGMENTS`] however long — and however repetitive —
/// the list the document spelled was.
#[derive(Debug, Clone)]
pub enum ShareWith {
    /// The array form: the segments the entry lists, each one the document declares.
    Named(BTreeSet<String>),
    /// The `*` form (nothing excepted) and the `{"except": [...]}` form: every other
    /// segment the document declares, less the ones listed here.
    AllExcept(BTreeSet<String>),
}

/// One `segment-actions[]` entry of action `send-via` / `send-to`: the segment the entry
/// acts on, and the network function groups it sends that segment's traffic through.
#[derive(Debug, Clone)]
pub struct ServiceInsertion {
    /// Whether the entry's action is `send-via`; `send-to` otherwise. The two are reported
    /// through separate members of `ServiceInsertionSegments`, so which one it is is kept.
    pub send_via: bool,
    /// The entry's `segment`, always one the document declares.
    pub segment: String,
    /// The entry's `via.network-function-groups`, each one the document declares. Held as
    /// a SET, so it is bounded by [`MAX_POLICY_NETWORK_FUNCTION_GROUPS`] however long the
    /// list the document spelled was, and a membership test over it is logarithmic.
    pub network_function_groups: BTreeSet<String>,
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

/// One `attachment-policies[]` rule of a core network policy document: which attachments
/// the rule claims, and what it then does with one.
///
/// Compared for equality where two policy documents are diffed ([`policy_changes`]): the
/// `attachment-policies` block of one document differs from the other's exactly when their
/// rule lists do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentPolicyRule {
    /// The rule's `rule-number`, which orders it against the others and is what an
    /// attachment reports as `Attachment$AttachmentPolicyRuleNumber` (an `Integer`,
    /// `smithy_query shape=Attachment`).
    pub rule_number: i32,
    /// Whether EVERY condition has to hold for the rule to claim an attachment — the
    /// `condition-logic` default `and` — or any one of them ([`POLICY_LOGIC_OR`]).
    pub all_conditions: bool,
    /// The rule's `conditions[]`, each capped at [`MAX_POLICY_CONDITIONS_PER_RULE`].
    pub conditions: Vec<AttachmentCondition>,
    /// The rule's `action.require-attachment-acceptance`: whether an attachment this rule
    /// claims has to be accepted before it is created.
    pub require_acceptance: bool,
    /// How the rule's action names the segment it puts the attachment in, or `None` where
    /// it names none.
    pub segment: Option<SegmentAssociation>,
    /// The rule's `action.add-to-network-function-group`, where it adds the attachment to a
    /// network function group instead of a segment.
    pub network_function_group: Option<String>,
}

/// How an attachment-policy rule's action names the segment the attachment joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentAssociation {
    /// `association-method: constant` — the one name the action's `segment` carries.
    Constant(String),
    /// `association-method: tag` — the value of the attachment's tag whose key the action's
    /// `tag-value-of-key` names, so an attachment carrying no such tag joins no segment.
    TagValueOfKey(String),
}

/// One `conditions[]` entry of an attachment-policy rule, kept as the document spells it:
/// the property compared ([`CONDITION_ANY`] and friends), the `operator`
/// ([`OPERATOR_EQUALS`] and friends) and the `key` / `value` the comparison uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentCondition {
    pub condition_type: String,
    pub operator: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
}

/// The attachment an `attachment-policies` rule is matched against: the properties of it
/// the published condition types compare.
#[derive(Debug, Clone, Copy)]
pub struct AttachmentCandidate<'a> {
    /// One of `ops::types::ATTACHMENT_TYPE_VALUES`, what `attachment-type` compares.
    pub attachment_type: &'a str,
    /// The Region of the attachment's edge location, what `region` compares.
    pub edge_location: &'a str,
    /// The ARN of the resource the attachment attaches, what `resource-id` compares.
    pub resource_arn: &'a str,
    /// The account that owns the attachment, what `account-id` compares.
    pub account_id: &'a str,
    /// The attachment's tags, what `tag-exists` and `tag-value` look in.
    pub tags: &'a [Tag],
}

/// What the policy in force decided about one attachment: the rule that claimed it, where
/// that rule put it, and whether it has to be accepted first. The default — every member
/// absent, no acceptance required — is the answer for an attachment no rule claims, which
/// includes every attachment on a core network whose policy carries no
/// `attachment-policies` at all.
#[derive(Debug, Clone, Default)]
pub struct AttachmentClaim {
    /// The `rule-number` of the claiming rule.
    pub rule_number: Option<i32>,
    /// The segment that rule put the attachment in.
    pub segment_name: Option<String>,
    /// The network function group that rule added the attachment to.
    pub network_function_group_name: Option<String>,
    /// Whether that rule asks for the attachment request to be accepted, which is what
    /// holds the attachment in [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`].
    pub require_acceptance: bool,
}

/// The value of the tag `key` names, or `None` when the list carries no such tag.
fn tag_value_of<'a>(tags: &'a [Tag], key: &str) -> Option<&'a str> {
    tags.iter()
        .find(|tag| tag.key.as_deref() == Some(key))
        .map(|tag| tag.value.as_deref().unwrap_or_default())
}

impl AttachmentCondition {
    /// Whether this condition holds for `candidate`.
    ///
    /// A condition type this service does not know, or one missing the `key` or `value` its
    /// comparison needs, holds for NOTHING: an unreadable condition must not silently widen
    /// the rule it belongs to into one that claims every attachment.
    fn holds(&self, candidate: &AttachmentCandidate<'_>) -> bool {
        match self.condition_type.as_str() {
            CONDITION_ANY => true,
            CONDITION_ACCOUNT_ID => self.compares(candidate.account_id),
            CONDITION_REGION => self.compares(candidate.edge_location),
            CONDITION_ATTACHMENT_TYPE => self.compares(candidate.attachment_type),
            CONDITION_RESOURCE_ID => self.compares(candidate.resource_arn),
            CONDITION_TAG_EXISTS => self
                .key
                .as_deref()
                .is_some_and(|key| tag_value_of(candidate.tags, key).is_some()),
            CONDITION_TAG_VALUE => self
                .key
                .as_deref()
                .and_then(|key| tag_value_of(candidate.tags, key))
                .is_some_and(|held| self.compares(held)),
            _ => false,
        }
    }

    /// Whether `held`, the attachment's own value, satisfies this condition's `operator`
    /// against its `value`.
    fn compares(&self, held: &str) -> bool {
        let Some(wanted) = self.value.as_deref() else {
            return false;
        };
        match self.operator.as_deref() {
            Some(OPERATOR_EQUALS) => held == wanted,
            Some(OPERATOR_NOT_EQUALS) => held != wanted,
            Some(OPERATOR_CONTAINS) => held.contains(wanted),
            Some(OPERATOR_BEGINS_WITH) => held.starts_with(wanted),
            Some(_) | None => false,
        }
    }
}

impl AttachmentPolicyRule {
    /// Whether this rule claims `candidate`: every condition holds, or — under
    /// [`POLICY_LOGIC_OR`] — any one does. A rule carrying no condition claims nothing.
    fn claims(&self, candidate: &AttachmentCandidate<'_>) -> bool {
        if self.conditions.is_empty() {
            return false;
        }
        if self.all_conditions {
            self.conditions
                .iter()
                .all(|condition| condition.holds(candidate))
        } else {
            self.conditions
                .iter()
                .any(|condition| condition.holds(candidate))
        }
    }

    /// The segment this rule puts `candidate` in: the constant it names, or the value of the
    /// tag it takes the name from — which is absent when the attachment carries no such tag.
    fn segment_for(&self, candidate: &AttachmentCandidate<'_>) -> Option<String> {
        match self.segment.as_ref()? {
            SegmentAssociation::Constant(name) => Some(name.clone()),
            SegmentAssociation::TagValueOfKey(key) => {
                tag_value_of(candidate.tags, key).map(str::to_string)
            }
        }
    }
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

    /// The (edge location, name) pairs the entries `groups` are placed at: an entry
    /// restricting itself to no edge location is placed at every edge location this core
    /// network has, which is the expansion [`Self::to_segments`] makes too.
    fn placements(&self, groups: &[NamedGroup]) -> BTreeSet<(String, String)> {
        let everywhere = self.everywhere();
        let mut placed = BTreeSet::new();
        for group in groups {
            let locations = group.edge_locations.as_deref().unwrap_or(&everywhere);
            for location in locations {
                placed.insert((location.clone(), group.name.clone()));
            }
        }
        placed
    }

    /// Whether this core network configures an edge in `location`.
    ///
    /// An attachment joins a resource to one of the core network's EDGES, so a create
    /// naming a location the policy in force configures none for is refused: the edges are
    /// a function of the stored policy document, so this answer comes from the same
    /// derived topology a read of the core network reports.
    pub fn has_edge_location(&self, location: &str) -> bool {
        self.edges
            .iter()
            .any(|edge| edge.edge_location.as_deref() == Some(location))
    }

    /// Whether this core network declares a segment named `name`.
    ///
    /// The sibling of [`Self::has_edge_location`] for the other half of the topology: a
    /// segment of a core network is one its policy document's `segments` declares, so a
    /// request naming any other segment addresses nothing ([`no_such_segment`]).
    pub fn has_segment(&self, name: &str) -> bool {
        self.segments.iter().any(|group| group.name == name)
    }

    /// Whether the segment `name` is present at the edge location `location`: it is
    /// declared, and either restricts itself to locations holding that one or restricts
    /// itself to none at all — which places it at every edge this core network has, the same
    /// expansion [`Self::to_segments`] reports, so ONE statement of that rule serves both.
    ///
    /// A segment that is not present at an edge has no route table there, so it holds no
    /// routing information at that edge however much the core network holds elsewhere.
    pub fn segment_present_at(&self, name: &str, location: &str) -> bool {
        self.segments
            .iter()
            .filter(|group| group.name == name)
            .any(|group| match group.edge_locations.as_deref() {
                Some(listed) => listed.iter().any(|held| held == location),
                None => self.everywhere().iter().any(|held| held == location),
            })
    }

    /// The static routes this policy declares for the segment `name`, in the order the
    /// document lists the `create-route` entries that declare them.
    pub fn static_routes_of<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a SegmentStaticRoute> {
        self.static_routes
            .iter()
            .filter(move |route| route.segment == name)
    }

    /// The ASN this policy assigns the edge it configures in `location`, or `None` where it
    /// configures no edge there or assigns that edge none.
    ///
    /// The ASN of an edge is the core network's own end of every BGP session that edge
    /// peers over, which is what a Connect peer reports as
    /// `ConnectPeerBgpConfiguration$CoreNetworkAsn`: it comes from the same derived topology
    /// a read of the core network reports, so the two cannot disagree.
    pub fn edge_asn(&self, location: &str) -> Option<i64> {
        self.edges
            .iter()
            .find(|edge| edge.edge_location.as_deref() == Some(location))
            .and_then(|edge| edge.asn)
    }

    /// What this policy decides about the attachment `candidate` describes.
    ///
    /// The rules of `attachment-policies` are evaluated in ascending `rule-number` and the
    /// FIRST one whose conditions claim the attachment decides it (Cloud WAN "Attachment
    /// policies" documentation), so a later rule never overrides an earlier one. Only the
    /// claiming rule's action asks for acceptance: an attachment no rule claims \u2014 every
    /// attachment, where the document carries no `attachment-policies` \u2014 needs none, since
    /// the action is the only place a document asks for one.
    ///
    /// Called ONCE, where the attachment is created, and the answer is stored on the
    /// attachment record: an attachment keeps the decision the policy in force made about
    /// it, so a later read never re-derives it from a policy that has since changed.
    pub fn attachment_claim(&self, candidate: &AttachmentCandidate<'_>) -> AttachmentClaim {
        let Some(rule) = self
            .attachment_policies
            .iter()
            .find(|rule| rule.claims(candidate))
        else {
            return AttachmentClaim::default();
        };
        AttachmentClaim {
            rule_number: Some(rule.rule_number),
            segment_name: rule.segment_for(candidate),
            network_function_group_name: rule.network_function_group.clone(),
            require_acceptance: rule.require_acceptance,
        }
    }

    /// The modelled `Segments` of this topology.
    ///
    /// A segment's `SharedSegments` are the segments its routes are shared with, which the
    /// document's `segment-actions` entries of action `share` declare: a segment no such
    /// entry acts on shares with nothing and reports the empty list. The answer is in the
    /// order the document declares the segments, and never names the segment itself.
    pub fn to_segments(&self) -> Vec<CoreNetworkSegment> {
        let everywhere = self.everywhere();
        let declared = self.declared_segments();
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
                shared_segments: Some(self.shared_segments_of(&group.name, &declared)),
            })
            .collect()
    }

    /// The modelled `NetworkFunctionGroups` of this topology.
    ///
    /// A group's `Segments` are the segments whose traffic the document's `segment-actions`
    /// send through this group, split by the action that sent them: `SendVia` for the
    /// `send-via` entries and `SendTo` for the `send-to` ones. A group no such entry names
    /// serves no segment and reports two empty lists. Each list is in the order the
    /// document declares the segments.
    pub fn to_network_function_groups(&self) -> Vec<CoreNetworkNetworkFunctionGroup> {
        let everywhere = self.everywhere();
        let declared = self.declared_segments();
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
                segments: Some(ServiceInsertionSegments {
                    send_via: Some(self.inserted_segments_of(&group.name, true, &declared)),
                    send_to: Some(self.inserted_segments_of(&group.name, false, &declared)),
                }),
            })
            .collect()
    }

    /// The names of the segments this document declares, in the order it declares them and
    /// each named once: the order every segment list of a reported topology is in, and the
    /// only names such a list may carry — a `segment-actions` entry naming anything else
    /// names no segment of this core network.
    fn declared_segments(&self) -> Vec<&str> {
        let mut declared: Vec<&str> = Vec::with_capacity(self.segments.len());
        for group in &self.segments {
            if !declared.contains(&group.name.as_str()) {
                declared.push(group.name.as_str());
            }
        }
        declared
    }

    /// The segments the routes of the segment `name` are shared with: the union of what
    /// every `share` entry acting on it declares, in `declared` order.
    ///
    /// Bounded by the caps: at most [`MAX_POLICY_SEGMENT_ACTIONS`] entries are walked, each
    /// holding at most [`MAX_POLICY_SEGMENTS`] names, and the answer is at most `declared`
    /// long.
    fn shared_segments_of(&self, name: &str, declared: &[&str]) -> Vec<String> {
        let mut shared: BTreeSet<&str> = BTreeSet::new();
        for share in self.shares.iter().filter(|share| share.segment == name) {
            match &share.with {
                ShareWith::Named(names) => shared.extend(names.iter().map(String::as_str)),
                ShareWith::AllExcept(excepted) => shared.extend(
                    declared
                        .iter()
                        .copied()
                        .filter(|candidate| !excepted.contains(*candidate)),
                ),
            }
        }
        declared
            .iter()
            .copied()
            // A segment never shares with itself, however the entry names it: its own
            // routes are already its own.
            .filter(|candidate| *candidate != name && shared.contains(candidate))
            .map(str::to_string)
            .collect()
    }

    /// The segments whose traffic the network function group `name` serves under the
    /// `send-via` action (`send_via`) or the `send-to` one, in `declared` order.
    ///
    /// Bounded by the caps the same way [`Self::shared_segments_of`] is: at most
    /// [`MAX_POLICY_SEGMENT_ACTIONS`] entries are walked, each answering whether it names
    /// this group in logarithmic time.
    fn inserted_segments_of(&self, name: &str, send_via: bool, declared: &[&str]) -> Vec<String> {
        let served: BTreeSet<&str> = self
            .service_insertions
            .iter()
            .filter(|insertion| {
                insertion.send_via == send_via && insertion.network_function_groups.contains(name)
            })
            .map(|insertion| insertion.segment.as_str())
            .collect();
        declared
            .iter()
            .copied()
            .filter(|candidate| served.contains(candidate))
            .map(str::to_string)
            .collect()
    }

    /// The distinct service insertion actions the `send-via` / `send-to` entries acting on
    /// the segment `name` declare.
    ///
    /// Bounded by [`MAX_POLICY_SEGMENT_ACTIONS`] entries, each naming at most
    /// [`MAX_POLICY_NETWORK_FUNCTION_GROUPS`] groups.
    fn insertions_of(&self, name: &str) -> BTreeSet<SegmentInsertion> {
        self.service_insertions
            .iter()
            .filter(|insertion| insertion.segment == name)
            .map(|insertion| SegmentInsertion {
                send_via: insertion.send_via,
                network_function_groups: insertion
                    .network_function_groups
                    .iter()
                    .cloned()
                    .collect(),
            })
            .collect()
    }

    /// What this policy states about each of its SEGMENTS at each edge location that segment
    /// is present in: the objects a change set compares to decide what a version does to one
    /// segment ([`policy_changes`]).
    ///
    /// The key is the placement [`Self::placements`] derives and the value is everything else
    /// the document states about that segment — what the `segment-actions` acting on it
    /// declare — so two documents placing one segment at the same edge location but sharing
    /// its routes with other segments, or serving it through other network function groups,
    /// configure it differently.
    ///
    /// What those actions declare is a property of the SEGMENT and not of one of its
    /// placements, so it is derived once per declared segment and SHARED by that segment's
    /// placements: a segment present in every edge location of the core network holds one
    /// copy rather than one per location, which keeps this linear in the document it came
    /// from however many edge locations [`MAX_POLICY_EDGE_LOCATIONS`] allows.
    fn segment_placements(&self) -> BTreeMap<(String, String), Arc<SegmentConfiguration>> {
        let declared = self.declared_segments();
        let configurations: BTreeMap<&str, Arc<SegmentConfiguration>> = declared
            .iter()
            .map(|name| {
                (
                    *name,
                    Arc::new(SegmentConfiguration {
                        shared_segments: self.shared_segments_of(name, &declared),
                        service_insertions: self.insertions_of(name),
                    }),
                )
            })
            .collect();
        self.placements(&self.segments)
            .into_iter()
            .map(|(location, name)| {
                let configuration = configurations
                    .get(name.as_str())
                    .cloned()
                    .unwrap_or_default();
                ((location, name), configuration)
            })
            .collect()
    }

    /// Every STATIC ROUTE this policy declares, as (segment, route).
    ///
    /// A `create-route` entry declares one route per prefix and destination it names — the
    /// routes [`Self::static_routes_of`] hands the listing of a segment's routing
    /// information. The entry declares them for the SEGMENT rather than for one edge
    /// location: the segment holds them in its route table wherever it is present, so one
    /// declaration is one route here and is not multiplied over the segment's edge
    /// locations. Held as a SET, so an entry declaring one prefix twice, or two entries
    /// declaring the same route, keep it once.
    ///
    /// Bounded by the caps, and so linear in the document: at most
    /// [`MAX_POLICY_SEGMENT_ACTIONS`] entries, each declaring at most
    /// [`MAX_POLICY_ROUTE_PREFIXES`] prefixes through at most
    /// [`MAX_POLICY_ROUTE_DESTINATIONS`] destinations.
    fn static_route_placements(&self) -> BTreeSet<(String, StaticRouteIdentity)> {
        let mut declared = BTreeSet::new();
        for route in &self.static_routes {
            for prefix in &route.destination_cidr_blocks {
                for destination in &route.destinations {
                    declared.insert((
                        route.segment.clone(),
                        StaticRouteIdentity {
                            prefix: prefix.clone(),
                            destination: destination.clone(),
                        },
                    ));
                }
            }
        }
        declared
    }
}

/// One change of a policy version's change set: what the difference between two policy
/// documents does to ONE object either of them configures.
///
/// Kept in this compact form rather than as a modelled shape because the SAME change is
/// reported two ways — as a `CoreNetworkChange` of the change set and as a
/// `CoreNetworkChangeEvent` of its execution — and because a change event's status and
/// timestamp are functions of the clock, not of the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyChange {
    /// What kind of object changed: one of the [`CHANGE_TYPE_CORE_NETWORK_EDGE`] family.
    pub change_type: &'static str,
    /// What the change does to it: one of the [`CHANGE_ACTION_ADD`] family.
    pub action: &'static str,
    /// The edge location the changed object sits at, for a kind that sits at one.
    pub edge_location: Option<String>,
    /// The changed segment's or network function group's name, for a kind that has one —
    /// and, for a change to one static route, the segment whose route tables hold it.
    pub name: Option<String>,
    /// Which static route this change acts on, for a change to one and `None` for every
    /// other change: a segment's route table holds one route per prefix and destination, so
    /// the pair is what identifies the route inside the segment [`Self::name`] gives.
    pub route: Option<StaticRouteIdentity>,
    /// What the policy IN FORCE states about the object this change acts on beyond the
    /// identity above, for a change that modifies or removes an object of a kind that
    /// carries any such values — `None` for every other change: these are the only values a
    /// reported change carries that its own identity does not already name, so the only ones
    /// the diff has to keep.
    pub previous: Option<ChangedObject>,
    /// What the SUBMITTED policy states about that object, for a change that adds or
    /// modifies it. See [`Self::previous`].
    pub new: Option<ChangedObject>,
}

/// What one of the two compared policies states about the object a change acts on, beyond
/// the identity that change already names.
///
/// A kind whose identity is everything a document states about it carries none of these — a
/// network function group at an edge location, one static route, the `attachment-policies`
/// block — so a change to one holds `None` on both sides and reports only that identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedObject {
    /// One edge of `core-network-configuration`: see [`EdgeConfiguration`].
    Edge(EdgeConfiguration),
    /// One segment of `segments` at one edge location: see [`SegmentConfiguration`]. Shared
    /// between that segment's placements, which a document states the same thing about.
    Segment(Arc<SegmentConfiguration>),
}

/// Which ONE static route of a segment's route table a change acts on: the prefix it
/// carries, and what that prefix is reached through.
///
/// A `create-route` entry declares one route per prefix and destination it names — the
/// routes [`PolicyTopology::static_routes_of`] hands the listing of a segment's routing
/// information — so the pair identifies one route, and is what a change to it reports as
/// `Cidr` and `DestinationIdentifier`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StaticRouteIdentity {
    /// The route's prefix: one `destination-cidr-blocks` entry of the `create-route` entry
    /// that declared it.
    pub prefix: String,
    /// What that prefix is reached through: one `destinations` entry of the same entry — an
    /// attachment of the segment, or [`POLICY_DESTINATION_BLACKHOLE`].
    pub destination: String,
}

/// What a policy document states about one segment beyond its name and the edge locations
/// it is present in: what the `segment-actions` acting on that segment declare.
///
/// These are the values a read of the core network reports for the segment and for the
/// groups that serve it (`CoreNetworkSegment$SharedSegments` through
/// [`PolicyTopology::to_segments`], and `ServiceInsertionSegments` through
/// [`PolicyTopology::to_network_function_groups`]), so two documents giving one segment
/// different ones configure it differently and a version that moves them modifies it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentConfiguration {
    /// The segments this segment's routes are shared with, in the order the document
    /// declares the segments ([`PolicyTopology::shared_segments_of`]).
    pub shared_segments: Vec<String>,
    /// The distinct service insertion actions acting on this segment
    /// ([`PolicyTopology::insertions_of`]), held as a SET: what a document sends a segment
    /// through does not depend on the order it lists the entries in, nor on one entry being
    /// listed twice, so neither makes two documents configure the segment differently.
    pub service_insertions: BTreeSet<SegmentInsertion>,
}

/// ONE service insertion action acting on a segment: which of the two actions it is, and the
/// network function groups it sends that segment's traffic through.
///
/// Ordered by the action and then by the groups it names, which is the order a change reports
/// them in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentInsertion {
    /// Whether the action is `send-via`; `send-to` otherwise.
    pub send_via: bool,
    /// The declared network function groups the action names, in name order.
    pub network_function_groups: Vec<String>,
}

impl SegmentInsertion {
    /// The modelled `ServiceInsertionAction` for this action: the action it takes, and the
    /// groups it names through `Via$NetworkFunctionGroups`.
    ///
    /// A `mode` and a `when-sent-to` are parts of the entry the derived topology does not
    /// keep — no operation of this slice acts on either — so neither is reported rather than
    /// being reported as a value the document did not state.
    fn to_shape(&self) -> ServiceInsertionAction {
        ServiceInsertionAction {
            action: Some(
                if self.send_via {
                    POLICY_ACTION_SEND_VIA
                } else {
                    POLICY_ACTION_SEND_TO
                }
                .to_string(),
            ),
            mode: None,
            when_sent_to: None,
            via: Some(Via {
                network_function_groups: Some(
                    self.network_function_groups
                        .iter()
                        .map(|name| NetworkFunctionGroup {
                            name: Some(name.clone()),
                        })
                        .collect(),
                ),
                with_edge_overrides: None,
            }),
        }
    }
}

/// How one policy document configures one edge, beyond the edge location that identifies
/// it: the ASN that edge peers with and the inside CIDR blocks it carries.
///
/// Compared for equality to decide whether two documents configure the same edge
/// differently, and reported as the values a change to that edge replaces or puts in place.
/// An edge the document gives no inside CIDR blocks carries none, which is why the list is
/// held flat rather than as an absent option: "no blocks" and "an empty list of blocks"
/// configure the edge the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeConfiguration {
    /// The edge's ASN, or `None` where the document assigns it none.
    pub asn: Option<i64>,
    /// The edge's inside CIDR blocks, empty where the document gives it none.
    pub inside_cidr_blocks: Vec<String>,
}

impl EdgeConfiguration {
    /// How `edge` is configured.
    fn of(edge: &CoreNetworkEdge) -> Self {
        Self {
            asn: edge.asn,
            inside_cidr_blocks: edge.inside_cidr_blocks.clone().unwrap_or_default(),
        }
    }

    /// The modelled change values for an edge configured this way at `edge_location`.
    fn to_change_values(&self, edge_location: &str) -> CoreNetworkChangeValues {
        CoreNetworkChangeValues {
            edge_locations: Some(vec![edge_location.to_string()]),
            asn: self.asn,
            inside_cidr_blocks: (!self.inside_cidr_blocks.is_empty())
                .then(|| self.inside_cidr_blocks.clone()),
            ..CoreNetworkChangeValues::default()
        }
    }
}

impl PolicyChange {
    /// This change's `IdentifierPath`: the type, then the edge location and the name where
    /// the kind carries them — "the `IdentifierPath` for a core network segment change
    /// might be `CORE_NETWORK_SEGMENT/us-east-1/devsegment`" (`smithy_query shape=
    /// CoreNetworkChangeEvent member=IdentifierPath`) — and then, for a change to one static
    /// route, the prefix that route carries and what it is reached through, which is what
    /// tells two routes of one segment apart.
    ///
    /// It "uniquely identifies the path for a change within the changeset" (same doc), so
    /// it is also the key one page of a listing resumes after.
    pub fn identifier_path(&self) -> String {
        let mut path = self.change_type.to_string();
        let route = self.route.as_ref();
        for component in [
            self.edge_location.as_deref(),
            self.name.as_deref(),
            route.map(|route| route.prefix.as_str()),
            route.map(|route| route.destination.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            path.push('/');
            path.push_str(component);
        }
        path
    }

    /// The resource this change acts on: the prefix for a change to one static route, which
    /// is what names a route in its segment's route table, the changed segment's or network
    /// function group's own name, the edge location for an edge, and nothing for a change to
    /// an object none of a policy's named entries identifies (the `attachment-policies`
    /// block as a whole).
    fn identifier(&self) -> Option<String> {
        self.route
            .as_ref()
            .map(|route| route.prefix.clone())
            .or_else(|| self.name.clone())
            .or_else(|| self.edge_location.clone())
    }

    /// The values the object this change acts on holds under one of the two policies: its
    /// identity, plus what `configured` says that policy states about it. `None` for a
    /// change to an object no member of the shape names, as [`Self::to_change_event`]
    /// reports no values for one.
    fn to_change_values(
        &self,
        configured: Option<&ChangedObject>,
    ) -> Option<CoreNetworkChangeValues> {
        match (self.change_type, self.edge_location.as_deref(), self.name.as_deref()) {
            (CHANGE_TYPE_CORE_NETWORK_EDGE, Some(location), _) => {
                // An edge change always carries the configuration of the side it reports —
                // the policy that configured the edge is the side that has one — so a side
                // with none reports no values rather than an edge configured with nothing.
                match configured {
                    Some(ChangedObject::Edge(edge)) => Some(edge.to_change_values(location)),
                    _ => None,
                }
            }
            (CHANGE_TYPE_CORE_NETWORK_SEGMENT, Some(location), Some(name)) => {
                // What the `segment-actions` of that side's document declare for this
                // segment. A segment no such action acts on is shared with nothing and
                // served by nothing, so each list is absent rather than empty.
                let segment = match configured {
                    Some(ChangedObject::Segment(held)) => Some(held),
                    _ => None,
                };
                Some(CoreNetworkChangeValues {
                    segment_name: Some(name.to_string()),
                    edge_locations: Some(vec![location.to_string()]),
                    shared_segments: segment
                        .map(|held| held.shared_segments.clone())
                        .filter(|shared| !shared.is_empty()),
                    service_insertion_actions: segment
                        .map(|held| {
                            held.service_insertions
                                .iter()
                                .map(SegmentInsertion::to_shape)
                                .collect::<Vec<_>>()
                        })
                        .filter(|actions| !actions.is_empty()),
                    ..CoreNetworkChangeValues::default()
                })
            }
            (CHANGE_TYPE_NETWORK_FUNCTION_GROUP, Some(location), Some(name)) => {
                Some(CoreNetworkChangeValues {
                    network_function_group_name: Some(name.to_string()),
                    edge_locations: Some(vec![location.to_string()]),
                    ..CoreNetworkChangeValues::default()
                })
            }
            (CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC, _, Some(segment)) => {
                // A static route is identified completely by the segment whose route tables
                // hold it, its prefix and what that prefix is reached through, so its
                // identity is also everything the document states about it.
                self.route.as_ref().map(|route| CoreNetworkChangeValues {
                    segment_name: Some(segment.to_string()),
                    cidr: Some(route.prefix.clone()),
                    destination_identifier: Some(route.destination.clone()),
                    ..CoreNetworkChangeValues::default()
                })
            }
            _ => None,
        }
    }

    /// The modelled `CoreNetworkChange` for this change: what it does to the object it
    /// names, the values that object held under the policy in force (`PreviousValues`) and
    /// the ones the submitted policy gives it (`NewValues`).
    ///
    /// Nothing precedes an addition and nothing follows a removal, so an
    /// [`CHANGE_ACTION_ADD`] carries no previous values and a [`CHANGE_ACTION_REMOVE`] no
    /// new ones; a [`CHANGE_ACTION_MODIFY`] carries both.
    pub fn to_core_network_change(&self) -> CoreNetworkChange {
        CoreNetworkChange {
            r#type: Some(self.change_type.to_string()),
            action: Some(self.action.to_string()),
            identifier: self.identifier(),
            previous_values: (self.action != CHANGE_ACTION_ADD)
                .then(|| self.to_change_values(self.previous.as_ref()))
                .flatten(),
            new_values: (self.action != CHANGE_ACTION_REMOVE)
                .then(|| self.to_change_values(self.new.as_ref()))
                .flatten(),
            identifier_path: Some(self.identifier_path()),
        }
    }

    /// The modelled `CoreNetworkChangeEvent` for this change, in `status` as of
    /// `event_time`.
    pub fn to_change_event(
        &self,
        status: &str,
        event_time: SystemTime,
    ) -> CoreNetworkChangeEvent {
        // `Values` describes the changed object, so a change to an object none of its
        // members names (the `attachment-policies` block as a whole) carries none rather
        // than an empty structure.
        let values = (self.edge_location.is_some() || self.name.is_some()).then(|| {
            // A static route sits in the route table of the segment it was declared for, so
            // its event names that segment too — "the segment name if the change event is
            // associated with a segment" (`smithy_query shape=CoreNetworkChangeEventValues
            // member=SegmentName`).
            let names_segment = self.change_type == CHANGE_TYPE_CORE_NETWORK_SEGMENT
                || self.change_type == CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC;
            CoreNetworkChangeEventValues {
                edge_location: self.edge_location.clone(),
                segment_name: names_segment.then(|| self.name.clone()).flatten(),
                network_function_group_name: (self.change_type
                    == CHANGE_TYPE_NETWORK_FUNCTION_GROUP)
                    .then(|| self.name.clone())
                    .flatten(),
                // The prefix of the static route this change acts on, which is what the
                // member reports for a static-route event.
                cidr: self.route.as_ref().map(|route| route.prefix.clone()),
                // The remaining members describe objects no policy document configures — a
                // peer edge and a routing policy association — so a change derived from two
                // documents fills none. `AttachmentId` is left out too: a route's
                // destination is whatever the document spelled, which names no attachment at
                // all for a blackholed route, so the destination travels in the change's own
                // `DestinationIdentifier` rather than as an attachment id.
                peer_edge_location: None,
                routing_policy_direction: None,
                attachment_id: None,
                routing_policy_association_details: None,
            }
        });
        CoreNetworkChangeEvent {
            r#type: Some(self.change_type.to_string()),
            action: Some(self.action.to_string()),
            identifier_path: Some(self.identifier_path()),
            event_time: Some(epoch_seconds(event_time)),
            status: Some(status.to_string()),
            values,
        }
    }
}

/// The changes a change set carries: the difference between the policy `in_force` and the
/// `submitted` one, one entry per object the two configure differently, ordered by
/// [`PolicyChange::identifier_path`].
///
/// An object only the submitted policy configures is [`CHANGE_ACTION_ADD`]ed, one only the
/// policy in force configures is [`CHANGE_ACTION_REMOVE`]d, and one both configure
/// differently is [`CHANGE_ACTION_MODIFY`]ed; an object both configure the same way is no
/// change at all and produces no entry, so two identical documents differ in nothing. The
/// objects compared are every one a document configures and [`policy_topology`] therefore
/// derives, with everything the document states about it:
///
/// * each edge of `core-network-configuration`, whose ASN and inside CIDR blocks are what a
///   modification of it is;
/// * each named `segments` entry at each edge location it is present in, carrying what the
///   `segment-actions` acting on that segment declare — the segments its routes are shared
///   with and the service insertion actions that serve it — so a document differing ONLY in
///   those modifies the segment rather than changing nothing;
/// * each named `network-function-groups` entry at each edge location it is present in,
///   which its placement describes completely: which segments it serves is stated on those
///   segments, through the `segment-actions` entries that act on them;
/// * each static route a `create-route` entry declares for a segment, identified by that
///   segment, its prefix and what that prefix reaches — one route per declaration, since a
///   segment holds its route table wherever it is present;
/// * the `attachment-policies` block as a whole, the one block that configures no object of
///   its own.
///
/// Both lists are bounded by the caps [`validate_policy_document`] enforces before a
/// document is stored, so the comparison and its result are bounded too.
pub fn policy_changes(in_force: &PolicyTopology, submitted: &PolicyTopology) -> Vec<PolicyChange> {
    let mut changes: Vec<PolicyChange> = Vec::new();

    let edges_before = edges_by_location(in_force);
    let edges_after = edges_by_location(submitted);
    for location in edges_before.keys().chain(edges_after.keys()).collect::<BTreeSet<_>>() {
        // An edge is configured by its ASN and its inside CIDR blocks, so a difference in
        // either is what makes the change a modification rather than no change.
        let previous_edge = edges_before.get(location).map(|edge| EdgeConfiguration::of(edge));
        let new_edge = edges_after.get(location).map(|edge| EdgeConfiguration::of(edge));
        let action = match (&previous_edge, &new_edge) {
            (None, Some(_)) => CHANGE_ACTION_ADD,
            (Some(_), None) => CHANGE_ACTION_REMOVE,
            (Some(held), Some(given)) if held != given => CHANGE_ACTION_MODIFY,
            _ => continue,
        };
        changes.push(PolicyChange {
            change_type: CHANGE_TYPE_CORE_NETWORK_EDGE,
            action,
            edge_location: Some((*location).to_string()),
            name: None,
            route: None,
            previous: previous_edge.map(ChangedObject::Edge),
            new: new_edge.map(ChangedObject::Edge),
        });
    }

    // A segment's placement carries what the `segment-actions` acting on it declare, so a
    // placement both policies make is a MODIFY exactly where those differ — which is what a
    // version changing only the sharing or the service insertion of a segment does to it.
    let segments_before = in_force.segment_placements();
    let segments_after = submitted.segment_placements();
    for placement in segments_before
        .keys()
        .chain(segments_after.keys())
        .collect::<BTreeSet<_>>()
    {
        let previous = segments_before.get(placement);
        let new = segments_after.get(placement);
        let action = match (previous, new) {
            (None, Some(_)) => CHANGE_ACTION_ADD,
            (Some(_), None) => CHANGE_ACTION_REMOVE,
            (Some(held), Some(given)) if held != given => CHANGE_ACTION_MODIFY,
            _ => continue,
        };
        let (location, name) = placement;
        changes.push(PolicyChange {
            change_type: CHANGE_TYPE_CORE_NETWORK_SEGMENT,
            action,
            edge_location: Some(location.clone()),
            name: Some(name.clone()),
            route: None,
            previous: previous.cloned().map(ChangedObject::Segment),
            new: new.cloned().map(ChangedObject::Segment),
        });
    }

    // A network function group's placement, and one static route, carry nothing beyond what
    // identifies them, so one both policies make is never a MODIFY: it is the same object,
    // unchanged.
    let groups_before = in_force.placements(&in_force.network_function_groups);
    let groups_after = submitted.placements(&submitted.network_function_groups);
    for (placements, action) in [
        (groups_after.difference(&groups_before), CHANGE_ACTION_ADD),
        (groups_before.difference(&groups_after), CHANGE_ACTION_REMOVE),
    ] {
        for (location, name) in placements {
            changes.push(PolicyChange {
                change_type: CHANGE_TYPE_NETWORK_FUNCTION_GROUP,
                action,
                edge_location: Some(location.clone()),
                name: Some(name.clone()),
                route: None,
                previous: None,
                new: None,
            });
        }
    }

    let routes_before = in_force.static_route_placements();
    let routes_after = submitted.static_route_placements();
    for (routes, action) in [
        (routes_after.difference(&routes_before), CHANGE_ACTION_ADD),
        (routes_before.difference(&routes_after), CHANGE_ACTION_REMOVE),
    ] {
        for (segment, route) in routes {
            changes.push(PolicyChange {
                change_type: CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC,
                action,
                edge_location: None,
                name: Some(segment.clone()),
                route: Some(route.clone()),
                previous: None,
                new: None,
            });
        }
    }

    if in_force.attachment_policies != submitted.attachment_policies {
        changes.push(PolicyChange {
            change_type: CHANGE_TYPE_ATTACHMENT_POLICIES_CONFIGURATION,
            action: match (
                in_force.attachment_policies.is_empty(),
                submitted.attachment_policies.is_empty(),
            ) {
                (true, false) => CHANGE_ACTION_ADD,
                (false, true) => CHANGE_ACTION_REMOVE,
                _ => CHANGE_ACTION_MODIFY,
            },
            edge_location: None,
            name: None,
            route: None,
            previous: None,
            new: None,
        });
    }

    changes.sort_by_cached_key(PolicyChange::identifier_path);
    changes
}

/// The edges a topology configures, by edge location. An entry the document spells without
/// a location names no edge and is already absent from the topology.
fn edges_by_location(topology: &PolicyTopology) -> BTreeMap<&str, &CoreNetworkEdge> {
    topology
        .edges
        .iter()
        .filter_map(|edge| edge.edge_location.as_deref().map(|location| (location, edge)))
        .collect()
}

/// Derive the reported topology of a core network from its policy document.
///
/// The rules are the published policy schema's own: an edge exists in each location the
/// `core-network-configuration` lists, carrying that entry's `asn` — or, where the entry
/// names none, the next ASN of the configuration's `asn-ranges` that no entry claims, as
/// Cloud WAN assigns one from the ranges — and its `inside-cidr-blocks`; a segment exists
/// for each named entry of `segments`, present in the edge locations it restricts itself
/// to or, where it names none, in every edge location of the core network; the same
/// location rule holds for each named entry of `network-function-groups`; and each
/// `attachment-policies` rule is kept with its conditions and its action, ordered by the
/// ascending `rule-number` the rules are evaluated in. The `segment-actions` entries say
/// what each segment's routes reach: a `share` entry shares the routes of the segment it
/// acts on with the segments its `share-with` names, a `send-via` / `send-to` entry
/// sends that segment's traffic through the network function groups its
/// `via.network-function-groups` names, and a `create-route` entry declares the static
/// routes that segment holds — the prefixes its `destination-cidr-blocks` lists, reached
/// through the attachments its `destinations` names ([`segment_actions`]).
///
/// A document this service cannot parse describes no topology, so it answers the empty
/// one rather than guessing: a document is only stored once
/// [`validate_policy_document`] has accepted it, which also caps every list this walks,
/// so the work and the result are bounded by those caps.
///
/// Called ONCE per document, where that document is accepted and BEFORE the store is
/// locked, and the answer is stored on the record beside it
/// ([`CoreNetworkPolicyRecord::topology`]): a read of a core network reports the derived
/// topology, and no operation — a read, an execution or a restore — re-parses the
/// document or parses one while holding the store lock.
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

    let named = |key: &str, cap: usize| -> Vec<NamedGroup> {
        root.get(key)
            .and_then(::serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            // Both counts are already capped where the document is accepted; the `take`s
            // here keep the derived topology bounded for any document that reaches the
            // deriver, so no caller can make one entry expand without limit.
            .take(cap)
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
                        .map(|listed| {
                            strings_of(
                                &listed[..listed.len().min(MAX_POLICY_EDGE_LOCATIONS)],
                            )
                        }),
                })
            })
            .collect()
    };

    let segments = named(POLICY_SEGMENTS, MAX_POLICY_SEGMENTS);
    let network_function_groups =
        named(POLICY_NETWORK_FUNCTION_GROUPS, MAX_POLICY_NETWORK_FUNCTION_GROUPS);
    // The segment actions are read LAST, because an action is only kept where the segments
    // and groups it names are ones the document declares.
    let actions = segment_actions(root, &segments, &network_function_groups);
    PolicyTopology {
        segments,
        network_function_groups,
        edges,
        attachment_policies: attachment_policies(root),
        shares: actions.shares,
        service_insertions: actions.service_insertions,
        static_routes: actions.static_routes,
    }
}

/// What [`segment_actions`] reads out of a document's `segment-actions`, one list per kind
/// of action it derives something from.
struct SegmentActions {
    shares: Vec<SegmentShare>,
    service_insertions: Vec<ServiceInsertion>,
    static_routes: Vec<SegmentStaticRoute>,
}

/// The `segment-actions` entries of a policy document's root object this slice derives the
/// reported topology from: the `share` entries, which decide which segments a segment's
/// routes are shared with, the `send-via` / `send-to` ones, which decide which segments a
/// network function group serves, and the `create-route` ones, which declare the static
/// routes a segment's route tables hold.
///
/// An entry is kept only where it is readable AND names declared resources: a segment or a
/// network function group no entry of `segments` / `network-function-groups` declares is
/// none of this core network's, so it is dropped rather than reported. An action of any
/// other kind decides none of the three and is passed over.
///
/// Every list this walks is capped, which is what bounds both the walk and what it retains:
/// the entry count at [`MAX_POLICY_SEGMENT_ACTIONS`], and one entry's name lists at the
/// segment and group caps, or at [`MAX_POLICY_ROUTE_PREFIXES`] /
/// [`MAX_POLICY_ROUTE_DESTINATIONS`] for a `create-route` entry — the counts
/// [`validate_policy_document`] refuses a document past, so a STORED document carries none
/// longer. The kept names are held as SETS where an entry names things once, so a list
/// repeating one name retains it once; a route's prefixes and destinations keep the
/// document's order, which is the order the routes are reported in.
fn segment_actions(
    root: &::serde_json::Map<String, ::serde_json::Value>,
    segments: &[NamedGroup],
    network_function_groups: &[NamedGroup],
) -> SegmentActions {
    fn declares(groups: &[NamedGroup], name: &str) -> bool {
        groups.iter().any(|group| group.name == name)
    }

    let mut shares = Vec::new();
    let mut service_insertions = Vec::new();
    let mut static_routes = Vec::new();
    for entry in root
        .get(POLICY_SEGMENT_ACTIONS)
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .take(MAX_POLICY_SEGMENT_ACTIONS)
    {
        let Some(action) = entry
            .get(POLICY_ACTION)
            .and_then(::serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(segment) = entry
            .get(POLICY_SEGMENT)
            .and_then(::serde_json::Value::as_str)
            .filter(|name| declares(segments, name))
        else {
            continue;
        };
        match action {
            POLICY_ACTION_SHARE => {
                if let Some(with) = share_with(entry.get(POLICY_SHARE_WITH), segments) {
                    shares.push(SegmentShare {
                        segment: segment.to_string(),
                        with,
                    });
                }
            }
            POLICY_ACTION_SEND_VIA | POLICY_ACTION_SEND_TO => {
                let groups: BTreeSet<String> = entry
                    .get(POLICY_VIA)
                    .and_then(::serde_json::Value::as_object)
                    .and_then(|via| via.get(POLICY_NETWORK_FUNCTION_GROUPS))
                    .and_then(::serde_json::Value::as_array)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .take(MAX_POLICY_NETWORK_FUNCTION_GROUPS)
                    .filter_map(::serde_json::Value::as_str)
                    .filter(|name| declares(network_function_groups, name))
                    .map(str::to_string)
                    .collect();
                // An entry sending a segment through no declared group serves nothing, so
                // there is nothing to keep.
                if !groups.is_empty() {
                    service_insertions.push(ServiceInsertion {
                        send_via: action == POLICY_ACTION_SEND_VIA,
                        segment: segment.to_string(),
                        network_function_groups: groups,
                    });
                }
            }
            POLICY_ACTION_CREATE_ROUTE => {
                let listed = |key: &str, cap: usize| -> Vec<String> {
                    entry
                        .get(key)
                        .and_then(::serde_json::Value::as_array)
                        .map_or(&[][..], Vec::as_slice)
                        .iter()
                        .take(cap)
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                };
                let destination_cidr_blocks =
                    listed(POLICY_DESTINATION_CIDR_BLOCKS, MAX_POLICY_ROUTE_PREFIXES);
                let destinations = listed(POLICY_DESTINATIONS, MAX_POLICY_ROUTE_DESTINATIONS);
                // An entry declaring no prefix declares no route, and one naming no
                // destination says nothing about what its prefixes reach — the published
                // schema requires both of a `create-route` entry — so neither is kept
                // rather than being completed with a rule the document did not state.
                if !destination_cidr_blocks.is_empty() && !destinations.is_empty() {
                    static_routes.push(SegmentStaticRoute {
                        segment: segment.to_string(),
                        destination_cidr_blocks,
                        destinations,
                    });
                }
            }
            _ => {}
        }
    }
    SegmentActions {
        shares,
        service_insertions,
        static_routes,
    }
}

/// How a `share` entry's `share-with` names the segments it shares with, or `None` where
/// the entry carries none or spells it in a form this slice cannot read — such an entry
/// shares with NOTHING, since an unreadable one must not silently widen the sharing it
/// belongs to.
///
/// The published spellings are the list of segment names, the string `*` for every other
/// segment, and an object excepting some of them. Each list is walked only as far as
/// [`MAX_POLICY_SEGMENTS`] entries — the count [`validate_policy_document`] refuses a
/// document past — and only names `segments` declares are kept, each once, so what one
/// entry retains is bounded by that cap.
fn share_with(value: Option<&::serde_json::Value>, segments: &[NamedGroup]) -> Option<ShareWith> {
    let declared = |name: &&str| segments.iter().any(|group| group.name == *name);
    let kept = |names: &[::serde_json::Value]| -> BTreeSet<String> {
        names
            .iter()
            .take(MAX_POLICY_SEGMENTS)
            .filter_map(::serde_json::Value::as_str)
            .filter(declared)
            .map(str::to_string)
            .collect()
    };
    match value? {
        ::serde_json::Value::String(spelt) if spelt == POLICY_SHARE_WITH_ALL => {
            Some(ShareWith::AllExcept(BTreeSet::new()))
        }
        ::serde_json::Value::Array(names) => Some(ShareWith::Named(kept(names))),
        ::serde_json::Value::Object(object) => object
            .get(POLICY_EXCEPT)
            .and_then(::serde_json::Value::as_array)
            .map(|excepted| ShareWith::AllExcept(kept(excepted))),
        _ => None,
    }
}

/// The `attachment-policies` rules of a policy document's root object, in the order they
/// are evaluated: ascending `rule-number`. A rule the document spells without the
/// `rule-number` that orders it, or without the `action` that says what it does, describes
/// no decision and is skipped — [`validate_policy_document`] refuses such a document, which
/// is what keeps a STORED document from carrying one. Both the rule count and each rule's
/// condition count are capped there too, so this walk is bounded.
fn attachment_policies(
    root: &::serde_json::Map<String, ::serde_json::Value>,
) -> Vec<AttachmentPolicyRule> {
    let mut rules: Vec<AttachmentPolicyRule> = root
        .get(POLICY_ATTACHMENT_POLICIES)
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .take(MAX_POLICY_ATTACHMENT_POLICIES)
        .filter_map(|entry| {
            let rule_number = entry
                .get(POLICY_RULE_NUMBER)
                .and_then(::serde_json::Value::as_i64)?;
            let action = entry
                .get(POLICY_ACTION)
                .and_then(::serde_json::Value::as_object)?;
            let conditions = entry
                .get(POLICY_CONDITIONS)
                .and_then(::serde_json::Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .take(MAX_POLICY_CONDITIONS_PER_RULE)
                .filter_map(|condition| {
                    let condition_type = condition
                        .get(POLICY_TYPE)
                        .and_then(::serde_json::Value::as_str)?;
                    Some(AttachmentCondition {
                        condition_type: condition_type.to_string(),
                        operator: text_of(condition, POLICY_OPERATOR),
                        key: text_of(condition, POLICY_KEY),
                        value: text_of(condition, POLICY_VALUE),
                    })
                })
                .collect();
            // The segment the action names, by the method it names it with. An action
            // asking for neither method names no segment, so the attachment joins none.
            let segment = match action
                .get(POLICY_ASSOCIATION_METHOD)
                .and_then(::serde_json::Value::as_str)
            {
                Some(POLICY_ASSOCIATION_CONSTANT) => action
                    .get(POLICY_SEGMENT)
                    .and_then(::serde_json::Value::as_str)
                    .map(|name| SegmentAssociation::Constant(name.to_string())),
                Some(POLICY_ASSOCIATION_TAG) => action
                    .get(POLICY_TAG_VALUE_OF_KEY)
                    .and_then(::serde_json::Value::as_str)
                    .map(|key| SegmentAssociation::TagValueOfKey(key.to_string())),
                Some(_) | None => None,
            };
            Some(AttachmentPolicyRule {
                // A `rule-number` outside the `Integer` the attachment reports it through
                // is clamped to that range rather than dropped: the rule still decides,
                // and the number it reports stays inside the modelled type.
                rule_number: rule_number.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                all_conditions: entry
                    .get(POLICY_CONDITION_LOGIC)
                    .and_then(::serde_json::Value::as_str)
                    != Some(POLICY_LOGIC_OR),
                conditions,
                require_acceptance: action
                    .get(POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE)
                    .and_then(::serde_json::Value::as_bool)
                    .unwrap_or(false),
                segment,
                network_function_group: action
                    .get(POLICY_ADD_TO_NETWORK_FUNCTION_GROUP)
                    .and_then(::serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    // Evaluated lowest rule number first, so the order is fixed here rather than at every
    // attachment create. `sort_by_key` is stable, so two rules sharing a number keep the
    // order the document listed them in.
    rules.sort_by_key(|rule| rule.rule_number);
    rules
}

/// The string at `key` of a JSON object, or `None` when it carries none there.
fn text_of(value: &::serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(::serde_json::Value::as_str)
        .map(str::to_string)
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

/// A stored record that remembers the idempotency token the request which produced it
/// carried, which is what lets [`replay_or_conflict`] recognise a retry of that request.
pub trait HasClientToken {
    /// The normalised token of the request that produced this record, or `None` where it
    /// carried none.
    fn client_token(&self) -> Option<&str>;
}

/// Every record of this service whose producing operation models an idempotency token:
/// each keeps it in a `client_token` field, so one implementation serves them all.
macro_rules! client_token_records {
    ($($record:ty),+ $(,)?) => {
        $(
            impl HasClientToken for $record {
                fn client_token(&self) -> Option<&str> {
                    self.client_token.as_deref()
                }
            }
        )+
    };
}

client_token_records!(
    AttachmentRecord,
    ConnectPeerRecord,
    CoreNetworkPolicyRecord,
    CoreNetworkRecord,
    PeeringRecord,
    PrefixListAssociationRecord,
);

/// The normalisation every idempotency token of this service goes through: an EMPTY token
/// is no token at all — the modelled member starts at length zero (`smithy_query
/// shape=ClientToken`) — so a request carrying one is idempotent against nothing and the
/// record it produces is stored under no token.
pub fn normalized_client_token<Token: AsRef<str>>(token: Option<Token>) -> Option<Token> {
    token.filter(|token| !token.as_ref().is_empty())
}

/// What the refusal of a reused idempotency token says about the record the token's first
/// request already produced: how that request is named, and the resource the refusal points
/// at.
#[derive(Debug)]
pub struct ClientTokenConflict {
    /// What the first request under the token DID, as the refusal names it ("created
    /// attachment attachment-0123…"): the only part of the message that varies by kind.
    pub effect: String,
    /// The id of the resource that request produced, for `ConflictException$ResourceId`.
    pub resource_id: String,
    /// That resource's type, for `ConflictException$ResourceType`.
    pub resource_type: String,
}

/// Resolve the idempotency token of a tokened write against the records a store already
/// holds, where each record remembers that token in the field [`HasClientToken`] names.
///
/// `Ok(Some(record))` is a retry of the request that produced `record`, which the caller
/// answers from that record instead of producing a second one; `Ok(None)` is a request that
/// nothing has been stored under yet, which the caller goes on to produce; `Err` is the same
/// token asking for something else, which is refused and stores nothing. `same_request` is
/// the per-kind comparison of a stored record against the request, and `conflict` names what
/// the stored record's own creation was.
pub fn replay_or_conflict<'store, Record>(
    token: Option<&str>,
    stored: impl IntoIterator<Item = &'store Record>,
    same_request: impl Fn(&Record) -> bool,
    conflict: impl FnOnce(&Record) -> ClientTokenConflict,
) -> Result<Option<&'store Record>, ConflictException>
where
    Record: HasClientToken + 'store,
{
    replay_or_conflict_by(
        token,
        stored,
        HasClientToken::client_token,
        same_request,
        conflict,
    )
}

/// [`replay_or_conflict`] where the token a stored record was produced under is reached by
/// `token_of` rather than by [`HasClientToken`].
///
/// The walk is the same one, and it is the only one: a record can remember the token of
/// MORE than one tokened operation — an attachment remembers the token of the create that
/// minted it and the token of the call that applied the label it currently carries — and
/// each of those tokens is resolved against the records holding it rather than against the
/// one field a record's [`HasClientToken`] names.
pub fn replay_or_conflict_by<'store, Record>(
    token: Option<&str>,
    stored: impl IntoIterator<Item = &'store Record>,
    token_of: impl Fn(&Record) -> Option<&str>,
    same_request: impl Fn(&Record) -> bool,
    conflict: impl FnOnce(&Record) -> ClientTokenConflict,
) -> Result<Option<&'store Record>, ConflictException> {
    let Some(token) = normalized_client_token(token) else {
        return Ok(None);
    };
    let Some(existing) = stored
        .into_iter()
        .find(|record| token_of(record) == Some(token))
    else {
        return Ok(None);
    };
    if same_request(existing) {
        return Ok(Some(existing));
    }
    let ClientTokenConflict {
        effect,
        resource_id,
        resource_type,
    } = conflict(existing);
    Err(ConflictException {
        message: format!("client token {token:?} already {effect}, with a different request"),
        resource_id,
        resource_type,
    })
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

/// Whether two `ConnectAttachmentOptions` ask for the same thing, for the idempotency
/// comparison a repeated `ClientToken` needs: the shape is generated and carries no
/// `PartialEq`, so the comparison goes through its members (`Protocol` is its only one:
/// `smithy_query shape=ConnectAttachmentOptions`).
pub fn same_connect_options(
    left: &ConnectAttachmentOptions,
    right: &ConnectAttachmentOptions,
) -> bool {
    left.protocol == right.protocol
}

/// Whether two `VpcOptions` ask for the same thing. See [`same_connect_options`]; an
/// absent `Options` asks for the modelled defaults, which is what an absent one on the
/// other side asks for too, so two absent options are the same request.
///
/// EVERY member of `VpcOptions` carries a modelled `@default` of `false` (`smithy_query
/// shape=VpcOptions`), so a set that leaves a member out asks for that member off, and a
/// set that is absent altogether asks for all four off. Each side is therefore read as the
/// four flags it asks for — the defaults where it says nothing — rather than as "present"
/// against "absent": an absent options set and one spelling the defaults out, the empty
/// object included, are the same request. Unlike [`same_location`], where every member is
/// optional with no default and an absent structure names no location at all, no spelling
/// of `VpcOptions` can leave a flag unasked-for.
pub fn same_vpc_options(left: Option<&VpcOptions>, right: Option<&VpcOptions>) -> bool {
    let members = |options: Option<&VpcOptions>| {
        options
            .map(|options| {
                (
                    options.ipv6_support,
                    options.appliance_mode_support,
                    options.dns_support,
                    options.security_group_referencing_support,
                )
            })
            .unwrap_or_default()
    };
    members(left) == members(right)
}

/// Whether two `Bandwidth` structures carry the same speeds. See [`same_connect_options`]:
/// the shape is generated and carries no `PartialEq`, so the comparison goes through its
/// members (`UploadSpeed` and `DownloadSpeed`: `smithy_query shape=Bandwidth`).
pub fn same_bandwidth(left: &Bandwidth, right: &Bandwidth) -> bool {
    (left.upload_speed, left.download_speed) == (right.upload_speed, right.download_speed)
}

/// Whether two `Location` structures carry the same members. See [`same_connect_options`];
/// a structure that is absent altogether differs from one carrying no member, because the
/// first names no location at all while the second names an empty one.
pub fn same_location(left: Option<&Location>, right: Option<&Location>) -> bool {
    let members = |location: Option<&Location>| {
        location.map(|location| {
            (
                location.address.clone(),
                location.latitude.clone(),
                location.longitude.clone(),
            )
        })
    };
    members(left) == members(right)
}

/// Whether two `AWSLocation` structures carry the same members. See [`same_location`].
pub fn same_aws_location(left: Option<&AWSLocation>, right: Option<&AWSLocation>) -> bool {
    let members = |location: Option<&AWSLocation>| {
        location.map(|location| (location.zone.clone(), location.subnet_arn.clone()))
    };
    members(left) == members(right)
}

/// Write the value an update SUPPLIED for one member over the value a record holds,
/// answering whether that CHANGED the record.
///
/// An absent member (`None`) is one the request left out, so nothing is written for it;
/// a supplied one replaces the stored value — the empty string included, which is how
/// this service's updates document removing the information a member held — and counts as
/// a change only where the stored value differs. Answering whether anything changed is
/// what lets every updater of this service apply one rule, the one
/// [`AttachmentRecord::set_edge_locations`] applies to an attachment: a request that asks
/// for the state the resource is already in leaves the service no change to apply, so it
/// writes nothing and owes no transition.
pub fn write_supplied<T: PartialEq>(held: &mut T, supplied: Option<T>) -> bool {
    write_supplied_by(held, supplied, |supplied, held| supplied == held)
}

/// [`write_supplied`] for a member whose type is a generated shape and therefore carries
/// no `PartialEq` (see [`same_connect_options`]): `same` is what compares the supplied
/// value against the stored one.
pub fn write_supplied_by<T>(
    held: &mut T,
    supplied: Option<T>,
    same: impl FnOnce(&T, &T) -> bool,
) -> bool {
    match supplied {
        Some(value) if !same(&value, held) => {
            *held = value;
            true
        }
        _ => false,
    }
}

/// The Region of an EC2 resource ARN that must name a resource of type `resource_type`
/// (`arn:<partition>:ec2:<region>:<account>:<resource_type>/<id>`), or `None` when the
/// string is not such an ARN or names no Region.
///
/// [`arn_region`] is enough where the MODEL constrains the ARN's form, but
/// `TransitGatewayArn` and `TransitGatewayRouteTableArn` carry only a length and the
/// permissive `^[\s\S]*$` (`smithy_query shape=TransitGatewayArn`), so for those the
/// handler is what has to establish the resource an ARN names before anything is stored
/// against it.
pub fn ec2_resource_region<'a>(arn: &'a str, resource_type: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" || fields.next()?.is_empty() || fields.next()? != EC2_NAMESPACE {
        return None;
    }
    let region = fields.next()?;
    // The account segment is not judged: a peered transit gateway may belong to another
    // account of the organization.
    let _account = fields.next()?;
    let (kind, id) = fields.next()?.split_once('/')?;
    if kind != resource_type || id.is_empty() {
        return None;
    }
    (!region.is_empty()).then_some(region)
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

/// What separates the listing a token belongs to from the key offset it names inside that
/// listing. A listing is named by its operation name, which carries no `:`, so the FIRST
/// `:` after the listing name ends it and everything beyond it is the key — however many
/// of these the key itself holds.
const PAGE_TOKEN_LISTING_SEPARATOR: char = ':';

/// Encode the continuation token that resumes `listing` AFTER `last_key`.
///
/// The token is opaque to the caller: hex of a prefixed key offset into the relevant
/// `BTreeMap`, which is why the listing map is ordered. Hex, so the token survives the
/// query string a `NextToken` travels back in (`@httpQuery("nextToken")`) unescaped,
/// whatever the key holds.
///
/// `listing` is the listing the token names a position IN — each paginated operation passes
/// its own operation name — because a key offset only means anything in the key space that
/// produced it: an attachment id names no position among site ids, and a zero-padded
/// version number names none among either. Carrying the listing in the token is what lets
/// [`decode_page_token`] tell a token of THIS listing from one of another.
fn encode_page_token(listing: &str, last_key: &str) -> String {
    hex::encode(format!(
        "{PAGE_TOKEN_PREFIX}{listing}{PAGE_TOKEN_LISTING_SEPARATOR}{last_key}"
    ))
}

/// Decode a continuation token minted by [`encode_page_token`] FOR `listing`, or `None`
/// when the token is not one this service produced for that listing — the caller answers
/// that with a `ValidationException`.
///
/// Only the ONE spelling [`encode_page_token`] writes for a position names that position: a
/// token is accepted only when it is byte-identical to the token this listing would mint
/// for the key it decodes to. `hex::decode` is laxer than `hex::encode` — it reads
/// `A-F` as well as the `a-f` the minter emits — so without that check a re-cased copy of a
/// token, a string no response of this service ever carried, would resume a listing.
///
/// A token minted by a DIFFERENT listing is rejected here too, rather than resolved against
/// a key space it names no position in: the keys of one listing sort arbitrarily against
/// another's, so such a token would otherwise be answered with an empty page (when no key of
/// this listing sorts after it) or with this listing's first page again (when every key
/// does), both of them silent.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let last_key = text
        .strip_prefix(PAGE_TOKEN_PREFIX)?
        .strip_prefix(listing)?
        .strip_prefix(PAGE_TOKEN_LISTING_SEPARATOR)?;
    (encode_page_token(listing, last_key) == token).then(|| last_key.to_string())
}

/// Decode a continuation token with [`decode_page_token`], or answer the ONE refusal every
/// paginated listing of this service gives for a token it did not mint — whether no listing
/// of this service minted it or another one did.
///
/// A request carrying such a token was itself read perfectly well — it is the value of the
/// single input member `NextToken` that names no position in the listing, which is what the
/// refusal says, whether the token came from another listing of this service or from
/// nowhere this service minted — so the refusal
/// is the member failure [`REASON_FIELD_VALIDATION_FAILED`], naming [`FIELD_NEXT_TOKEN`]
/// in `Fields`, and not [`REASON_CANNOT_PARSE`], which this service answers only where the
/// TEXT it was handed could not be read at all (a resource policy document that is no
/// JSON: [`validate_resource_policy_document`]). Stated once here so no listing of this
/// service can answer another `Reason`, or another `Fields` entry, for the same input.
pub fn decode_page_token_or_reject(
    listing: &str,
    token: &str,
) -> Result<String, ValidationException> {
    decode_page_token(listing, token).ok_or_else(|| ValidationException {
        message: "NextToken does not name a position in this listing".to_string(),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: FIELD_NEXT_TOKEN.to_string(),
            message: "the token names no position in this listing".to_string(),
        }]),
    })
}

/// Take one page of `listing`'s `entries` (which must be sorted by key) starting after
/// `after`.
///
/// `max_results` is the caller's page size; absent means "every remaining item". A
/// `next_token` is emitted only when items remain after this page, so a client that
/// follows tokens sees every item exactly once.
///
/// `listing` is the same listing name the `after` key was decoded with
/// ([`decode_page_token_or_reject`]): the token minted here names a position in THIS
/// listing's key space, so only this listing accepts it back.
pub fn paginate<T>(
    listing: &str,
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
            .map(|(key, _)| encode_page_token(listing, key))
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
