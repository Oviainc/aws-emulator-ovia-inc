// SPDX-License-Identifier: CC0-1.0

//! How this emulator CLASSIFIES the Outposts failures a client branches on: the HTTP
//! status and the wire error code (restJson1's `__type` and `x-amzn-errortype`), rather
//! than the wording of a message.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as a signed `aws outposts` call arrives — reusing the
//! harness of [`crate::site_and_outpost_lifecycle`], so what it asserts is the
//! emulator's HTTP behaviour.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A delete this service refuses because its target still has dependent resources is
//!    a refusal of the REQUEST, so it answers the modelled `ValidationException` (HTTP
//!    400) naming the target — not `ConflictException` (409), which tells an SDK retry
//!    policy and a client's conflict branch that the same request might later succeed.
//! 2. A `ResourceArn` that is not an Outposts resource ARN is an ARN-syntax failure: the
//!    tagging operations answer HTTP 400 with the error code `BadRequestException`, for
//!    every operation that binds that member and every way a value can fail to be such
//!    an ARN. A well-formed one is never refused that way; it reaches the operation.
//! 3. An Availability Zone belongs to exactly one region, so a create naming a zone — by
//!    name or by zone ID — of a region other than the one the request is addressed to is
//!    the modelled `ValidationException` (HTTP 400), decided BEFORE any resource the
//!    request names is looked up (so a bad zone is never reported as a missing site).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// A well-formed `SiteId` this emulator never mints, so it names no stored site while
/// still satisfying the modelled `^(arn:…)?(os-[a-f0-9]{17})$` pattern.
const ABSENT_SITE_ID: &str = "os-00000000000000000";

/// The account id every generated emulator presents, which its ARNs carry.
const ACCOUNT_ID: &str = "000000000000";

/// The code this service names an ARN-syntax refusal with (rule 2).
const ARN_SYNTAX_CODE: &str = "BadRequestException";

/// A JSON tag object as a comparable map. A value that is not an object (an absent
/// member, or a case that sends none) is no tags at all, which is what an untagged
/// resource has. ONE conversion, so a case's expectation and the emulator's answer
/// cannot be compared through two different readings.
pub(crate) fn tag_map(tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The tags a `ListTagsForResource` answer — or a resource's own projection —
/// reports, as a comparable map.
pub(crate) fn reported_tags(body: &Value) -> BTreeMap<String, String> {
    body.get("Tags").map(tag_map).unwrap_or_default()
}

/// Rule 1, over the dependent counts a site can have: one Outpost, several, and
/// several after some have gone. The refusal is stated for both identifier forms
/// `SiteId` accepts, and the site is still there afterwards — a refused delete that
/// deleted anything would show up on the read-back.
#[tokio::test]
async fn deleting_a_site_with_dependents_is_refused_as_a_bad_request() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "site with dependents" }))
        .await;
    let site_id = text(&site, "SiteId");
    let site_arn = text(&site, "SiteArn");

    let mut dependents: Vec<String> = Vec::new();
    for count in 1..=3 {
        let outpost = outposts
            .create_outpost(&json!({ "Name": format!("dependent-{count}"), "SiteId": site_id }))
            .await;
        dependents.push(text(&outpost, "OutpostId"));

        for identifier in [site_id.clone(), site_arn.clone()] {
            let case = format!("{count} dependent(s), site named as {identifier:?}");
            let refused = outposts
                .call("DELETE", &format!("/sites/{}", segment(&identifier)), None)
                .await;
            assert_eq!(
                (
                    refused.status,
                    refused.error_code.as_str(),
                    refused.errortype_header.as_str()
                ),
                (400, "ValidationException", "ValidationException"),
                "case {case}: a site that still has Outposts is refused as a bad request, \
                 got {:?}",
                refused.body
            );
            assert!(
                text(&refused.body, "Message").contains(&site_id),
                "case {case}: the refusal must name the site it refused, got {:?}",
                refused.body
            );
        }
    }

    // Every dependent gone, and the same request succeeds — so what was refused was
    // the dependency, not the site.
    for outpost_id in &dependents {
        outposts
            .ok("DELETE", &format!("/outposts/{outpost_id}"), None)
            .await;
    }
    let deleted = outposts
        .ok("DELETE", &format!("/sites/{site_id}"), None)
        .await;
    assert_eq!(
        deleted,
        json!({}),
        "with no dependents left the site deletes, got {deleted:?}"
    );
}

