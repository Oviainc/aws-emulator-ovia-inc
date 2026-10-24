// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a `Site` and an `Outpost` projection REPORTS, driven
//! over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! A. A taggable resource's tag map is part of the resource, so every projection of
//!    it — the create, the single read and the list entry — reports the `Tags`
//!    member, EMPTY for a resource that carries no tags. `TagMap`'s `length(min: 1)`
//!    constrains what a request may carry, not what a response reports.
//! B. An Outpost's Availability Zone has two modelled identities (the zone name and
//!    the zone id); whichever one the create names, the service resolves the other
//!    and every projection of the Outpost reports BOTH — and nothing is invented for
//!    a value no identity can be resolved from.
//! C. A created Outpost is still being provisioned, so it reports the non-terminal
//!    `LifeCycleStatus`; the transition to the terminal one is owed at an instant,
//!    and the first read at or after it reports the settled status, which later
//!    reads do not move again.

use serde_json::{json, Map, Value};

use crate::site_and_outpost_lifecycle::{
    ids, query_value, settle, text, without, Outposts,
};

/// The region codes rule B is exercised over, each with the Availability-Zone-ID
/// abbreviation AWS forms it as: the two-letter area code, the initials of each word
/// of the direction component, then the region number.
///
/// Source: the Availability Zone IDs the AWS documentation publishes — `use1-az1`
/// for `us-east-1`, `usw2-az2` for `us-west-2`, `apse2-az1` for `ap-southeast-2` and
/// `usgw1-az1` for `us-gov-west-1` — of which the rows below span the compound
/// directions (`southeast`, `northeast`), the single-word ones (`east`, `west`,
/// `north`, `south`, `central`) and a partition segment (`gov`).
const REGIONS: &[(&str, &str)] = &[
    ("us-east-1", "use1"),
    ("us-west-2", "usw2"),
    ("eu-west-1", "euw1"),
    ("eu-central-1", "euc1"),
    ("eu-north-1", "eun1"),
    ("ap-southeast-2", "apse2"),
    ("ap-northeast-1", "apne1"),
    ("ap-south-1", "aps1"),
    ("ca-central-1", "cac1"),
    ("sa-east-1", "sae1"),
    ("af-south-1", "afs1"),
    ("me-south-1", "mes1"),
    ("us-gov-west-1", "usgw1"),
];

/// The highest `az<n>` index the modelled `AvailabilityZoneId` pattern
/// `^[a-zA-Z]+\d-[a-zA-Z]+\d$` can name: it ends in exactly ONE digit.
const MAX_ZONE_INDEX: u32 = 9;

/// Whether a value is the modelled `AvailabilityZoneId` form
/// (`^[a-zA-Z]+\d-[a-zA-Z]+\d$`, len 1..255).
fn is_zone_id(value: &str) -> bool {
    let Some((prefix, suffix)) = value.split_once('-') else {
        return false;
    };
    let letters_then_digit = |part: &str| match part.len() {
        0 | 1 => false,
        length => {
            part.as_bytes()[length - 1].is_ascii_digit()
                && part.as_bytes()[..length - 1]
                    .iter()
                    .all(u8::is_ascii_alphabetic)
        }
    };
    !value.is_empty()
        && value.len() <= 255
        && letters_then_digit(prefix)
        && letters_then_digit(suffix)
}

/// Whether a value is the modelled `LifeCycleStatus` form (len 1..20, pattern
/// `^[ A-Za-z]+$`).
fn is_life_cycle_status(value: &str) -> bool {
    (1..=20).contains(&value.len())
        && value
            .bytes()
            .all(|b| b == b' ' || b.is_ascii_alphabetic())
}

/// The one list entry whose identifier is `id`, or `Value::Null` when the list does
/// not hold it.
///
/// An `Outpost` carries BOTH `OutpostId` and the `SiteId` of the site it is at, so
/// the Outpost's own identifier is read first — exactly as
/// [`crate::site_and_outpost_lifecycle::ids`] reads it.
fn entry(output: &Value, list: &str, id: &str) -> Value {
    output
        .get(list)
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| {
                    let found = item.get("OutpostId").or_else(|| item.get("SiteId"));
                    found.and_then(Value::as_str) == Some(id)
                })
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// A map of `count` distinct tags, used to reach the modelled `TagMap` maximum.
fn many_tags(count: usize) -> Value {
    let mut tags = Map::new();
    for index in 0..count {
        tags.insert(format!("key{index}"), json!("value"));
    }
    Value::Object(tags)
}

