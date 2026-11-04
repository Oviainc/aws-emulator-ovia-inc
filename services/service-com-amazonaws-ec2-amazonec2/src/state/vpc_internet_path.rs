// SPDX-License-Identifier: CC0-1.0

//! A VPC's ROUTE TABLES, and the ATTACHMENT an internet gateway of one carries.
//!
//! One module, because the two are one subject: "to use an internet gateway, your subnet's
//! route table must contain a route that directs internet-bound traffic to the internet
//! gateway" (Amazon VPC User Guide, "Connect to the internet using an internet gateway"), so
//! a gateway is only reachable through a route and a route only applies to a subnet through
//! an association. Keeping them together is what lets one writer state the rule that spans
//! them — a route may only point at a gateway attached to the route table's own VPC —
//! instead of two halves of it drifting apart. The internet gateway STORE itself, with its
//! id form, its bounds and its byte charge, is [`super::internet_gateways`]'s; what lives
//! here is the attachment `AttachInternetGateway` records on one.
//!
//! The route table store holds the MODELLED shape
//! ([`ComAmazonawsEc2Amazonec2State::route_tables`] holds `RouteTable`), for the reason the
//! VPC store holds a `Vpc`: `DescribeRouteTables` reports exactly what a writer stored, so
//! no two operations can disagree about one table. A route and an association live on the
//! table's own `Routes` and `Associations` members — the one place each is held, so a
//! describe reports them rather than a second copy kept beside them.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>`
//! like every other primitive there.

use super::{tag_list_bytes, ComAmazonawsEc2Amazonec2State, MAX_STORED_RECORDS_PER_STORE};
use crate::ops::types::{InternetGateway, InternetGatewayAttachment, Route, RouteTable,
    RouteTableAssociation, RouteTableAssociationState, Tag};
use crate::scaffolded::Scaffolded;

/// The id prefix of a route table (`RouteTable$RouteTableId`), which EC2 spells `rtb-<hex>`.
pub const ROUTE_TABLE_ID_PREFIX: &str = "rtb";

/// The id prefix of an association between a route table and a subnet
/// (`RouteTableAssociation$RouteTableAssociationId`), which EC2 spells `rtbassoc-<hex>`.
pub const ROUTE_TABLE_ASSOCIATION_ID_PREFIX: &str = "rtbassoc";

/// The `ResourceType` a `CreateRouteTable` tag specification may name: the table it creates,
/// spelled as the modelled `ResourceType` enum spells it
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const RESOURCE_TYPE_ROUTE_TABLE: &str = "route-table";

/// The state an internet gateway's attachment to a VPC is reported in: "for an internet
/// gateway, the state is `available` when attached to a VPC; otherwise, this value is not
/// returned" (`InternetGatewayAttachment$State`), which the
/// `DescribeInternetGatewaysRequest$Filters` doc says again of `attachment.state`
/// ("`available`. Present only if a VPC is attached"). So it is the one value this member
/// ever carries, and it is NOT one of the `AttachmentStatus` enum's four members, which
/// describe the attachment of an elastic network interface or a volume.
pub const INTERNET_GATEWAY_ATTACHMENT_STATE_AVAILABLE: &str = "available";

/// A route whose target is reachable: the `RouteState` enum's `active` member
/// (`smithy_query(query="shape",shape="RouteState")`).
pub const ROUTE_STATE_ACTIVE: &str = "active";

/// A route whose target is not available — "the specified gateway isn't attached to the
/// VPC, or the specified NAT instance has been terminated" (`Route$State`): the `RouteState`
/// enum's `blackhole` member.
pub const ROUTE_STATE_BLACKHOLE: &str = "blackhole";

/// "`CreateRouteTable` - The route was automatically created when the route table was
/// created" (`Route$Origin`): the origin of the local route every table comes with.
pub const ROUTE_ORIGIN_CREATE_ROUTE_TABLE: &str = "CreateRouteTable";

/// "`CreateRoute` - The route was manually added to the route table" (`Route$Origin`).
pub const ROUTE_ORIGIN_CREATE_ROUTE: &str = "CreateRoute";

/// The target of the route every route table comes with: "every route table contains a
/// local route for communication within the VPC", whose target the API reports as `local`
/// (Amazon VPC User Guide, "Route tables" — "Routes to IPv4 and IPv6 addresses within a
/// VPC … the target is `local`").
pub const LOCAL_ROUTE_GATEWAY_ID: &str = "local";

