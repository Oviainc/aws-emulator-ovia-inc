// SPDX-License-Identifier: CC0-1.0

//! One membership, ONE order — whichever order the request listed the blocks in.
//!
//! `IPSet$Addresses` "contains an array of strings that specifies zero or more IP
//! addresses or blocks of IP addresses that you want WAF to inspect for in incoming
//! requests", so what an IP set holds is the SET those strings name: the array is a
//! way of writing a membership down, not part of it. The RULE asserted here is the
//! consequence for a read: the order `GetIPSet` reports is a function of the
//! MEMBERSHIP alone, so two sets holding the same blocks read back identically no
//! matter which order each was written in, and either write path (`CreateIPSet` or
//! `UpdateIPSet`) stores the same list.
//!
//! [`crate::ip_set_address_order`] pins the order itself for a few memberships and
//! walks three orders of each. This suite is the invariant over ALL of them: every
//! permutation of the request's list, for memberships that differ in size, in
//! family, in how far apart the blocks are, and in how many of them differ only in
//! their prefix length — where two blocks can land in the same place in the layout
//! and the request order would otherwise decide which comes first.
//!
//! Clients diff a read against the configuration they applied, so an order that
//! depended on the request would make an unchanged configuration look changed, and
//! the `LockToken` round trip would turn that into a write that was never needed.

use serde_json::json;

use crate::ip_set_mutations::{addresses, create_ip_set, get_ip_set, membership_of, sorted};
use crate::web_acl_lifecycle::Waf;

/// One membership the invariant is asserted over.
struct Membership {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The address family the set is created with.
    version: &'static str,
    /// The blocks the set holds, in the order this row writes them down first.
    members: &'static [&'static str],
    /// The same blocks in the notation they are STORED in, which is what a read
    /// reports: an IPv4 entry as submitted, an IPv6 entry fully expanded (the
    /// notation `crate::ip_set_canonicalization` spans). Compared as a set, so this
    /// is the membership rather than the order.
    stored: &'static [&'static str],
}

/// The memberships walked below: every size from two to five, both families, blocks
/// far apart, blocks adjacent, and blocks that share an address and differ only in
/// their prefix length.
const MEMBERSHIPS: &[Membership] = &[
    Membership {
        case: "two adjacent /24 blocks",
        version: "IPV4",
        members: &["10.0.0.0/24", "10.0.1.0/24"],
        stored: &["10.0.0.0/24", "10.0.1.0/24"],
    },
    Membership {
        case: "three blocks in the private ranges",
        version: "IPV4",
        members: &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"],
        stored: &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"],
    },
    Membership {
        case: "four blocks in the documentation ranges",
        version: "IPV4",
        members: &[
            "203.0.113.0/24",
            "198.51.100.0/24",
            "192.0.2.0/24",
            "198.18.0.0/15",
        ],
        stored: &[
            "203.0.113.0/24",
            "198.51.100.0/24",
            "192.0.2.0/24",
            "198.18.0.0/15",
        ],
    },
    Membership {
        case: "four prefix lengths of one address",
        version: "IPV4",
        members: &[
            "203.0.113.0/24",
            "203.0.113.0/25",
            "203.0.113.0/26",
            "203.0.113.0/32",
        ],
        stored: &[
            "203.0.113.0/24",
            "203.0.113.0/25",
            "203.0.113.0/26",
            "203.0.113.0/32",
        ],
    },
    Membership {
        case: "five adjacent /16 blocks",
        version: "IPV4",
        members: &[
            "10.1.0.0/16",
            "10.2.0.0/16",
            "10.3.0.0/16",
            "10.4.0.0/16",
            "10.5.0.0/16",
        ],
        stored: &[
            "10.1.0.0/16",
            "10.2.0.0/16",
            "10.3.0.0/16",
            "10.4.0.0/16",
            "10.5.0.0/16",
        ],
    },
    Membership {
        case: "four IPv6 blocks written in the short notation",
        version: "IPV6",
        members: &[
            "2001:db8:1::/48",
            "2001:db8:2::/48",
            "2001:db8:3::/48",
            "2001:db8:4::/48",
        ],
        stored: &[
            "2001:0db8:0001:0000:0000:0000:0000:0000/48",
            "2001:0db8:0002:0000:0000:0000:0000:0000/48",
            "2001:0db8:0003:0000:0000:0000:0000:0000/48",
            "2001:0db8:0004:0000:0000:0000:0000:0000/48",
        ],
    },
];

