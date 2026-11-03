// SPDX-License-Identifier: CC0-1.0

//! The CONTENTS of the two service-owned reference catalogs the API publishes:
//! the health-checker source ranges `GetCheckerIpRanges` reports and the country
//! naming the geolocation catalog carries.
//!
//! The sibling suites assert the shape of those catalogs — that every range is a
//! well-formed, sorted, de-duplicated CIDR block and that every location is one
//! well-formed kind. This one asserts what is IN them, which is the part a client
//! acts on: a firewall is programmed from the ranges, and a country name is
//! displayed to a user and compared against what Route 53 returns.
//!
//! The rules under test:
//!
//! 1. Every published health-checker range belongs to one of the two parts the
//!    service's published source has: a `/23` inside the consolidated
//!    `15.177.0.0/18` allocation the checkers source from, or a single-address
//!    `/32` left in one of the retired per-region blocks. The retired `/26`
//!    blocks themselves are not published as blocks — a caller that allow-listed
//!    them would allow 64 addresses no checker sources from and miss the ones it
//!    does.
//! 2. `CountryName` is Route 53's own country naming, not an ASCII-folded ISO
//!    long form: it keeps a name's non-ASCII letters, uses a country's short form
//!    and brackets a parenthetical with square brackets. The same spelling
//!    reaches the wire from `ListGeoLocations` and from `GetGeoLocation`, byte for
//!    byte, so a client that validates one against the other agrees with itself.
//!
//! Every test drives the service through the real router, using the same harness
//! the hosted-zone suites use, so what is asserted is the HTTP/restXml behaviour a
//! Route 53 client sees.

use std::collections::BTreeMap;

use crate::checker_ip_ranges::{parse_cidr, reported_blocks};
use crate::geo_locations::{entries, list};
use crate::hosted_zone_records::{call, router};

/// The consolidated allocation Route 53's health checkers source from.
///
/// Source: the Route 53 health-checks entry of the published `ip-ranges.json`
/// that `GetCheckerIpRanges`'s own documentation points at ("we recommend that
/// you download ip-ranges.json"), which publishes this one `/18` beside the
/// retired regional blocks below.
const CONSOLIDATED_ALLOCATION: &str = "15.177.0.0/18";

/// The retired per-region blocks, which the published source still lists for the
/// regions whose checkers have not moved into [`CONSOLIDATED_ALLOCATION`].
///
/// Source: the same `ip-ranges.json` entry. Written out here, not read from the
/// crate under test, and deliberately including rows the recorded case never
/// named (`177.71.207.128/26`, `176.34.159.192/26`): each one has to hold exactly
/// one published checker address.
const RETIRED_REGIONAL_BLOCKS: &[&str] = &[
    "54.183.255.128/26",
    "54.228.16.0/26",
    "54.232.40.64/26",
    "54.241.32.64/26",
    "54.243.31.192/26",
    "54.244.52.192/26",
    "54.245.168.0/26",
    "54.248.220.0/26",
    "54.250.253.192/26",
    "54.251.31.128/26",
    "54.252.79.128/26",
    "54.252.254.192/26",
    "54.255.254.192/26",
    "107.23.255.0/26",
    "176.34.159.192/26",
    "177.71.207.128/26",
];

/// Route 53's own name for a country, as `(country code, name)`.
///
/// Source: the service's geolocation catalog, whose naming is NOT the ISO
/// standard's English spelling. The rows span the three ways it differs from a
/// folded ISO long form, and most of them are codes the recorded case never named
/// (`CI`, `CV`, `CW`, `RE`, `ST`, `CD`, `CG`):
///
/// * a name's non-ASCII letters are kept rather than folded (`AX`, `BL`, `CI`,
///   `CW`, `RE`, `ST`);
/// * a country's short form is used rather than an ISO long form (`BQ`, `CV`);
/// * a parenthetical is bracketed with square brackets (`CC`);
/// * a name that distinguishes two countries sharing one name does it with the
///   capital (`CD`, `CG`).
const PUBLISHED_COUNTRY_NAMES: &[(&str, &str)] = &[
    ("AX", "Åland"),
    ("BL", "Saint Barthélemy"),
    ("BQ", "Bonaire"),
    ("CC", "Cocos [Keeling] Islands"),
    ("CD", "Congo - Kinshasa"),
    ("CG", "Congo - Brazzaville"),
    ("CI", "Côte d'Ivoire"),
    ("CV", "Cape Verde"),
    ("CW", "Curaçao"),
    ("RE", "Réunion"),
    ("ST", "São Tomé and Príncipe"),
];

/// Whether the block `inner` lies entirely inside the block `outer`.
///
/// A block is inside another when it is at least as specific and its network
/// address agrees on every bit the outer prefix fixes.
fn within(outer: &str, inner: &str) -> bool {
    let (Some((outer_network, outer_prefix)), Some((inner_network, inner_prefix))) =
        (parse_cidr(outer), parse_cidr(inner))
    else {
        return false;
    };
    if inner_prefix < outer_prefix {
        return false;
    }
    let mask = u32::MAX
        .checked_shl(32 - u32::from(outer_prefix))
        .unwrap_or(u32::MAX);
    outer_network & mask == inner_network & mask
}

