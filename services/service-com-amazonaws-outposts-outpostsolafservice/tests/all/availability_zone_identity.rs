// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for which Availability Zone identity `CreateOutpost` accepts,
//! driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an
//! Availability Zone identifier is usable only where it names a zone the region the
//! request is addressed to HAS — a zone name is that region's code plus a zone
//! letter, a zone id is that region's abbreviation plus `-az<n>` with the region's
//! zones numbered from `az1` — and the two identities of a zone are one zone's, so a
//! request naming both must name the SAME zone with them. Anything else describes a
//! zone that cannot exist, which this API answers as its `ValidationException` (400)
//! rather than storing.
//!
//! The cross-REGION half of the rule (a zone of another region) lives in
//! [`crate::error_classification`]; this suite covers the two halves that are about
//! the zone itself: an identifier that names no zone of the request's own region, and
//! a pair naming two zones.

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{text, Outposts};

/// The region codes the rules below are exercised over, each with the
/// Availability-Zone-ID abbreviation AWS forms it as: the two-letter area code, the
/// initials of each word of the rest of the code, then the region number.
///
/// Source: the Availability Zone IDs the AWS documentation publishes — `use1-az1`
/// for `us-east-1`, `usw2-az2` for `us-west-2`, `apse2-az1` for `ap-southeast-2` and
/// `usgw1-az1` for `us-gov-west-1` — of which the rows span a single direction word,
/// a compound one, a non-direction word and a partition segment.
const REGIONS: &[(&str, &str)] = &[
    ("us-east-1", "use1"),
    ("us-west-2", "usw2"),
    ("ap-southeast-2", "apse2"),
    ("eu-central-1", "euc1"),
    ("us-gov-west-1", "usgw1"),
];

/// The lowest `az<n>` index a zone of a region carries: AWS numbers a region's zones
/// from `az1`.
const FIRST_ZONE_INDEX: u32 = 1;

/// The highest one: the modelled `AvailabilityZoneId` pattern
/// `^[a-zA-Z]+\d-[a-zA-Z]+\d$` ends in exactly ONE digit, so `az9` is the last index
/// this API can name.
const LAST_ZONE_INDEX: u32 = 9;

/// The zone letter the service's name <-> id mapping pairs an `az<n>` index with
/// (`…a` <-> `…-az1`).
fn zone_letter(index: u32) -> char {
    char::from(b'a' + u8::try_from(index - 1).expect("a zone index fits in a byte offset"))
}

/// The `Name` of every Outpost the list reports — how a case proves a refused create
/// stored nothing.
fn listed_names(output: &Value) -> Vec<String> {
    output
        .get("Outposts")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text(item, "Name")).collect())
        .unwrap_or_default()
}

/// Dispatch one `CreateOutpost` in `region` and require it to be refused as a bad
/// parameter whose message names every value in `named` back, with nothing stored for
/// the Outpost `Name` the request carried.
async fn refused(outposts: &Outposts, case: &str, region: &str, request: &Value, named: &[&str]) {
    let reply = outposts
        .call_in(region, "POST", "/outposts", Some(request))
        .await;
    assert_eq!(
        (
            reply.status,
            reply.error_code.as_str(),
            reply.errortype_header.as_str()
        ),
        (400, "ValidationException", "ValidationException"),
        "case {case:?} in {region}: a zone identity naming no one zone is a bad \
         parameter, got {} {:?}",
        reply.status,
        reply.body
    );
    let message = text(&reply.body, "Message");
    for value in named {
        assert!(
            message.contains(value),
            "case {case:?} in {region}: the refusal must name the value it refused \
             ({value}), got {message:?}"
        );
    }
    let name = text(request, "Name");
    let listed = listed_names(&outposts.list_outposts("").await);
    assert!(
        !listed.contains(&name),
        "case {case:?} in {region}: a refused create must store nothing, yet \
         {name:?} is listed among {listed:?}"
    );
}