/// Rule 2, over the ways a value can fail to be an Outposts resource ARN, on EVERY
/// operation that binds `ResourceArn` (`ListTagsForResource` GET, `TagResource` POST,
/// `UntagResource` DELETE — all three on `/tags/{ResourceArn}`).
///
/// The rows are the classes the modelled `Arn`
/// `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$`
/// can be failed in: not an ARN at all, a bare resource id, another service's ARN, a
/// malformed account or resource id, a missing resource type, and a value past the
/// member's 1011-character maximum.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_outposts_arn_is_an_arn_syntax_refusal() {
    let outposts = Outposts::new();
    let over_long = format!(
        "arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-{}",
        "a".repeat(1011)
    );
    let malformed: Vec<(&str, String)> = vec![
        ("not an ARN at all", "not-a-valid-arn".to_string()),
        ("a bare resource id", "op-0123456789abcdef0".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:s3:us-east-1:{ACCOUNT_ID}:bucket/op-0123456789abcdef0"),
        ),
        (
            "an account that is not 12 digits",
            "arn:aws:outposts:us-east-1:12345:outpost/op-0123456789abcdef0".to_string(),
        ),
        (
            "a resource id that is not 17 hex characters",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-nothexadecimal0"),
        ),
        (
            "no resource type",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:op-0123456789abcdef0"),
        ),
        ("past the member's maximum length", over_long),
    ];

    // Every operation that binds `ResourceArn`, with the rest of its input valid so the
    // ARN is the only thing wrong with the request.
    let operations: [(&str, &str, Option<Value>); 3] = [
        ("ListTagsForResource", "GET", None),
        (
            "TagResource",
            "POST",
            Some(json!({ "Tags": { "Env": "dev" } })),
        ),
        ("UntagResource", "DELETE", None),
    ];

    for (case, arn) in &malformed {
        for (operation, method, body) in &operations {
            let path = match *operation {
                "UntagResource" => format!("/tags/{}?tagKeys=Env", segment(arn)),
                _ => format!("/tags/{}", segment(arn)),
            };
            let refused = outposts.call(method, &path, body.as_ref()).await;
            assert_eq!(
                (
                    refused.status,
                    refused.error_code.as_str(),
                    refused.errortype_header.as_str()
                ),
                (400, ARN_SYNTAX_CODE, ARN_SYNTAX_CODE),
                "case {operation} with {case} ({arn:?}): an ARN this API cannot read is an \
                 ARN-syntax refusal, got {:?}",
                refused.body
            );
        }
    }

    // The ARN is read before the rest of the request is reported on, so an
    // ARN-syntax failure is what the caller is told about even when another member
    // is invalid too: here an empty `Tags` map, which is below the modelled
    // `TagMap` minimum of 1.
    let also_invalid = outposts
        .call(
            "POST",
            &format!("/tags/{}", segment("not-a-valid-arn")),
            Some(&json!({ "Tags": {} })),
        )
        .await;
    assert_eq!(
        (
            also_invalid.status,
            also_invalid.error_code.as_str(),
            also_invalid.errortype_header.as_str()
        ),
        (400, ARN_SYNTAX_CODE, ARN_SYNTAX_CODE),
        "case TagResource with a bad ARN and a second invalid member: the ARN-syntax \
         refusal still wins, got {:?}",
        also_invalid.body
    );

    // The other side of the rule: a well-formed Outposts resource ARN is NEVER refused
    // as bad syntax — it reaches the operation, which answers from the store (or, for an
    // operation nobody has implemented yet, says so).
    let well_formed = format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-0123456789abcdef0");
    for (operation, method, body) in &operations {
        let path = match *operation {
            "UntagResource" => format!("/tags/{}?tagKeys=Env", segment(&well_formed)),
            _ => format!("/tags/{}", segment(&well_formed)),
        };
        let reply = outposts.call(method, &path, body.as_ref()).await;
        assert_ne!(
            reply.error_code, ARN_SYNTAX_CODE,
            "case {operation} with a well-formed ARN: must not be an ARN-syntax refusal, \
             got {} {:?}",
            reply.status, reply.body
        );
    }
}

