// SPDX-License-Identifier: CC0-1.0

//! How this emulator classifies a refusal caused by a resource that still DEPENDS on
//! the resource a request names.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — so what
//! it asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! request harness is the lifecycle suite's, reused rather than rebuilt.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! * When every parameter of a request is valid and what blocks it is another resource
//!   that still depends on the one the request names, the refusal is about the state of
//!   the resource rather than about the request, so the service answers the modelled
//!   `ConflictException` (HTTP 409) with `ResourceId` / `ResourceType` naming the
//!   dependent resource, and the call changes nothing. Once no dependent is left, the
//!   same request succeeds.
//!
//! `DeleteSite` is the one call a site's Outposts hold back that AWS does NOT classify
//! that way — it answers 400 `ValidationException` with a published sentence, which is
//! [`crate::site_delete_refusal`]'s rule — so this suite drives it alongside the
//! conflict rows to pin the boundary between the two classifications on one workflow.
//!
//! The rows differ along the operation the dependent holds back (`DeleteSite`,
//! `UpdateSiteAddress` on the operating address), how many dependents the resource has
//! (one, two, three, and several after some have gone), the identifier form the request
//! names the resource by (bare id, full ARN) and the region the whole workflow is
//! addressed to — not along the name or wording of any one resource.

use serde_json::{json, Value};

use super::site_and_outpost_lifecycle::{
    address, segment, text, Outposts, ABSENT_SITE_ID, HOME_REGION,
};

/// The `ResourceType` a conflict about an Outpost names.
///
/// Source: `smithy_query(query="shape", shape="ConflictException,ResourceType")` —
/// `ConflictException$ResourceType` targets the modelled `ResourceType` enum, whose
/// whole value set is `["OUTPOST", "ORDER"]`. The only resource this service records as
/// depending on a site is an Outpost, so a site's dependent conflict names `OUTPOST`;
/// there is no `SITE` value for such a conflict to name the site itself with.
const RESOURCE_TYPE_OUTPOST: &str = "OUTPOST";

/// The `AddressType` naming the address a site is at — the one an Outpost at the site
/// holds back (`UpdateSiteAddress`: "after all Outposts that belong to the site have
/// been deactivated").
const OPERATING: &str = "OPERATING_ADDRESS";

/// How the refusal of a call held back by a site's Outposts is classified.
enum Refusal {
    /// The dependent-naming conflict the rule above states.
    Conflict,
    /// The published validation refusal `DeleteSite` answers instead.
    Validation,
}

/// One call a site's dependents hold back: the case name, the HTTP method, the path
/// around the site identifier, and the body (absent for `DeleteSite`, which binds its
/// only member to the URI).
struct HeldBack {
    case: &'static str,
    method: &'static str,
    /// The path the site identifier is substituted into, `{site}` marking the slot.
    path: &'static str,
    body: Option<Value>,
    /// Which refusal this call's dependents hold it back with.
    refusal: Refusal,
}

/// Both calls a site's Outposts hold back, so the rule is asserted across the whole
/// class rather than on the one operation it was noticed on.
fn held_back_calls() -> Vec<HeldBack> {
    vec![
        HeldBack {
            case: "DeleteSite",
            method: "DELETE",
            path: "/sites/{site}",
            body: None,
            refusal: Refusal::Validation,
        },
        HeldBack {
            case: "UpdateSiteAddress on the operating address",
            method: "PUT",
            path: "/sites/{site}/address",
            body: Some(json!({
                "AddressType": OPERATING,
                "Address": address("IE", "Leinster", "Dublin"),
            })),
            refusal: Refusal::Conflict,
        },
    ]
}

