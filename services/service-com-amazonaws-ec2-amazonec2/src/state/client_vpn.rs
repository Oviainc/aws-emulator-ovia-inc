// SPDX-License-Identifier: CC0-1.0

//! Client VPN endpoints, the target networks associated with them, and the routes of their
//! route tables.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`: the stores, their
//! id forms, their refusals and the clock-owed `associating` -> `associated` and
//! `creating` -> `active` moves are one subject, and keeping them in their own file leaves
//! `state.rs` readable — and leaves this fix out of the end of that file, which is where
//! every concurrently landing EC2 fix inserts. Everything here is re-exported by
//! `crate::state`, so a handler reaches it as
//! `crate::state::require_known_client_vpn_endpoint` exactly as it reaches the helpers that
//! live in the parent module.
//!
//! The stores themselves are fields of [`ComAmazonawsEc2Amazonec2State`]
//! (`client_vpn_endpoints`, `client_vpn_target_networks`, `client_vpn_routes`), because the
//! state struct is one struct; it is only the code that reads and writes them that lives
//! here.

use super::*;

/// The id prefix EC2 mints a Client VPN endpoint with: `cvpn-endpoint-`, as the AWS Client
/// VPN documentation's own ids read (for example `cvpn-endpoint-0ac3a1abbccba1234`). The
/// model puts no pattern on `ClientVpnEndpointId`
/// (`smithy_query(query="shape",shape="ClientVpnEndpointId")` is a plain `String`), so the
/// prefix is the one thing that says which resource an id names.
///
/// The prefix is what an id of this kind READS as; it is not a constraint this surface
/// enforces, because Client VPN answers one refusal
/// ([`invalid_client_vpn_endpoint_id_not_found`]) for every value it cannot resolve — see
/// [`require_client_vpn_endpoint`]. So nothing here refuses an id for its FORM, and the
/// prefix serves the mint and the id spaces it keeps apart.
pub const CLIENT_VPN_ENDPOINT_ID_PREFIX: &str = "cvpn-endpoint";

/// The id prefix EC2 mints a target network association with: `cvpn-assoc-`, as the AWS
/// documentation's own association ids read (for example `cvpn-assoc-0c9a06dc85b3e9bc1`).
pub const CLIENT_VPN_TARGET_NETWORK_ASSOCIATION_ID_PREFIX: &str = "cvpn-assoc";

/// `InvalidClientVpnEndpointId.NotFound` (HTTP 400): the request named a Client VPN
/// endpoint id this account holds no endpoint for.
pub const INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND: &str = "InvalidClientVpnEndpointId.NotFound";

/// "associating" — the state a target network association starts in
/// (`AssociationStatusCode`), which `AssociateClientVpnTargetNetwork` answers with.
pub const ASSOCIATION_STATUS_ASSOCIATING: &str = "associating";

/// "associated" — the state the clock owes an `associating` target network association
/// ([`TRANSITION_DELAY`] later, through
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const ASSOCIATION_STATUS_ASSOCIATED: &str = "associated";

/// "pending-associate" — "The Client VPN endpoint has been created but no target networks
/// have been associated" (`ClientVpnEndpointStatusCode`), where an endpoint sits until one
/// is.
pub const CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE: &str = "pending-associate";

/// The most characters of a refused `ClientVpnEndpointId` its message repeats back.
///
/// The message names the endpoint the request could not resolve, and that is the request's own
/// bytes — `ClientVpnEndpointId` is a bare string the model puts no `@length` on, and this
/// surface refuses a value of any form by LOOKUP rather than by its form — so the echo is
/// bounded here the way [`MAX_ECHOED_COIP_POOL_ID_CHARS`] bounds a refused pool id's. Far
/// above the longest endpoint id this service mints ([`CLIENT_VPN_ENDPOINT_ID_PREFIX`] plus 17
/// digits), so a real id is always named in full.
pub const MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS: usize = 128;