/// The state a route table association is reported in once it holds: the
/// `RouteTableAssociationStateCode` enum's `associated` member
/// (`smithy_query(query="shape",shape="RouteTableAssociationStateCode"`)).
pub const ROUTE_TABLE_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// How many routes one route table holds before a create must refuse.
///
/// "Routes per route table (non-propagated routes)" is 50 by default, adjustable to 1,000
/// (Amazon VPC quotas), so 50 is what an account's table holds without an increase. The
/// local route every table comes with is one of its routes and counts like any other.
pub const MAX_ROUTES_PER_ROUTE_TABLE: usize = 50;

/// EC2's error code for a request that named a route table id this account holds no table
/// for (EC2 API "Error codes": `InvalidRouteTableID.NotFound`).
pub const INVALID_ROUTE_TABLE_ID_NOT_FOUND: &str = "InvalidRouteTableID.NotFound";

/// EC2's error code for a `CreateRoute` whose target is a gateway id this account holds no
/// gateway for (EC2 API "Error codes": `InvalidGatewayID.NotFound`).
pub const INVALID_GATEWAY_ID_NOT_FOUND: &str = "InvalidGatewayID.NotFound";

/// EC2's quota code for one route table too many (EC2 API "Error codes":
/// `RouteTableLimitExceeded`).
pub const ROUTE_TABLE_LIMIT_EXCEEDED: &str = "RouteTableLimitExceeded";

/// Refuse a request that named a route table id this account holds no table for: HTTP 400
/// [`INVALID_ROUTE_TABLE_ID_NOT_FOUND`], worded the same way.
pub fn invalid_route_table_id_not_found<E>(route_table_id: &str) -> Scaffolded<E> {
    super::id_not_found(
        INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        "routeTable ID",
        route_table_id,
    )
}

/// Refuse a `CreateRoute` whose target names no gateway of the account: HTTP 400
/// [`INVALID_GATEWAY_ID_NOT_FOUND`], worded the same way.
pub fn invalid_gateway_id_not_found<E>(gateway_id: &str) -> Scaffolded<E> {
    super::id_not_found(INVALID_GATEWAY_ID_NOT_FOUND, "gateway ID", gateway_id)
}

/// Refuse a create the store has no room for: HTTP 400 [`ROUTE_TABLE_LIMIT_EXCEEDED`], the
/// answer for an [`ComAmazonawsEc2Amazonec2State::insert_route_table`] that stored nothing.
pub fn route_table_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(ROUTE_TABLE_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a route the table already holds for that destination: HTTP 400
/// [`super::ROUTE_ALREADY_EXISTS`] — the code this crate already words a duplicate route
/// with — naming the destination the request sent: "each route in a table specifies a
/// destination and a target", and one destination has one target, so a second route for it
/// is refused rather than silently replacing the first (that is `ReplaceRoute`).
pub fn route_already_exists<E>(destination: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        super::ROUTE_ALREADY_EXISTS,
        400,
        format!("The route identified by {destination} already exists."),
    )
}

/// Refuse a route the table has no room for: HTTP 400 [`super::ROUTE_LIMIT_EXCEEDED`], in
/// the sentence this crate words every quota refusal with, reporting the bound it would have
/// crossed ([`MAX_ROUTES_PER_ROUTE_TABLE`]).
pub fn route_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        super::ROUTE_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of routes has been reached \
             ({MAX_ROUTES_PER_ROUTE_TABLE})"
        ),
    )
}

/// Refuse an attachment the resource already has: HTTP 400
/// [`super::RESOURCE_ALREADY_ASSOCIATED`], the code EC2's error list gives for exactly this
/// — "the resource is already associated. For example, you might be trying to attach more
/// than one internet gateway to a VPC".
///
/// The message names the PAIR that already holds, whichever side of it the request
/// collided with: the gateway the request named when that gateway is attached somewhere,
/// and the gateway the VPC already has when the VPC is the side that is taken. A caller
/// cannot act on the refusal without being told which association is in the way.
pub fn resource_already_attached<E>(internet_gateway_id: &str, vpc_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        super::RESOURCE_ALREADY_ASSOCIATED,
        400,
        format!("resource {internet_gateway_id} is already attached to network {vpc_id}"),
    )
}

