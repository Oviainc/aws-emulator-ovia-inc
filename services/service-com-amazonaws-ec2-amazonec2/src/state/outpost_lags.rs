// SPDX-License-Identifier: CC0-1.0

//! The Outpost link aggregation group (LAG) store and the service link virtual interfaces that
//! run over a LAG: the ids each is named by, the ARN of the Outpost they belong to, the bounds
//! the stores are written through, and the one seam each resource is placed through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.
//!
//! "LAGs are only available for second-generation Outposts racks" (`DescribeOutpostLags`), and
//! nothing in the EC2 API this emulator serves racks an Outpost — the reason
//! [`ComAmazonawsEc2Amazonec2State::local_gateways`] starts empty too. So
//! [`ComAmazonawsEc2Amazonec2State::insert_outpost_lag`] is the seam an account's LAGs are
//! placed through, and an account without an Outpost has no LAG to report.

use super::*;

/// The id prefix EC2 mints an Outpost LAG with: `ola-`, the prefix the ids in this surface's
/// own requests carry (`CreateLocalGatewayVirtualInterfaceRequest$OutpostLagId`, "References
/// the Link Aggregation Group (LAG) that connects the Outpost to on-premises network
/// devices"). The model puts no pattern on `OutpostLagId`, so the prefix is the one thing that
/// says which resource an id names.
pub const OUTPOST_LAG_ID_PREFIX: &str = "ola";

/// The id prefix AWS mints an Outpost with: `op-`, as the one `OutpostArn` pattern this model
/// carries spells it out — `^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/op-[a-f0-9]{17}$`
/// (`smithy_query(query="shape",shape="OutpostArn")`), the ARN
/// [`CreateCapacityReservationRequest$OutpostArn`] is held to.
pub const OUTPOST_ID_PREFIX: &str = "op";

/// The resource segment an Outpost's ARN names it under: `outpost/<id>`, from that same
/// `OutpostArn` pattern. The one spelling, so the ARN builder and the reader that takes an
/// Outpost id back out of an ARN cannot drift apart.
const OUTPOST_ARN_RESOURCE: &str = "outpost";

/// How many Outpost LAGs this emulator stores before a write must refuse.
///
/// CHOSEN, like [`MAX_LOCAL_GATEWAYS`] and for the same reason: nothing in this emulator
/// reclaims a stored LAG, so the store needs a ceiling that is not the process's memory. A
/// count ceiling is no bound on the caller-chosen bytes each record holds, so those are
/// charged against [`MAX_STORED_BYTES`] too ([`outpost_lag_payload_bytes`]).
pub const MAX_OUTPOST_LAGS: usize = 4096;

/// The ARN of the Outpost a LAG belongs to:
/// `arn:aws:outposts:<region>:<account>:outpost/<outpost-id>`, the form the model's own
/// `OutpostArn` pattern publishes. Built here rather than by each writer so the ARN a LAG
/// reports and the ARN a local gateway reports have ONE form.
pub fn outpost_arn(region: &str, outpost_id: &str) -> String {
    ::engine::emulator_arn(
        "outposts",
        region,
        &format!("{OUTPOST_ARN_RESOURCE}/{outpost_id}"),
    )
}

/// "The Outpost ID" an Outpost ARN names — the `outpost/<id>` segment of
/// [`outpost_arn`] — or `None` for an ARN that names no Outpost.
///
/// An Outpost id is a fact OF its ARN rather than a member of its own: the modelled
/// `OutpostLag` shape carries the ARN, so the one filter that selects on the id reads it back
/// out here instead of a second copy being stored beside the ARN.
pub fn outpost_id_of_arn(arn: &str) -> Option<&str> {
    arn.rsplit_once(':')
        .map(|(_, resource)| resource)
        .and_then(|resource| resource.strip_prefix(OUTPOST_ARN_RESOURCE))
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|outpost_id| !outpost_id.is_empty())
}

/// Whether `id` is a well-formed Outpost LAG id — one EC2 could have issued.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`OUTPOST_LAG_ID_PREFIX`], whose doc carries the evidence for it).
pub fn outpost_lag_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(OUTPOST_LAG_ID_PREFIX, id)
}