/// Refuse a request that named a Client VPN endpoint id this account holds no endpoint for:
/// HTTP 400 [`INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND`], worded as the Client VPN surface
/// words it — `Endpoint <id> does not exist`, naming the id the request sent, of which the
/// first [`MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS`] characters are named. That is not
/// the `The <kind> ID '<id>' does not exist` wording [`id_not_found`] writes for the
/// resources whose refusal reads that way, so this one is built here rather than through it.
pub fn invalid_client_vpn_endpoint_id_not_found<E>(id: &str) -> Scaffolded<E> {
    let echoed: String = id
        .chars()
        .take(MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS)
        .collect();
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND,
        400,
        format!("Endpoint {echoed} does not exist"),
    )
}

/// The Client VPN endpoint `id` names, or the one refusal this surface answers for a value it
/// holds no endpoint for: HTTP 400 [`INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND`].
///
/// Resolution by LOOKUP alone, with no id-form arm in front of it: the model puts no pattern
/// on the member (`smithy_query(query="shape",shape="ClientVpnEndpointId")` is a plain
/// `String`), so no id form is a declared constraint of it, and AWS's EC2 error catalogue
/// documents `InvalidClientVpnEndpointId.NotFound` for an endpoint it cannot resolve and no
/// `.Malformed` counterpart to tell "not an id of this kind" from "no such endpoint" with.
/// So a value of any form that this account holds no endpoint for is answered `Endpoint <id>
/// does not exist`.
///
/// That is what EVERY operation taking a `ClientVpnEndpointId` resolves it through, the ones
/// that report what hangs off an endpoint as much as the ones that mutate it:
/// [`require_known_client_vpn_endpoint`] is this lookup for a caller that needs no endpoint
/// back. One id, one refusal, whichever operation was called.
pub fn require_client_vpn_endpoint<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<&'store crate::ops::types::ClientVpnEndpoint, Scaffolded<E>> {
    store
        .client_vpn_endpoints
        .get(id)
        .ok_or_else(|| invalid_client_vpn_endpoint_id_not_found(id))
}

/// Resolve a `ClientVpnEndpointId` for an operation that only needs to know the endpoint
/// EXISTS — the operations that REPORT what hangs off one — by the same lookup
/// [`require_client_vpn_endpoint`] does, so an id the account holds no endpoint for is
/// refused the one way this surface refuses it whatever form the value takes.
///
/// It is the reading half of that function and nothing more: the two are named apart only so
/// a reporting operation does not have to borrow an endpoint it never looks at.
pub fn require_known_client_vpn_endpoint<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_client_vpn_endpoint::<E>(store, id).map(|_| ())
}

/// One target network association: the modelled [`crate::ops::types::TargetNetwork`] every
/// operation that reports one answers with, plus the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// of this module holds one: a member a writer sets round-trips through
/// `DescribeClientVpnTargetNetworks` without a field-by-field copy that can silently drop
/// one.
#[derive(Debug, Clone)]
pub struct ClientVpnTargetNetworkRecord {
    /// The association as the model shapes it, carrying the status reached so far.
    pub view: crate::ops::types::TargetNetwork,
    /// The `AssociationStatusCode` the clock owes this association while it is still
    /// transient (`associating` -> `associated`); `None` once it has been applied.
    pub pending: Option<PendingStateMove>,
}

/// What one target network association is written with: the endpoint it associates a
/// network with, the subnet that is the target network, the VPC that subnet is in, and the
/// security groups applied to the association.
///
/// Every member the service itself owns — the association id and the status it starts in —
/// is derived by [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_target_network`] rather
/// than taken from a caller, so two writers cannot report one association differently.
#[derive(Debug, Clone)]
pub struct ClientVpnTargetNetworkSpec {
    /// "The ID of the Client VPN endpoint with which the target network is associated"
    /// (`TargetNetwork$ClientVpnEndpointId`).
    pub client_vpn_endpoint_id: String,
    /// "The ID of the subnet specified as the target network"
    /// (`TargetNetwork$TargetNetworkId`).
    pub target_network_id: String,
    /// "The ID of the VPC in which the target network (subnet) is located"
    /// (`TargetNetwork$VpcId`).
    pub vpc_id: String,
    /// "The IDs of the security groups applied to the target network association"
    /// (`TargetNetwork$SecurityGroups`).
    pub security_groups: Vec<String>,
}