#[tokio::test]
async fn every_published_range_is_a_consolidated_sub_block_or_a_single_checker_address() {
    // One row per range the service publishes — every one of them, not the one a
    // recording happened to carry.
    let router = router();
    let reported = reported_blocks(&router).await;

    let mut consolidated = 0usize;
    let mut single_addresses = 0usize;
    for case in &reported {
        let (_, prefix) = parse_cidr(case)
            .unwrap_or_else(|| panic!("case {case:?}: every published range is an IPv4 CIDR block"));
        if within(CONSOLIDATED_ALLOCATION, case) {
            assert_eq!(
                prefix, 23,
                "case {case:?}: the consolidated allocation is published as its /23 sub-blocks, \
                 got /{prefix}"
            );
            consolidated += 1;
            continue;
        }
        assert_eq!(
            prefix, 32,
            "case {case:?}: a range outside {CONSOLIDATED_ALLOCATION} is one checker's own \
             address, got /{prefix}"
        );
        let holder = RETIRED_REGIONAL_BLOCKS
            .iter()
            .find(|block| within(block, case));
        assert!(
            holder.is_some(),
            "case {case:?}: a single checker address sits in one of the retired regional blocks, \
             got none of {RETIRED_REGIONAL_BLOCKS:?}"
        );
        single_addresses += 1;
    }

    assert!(
        consolidated > 0 && single_addresses > 0,
        "both published parts must reach the wire, got {consolidated} sub-block(s) of \
         {CONSOLIDATED_ALLOCATION} and {single_addresses} single address(es) in {reported:?}"
    );
}

#[tokio::test]
async fn no_retired_regional_block_is_published_as_a_block() {
    // One row per retired block, including the ones the recorded case never
    // named: the catalog publishes one address out of each, never the /26 itself.
    let router = router();
    let reported = reported_blocks(&router).await;

    for case in RETIRED_REGIONAL_BLOCKS {
        assert!(
            !reported.iter().any(|block| block == case),
            "case {case:?}: a retired per-region block is not published as a block, got \
             {reported:?}"
        );
        let held: Vec<&String> = reported
            .iter()
            .filter(|block| within(case, block))
            .collect();
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: exactly one published checker address sits in a retired block, got \
             {held:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_carries_route_53s_own_country_naming() {
    // The rows differ along the three ways that naming departs from a folded ISO
    // long form, and most of them are codes the recorded case never named.
    let router = router();
    let reply = list(&router, "").await;
    let listed: BTreeMap<String, String> = entries(&reply.body)
        .into_iter()
        .filter(|entry| entry.subdivision_code.is_none())
        .filter_map(|entry| entry.country_code.zip(entry.country_name))
        .collect();

    for (code, expected) in PUBLISHED_COUNTRY_NAMES {
        let listed_name = listed.get(*code).map(String::as_str);
        assert_eq!(
            listed_name,
            Some(*expected),
            "case {code:?}: the listing carries Route 53's own name, expected {expected:?}, got \
             {listed_name:?}"
        );

        // The SAME spelling from the resolver, byte for byte: a client that
        // validates a user-chosen location against the listing and then asks
        // about it has to read one name for one country.
        let resolved = call(
            &router,
            "GET",
            &format!("/2013-04-01/geolocation?countrycode={code}"),
            "",
        )
        .await;
        assert_eq!(
            resolved.status, 200,
            "case {code:?}: expected 200 from GetGeoLocation, got {} with body {}",
            resolved.status, resolved.body
        );
        let resolved_name = resolved.first("CountryName");
        assert_eq!(
            resolved_name,
            Some(*expected),
            "case {code:?}: the resolver carries the listing's name, expected {expected:?}, got \
             {resolved_name:?}"
        );
    }
}

#[tokio::test]
async fn no_country_name_is_a_folded_or_parenthesised_form() {
    // The rule applied to the WHOLE table rather than to the rows above: a name
    // is published as Route 53 writes it, so none of them brackets a
    // parenthetical with round brackets, and the names that carry non-ASCII
    // letters carry them on the wire.
    let router = router();
    let reply = list(&router, "").await;
    let listed: BTreeMap<String, String> = entries(&reply.body)
        .into_iter()
        .filter(|entry| entry.subdivision_code.is_none())
        .filter_map(|entry| entry.country_code.zip(entry.country_name))
        .collect();
    assert!(
        listed.len() > PUBLISHED_COUNTRY_NAMES.len(),
        "the catalog must hold the supported countries, got {} of them",
        listed.len()
    );

    for (code, name) in &listed {
        assert!(
            !name.contains('(') && !name.contains(')'),
            "case {code:?}: a parenthetical is bracketed with square brackets, got {name:?}"
        );
        assert_eq!(
            name.trim(),
            name.as_str(),
            "case {code:?}: a name carries no surrounding space, got {name:?}"
        );
    }

    let unfolded: Vec<&String> = listed
        .iter()
        .filter(|(_, name)| !name.is_ascii())
        .map(|(code, _)| code)
        .collect();
    for (code, expected) in PUBLISHED_COUNTRY_NAMES
        .iter()
        .filter(|(_, name)| !name.is_ascii())
    {
        assert!(
            unfolded.iter().any(|listed| *listed == code),
            "case {code:?}: {expected:?} keeps its non-ASCII letters on the wire, got the \
             non-ASCII names of {unfolded:?}"
        );
    }
}
