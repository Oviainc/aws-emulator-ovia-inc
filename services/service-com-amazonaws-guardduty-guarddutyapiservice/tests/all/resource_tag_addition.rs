// SPDX-License-Identifier: CC0-1.0

//! `TagResource` against every GuardDuty resource a tag ARN can name, end to end over
//! the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler are exercised
//! together. The resources themselves come from the `resource_tag_removal` fixture, so
//! the add and the remove are driven against the same set.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. "Adds tags to a resource": the request's map is MERGED into the map the resource
//!    carries, by key. A key the resource does not carry starts being carried, a key it
//!    already carries takes the request's value, a key the request does not name is
//!    left alone — so adding is idempotent, and the resource's OWN read is what reports
//!    the result.
//! 2. The ARN names the resource by its resource PATH, which is that resource's own
//!    REST URI: the detector, and each sub-resource segment
//!    `state::TAGGED_SUB_RESOURCE_SEGMENTS` names.
//! 3. Nothing else moves: the tags of every OTHER resource are untouched.
//! 4. An ARN this endpoint resolves no resource for — another account's, another
//!    partition's, another Region's, an id nothing minted, a resource type GuardDuty
//!    does not tag — is refused with HTTP 400 under the `InvalidInputException` class,
//!    which names the member and never echoes the ARN, and persists nothing.
//! 5. A resource carries at most `state::TAGS_PER_RESOURCE` tags: an add that would
//!    take it past that is refused with the same class and persists NONE of the
//!    request's tags, while an add that only overwrites keys it already carries is
//!    accepted at the ceiling.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, new_router, Router, ABSENT_DETECTOR_ID};
use crate::resource_tag_removal::{arn_of, tag_map, tagged_resources, tags_of, uri_label, Tagged};
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

/// The most entries one request's map may carry: `TagMap` is `len 1..200`
/// (`smithy_query(shape="TagMap")`), which is a different quantity from how many tags
/// one resource may hold (`state::TAGS_PER_RESOURCE`).
const TAG_MAP_MAX_ENTRIES: usize = 200;

/// The tags every resource in this suite is CREATED with, so each row only has to name
/// what it adds. The value of one of them is the empty string a `TagValue` (`len
/// 0..256` — `smithy_query(shape="TagMap")`) may be.
const BASE_TAGS: [(&str, &str); 2] = [("Env", "prod"), ("Reviewed", "")];

/// `BASE_TAGS` as a request/response `TagMap`.
fn base_tags() -> Value {
    tag_map(&BASE_TAGS)
}

/// The tags a resource created with `BASE_TAGS` carries once `added` has been merged
/// into it — the rule itself, so a row states only its own map.
fn merged_with(added: &[(&str, &str)]) -> Value {
    let mut pairs: Vec<(&str, &str)> = BASE_TAGS
        .iter()
        .copied()
        .filter(|(key, _)| !added.iter().any(|(name, _)| name == key))
        .collect();
    pairs.extend_from_slice(added);
    tag_map(&pairs)
}

/// Add `tags` to the resource `arn` names, through `region`'s endpoint.
///
/// `/tags/{ResourceArn}` binds a NON-GREEDY URI label and `Tags` travels in the body
/// under the `jsonName` `tags` (`smithy_query(operation="TagResource")`).
pub(crate) async fn tag_in(router: &Router, region: &str, arn: &str, tags: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/tags/{}", uri_label(arn)),
        Some(json!({"tags": tags})),
    )
    .await
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
// Rules 1 + 2: the named keys are added or overwritten, the rest stay, for every
// resource class.
// ---------------------------------------------------------------------------

/// The tags one `TagResource` call carries. The rows differ along everything the rule
/// generalises over: the COUNT added (one, several, a map that fills the resource to
/// its ceiling), WHICH keys (fresh, already carried, each character class a `TagKey`
/// admits, the longest one), the VALUE written (ordinary, empty, the longest one) and
/// whether the add overlaps what the resource already carries.
fn addition_rows() -> Vec<(String, Vec<(String, String)>)> {
    let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
    let mut rows: Vec<(String, Vec<(String, String)>)> = vec![
        ("one fresh key".to_string(), vec![pair("Owner", "payments")]),
        (
            "three fresh keys at once".to_string(),
            vec![
                pair("Owner", "payments"),
                pair("cost-center", "4711"),
                pair("Team/Owner", "security"),
            ],
        ),
        (
            "a fresh key whose value is the empty string".to_string(),
            vec![pair("Pending", "")],
        ),
        (
            "a key the resource already carries, with a new value".to_string(),
            vec![pair("Env", "staging")],
        ),
        (
            "a key the resource already carries, with the same value".to_string(),
            vec![pair("Env", "prod")],
        ),
        (
            "one carried key and one fresh key together".to_string(),
            vec![pair("Env", "staging"), pair("Owner", "payments")],
        ),
        (
            "a key using every character class the TagKey pattern admits".to_string(),
            vec![pair("a+b-c.d_e:f/g", "mixed")],
        ),
        (
            "a key at the published maximum length of 128".to_string(),
            vec![(longest_key(), "at-the-bound".to_string())],
        ),
        (
            "a value at the published maximum length of 256".to_string(),
            vec![pair("Description", &longest_value())],
        ),
    ];

    // A map that takes the resource exactly to the per-resource ceiling: the two keys
    // the create gave it, re-stated, plus fresh ones for the rest of it.
    let mut at_the_ceiling: Vec<(String, String)> = BASE_TAGS
        .iter()
        .map(|(key, value)| pair(key, value))
        .collect();
    at_the_ceiling.extend(
        (0..TAGS_PER_RESOURCE - BASE_TAGS.len())
            .map(|index| (format!("filler-{index}"), format!("value-{index}"))),
    );
    rows.push((
        format!(
            "a map that fills the resource to its ceiling of {} tags",
            at_the_ceiling.len()
        ),
        at_the_ceiling,
    ));
    rows
}

