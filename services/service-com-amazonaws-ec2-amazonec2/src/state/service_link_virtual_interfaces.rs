// SPDX-License-Identifier: CC0-1.0

//! What a service link virtual interface shares with the Outpost LAG it runs over.
//!
//! The interfaces themselves, their ids, ARNs and the one seam they are stored through live
//! with the LAG store in [`super::outpost_lags`], a LAG and the interfaces over it being one
//! concern. What lives here is the one reading ACROSS the two stores that a report of the
//! service link virtual interfaces needs and the stored interface carries no member for: the
//! local gateway virtual interfaces that run over the same LAG.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches it as `crate::state::<item>` like every other
//! primitive there.

use super::*;

/// "The IDs of the local gateway virtual interfaces associated with the Outpost LAG"
/// `outpost_lag_id` names (`OutpostLag$LocalGatewayVirtualInterfaceIds`), or no id at all for an
/// interface that runs over no LAG, or over one this store does not hold.
///
/// `DescribeServiceLinkVirtualInterfaces` documents a `local-gateway-virtual-interface-id`
/// filter ("The ID of the local gateway virtual interface" —
/// `DescribeServiceLinkVirtualInterfacesRequest$Filters`) while `ServiceLinkVirtualInterface`
/// declares no member for one: what a service link virtual interface shares with a local
/// gateway virtual interface is the LAG they both run over, which the stored interface DOES
/// name (`ServiceLinkVirtualInterface$OutpostLagId`), so the association is read off that LAG
/// here rather than copied onto a second record — the same way `DescribeOutpostLags` reads an
/// interface's own attributes off [`ComAmazonawsEc2Amazonec2State::service_link_virtual_interfaces`]
/// instead of storing them twice.
pub fn local_gateway_virtual_interface_ids_over_outpost_lag<'a>(
    outpost_lags: &'a BTreeMap<String, crate::ops::types::OutpostLag>,
    outpost_lag_id: Option<&str>,
) -> &'a [String] {
    outpost_lag_id
        .and_then(|outpost_lag_id| outpost_lags.get(outpost_lag_id))
        .and_then(|outpost_lag| outpost_lag.local_gateway_virtual_interface_ids.as_deref())
        .unwrap_or_default()
}
