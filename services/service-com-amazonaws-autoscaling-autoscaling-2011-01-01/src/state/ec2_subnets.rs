// SPDX-License-Identifier: CC0-1.0

//! The Availability Zones of the subnets an Auto Scaling request names, read from the
//! account's own Amazon EC2 store.
//!
//! `VPCZoneIdentifier` is "a comma-separated list of subnet IDs for a virtual private
//! cloud (VPC) where instances in the Auto Scaling group can be created"
//! (`CreateAutoScalingGroup$VPCZoneIdentifier`). A subnet exists in exactly one
//! Availability Zone, and the zone it is in is a fact about that subnet rather than a
//! value the Auto Scaling caller supplies — so a group that names only subnets still has
//! zones, and they are the zones of those subnets. This module is the one place that
//! resolution happens: the subnets live in Amazon EC2's own store, reached through EC2's
//! own accessor, so one subnet is never described by two services differently.
//!
//! LOCKING. [`zones_of`] takes EC2's store lock for the one pass over the named subnets
//! that answers it and never awaits while holding it. Callers in `src/ops/` resolve the
//! zones BEFORE taking this service's own lock, so the two stores are never held at once;
//! EC2 never reaches this service, so no cycle exists.

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

/// The subnet ids a `VPCZoneIdentifier` value names, in the order the caller wrote them.
///
/// The member is one comma-separated string, so the ids are what the commas separate,
/// with surrounding whitespace trimmed and empty entries dropped — a trailing comma names
/// no subnet.
pub fn subnet_ids(vpc_zone_identifier: &str) -> Vec<&str> {
    vpc_zone_identifier
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect()
}

/// What came of resolving a `VPCZoneIdentifier` against the account's EC2 subnets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneResolution {
    /// The zone of each named subnet, deduplicated, in the order the subnets were named:
    /// the Availability Zones of the group.
    Zones(Vec<String>),
    /// This id names no subnet the group could launch into: the account's EC2 store holds
    /// none with that id, or the one it holds is not in the Region the request was signed
    /// for. Carries the id the request named.
    UnknownSubnet(String),
}

/// The refusal for a subnet id that names no subnet of the signed Region.
///
/// ONE spelling for both handlers. The sentence is the one Amazon EC2 states for an id it
/// holds no subnet for (its `invalid_subnet_id_not_found`) — this resolution IS that
/// lookup — carried under the error code Amazon EC2 Auto Scaling answers a request it
/// cannot satisfy with. The id comes from the request.
pub fn unknown_subnet_error<E>(subnet_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::validation_error(format!(
        "The subnet ID '{subnet_id}' does not exist"
    ))
}

/// The refusal for a request that leaves the group with nowhere to place an instance:
/// neither a zone nor a subnet. ONE spelling for both handlers.
pub fn no_placement_error<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::validation_error(
        "At least one Availability Zone or VPC Subnet is required: specify AvailabilityZones \
         or VPCZoneIdentifier",
    )
}

/// The Availability Zones of the subnets `vpc_zone_identifier` names, as the Region
/// `region` holds them.
///
/// Every named subnet is resolved — a request naming one subnet the account cannot launch
/// into names it whichever position it is in — and the zones are deduplicated, because two
/// subnets of one zone place instances in that one zone.
///
/// A subnet resolves only to a zone OF `region`: a subnet is identified within the Region
/// it is in, and the group is created in the Region the request is signed for, so a subnet
/// of some other Region is as unresolvable here as one the account does not hold. Which
/// zones a Region has is EC2's own catalog ([`ec2::availability_zones_in_region`]), the
/// same one its own subnets are placed from. An id that resolves to nothing answers
/// [`ZoneResolution::UnknownSubnet`], naming it, and so does a subnet the store holds
/// without a zone: neither yields a zone to launch into.
pub fn zones_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    vpc_zone_identifier: &str,
) -> ZoneResolution {
    let ids = subnet_ids(vpc_zone_identifier);
    let Some(state) = ec2::service_state(engine) else {
        // No EC2 store on this engine is no subnet state to read, so the first named
        // subnet is as unresolvable as one the store does not hold.
        return match ids.first() {
            Some(id) => ZoneResolution::UnknownSubnet((*id).to_string()),
            None => ZoneResolution::Zones(Vec::new()),
        };
    };
    let zones_of_region = ec2::availability_zones_in_region(region);
    let store = state.lock();
    let mut zones: Vec<String> = Vec::new();
    for id in ids {
        let zone = store
            .subnets
            .get(id)
            .and_then(|subnet| subnet.availability_zone.clone())
            .filter(|zone| {
                zones_of_region
                    .iter()
                    .any(|of_region| of_region.zone_name == *zone)
            });
        let Some(zone) = zone else {
            return ZoneResolution::UnknownSubnet(id.to_string());
        };
        if !zones.contains(&zone) {
            zones.push(zone);
        }
    }
    ZoneResolution::Zones(zones)
}
