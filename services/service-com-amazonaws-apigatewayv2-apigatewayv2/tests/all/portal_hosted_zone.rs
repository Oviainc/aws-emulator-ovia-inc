// SPDX-License-Identifier: CC0-1.0

//! The hosted zone a developer portal's default domain is aliased through:
//! `EndpointConfigurationResponse$PortalDomainHostedZoneId`, "the portal domain hosted
//! zone identifier" (`smithy_query(query="shape", shape="EndpointConfigurationResponse")`:
//! `@required`, `@length min 1 max 64`).
//!
//! The rule these cases assert, one level above the recorded case (one portal created in
//! one Region and read once):
//!
//! > **The hosted zone id a portal reports is the zone of the Region's portal default
//! > domain, not a value minted for the portal**: AWS assigns one such zone per Region, so
//! > every portal created in a Region reports the SAME id, every reader of a portal and
//! > every write to it reports that same id, and a portal in another Region reports a
//! > different one. Where AWS's own answer for the Region is known, the service reports
//! > THAT id ([`PORTAL_DOMAIN_HOSTED_ZONES`], pinned here by value in
//! > [`PUBLISHED_ZONE_IDS`]) rather than one of its own.
//!
//! The rows span what the rule generalises over: the Region the request is signed for (the
//! Regions AWS's published answer is known for, and Regions it is not, in three
//! partitions), whether
//! the portal brought a domain of its own or took API Gateway's default, how many portals
//! a Region holds, which operation reports the configuration (`CreatePortal`, `GetPortal`,
//! `ListPortals`, `UpdatePortal`), and the Region a later patch is signed for. The entry
//! the issue cited (`us-west-2`) is ONE row; the published CloudFront zone the same crate
//! carries for an `EDGE` custom domain name is covered by `custom_domain_hosted_zone`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    published_portal_domain_hosted_zone_id, PORTAL_DOMAIN_HOSTED_ZONES,
};

use crate::api_lifecycle::{as_str, call_in_region, emulator};
use crate::portal_lifecycle::portal_content;

/// The portals collection (`CreatePortal`'s modelled `POST /v2/portals`).
const COLLECTION: &str = "/v2/portals";

/// The member under test, as `EndpointConfigurationResponse` names it on the wire.
const HOSTED_ZONE_ID: &str = "portalDomainHostedZoneId";

/// The modelled bound of `PortalDomainHostedZoneId` (`__stringMin1Max64`).
const HOSTED_ZONE_ID_LEN: (usize, usize) = (1, 64);

/// Regions with no published portal zone id carried by this service, spanning three
/// partitions' naming. Asserted for the per-Region invariance the member keeps everywhere.
const UNPUBLISHED_REGIONS: &[&str] = &["eu-west-1", "ap-southeast-2", "us-east-2", "af-south-1"];

/// What AWS itself answers for a portal created in a Region, Region by Region: the zone of
/// that Region's portal default domain, which AWS assigns and this suite pins by VALUE.
///
/// The suite's OWN copy of the catalog the service carries
/// ([`PORTAL_DOMAIN_HOSTED_ZONES`]), so the service asserts nothing against itself: an id
/// added to or edited in the service's table without a published AWS answer behind it
/// fails [`the_service_carries_the_zone_ids_aws_publishes`], and a Region added to either
/// belongs in both. `us-west-2` is the recorded AWS answer for a portal created there.
const PUBLISHED_ZONE_IDS: &[(&str, &str)] = &[("us-west-2", "Z091913938PZVWK6ZMXI7")];

/// A portal request taking API Gateway's own default domain.
fn default_domain_request(display_name: &str) -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content(display_name)
    })
}

/// A portal request bringing the account's own domain and certificate — the other
/// alternative of `EndpointConfigurationRequest`, which moves `DomainName` and
/// `CertificateArn` but not the zone of the default domain.
fn own_domain_request(display_name: &str, region: &str) -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": {
            "acmManaged": {
                "certificateArn": format!(
                    "arn:aws:acm:{region}:000000000000:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"
                ),
                "domainName": format!("developers.{region}.example.com")
            }
        },
        "portalContent": portal_content(display_name)
    })
}

