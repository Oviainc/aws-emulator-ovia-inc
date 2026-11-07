// SPDX-License-Identifier: CC0-1.0

//! The notation an IP set's addresses are stored in, end to end over the wire.
//!
//! `IPSet$Addresses` are CIDR blocks of the family `IPAddressVersion` declares, and
//! an IPv6 address has many notations for one value: `::` elides a run of zero
//! groups, a group's leading zeros may be dropped, and the hex digits may be in
//! either case. The RULE asserted here is therefore: an `IPV6` set stores and
//! reports every entry in the one fully expanded, zero-padded, lowercase notation of
//! its value, with the prefix length it was given — so two notations of one block are
//! ONE entry, resubmitting the canonical list round-trips it unchanged, and a set's
//! membership does not depend on the notation a request happened to use. An `IPV4`
//! address has a single notation, so an `IPV4` set's entries are reported with the
//! characters they were submitted with.
//!
//! Each suite asserts that over a TABLE spanning the class: an elided zero run at the
//! front, in the middle and at the end, a value written out in full, upper-case
//! digits, the all-zero and the loopback addresses, an IPv4-mapped value, every
//! prefix length at the published bounds, and both scopes.
//!
//! The ORDER a read reports a whole list in is the set's own, not the submitted one;
//! `crate::ip_set_address_order` spans that rule, so a multi-entry list is compared
//! here as the set it names — and read-to-read, where the order must not move.

use serde_json::{json, Value};

use crate::ip_set_mutations::{create_ip_set, get_ip_set};
use crate::web_acl_lifecycle::Waf;

