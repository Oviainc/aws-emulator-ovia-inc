// SPDX-License-Identifier: CC0-1.0

//! The ORDER an IP set's `Addresses` are reported in, end to end over the wire.
//!
//! `IPSet$Addresses` "contains an array of strings that specifies zero or more IP
//! addresses or blocks of IP addresses that you want WAF to inspect for in incoming
//! requests": what the set holds is the MEMBERSHIP those strings name, and the order
//! a read reports it in is the set's own — a function of the members, not of the
//! order a request happened to list them in. The RULE asserted here is therefore:
//!
//! * one membership, one order: the same blocks submitted in any order, through a
//!   create or through an update, read back in one order, and reading twice reports
//!   that order again;
//! * the stored list is a fixed point: resubmitting exactly what a read reported
//!   stores exactly that;
//! * a block named twice is one member, and no member is lost.
//!
//! The first row of the table is a membership the real service was recorded
//! answering in the order spelled out below; the other rows are the same rule over
//! memberships that differ in size, in family, and in how far apart the blocks are.
//! Clients diff `GetIPSet` against their intended configuration, so what matters is
//! that one membership always reads the same way — otherwise an idempotent apply
//! rewrites the set, and the `LockToken` round trip makes that an avoidable write.

use serde_json::json;

use crate::ip_set_mutations::{addresses, create_ip_set, get_ip_set, membership_of};
use crate::web_acl_lifecycle::Waf;

/// One membership, and the order a read must report it in.
struct Ordering {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The address family the set is created with.
    version: &'static str,
    /// The blocks the set holds, as the request lists them.
    submitted: &'static [&'static str],
    /// The order a read reports them in, where this suite pins it.
    stored: &'static [&'static str],
}

/// The rows whose order is pinned: the recorded membership, the same membership with
/// one more block, and a single block, which has only one order.
const ORDERINGS: &[Ordering] = &[
    Ordering {
        case: "two adjacent /24 blocks",
        version: "IPV4",
        submitted: &["10.0.0.0/24", "10.0.1.0/24"],
        stored: &["10.0.1.0/24", "10.0.0.0/24"],
    },
    Ordering {
        case: "three adjacent /24 blocks",
        version: "IPV4",
        submitted: &["10.0.0.0/24", "10.0.1.0/24", "10.0.2.0/24"],
        stored: &["10.0.1.0/24", "10.0.2.0/24", "10.0.0.0/24"],
    },
    Ordering {
        case: "one block, which has one order",
        version: "IPV4",
        submitted: &["192.0.2.0/24"],
        stored: &["192.0.2.0/24"],
    },
];

/// Memberships the suite asserts the INVARIANTS over, without pinning the order:
/// every size from one to several, both families, blocks far apart and blocks
/// differing only in their prefix length.
const MEMBERSHIPS: &[(&str, &str, &[&str])] = &[
    ("one IPv4 block", "IPV4", &["10.0.0.0/8"]),
    (
        "three IPv4 blocks in different ranges",
        "IPV4",
        &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"],
    ),
    (
        "five IPv4 blocks",
        "IPV4",
        &[
            "10.1.0.0/16",
            "10.2.0.0/16",
            "10.3.0.0/16",
            "10.4.0.0/16",
            "10.5.0.0/16",
        ],
    ),
    (
        "one address at two prefix lengths",
        "IPV4",
        &["203.0.113.0/24", "203.0.113.0/32"],
    ),
    (
        "three IPv6 blocks",
        "IPV6",
        &[
            "2001:0db8:0001:0000:0000:0000:0000:0000/48",
            "2001:0db8:0002:0000:0000:0000:0000:0000/48",
            "2001:0db8:0003:0000:0000:0000:0000:0000/48",
        ],
    ),
];

/// `entries` with the first element moved to the end: another order for one
/// membership.
fn rotated(entries: &[&str]) -> Vec<String> {
    let mut rotated: Vec<String> = entries.iter().map(|entry| (*entry).to_string()).collect();
    if !rotated.is_empty() {
        let head = rotated.remove(0);
        rotated.push(head);
    }
    rotated
}

