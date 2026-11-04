// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` against every GuardDuty resource a tag ARN can name, end to
//! end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler are exercised
//! together. The resources come from the `resource_tag_removal` fixture, and the
//! writes from the `TagResource`/`UntagResource` helpers beside it, so the report and
//! the two writers are driven against the SAME set of resources.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. "When invoked, this operation returns all assigned tags for a given resource":
//!    the response's `tags` map carries every key the resource its `ResourceArn` names
//!    carries, with the value it carries, and nothing of any other resource's — so it
//!    agrees key for key with the `tags` member of that resource's own read.
//! 2. The ARN names the resource by its resource PATH, which is that resource's own
//!    REST URI: the detector, and each sub-resource segment
//!    `state::TAGGED_SUB_RESOURCE_SEGMENTS` names.
//! 3. The map reported is the one the writers left: what a create stored, what
//!    `TagResource` merged in, and what `UntagResource` took out.
//! 4. The `tags` member is ALWAYS reported, EMPTY for a resource nothing tagged, never
//!    an absent member.
//! 5. It is a REPORT: calling it changes nothing a later caller — itself or the
//!    resource's own read — sees.
//! 6. An ARN this endpoint resolves no resource for — another account's, another
//!    partition's, another Region's, an id nothing minted, a resource type GuardDuty
//!    does not tag — is refused with HTTP 400 under the `InvalidInputException` class,
//!    which names the member and never echoes the ARN.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, new_router, Router, ABSENT_DETECTOR_ID};
use crate::resource_tag_addition::tag_in;
use crate::resource_tag_removal::{
    arn_of, tag_map, tagged_resources, tags_of, untag_in, uri_label, Tagged,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::service::refusals::INVALID_RESOURCE_ARN_MESSAGE;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    TAGGED_SUB_RESOURCE_SEGMENTS, TAGS_PER_RESOURCE,
};

/// The Region every request in this suite is signed for, unless a case names another.
const REGION: &str = "us-east-1";

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed minted id (32 hexadecimal characters) that no create ever minted.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// The tags the resources of a case that is not about one particular map are created
/// with. The value of one of them is the empty string a `TagValue` (`len 0..256` —
/// `smithy_query(shape="TagMap")`) may be.
const BASE_TAGS: [(&str, &str); 3] = [("Env", "prod"), ("Team/Owner", "security"), ("Reviewed", "")];

/// `BASE_TAGS` as a request/response `TagMap`.
fn base_tags() -> Value {
    tag_map(&BASE_TAGS)
}

/// Report the tags of the resource `arn` names, through `region`'s endpoint.
///
/// `ListTagsForResource` is bound to `GET /tags/{ResourceArn}` with a NON-GREEDY URI
/// label, and `Tags` travels in the body under the `jsonName` `tags`
/// (`smithy_query(operation="ListTagsForResource")`).
async fn list_in(router: &Router, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", uri_label(arn)),
        None,
    )
    .await
}