/// Refuse an association the subnet already has: HTTP 400
/// [`super::RESOURCE_ALREADY_ASSOCIATED`] as well, since "each subnet in your VPC must be
/// associated with a route table" and with exactly one — changing which one is
/// `ReplaceRouteTableAssociation`, not a second `AssociateRouteTable`. The message names
/// the association that is in the way, the way the attachment refusal above does.
pub fn subnet_already_associated<E>(subnet_id: &str, route_table_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        super::RESOURCE_ALREADY_ASSOCIATED,
        400,
        format!("resource {subnet_id} is already associated with route table {route_table_id}"),
    )
}

/// The caller-chosen bytes one stored route table holds: its tags, and nothing else — the
/// ids, the owning account, the VPC id and the local route are derived by this module, so
/// they are fixed-size.
///
/// The ONE definition of that charge, the way [`super::network_acl_payload_bytes`] is for a
/// network ACL: the insert below charges exactly it and the removal gives back exactly it,
/// whoever asked for the record — which is what keeps the two sides symmetric, a table the
/// SERVICE minted carrying no tags and so costing nothing either way.
pub fn route_table_payload_bytes(tags: &[Tag]) -> usize {
    tag_list_bytes(tags)
}

// A route holds no bytes the byte budget counts, which is why there is no
// `route_payload_bytes` beside [`route_table_payload_bytes`]: a route's destination is a
// CIDR block or the id of a prefix list this account holds, each bounded in length by its
// own form, and every target is an id of a stored resource. How many of them there can be
// is bounded by `MAX_ROUTES_PER_ROUTE_TABLE` per table and by the table ceiling, so routes
// are finite without a charge against `MAX_STORED_BYTES` — which also keeps the charge and
// the release symmetric for the local route a table comes with, which no caller asked for.

/// Does `internet_gateway` attach the VPC `vpc_id`?
///
/// The one reading of an attachment, so the delete path that treats an attached gateway as
/// a dependency of its VPC ([`ComAmazonawsEc2Amazonec2State::vpc_dependencies`]) and the
/// attach that refuses a VPC already taken ask the same question of the same member.
pub fn internet_gateway_attaches_vpc(internet_gateway: &InternetGateway, vpc_id: &str) -> bool {
    internet_gateway
        .attachments
        .iter()
        .flatten()
        .any(|attachment| attachment.vpc_id.as_deref() == Some(vpc_id))
}

/// What ONE route table is stored with, beyond everything the service derives (the id, the
/// owning account, and the local route and main association below).
#[derive(Debug, Clone)]
pub struct RouteTableSpec {
    /// "The ID of the VPC" (`RouteTable$VpcId`) the table routes for.
    pub vpc_id: String,
    /// The IPv4 CIDR blocks the VPC has associated, each of which the table comes with a
    /// local route for: "every route table contains a local route for communication within
    /// the VPC" (Amazon VPC User Guide, "Route tables").
    pub local_cidr_blocks: Vec<String>,
    /// The IPv6 CIDR blocks the VPC has associated, each of which the table comes with a
    /// local route of its own: the same guide's "routes to IPv4 and IPv6 addresses within a
    /// VPC" row, whose target is `local` too — so a VPC with an IPv6 block routes it
    /// locally as well as its IPv4 blocks.
    pub local_ipv6_cidr_blocks: Vec<String>,
    /// Whether this is the VPC's MAIN route table — "your VPC automatically comes with a
    /// main route table" (same guide), the one a subnet with no association of its own
    /// uses. A main table is reported with the implicit association
    /// `RouteTableAssociation$Main` marks, and it carries no tags of its own, the service
    /// having asked for it.
    pub main: bool,
    /// "The tags to assign to the route table"
    /// (`CreateRouteTableRequest$TagSpecifications`).
    pub tags: Vec<Tag>,
}

