// SPDX-License-Identifier: CC0-1.0

//! How `DeleteSite` refuses a site that still has Outposts linked to it.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — reusing
//! the harness of [`crate::site_and_outpost_lifecycle`], so what it asserts is the
//! emulator's HTTP behaviour rather than a handler called directly.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! * While at least one Outpost is linked to the site a `DeleteSite` names, the call is
//!   refused with the modelled `ValidationException` (HTTP 400) and the sentence AWS
//!   publishes for it — the fixed text around one variable part, the site the request
//!   named, resolved to its bare id and with no dependent Outpost, count or resource
//!   member added — and nothing is written, so the site and all its Outposts are
//!   exactly as they were. The same request is accepted the moment no Outpost is linked
//!   to the site any more.
//!
//! The rows differ along what the rule generalises over: how many Outposts are linked
//! to the site (one, two, three, and several after some have gone), the identifier form
//! the request names the site by (bare id, full site ARN), and the region the whole
//! workflow is addressed to. The site's own name and address are not what the rule is
//! about, so they do not vary.
//!
//! How this refusal is CLASSIFIED against the other calls a site's Outposts hold back —
//! which answer the modelled `ConflictException` instead — is
//! [`crate::dependent_resource_conflict`].

use serde_json::json;

use crate::site_and_outpost_lifecycle::{segment, text, Outposts, Reply, HOME_REGION};

/// The sentence AWS answers for this refusal, written out here rather than read from
/// the crate, so the wording is pinned independently of the implementation.
///
/// Source: the recorded `DeleteSite` refusal of a site that still has an Outpost —
/// fixed text `Site ` and ` cannot be deleted because it has one or more outposts.`
/// around the one variable part, the site id the request resolved to.
fn expected_message(site_id: &str) -> String {
    format!("Site {site_id} cannot be deleted because it has one or more outposts.")
}

/// `DeleteSite` on either identifier form, addressed to `region`.
async fn delete_site(outposts: &Outposts, region: &str, identifier: &str) -> Reply {
    outposts
        .call_in(
            region,
            "DELETE",
            &format!("/sites/{}", segment(identifier)),
            None,
        )
        .await
}