/// The caller-chosen bytes one stored target network association holds: the ids of the
/// resources it joins and the security groups applied to it, on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs. Everything else on the
/// record — its own id and its status — is derived here, so it is fixed-size.
///
/// The ONE definition of that charge, the way
/// [`local_gateway_vif_group_association_payload_bytes`] is for its own store: the insert
/// charges exactly it, and a removal path must give back the same number.
pub fn client_vpn_target_network_payload_bytes(view: &crate::ops::types::TargetNetwork) -> usize {
    let security_group_bytes = view
        .security_groups
        .iter()
        .flatten()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(view.client_vpn_endpoint_id.as_deref().map_or(0, str::len))
        .saturating_add(view.target_network_id.as_deref().map_or(0, str::len))
        .saturating_add(view.vpc_id.as_deref().map_or(0, str::len))
        .saturating_add(security_group_bytes)
}

/// The caller-chosen bytes one stored Client VPN endpoint holds: its description, the client
/// CIDR block it assigns addresses from, the VPC and security groups it is attached to, the
/// server certificate it presents and its tags, on top of that same fixed overhead.
pub fn client_vpn_endpoint_payload_bytes(view: &crate::ops::types::ClientVpnEndpoint) -> usize {
    let security_group_bytes = view
        .security_group_ids
        .iter()
        .flatten()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(view.description.as_deref().map_or(0, str::len))
        .saturating_add(view.client_cidr_block.as_deref().map_or(0, str::len))
        .saturating_add(view.vpc_id.as_deref().map_or(0, str::len))
        .saturating_add(view.server_certificate_arn.as_deref().map_or(0, str::len))
        .saturating_add(security_group_bytes)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Client VPN endpoint fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// writer asks this BEFORE inserting, because nothing here reclaims a stored endpoint.
    pub fn has_client_vpn_endpoint_capacity(&self) -> bool {
        self.client_vpn_endpoints.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another target network association fits inside that same ceiling.
    pub fn has_client_vpn_target_network_capacity(&self) -> bool {
        self.client_vpn_target_networks.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a Client VPN endpoint and answer the `ClientVpnEndpointId` it was given — or
    /// `None` when one of the store's bounds would be crossed and NOTHING was stored.
    ///
    /// The seam `CreateClientVpnEndpoint` is to call — still a scaffold, so today only the
    /// crate's tests seed through it; it is not dead code.
    ///
    /// The id is minted here rather than taken from `view`, exactly as
    /// [`Self::insert_ipam_internet_registry_association`] mints an association's, so no
    /// path can store an endpoint under an id this service could not have issued. An
    /// endpoint is stored with no target network associated, which is what
    /// `pending-associate` means ("The Client VPN endpoint has been created but no target
    /// networks have been associated" — `ClientVpnEndpointStatusCode`), so a `view` that
    /// names no status gets that one.
    pub fn insert_client_vpn_endpoint(
        &mut self,
        mut view: crate::ops::types::ClientVpnEndpoint,
    ) -> Option<String> {
        let charge = client_vpn_endpoint_payload_bytes(&view);
        if !self.has_client_vpn_endpoint_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let endpoint_id = self.next_id(CLIENT_VPN_ENDPOINT_ID_PREFIX);
        view.client_vpn_endpoint_id = Some(endpoint_id.clone());
        if view.status.is_none() {
            view.status = Some(crate::ops::types::ClientVpnEndpointStatus {
                code: Some(CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE.to_string()),
                message: None,
            });
        }
        self.add_stored_bytes(charge);
        self.client_vpn_endpoints.insert(endpoint_id.clone(), view);
        Some(endpoint_id)
    }

    /// Associate one target network with a Client VPN endpoint, and answer the association
    /// as its writer reports it — or `None` when one of the store's bounds would be crossed
    /// and NOTHING was stored.
    ///
    /// The seam `AssociateClientVpnTargetNetwork` is to call — still a scaffold, so today
    /// only the crate's tests seed through it; it is not dead code.
    ///
    /// The association starts in `associating`, the state `AssociateClientVpnTargetNetwork`
    /// answers with, and the clock owes it `associated` one [`TRANSITION_DELAY`] later
    /// through [`Self::materialize_due_transitions`]: the span is never zero, so a caller
    /// describing the association straight after the call that made it still sees
    /// `associating`.
    pub fn insert_client_vpn_target_network(
        &mut self,
        spec: ClientVpnTargetNetworkSpec,
    ) -> Option<crate::ops::types::TargetNetwork> {
        let view = crate::ops::types::TargetNetwork {
            association_id: None,
            vpc_id: Some(spec.vpc_id),
            target_network_id: Some(spec.target_network_id),
            client_vpn_endpoint_id: Some(spec.client_vpn_endpoint_id),
            status: Some(crate::ops::types::AssociationStatus {
                code: Some(ASSOCIATION_STATUS_ASSOCIATING.to_string()),
                message: None,
            }),
            security_groups: Some(spec.security_groups),
            // "The Availability Zone names for the target network association, if the
            // Client VPN endpoint uses a Transit Gateway"
            // (`TargetNetwork$AvailabilityZones`) — nothing for an endpoint that does not.
            availability_zones: None,
            availability_zone_ids: None,
        };
        let charge = client_vpn_target_network_payload_bytes(&view);
        if !self.has_client_vpn_target_network_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let association_id = self.next_id(CLIENT_VPN_TARGET_NETWORK_ASSOCIATION_ID_PREFIX);
        let view = crate::ops::types::TargetNetwork {
            association_id: Some(association_id.clone()),
            ..view
        };
        self.add_stored_bytes(charge);
        let stored = view.clone();
        self.client_vpn_target_networks.insert(
            association_id,
            ClientVpnTargetNetworkRecord {
                view,
                pending: Some(PendingStateMove::starting_now(
                    ASSOCIATION_STATUS_ASSOCIATED,
                )),
            },
        );
        Some(stored)
    }

    /// Apply the `associating` -> `associated` move every target network association that
    /// has reached its due instant by `now` owes.
    ///
    /// Called by [`Self::materialize_due_transitions`], which is what a reporting operation
    /// calls: the move was owed at its instant whether or not anybody asked, so applying it
    /// on a read changes nothing a later caller would not have seen anyway. The endpoint's
    /// own `pending-associate` -> `available` move is NOT here: it belongs to the operation
    /// that associates the network, which is what decides an endpoint "has been created and
    /// a target network has been associated" (`ClientVpnEndpointStatusCode`).
    pub(crate) fn materialize_due_client_vpn_target_networks(&mut self, now: Instant) {
        for record in self.client_vpn_target_networks.values_mut() {
            let due = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string);
            if let Some(code) = due {
                record.view.status = Some(crate::ops::types::AssociationStatus {
                    code: Some(code),
                    message: None,
                });
                record.pending = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The routes of a Client VPN endpoint's route table
// ---------------------------------------------------------------------------

/// "creating" — the state a route starts in (`ClientVpnRouteStatusCode`), which
/// `CreateClientVpnRoute` answers with.
pub const CLIENT_VPN_ROUTE_STATUS_CREATING: &str = "creating";

/// "active" — the state the clock owes a `creating` route ([`TRANSITION_DELAY`] later,
/// through [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const CLIENT_VPN_ROUTE_STATUS_ACTIVE: &str = "active";

/// "add-route" — the `Origin` of a route a caller added: "`add-route` indicates that the
/// route was manually added using the CreateClientVpnRoute action"
/// (`ClientVpnRoute$Origin`).
pub const CLIENT_VPN_ROUTE_ORIGIN_ADD_ROUTE: &str = "add-route";

/// "associate" — the other `Origin` that member takes: "`associate` indicates that the route
/// was automatically added when the target network was associated with the Client VPN
/// endpoint" (`ClientVpnRoute$Origin`), which is the operation that associates a target
/// network's to write, not this one's.
pub const CLIENT_VPN_ROUTE_ORIGIN_ASSOCIATE: &str = "associate";

/// "Nat" — the `Type` ("The route type", `ClientVpnRoute$Type`) EC2 reports for every route
/// of a Client VPN endpoint, as the AWS CLI reference's own `describe-client-vpn-routes`
/// example output reports both the route added with a target network and the route added by
/// `CreateClientVpnRoute`.
pub const CLIENT_VPN_ROUTE_TYPE_NAT: &str = "Nat";

/// The `TargetVpcSubnetId` that names the endpoint's own client network rather than a
/// subnet: "if you're adding a route for the local network, specify `local`"
/// (`CreateClientVpnRouteRequest$TargetVpcSubnetId`). It is not a subnet id, so it is not
/// resolved against the endpoint's target networks.
pub const CLIENT_VPN_ROUTE_TARGET_LOCAL: &str = "local";

/// `InvalidClientVpnDuplicateRoute` (HTTP 400): the endpoint's route table already holds a
/// route for that destination through that target, so adding this one would make two.
pub const INVALID_CLIENT_VPN_DUPLICATE_ROUTE: &str = "InvalidClientVpnDuplicateRoute";

/// `InvalidClientVpnActiveAssociationNotFound` (HTTP 400): the subnet a route would send
/// traffic through is not a target network of the endpoint, so there is no path for the
/// route to be programmed into.
pub const INVALID_CLIENT_VPN_ACTIVE_ASSOCIATION_NOT_FOUND: &str =
    "InvalidClientVpnActiveAssociationNotFound";

/// What separates a route's destination from its target in the position a page token names
/// ([`ClientVpnRouteKey::cursor`]): a character that orders BELOW every character a CIDR
/// block or a subnet id carries, so comparing two cursors as strings orders the pairs they
/// name rather than running the two halves together.
pub const CLIENT_VPN_ROUTE_CURSOR_SEPARATOR: char = ' ';

/// Refuse a route an endpoint's route table already holds: HTTP 400
/// [`INVALID_CLIENT_VPN_DUPLICATE_ROUTE`], naming the destination and the endpoint the
/// request sent.
pub fn client_vpn_duplicate_route<E>(destination_cidr: &str, endpoint_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_DUPLICATE_ROUTE,
        400,
        format!("Route {destination_cidr} already exists in Client VPN endpoint {endpoint_id}"),
    )
}

/// The id prefix EC2 mints a subnet with — `subnet-`, the prefix
/// [`ComAmazonawsEc2Amazonec2State::insert_subnet`] mints a subnet id under. A
/// `TargetVpcSubnetId` is read against it, because a value that is not a subnet id at all is
/// refused before anything is looked up.
pub const SUBNET_ID_PREFIX: &str = "subnet";

/// `InvalidSubnetID.Malformed` (HTTP 400): the value given for a subnet is not a subnet id.
///
/// The same `Invalid<Kind>.Malformed` question [`require_well_formed_id`] asks of every
/// resource id of this crate, in the wording the Client VPN surface answers it with — `The
/// subnet ID '<id>' is malformed`, naming the member's own resource kind rather than the
/// generic `Invalid id: '<id>'`.
pub const INVALID_SUBNET_ID_MALFORMED: &str = "InvalidSubnetID.Malformed";

/// Refuse a value for a subnet that is not a subnet id: HTTP 400
/// [`INVALID_SUBNET_ID_MALFORMED`], naming the value the request sent.
pub fn invalid_subnet_id_malformed<E>(subnet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SUBNET_ID_MALFORMED,
        400,
        format!("The subnet ID '{subnet_id}' is malformed"),
    )
}

/// Refuse a route through a subnet that is not a target network of the endpoint: HTTP 400
/// [`INVALID_CLIENT_VPN_ACTIVE_ASSOCIATION_NOT_FOUND`], naming the subnet and the endpoint.
pub fn client_vpn_active_association_not_found<E>(
    subnet_id: &str,
    endpoint_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_ACTIVE_ASSOCIATION_NOT_FOUND,
        400,
        format!(
            "There is no active association for subnet {subnet_id} in Client VPN endpoint \
             {endpoint_id}"
        ),
    )
}

/// What names ONE route of a Client VPN endpoint's route table: the endpoint it belongs to,
/// the destination it routes, and the target that traffic leaves through.
///
/// Those three are exactly what `DeleteClientVpnRoute` names a route by
/// (`ClientVpnEndpointId` + `DestinationCidrBlock` + `TargetVpcSubnetId`), and
/// `ClientVpnRoute` carries no id member of its own, so the store is keyed by them rather
/// than by a minted id. `target` is the empty string for a route of a Transit Gateway-based
/// endpoint, where "this parameter is not required"
/// (`CreateClientVpnRouteRequest$TargetVpcSubnetId`) because traffic leaves through the
/// endpoint's attachment instead.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClientVpnRouteKey {
    /// "The ID of the Client VPN endpoint with which the route is associated"
    /// (`ClientVpnRoute$ClientVpnEndpointId`).
    pub client_vpn_endpoint_id: String,
    /// "The IPv4 address range, in CIDR notation, of the route destination"
    /// (`ClientVpnRoute$DestinationCidr`), in the canonical form this service stores every
    /// CIDR member in.
    pub destination_cidr: String,
    /// "The ID of the subnet through which traffic is routed"
    /// (`ClientVpnRoute$TargetSubnet`), [`CLIENT_VPN_ROUTE_TARGET_LOCAL`] for a route of the
    /// local network, or empty for a route that leaves through a Transit Gateway attachment.
    pub target: String,
}

impl ClientVpnRouteKey {
    /// This route's position in the order its endpoint's routes are reported and paged in:
    /// the destination, then the target.
    ///
    /// A single string so a page token can name a position with one cursor
    /// ([`CLIENT_VPN_ROUTE_CURSOR_SEPARATOR`] says why the halves cannot run together), and
    /// the reporting path both SORTS and RESUMES by it, so the paging order and the cursor
    /// are the same order by construction.
    pub fn cursor(&self) -> String {
        format!(
            "{destination}{CLIENT_VPN_ROUTE_CURSOR_SEPARATOR}{target}",
            destination = self.destination_cidr,
            target = self.target,
        )
    }
}

/// One route of a Client VPN endpoint's route table: the modelled
/// [`crate::ops::types::ClientVpnRoute`] every operation that reports one answers with, plus
/// the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// of this module holds one: a member a writer sets round-trips through
/// `DescribeClientVpnRoutes` without a field-by-field copy that can silently drop one.
#[derive(Debug, Clone)]
pub struct ClientVpnRouteRecord {
    /// The route as the model shapes it, carrying the status reached so far.
    pub view: crate::ops::types::ClientVpnRoute,
    /// The `ClientVpnRouteStatusCode` the clock owes this route while it is still transient
    /// (`creating` -> `active`); `None` once it has been applied.
    pub pending: Option<PendingStateMove>,
}

/// What one route is written with: the endpoint whose route table it joins, the destination
/// it routes, what traffic for that destination leaves through, and the description the
/// request gave it.
///
/// Every member the service itself owns — the route's type, its origin and the status it
/// starts in — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_route`] rather than taken from a
/// caller, so two writers cannot report one route differently.
#[derive(Debug, Clone)]
pub struct ClientVpnRouteSpec {
    /// The endpoint whose route table takes the route.
    pub client_vpn_endpoint_id: String,
    /// The destination range, in the canonical form this service stores a CIDR member in.
    pub destination_cidr: String,
    /// The subnet traffic is routed through, or [`CLIENT_VPN_ROUTE_TARGET_LOCAL`]; `None`
    /// for a Transit Gateway-based endpoint, whose routes leave through its attachment.
    pub target_subnet: Option<String>,
    /// "The ID of the Transit Gateway attachment, if the route targets a Transit Gateway"
    /// (`ClientVpnRoute$TransitGatewayAttachmentId`) — read off the endpoint's own stored
    /// configuration, never from the request.
    pub transit_gateway_attachment_id: Option<String>,
    /// "A brief description of the route" (`CreateClientVpnRouteRequest$Description`).
    pub description: Option<String>,
}

/// The caller-chosen bytes one stored route holds: the ids of the resources it joins, the
/// destination range and the description, on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
///
/// The ONE definition of that charge, the way
/// [`client_vpn_target_network_payload_bytes`] is for the store beside it: the insert charges
/// exactly it, and a removal path must give back the same number.
pub fn client_vpn_route_payload_bytes(view: &crate::ops::types::ClientVpnRoute) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(view.client_vpn_endpoint_id.as_deref().map_or(0, str::len))
        .saturating_add(view.destination_cidr.as_deref().map_or(0, str::len))
        .saturating_add(view.target_subnet.as_deref().map_or(0, str::len))
        .saturating_add(view.description.as_deref().map_or(0, str::len))
        .saturating_add(
            view.transit_gateway_attachment_id
                .as_deref()
                .map_or(0, str::len),
        )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// writer asks this BEFORE inserting, because nothing here reclaims a stored route.
    pub fn has_client_vpn_route_capacity(&self) -> bool {
        self.client_vpn_routes.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The route `key` names, or `None` when that endpoint's route table holds none.
    pub fn client_vpn_route(&self, key: &ClientVpnRouteKey) -> Option<&ClientVpnRouteRecord> {
        self.client_vpn_routes.get(key)
    }

    /// Is `subnet_id` a target network of `endpoint_id`?
    ///
    /// The question `CreateClientVpnRoute` asks of its target subnet — "The specified subnet
    /// must be an existing target network of the Client VPN endpoint"
    /// (`CreateClientVpnRouteRequest$TargetVpcSubnetId`) — answered against the association
    /// store, so the route path and `DescribeClientVpnTargetNetworks` read one set of
    /// associations.
    pub fn client_vpn_endpoint_has_target_network(
        &self,
        endpoint_id: &str,
        subnet_id: &str,
    ) -> bool {
        self.client_vpn_target_networks.values().any(|record| {
            record.view.client_vpn_endpoint_id.as_deref() == Some(endpoint_id)
                && record.view.target_network_id.as_deref() == Some(subnet_id)
        })
    }

    /// Add one route to a Client VPN endpoint's route table and answer the key it is held
    /// under beside the route as its writer reports it — or `None` when one of the store's
    /// bounds would be crossed and NOTHING was stored.
    ///
    /// The seam `CreateClientVpnRoute` calls. Every member the service owns is derived here:
    /// the route `Type` EC2 reports for a Client VPN route, the `add-route` `Origin` that
    /// says a caller added it rather than a target network association, and the `creating`
    /// status it starts in — which the clock owes `active` one [`TRANSITION_DELAY`] later
    /// through [`Self::materialize_due_transitions`]. The span is never zero, so a caller
    /// describing the route straight after the call that made it still sees `creating`.
    pub fn insert_client_vpn_route(
        &mut self,
        spec: ClientVpnRouteSpec,
    ) -> Option<(ClientVpnRouteKey, crate::ops::types::ClientVpnRoute)> {
        let key = ClientVpnRouteKey {
            client_vpn_endpoint_id: spec.client_vpn_endpoint_id.clone(),
            destination_cidr: spec.destination_cidr.clone(),
            target: spec.target_subnet.clone().unwrap_or_default(),
        };
        let view = crate::ops::types::ClientVpnRoute {
            client_vpn_endpoint_id: Some(spec.client_vpn_endpoint_id),
            destination_cidr: Some(spec.destination_cidr),
            target_subnet: spec.target_subnet,
            r#type: Some(CLIENT_VPN_ROUTE_TYPE_NAT.to_string()),
            origin: Some(CLIENT_VPN_ROUTE_ORIGIN_ADD_ROUTE.to_string()),
            status: Some(crate::ops::types::ClientVpnRouteStatus {
                code: Some(CLIENT_VPN_ROUTE_STATUS_CREATING.to_string()),
                message: None,
            }),
            description: spec.description,
            transit_gateway_attachment_id: spec.transit_gateway_attachment_id,
        };
        let charge = client_vpn_route_payload_bytes(&view);
        if !self.has_client_vpn_route_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        self.add_stored_bytes(charge);
        let stored = view.clone();
        self.client_vpn_routes.insert(
            key.clone(),
            ClientVpnRouteRecord {
                view,
                pending: Some(PendingStateMove::starting_now(
                    CLIENT_VPN_ROUTE_STATUS_ACTIVE,
                )),
            },
        );
        Some((key, stored))
    }

    /// Apply the `creating` -> `active` move every route that has reached its due instant by
    /// `now` owes.
    ///
    /// Called by [`Self::materialize_due_transitions`], which is what a reporting operation
    /// calls: the move was owed at its instant whether or not anybody asked, so applying it
    /// on a read changes nothing a later caller would not have seen anyway.
    pub(crate) fn materialize_due_client_vpn_routes(&mut self, now: Instant) {
        for record in self.client_vpn_routes.values_mut() {
            let due = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string);
            if let Some(code) = due {
                record.view.status = Some(crate::ops::types::ClientVpnRouteStatus {
                    code: Some(code),
                    message: None,
                });
                record.pending = None;
            }
        }
    }
}