/// What ONE route in a route table points at.
///
/// "Each route in a route table specifies a destination and a target" (Amazon VPC User
/// Guide, "Route tables"), and `Route` carries one member per kind of target; this is that
/// choice as a value, so a writer names the target once and [`RouteSpec::into_route`]
/// fills the member that kind belongs in. Only the kinds whose resources this store holds
/// are here: a target the emulator has no resource store for cannot be resolved, so the
/// operation that would store it answers that it is not implemented rather than storing a
/// route to nothing.
#[derive(Debug, Clone)]
pub enum RouteTarget {
    /// "The ID of a gateway attached to your VPC" (`Route$GatewayId`) — an internet
    /// gateway, a virtual private gateway, or the `local` target of the route every table
    /// comes with.
    Gateway(String),
    /// "The ID of a NAT gateway" (`Route$NatGatewayId`).
    NatGateway(String),
    /// "The ID of a NAT instance in your VPC" (`Route$InstanceId`), reported with "the ID
    /// of Amazon Web Services account that owns the instance" (`Route$InstanceOwnerId`).
    Instance(String),
    /// "The ID of the network interface" (`Route$NetworkInterfaceId`).
    NetworkInterface(String),
    /// "The ID of a transit gateway" (`Route$TransitGatewayId`).
    TransitGateway(String),
    /// "The ID of the carrier gateway" (`Route$CarrierGatewayId`).
    CarrierGateway(String),
    /// "The ID of the local gateway" (`Route$LocalGatewayId`).
    LocalGateway(String),
    /// "The ID of a VPC peering connection" (`Route$VpcPeeringConnectionId`).
    VpcPeeringConnection(String),
}

/// What ONE route is stored with: where it matches and what it points at.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    /// "The IPv4 CIDR block used for the destination match" (`Route$DestinationCidrBlock`).
    pub destination_cidr_block: Option<String>,
    /// "The IPv6 CIDR block used for the destination match"
    /// (`Route$DestinationIpv6CidrBlock`).
    pub destination_ipv6_cidr_block: Option<String>,
    /// "The prefix of the Amazon Web Services service" (`Route$DestinationPrefixListId`).
    pub destination_prefix_list_id: Option<String>,
    /// What the route points at.
    pub target: RouteTarget,
    /// "Describes how the route was created" (`Route$Origin`):
    /// [`ROUTE_ORIGIN_CREATE_ROUTE_TABLE`] for the local route a table comes with,
    /// [`ROUTE_ORIGIN_CREATE_ROUTE`] for one a caller added.
    pub origin: &'static str,
}

impl RouteSpec {
    /// The local route a route table comes with for one of its VPC's IPv4 CIDR blocks:
    /// "every route table contains a local route for communication within the VPC", whose
    /// target is [`LOCAL_ROUTE_GATEWAY_ID`] and whose origin is the create of the table
    /// itself.
    pub fn local(cidr_block: &str) -> Self {
        Self {
            destination_cidr_block: Some(cidr_block.to_string()),
            destination_ipv6_cidr_block: None,
            destination_prefix_list_id: None,
            target: RouteTarget::Gateway(LOCAL_ROUTE_GATEWAY_ID.to_string()),
            origin: ROUTE_ORIGIN_CREATE_ROUTE_TABLE,
        }
    }

    /// The local route a route table comes with for one of its VPC's IPv6 CIDR blocks: the
    /// same rule one address family along — "routes to IPv4 and IPv6 addresses within a
    /// VPC" have the target `local` (Amazon VPC User Guide, "Route tables") — so it differs
    /// from [`Self::local`] only in which destination member carries the block.
    pub fn local_ipv6(cidr_block: &str) -> Self {
        Self {
            destination_cidr_block: None,
            destination_ipv6_cidr_block: Some(cidr_block.to_string()),
            destination_prefix_list_id: None,
            target: RouteTarget::Gateway(LOCAL_ROUTE_GATEWAY_ID.to_string()),
            origin: ROUTE_ORIGIN_CREATE_ROUTE_TABLE,
        }
    }

    /// The destination this route matches on, as one string — the member it carries,
    /// whichever of the three that is.
    ///
    /// It is what identifies a route inside its table ("the route identified by <x>"), so
    /// the duplicate check, the refusal's message and the `route.destination-*` filters all
    /// read it from here.
    pub fn destination(&self) -> &str {
        self.destination_cidr_block
            .as_deref()
            .or(self.destination_ipv6_cidr_block.as_deref())
            .or(self.destination_prefix_list_id.as_deref())
            .unwrap_or_default()
    }