/// Every permutation of `entries`, each exactly once.
///
/// Heap's algorithm, which reaches each permutation by one swap from the last; the
/// row sizes above bound the count at 5! = 120 per row.
fn permutations(entries: &[&'static str]) -> Vec<Vec<&'static str>> {
    let mut working: Vec<&'static str> = entries.to_vec();
    let mut counters = vec![0_usize; working.len()];
    let mut generated = vec![working.clone()];
    let mut index = 0;
    while index < working.len() {
        if counters[index] < index {
            let swap_with = if index % 2 == 0 { 0 } else { counters[index] };
            working.swap(swap_with, index);
            generated.push(working.clone());
            counters[index] += 1;
            index = 0;
        } else {
            counters[index] = 0;
            index += 1;
        }
    }
    generated
}

/// The order a read reports a membership in does not depend on the order the
/// `CreateIPSet` request listed it in.
#[tokio::test]
async fn every_order_of_one_membership_is_created_as_one_list() {
    let waf = Waf::new();
    for (row, membership) in MEMBERSHIPS.iter().enumerate() {
        let Membership {
            case,
            version,
            members,
            stored: expected_members,
        } = *membership;

        // The list the first order stores: what every other order must store too.
        let mut stored: Option<(Vec<&'static str>, Vec<String>)> = None;
        for listed in permutations(members) {
            // One set at a time, under one name: WAF's published quota bounds how
            // many IP sets a scope holds, and what this row compares is the list a
            // create stores, which does not depend on what else exists.
            let name = format!("created-{row}");
            let (id, _arn, lock) =
                create_ip_set(&waf, "REGIONAL", &name, version, &listed, None).await;
            let reported = addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await);
            waf.ok(
                "DeleteIPSet",
                json!({
                    "Name": &name,
                    "Scope": "REGIONAL",
                    "Id": &id,
                    "LockToken": &lock,
                }),
            )
            .await;
            // Every order stores the whole membership and nothing else …
            assert_eq!(
                membership_of(&reported),
                sorted(expected_members),
                "case {case:?} created as {listed:?}: expected the blocks the request \
                 named, got {reported:?}"
            );
            // … and stores them as one list, the one the first order stored.
            match &stored {
                None => stored = Some((listed, reported)),
                Some((first_order, expected)) => assert_eq!(
                    &reported, expected,
                    "case {case:?}: the order a read reports is the membership's own, so \
                     creating it as {listed:?} must store the list creating it as \
                     {first_order:?} stored ({expected:?}), got {reported:?}"
                ),
            }
        }
    }
}

/// `UpdateIPSet` stores the membership the same way a create does, in every order,
/// and the stored list is a fixed point of the update.
#[tokio::test]
async fn every_order_of_one_membership_is_updated_into_one_list() {
    let waf = Waf::new();
    for (row, membership) in MEMBERSHIPS.iter().enumerate() {
        let Membership {
            case,
            version,
            members,
            stored: expected_members,
        } = *membership;
        // One set, created with a placeholder membership that every iteration below
        // replaces wholesale; its family is what the rows vary, so the placeholder is
        // of the same family.
        let name = format!("updated-{row}");
        let seed: &[&str] = if version == "IPV6" {
            &["2001:db8:ffff::/48"]
        } else {
            &["192.0.2.128/25"]
        };
        let (id, _arn, lock) = create_ip_set(&waf, "REGIONAL", &name, version, seed, None).await;
        let mut lock = lock;

        let mut stored: Option<Vec<String>> = None;
        for listed in permutations(members) {
            let updated = waf
                .ok(
                    "UpdateIPSet",
                    json!({
                        "Name": &name,
                        "Scope": "REGIONAL",
                        "Id": &id,
                        "LockToken": &lock,
                        "Addresses": &listed,
                    }),
                )
                .await;
            lock = updated["NextLockToken"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: NextLockToken, got {updated}"))
                .to_string();
            let reported = addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await);
            match &stored {
                None => {
                    // The update replaced the seed wholesale: the blocks it submitted
                    // and no others.
                    assert_eq!(
                        membership_of(&reported),
                        sorted(expected_members),
                        "case {case:?}: an update must store exactly the blocks it \
                         submitted, got {reported:?}"
                    );
                    stored = Some(reported);
                }
                Some(expected) => assert_eq!(
                    &reported, expected,
                    "case {case:?}: updating to {listed:?} must store the same list as \
                     every other order of that membership ({expected:?}), got {reported:?}"
                ),
            }
        }

        // The fixed point: resubmitting what the read reported stores exactly that.
        let stored = stored.expect("at least one permutation");
        let updated = waf
            .ok(
                "UpdateIPSet",
                json!({
                    "Name": &name,
                    "Scope": "REGIONAL",
                    "Id": &id,
                    "LockToken": &lock,
                    "Addresses": &stored,
                }),
            )
            .await;
        assert!(
            updated["NextLockToken"].as_str().is_some(),
            "case {case:?}: a successful update must answer a lock token, got {updated}"
        );
        let round_tripped = addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await);
        assert_eq!(
            round_tripped, stored,
            "case {case:?}: resubmitting the stored list must store exactly that, got \
             {round_tripped:?}"
        );

        // And it is the SAME list a create of that membership stores: one rule, one
        // canonical form, whichever write path reached it.
        let created_name = format!("updated-{row}-created");
        let (created_id, _arn, _lock) =
            create_ip_set(&waf, "REGIONAL", &created_name, version, members, None).await;
        let by_create = addresses(&get_ip_set(&waf, "REGIONAL", &created_name, &created_id).await);
        assert_eq!(
            by_create, stored,
            "case {case:?}: a create and an update of one membership must store one \
             list, got {by_create:?} against {stored:?}"
        );
    }
}
