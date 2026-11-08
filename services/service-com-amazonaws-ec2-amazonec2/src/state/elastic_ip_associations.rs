// SPDX-License-Identifier: CC0-1.0

//! What an Elastic IP ASSOCIATION is, as both sides of it are stored.
//!
//! "Associates an Elastic IP address ... with an instance or a network interface"
//! (`AssociateAddress`), and an instance carries an address only THROUGH an interface:
//! "each instance has a default network interface, called the primary network interface"
//! (`Instance$NetworkInterfaces`), which is what an address named with an `InstanceId`
//! binds to. So one association is one fact about one interface, and it is written in the
//! two places that report it — the account's [`Address`] record, which `DescribeAddresses`
//! reports (`associationId`, `networkInterfaceId`, `networkInterfaceOwnerId`,
//! `privateIpAddress` and the instance that holds the interface), and the interface's own
//! `NetworkInterface$Association`, which `DescribeNetworkInterfaces` and
//! `DescribeNetworkInterfaceAttribute` report.
//!
//! Both sides move together here, through
//! [`ComAmazonawsEc2Amazonec2State::associate_elastic_ip`] and the reverse
//! [`ComAmazonawsEc2Amazonec2State::detach_elastic_ip`] (`super::elastic_ips_and_transit_gateway_route_tables`),
//! so no reader of either store can see an association the other does not have — which is
//! what lets `DisassociateAddress` undo exactly what `AssociateAddress` recorded and
//! `ReleaseAddress` then see an address nothing holds.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;
use crate::ops::types::NetworkInterfaceAssociation;