/// One tag set to create a resource with, and the `Tags` member every projection of
/// that resource must then report.
struct TagCase {
    case: &'static str,
    sent: Option<Value>,
    expected: Value,
}

/// Rule A's rows: the tag sets a create can carry, spanning none, one, several and
/// the modelled bounds — `TagMap` is `length(1..50)`, `TagKey` `length(1..128)` and
/// `TagValue` `length(0..256)`.
fn tag_cases() -> Vec<TagCase> {
    let several = json!({ "Env": "prod", "team": "platform", "cost.center": "4711" });
    let fifty = many_tags(50);
    let mut at_the_bounds = Map::new();
    at_the_bounds.insert("k".repeat(128), json!("v".repeat(256)));
    vec![
        TagCase {
            case: "no Tags member at all",
            sent: None,
            expected: json!({}),
        },
        TagCase {
            case: "one tag",
            sent: Some(json!({ "Env": "dev" })),
            expected: json!({ "Env": "dev" }),
        },
        TagCase {
            case: "several tags",
            sent: Some(several.clone()),
            expected: several,
        },
        TagCase {
            case: "the modelled maximum of 50 tags",
            sent: Some(fifty.clone()),
            expected: fifty,
        },
        TagCase {
            case: "a key and a value at their modelled maxima",
            sent: Some(Value::Object(at_the_bounds.clone())),
            expected: Value::Object(at_the_bounds),
        },
    ]
}

/// Rule A over both taggable resources in this slice and all three projections of
/// each: the create, the single read and the list entry.
#[tokio::test]
async fn every_projection_reports_the_resource_tag_map() {
    let outposts = Outposts::new();
    let host = outposts
        .create_site(&json!({ "Name": "tag host site" }))
        .await;
    let host_id = text(&host, "SiteId");

    for TagCase {
        case,
        sent,
        expected,
    } in tag_cases()
    {
        let mut site_request = json!({ "Name": format!("site with {case}") });
        let mut outpost_request =
            json!({ "Name": format!("outpost with {case}"), "SiteId": host_id });
        if let Some(tags) = &sent {
            site_request["Tags"] = tags.clone();
            outpost_request["Tags"] = tags.clone();
        }

        let site = outposts.create_site(&site_request).await;
        let site_id = text(&site, "SiteId");
        let site_projections = [
            ("CreateSite", site.clone()),
            ("GetSite", outposts.get_site(&site_id).await),
            (
                "ListSites",
                entry(&outposts.list_sites("").await, "Sites", &site_id),
            ),
        ];
        for (operation, projection) in site_projections {
            assert_eq!(
                projection.get("Tags"),
                Some(&expected),
                "case {case:?}: {operation} must report the site tags, got {projection:?}"
            );
        }

        let outpost = outposts.create_outpost(&outpost_request).await;
        let outpost_id = text(&outpost, "OutpostId");
        let outpost_projections = [
            ("CreateOutpost", outpost.clone()),
            ("GetOutpost", outposts.get_outpost(&outpost_id).await),
            (
                "ListOutposts",
                entry(&outposts.list_outposts("").await, "Outposts", &outpost_id),
            ),
        ];
        for (operation, projection) in outpost_projections {
            assert_eq!(
                projection.get("Tags"),
                Some(&expected),
                "case {case:?}: {operation} must report the Outpost tags, got {projection:?}"
            );
        }
    }
}