/// Rule 2's reachable side, end to end: with a readable ARN, `ListTagsForResource`
/// answers the tags that were PERSISTED on the resource the ARN names, and an ARN that
/// names nothing is the modelled 404.
///
/// The rows differ along the resource kind (site, Outpost) and the tag map (none, one,
/// several), since those are what the answer is computed from.
#[tokio::test]
async fn list_tags_for_resource_answers_the_tags_persisted_on_the_resource() {
    let outposts = Outposts::new();
    let tag_maps: [(&str, Value); 3] = [
        ("no tags", json!(null)),
        ("one tag", json!({ "Env": "prod" })),
        (
            "several tags",
            json!({ "Env": "dev", "team": "platform", "cost-centre": "4711" }),
        ),
    ];

    let mut cases: Vec<(String, String, BTreeMap<String, String>)> = Vec::new();
    for (case, tags) in &tag_maps {
        let mut site_request = json!({ "Name": format!("tagged site: {case}") });
        let mut outpost_request = json!({ "Name": format!("tagged outpost: {case}") });
        let expected: BTreeMap<String, String> = tag_map(tags);
        if tags.is_object() {
            site_request["Tags"] = tags.clone();
            outpost_request["Tags"] = tags.clone();
        }

        let site = outposts.create_site(&site_request).await;
        outpost_request["SiteId"] = json!(text(&site, "SiteId"));
        let outpost = outposts.create_outpost(&outpost_request).await;
        cases.push((
            format!("site with {case}"),
            text(&site, "SiteArn"),
            expected.clone(),
        ));
        cases.push((
            format!("outpost with {case}"),
            text(&outpost, "OutpostArn"),
            expected,
        ));
    }

    for (case, arn, expected) in &cases {
        let answered = outposts
            .ok("GET", &format!("/tags/{}", segment(arn)), None)
            .await;
        assert_eq!(
            &reported_tags(&answered),
            expected,
            "case {case:?}: ListTagsForResource answers the tags the resource was stored \
             with, got {answered:?}"
        );
    }

    // A readable ARN that names nothing is the not-found error this operation models —
    // not an ARN-syntax refusal, and not an empty success.
    for (case, arn) in [
        (
            "absent outpost",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-00000000000000000"),
        ),
        (
            "absent site",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:site/{ABSENT_SITE_ID}"),
        ),
        (
            "a resource kind this service does not hold",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:order/oo-00000000000000000"),
        ),
    ] {
        let reply = outposts
            .call("GET", &format!("/tags/{}", segment(&arn)), None)
            .await;
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                reply.errortype_header.as_str()
            ),
            (404, "NotFoundException", "NotFoundException"),
            "case {case:?}: an ARN naming nothing is not found, got {:?}",
            reply.body
        );
    }
}

/// One row of rule 3's refused table: the case, the region the request is addressed to,
/// the `AvailabilityZone` and the `AvailabilityZoneId` it names, and the value the
/// refusal must name back.
type RefusedZone = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
);

