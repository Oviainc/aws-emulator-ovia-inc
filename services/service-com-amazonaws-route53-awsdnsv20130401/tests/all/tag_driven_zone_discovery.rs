// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the path a tag-driven client walks to find its OWN
//! hosted zones: page `ListHostedZones`, keep the zones whose
//! `Config/PrivateZone` says they are public, and read each one's tags back
//! through `ListTagsForResource`.
//!
//! The rule under test: a hosted zone created in the account is enumerated by
//! `ListHostedZones` exactly once across the `Marker`/`NextMarker` pagination,
//! reporting the private flag it was created with, and reports through
//! `ListTagsForResource` exactly the tags the account put on it. That is what
//! makes `aws route53 list-hosted-zones` + `list-tags-for-resource`, an
//! infrastructure-as-code data source and an inventory scan able to select a
//! zone by a tag instead of by an id nobody knows in advance.
//!
//! The two reads are asserted TOGETHER rather than one per suite, because the
//! defect this pins is in their composition: a listing that enumerates nothing,
//! a listing that loses a zone between pages, a private zone leaking into the
//! public set and a tag set that comes back empty are one failure to a client —
//! it finds no zone to work with — and each of them passes a test that asserts
//! only one of the two reads.
//!
//! So the rows differ along what the rule generalises over: the number of
//! zones, whether a zone is public or private, how many tags it carries (none,
//! one, several), the keys and values of those tags, and the page size the walk
//! pages with — never merely along which zone is named. The tag that marks a
//! zone for selection is this suite's own choice: WHICH tag a client selects
//! by is the account's, never part of the API contract.
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` uses, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_body, router};
use crate::resource_tags::{change_tags, list_tags, tag_pairs_in};

/// The tag this suite's client selects its zones by. Both halves are the
/// TEST's: a marker tag is a value the account writes, so no part of it comes
/// from the API.
const MARKER: (&str, &str) = ("zone-role", "public-apex");

/// One zone the account holds: how it is created, and the tags put on it.
struct ZoneRow {
    case: &'static str,
    name: &'static str,
    /// Whether the zone is created as a PRIVATE hosted zone.
    private: bool,
    /// The tags written on it with `ChangeTagsForResource`, in any order.
    tags: &'static [(&'static str, &'static str)],
}

/// The account this suite's walks run against: zones differing in kind, in tag
/// count and in the keys and values they carry.
fn account() -> Vec<ZoneRow> {
    vec![
        ZoneRow {
            case: "a public zone carrying the marker tag alone",
            name: "apex.example.com",
            private: false,
            tags: &[MARKER],
        },
        ZoneRow {
            case: "a public zone carrying the marker tag among others",
            name: "mail.example.net",
            private: false,
            tags: &[MARKER, ("Env", "prod"), ("Owner", "dns-team")],
        },
        ZoneRow {
            case: "a public zone carrying no tag at all",
            name: "untagged.example.org",
            private: false,
            tags: &[],
        },
        ZoneRow {
            case: "a public zone carrying another account's tags only",
            name: "deep.sub.zone.example.co.uk",
            private: false,
            tags: &[("Env", "staging"), ("zone-role", "internal-apex")],
        },
        ZoneRow {
            case: "a private zone carrying the marker tag",
            name: "internal.example.com",
            private: true,
            tags: &[MARKER, ("Env", "dev")],
        },
        ZoneRow {
            case: "a private zone carrying no tag",
            name: "internal.example.net",
            private: true,
            tags: &[],
        },
    ]
}

/// The `HostedZoneConfig` (and, for a private zone, the first `VPC`
/// association) one row is created with.
fn create_config(row: &ZoneRow) -> &'static str {
    if row.private {
        "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
         <VPC><VPCRegion>us-west-2</VPCRegion><VPCId>vpc-0a1b2c3d</VPCId></VPC>"
    } else {
        ""
    }
}

/// Create one row's zone, tag it, and return its BARE (`Z…`) id — the id
/// `ListTagsForResource` takes, which is the listing's `Id` after the last `/`.
async fn create_and_tag(router: &MultiServiceRouter, row: &ZoneRow, reference: &str) -> String {
    let created = call(
        router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body(row.name, reference, create_config(row)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
        row.case, created.status, created.body
    );
    let zone = created
        .first("HostedZone/Id")
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the create must answer with HostedZone/Id; body {}",
                row.case, created.body
            )
        })
        .trim_start_matches("/hostedzone/")
        .to_string();

    if !row.tags.is_empty() {
        let tagged = change_tags(router, "hostedzone", &zone, row.tags, &[]).await;
        assert_eq!(
            tagged.status, 200,
            "case {:?}: expected 200 from ChangeTagsForResource, got {} with body {}",
            row.case, tagged.status, tagged.body
        );
    }
    zone
}

/// Every zone `ListHostedZones` enumerates, as `(bare id, PrivateZone flag)`
/// pairs in the order the walk met them, paged the way `page_size` asks for
/// (`None` sends no `maxitems` at all).
///
/// The walk asserts the pagination contract it depends on as it goes: a page is
/// never longer than the `maxitems` asked for, `NextMarker` is present exactly
/// when `IsTruncated` is true, and the response echoes the `Marker` the request
/// resumed from.
async fn enumerate(
    router: &MultiServiceRouter,
    page_size: Option<i32>,
    case: &str,
    zone_count: usize,
) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut marker: Option<String> = None;
    // One page per zone is the most a walk can need, plus the final empty-ish
    // page; a walk that wants more is a marker that is not advancing.
    let page_limit = zone_count + 2;
    for page_number in 1..=page_limit {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxitems={size}"));
        }
        if let Some(at) = &marker {
            query.push(format!("marker={at}"));
        }
        let uri = if query.is_empty() {
            "/2013-04-01/hostedzone".to_string()
        } else {
            format!("/2013-04-01/hostedzone?{}", query.join("&"))
        };
        let page = call(router, "GET", &uri, "").await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200 from ListHostedZones, got {} with body {}",
            page.status, page.body
        );
        // `<Marker></Marker>` carries no text, so the first page's echo of the
        // empty marker reads as the empty string.
        assert_eq!(
            page.first("Marker").unwrap_or(""),
            marker.as_deref().unwrap_or(""),
            "case {case:?}: the response must echo the Marker it resumed from; body {}",
            page.body
        );

        let ids: Vec<String> = page
            .all("HostedZone/Id")
            .into_iter()
            .map(|id| id.trim_start_matches("/hostedzone/").to_string())
            .collect();
        let flags: Vec<String> = page
            .all("HostedZone/Config/PrivateZone")
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            ids.len(),
            flags.len(),
            "case {case:?}: every enumerated zone must report Config/PrivateZone, got \
             {} zone(s) and {} flag(s); body {}",
            ids.len(),
            flags.len(),
            page.body
        );
        if let Some(size) = page_size {
            assert!(
                ids.len() <= size as usize,
                "case {case:?}: a page must hold at most the maxitems asked for ({size}), \
                 got {}; body {}",
                ids.len(),
                page.body
            );
        }
        found.extend(ids.into_iter().zip(flags));

        let truncated = page.first("IsTruncated") == Some("true");
        let next = page.first("NextMarker").map(str::to_string);
        assert_eq!(
            truncated,
            next.is_some(),
            "case {case:?}: NextMarker is present exactly when IsTruncated is true; body {}",
            page.body
        );
        match next {
            Some(at) => marker = Some(at),
            None => return found,
        }
        assert!(
            page_number < page_limit,
            "case {case:?}: the walk did not finish within {page_limit} page(s), so a \
             marker is not advancing"
        );
    }
    found
}

/// The tags `ListTagsForResource` reports for one zone, in the order it reports
/// them.
async fn tags_reported(router: &MultiServiceRouter, zone: &str, case: &str) -> Vec<(String, String)> {
    let listed = list_tags(router, "hostedzone", zone).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.first("ResourceTagSet/ResourceId"),
        Some(zone),
        "case {case:?}: the tag set must name the zone it was asked about; body {}",
        listed.body
    );
    tag_pairs_in(&listed.body)
}

/// A row's tags as the reads report them: keyed by tag key, which is the order
/// `ResourceTagSet/Tags` carries and the shape a client selects on.
fn expected_tags(row: &ZoneRow) -> Vec<(String, String)> {
    let sorted: BTreeMap<String, String> = row
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    sorted.into_iter().collect()
}

#[tokio::test]
async fn a_tagged_zone_is_enumerated_and_reports_its_tags_through_every_page_size() {
    let rows = account();
    let router = router();

    // The account, and the id each row was created under.
    let mut zones: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let zone = create_and_tag(&router, row, &format!("discovery-ref-{index}")).await;
        zones.push((index, zone));
    }

    let created_ids: BTreeMap<String, usize> = zones
        .iter()
        .map(|(index, zone)| (zone.clone(), *index))
        .collect();
    assert_eq!(
        created_ids.len(),
        rows.len(),
        "every create must mint a distinct zone id, got {created_ids:?}"
    );

    // Every page size the rule has to hold across: one zone per page, two, one
    // more than the account holds, and no `maxitems` at all (the service's own
    // page size).
    let page_sizes: [Option<i32>; 5] = [Some(1), Some(2), Some(3), Some(rows.len() as i32 + 1), None];
    for page_size in page_sizes {
        let case = format!("maxitems={page_size:?}");
        let enumerated = enumerate(&router, page_size, &case, rows.len()).await;

        // Every zone the account holds, exactly once, with the flag it was
        // created with.
        let walked: BTreeMap<String, String> = enumerated.iter().cloned().collect();
        assert_eq!(
            walked.len(),
            enumerated.len(),
            "case {case:?}: paging must enumerate each zone exactly once, got {enumerated:?}"
        );
        let expected_flags: BTreeMap<String, String> = created_ids
            .iter()
            .map(|(zone, index)| {
                (
                    zone.clone(),
                    if rows[*index].private { "true" } else { "false" }.to_string(),
                )
            })
            .collect();
        assert_eq!(
            walked, expected_flags,
            "case {case:?}: ListHostedZones must enumerate every zone of the account with \
             the private flag it was created with"
        );

        // The second half of the walk: the tags each enumerated PUBLIC zone
        // reports, which is what a tag-driven client selects on.
        let mut selected: Vec<&str> = Vec::new();
        for (zone, flag) in &enumerated {
            let index = created_ids[zone];
            let row = &rows[index];
            let reported = tags_reported(&router, zone, row.case).await;
            assert_eq!(
                reported,
                expected_tags(row),
                "case {:?} (walked with {case:?}): ListTagsForResource must report the tags \
                 written on the zone, got {reported:?}",
                row.case
            );
            if flag == "false"
                && reported.contains(&(MARKER.0.to_string(), MARKER.1.to_string()))
            {
                selected.push(row.name);
            }
        }

        // The selection the whole walk exists for: the public zones carrying the
        // marker tag, and no private one however it is tagged.
        let expected_selection: Vec<&str> = rows
            .iter()
            .filter(|row| !row.private && row.tags.contains(&MARKER))
            .map(|row| row.name)
            .collect();
        selected.sort_unstable();
        let mut expected_selection = expected_selection;
        expected_selection.sort_unstable();
        assert_eq!(
            selected, expected_selection,
            "case {case:?}: a client selecting public zones by the marker tag must find \
             exactly the public zones it was written on"
        );
    }
}

#[tokio::test]
async fn the_discovery_walk_reports_the_same_zones_and_tags_on_every_pass() {
    // The rule's other half: both reads are REPORTING calls, so walking the
    // account twice answers identically — a listing or a tag read may not
    // consume, move or hide what the next caller sees.
    let rows = account();
    let router = router();
    for (index, row) in rows.iter().enumerate() {
        create_and_tag(&router, row, &format!("repeat-ref-{index}")).await;
    }

    let first = enumerate(&router, Some(2), "first pass", rows.len()).await;
    let second = enumerate(&router, Some(2), "second pass", rows.len()).await;
    assert_eq!(
        first, second,
        "two passes over ListHostedZones must enumerate the same zones in the same order"
    );

    for (zone, _) in &first {
        let once = tags_reported(&router, zone, "first tag read").await;
        let twice = tags_reported(&router, zone, "second tag read").await;
        assert_eq!(
            once, twice,
            "zone {zone}: two tag reads must report the same tag set"
        );
    }
}

#[tokio::test]
async fn an_account_holding_no_zone_enumerates_an_empty_untruncated_listing() {
    // The low bound of the count the rule generalises over: a client walking an
    // account that holds no zone reads an empty, untruncated page rather than a
    // refusal — and finds no zone to select, which is the honest answer.
    let router = router();
    let page = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    assert_eq!(
        page.status, 200,
        "expected 200 from ListHostedZones, got {} with body {}",
        page.status, page.body
    );
    assert_eq!(
        page.all("HostedZone/Id"),
        Vec::<&str>::new(),
        "an account holding no zone must enumerate none; body {}",
        page.body
    );
    assert_eq!(
        page.first("IsTruncated"),
        Some("false"),
        "an empty listing is not truncated; body {}",
        page.body
    );
    assert_eq!(
        page.first("NextMarker"),
        None,
        "an untruncated listing carries no NextMarker; body {}",
        page.body
    );
}
