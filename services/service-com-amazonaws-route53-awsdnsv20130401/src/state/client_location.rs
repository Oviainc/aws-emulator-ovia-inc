// SPDX-License-Identifier: CC0-1.0

//! Where the query a geolocation record set routes by appears to come FROM.
//!
//! A geolocation record set is answered by the location of the client that asked,
//! so answering one needs two things: the address the query appears to come from,
//! and the location that address sits in. The first is in the request —
//! `TestDNSAnswer`'s `EDNS0ClientSubnetIP`/`EDNS0ClientSubnetMask` ("if you
//! specify 192.0.2.44 for edns0clientsubnetip and 24 for edns0clientsubnetmask,
//! the checking tool will simulate a request from 192.0.2.0/24") and, when the
//! request names no client subnet, its `ResolverIP`. The second is what this
//! module is: a lookup from an address to the continent and country it is
//! registered in.
//!
//! ## Where the blocks come from, and what follows from that
//!
//! [`REGISTERED_BLOCKS`] is drawn from the public delegation records the regional
//! internet registries publish for the IPv4 address space — the `cc` (country)
//! field of an allocation, and the registry that administers it — which is the
//! same kind of data an authoritative name server resolves a client subnet with.
//! It is a SUBSET of those records, not all of them: no address block is invented
//! here, and an address no block in the table covers is simply one this emulator
//! cannot place.
//!
//! That is not a hole in the behaviour, because the service documents the answer
//! for exactly that case. `GeoLocation`'s own documentation: "Two groups of
//! queries are routed to the resource that you specify in this record: queries
//! that come from locations for which you haven't created geolocation resource
//! record sets and queries from IP addresses that aren't mapped to a location. If
//! you don't create a `*` resource record set, Route 53 returns a 'no answer'
//! response for queries from those locations." An unplaceable address is one of
//! those, so it is answered by the group's `*` member, or by no record at all.
//!
//! What matters about the table, therefore, is that a lookup in it is a FUNCTION
//! of the address — two queries from the same client subnet place alike, and two
//! from different countries place differently — never that a particular block is
//! in it.
//!
//! Nothing here is ever contacted: an address in the table is only ever compared,
//! as a number, against an address a request carried.

use std::net::Ipv4Addr;

/// The widest IPv4 prefix there is: an address is 32 bits, so a mask may name at
/// most that many of them (`EDNS0ClientSubnetMask`'s own documentation: "IPv4:
/// Specify a value between 0 and 32").
const IPV4_BITS: u32 = 32;

/// How many bits of an IPv4 client address a query is simulated from when the
/// request names an address but no mask.
///
/// `EDNS0ClientSubnetMask`'s own documentation: "The default value is 24 bits for
/// IPv4 addresses and 64 bits for IPv6 addresses."
const DEFAULT_IPV4_CLIENT_SUBNET_MASK: u32 = 24;

/// The location one address block is registered in.
///
/// The codes are the ones the API speaks: `ContinentCode` is one of the seven
/// `GeoLocation$ContinentCode` documents (AF, AN, AS, EU, OC, NA, SA) and
/// `CountryCode` is an ISO 3166-1 alpha-2 code, which is what
/// `GeoLocation$CountryCode` documents ("Amazon Route 53 uses the two-letter
/// country codes that are specified in ISO standard 3166-1 alpha-2").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientLocation {
    /// The continent the address is registered in.
    pub continent_code: &'static str,
    /// The country the address is registered in.
    pub country_code: &'static str,
    /// The subdivision (a state of the United States, which is the only kind
    /// `GeoLocation$SubdivisionCode` admits) the address is registered in.
    ///
    /// The delegation records [`REGISTERED_BLOCKS`] is drawn from carry a country
    /// per allocation and nothing finer, so no block in it places an address
    /// inside a country; a record set that routes by a subdivision therefore
    /// covers no query yet, exactly as a country no block names covers none.
    pub subdivision_code: Option<&'static str>,
}

