// SPDX-License-Identifier: CC0-1.0

//! Where the account's network puts a Classic load balancer: whether the VPC it is in is
//! the account's DEFAULT VPC.
//!
//! That is the one fact the EC2-Classic zone API turns on. `AvailabilityZones` is how a
//! load balancer "in EC2-Classic or a default VPC" is placed, and a load balancer in a VPC
//! of the account's own is placed by its SUBNETS instead — so the zone-membership call is
//! refused for it and `AttachLoadBalancerToSubnets` is the operation that moves it.
//!
//! A VPC is not this service's resource: which VPCs the account holds, in which Region, and
//! which one of them is its default is EC2's to say, so the answer is read from EC2's own
//! store — the same store a health check reads the instance inventory from
//! ([`super::observe_instances`]) — rather than held a second time here. With no EC2 to
//! ask, or for a VPC EC2 holds no record of, NOTHING is known: nothing is refused on that
//! silence.

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

/// The sentence `EnableAvailabilityZonesForLoadBalancer` refuses a load balancer of a
/// non-default VPC with.
///
/// Entirely fixed text: it states which load balancers the EC2-Classic zone API applies to
/// at all, and no part of it comes from the request.
pub const ZONES_IN_NON_DEFAULT_VPC_MESSAGE: &str =
    "You cannot enable Availability Zones for ELBs in a non default VPC";

/// Whether `vpc_id` is a VPC the account holds in `region` that is NOT its default one —
/// the placement the EC2-Classic zone API does not apply to.
///
/// `false` for a load balancer in no VPC at all (EC2-Classic, which the operation is
/// documented for) and for one in the account's default VPC (which it is documented for
/// too). `false` as well where nothing is known: an emulator with no EC2 mounted, a VPC
/// EC2 holds no record of, a VPC of another Region — EC2 being regional, a request signed
/// for one Region learns nothing about another's network — and a VPC EC2 reports nothing
/// either way about, so only a VPC it POSITIVELY reports as not the default one is a
/// non-default placement.
pub fn in_non_default_vpc<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    vpc_id: Option<&str>,
) -> bool {
    let Some(vpc_id) = vpc_id else {
        return false;
    };
    let Some(network) = ec2::service_state(engine) else {
        return false;
    };
    let store = network.lock();
    if !store.vpc_is_in_region(Some(vpc_id), region) {
        return false;
    }
    // `Vpc$IsDefault` is "indicates whether the VPC is the default VPC", so a VPC EC2
    // reports it `false` for is one of the account's own — the ones a load balancer is
    // placed in by subnet. A record EC2 reports the flag for neither way says nothing, and
    // nothing is what is refused on it.
    store
        .vpcs
        .get(vpc_id)
        .is_some_and(|vpc| vpc.is_default == Some(false))
}
