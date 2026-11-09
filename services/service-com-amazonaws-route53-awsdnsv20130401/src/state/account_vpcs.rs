// SPDX-License-Identifier: CC0-1.0

//! The VPCs the calling account holds, which is what a Route 53 request naming a
//! VPC is resolved against.
//!
//! An Amazon VPC is not a Route 53 resource: it belongs to the account's own
//! Amazon EC2 inventory — the one `DescribeVpcs` answers from — so whether a
//! request names a VPC of this account is a fact of THAT store, reached through
//! the shared engine, and never of Route 53's own association records. A zone
//! association is a reference to a VPC, not a claim that the VPC exists: a
//! private zone keeps its associations while the VPC behind one is anything at
//! all, so reading ownership off them would report a VPC the account does not
//! hold as one it does, and would make a VPC stop being the account's the moment
//! its last association was removed.
//!
//! EC2 is a REGIONAL service: a VPC belongs to the Region it was created in, and
//! a request naming another Region names no VPC at all. Route 53 takes both —
//! `VPC$VPCId` and `VPC$VPCRegion` ("the region that an Amazon VPC was created
//! in") — so the pair is what is resolved here, through Amazon EC2's own reading
//! of which Region a VPC is in.
//!
//! A pure read: nothing is written to either store. The Amazon EC2 store's lock
//! is taken for this one pass and released before returning, and a handler calls
//! this with no lock of its own held — the two stores are never locked at once,
//! exactly as in [`super::monitored_alarm`].

use ::service_com_amazonaws_ec2_amazonec2 as ec2;

/// Whether the account holds the VPC `vpc_id` in `vpc_region`.
pub fn account_holds_vpc<Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_id: &str,
    vpc_region: &str,
) -> bool {
    let Some(state) = ec2::state::service_state(engine) else {
        // No Amazon EC2 store on this engine is no inventory to resolve against,
        // so no VPC can be shown to be one the account does not hold.
        return true;
    };
    let store = state.lock();
    store.vpcs.contains_key(vpc_id) && store.vpc_is_in_region(Some(vpc_id), vpc_region)
}

/// The sentence Route 53 refuses a request about a VPC the calling account does
/// not hold with (`AccessDenied`, HTTP 403).
///
/// Built wholly from the REQUEST — the VPC id and the Region it was asked about,
/// verbatim — because that is the pair that resolved to no VPC of this account;
/// the refusal claims nothing about what the account does hold.
pub fn vpc_not_owned_message(vpc_id: &str, vpc_region: &str) -> String {
    format!("The VPC: {vpc_id} in region {vpc_region} that you provided is not owned by you.")
}
