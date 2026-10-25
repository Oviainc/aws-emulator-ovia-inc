// SPDX-License-Identifier: CC0-1.0

//! The `CreatePublicIpv4Pool` side of the public IPv4 pool store: what a create may choose
//! about the pool it mints, and the refusal for a create this store cannot hold.
//!
//! The pool RECORD itself ([`super::PublicIpv4PoolRecord`]), the insert that mints its id
//! and charges its bytes, the removal, and the projection `DescribePublicIpv4Pools` reports
//! it through all live in the parent module's "Public IPv4 address pools" section, beside
//! the BYOIP provision that also creates a pool — so a pool has ONE definition and the
//! create cannot report one differently. Only the rules a create ADDS live here.

use super::{
    byoip_network_border_group_allowed, invalid_parameter_value, network_border_group_of_region,
    resource_limit_exceeded, AddressFamily, MAX_STORED_BYTES, MAX_STORED_RECORDS_PER_STORE,
};
use crate::scaffolded::Scaffolded;

/// The network border group the pool a create mints is advertised from, for a request signed
/// for `region` that asked for `requested`.
///
/// "The Availability Zone (AZ) or Local Zone (LZ) network border group that the resource
/// that the IP address is assigned to is in. Defaults to an AZ network border group"
/// (`CreatePublicIpv4PoolRequest$NetworkBorderGroup`): a request that names none advertises
/// from the Region it was signed for, every Availability Zone of a Region sharing the one
/// group named for it ([`network_border_group_of_region`]), and a request that names one may
/// name that Region or one of its Local Zone groups. Which groups a Region advertises
/// addresses of a family from is decided by the ONE helper that knows them
/// ([`byoip_network_border_group_allowed`]), so the pool surface and the BYOIP surface
/// cannot disagree about a group; a public IPv4 pool holds IPv4 addresses only
/// (`PublicIpv4PoolRange$FirstAddress` is a dotted quad), so the family is always IPv4 here.
///
/// Anything else names a location this Region does not advertise from, which is HTTP 400
/// `InvalidParameterValue` — the code EC2 answers a member value it cannot accept with, and
/// the one the provision answers the same mistake with. The refusal NAMES the value the
/// request sent, so a caller can tell which member was rejected from the text alone.
pub fn public_ipv4_pool_network_border_group<E>(
    requested: Option<&str>,
    region: &str,
) -> Result<String, Scaffolded<E>> {
    match requested {
        None => Ok(network_border_group_of_region(region)),
        Some(group)
            if byoip_network_border_group_allowed(group, region, AddressFamily::Ipv4) =>
        {
            Ok(group.to_string())
        }
        Some(group) => Err(invalid_parameter_value(format!(
            "Value ({group}) for parameter networkBorderGroup is invalid. \
             A public IPv4 pool can only be advertised from {region} or from one of its \
             Local Zone network border groups."
        ))),
    }
}

/// Refuse a create the store has no room for: HTTP 400 `ResourceLimitExceeded`.
///
/// Both ceilings the store enforces answer here, because
/// [`super::ComAmazonawsEc2Amazonec2State::insert_public_ipv4_pool`] refuses for either and
/// a caller can do the same thing about both — the record count
/// ([`MAX_STORED_RECORDS_PER_STORE`]) and the caller-supplied bytes every store of this
/// service shares ([`MAX_STORED_BYTES`]). "You have exceeded an Amazon EC2 resource limit"
/// (EC2 API error codes) is the code this service answers a ceiling of its own with
/// ([`resource_limit_exceeded`]): EC2 publishes no `PublicIpv4PoolLimitExceeded`, and a
/// code this emulator invented would be a code no client branches on. DELIBERATELY not the
/// `<Resource>LimitExceeded` convention the neighbouring BYOIP ceiling follows
/// ([`super::byoip_cidr_limit_exceeded`], whose `ByoipCidrLimitExceeded` IS a code the real
/// service answers) — the convention holds only where AWS publishes the resource's own
/// code, and for a public IPv4 pool it does not.
pub fn public_ipv4_pool_store_full<E>() -> Scaffolded<E> {
    resource_limit_exceeded(format!(
        "This emulator holds at most {MAX_STORED_RECORDS_PER_STORE} public IPv4 pools and \
         at most {MAX_STORED_BYTES} bytes of caller-supplied data, and one of the two is \
         reached"
    ))
}