/// Rule A's other side: an EMPTY `Tags` map in a REQUEST is still refused, because
/// `TagMap`'s `length(min: 1)` constrains the input the model validates.
#[tokio::test]
async fn an_empty_tag_map_in_a_request_is_refused() {
    let outposts = Outposts::new();
    let site = outposts.create_site(&json!({ "Name": "bounds site" })).await;
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "CreateSite",
            "/sites",
            json!({ "Name": "empty tags", "Tags": {} }),
        ),
        (
            "CreateOutpost",
            "/outposts",
            json!({
                "Name": "empty tags",
                "SiteId": text(&site, "SiteId"),
                "Tags": {},
            }),
        ),
    ];
    for (case, path, request) in &cases {
        let reply = outposts.call("POST", path, Some(request)).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: an empty Tags map is below the modelled minimum, got {} {} {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule B, exhaustively over the whole domain the rule has: every region in
/// [`REGIONS`] crossed with every zone index the modelled one-digit
/// `AvailabilityZoneId` can name, named BOTH ways round — so no generator is needed
/// to cover it. Each create is ADDRESSED to the region whose zone it names, because a
/// zone is only usable in its own region.
#[tokio::test]
async fn an_outpost_reports_both_availability_zone_identities() {
    let outposts = Outposts::new();

    for (region, abbreviation) in REGIONS {
        // An Outpost is placed on a site of its OWN region, and is readable only
        // through that region, so each region's rows get a site of their own there.
        let site_id = outposts
            .site_id_in(region, &format!("zone site {region}"))
            .await;
        for index in 1..=MAX_ZONE_INDEX {
            let letter = char::from(b'a' + u8::try_from(index - 1).unwrap_or(0));
            let zone = format!("{region}{letter}");
            let zone_id = format!("{abbreviation}-az{index}");

            // Named by the zone name, and named by the zone id: either way the
            // stored Outpost carries the same pair.
            let named_by = [
                (
                    format!("{zone} by name"),
                    json!({ "Name": format!("by-name-{zone}"), "SiteId": site_id, "AvailabilityZone": zone }),
                ),
                (
                    format!("{zone_id} by id"),
                    json!({ "Name": format!("by-id-{zone_id}"), "SiteId": site_id, "AvailabilityZoneId": zone_id }),
                ),
            ];
            for (case, request) in named_by {
                let outpost = outposts.create_outpost_in(region, &request).await;
                let outpost_id = text(&outpost, "OutpostId");
                assert!(
                    is_zone_id(&text(&outpost, "AvailabilityZoneId")),
                    "case {case:?}: AvailabilityZoneId must be the modelled form, got {outpost:?}"
                );
                // The create, the read and the list entry all report the pair, and
                // both modelled zone filters select the Outpost by it.
                let filtered = format!(
                    "?AvailabilityZoneFilter={}&AvailabilityZoneIdFilter={}",
                    query_value(&zone),
                    query_value(&zone_id)
                );
                let projections = [
                    ("CreateOutpost", outpost.clone()),
                    ("GetOutpost", outposts.get_outpost_in(region, &outpost_id).await),
                    (
                        "ListOutposts",
                        entry(
                            &outposts.list_outposts_in(region, &filtered).await,
                            "Outposts",
                            &outpost_id,
                        ),
                    ),
                ];
                for (operation, projection) in projections {
                    assert_eq!(
                        (
                            text(&projection, "AvailabilityZone"),
                            text(&projection, "AvailabilityZoneId")
                        ),
                        (zone.clone(), zone_id.clone()),
                        "case {case:?}: {operation} must report both zone identities, \
                         got {projection:?}"
                    );
                }
            }
        }
    }
}

/// Rule B's other side: a zone value the OTHER identity cannot be resolved from
/// leaves that member ABSENT rather than carrying an invented value.
///
/// Every identifier that names no zone of the request's region is refused outright
/// (covered in [`crate::availability_zone_identity`]), so the only accepted value the
/// second identity cannot be resolved from is a zone of a region whose zone-ID
/// abbreviation does not follow the published `<area><word initials><number>` form —
/// the rows below are addressed to such a region, naming the zone each way round.
#[tokio::test]
async fn an_unresolvable_zone_leaves_the_other_identity_absent() {
    let outposts = Outposts::new();
    // The region every row below is addressed to, and whose zones they name: its
    // final segment is not the single digit an `AvailabilityZoneId` abbreviation ends
    // in, so the name <-> id mapping has nothing to map it through.
    let region = "zz-testing-12";
    // The site is created in that region too: an Outpost is placed on a site of its
    // OWN region.
    let site_id = outposts
        .site_id_in(region, "unresolvable zone site")
        .await;

    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a zone of a region with no zone-ID abbreviation, named by name",
            "AvailabilityZone",
            "zz-testing-12a",
        ),
        (
            "a zone of a region with no zone-ID abbreviation, named by id",
            "AvailabilityZoneId",
            "zzt1-az1",
        ),
    ];
    for (case, member, value) in &cases {
        let other = match *member {
            "AvailabilityZone" => "AvailabilityZoneId",
            _ => "AvailabilityZone",
        };
        let mut request = json!({ "Name": format!("outpost {case}"), "SiteId": site_id });
        request[*member] = json!(value);
        let outpost = outposts.create_outpost_in(region, &request).await;
        assert_eq!(
            outpost.get(*member).and_then(Value::as_str),
            Some(*value),
            "case {case:?}: {member} must come back as it was stored, got {outpost:?}"
        );
        assert_eq!(
            outpost.get(other),
            None,
            "case {case:?}: {other} must stay absent rather than be invented, got {outpost:?}"
        );
        // The read-back agrees with the create, so nothing filled the gap later.
        let read = outposts
            .get_outpost_in(region, &text(&outpost, "OutpostId"))
            .await;
        assert_eq!(
            read.get(other),
            None,
            "case {case:?}: GetOutpost must report no {other} either, got {read:?}"
        );
    }
}