/// `CreateOutpost` in `region` carrying `member: value`, read back through
/// `GetOutpost`, as the `(AvailabilityZone, AvailabilityZoneId)` the store holds.
async fn accepted_pair(
    outposts: &Outposts,
    region: &str,
    site_id: &str,
    name: &str,
    identity: &[(&str, &str)],
) -> (String, String) {
    let mut request = json!({ "Name": name, "SiteId": site_id });
    for (member, value) in identity {
        request[*member] = json!(value);
    }
    let outpost = outposts.create_outpost_in(region, &request).await;
    let read = outposts.get_outpost(&text(&outpost, "OutpostId")).await;
    (
        text(&read, "AvailabilityZone"),
        text(&read, "AvailabilityZoneId"),
    )
}

/// The rule's first half: an identifier whose ZONE part names no zone of the region
/// is refused, and nothing is stored for it.
///
/// Every row is addressed to the region its value names, so none is refused as a
/// cross-region mismatch: each is refused because that region has no such zone. Both
/// published bounds of the index range are exercised as ACCEPTED rows, and one past
/// each bound — `az0`, and the letter one past the last index — as refused ones.
#[tokio::test]
async fn create_outpost_refuses_an_identifier_that_names_no_zone_of_the_region() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "zone identity site" }))
        .await;
    let site_id = text(&site, "SiteId");

    // (case, region, member, value)
    let mut refusals: Vec<(String, &str, &str, String)> = Vec::new();
    for &(region, abbreviation) in REGIONS {
        refusals.push((
            format!("{abbreviation}: a zone id one index below the first"),
            region,
            "AvailabilityZoneId",
            format!("{abbreviation}-az{}", FIRST_ZONE_INDEX - 1),
        ));
        refusals.push((
            format!("{region}: a zone letter one past the last index"),
            region,
            "AvailabilityZone",
            format!("{region}{}", zone_letter(LAST_ZONE_INDEX + 1)),
        ));
        refusals.push((
            format!("{region}: a zone letter far past the last index"),
            region,
            "AvailabilityZone",
            format!("{region}z"),
        ));
        refusals.push((
            format!("{region}: a region code with no zone letter"),
            region,
            "AvailabilityZone",
            region.to_string(),
        ));
        refusals.push((
            format!("{abbreviation}: an id whose zone part is not az<n>"),
            region,
            "AvailabilityZoneId",
            format!("{abbreviation}-ab1"),
        ));
    }
    for (case, region, member, value) in &refusals {
        let mut request = json!({ "Name": format!("refused {case}"), "SiteId": site_id });
        request[*member] = json!(value);
        refused(&outposts, case, region, &request, &[value.as_str()]).await;
    }

    // Each published bound of the index range, named BOTH ways round, IS a zone of
    // its region: what the rows above refuse is the values outside the range, not the
    // range itself.
    for &(region, abbreviation) in REGIONS {
        for index in [FIRST_ZONE_INDEX, LAST_ZONE_INDEX] {
            let zone = format!("{region}{}", zone_letter(index));
            let zone_id = format!("{abbreviation}-az{index}");
            for (named_by, member, value) in [
                ("by name", "AvailabilityZone", zone.as_str()),
                ("by id", "AvailabilityZoneId", zone_id.as_str()),
            ] {
                let case = format!("{zone_id} {named_by}");
                let stored = accepted_pair(
                    &outposts,
                    region,
                    &site_id,
                    &format!("accepted {case}"),
                    &[(member, value)],
                )
                .await;
                assert_eq!(
                    stored,
                    (zone.clone(), zone_id.clone()),
                    "case {case:?}: a zone at a published index bound is accepted and \
                     reports both identities, got {stored:?}"
                );
            }
        }
    }
}