/// What one Outpost LAG is written with: the Outpost it aggregates the links of, the state it
/// is in, the virtual interfaces that run over it, and its tags.
///
/// Every member the service itself owns — the LAG id and the owning account — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_outpost_lag`] rather than taken from the caller, so
/// two writers cannot report one LAG differently.
#[derive(Debug, Clone, Default)]
pub struct OutpostLagSpec {
    /// "The Amazon Resource Number (ARN)" the LAG reports (`OutpostLag$OutpostArn`): the ARN
    /// of the Outpost it belongs to, in the form [`outpost_arn`] builds.
    pub outpost_arn: String,
    /// "The current state of the Outpost LAG" (`OutpostLag$State`), which the model leaves an
    /// unconstrained string.
    pub state: String,
    /// "The IDs of the local gateway virtual interfaces associated with the Outpost LAG"
    /// (`OutpostLag$LocalGatewayVirtualInterfaceIds`). A virtual interface created ON this LAG
    /// is added to this list by
    /// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_virtual_interface`], the one place
    /// an interface is stored, so the list cannot drift from the interface store.
    pub local_gateway_virtual_interface_ids: Vec<String>,
    /// "The service link virtual interface IDs associated with the Outpost LAG"
    /// (`OutpostLag$ServiceLinkVirtualInterfaceIds`).
    pub service_link_virtual_interface_ids: Vec<String>,
    /// "The tags associated with the Outpost LAG" (`OutpostLag$Tags`).
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored Outpost LAG holds: its tags, the Outpost ARN it names,
/// the state a writer pinned on it and the ids of the virtual interfaces over it. Everything
/// else on the record — the LAG id and the owning account — is derived by this module, so it is
/// fixed-size.
///
/// The ONE definition of that charge, the way [`local_gateway_payload_bytes`] is for the local
/// gateways of the same Outpost: [`ComAmazonawsEc2Amazonec2State::insert_outpost_lag`] charges
/// exactly it, and a removal path must give back the same number.
pub fn outpost_lag_payload_bytes(spec: &OutpostLagSpec) -> usize {
    spec.local_gateway_virtual_interface_ids
        .iter()
        .chain(spec.service_link_virtual_interface_ids.iter())
        .fold(tag_list_bytes(&spec.tags), |total, id| {
            total.saturating_add(id.len())
        })
        .saturating_add(spec.outpost_arn.len())
        .saturating_add(spec.state.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Outpost LAG fits inside the store's ceiling ([`MAX_OUTPOST_LAGS`]).
    pub fn has_outpost_lag_capacity(&self) -> bool {
        self.outpost_lags.len() < MAX_OUTPOST_LAGS
    }

    /// Store one Outpost LAG and answer the id it was given — or `None` when the store cannot
    /// take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte charged:
    /// the count ceiling ([`Self::has_outpost_lag_capacity`]) and the byte budget
    /// ([`Self::would_exceed_stored_bytes`] over [`outpost_lag_payload_bytes`]) — the same
    /// contract as [`Self::insert_local_gateway`]. A caller that gets `None` must refuse the
    /// request rather than report a LAG it did not store.
    ///
    /// No EC2 operation creates an Outpost LAG (one comes with a second-generation Outposts
    /// rack), so this is the seam an account's LAGs are placed through rather than dead code: it
    /// is what gives `DescribeOutpostLags` something to report.
    pub fn insert_outpost_lag(&mut self, spec: OutpostLagSpec) -> Option<String> {
        let payload_bytes = outpost_lag_payload_bytes(&spec);
        if !self.has_outpost_lag_capacity() || self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        let outpost_lag_id = self.next_id(OUTPOST_LAG_ID_PREFIX);
        let outpost_lag = crate::ops::types::OutpostLag {
            outpost_lag_id: Some(outpost_lag_id.clone()),
            outpost_arn: Some(spec.outpost_arn),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            state: Some(spec.state),
            local_gateway_virtual_interface_ids: Some(spec.local_gateway_virtual_interface_ids),
            service_link_virtual_interface_ids: Some(spec.service_link_virtual_interface_ids),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(payload_bytes);
        self.outpost_lags
            .insert(outpost_lag_id.clone(), outpost_lag);
        Some(outpost_lag_id)
    }
}

// =============================================================================
// The service link virtual interfaces that run over a LAG
// =============================================================================

/// The id prefix a service link virtual interface is minted with: `slvi-`, abbreviated from the
/// `ResourceType` member the resource is tagged and named under
/// ([`RESOURCE_TYPE_SERVICE_LINK_VIRTUAL_INTERFACE`]).
///
/// CHOSEN: the model puts no pattern on `ServiceLinkVirtualInterfaceId` and publishes no
/// example of one, so the prefix is this emulator's — what it must do is say which resource an
/// id names, which is why it is not a prefix of, and does not start with, any other id prefix
/// of this surface ([`OUTPOST_LAG_ID_PREFIX`], [`LOCAL_GATEWAY_VIF_ID_PREFIX`]).
pub const SERVICE_LINK_VIF_ID_PREFIX: &str = "slvi";

/// The `ResourceType` enum member a service link virtual interface is tagged and named under:
/// `service-link-virtual-interface` (`smithy_query(query="shape",shape="ResourceType")`), which
/// is also the resource path [`service_link_virtual_interface_arn`] builds its ARNs under.
pub const RESOURCE_TYPE_SERVICE_LINK_VIRTUAL_INTERFACE: &str = "service-link-virtual-interface";

/// How many service link virtual interfaces this emulator stores before a write must refuse —
/// CHOSEN, for the reason [`MAX_OUTPOST_LAGS`] is.
pub const MAX_SERVICE_LINK_VIFS: usize = 4096;

/// The ARN of a service link virtual interface:
/// `arn:aws:ec2:<region>:<account>:service-link-virtual-interface/<id>`, the form
/// `ServiceLinkVirtualInterface$ServiceLinkVirtualInterfaceArn` carries ("The Amazon Resource
/// Number (ARN) for the service link virtual interface") under the resource path
/// `ResourceType`'s own member names ([`RESOURCE_TYPE_SERVICE_LINK_VIRTUAL_INTERFACE`]).
pub fn service_link_virtual_interface_arn(region: &str, interface_id: &str) -> String {
    ::engine::emulator_arn(
        "ec2",
        region,
        &format!("{RESOURCE_TYPE_SERVICE_LINK_VIRTUAL_INTERFACE}/{interface_id}"),
    )
}

/// Whether `id` is a well-formed service link virtual interface id — one EC2 could have issued:
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under [`SERVICE_LINK_VIF_ID_PREFIX`].
pub fn service_link_virtual_interface_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(SERVICE_LINK_VIF_ID_PREFIX, id)
}

/// What one service link virtual interface is written with: the Outpost and the LAG it runs
/// over, its VLAN, its local and peer addresses, the peer's BGP ASN, the Region whose ARN
/// namespace its own ARN names, its configuration state, and its tags.
///
/// Every member the service itself owns — the interface id, its ARN, the owning account and the
/// Outpost id its Outpost ARN names — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_service_link_virtual_interface`] rather than taken
/// from the caller, so two writers cannot report one interface differently.
#[derive(Debug, Clone, Default)]
pub struct ServiceLinkVirtualInterfaceSpec {
    /// "The Outpost Amazon Resource Number (ARN) for the service link virtual interface"
    /// (`ServiceLinkVirtualInterface$OutpostArn`), in the form [`outpost_arn`] builds. "The
    /// Outpost ID" the interface reports is read back out of it ([`outpost_id_of_arn`]).
    pub outpost_arn: String,
    /// "The link aggregation group (LAG) ID for the service link virtual interface"
    /// (`ServiceLinkVirtualInterface$OutpostLagId`) — the LAG whose
    /// `ServiceLinkVirtualInterfaceIds` gains the minted id when this store holds that LAG.
    pub outpost_lag_id: Option<String>,
    /// "The IPv4 address assigned to the local gateway virtual interface on the Outpost side".
    pub local_address: Option<String>,
    /// "The IPv4 peer address for the service link virtual interface".
    pub peer_address: Option<String>,
    /// "The ASN for the Border Gateway Protocol (BGP) associated with the service link virtual
    /// interface".
    pub peer_bgp_asn: Option<i64>,
    /// "The virtual local area network for the service link virtual interface".
    pub vlan: Option<i32>,
    /// The Region the interface's own ARN names.
    pub region: String,
    /// "The current state of the service link virtual interface"
    /// (`ServiceLinkVirtualInterface$ConfigurationState`), one of
    /// [`crate::ops::types::SERVICE_LINK_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES`].
    pub configuration_state: String,
    /// "The tags associated with the service link virtual interface".
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored service link virtual interface holds: its tags, the
/// Outpost ARN and LAG id it names, its two addresses, the configuration state a writer pinned
/// on it, and the Region, which the stored ARN carries. Everything else on the record — the
/// interface id, the rest of the ARN, the owning account, the VLAN and the ASN — is derived by
/// this module or fixed-size.
///
/// The ONE definition of that charge, the way [`outpost_lag_payload_bytes`] is for the LAG the
/// interface runs over.
pub fn service_link_vif_payload_bytes(spec: &ServiceLinkVirtualInterfaceSpec) -> usize {
    [
        spec.outpost_lag_id.as_deref(),
        spec.local_address.as_deref(),
        spec.peer_address.as_deref(),
    ]
    .into_iter()
    .flatten()
    .fold(tag_list_bytes(&spec.tags), |total, value| {
        total.saturating_add(value.len())
    })
    .saturating_add(spec.outpost_arn.len())
    .saturating_add(spec.configuration_state.len())
    .saturating_add(spec.region.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another service link virtual interface fits inside the store's ceiling
    /// ([`MAX_SERVICE_LINK_VIFS`]).
    pub fn has_service_link_vif_capacity(&self) -> bool {
        self.service_link_virtual_interfaces.len() < MAX_SERVICE_LINK_VIFS
    }

    /// Store one service link virtual interface and answer the id it was given — or `None` when
    /// the store cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte charged:
    /// the count ceiling ([`Self::has_service_link_vif_capacity`]) and the byte budget
    /// ([`Self::would_exceed_stored_bytes`] over [`service_link_vif_payload_bytes`]) — the same
    /// contract as [`Self::insert_outpost_lag`].
    ///
    /// An interface that runs over a LAG this store holds is one of "The service link virtual
    /// interface IDs associated with the Outpost LAG"
    /// (`OutpostLag$ServiceLinkVirtualInterfaceIds`), so the minted id is added to that LAG's
    /// list HERE, in the one place an interface is stored: the LAG and this store cannot report
    /// a different association. A spec naming a LAG this store does not hold leaves the lists it
    /// has alone.
    pub fn insert_service_link_virtual_interface(
        &mut self,
        spec: ServiceLinkVirtualInterfaceSpec,
    ) -> Option<String> {
        let payload_bytes = service_link_vif_payload_bytes(&spec);
        if !self.has_service_link_vif_capacity() || self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        let interface_id = self.next_id(SERVICE_LINK_VIF_ID_PREFIX);
        let outpost_lag_id = spec.outpost_lag_id.clone();
        let interface = crate::ops::types::ServiceLinkVirtualInterface {
            service_link_virtual_interface_id: Some(interface_id.clone()),
            service_link_virtual_interface_arn: Some(service_link_virtual_interface_arn(
                &spec.region,
                &interface_id,
            )),
            outpost_id: outpost_id_of_arn(&spec.outpost_arn).map(str::to_string),
            outpost_arn: Some(spec.outpost_arn),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            local_address: spec.local_address,
            peer_address: spec.peer_address,
            peer_bgp_asn: spec.peer_bgp_asn,
            vlan: spec.vlan,
            outpost_lag_id: spec.outpost_lag_id,
            tags: Some(spec.tags),
            configuration_state: Some(spec.configuration_state),
        };
        self.add_stored_bytes(payload_bytes);
        if let Some(outpost_lag) = outpost_lag_id
            .as_deref()
            .and_then(|outpost_lag_id| self.outpost_lags.get_mut(outpost_lag_id))
        {
            outpost_lag
                .service_link_virtual_interface_ids
                .get_or_insert_with(Vec::new)
                .push(interface_id.clone());
        }
        self.service_link_virtual_interfaces
            .insert(interface_id.clone(), interface);
        Some(interface_id)
    }
}