/// The rule, over every call a site's Outposts hold back, every dependent count, both
/// identifier forms and two regions.
#[tokio::test]
async fn a_call_held_back_by_a_dependent_resource_is_a_conflict_naming_that_dependent() {
    let outposts = Outposts::new();
    let calls = held_back_calls();

    for region in [HOME_REGION, "us-west-2"] {
        let site = outposts
            .create_site_in(
                region,
                &json!({
                    "Name": "site with dependents",
                    "OperatingAddress": address("US", "WA", "Seattle"),
                }),
            )
            .await;
        let site_id = text(&site, "SiteId");
        let site_arn = text(&site, "SiteArn");

        let mut dependents: Vec<String> = Vec::new();
        for count in 1..=3 {
            let outpost = outposts
                .create_outpost_in(
                    region,
                    &json!({ "Name": format!("dependent-{count}"), "SiteId": site_id }),
                )
                .await;
            dependents.push(text(&outpost, "OutpostId"));

            for identifier in [site_id.clone(), site_arn.clone()] {
                for call in &calls {
                    let case = format!(
                        "{} in {region} with {count} dependent(s), site named as {identifier:?}",
                        call.case
                    );
                    let path = call.path.replace("{site}", &segment(&identifier));
                    let refused = outposts
                        .call_in(region, call.method, &path, call.body.as_ref())
                        .await;
                    if matches!(call.refusal, Refusal::Validation) {
                        // The one call of the class AWS refuses as a bad parameter;
                        // its own suite states the sentence.
                        assert_eq!(
                            (
                                refused.status,
                                refused.error_code.as_str(),
                                refused.errortype_header.as_str()
                            ),
                            (400, "ValidationException", "ValidationException"),
                            "case {case}: this call's dependent refusal is the published \
                             validation refusal, got {} {:?}",
                            refused.status,
                            refused.body
                        );
                        continue;
                    }
                    assert_eq!(
                        (
                            refused.status,
                            refused.error_code.as_str(),
                            refused.errortype_header.as_str()
                        ),
                        (409, "ConflictException", "ConflictException"),
                        "case {case}: a call held back by a dependent resource is the modelled \
                         conflict, got {} {:?}",
                        refused.status,
                        refused.body
                    );
                    // The conflict names the dependent that holds the call back, which
                    // is the resource the caller has to clear first.
                    assert_eq!(
                        text(&refused.body, "ResourceType"),
                        RESOURCE_TYPE_OUTPOST,
                        "case {case}: the conflict names the type of the dependent, got {:?}",
                        refused.body
                    );
                    let named = text(&refused.body, "ResourceId");
                    assert!(
                        dependents.contains(&named),
                        "case {case}: ResourceId must name one of the site's Outposts \
                         {dependents:?}, got {named:?}"
                    );
                }
            }
        }

        // Every refused call changed nothing: the site reads back exactly as it was
        // created, and all three dependents are still there.
        let read_back = outposts
            .ok_in(region, "GET", &format!("/sites/{site_id}"), None)
            .await["Site"]
            .clone();
        assert_eq!(
            read_back, site,
            "in {region}: a refused call leaves the site exactly as it was"
        );
        for outpost_id in &dependents {
            let still_there = outposts.get_outpost_in(region, outpost_id).await;
            assert_eq!(
                text(&still_there, "SiteId"),
                site_id,
                "in {region}: a refused call leaves {outpost_id} at the site, got \
                 {still_there:?}"
            );
        }

        // The other side of the rule: with the dependents gone, every one of the same
        // requests is accepted — so what was refused was the dependency and nothing
        // else. The address change comes first, since the delete takes the site away.
        for outpost_id in &dependents {
            outposts
                .ok_in(region, "DELETE", &format!("/outposts/{outpost_id}"), None)
                .await;
        }
        for call in calls.iter().rev() {
            let case = format!("{} in {region} with no dependents left", call.case);
            let path = call.path.replace("{site}", &segment(&site_id));
            let reply = outposts
                .call_in(region, call.method, &path, call.body.as_ref())
                .await;
            assert_eq!(
                reply.status,
                200,
                "case {case}: with no dependent left the same request is accepted, got {} {:?}",
                reply.status,
                reply.body
            );
        }
    }
}

/// The boundary of the rule: a conflict is answered only for a request whose parameters
/// are all valid and whose resource exists. An identifier that names no site is the
/// not-found error each call models (404), and one that is no site identifier at all is
/// the modelled `ValidationException` (400) — never the conflict.
#[tokio::test]
async fn a_held_back_call_answers_a_conflict_only_for_an_existing_resource() {
    let outposts = Outposts::new();

    for call in &held_back_calls() {
        for (case, identifier, status, code) in [
            (
                "an id no site is stored under",
                ABSENT_SITE_ID,
                404,
                "NotFoundException",
            ),
            (
                "a value that is no site identifier",
                "not-a-site-id",
                400,
                "ValidationException",
            ),
        ] {
            let path = call.path.replace("{site}", &segment(identifier));
            let reply = outposts.call(call.method, &path, call.body.as_ref()).await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (status, code),
                "case {} with {case}: expected {status} {code}, got {} {} {:?}",
                call.case,
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }
}
