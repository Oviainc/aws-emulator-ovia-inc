// SPDX-License-Identifier: CC0-1.0

//! The notation and the ORDER an IP set holds its `Addresses` in.
//!
//! `IPSet$Addresses` "contains an array of strings that specifies zero or more IP
//! addresses or blocks of IP addresses that you want WAF to inspect for in incoming
//! requests", and what WAF holds is the SET those strings name: a block named twice
//! is one entry, and the order the array happens to arrive in is not part of the
//! set. The order a READ reports is therefore WAF's own — derived from the entries
//! themselves, not from the request — and recorded WAF responses show it: an
//! address list comes back in neither the submitted nor the sorted order, and the
//! same membership comes back the same way every time.
//!
//! Both halves of that are what makes a read usable: a client that compares
//! `GetIPSet` against its intended configuration sees the same list for the same
//! membership whichever order it wrote it in, so an idempotent apply does not
//! rewrite the set, and the stored list is a fixed point — resubmitting what a read
//! reported stores exactly that.

#[allow(unused_imports)]
use super::*;

/// The smallest table the set's entries are laid out in, in buckets: the floor a
/// hash table keeps even for a membership far below it.
const ADDRESS_TABLE_MIN_BUCKETS: usize = 16;

/// How full the table is allowed to get before it doubles, as a fraction: three
/// quarters.
const ADDRESS_TABLE_LOAD_NUMERATOR: usize = 3;
/// See [`ADDRESS_TABLE_LOAD_NUMERATOR`].
const ADDRESS_TABLE_LOAD_DENOMINATOR: usize = 4;

/// The multiplier the string hash below accumulates with: the odd prime a
/// polynomial string hash is classically specified at.
const ADDRESS_HASH_MULTIPLIER: i32 = 31;

/// The form an IP set's `Addresses` are STORED in, for the family the set declares.
///
/// Two steps, in order:
///
/// 1. Each entry is taken to the one notation of its value. An IPv6 address has
///    many — `::` elides a run of zero groups, a group's leading zeros may be
///    dropped, and the hex digits may be in either case — so an `IPV6` entry is
///    stored fully expanded, zero-padded and lowercase, with the prefix length it
///    was given. An IPv4 address has a single notation, so an `IPV4` entry is
///    stored exactly as submitted. An entry this does not recognize as an address
///    of the family (which [`validate_cidr`] refuses on ingress) is kept verbatim
///    rather than silently changed.
/// 2. The entries are laid out as the SET they are: a value that appears twice is
///    one entry, and the list is ordered by [`address_set_order`] — the set's own
///    order, which does not depend on the order the request listed them in.
pub fn canonical_ip_set_addresses(ip_address_version: &str, addresses: &[String]) -> Vec<String> {
    let mut members: Vec<String> = Vec::with_capacity(addresses.len());
    let mut seen: ::std::collections::HashSet<String> =
        ::std::collections::HashSet::with_capacity(addresses.len());
    for address in addresses {
        let canonical = if ip_address_version == "IPV6" {
            canonical_ipv6_cidr(address).unwrap_or_else(|| address.clone())
        } else {
            address.clone()
        };
        if seen.insert(canonical.clone()) {
            members.push(canonical);
        }
    }
    address_set_order(members)
}

/// One IPv6 CIDR block in the fully expanded, zero-padded, lowercase notation of
/// its value, with its prefix length; `None` when `address` is not an IPv6 CIDR
/// block. See [`canonical_ip_set_addresses`].
fn canonical_ipv6_cidr(address: &str) -> Option<String> {
    let (literal, prefix) = address.split_once('/')?;
    let parsed = literal.parse::<::std::net::Ipv6Addr>().ok()?;
    let prefix_len = prefix.parse::<u32>().ok()?;
    let groups: Vec<String> = parsed
        .segments()
        .iter()
        .map(|group| format!("{group:04x}"))
        .collect();
    Some(format!("{}/{prefix_len}", groups.join(":")))
}

/// `members` in the order the set holds them: the order a read reports.
///
/// The set is laid out in a table, so the order its members are enumerated in is
/// the table's: an entry sits in the slot its own hash ([`address_bucket_hash`])
/// selects, and the slots are enumerated in ascending order. The table has
/// [`ADDRESS_TABLE_MIN_BUCKETS`] buckets and doubles whenever the membership would
/// fill more than
/// [`ADDRESS_TABLE_LOAD_NUMERATOR`]/[`ADDRESS_TABLE_LOAD_DENOMINATOR`] of it, so a
/// set's layout is fixed by its size and its members' values alone.
///
/// Two entries can select one bucket, and then the bucket alone does not say which
/// of them comes first; what decides it is their own values, compared as the
/// strings they are stored as. The position each held in the request decides
/// nothing — which is the rule this exists for: the order is a function of the
/// MEMBERSHIP, not of the request. The same addresses submitted in any order are
/// stored in one order, whichever write path stored them, and reading a set twice
/// reports one order.
pub fn address_set_order(members: Vec<String>) -> Vec<String> {
    let buckets = address_table_buckets(members.len());
    let mut ordered: Vec<(usize, String)> = members
        .into_iter()
        .map(|member| (address_bucket(&member, buckets), member))
        .collect();
    // By bucket, then by the entry's own value: the two keys the layout is a
    // function of, which is the pair order on these entries. Neither depends on
    // the order the request listed them in, and the entries are distinct, so no
    // two compare equal and the sort needs no stability.
    ordered.sort_unstable();
    ordered
        .into_iter()
        .map(|(_bucket, member)| member)
        .collect()
}

/// How many buckets the table holding `members` entries has: a power of two, at
/// least [`ADDRESS_TABLE_MIN_BUCKETS`], doubled until the entries occupy no more
/// than the load factor allows.
fn address_table_buckets(members: usize) -> usize {
    let mut buckets = ADDRESS_TABLE_MIN_BUCKETS;
    while members * ADDRESS_TABLE_LOAD_DENOMINATOR
        > buckets * ADDRESS_TABLE_LOAD_NUMERATOR
    {
        match buckets.checked_mul(2) {
            Some(doubled) => buckets = doubled,
            // Unreachable for any membership a set may hold (`MAX_IP_SET_ADDRESSES`
            // is 10,000): stated here so the growth cannot wrap instead.
            None => break,
        }
    }
    buckets
}

/// The bucket `member` sits in, in a table of `buckets` buckets.
///
/// The table masks the hash down to its size, so the high half of the hash would
/// never be looked at; it is folded into the low half first (hash exclusive-or its
/// own top sixteen bits), which is what keeps two entries differing only in their
/// high bits out of one bucket.
fn address_bucket(member: &str, buckets: usize) -> usize {
    let hash = address_bucket_hash(member);
    let spread = hash ^ ((hash as u32) >> 16) as i32;
    (spread as u32 as usize) & buckets.saturating_sub(1)
}

/// The hash of one entry: the polynomial hash of its characters, accumulated over
/// the 16-bit code units the string is made of and wrapping at 32 bits.
///
/// This is not a hash chosen FOR hashing — `std`'s own hasher would serve that, and
/// a crate's would serve it better. It is the function that decides which bucket an
/// entry sits in, and so decides the ORDER a read reports: another function would
/// answer another order, which is the one thing this must not do.
fn address_bucket_hash(member: &str) -> i32 {
    member.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(ADDRESS_HASH_MULTIPLIER)
            .wrapping_add(i32::from(unit))
    })
}