/// Rule C: every create reports the provisioning status, and once the clock-owed
/// transition is due every projection reports the settled one — and keeps reporting
/// it, while an Outpost created after that settle starts provisioning in its turn.
///
/// The rows differ along what else the create carries (tags, each zone identity, the
/// hardware type), none of which the lifecycle depends on.
#[tokio::test]
async fn a_created_outpost_provisions_then_settles_on_the_clock() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "lifecycle site" }))
        .await;
    let site_id = text(&site, "SiteId");

    let requests: Vec<(&str, Value)> = vec![
        (
            "name and site only",
            json!({ "Name": "plain", "SiteId": site_id }),
        ),
        (
            "tags and a zone name",
            json!({
                "Name": "tagged",
                "SiteId": site_id,
                "Tags": { "Env": "dev" },
                "AvailabilityZone": "us-east-1b",
            }),
        ),
        (
            "a zone id and a hardware type",
            json!({
                "Name": "server form factor",
                "SiteId": site_id,
                "AvailabilityZoneId": "use1-az3",
                "SupportedHardwareType": "SERVER",
            }),
        ),
    ];

    let mut created: Vec<(&str, Value)> = Vec::new();
    for (case, request) in &requests {
        let outpost = outposts.create_outpost(request).await;
        let status = text(&outpost, "LifeCycleStatus");
        assert_eq!(
            status, "PENDING",
            "case {case:?}: a just-created Outpost is still provisioning, got {outpost:?}"
        );
        assert!(
            is_life_cycle_status(&status),
            "case {case:?}: LifeCycleStatus must be the modelled form, got {status:?}"
        );
        created.push((case, outpost));
    }

    // Nothing sleeps inside the service: the transition is owed at an instant, so
    // waiting past it is what makes it due.
    settle();

    for (case, outpost) in &created {
        let outpost_id = text(outpost, "OutpostId");
        let read = outposts.get_outpost(&outpost_id).await;
        let listed = entry(&outposts.list_outposts("").await, "Outposts", &outpost_id);
        for (operation, projection) in [("GetOutpost", &read), ("ListOutposts", &listed)] {
            assert_eq!(
                text(projection, "LifeCycleStatus"),
                "ACTIVE",
                "case {case:?}: {operation} must report the settled status, got {projection:?}"
            );
        }
        // The transition moves the status and nothing else, and a second read does
        // not move it again.
        assert_eq!(
            without(&read, "LifeCycleStatus"),
            without(outpost, "LifeCycleStatus"),
            "case {case:?}: settling must change only LifeCycleStatus"
        );
        assert_eq!(
            outposts.get_outpost(&outpost_id).await,
            read,
            "case {case:?}: a settled Outpost reads the same every time"
        );
        // The status filter selects on the status the Outpost now holds.
        assert!(
            ids(
                &outposts.list_outposts("?LifeCycleStatusFilter=ACTIVE").await,
                "Outposts"
            )
            .contains(&outpost_id),
            "case {case:?}: LifeCycleStatusFilter=ACTIVE must select a settled Outpost"
        );
        assert!(
            !ids(
                &outposts
                    .list_outposts("?LifeCycleStatusFilter=PENDING")
                    .await,
                "Outposts"
            )
            .contains(&outpost_id),
            "case {case:?}: LifeCycleStatusFilter=PENDING must not select a settled Outpost"
        );
    }

    // The span is per-Outpost, not a one-off for the store: one created now is
    // provisioning even though every earlier Outpost has settled.
    let latecomer = outposts
        .create_outpost(&json!({ "Name": "latecomer", "SiteId": site_id }))
        .await;
    assert_eq!(
        text(&latecomer, "LifeCycleStatus"),
        "PENDING",
        "an Outpost created after the others settled provisions in its turn, got {latecomer:?}"
    );
}