/// Blocks of the IPv4 address space and the location each is registered in, as
/// `(network address, prefix length, continent code, country code)`.
///
/// Source: the regional registries' published delegation records, as described in
/// this module's own documentation — the holder named beside each row is the
/// organization that block is delegated to. The rows span the six inhabited
/// continents the API names and thirteen countries, in no particular order: a
/// lookup takes the LONGEST prefix that covers the address ([`locate`]), so the
/// order of the table says nothing about the answer and a narrow block inside a
/// wider one places an address more precisely than the wider one does.
const REGISTERED_BLOCKS: &[(&str, u32, &str, &str)] = &[
    // Europe (RIPE NCC).
    ("62.154.0.0", 15, "EU", "DE"),     // Deutsche Telekom AG
    ("217.0.0.0", 13, "EU", "DE"),      // Deutsche Telekom AG
    ("193.252.0.0", 16, "EU", "FR"),    // Orange S.A.
    ("193.0.0.0", 21, "EU", "NL"),      // RIPE NCC, Amsterdam
    ("192.36.148.0", 24, "EU", "SE"),   // Netnod, Stockholm
    ("212.58.224.0", 19, "EU", "GB"),   // British Broadcasting Corporation
    // Asia (APNIC).
    ("210.128.0.0", 11, "AS", "JP"),    // JPNIC
    ("202.12.27.0", 24, "AS", "JP"),    // WIDE Project, Tokyo
    ("223.5.5.0", 24, "AS", "CN"),      // Alibaba Cloud public resolver
    ("119.29.29.0", 24, "AS", "CN"),    // Tencent public resolver
    ("168.126.63.0", 24, "AS", "KR"),   // KT Corporation public resolver
    // North America (ARIN).
    ("8.8.8.0", 24, "NA", "US"),        // Google public resolver
    ("208.67.222.0", 24, "NA", "US"),   // Cisco OpenDNS public resolver
    ("192.5.5.0", 24, "NA", "US"),      // Internet Systems Consortium
    ("4.0.0.0", 8, "NA", "US"),         // Level 3 Parent, LLC
    ("24.0.0.0", 12, "NA", "US"),       // Comcast Cable Communications
    ("149.112.121.0", 24, "NA", "CA"),  // Canadian Internet Registration Authority
    // Oceania (APNIC).
    ("1.128.0.0", 11, "OC", "AU"),      // Telstra
    ("139.130.0.0", 16, "OC", "AU"),    // Telstra
    // South America (LACNIC).
    ("200.160.0.0", 20, "SA", "BR"),    // Núcleo de Informação e Coordenação do Ponto BR
    // Africa (AFRINIC).
    ("196.25.0.0", 16, "AF", "ZA"),     // Telkom SA
];

/// The location `address` sits in, or `None` when no block covers it.
///
/// The covering block with the LONGEST prefix wins, which is how a routing
/// decision reads a table of address blocks: the most specific block that holds
/// an address is the one that says where it is.
pub fn locate(address: Ipv4Addr) -> Option<ClientLocation> {
    REGISTERED_BLOCKS
        .iter()
        .filter(|(network, bits, _, _)| covers(network, *bits, address))
        .max_by_key(|(_, bits, _, _)| *bits)
        .map(|(_, _, continent_code, country_code)| ClientLocation {
            continent_code,
            country_code,
            // Country granularity: see [`ClientLocation::subdivision_code`].
            subdivision_code: None,
        })
}

/// Whether the block `network/bits` holds `address`.
///
/// A block whose network address is not a well-formed IPv4 address holds
/// nothing, which keeps a malformed row in the table from placing every query
/// rather than none.
fn covers(network: &str, bits: u32, address: Ipv4Addr) -> bool {
    let Ok(network) = network.parse::<Ipv4Addr>() else {
        return false;
    };
    mask_to(u32::from(network), bits) == mask_to(u32::from(address), bits)
}

/// `address` with every bit below the top `bits` cleared — the network address of
/// the subnet that names `bits` of it.
fn mask_to(address: u32, bits: u32) -> u32 {
    match bits.min(IPV4_BITS) {
        0 => 0,
        bits => address & (u32::MAX << (IPV4_BITS - bits)),
    }
}

/// The location the EDNS0 client subnet `address`/`mask` appears to be in.
///
/// The request names an address and, optionally, how many of its bits the query
/// is simulated from, so the subnet it stands for is resolved rather than the
/// address: the location is looked up for the subnet's network address, which is
/// why a subnet too wide to sit inside one registered block places nowhere
/// instead of placing in whichever country its first address belongs to.
///
/// Only IPv4 is placed. The blocks this module holds are IPv4 delegations, so an
/// IPv6 client subnet is an address "that [isn't] mapped to a location" and is
/// answered by the group's default member, the same as any other unplaceable
/// address.
pub fn locate_client_subnet(address: &str, mask: Option<&str>) -> Option<ClientLocation> {
    let address = address.parse::<Ipv4Addr>().ok()?;
    let bits = mask
        .and_then(|mask| mask.parse::<u32>().ok())
        .unwrap_or(DEFAULT_IPV4_CLIENT_SUBNET_MASK)
        .min(IPV4_BITS);
    locate(Ipv4Addr::from(mask_to(u32::from(address), bits)))
}

/// The location the single address `address` appears to be in — a resolver's own
/// address, which carries no mask of its own.
pub fn locate_address(address: &str) -> Option<ClientLocation> {
    locate(address.parse::<Ipv4Addr>().ok()?)
}