/// Rule 3, over regions of several shapes and both ways to name a zone.
///
/// The accepted rows pair a region with one of its own zones; the refused rows pair it
/// with another region's. The regions span the forms an AWS region code takes — a
/// single direction word (`us-east-1`), a compound one (`ap-southeast-2`), a
/// non-direction word (`eu-central-1`, `ca-central-1`) and a partition word
/// (`us-gov-west-1`) — because the zone ID's short code is derived from exactly that
/// structure (`us-east-1` -> `use1-az1`, `ap-southeast-2` -> `apse2-az1`,
/// `us-gov-west-1` -> `usgw1-az1`), as the AWS Availability Zone ID documentation
/// describes it.
///
/// Every refused row names a site that does NOT exist, so a row that passed only
/// because the site lookup ran first would answer 404 and fail here.
#[tokio::test]
async fn create_outpost_refuses_an_availability_zone_of_another_region() {
    let outposts = Outposts::new();
    let site = outposts.create_site(&json!({ "Name": "zone site" })).await;
    let site_id = text(&site, "SiteId");

    // (case, region, AvailabilityZone, AvailabilityZoneId)
    let accepted: [(&str, &str, Option<&str>, Option<&str>); 7] = [
        ("zone name only", "us-east-1", Some("us-east-1a"), None),
        ("zone id only", "us-east-1", None, Some("use1-az1")),
        (
            "both, another region",
            "us-west-2",
            Some("us-west-2b"),
            Some("usw2-az3"),
        ),
        (
            "compound direction",
            "ap-southeast-2",
            Some("ap-southeast-2c"),
            Some("apse2-az1"),
        ),
        (
            "non-direction word",
            "eu-central-1",
            Some("eu-central-1a"),
            Some("euc1-az2"),
        ),
        (
            "partition word",
            "us-gov-west-1",
            Some("us-gov-west-1a"),
            Some("usgw1-az1"),
        ),
        ("no zone named at all", "ca-central-1", None, None),
    ];
    for (case, region, zone, zone_id) in accepted {
        let mut request = json!({ "Name": format!("zoned-{case}"), "SiteId": site_id });
        if let Some(zone) = zone {
            request["AvailabilityZone"] = json!(zone);
        }
        if let Some(zone_id) = zone_id {
            request["AvailabilityZoneId"] = json!(zone_id);
        }
        let reply = outposts.call_in(region, "POST", "/outposts", Some(&request)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} in {region}: a zone of the request's own region is accepted, \
             got {} {:?}",
            reply.status, reply.body
        );
        let outpost = reply.body["Outpost"].clone();
        // Whichever identity the request named comes back as it was sent. The OTHER
        // identity is resolved from it, which is `site_and_outpost_projection`'s rule
        // B, not this one's.
        for (member, sent) in [("AvailabilityZone", zone), ("AvailabilityZoneId", zone_id)] {
            if let Some(sent) = sent {
                assert_eq!(
                    outpost.get(member).and_then(Value::as_str),
                    Some(sent),
                    "case {case:?} in {region}: the stored Outpost carries the {member} it \
                     was created with, got {outpost:?}"
                );
            }
        }
    }

    let refused: [RefusedZone; 7] = [
        (
            "zone id of another region",
            "us-west-2",
            None,
            Some("use1-az1"),
            "use1-az1",
        ),
        (
            "zone name of another region",
            "us-east-1",
            Some("eu-west-1b"),
            None,
            "eu-west-1b",
        ),
        (
            "neighbouring region's zone name",
            "us-east-1",
            Some("us-east-2a"),
            None,
            "us-east-2a",
        ),
        (
            "same geography, other direction",
            "ca-central-1",
            Some("us-east-1a"),
            None,
            "us-east-1a",
        ),
        (
            "compound-direction region's zone id elsewhere",
            "ap-southeast-2",
            None,
            Some("apne1-az1"),
            "apne1-az1",
        ),
        (
            "name of this region, id of another",
            "us-east-1",
            Some("us-east-1a"),
            Some("usw2-az1"),
            "usw2-az1",
        ),
        (
            "both of another region: the zone id is the one named back",
            "us-west-2",
            Some("us-east-1a"),
            Some("use1-az1"),
            "use1-az1",
        ),
    ];
    for (case, region, zone, zone_id, offending) in refused {
        // The site is absent on purpose: the zone must be judged before the lookup.
        let mut request = json!({ "Name": format!("rejected-{case}"), "SiteId": ABSENT_SITE_ID });
        if let Some(zone) = zone {
            request["AvailabilityZone"] = json!(zone);
        }
        if let Some(zone_id) = zone_id {
            request["AvailabilityZoneId"] = json!(zone_id);
        }
        let reply = outposts.call_in(region, "POST", "/outposts", Some(&request)).await;
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                reply.errortype_header.as_str()
            ),
            (400, "ValidationException", "ValidationException"),
            "case {case:?} in {region}: a zone of another region is a bad parameter, \
             got {:?}",
            reply.body
        );
        assert!(
            text(&reply.body, "Message").contains(offending),
            "case {case:?} in {region}: the refusal must name the zone it refused \
             ({offending}), got {:?}",
            reply.body
        );
    }

    // A value that names the request's OWN region but no zone within it is refused
    // too — it is no zone of this region either — but it is not a cross-region
    // mismatch, so it is not worded as one. (Only the zone NAME can carry such a
    // value: a bare region abbreviation is not the modelled `AvailabilityZoneId`
    // form, so the boundary refuses it before the handler sees it.)
    for (case, member, value) in [(
        "a region code with no zone letter",
        "AvailabilityZone",
        "us-west-2",
    )] {
        let mut request = json!({ "Name": format!("no-zone-{case}"), "SiteId": ABSENT_SITE_ID });
        request[member] = json!(value);
        let reply = outposts
            .call_in("us-west-2", "POST", "/outposts", Some(&request))
            .await;
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                reply.errortype_header.as_str()
            ),
            (400, "ValidationException", "ValidationException"),
            "case {case:?}: a value naming no zone of the request's region is a bad \
             parameter, got {:?}",
            reply.body
        );
        let message = text(&reply.body, "Message");
        assert!(
            message.contains(value) && !message.contains("does not match your region"),
            "case {case:?}: the refusal must name {value:?} without calling it a \
             cross-region mismatch, got {message:?}"
        );
    }

    // With a zone of the request's own region, an absent site is still the site's own
    // error — the zone check narrows nothing else.
    let reply = outposts
        .call_in(
            "us-west-2",
            "POST",
            "/outposts",
            Some(&json!({
                "Name": "right zone, no site",
                "SiteId": ABSENT_SITE_ID,
                "AvailabilityZoneId": "usw2-az1",
            })),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "NotFoundException"),
        "a valid zone with an absent site is the site's not-found error, got {:?}",
        reply.body
    );
}