/// The `tags` map `ListTagsForResource` reports for `arn`, asserting the operation
/// answered 200 and reported the member at all — `Value::Null` would be an absent
/// member, which rule 4 forbids.
async fn listed(router: &Router, region: &str, arn: &str, case: &str) -> Value {
    let (status, body) = list_in(router, region, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    let tags = body.get("tags").cloned().unwrap_or(Value::Null);
    assert!(
        tags.is_object(),
        "case {case:?}: the tags member is always reported, empty when nothing tagged \
         the resource, got {tags:?}"
    );
    tags
}

/// A key of the published maximum `TagKey` length (`len 1..128`).
fn longest_key() -> String {
    format!("k{}", "e".repeat(127))
}

/// A value of the published maximum `TagValue` length (`len 0..256`).
fn longest_value() -> String {
    "v".repeat(256)
}

// ---------------------------------------------------------------------------
// Rules 1 + 2: every key the resource carries, for every resource class.
// ---------------------------------------------------------------------------

/// The tag maps a resource is created with, and which the report must hand back
/// unchanged. The rows differ along everything the rule generalises over: the COUNT of
/// entries (one, several, a map at the per-resource ceiling), the key ALPHABET and
/// LENGTH (`len 1..128` with the punctuation `smithy_query(shape="TagMap")` admits,
/// both bounds included) and the value LENGTH (`len 0..256`, both bounds included).
fn listing_rows() -> Vec<(String, Vec<(String, String)>)> {
    let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
    let mut rows: Vec<(String, Vec<(String, String)>)> = vec![
        ("a single tag".to_string(), vec![pair("Env", "prod")]),
        (
            "three tags, one of them valued with the empty string".to_string(),
            BASE_TAGS
                .iter()
                .map(|(key, value)| pair(key, value))
                .collect(),
        ),
        (
            "a key using every character class the TagKey pattern admits".to_string(),
            vec![pair("a+b-c.d_e:f/g", "mixed")],
        ),
        (
            "a one-character key, the low end of the key's length".to_string(),
            vec![pair("k", "v")],
        ),
        (
            "a key and a value at their published maxima, 128 and 256".to_string(),
            vec![(longest_key(), longest_value())],
        ),
    ];

    // A map that fills the resource to the most tags it may carry
    // (`state::TAGS_PER_RESOURCE`).
    let at_the_ceiling: Vec<(String, String)> = (0..TAGS_PER_RESOURCE)
        .map(|index| (format!("filler-{index}"), format!("value-{index}")))
        .collect();
    rows.push((
        format!(
            "a map at the per-resource ceiling of {} tags",
            at_the_ceiling.len()
        ),
        at_the_ceiling,
    ));
    rows
}

#[tokio::test]
async fn listing_reports_every_tag_the_resource_carries() {
    for (row, tags) in listing_rows() {
        // A fresh store per row: each row creates one resource of every class, and
        // several of those are one per account and Region.
        let router = new_router();
        let pairs: Vec<(&str, &str)> = tags
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let expected = tag_map(&pairs);
        let (_, resources) = tagged_resources(&router, REGION, &expected).await;

        for resource in &resources {
            let case = format!("{row} on {}", resource.case);
            let actual = listed(&router, REGION, &resource.arn(REGION), &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the tags the create stored {expected:?}, got {actual:?}"
            );

            // The report and the resource's OWN read agree: there is one map, kept on
            // the resource's record.
            let own = tags_of(&router, REGION, resource, &case).await;
            assert_eq!(
                actual, own,
                "case {case:?}: the report must match the resource's own read {own:?}, got {actual:?}"
            );
        }
    }
}

/// Every sub-resource segment the service resolves is named by one of the ARNs the
/// fixture builds, so a segment nothing here reports on would fail this rather than
/// pass silently.
#[tokio::test]
async fn every_tagged_sub_resource_segment_is_reported_through_its_arn() {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    for (segment, _) in TAGGED_SUB_RESOURCE_SEGMENTS {
        let matching: Vec<&Tagged> = resources
            .iter()
            .filter(|resource| resource.path.contains(&format!("/{segment}/")))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {segment:?}: exactly one resource must be named through this segment, got {}",
            matching.len()
        );
        let case = format!("listing through the {segment:?} segment");
        let expected = base_tags();
        let actual = listed(&router, REGION, &matching[0].arn(REGION), &case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // The detector itself carries no sub-resource segment at all, and is reported the
    // same way.
    let detector = &resources[0];
    let expected = base_tags();
    let actual = listed(&router, REGION, &detector.arn(REGION), detector.case).await;
    assert_eq!(
        actual, expected,
        "case {:?}: expected {expected:?}, got {actual:?}",
        detector.case
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the report tracks the writers, and nothing else moves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_reports_the_map_tagging_and_untagging_left() {
    // How many classes a tag ARN names: the detector, plus one per sub-resource
    // segment the service resolves.
    let class_count = TAGGED_SUB_RESOURCE_SEGMENTS.len() + 1;

    for target in 0..class_count {
        // One store per target, so what the report shows is this target's writes.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        assert_eq!(
            resources.len(),
            class_count,
            "every class a tag ARN names must be created here, got {}",
            resources.len()
        );
        let arn = resources[target].arn(REGION);
        let case = format!("writing to {}", resources[target].case);

        // What the create stored.
        let actual = listed(&router, REGION, &arn, &case).await;
        assert_eq!(
            actual,
            base_tags(),
            "case {case:?}: expected the created tags, got {actual:?}"
        );

        // What `TagResource` merged in: a fresh key, and a new value for a carried one.
        let added = tag_map(&[("Owner", "payments"), ("Env", "staging")]);
        let (status, body) = tag_in(&router, REGION, &arn, &added).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the tag to answer 204, got {status} {body:?}"
        );
        let after_tag = json!({
            "Env": "staging",
            "Team/Owner": "security",
            "Reviewed": "",
            "Owner": "payments",
        });
        let actual = listed(&router, REGION, &arn, &case).await;
        assert_eq!(
            actual, after_tag,
            "case {case:?}: expected the merged tags {after_tag:?}, got {actual:?}"
        );

        // What `UntagResource` took out, including a key the resource never carried.
        let (status, body) = untag_in(&router, REGION, &arn, &["Env", "Absent"]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the untag to answer 204, got {status} {body:?}"
        );
        let after_untag = json!({
            "Team/Owner": "security",
            "Reviewed": "",
            "Owner": "payments",
        });
        let actual = listed(&router, REGION, &arn, &case).await;
        assert_eq!(
            actual, after_untag,
            "case {case:?}: expected the remaining tags {after_untag:?}, got {actual:?}"
        );

        // No other resource's report moved.
        let untouched = base_tags();
        for (index, resource) in resources.iter().enumerate() {
            if index == target {
                continue;
            }
            let other = format!("{case}, listing {}", resource.case);
            let actual = listed(&router, REGION, &resource.arn(REGION), &other).await;
            assert_eq!(
                actual, untouched,
                "case {other:?}: a write elsewhere must change nothing here, expected \
                 {untouched:?}, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an untagged resource reports an EMPTY map.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_untagged_resource_reports_an_empty_map() {
    // A detector created with no `tags` member at all.
    let router = new_router();
    let (status, body) = call_in(
        &router,
        REGION,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a create with no tags member\": expected 200, got {status} {body:?}"
    );
    let detector = body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a detectorId, got {body:?}"))
        .to_string();
    let case = "a detector created with no tags member at all";
    let actual = listed(
        &router,
        REGION,
        &arn_of(REGION, &format!("detector/{detector}")),
        case,
    )
    .await;
    assert_eq!(
        actual,
        json!({}),
        "case {case:?}: expected an empty map, got {actual:?}"
    );

    // And every class whose every key has been removed again.
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
    let every_key: Vec<&str> = BASE_TAGS.iter().map(|(key, _)| *key).collect();
    for resource in &resources {
        let case = format!("every key removed from {}", resource.case);
        let (status, body) = untag_in(&router, REGION, &resource.arn(REGION), &every_key).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the untag to answer 204, got {status} {body:?}"
        );
        let actual = listed(&router, REGION, &resource.arn(REGION), &case).await;
        assert_eq!(
            actual,
            json!({}),
            "case {case:?}: expected an empty map, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: it reports, it does not consume.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
    let expected = base_tags();

    for resource in &resources {
        let arn = resource.arn(REGION);
        for attempt in 1..=3 {
            let case = format!("listing {} attempt {attempt}", resource.case);
            let actual = listed(&router, REGION, &arn, &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: a report must hand back the same map every time, \
                 expected {expected:?}, got {actual:?}"
            );
        }
        // And the resource's own read still reports it too.
        let case = format!("reading {} after three reports", resource.case);
        let own = tags_of(&router, REGION, resource, &case).await;
        assert_eq!(
            own, expected,
            "case {case:?}: expected {expected:?}, got {own:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: an ARN this endpoint resolves nothing for is a classified HTTP 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_resource_arn_is_refused_without_echoing_it() {
    let router = new_router();
    let (detector, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    // Each row: the case, the Region whose endpoint is called, and the ARN.
    let rows: Vec<(String, &str, String)> = vec![
        (
            "a detectorId no create minted".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{ABSENT_DETECTOR_ID}")),
        ),
        (
            "an ipSetId the detector does not hold".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/ipset/{ABSENT_ID}")),
        ),
        (
            "a threatIntelSetId the detector does not hold".to_string(),
            REGION,
            arn_of(
                REGION,
                &format!("detector/{detector}/threatintelset/{ABSENT_ID}"),
            ),
        ),
        (
            "a trustedEntitySetId the detector does not hold".to_string(),
            REGION,
            arn_of(
                REGION,
                &format!("detector/{detector}/trustedentityset/{ABSENT_ID}"),
            ),
        ),
        (
            "a threatEntitySetId the detector does not hold".to_string(),
            REGION,
            arn_of(
                REGION,
                &format!("detector/{detector}/threatentityset/{ABSENT_ID}"),
            ),
        ),
        (
            "a filterName the detector does not hold".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/filter/absent-filter")),
        ),
        (
            "a destinationId the detector does not hold".to_string(),
            REGION,
            arn_of(
                REGION,
                &format!("detector/{detector}/publishingDestination/{ABSENT_ID}"),
            ),
        ),
        (
            "another account's detector".to_string(),
            REGION,
            format!("arn:aws:guardduty:{REGION}:111122223333:detector/{detector}"),
        ),
        (
            "another partition's detector".to_string(),
            REGION,
            format!("arn:aws-cn:guardduty:{REGION}:000000000000:detector/{detector}"),
        ),
        (
            "an ARN claiming another Region".to_string(),
            REGION,
            arn_of("eu-west-1", &format!("detector/{detector}")),
        ),
        (
            "this Region's ARN at another Region's endpoint".to_string(),
            "eu-west-1",
            arn_of(REGION, &format!("detector/{detector}")),
        ),
        (
            "a sub-resource type GuardDuty does not tag".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/widget/{ABSENT_ID}")),
        ),
    ];

    for (case, region, arn) in &rows {
        let (status, body) = list_in(&router, region, arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        let class = error_code(&body);
        assert_eq!(
            class, INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, INVALID_RESOURCE_ARN_MESSAGE,
            "case {case:?}: expected {INVALID_RESOURCE_ARN_MESSAGE:?}, got {message:?}"
        );
        for echoed in [arn.as_str(), detector.as_str()] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
    }

    // A refusal is not a write either: every resource still reports its own tags.
    let untouched = base_tags();
    for resource in &resources {
        let actual = listed(&router, REGION, &resource.arn(REGION), resource.case).await;
        assert_eq!(
            actual, untouched,
            "case {:?}: a refused report must change nothing, expected {untouched:?}, \
             got {actual:?}",
            resource.case
        );
    }
}

/// A request the model itself refuses never reaches the handler, and is answered with
/// HTTP 400 rather than a success: `ResourceArn` is a `GuardDutyArn` whose pattern
/// admits only this service's detector resource path
/// (`smithy_query(operation="ListTagsForResource")`).
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let router = new_router();

    let rows: Vec<(&str, String)> = vec![
        (
            "a Malware Protection plan ARN, which the pattern does not admit",
            arn_of(REGION, &format!("malware-protection-plan/{ABSENT_ID}")),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::guardduty-findings".to_string(),
        ),
        (
            "a detector path shorter than the pattern admits",
            arn_of(REGION, "detector/short"),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = list_in(&router, REGION, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
    }
}