    /// The route as it is stored and reported: the destination it matches, the one target
    /// member its kind belongs in, and the state and origin of the route.
    ///
    /// `state` is the caller's to state, because what makes a route `active` rather than
    /// [`ROUTE_STATE_BLACKHOLE`] is whether its target is available — a question about the
    /// target, not about the route.
    pub fn into_route(self, state: &str) -> Route {
        let mut route = Route {
            destination_cidr_block: self.destination_cidr_block,
            destination_ipv6_cidr_block: self.destination_ipv6_cidr_block,
            destination_prefix_list_id: self.destination_prefix_list_id,
            egress_only_internet_gateway_id: None,
            gateway_id: None,
            instance_id: None,
            instance_owner_id: None,
            nat_gateway_id: None,
            transit_gateway_id: None,
            local_gateway_id: None,
            carrier_gateway_id: None,
            network_interface_id: None,
            origin: Some(self.origin.to_string()),
            state: Some(state.to_string()),
            vpc_peering_connection_id: None,
            core_network_arn: None,
            odb_network_arn: None,
            ip_address: None,
        };
        match self.target {
            RouteTarget::Gateway(gateway_id) => route.gateway_id = Some(gateway_id),
            RouteTarget::NatGateway(nat_gateway_id) => route.nat_gateway_id = Some(nat_gateway_id),
            RouteTarget::Instance(instance_id) => {
                route.instance_id = Some(instance_id);
                // "The ID of Amazon Web Services account that owns the instance"
                // (`Route$InstanceOwnerId`): this emulator serves one account.
                route.instance_owner_id = Some(::engine::emulator_account_id().to_string());
            }
            RouteTarget::NetworkInterface(network_interface_id) => {
                route.network_interface_id = Some(network_interface_id)
            }
            RouteTarget::TransitGateway(transit_gateway_id) => {
                route.transit_gateway_id = Some(transit_gateway_id)
            }
            RouteTarget::CarrierGateway(carrier_gateway_id) => {
                route.carrier_gateway_id = Some(carrier_gateway_id)
            }
            RouteTarget::LocalGateway(local_gateway_id) => {
                route.local_gateway_id = Some(local_gateway_id)
            }
            RouteTarget::VpcPeeringConnection(connection_id) => {
                route.vpc_peering_connection_id = Some(connection_id)
            }
        }
        route
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The VPC the internet gateway `internet_gateway_id` is attached to, if it is attached
    /// to one.
    ///
    /// A gateway attaches at most one VPC — an attach is refused while it holds one — so
    /// this answers the attachment rather than one of several.
    pub fn internet_gateway_attached_vpc(&self, internet_gateway_id: &str) -> Option<String> {
        self.internet_gateways
            .get(internet_gateway_id)?
            .attachments
            .iter()
            .flatten()
            .find_map(|attachment| attachment.vpc_id.clone())
    }

    /// The internet gateway attached to the VPC `vpc_id`, if the VPC has one.
    ///
    /// "You can attach only one internet gateway to a VPC at a time" (Amazon VPC User
    /// Guide, "Connect to the internet using an internet gateway"), so the VPC has at most
    /// one and this is the one an attach has to make room for and a route may point at.
    pub fn internet_gateway_of_vpc(&self, vpc_id: &str) -> Option<String> {
        self.internet_gateways
            .iter()
            .find(|(_, internet_gateway)| internet_gateway_attaches_vpc(internet_gateway, vpc_id))
            .map(|(internet_gateway_id, _)| internet_gateway_id.clone())
    }

    /// Record that the internet gateway `internet_gateway_id` is attached to the VPC
    /// `vpc_id`, and answer whether it was recorded.
    ///
    /// The one writer of an `InternetGateway$Attachments`, so every reader of an attachment
    /// reads the same record and the state it is reported in
    /// ([`INTERNET_GATEWAY_ATTACHMENT_STATE_AVAILABLE`]) is written in one place. The
    /// record holds no caller-chosen bytes — both sides of the pair are ids this service
    /// minted — so there is nothing to charge against the byte budget, and the gateway's own
    /// charge stays exactly what [`Self::insert_internet_gateway`] took.
    ///
    /// `false`, with nothing written, for a gateway the store does not hold.
    pub fn attach_internet_gateway(&mut self, internet_gateway_id: &str, vpc_id: &str) -> bool {
        let Some(internet_gateway) = self.internet_gateways.get_mut(internet_gateway_id) else {
            return false;
        };
        internet_gateway
            .attachments
            .get_or_insert_with(Vec::new)
            .push(InternetGatewayAttachment {
                state: Some(INTERNET_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
                vpc_id: Some(vpc_id.to_string()),
            });
        true
    }

    /// Whether another route table fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// create asks first so it can refuse instead of overshooting.
    pub fn has_route_table_capacity(&self) -> bool {
        self.route_tables.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a route table and answer the `RouteTableId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller
    /// — the id, the owning account, the local route the table comes with for each of its
    /// VPC's CIDR blocks, and the implicit association a MAIN table is reported with — so
    /// two writers cannot report the same table differently.
    ///
    /// The caller-chosen bytes the table holds ([`route_table_payload_bytes`]) are
    /// charged against the store's byte budget here, so no writer can store them uncharged;
    /// a MAIN table carries no tags, so it costs nothing.
    ///
    /// `None`, with NOTHING stored, when the store is at its table ceiling or the bytes
    /// this one would hold would cross [`super::MAX_STORED_BYTES`].
    pub fn insert_route_table(&mut self, spec: RouteTableSpec) -> Option<String> {
        let charged_bytes = route_table_payload_bytes(&spec.tags);
        if !self.has_route_table_capacity() || self.would_exceed_stored_bytes(charged_bytes) {
            return None;
        }
        let route_table_id = self.next_id(ROUTE_TABLE_ID_PREFIX);
        // "Indicates whether this is the main route table"
        // (`RouteTableAssociation$Main`): the main table is reported with one association
        // that names no subnet — the implicit association every subnet without one of its
        // own uses ("a subnet ID is not returned for an implicit association" —
        // `RouteTableAssociation$SubnetId`).
        let associations = if spec.main {
            let association_id = self.next_id(ROUTE_TABLE_ASSOCIATION_ID_PREFIX);
            vec![RouteTableAssociation {
                main: Some(true),
                route_table_association_id: Some(association_id),
                route_table_id: Some(route_table_id.clone()),
                subnet_id: None,
                gateway_id: None,
                public_ipv4_pool: None,
                association_state: Some(associated_state()),
            }]
        } else {
            Vec::new()
        };
        let route_table = RouteTable {
            associations: Some(associations),
            // "Any virtual private gateway (VGW) propagating routes"
            // (`RouteTable$PropagatingVgws`): none until `EnableVgwRoutePropagation` says
            // so.
            propagating_vgws: Some(Vec::new()),
            route_table_id: Some(route_table_id.clone()),
            routes: Some(Vec::new()),
            tags: Some(spec.tags),
            vpc_id: Some(spec.vpc_id),
            // "The ID of the Amazon Web Services account that owns the route table"
            // (`RouteTable$OwnerId`): this emulator serves one account.
            owner_id: Some(::engine::emulator_account_id().to_string()),
        };
        self.add_stored_bytes(charged_bytes);
        self.route_tables
            .insert(route_table_id.clone(), route_table);
        for cidr_block in spec.local_cidr_blocks {
            self.insert_route(&route_table_id, RouteSpec::local(&cidr_block));
        }
        for cidr_block in spec.local_ipv6_cidr_blocks {
            self.insert_route(&route_table_id, RouteSpec::local_ipv6(&cidr_block));
        }
        Some(route_table_id)
    }

    /// Give the VPC `vpc_id` the main route table it comes with, and answer that table's
    /// id.
    ///
    /// "Your VPC automatically comes with a main route table that you can modify" (Amazon
    /// VPC User Guide, "Route tables"), carrying the local route for the VPC's own CIDR
    /// block. Called from [`Self::insert_vpc`], which is what makes it true of every VPC
    /// this store holds — so the block is the IPv4 one the VPC is created with, an IPv6
    /// block being associated only after the VPC exists.
    ///
    /// `None`, with nothing stored, when [`Self::insert_route_table`] refuses: the store is
    /// at its route-table ceiling.
    pub fn insert_main_route_table(&mut self, vpc_id: &str, cidr_block: &str) -> Option<String> {
        self.insert_route_table(RouteTableSpec {
            vpc_id: vpc_id.to_string(),
            local_cidr_blocks: vec![cidr_block.to_string()],
            local_ipv6_cidr_blocks: Vec::new(),
            main: true,
            tags: Vec::new(),
        })
    }

    /// Whether the route table `route_table_id` already holds a route for `destination`.
    ///
    /// One destination has one target, so this is what a create asks before storing a
    /// second route for it — read through [`RouteSpec::destination`] on one side and the
    /// stored route's own members on the other, so the two readings cannot differ.
    pub fn route_table_holds_destination(&self, route_table_id: &str, destination: &str) -> bool {
        self.route_tables
            .get(route_table_id)
            .and_then(|route_table| route_table.routes.as_deref())
            .is_some_and(|routes| {
                routes.iter().any(|route| {
                    [
                        route.destination_cidr_block.as_deref(),
                        route.destination_ipv6_cidr_block.as_deref(),
                        route.destination_prefix_list_id.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|held| held == destination)
                })
            })
    }

    /// How many routes the route table `route_table_id` holds, which is what
    /// [`MAX_ROUTES_PER_ROUTE_TABLE`] bounds.
    pub fn route_count(&self, route_table_id: &str) -> usize {
        self.route_tables
            .get(route_table_id)
            .and_then(|route_table| route_table.routes.as_deref())
            .map_or(0, <[Route]>::len)
    }

    /// Store the route `spec` describes in the route table `route_table_id`, and answer
    /// whether it was stored.
    ///
    /// The one place a route enters a stored table, so `CreateRoute`, the local route a
    /// table comes with and the default route the account's default VPC comes with agree on
    /// what a route looks like and on what it costs the store's byte budget. Routes are
    /// kept in the order they were added, which puts the local route a table comes with
    /// first — the order `DescribeRouteTables` reports them in.
    ///
    /// The route is stored [`ROUTE_STATE_ACTIVE`]: a writer resolves the target first and
    /// refuses a target that is not there, so no route is stored pointing at nothing.
    ///
    /// `false`, with NOTHING written, for a table the store does not hold or one already at
    /// [`MAX_ROUTES_PER_ROUTE_TABLE`] routes — the bound a caller must map onto the refusal
    /// its operation answers ([`route_limit_exceeded`]).
    pub fn insert_route(&mut self, route_table_id: &str, spec: RouteSpec) -> bool {
        if !self.route_tables.contains_key(route_table_id)
            || self.route_count(route_table_id) >= MAX_ROUTES_PER_ROUTE_TABLE
        {
            return false;
        }
        let route = spec.into_route(ROUTE_STATE_ACTIVE);
        let Some(route_table) = self.route_tables.get_mut(route_table_id) else {
            return false;
        };
        route_table.routes.get_or_insert_with(Vec::new).push(route);
        true
    }

    /// Record that the subnet `subnet_id` is associated with the route table
    /// `route_table_id`, and answer the `RouteTableAssociationId` it was given.
    ///
    /// The one writer of an explicit `RouteTable$Associations` entry, so every reader of an
    /// association reads the same record. The association id is minted out of the store's
    /// own counter and the record holds no caller-chosen bytes, so there is nothing to
    /// charge against the byte budget.
    ///
    /// `None`, with nothing written, for a table the store does not hold.
    pub fn associate_subnet_with_route_table(
        &mut self,
        route_table_id: &str,
        subnet_id: &str,
    ) -> Option<String> {
        if !self.route_tables.contains_key(route_table_id) {
            return None;
        }
        let association_id = self.next_id(ROUTE_TABLE_ASSOCIATION_ID_PREFIX);
        let route_table = self.route_tables.get_mut(route_table_id)?;
        route_table
            .associations
            .get_or_insert_with(Vec::new)
            .push(RouteTableAssociation {
                // An explicit association is not the main one, whichever table it is with:
                // "indicates whether this is the main route table"
                // (`RouteTableAssociation$Main`).
                main: Some(false),
                route_table_association_id: Some(association_id.clone()),
                route_table_id: Some(route_table_id.to_string()),
                subnet_id: Some(subnet_id.to_string()),
                gateway_id: None,
                public_ipv4_pool: None,
                association_state: Some(associated_state()),
            });
        Some(association_id)
    }

    /// The explicit association the subnet `subnet_id` already has, as the route table it
    /// is with, if it has one.
    ///
    /// "Each subnet in your VPC must be associated with a route table", and with exactly
    /// one, so this is what an associate asks before making a second one.
    pub fn route_table_associated_with_subnet(&self, subnet_id: &str) -> Option<String> {
        self.route_tables
            .iter()
            .find(|(_, route_table)| {
                route_table
                    .associations
                    .iter()
                    .flatten()
                    .any(|association| association.subnet_id.as_deref() == Some(subnet_id))
            })
            .map(|(route_table_id, _)| route_table_id.clone())
    }

    /// Drop every association that names the subnet `subnet_id`, whichever table holds it —
    /// what a subnet's removal owes, since an association is the pair and cannot outlive
    /// either side of it. The records hold no caller-supplied bytes, so there is nothing to
    /// give back to the budget.
    pub fn disassociate_subnet_from_route_tables(&mut self, subnet_id: &str) {
        for route_table in self.route_tables.values_mut() {
            if let Some(associations) = route_table.associations.as_mut() {
                associations
                    .retain(|association| association.subnet_id.as_deref() != Some(subnet_id));
            }
        }
    }

    /// Remove the route table `route_table_id` and answer it as it was last reported, or
    /// `None` when the store holds no such table.
    ///
    /// The bytes the record held go back to the store's budget on exactly the terms
    /// [`Self::insert_route_table`] took them on — the table's tags, routes holding none —
    /// so a table that is created and deleted leaves the budget where it started. A table's
    /// routes and associations go with it: each is a resource OF the table, reachable only
    /// through it.
    pub fn remove_route_table(&mut self, route_table_id: &str) -> Option<RouteTable> {
        let route_table = self.route_tables.remove(route_table_id)?;
        // The `CreateRouteTable` `ClientToken` that minted this table, if one did, goes
        // with it: a token left naming a table the store no longer holds would replay into
        // a dangling id.
        self.forget_route_table_client_tokens_of(route_table_id);
        self.release_stored_bytes(route_table_payload_bytes(
            route_table.tags.as_deref().unwrap_or_default(),
        ));
        Some(route_table)
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The route table the `CreateRouteTable` `ClientToken` `client_token` already minted,
    /// if one did.
    ///
    /// A READ: a token that has minted nothing is not recorded here. The map is kept in
    /// step by [`Self::forget_route_table_client_tokens_of`], so a token never names a
    /// table the store no longer holds, and the caller resolves it against
    /// [`Self::route_tables`] either way. The same contract
    /// [`Self::network_acl_minted_by_client_token`] has for a network ACL.
    pub fn route_table_minted_by_client_token(&self, client_token: &str) -> Option<String> {
        self.route_table_client_tokens.get(client_token).cloned()
    }

    /// Record that the `ClientToken` `client_token` minted the route table
    /// `route_table_id`, so a replay of that request reports that table instead of creating
    /// a second one.
    ///
    /// The one writer of [`Self::route_table_client_tokens`]. The token's own bytes are
    /// bounded by [`super::MAX_CLIENT_TOKEN_CHARS`], which the create holds it to, and the
    /// map holds at most one entry per stored table.
    pub fn record_route_table_client_token(&mut self, client_token: String, route_table_id: &str) {
        self.route_table_client_tokens
            .insert(client_token, route_table_id.to_string());
    }

    /// Drop every `ClientToken` entry that names the route table `route_table_id` — what
    /// the removal of a table owes, since a token that names a table the store no longer
    /// holds could otherwise replay into a dangling id.
    pub fn forget_route_table_client_tokens_of(&mut self, route_table_id: &str) {
        self.route_table_client_tokens
            .retain(|_, minted| minted.as_str() != route_table_id);
    }
}

/// The state an association is reported in: "the state of the association"
/// (`RouteTableAssociation$AssociationState`), which for one this service holds is always
/// [`ROUTE_TABLE_ASSOCIATION_STATE_ASSOCIATED`] — an association is recorded once it holds,
/// and there is no window in which it is on its way in or out.
///
/// The ONE place that is built, so the main association a table comes with and an explicit
/// one a caller made cannot be reported differently.
fn associated_state() -> RouteTableAssociationState {
    RouteTableAssociationState {
        state: Some(ROUTE_TABLE_ASSOCIATION_STATE_ASSOCIATED.to_string()),
        status_message: None,
    }
}