/// The `Addresses` `GetIPSet` reports for `(scope, name, id)`.
fn stored_addresses(got: &Value, case: &str) -> Vec<String> {
    got["IPSet"]["Addresses"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: GetIPSet must report Addresses, got {got}"))
        .iter()
        .map(|address| address.as_str().unwrap_or_default().to_string())
        .collect()
}

/// One submitted entry and the notation it is stored in.
struct Canonical {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The entry the request submits.
    submitted: &'static str,
    /// The notation a read must report it in.
    stored: &'static str,
}

/// The table: every way an IPv6 value can be written short, plus the bounds of the
/// prefix length.
///
/// `IPSet$Addresses` documents "WAF supports all IPv4 and IPv6 CIDR ranges except for
/// `/0`", so `/1` and `/128` are the published bounds for this family and both are
/// rows.
const IPV6_ROWS: &[Canonical] = &[
    Canonical {
        case: "a zero run elided before the last group",
        submitted: "2001:db8::1/128",
        stored: "2001:0db8:0000:0000:0000:0000:0000:0001/128",
    },
    Canonical {
        case: "the same value written out with single-digit groups",
        submitted: "2001:db8:0:0:0:0:0:2/128",
        stored: "2001:0db8:0000:0000:0000:0000:0000:0002/128",
    },
    Canonical {
        case: "a zero run elided at the end",
        submitted: "fe80::/64",
        stored: "fe80:0000:0000:0000:0000:0000:0000:0000/64",
    },
    Canonical {
        case: "a documentation prefix",
        submitted: "2001:db8::/32",
        stored: "2001:0db8:0000:0000:0000:0000:0000:0000/32",
    },
    Canonical {
        case: "the loopback address, elided at the front",
        submitted: "::1/128",
        stored: "0000:0000:0000:0000:0000:0000:0000:0001/128",
    },
    Canonical {
        case: "the all-zero address at the smallest supported prefix length",
        submitted: "::/1",
        stored: "0000:0000:0000:0000:0000:0000:0000:0000/1",
    },
    Canonical {
        case: "upper-case hex digits",
        submitted: "2001:DB8:ABCD::FFFF/48",
        stored: "2001:0db8:abcd:0000:0000:0000:0000:ffff/48",
    },
    Canonical {
        case: "a value with a zero run in the middle",
        submitted: "2001:db8:1::ff:fe:1/128",
        stored: "2001:0db8:0001:0000:0000:00ff:00fe:0001/128",
    },
    Canonical {
        case: "an IPv4-mapped value",
        submitted: "::ffff:192.0.2.1/128",
        stored: "0000:0000:0000:0000:0000:ffff:c000:0201/128",
    },
    Canonical {
        case: "a value already in the canonical notation",
        submitted: "1111:0000:0000:0000:0000:0000:0000:0111/128",
        stored: "1111:0000:0000:0000:0000:0000:0000:0111/128",
    },
];

/// Every IPv4 row: a single-notation family, reported exactly as submitted — host
/// bits below the prefix length included, which nothing here rewrites.
const IPV4_ROWS: &[Canonical] = &[
    Canonical {
        case: "a /8 block",
        submitted: "10.0.0.0/8",
        stored: "10.0.0.0/8",
    },
    Canonical {
        case: "one address as a /32",
        submitted: "192.0.2.44/32",
        stored: "192.0.2.44/32",
    },
    Canonical {
        case: "host bits below the prefix length",
        submitted: "10.0.0.1/8",
        stored: "10.0.0.1/8",
    },
    Canonical {
        case: "the smallest supported prefix length",
        submitted: "0.0.0.0/1",
        stored: "0.0.0.0/1",
    },
];

/// A create stores each entry in its family's notation, and `GetIPSet` reports that:
/// the IPv6 rows canonicalized, the IPv4 rows byte for byte. Both scopes, because the
/// notation of an address is a property of the address.
#[tokio::test]
async fn an_ip_set_stores_every_address_in_its_familys_notation() {
    let waf = Waf::new();
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        for (version, rows) in [("IPV6", IPV6_ROWS), ("IPV4", IPV4_ROWS)] {
            for (index, row) in rows.iter().enumerate() {
                let case = format!("{scope} {version}: {}", row.case);
                let name = format!("canon-{version}-{scope}-{index}");
                let (id, _arn, lock) = create_ip_set(
                    &waf,
                    scope,
                    &name,
                    version,
                    &[row.submitted],
                    Some("canonicalization"),
                )
                .await;
                let got = get_ip_set(&waf, scope, &name, &id).await;
                let reported = stored_addresses(&got, &case);
                assert_eq!(
                    reported,
                    vec![row.stored.to_string()],
                    "case {case:?}: expected the stored notation {:?}, got {reported:?}",
                    row.stored
                );

                // Resubmitting what the read reported round-trips: the stored form is
                // a fixed point of the normalization, so an update that changes
                // nothing reports the same entry.
                let updated = waf
                    .ok(
                        "UpdateIPSet",
                        json!({
                            "Name": &name,
                            "Scope": scope,
                            "Id": &id,
                            "LockToken": &lock,
                            "Addresses": [row.stored],
                        }),
                    )
                    .await;
                assert!(
                    updated["NextLockToken"].as_str().is_some(),
                    "case {case:?}: a successful update must answer a lock token, got \
                     {updated}"
                );
                let again = get_ip_set(&waf, scope, &name, &id).await;
                let round_tripped = stored_addresses(&again, &case);
                assert_eq!(
                    round_tripped, reported,
                    "case {case:?}: resubmitting the stored list must round-trip it, got \
                     {round_tripped:?}"
                );
            }
        }
    }
}