/// Assert that `reply` is the published refusal of `site_id`: the modelled
/// `ValidationException`, its status, and AWS's sentence with this site's bare id in
/// it — and none of the resource members the service's conflict error carries.
fn assert_refused(reply: &Reply, case: &str, site_id: &str) {
    assert_eq!(
        (
            reply.status,
            reply.error_code.as_str(),
            reply.errortype_header.as_str()
        ),
        (400, "ValidationException", "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {} {} {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        text(&reply.body, "Message"),
        expected_message(site_id),
        "case {case:?}: expected the published refusal of {site_id}, got {:?}",
        reply.body
    );
    for member in ["ResourceId", "ResourceType"] {
        assert!(
            reply.body.get(member).is_none(),
            "case {case:?}: this refusal names no {member}, got {:?}",
            reply.body
        );
    }
}

/// The rule, over the number of linked Outposts, both identifier forms and two
/// regions.
///
/// Each row drives the whole workflow: the refusal while Outposts are linked (by both
/// forms the modelled `SiteId` pattern accepts), the read-back showing nothing was
/// written, the refusal still standing while only some of the Outposts have gone, and
/// the same request accepted once the last one has.
#[tokio::test]
async fn delete_site_is_refused_while_an_outpost_is_linked_to_the_site() {
    // (case, region, how many Outposts are linked to the site)
    let cases: [(&str, &str, usize); 6] = [
        ("one linked Outpost", HOME_REGION, 1),
        ("two linked Outposts", HOME_REGION, 2),
        ("three linked Outposts", HOME_REGION, 3),
        ("one linked Outpost, another region", "us-west-2", 1),
        ("two linked Outposts, another region", "eu-central-1", 2),
        ("three linked Outposts, another region", "us-west-2", 3),
    ];

    for (case, region, linked) in cases {
        let outposts = Outposts::new();
        let site = outposts
            .create_site_in(region, &json!({ "Name": "site with Outposts" }))
            .await;
        let site_id = text(&site, "SiteId");
        let site_arn = text(&site, "SiteArn");

        let mut dependents: Vec<String> = Vec::new();
        for index in 1..=linked {
            let outpost = outposts
                .create_outpost_in(
                    region,
                    &json!({ "Name": format!("linked-{index}"), "SiteId": site_id }),
                )
                .await;
            dependents.push(text(&outpost, "OutpostId"));
        }

        // Both identifier forms name the same site, so both are refused — and the
        // refusal names the site by the bare id the form resolved to either way.
        for identifier in [site_id.clone(), site_arn.clone()] {
            let row = format!("{case} in {region}, site named as {identifier:?}");
            let refused = delete_site(&outposts, region, &identifier).await;
            assert_refused(&refused, &row, &site_id);
        }

        // The refusals wrote nothing: the site reads back exactly as it was created,
        // and every Outpost is still linked to it.
        let read_back = outposts
            .ok_in(region, "GET", &format!("/sites/{site_id}"), None)
            .await["Site"]
            .clone();
        assert_eq!(
            read_back, site,
            "case {case:?} in {region}: a refused delete leaves the site exactly as it was, \
             got {read_back:?}"
        );
        for outpost_id in &dependents {
            let still_linked = outposts.get_outpost_in(region, outpost_id).await;
            assert_eq!(
                text(&still_linked, "SiteId"),
                site_id,
                "case {case:?} in {region}: a refused delete leaves {outpost_id} linked, \
                 got {still_linked:?}"
            );
        }

        // Clearing the Outposts one at a time: the refusal stands while any of them is
        // still linked, and the LAST one is what the delete was waiting for.
        for (cleared, outpost_id) in dependents.iter().enumerate() {
            outposts
                .ok_in(region, "DELETE", &format!("/outposts/{outpost_id}"), None)
                .await;
            let remaining = dependents.len() - cleared - 1;
            let row = format!("{case} in {region}, {remaining} still linked");
            let reply = delete_site(&outposts, region, &site_id).await;
            if remaining > 0 {
                assert_refused(&reply, &row, &site_id);
            } else {
                assert_eq!(
                    (reply.status, &reply.body),
                    (200, &json!({})),
                    "case {row:?}: with no Outpost linked the delete is accepted with an \
                     empty body, got {} {:?}",
                    reply.status,
                    reply.body
                );
            }
        }

        // The accepted delete removed the site, so every later call sees it gone.
        let gone = outposts
            .call_in(region, "GET", &format!("/sites/{site_id}"), None)
            .await;
        assert_eq!(
            (gone.status, gone.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?} in {region}: the deleted site is gone to GetSite, got {} {:?}",
            gone.status,
            gone.body
        );
    }
}

/// Where the Outpost a row creates is placed, relative to the site the row deletes.
enum Placement {
    /// No Outpost exists at all.
    None,
    /// On the site the row deletes.
    ThisSite,
    /// On another site of the same region.
    AnotherSite,
    /// On a site of another region entirely.
    AnotherRegion,
}

/// The boundary of the rule in the other direction: what holds the delete back is an
/// Outpost linked to THIS site in THIS region, and nothing else.
///
/// The rows differ along where the Outpost that exists is placed — nowhere, on the site
/// the request names, on another site of the same region, or on a site of another
/// region. Only the second holds the delete back.
#[tokio::test]
async fn only_an_outpost_linked_to_the_named_site_refuses_its_delete() {
    let cases: [(&str, Placement); 4] = [
        ("no Outpost anywhere", Placement::None),
        ("an Outpost on this very site", Placement::ThisSite),
        (
            "an Outpost on another site of the same region",
            Placement::AnotherSite,
        ),
        (
            "an Outpost on a site of another region",
            Placement::AnotherRegion,
        ),
    ];

    for (case, placement) in cases {
        let outposts = Outposts::new();
        let site_id = outposts.site_id_in(HOME_REGION, "deleted site").await;
        let (host_region, host_site) = match placement {
            Placement::None => (HOME_REGION, None),
            Placement::ThisSite => (HOME_REGION, Some(site_id.clone())),
            Placement::AnotherSite => (
                HOME_REGION,
                Some(outposts.site_id_in(HOME_REGION, "other site").await),
            ),
            Placement::AnotherRegion => {
                let region = "ap-southeast-2";
                (
                    region,
                    Some(outposts.site_id_in(region, "other site").await),
                )
            }
        };
        if let Some(host_site) = &host_site {
            outposts
                .create_outpost_in(
                    host_region,
                    &json!({ "Name": "an Outpost", "SiteId": host_site }),
                )
                .await;
        }

        let reply = delete_site(&outposts, HOME_REGION, &site_id).await;
        if matches!(placement, Placement::ThisSite) {
            assert_refused(&reply, case, &site_id);
        } else {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: no Outpost is linked to this site, expected HTTP 200, \
                 got {} {:?}",
                reply.status, reply.body
            );
        }
    }
}