#[tokio::test]
async fn adding_tags_merges_into_the_map_the_resource_already_carries() {
    for (row, added) in addition_rows() {
        // Each row gets a store of its own, so what a read reports is this row's work.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        let pairs: Vec<(&str, &str)> = added
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let request = tag_map(&pairs);
        let expected = merged_with(&pairs);

        for resource in &resources {
            let case = format!("{row} on {}", resource.case);
            let (status, body) = tag_in(&router, REGION, &resource.arn(REGION), &request).await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected 204, got {status} {body:?}"
            );
            assert!(
                body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
                "case {case:?}: expected no response members, got {body:?}"
            );

            let actual = tags_of(&router, REGION, resource, &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the merged tags {expected:?}, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rules 1 + 3: idempotent, and no other resource moves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tagging_one_resource_is_idempotent_and_touches_no_other() {
    // How many classes a tag ARN names: the detector, plus one per sub-resource segment
    // the service resolves.
    let class_count = TAGGED_SUB_RESOURCE_SEGMENTS.len() + 1;

    for target in 0..class_count {
        // One store per target, so what the assertions see is this call's work.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        assert_eq!(
            resources.len(),
            class_count,
            "every class a tag ARN names must be created here, got {}",
            resources.len()
        );
        let case = format!("tagging {}", resources[target].case);
        let arn = resources[target].arn(REGION);
        let added = [("Owner", "payments"), ("Env", "staging")];
        let request = tag_map(&added);
        let expected = merged_with(&added);

        // The same request twice: the second changes nothing and is accepted.
        for attempt in 1..=2 {
            let (status, body) = tag_in(&router, REGION, &arn, &request).await;
            assert_eq!(
                status, 204,
                "case {case:?} attempt {attempt}: expected 204, got {status} {body:?}"
            );
            let actual = tags_of(&router, REGION, &resources[target], &case).await;
            assert_eq!(
                actual, expected,
                "case {case:?} attempt {attempt}: expected {expected:?}, got {actual:?}"
            );
        }

        // Every other resource still carries exactly what the create gave it.
        let untouched = base_tags();
        for (index, resource) in resources.iter().enumerate() {
            if index == target {
                continue;
            }
            let other = format!("{case}, reading {}", resource.case);
            let actual = tags_of(&router, REGION, resource, &other).await;
            assert_eq!(
                actual, untouched,
                "case {other:?}: a tag elsewhere must change nothing here, expected {untouched:?}, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an ARN this endpoint resolves nothing for is a classified HTTP 400.
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

    let request = tag_map(&[("Owner", "payments")]);
    for (case, region, arn) in &rows {
        let (status, body) = tag_in(&router, region, arn, &request).await;
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

    // Every refusal persisted nothing: each resource still carries just its two tags.
    let untouched = base_tags();
    for resource in &resources {
        let actual = tags_of(&router, REGION, resource, resource.case).await;
        assert_eq!(
            actual, untouched,
            "case {:?}: a refused tag must change nothing, expected {untouched:?}, got {actual:?}",
            resource.case
        );
    }
}

/// A request the model itself refuses never reaches the handler, and is answered with
/// HTTP 400 rather than a success: `ResourceArn` is a `GuardDutyArn` whose pattern
/// admits only this service's detector resource path, `Tags` is a `TagMap` (`len
/// 1..200`) and its values are `len 0..256`
/// (`smithy_query(operation="TagResource")`, `smithy_query(shape="TagMap")`). The map
/// is exercised one below its minimum and one past its maximum, and a value one past
/// its maximum; the map's minimum is accepted in
/// `adding_tags_merges_into_the_map_the_resource_already_carries`. How many tags one
/// RESOURCE may carry is a different quantity, pinned in the
/// `tags_per_resource_ceiling` suite.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let router = new_router();
    let (detector, resources) = tagged_resources(&router, REGION, &base_tags()).await;
    let detector_arn = arn_of(REGION, &format!("detector/{detector}"));

    // One past `TagMap`'s own `len 1..200`.
    let one_past_the_maximum: Vec<(String, String)> = (0..=TAG_MAP_MAX_ENTRIES)
        .map(|index| (format!("filler-{index}"), format!("value-{index}")))
        .collect();
    let too_many: Vec<(&str, &str)> = one_past_the_maximum
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let over_long_value = format!("{}x", longest_value());

    let rows: Vec<(String, String, Value)> = vec![
        (
            "an empty tag map, one below the minimum of 1".to_string(),
            detector_arn.clone(),
            json!({}),
        ),
        (
            format!("a tag map one past the maximum, with {} tags", too_many.len()),
            detector_arn.clone(),
            tag_map(&too_many),
        ),
        (
            "a value one character past the maximum length of 256".to_string(),
            detector_arn.clone(),
            tag_map(&[("Description", over_long_value.as_str())]),
        ),
        (
            "a Malware Protection plan ARN, which the pattern does not admit".to_string(),
            arn_of(REGION, &format!("malware-protection-plan/{ABSENT_ID}")),
            tag_map(&[("Owner", "payments")]),
        ),
        (
            "another service's ARN".to_string(),
            "arn:aws:s3:::guardduty-findings".to_string(),
            tag_map(&[("Owner", "payments")]),
        ),
        (
            "a detector path shorter than the pattern admits".to_string(),
            arn_of(REGION, "detector/short"),
            tag_map(&[("Owner", "payments")]),
        ),
    ];

    for (case, arn, tags) in rows {
        let (status, body) = tag_in(&router, REGION, &arn, &tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
    }

    // None of them persisted anything.
    let untouched = base_tags();
    for resource in &resources {
        let actual = tags_of(&router, REGION, resource, resource.case).await;
        assert_eq!(
            actual, untouched,
            "case {:?}: a refused tag must change nothing, expected {untouched:?}, got {actual:?}",
            resource.case
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: a resource carries at most TAGS_PER_RESOURCE tags.
// ---------------------------------------------------------------------------

/// The map that takes a resource created with `BASE_TAGS` exactly to the ceiling.
fn fill_to_the_ceiling() -> Vec<(String, String)> {
    (0..TAGS_PER_RESOURCE - BASE_TAGS.len())
        .map(|index| (format!("fill-{index}"), format!("value-{index}")))
        .collect()
}

#[tokio::test]
async fn an_add_past_the_tag_ceiling_is_refused_and_persists_nothing() {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    for resource in &resources {
        let case = resource.case;
        let arn = resource.arn(REGION);

        // At the ceiling: accepted, and every tag is readable back.
        let filled = fill_to_the_ceiling();
        let pairs: Vec<(&str, &str)> = filled
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&pairs)).await;
        assert_eq!(
            status, 204,
            "case {case:?}: an add up to the ceiling must be accepted, got {status} {body:?}"
        );
        let at_the_ceiling = merged_with(&pairs);
        let actual = tags_of(&router, REGION, resource, case).await;
        assert_eq!(
            actual, at_the_ceiling,
            "case {case:?}: expected {TAGS_PER_RESOURCE} tags, got {actual:?}"
        );

        // One fresh key past it: refused under the same class, naming the limit and
        // nothing of the caller's.
        let over = tag_map(&[("one-too-many", "refused"), ("also-refused", "refused")]);
        let (status, body) = tag_in(&router, REGION, &arn, &over).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an add past the ceiling must be refused, got {status} {body:?}"
        );
        let class = error_code(&body);
        assert_eq!(
            class, INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&TAGS_PER_RESOURCE.to_string()),
            "case {case:?}: the refusal must name the limit {TAGS_PER_RESOURCE}, got {message:?}"
        );
        assert!(
            !message.contains("one-too-many"),
            "case {case:?}: the refusal must not echo the caller's keys, got {message:?}"
        );

        // It persisted NOTHING — not even the key that would have fitted on its own.
        let actual = tags_of(&router, REGION, resource, case).await;
        assert_eq!(
            actual, at_the_ceiling,
            "case {case:?}: a refused add must leave the map as it was, got {actual:?}"
        );

        // An add that only overwrites keys the resource already carries grows it by
        // nothing, so it is accepted at the ceiling.
        let (status, body) = tag_in(&router, REGION, &arn, &tag_map(&[("Env", "staging")])).await;
        assert_eq!(
            status, 204,
            "case {case:?}: an overwrite at the ceiling must be accepted, got {status} {body:?}"
        );
        let mut overwritten = at_the_ceiling.clone();
        overwritten["Env"] = json!("staging");
        let actual = tags_of(&router, REGION, resource, case).await;
        assert_eq!(
            actual, overwritten,
            "case {case:?}: expected the overwritten value at the ceiling, got {actual:?}"
        );
    }
}

/// The ceiling is reached through `TagResource` only: every resource class is created
/// with the same two tags, so the fixture itself never starts one near the limit.
#[tokio::test]
async fn every_tagged_resource_class_starts_from_the_tags_its_create_stored() {
    let router = new_router();
    let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    let expected = base_tags();
    for resource in &resources {
        let actual = tags_of(&router, REGION, resource, resource.case).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected the created tags {expected:?}, got {actual:?}",
            resource.case
        );
    }

    // Every sub-resource segment the service resolves is named by one of those ARNs,
    // so a segment nothing here creates would fail this rather than pass silently.
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
    }
}