/// A whole list keeps every entry, each in its family's notation, and an
/// `UpdateIPSet` replacing it canonicalizes the replacement the same way a create
/// does — then resubmitting what the read reported round-trips it unchanged, order
/// included, because the stored list is a fixed point of the normalization.
#[tokio::test]
async fn a_whole_address_list_is_canonicalized_entry_by_entry() {
    let waf = Waf::new();
    let submitted: Vec<&str> = IPV6_ROWS.iter().map(|row| row.submitted).collect();
    let expected: Vec<String> = IPV6_ROWS
        .iter()
        .map(|row| row.stored.to_string())
        .collect();
    let case = "every IPv6 notation in one set";

    // Created with the first half, then updated to the whole list, so both write
    // paths are asserted against the same expectation.
    let (id, _arn, lock) = create_ip_set(
        &waf,
        "REGIONAL",
        "canon-list",
        "IPV6",
        &submitted[..2],
        None,
    )
    .await;
    let after_create = stored_addresses(&get_ip_set(&waf, "REGIONAL", "canon-list", &id).await, case);
    assert_eq!(
        as_set(&after_create),
        as_set(&expected[..2]),
        "case {case:?}: a create must canonicalize every entry it stores, got \
         {after_create:?}"
    );

    waf.ok(
        "UpdateIPSet",
        json!({
            "Name": "canon-list",
            "Scope": "REGIONAL",
            "Id": &id,
            "LockToken": &lock,
            "Addresses": submitted,
        }),
    )
    .await;
    let after_update = stored_addresses(&get_ip_set(&waf, "REGIONAL", "canon-list", &id).await, case);
    assert_eq!(
        as_set(&after_update),
        as_set(&expected),
        "case {case:?}: an update must canonicalize every entry it stores, got \
         {after_update:?}"
    );

    // The fixed point: an update resubmitting exactly what the read reported stores
    // exactly that, entry for entry and in the same positions.
    let lock = get_ip_set(&waf, "REGIONAL", "canon-list", &id).await["LockToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    waf.ok(
        "UpdateIPSet",
        json!({
            "Name": "canon-list",
            "Scope": "REGIONAL",
            "Id": &id,
            "LockToken": &lock,
            "Addresses": &after_update,
        }),
    )
    .await;
    let round_tripped =
        stored_addresses(&get_ip_set(&waf, "REGIONAL", "canon-list", &id).await, case);
    assert_eq!(
        round_tripped, after_update,
        "case {case:?}: resubmitting the stored list must round-trip it unchanged, \
         got {round_tripped:?}"
    );
}

/// `entries` as the set they name, in one order, so two lists of the same entries
/// compare equal whatever order each reports them in.
fn as_set(entries: &[String]) -> Vec<String> {
    let mut members = entries.to_vec();
    members.sort();
    members
}

/// Canonicalization is what makes the set's membership well defined: two notations of
/// one block are ONE entry, and two blocks that differ — in the address or only in
/// the prefix length — stay two.
#[tokio::test]
async fn two_notations_of_one_block_are_one_entry() {
    let waf = Waf::new();
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "the short and the expanded notation of one block",
            vec![
                "2001:db8::1/128",
                "2001:0db8:0000:0000:0000:0000:0000:0001/128",
            ],
            vec!["2001:0db8:0000:0000:0000:0000:0000:0001/128"],
        ),
        (
            "three notations of one block, with another block between them",
            vec![
                "fe80::/64",
                "2001:db8::/32",
                "FE80:0:0:0:0:0:0:0/64",
                "fe80:0000:0000:0000:0000:0000:0000:0000/64",
            ],
            vec![
                "fe80:0000:0000:0000:0000:0000:0000:0000/64",
                "2001:0db8:0000:0000:0000:0000:0000:0000/32",
            ],
        ),
        (
            "one address at two prefix lengths",
            vec!["2001:db8::/32", "2001:db8::/64"],
            vec![
                "2001:0db8:0000:0000:0000:0000:0000:0000/32",
                "2001:0db8:0000:0000:0000:0000:0000:0000/64",
            ],
        ),
        (
            "two different blocks, both kept",
            vec!["2001:db8::1/128", "2001:db8::2/128"],
            vec![
                "2001:0db8:0000:0000:0000:0000:0000:0001/128",
                "2001:0db8:0000:0000:0000:0000:0000:0002/128",
            ],
        ),
    ];

    for (index, (case, submitted, expected)) in cases.into_iter().enumerate() {
        let name = format!("membership-{index}");
        let (id, _arn, _lock) =
            create_ip_set(&waf, "REGIONAL", &name, "IPV6", &submitted, None).await;
        let reported = stored_addresses(&get_ip_set(&waf, "REGIONAL", &name, &id).await, case);
        assert_eq!(
            as_set(&reported),
            as_set(
                &expected
                    .iter()
                    .map(|address| (*address).to_string())
                    .collect::<Vec<String>>()
            ),
            "case {case:?}: expected one entry per block, got {reported:?}"
        );
    }
}