/// The association one `AssociateAddress` call records: which interface the Elastic IP
/// address binds to, which private address of that interface it is bound to, and the
/// instance that holds the interface, when one does.
///
/// A spec rather than separate arguments, so the one writer below takes the whole fact at
/// once and a caller cannot record half of it. `network_interface_id` is `None` only for an
/// instance this store holds no interface record for: the association is then the address's
/// own, with no interface to name — never an id no reader could resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElasticIpAssociationSpec {
    /// The interface the address is bound to, as its `NetworkInterfaceId`.
    pub network_interface_id: Option<String>,
    /// The instance that holds that interface, as its `InstanceId` — "the ID of the
    /// instance that the address is associated with (if any)" (`Address$InstanceId`), so an
    /// interface nothing holds associates an address with no instance.
    pub instance_id: Option<String>,
    /// "The primary or secondary private IP address to associate with the Elastic IP
    /// address" (`AssociateAddressRequest$PrivateIpAddress`), already resolved to one the
    /// interface holds — `None` for an interface that holds no private IPv4 address at all,
    /// which has none for the association to name.
    pub private_ip_address: Option<String>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// The `NetworkInterfaceId` of the instance `instance_id`'s PRIMARY network interface —
    /// the one attached at [`PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX`] — or `None` when this
    /// store holds no attached interface for it.
    ///
    /// "The instance must have exactly one attached network interface"
    /// (`AssociateAddressRequest$InstanceId`), and that one is the launch's own
    /// ([`Self::mint_launched_primary_interface`]), so this is what an association named
    /// with an instance id binds to. Read off the one interface store, through the
    /// per-instance index [`Self::network_interfaces_by_instance`], so the interface an
    /// association names is one `DescribeNetworkInterfaces` reports.
    pub fn primary_network_interface_of_instance(&self, instance_id: &str) -> Option<&NetworkInterface> {
        let held = self.network_interfaces_by_instance.get(instance_id)?;
        held.iter()
            .filter_map(|network_interface_id| self.network_interfaces.get(network_interface_id))
            .filter(|interface| {
                interface
                    .attachment
                    .as_ref()
                    .is_some_and(|attachment| !attachment_is_detached(attachment))
            })
            .min_by_key(|interface| {
                interface
                    .attachment
                    .as_ref()
                    .and_then(|attachment| attachment.device_index)
                    .unwrap_or(i32::MAX)
            })
    }

    /// The private IPv4 addresses the interface `network_interface_id` holds, primary first.
    ///
    /// The set an `AssociateAddress` request's `PrivateIpAddress` is resolved against, and
    /// whose first entry is the primary address a request naming none binds to — "if no
    /// private IP address is specified, the Elastic IP address is associated with the
    /// primary private IP address" (`AssociateAddressRequest$PrivateIpAddress`).
    pub fn private_ip_addresses_of_interface(&self, network_interface_id: &str) -> Vec<String> {
        let Some(interface) = self.network_interfaces.get(network_interface_id) else {
            return Vec::new();
        };
        let mut addresses: Vec<(bool, String)> = interface
            .private_ip_addresses
            .iter()
            .flatten()
            .filter_map(|held| {
                let address = held.private_ip_address.clone()?;
                Some((held.primary != Some(true), address))
            })
            .collect();
        addresses.sort();
        addresses
            .into_iter()
            .map(|(_, address)| address)
            .collect()
    }

    /// The allocation id of the address currently associated with the interface
    /// `network_interface_id`, or `None` when no address of the account is.
    ///
    /// Scanned off the address records themselves, like
    /// [`Self::allocation_id_associated_with_instance`] and for the same reason:
    /// `Address$NetworkInterfaceId` is where the association is stored, so there is no
    /// second copy of it to fall out of step.
    pub fn allocation_id_associated_with_interface(
        &self,
        network_interface_id: &str,
    ) -> Option<String> {
        self.addresses
            .iter()
            .find(|(_, address)| {
                address.network_interface_id.as_deref() == Some(network_interface_id)
            })
            .map(|(allocation_id, _)| allocation_id.clone())
    }

    /// The allocation id of the address whose association is `association_id`, or `None`
    /// when no address of the account carries it.
    ///
    /// The id `DisassociateAddress` names an association by — "the association ID. This
    /// parameter is required" (`DisassociateAddressRequest$AssociationId`) — resolved
    /// against the records the association was written on.
    pub fn allocation_id_of_elastic_ip_association(&self, association_id: &str) -> Option<String> {
        self.addresses
            .iter()
            .find(|(_, address)| address.association_id.as_deref() == Some(association_id))
            .map(|(allocation_id, _)| allocation_id.clone())
    }

    /// Whether the address `allocation_id` already carries exactly the association `spec`
    /// describes.
    ///
    /// "This is an idempotent operation. If you perform the operation more than once,
    /// Amazon EC2 doesn't return an error" (`AssociateAddress`): an association that
    /// already holds keeps the id it was given, so the standing one is answered rather than
    /// replaced, and nothing a later reader of either store sees changes.
    pub fn elastic_ip_association_stands(
        &self,
        allocation_id: &str,
        spec: &ElasticIpAssociationSpec,
    ) -> Option<String> {
        let address = self.addresses.get(allocation_id)?;
        let same = address.network_interface_id == spec.network_interface_id
            && address.instance_id == spec.instance_id
            && address.private_ip_address == spec.private_ip_address;
        same.then(|| address.association_id.clone()).flatten()
    }

    /// Record the association `spec` on the address `allocation_id` and answer the
    /// `AssociationId` it was given, or `None` when the account holds no such address.
    ///
    /// The ONE writer of an Elastic IP association, so every path that makes one — by
    /// instance id or by network interface id — leaves the same record behind. It performs
    /// the remap both ways round first: the address leaves whatever it was on, and the
    /// interface (and the instance holding it) gives up whatever address it was reporting,
    /// because "if you associate an Elastic IP address with an instance that has an existing
    /// Elastic IP address, the existing address is disassociated from the instance, but
    /// remains allocated to your account" (`AssociateAddress`).
    ///
    /// The association id is the service's to mint, never the caller's, and comes from the
    /// store's own monotonic source, so no two associations share one. Nothing
    /// caller-chosen is stored — the ids are minted or already held and the private address
    /// is the interface's own — so there is no byte charge to make.
    pub fn associate_elastic_ip(
        &mut self,
        allocation_id: &str,
        spec: &ElasticIpAssociationSpec,
    ) -> Option<String> {
        if !self.addresses.contains_key(allocation_id) {
            return None;
        }
        // Both sides are freed before either is written, through the one reverse primitive,
        // so no record is left naming an association the other no longer has.
        self.detach_elastic_ip(allocation_id);
        if let Some(network_interface_id) = spec.network_interface_id.as_deref() {
            if let Some(displaced) = self.allocation_id_associated_with_interface(network_interface_id)
            {
                self.detach_elastic_ip(&displaced);
            }
        }
        if let Some(instance_id) = spec.instance_id.as_deref() {
            if let Some(displaced) = self.allocation_id_associated_with_instance(instance_id) {
                self.detach_elastic_ip(&displaced);
            }
        }

        let association_id = self.next_id(ELASTIC_IP_ASSOCIATION_ID_PREFIX);
        let ip_owner_id = ::engine::emulator_account_id().to_string();
        let address = self.addresses.get_mut(allocation_id)?;
        address.association_id = Some(association_id.clone());
        address.instance_id = spec.instance_id.clone();
        address.network_interface_id = spec.network_interface_id.clone();
        // "The ID of the Amazon Web Services account that owns the network interface"
        // (`Address$NetworkInterfaceOwnerId`): this account holds every interface this
        // store has, and an association naming no interface names no owner either.
        address.network_interface_owner_id = spec
            .network_interface_id
            .as_ref()
            .map(|_| ip_owner_id.clone());
        address.private_ip_address = spec.private_ip_address.clone();
        let public_ip = address.public_ip.clone();

        // The interface's own half of the association: "the association information for an
        // Elastic IP address (IPv4) associated with the network interface"
        // (`NetworkInterface$Association`), carried on the interface AND on the private
        // address it is bound to, which is where `DescribeNetworkInterfaces` reports each.
        if let Some(network_interface_id) = spec.network_interface_id.as_deref() {
            let region = self
                .network_interfaces
                .get(network_interface_id)
                .and_then(|interface| interface.availability_zone.clone())
                .map(|zone| region_of_zone_name(&zone).to_string())
                .unwrap_or_default();
            let association = NetworkInterfaceAssociation {
                allocation_id: Some(allocation_id.to_string()),
                association_id: Some(association_id.clone()),
                // An Elastic IP the ACCOUNT allocated is owned by that account, where an
                // address AWS assigned itself reports `amazon`
                // (`NetworkInterfaceAssociation$IpOwnerId`).
                ip_owner_id: Some(ip_owner_id),
                public_dns_name: public_ip
                    .as_deref()
                    .map(|public_ip| public_dns_name(public_ip, &region)),
                public_ip: public_ip.clone(),
                // Both belong to address pools this emulator allocates no address out of:
                // a customer-owned pool and a carrier's.
                customer_owned_ip: None,
                carrier_ip: None,
            };
            if let Some(interface) = self.network_interfaces.get_mut(network_interface_id) {
                interface.association = Some(association.clone());
                // The private address the Elastic IP is bound to carries the association
                // too; an interface that holds none has nothing to carry it.
                if let Some(bound_to) = spec.private_ip_address.as_deref() {
                    if let Some(held) = interface
                        .private_ip_addresses
                        .iter_mut()
                        .flatten()
                        .find(|held| held.private_ip_address.as_deref() == Some(bound_to))
                    {
                        held.association = Some(association);
                    }
                }
            }
        }

        // The instance now reports the Elastic IP address as its public address
        // (`Instance$PublicIpAddress`), which is what makes the association visible from
        // the instance's side as well.
        if let Some(instance) = spec
            .instance_id
            .as_deref()
            .and_then(|instance_id| self.instances.get_mut(instance_id))
        {
            instance.public_ip = public_ip;
        }

        Some(association_id)
    }

    /// Take the association of the address `allocation_id` off the interface
    /// `network_interface_id`, with the private address it was bound to.
    ///
    /// The interface half of [`Self::detach_elastic_ip`], which is its one caller: an
    /// interface holding some OTHER address — an Elastic IP of another allocation, or a
    /// public address AWS assigned it, which carries no allocation id at all — keeps what it
    /// holds, so ending one association cannot take another away.
    pub(super) fn clear_interface_elastic_ip_association(
        &mut self,
        network_interface_id: &str,
        allocation_id: &str,
    ) {
        let holds_this = |association: &NetworkInterfaceAssociation| {
            association.allocation_id.as_deref() == Some(allocation_id)
        };
        let Some(interface) = self.network_interfaces.get_mut(network_interface_id) else {
            return;
        };
        if !interface.association.as_ref().is_some_and(holds_this) {
            return;
        }
        interface.association = None;
        for held in interface.private_ip_addresses.iter_mut().flatten() {
            if held.association.as_ref().is_some_and(holds_this) {
                held.association = None;
            }
        }
    }
}