/// The rule's second half: a request naming BOTH identities names one zone with them,
/// so a pair the name <-> id mapping does not pair is refused, and a pair it does is
/// stored and read back exactly as supplied.
///
/// The rows span every region form in [`REGIONS`] crossed with index pairs that
/// differ by one, by the whole width of the range and in both directions — plus a
/// pair that agrees in mixed case, since the modelled patterns admit either case.
#[tokio::test]
async fn create_outpost_refuses_two_identities_that_name_different_zones() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "zone pair site" }))
        .await;
    let site_id = text(&site, "SiteId");

    // (name index, id index): two identities name two zones whenever the indices
    // differ, however far apart they are and whichever way round.
    let mismatched = [
        (FIRST_ZONE_INDEX, FIRST_ZONE_INDEX + 1),
        (FIRST_ZONE_INDEX + 1, FIRST_ZONE_INDEX),
        (FIRST_ZONE_INDEX, LAST_ZONE_INDEX),
        (LAST_ZONE_INDEX, FIRST_ZONE_INDEX),
        (3, 5),
    ];
    for &(region, abbreviation) in REGIONS {
        for (named, identified) in mismatched {
            let zone = format!("{region}{}", zone_letter(named));
            let zone_id = format!("{abbreviation}-az{identified}");
            let case = format!("{zone} with {zone_id}");
            let request = json!({
                "Name": format!("refused {case}"),
                "SiteId": site_id,
                "AvailabilityZone": zone,
                "AvailabilityZoneId": zone_id,
            });
            refused(
                &outposts,
                &case,
                region,
                &request,
                &[zone.as_str(), zone_id.as_str()],
            )
            .await;
        }

        // The pairs the mapping DOES pair are accepted and come back as supplied.
        for index in [FIRST_ZONE_INDEX, FIRST_ZONE_INDEX + 1, LAST_ZONE_INDEX] {
            let zone = format!("{region}{}", zone_letter(index));
            let zone_id = format!("{abbreviation}-az{index}");
            let agreeing = [
                (format!("{zone} with {zone_id}"), zone.clone()),
                (
                    format!("{zone} with {zone_id}, the name in upper case"),
                    zone.to_ascii_uppercase(),
                ),
            ];
            for (case, named_zone) in agreeing {
                let stored = accepted_pair(
                    &outposts,
                    region,
                    &site_id,
                    &format!("accepted {case}"),
                    &[
                        ("AvailabilityZone", named_zone.as_str()),
                        ("AvailabilityZoneId", zone_id.as_str()),
                    ],
                )
                .await;
                assert_eq!(
                    stored,
                    (named_zone.clone(), zone_id.clone()),
                    "case {case:?}: a pair naming one zone is stored as supplied, \
                     got {stored:?}"
                );
            }
        }
    }
}

/// The one pair the name <-> id mapping cannot judge: both identities of a region
/// whose code carries no zone-ID abbreviation, where neither identity resolves to the
/// other.
///
/// That pair is taken on trust — the same tolerance the region part of an id in such
/// a region gets — and stored as supplied, rather than being refused as two zones. A
/// pair of that region's name with an id that DOES resolve, to a different zone, is
/// still two zones and still refused.
#[tokio::test]
async fn a_pair_of_a_region_with_no_zone_id_abbreviation_is_judged_by_what_resolves() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "unabbreviable region site" }))
        .await;
    let site_id = text(&site, "SiteId");
    // A region code whose final segment is not the single digit a zone-ID
    // abbreviation ends in, so the mapping has nothing to map it through.
    let region = "zz-testing-12";
    let zone = "zz-testing-12a";

    let stored = accepted_pair(
        &outposts,
        region,
        &site_id,
        "accepted unmappable pair",
        &[
            ("AvailabilityZone", zone),
            ("AvailabilityZoneId", "zzt1-az1"),
        ],
    )
    .await;
    assert_eq!(
        stored,
        (zone.to_string(), "zzt1-az1".to_string()),
        "a pair neither direction can map is stored as supplied, got {stored:?}"
    );

    let resolving_id = "use1-az1";
    let request = json!({
        "Name": "refused unmappable name with a resolving id",
        "SiteId": site_id,
        "AvailabilityZone": zone,
        "AvailabilityZoneId": resolving_id,
    });
    refused(
        &outposts,
        "an unmappable name with an id that resolves elsewhere",
        region,
        &request,
        &[zone, resolving_id],
    )
    .await;
}