/// The order a read reports is the one this suite pins for the membership, whichever
/// order the request listed the blocks in.
#[tokio::test]
async fn a_membership_is_reported_in_the_order_the_set_holds_it() {
    let waf = Waf::new();
    for (index, ordering) in ORDERINGS.iter().enumerate() {
        let Ordering {
            case,
            version,
            submitted,
            stored,
        } = *ordering;
        let expected: Vec<String> = stored.iter().map(|entry| (*entry).to_string()).collect();

        // Every order the request can list the membership in: as written, reversed,
        // and rotated.
        let mut reversed: Vec<String> =
            submitted.iter().map(|entry| (*entry).to_string()).collect();
        reversed.reverse();
        let orders: Vec<(&str, Vec<String>)> = vec![
            (
                "as written",
                submitted.iter().map(|entry| (*entry).to_string()).collect(),
            ),
            ("reversed", reversed),
            ("rotated", rotated(submitted)),
        ];

        for (order, listed) in orders {
            let name = format!("order-{index}-{order}").replace(' ', "-");
            let listed: Vec<&str> = listed.iter().map(String::as_str).collect();
            let (id, _arn, _lock) =
                create_ip_set(&waf, "REGIONAL", &name, version, &listed, None).await;
            let reported = addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await);
            assert_eq!(
                reported, expected,
                "case {case:?} submitted {order}: expected the set's own order \
                 {expected:?}, got {reported:?}"
            );
        }
    }
}

/// One membership, one order — through either write path, read after read — and the
/// stored list is a fixed point of the write.
#[tokio::test]
async fn one_membership_reads_back_one_way() {
    let waf = Waf::new();
    for (index, (case, version, members)) in MEMBERSHIPS.iter().enumerate() {
        // Created with the membership as written.
        let name = format!("set-{index}");
        let (id, _arn, lock) =
            create_ip_set(&waf, "REGIONAL", &name, version, members, None).await;
        let stored = addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await);
        assert_eq!(
            membership_of(&stored),
            membership_of(&members.iter().map(|m| (*m).to_string()).collect::<Vec<String>>()),
            "case {case:?}: the set must hold exactly the blocks it was given, got \
             {stored:?}"
        );

        // Read again: a read is a report, so the order does not move.
        let again = addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await);
        assert_eq!(
            again, stored,
            "case {case:?}: reading twice must report one order, got {again:?} after \
             {stored:?}"
        );

        // Another set, created with the same membership in another order: one
        // membership, one order.
        let other_name = format!("set-{index}-rotated");
        let rotated = rotated(members);
        let rotated: Vec<&str> = rotated.iter().map(String::as_str).collect();
        let (other_id, _arn, _lock) =
            create_ip_set(&waf, "REGIONAL", &other_name, version, &rotated, None).await;
        let other = addresses(&get_ip_set(&waf, "REGIONAL", &other_name, &other_id).await);
        assert_eq!(
            other, stored,
            "case {case:?}: the same membership in another order must read back the \
             same way, got {other:?} against {stored:?}"
        );

        // The fixed point: an update resubmitting what the read reported stores
        // exactly that, and an update is the same write as a create.
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
            "case {case:?}: resubmitting the stored list must store exactly that, \
             got {round_tripped:?}"
        );
    }
}

/// A block named twice is ONE member, and naming it twice does not change the order
/// the membership reads back in.
#[tokio::test]
async fn a_block_named_twice_is_one_member() {
    let waf = Waf::new();
    for (index, (case, version, members)) in MEMBERSHIPS.iter().enumerate() {
        let name = format!("once-{index}");
        let (id, _arn, _lock) =
            create_ip_set(&waf, "REGIONAL", &name, version, members, None).await;
        let once = addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await);

        // The same membership, with every block named a second time.
        let mut doubled: Vec<&str> = members.to_vec();
        doubled.extend_from_slice(members);
        let twice_name = format!("twice-{index}");
        let (twice_id, _arn, _lock) =
            create_ip_set(&waf, "REGIONAL", &twice_name, version, &doubled, None).await;
        let twice = addresses(&get_ip_set(&waf, "REGIONAL", &twice_name, &twice_id).await);
        assert_eq!(
            twice, once,
            "case {case:?}: a block named twice must be one member, in the same \
             order, got {twice:?} against {once:?}"
        );
    }
}