/// Create one portal in `region` and answer the created document.
async fn create(
    emulator: &MultiServiceRouter,
    case: &str,
    region: &str,
    request: &Value,
) -> Value {
    let (status, body) =
        call_in_region(emulator, region, "POST", COLLECTION, Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortal, got {status} {body}"
    );
    body
}

/// The `portalDomainHostedZoneId` of a document carrying an `endpointConfiguration`,
/// asserting the member is reported at all and within its modelled bound.
fn hosted_zone_id(case: &str, body: &Value) -> String {
    let endpoint = body.get("endpointConfiguration").unwrap_or_else(|| {
        panic!("case {case:?}: the response must report an endpointConfiguration, got {body}")
    });
    let id = as_str(endpoint, HOSTED_ZONE_ID);
    let (min, max) = HOSTED_ZONE_ID_LEN;
    assert!(
        (min..=max).contains(&id.chars().count()),
        "case {case:?}: expected a hosted zone id within the modelled {min}..{max} bound, \
         got {id:?}"
    );
    assert!(
        id.starts_with('Z')
            && id[1..]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        "case {case:?}: expected a Route 53 hosted zone id (a leading Z and uppercase \
         alphanumerics), got {id:?}"
    );
    id
}

/// The zone ids every reader of `portal_id` reports: the portal read back, the portal as
/// `ListPortals` summarises it, and the portal after a patch signed for `patch_region` —
/// which moves the domain the account brought and must leave the Region's zone alone.
async fn zone_ids_from_every_reader(
    emulator: &MultiServiceRouter,
    case: &str,
    region: &str,
    patch_region: &str,
    portal_id: &str,
) -> Vec<(&'static str, String)> {
    let mut reported = Vec::new();

    let (status, fetched) = call_in_region(
        emulator,
        region,
        "GET",
        &format!("{COLLECTION}/{portal_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetPortal, got {status} {fetched}"
    );
    reported.push(("GetPortal", hosted_zone_id(case, &fetched)));

    let (status, listed) = call_in_region(emulator, region, "GET", COLLECTION, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListPortals, got {status} {listed}"
    );
    let items = listed["items"].as_array().cloned().unwrap_or_default();
    let summary = items
        .iter()
        .find(|item| item.get("portalId").and_then(Value::as_str) == Some(portal_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListPortals must summarise the portal, got {listed}")
        });
    reported.push(("ListPortals", hosted_zone_id(case, &summary)));

    let (status, patched) = call_in_region(
        emulator,
        patch_region,
        "PATCH",
        &format!("{COLLECTION}/{portal_id}"),
        Some(json!({
            "endpointConfiguration": {
                "acmManaged": {
                    "certificateArn": format!(
                        "arn:aws:acm:{patch_region}:000000000000:certificate/\
                         1c2d3e4f-5a6b-7c8d-9e0f-1a2b3c4d5e6f"
                    ),
                    "domainName": format!("patched.{patch_region}.example.com")
                }
            }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdatePortal, got {status} {patched}"
    );
    reported.push(("UpdatePortal", hosted_zone_id(case, &patched)));

    reported
}

/// Every Region reports ONE zone id — the published one where this service carries it —
/// for every portal in it, through every reader, and a different one from any other
/// Region.
#[tokio::test]
async fn a_portals_hosted_zone_is_the_regions_and_not_the_portals() {
    let emulator = emulator();

    // Every Region AWS's published answer is known for, plus Regions it is not: both
    // answer one id per Region, and only the first is pinned to a value.
    let regions: Vec<&str> = PUBLISHED_ZONE_IDS
        .iter()
        .map(|(region, _)| *region)
        .chain(UNPUBLISHED_REGIONS.iter().copied())
        .collect();

    let mut by_region: BTreeMap<&str, String> = BTreeMap::new();
    for region in regions {
        let case = region;
        // Two portals in the Region, differing in the endpoint configuration they asked
        // for: the zone belongs to neither of them.
        let first = create(
            &emulator,
            case,
            region,
            &default_domain_request("Default Domain Portal"),
        )
        .await;
        let second = create(
            &emulator,
            case,
            region,
            &own_domain_request("Own Domain Portal", region),
        )
        .await;
        let first_id = hosted_zone_id(case, &first);
        let second_id = hosted_zone_id(case, &second);
        assert_eq!(
            first_id, second_id,
            "case {case:?}: expected every portal in a region to report that region's \
             hosted zone id, got {first_id:?} and {second_id:?}"
        );

        // Where AWS's own answer for the Region is known, that is the id reported.
        if let Some((_, published)) = PUBLISHED_ZONE_IDS
            .iter()
            .find(|(published_region, _)| *published_region == region)
        {
            assert_eq!(
                first_id, *published,
                "case {case:?}: expected the hosted zone AWS publishes for the region \
                 ({published:?}), got {first_id:?}"
            );
        }

        // Every reader reports it, and a patch signed for another Region leaves it alone.
        let patch_region = UNPUBLISHED_REGIONS
            .iter()
            .copied()
            .find(|other| *other != region)
            .expect("a second region to sign the patch for");
        let portal_id = as_str(&first, "portalId");
        for (reader, reported) in
            zone_ids_from_every_reader(&emulator, case, region, patch_region, &portal_id).await
        {
            assert_eq!(
                reported, first_id,
                "case {case:?}: expected {reader} to report the region's hosted zone id \
                 {first_id:?}, got {reported:?}"
            );
        }

        by_region.insert(region, first_id);
    }

    // One zone per Region: no two Regions share an id.
    for (region, id) in &by_region {
        let sharing: Vec<&&str> = by_region
            .iter()
            .filter(|(other, other_id)| *other != region && *other_id == id)
            .map(|(other, _)| other)
            .collect();
        assert!(
            sharing.is_empty(),
            "case {region:?}: expected a hosted zone id of its own, but {sharing:?} report \
             {id:?} too"
        );
    }
}

/// The service carries exactly the ids AWS publishes: every entry of its catalog is a
/// Region AWS's answer is known for and carries that answer, each within the member's
/// modelled bound, and a Region AWS's answer is not known for carries nothing.
#[test]
fn the_service_carries_the_zone_ids_aws_publishes() {
    let (min, max) = HOSTED_ZONE_ID_LEN;
    let carried: BTreeMap<&str, &str> = PORTAL_DOMAIN_HOSTED_ZONES
        .iter()
        .map(|zone| (zone.region, zone.hosted_zone_id))
        .collect();
    let published: BTreeMap<&str, &str> = PUBLISHED_ZONE_IDS.iter().copied().collect();
    assert_eq!(
        carried, published,
        "expected the service to carry AWS's published zone ids, region for region, got \
         {carried:?} against {published:?}"
    );
    assert_eq!(
        carried.len(),
        PORTAL_DOMAIN_HOSTED_ZONES.len(),
        "expected one entry per region in the carried catalog, got {carried:?}"
    );
    for (region, id) in &published {
        assert!(
            (min..=max).contains(&id.chars().count()),
            "case {region:?}: expected a published id within the modelled {min}..{max} \
             bound, got {id:?}"
        );
        assert_eq!(
            published_portal_domain_hosted_zone_id(region),
            Some(*id),
            "case {region:?}: expected the region's published id, got {:?}",
            published_portal_domain_hosted_zone_id(region)
        );
    }
    for region in UNPUBLISHED_REGIONS {
        assert_eq!(
            published_portal_domain_hosted_zone_id(region),
            None,
            "case {region:?}: AWS's answer for this region is not known, so the service \
             carries none, got {:?}",
            published_portal_domain_hosted_zone_id(region)
        );
    }
}
