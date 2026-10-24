// SPDX-License-Identifier: CC0-1.0

//! Client VPN endpoints and the target networks associated with them.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`: the two stores,
//! their id forms, their refusals and the clock-owed `associating` -> `associated` move are
//! one subject, and keeping them in their own file leaves `state.rs` readable — and leaves
//! this fix out of the end of that file, which is where every concurrently landing EC2 fix
//! inserts. Everything here is re-exported by `crate::state`, so a handler reaches it as
//! `crate::state::require_known_client_vpn_endpoint` exactly as it reaches the helpers that
//! live in the parent module.
//!
//! The stores themselves are fields of [`ComAmazonawsEc2Amazonec2State`]
//! (`client_vpn_endpoints`, `client_vpn_target_networks`), because the state struct is one
//! struct; it is only the code that reads and writes them that lives here.

use super::*;

/// The id prefix EC2 mints a Client VPN endpoint with: `cvpn-endpoint-`, as the AWS Client
/// VPN documentation's own ids read (for example `cvpn-endpoint-0ac3a1abbccba1234`). The
/// model puts no pattern on `ClientVpnEndpointId`
/// (`smithy_query(query="shape",shape="ClientVpnEndpointId")` is a plain `String`), so the
/// prefix is the one thing that says which resource an id names.
///
/// An endpoint id is read by the generic [`prefixed_id_is_well_formed`] under this prefix
/// rather than by the stricter [`issued_long_id_is_well_formed`]: AWS's own documentation
/// uses `cvpn-endpoint-00000000000000000` as a placeholder endpoint id and answers
/// `InvalidClientVpnEndpointId.NotFound` for it rather than `.Malformed`, so an all-zero
/// suffix is a well-formed id that names nothing.
pub const CLIENT_VPN_ENDPOINT_ID_PREFIX: &str = "cvpn-endpoint";

/// The id prefix EC2 mints a target network association with: `cvpn-assoc-`, as the AWS
/// documentation's own association ids read (for example `cvpn-assoc-0c9a06dc85b3e9bc1`).
pub const CLIENT_VPN_TARGET_NETWORK_ASSOCIATION_ID_PREFIX: &str = "cvpn-assoc";

/// `InvalidClientVpnEndpointId.NotFound` (HTTP 400): the request named a Client VPN
/// endpoint id this account holds no endpoint for.
pub const INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND: &str = "InvalidClientVpnEndpointId.NotFound";

/// `InvalidClientVpnEndpointId.Malformed` (HTTP 400): the value is not a Client VPN
/// endpoint id at all, which EC2 answers BEFORE it looks one up — so a caller can tell "not
/// an id of this kind" from "no such endpoint".
pub const INVALID_CLIENT_VPN_ENDPOINT_ID_MALFORMED: &str = "InvalidClientVpnEndpointId.Malformed";

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

/// Refuse a request that named a Client VPN endpoint id this account holds no endpoint for:
/// HTTP 400 [`INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND`], through the one writer of that
/// refusal ([`id_not_found`]).
pub fn invalid_client_vpn_endpoint_id_not_found<E>(id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND,
        "Client VPN endpoint ID",
        id,
    )
}

/// Refuse a `ClientVpnEndpointId` this service cannot resolve: a malformed one before an
/// unknown one, the order [`require_well_formed_id`] documents.
///
/// The ONE place a Client VPN endpoint id is resolved, so every operation that takes one
/// refuses the same id the same way; the contract [`require_known_instance`] writes out for
/// instances.
pub fn require_known_client_vpn_endpoint<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_id(
        CLIENT_VPN_ENDPOINT_ID_PREFIX,
        id,
        INVALID_CLIENT_VPN_ENDPOINT_ID_MALFORMED,
    )?;
    if !store.client_vpn_endpoints.contains_key(id) {
        return Err(invalid_client_vpn_endpoint_id_not_found(id));
    }
    Ok(())
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
