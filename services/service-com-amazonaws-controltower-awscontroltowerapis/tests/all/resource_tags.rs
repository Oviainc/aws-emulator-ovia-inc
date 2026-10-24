// SPDX-License-Identifier: CC0-1.0

//! Resource tagging, end to end over HTTP: `TagResource` and `ListTagsForResource`.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees — status
//! codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * applying tags MERGES the supplied keys into the resource's tag set — a key already
//!   attached takes the new value, every other key is left alone — and what is persisted
//!   is what a later read reports, for any resource this service mints an ARN for and any
//!   tag map inside `TagMap`'s bounds;
//! * one resource's tags are its own: tagging one never changes another's;
//! * an `Arn` that names no resource of this service is `ResourceNotFoundException` (404)
//!   for both operations, and such a refusal persists nothing;
//! * reading is reporting: it neither creates an entry nor changes what the next read
//!   sees, and an untagged resource reports an EMPTY map rather than 404;
//! * a resource's tag map is BOUNDED at `MAX_TAGS_PER_RESOURCE`, and a request past that
//!   count is refused with the modelled `ValidationException` (400) and writes nothing.
//!
//! Every bound written out by hand here names its source. `TagMap` is `@length(0..200)`
//! with a `TagKey` of `@length(1..128)` and a `TagValue` of `@length(0..256)`, and both
//! operations bind `@http /tags/{resourceArn}` (POST 204 / GET 200) over an `Arn` of
//! `@length(20..2048)`, pattern `^arn:aws[0-9a-zA-Z_\-:\/]+$` — from
//! `smithy_query(query="operation", operation="TagResource,ListTagsForResource")` and
//! `smithy_query(query="shape", shape="TagMap,TagKey,TagValue,Arn")` on this crate's
//! `smithy-model/`.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{call, dispatch, emulator, past_the_settle_span, Answer};

/// The request path both tagging operations bind: `/tags/{resourceArn}`.
///
/// The label is NON-greedy, so the ARN is a single path segment and its `/` separators
/// travel percent-encoded; the front door decodes the segment before the handler sees it.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// `TagResource`: POST the tag map for `arn`.
async fn apply_tags(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> Answer {
    let map: serde_json::Map<String, serde_json::Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect();
    dispatch(
        router,
        "POST",
        &tags_path(arn),
        Some(serde_json::json!({ "tags": map })),
    )
    .await
}

/// `ListTagsForResource`: GET the tags of `arn`. The operation carries no body.
async fn read_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    dispatch(router, "GET", &tags_path(arn), None).await
}

/// The `tags` member of a `ListTagsForResource` answer, as a key-ordered map so a case
/// compares SETS rather than JSON object order. An absent member is the empty map:
/// restJson1 may omit an empty map rather than emit `{}`, and both say "no tags".
fn reported_tags(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .get("tags")
        .as_object()
        .map(|members| {
            members
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A tag map from `(key, value)` pairs, for the tables below.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the resource table: a resource kind this service mints ARNs for, the ARN
/// one call minted, and the tags that call created it with.
struct Resource {
    case: &'static str,
    arn: String,
    created_with: BTreeMap<String, String>,
}

/// Every resource kind this service tags, created on `router`: a landing zone (created
/// WITH tags, so the merge starts from a non-empty set), an enabled control (created with
/// no `tags` member at all) and an enabled baseline (created with an EMPTY tag map).
///
/// The account holds at most one landing zone, so there is one of those per router.
async fn resources(router: &MultiServiceRouter) -> Vec<Resource> {
    let created_with = tags(&[("Owner", "platform")]);
    let landing_zone = call(
        router,
        "/create-landingzone",
        serde_json::json!({
            "version": "3.3",
            "manifest": { "governedRegions": ["us-west-2"] },
            "tags": { "Owner": "platform" },
        }),
    )
    .await;
    assert_eq!(
        landing_zone.status, 200,
        "create-landingzone: expected 200, got {} body {:?}",
        landing_zone.status, landing_zone.body
    );

    let control = call(
        router,
        "/enable-control",
        serde_json::json!({
            "controlIdentifier": "arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES",
            "targetIdentifier": "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-1a",
        }),
    )
    .await;
    assert_eq!(
        control.status, 200,
        "enable-control: expected 200, got {} body {:?}",
        control.status, control.body
    );

    let baseline = call(
        router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
            "baselineVersion": "4.0",
            "targetIdentifier": "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-2b",
            "tags": {},
        }),
    )
    .await;
    assert_eq!(
        baseline.status, 200,
        "enable-baseline: expected 200, got {} body {:?}",
        baseline.status, baseline.body
    );

    vec![
        Resource {
            case: "a landing zone, created with one tag",
            arn: landing_zone.string("arn"),
            created_with,
        },
        Resource {
            case: "an enabled control, created with no tags member",
            arn: control.string("arn"),
            created_with: BTreeMap::new(),
        },
        Resource {
            case: "an enabled baseline, created with an empty tag map",
            arn: baseline.string("arn"),
            created_with: BTreeMap::new(),
        },
    ]
}

/// One row of the application table: the tag map one `TagResource` call carries.
struct Application {
    case: &'static str,
    tags: BTreeMap<String, String>,
}

/// The application table. Rows differ along what the rule generalises over: the number of
/// keys (none, one, several), the key and value forms at `TagKey`/`TagValue`'s published
/// length bounds, a key ALREADY attached (the overwrite half of the merge) and a key that
/// is not (the add half).
fn applications() -> Vec<Application> {
    // `TagKey` is @length(min 1, max 128); `TagValue` is @length(min 0, max 256).
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        Application {
            case: "an empty tag map",
            tags: BTreeMap::new(),
        },
        Application {
            case: "one tag at both length minima: a 1-character key and an empty value",
            tags: tags(&[("K", "")]),
        },
        Application {
            case: "a key and a value at their length maxima",
            tags: tags(&[(longest_key.as_str(), longest_value.as_str())]),
        },
        Application {
            case: "several tags at once",
            tags: tags(&[("Team", "platform"), ("Cost-Centre", "4711"), ("Env", "prod")]),
        },
        Application {
            case: "a key already attached, with a different value",
            tags: tags(&[("Team", "governance")]),
        },
        Application {
            case: "a key that is not attached yet, beside the ones that are",
            tags: tags(&[("Owner", "cloud-ops")]),
        },
    ]
}

/// RULE: applying tags merges the supplied keys into the resource's tag set and what is
/// persisted is what a later read reports — for every resource kind and every tag map
/// inside `TagMap`'s bounds — and one resource's tags are its own.
#[tokio::test]
async fn applying_tags_merges_them_and_a_read_reports_what_was_persisted() {
    let router = emulator();
    let resources = resources(&router).await;

    // What each resource is expected to carry, grown row by row as the merge proceeds.
    let mut expected: Vec<BTreeMap<String, String>> = Vec::new();

    for resource in &resources {
        // The create's own `tags` member lands in the same store, so a read reports it
        // before any `TagResource` call.
        let initial = read_tags(&router, &resource.arn).await;
        assert_eq!(
            initial.status, 200,
            "case {:?}: expected the resource's tags to be readable, got {} body {:?}",
            resource.case, initial.status, initial.body
        );
        assert_eq!(
            reported_tags(&initial),
            resource.created_with,
            "case {:?}: expected the tags the create carried, got {:?}",
            resource.case,
            reported_tags(&initial)
        );
        expected.push(resource.created_with.clone());
    }

    for row in applications() {
        for (index, resource) in resources.iter().enumerate() {
            let applied = apply_tags(&router, &resource.arn, &row.tags).await;
            assert_eq!(
                applied.status, 204,
                "case {:?} on {:?}: expected 204, got {} body {:?}",
                row.case, resource.case, applied.status, applied.body
            );
            assert!(
                applied
                    .body
                    .as_object()
                    .is_none_or(serde_json::Map::is_empty),
                "case {:?} on {:?}: a 204 answer carries no members, got {:?}",
                row.case,
                resource.case,
                applied.body
            );

            // The merge: the row's keys take their new values, every other key stays.
            expected[index].extend(row.tags.clone());

            let read = read_tags(&router, &resource.arn).await;
            assert_eq!(
                read.status, 200,
                "case {:?} on {:?}: expected 200, got {} body {:?}",
                row.case, resource.case, read.status, read.body
            );
            assert_eq!(
                reported_tags(&read),
                expected[index],
                "case {:?} on {:?}: expected the merged tag set {:?}, got {:?}",
                row.case,
                resource.case,
                expected[index],
                reported_tags(&read)
            );
        }
    }

    // One resource's tags are its own: every resource still reports exactly its own set
    // after every other one was tagged.
    for (index, resource) in resources.iter().enumerate() {
        let read = read_tags(&router, &resource.arn).await;
        assert_eq!(
            reported_tags(&read),
            expected[index],
            "case {:?}: tagging the other resources must not change this one, expected \
             {:?}, got {:?}",
            resource.case,
            expected[index],
            reported_tags(&read)
        );
    }
}

/// RULE (the other side of it): reading is reporting. A read neither creates a tag entry
/// nor changes what the next read sees, and an untagged resource is an EMPTY map rather
/// than a 404.
#[tokio::test]
async fn reading_tags_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let resources = resources(&router).await;

    for resource in &resources {
        let first = read_tags(&router, &resource.arn).await;
        let second = read_tags(&router, &resource.arn).await;
        assert_eq!(
            (first.status, second.status),
            (200, 200),
            "case {:?}: expected both reads to answer 200, got {:?} and {:?}",
            resource.case,
            first.status,
            second.status
        );
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {:?}: two reads must report the same tags, got {:?} then {:?}",
            resource.case,
            reported_tags(&first),
            reported_tags(&second)
        );
        assert_eq!(
            reported_tags(&second),
            resource.created_with,
            "case {:?}: reading must not invent tags, expected {:?}, got {:?}",
            resource.case,
            resource.created_with,
            reported_tags(&second)
        );
    }

    // And a tag applied after those reads is still the only change the store took.
    let subject = &resources[1];
    let applied = apply_tags(&router, &subject.arn, &tags(&[("Env", "staging")])).await;
    assert_eq!(
        applied.status, 204,
        "tagging after the reads: expected 204, got {} body {:?}",
        applied.status, applied.body
    );
    let read = read_tags(&router, &subject.arn).await;
    assert_eq!(
        reported_tags(&read),
        tags(&[("Env", "staging")]),
        "case {:?}: expected only the tag that was applied, got {:?}",
        subject.case,
        reported_tags(&read)
    );
}

/// RULE: an `Arn` that names no resource of this service is `ResourceNotFoundException`
/// (404) for both operations — whether or not the account holds other resources — and the
/// refused `TagResource` persists nothing.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_not_found() {
    let router = emulator();
    let resources = resources(&router).await;

    // A resource whose ARN this service DID mint and then tore down: the tags went with
    // it, so the ARN names nothing again. The landing zone's create operation must be due
    // before it can be deleted.
    tokio::time::sleep(past_the_settle_span()).await;
    let deleted = call(
        &router,
        "/delete-landingzone",
        serde_json::json!({ "landingZoneIdentifier": resources[0].arn }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "delete-landingzone: expected 200, got {} body {:?}",
        deleted.status, deleted.body
    );

    // Every value satisfies `Arn`'s @length(20..2048) and
    // @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$, so the front door passes them and the
    // handler is what refuses them.
    let cases: Vec<(&str, String)> = vec![
        (
            "a landing zone ARN nothing minted",
            "arn:aws:controltower:us-west-2:000000000000:landingzone/\
             00000000-0000-0000-0000-000000000000"
                .to_string(),
        ),
        (
            "an enabled control ARN nothing minted",
            "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/\
             deadbeef-dead-beef-dead-beefdeadbeef"
                .to_string(),
        ),
        (
            "an enabled baseline ARN nothing minted",
            "arn:aws:controltower:us-west-2:000000000000:enabledbaseline/\
             deadbeef-dead-beef-dead-beefdeadbeef"
                .to_string(),
        ),
        (
            "an ARN of exactly the length min of 20",
            format!("arn:aws{}", "a".repeat(13)),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::emulator-tagging-bucket".to_string(),
        ),
        (
            "the landing zone's own ARN, after it was deleted",
            resources[0].arn.clone(),
        ),
    ];

    for (case, arn) in cases {
        let applied = apply_tags(&router, &arn, &tags(&[("Team", "platform")])).await;
        assert_eq!(
            (applied.status, applied.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected TagResource to answer 404 ResourceNotFoundException, \
             got {:?} body {:?}",
            (applied.status, applied.error_code.as_deref()),
            applied.body
        );

        let read = read_tags(&router, &arn).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected ListTagsForResource to answer 404 \
             ResourceNotFoundException — a refused tagging persists nothing, so the ARN \
             must not become an empty tag set — got {:?} body {:?}",
            (read.status, read.error_code.as_deref()),
            read.body
        );
    }

    // The resources that DO exist are untouched by the refusals.
    for resource in &resources[1..] {
        let read = read_tags(&router, &resource.arn).await;
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, resource.created_with.clone()),
            "case {:?}: a refused tagging of another ARN must not change this resource, \
             got {:?} body {:?}",
            resource.case,
            read.status,
            read.body
        );
    }
}

/// RULE: a resource's tag map is BOUNDED at `MAX_TAGS_PER_RESOURCE` — `TagMap`'s own
/// `@length` max, beyond which a read could not report the result inside the model's
/// constraint. A request that would take it past that count is refused with the modelled
/// `ValidationException` (400) and writes nothing, while one that only overwrites keys
/// already attached is accepted at the cap.
#[tokio::test]
async fn a_resources_tag_map_is_bounded() {
    use service_com_amazonaws_controltower_awscontroltowerapis::state::MAX_TAGS_PER_RESOURCE;

    let router = emulator();
    let resources = resources(&router).await;
    // The enabled control starts untagged, so the cap is reached by this test's own
    // writes alone.
    let subject = &resources[1];

    let full: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("Key{index:04}"), format!("value-{index}")))
        .collect();
    let filled = apply_tags(&router, &subject.arn, &full).await;
    assert_eq!(
        filled.status, 204,
        "filling the tag map to its cap of {MAX_TAGS_PER_RESOURCE}: expected 204, got {} \
         body {:?}",
        filled.status, filled.body
    );
    let read = read_tags(&router, &subject.arn).await;
    assert_eq!(
        reported_tags(&read).len(),
        MAX_TAGS_PER_RESOURCE,
        "at the cap: expected {MAX_TAGS_PER_RESOURCE} tags, got {} body {:?}",
        reported_tags(&read).len(),
        read.body
    );

    // One key past the cap is refused, and nothing is written: the set a read reports is
    // exactly the one from before the refusal.
    let over = apply_tags(&router, &subject.arn, &tags(&[("OneTooMany", "x")])).await;
    assert_eq!(
        (over.status, over.error_code.as_deref()),
        (400, Some("ValidationException")),
        "one key past the cap of {MAX_TAGS_PER_RESOURCE}: expected 400 \
         ValidationException, got {:?} body {:?}",
        (over.status, over.error_code.as_deref()),
        over.body
    );
    let after = read_tags(&router, &subject.arn).await;
    assert_eq!(
        reported_tags(&after),
        reported_tags(&read),
        "a refused tagging must write nothing: expected the tag set from before it, got \
         {:?}",
        reported_tags(&after)
    );

    // The cap bounds the tag COUNT, not the calls: overwriting a key already attached is
    // accepted at the cap, because the merged set is no larger.
    let overwrite = apply_tags(&router, &subject.arn, &tags(&[("Key0000", "overwritten")])).await;
    assert_eq!(
        overwrite.status, 204,
        "overwriting a key already attached while at the cap: expected 204, got {} body \
         {:?}",
        overwrite.status, overwrite.body
    );
    let overwritten = read_tags(&router, &subject.arn).await;
    assert_eq!(
        reported_tags(&overwritten).len(),
        MAX_TAGS_PER_RESOURCE,
        "after an overwrite at the cap: expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        reported_tags(&overwritten).len()
    );
    assert_eq!(
        reported_tags(&overwritten).get("Key0000").map(String::as_str),
        Some("overwritten"),
        "after an overwrite at the cap: expected the new value, got {:?}",
        reported_tags(&overwritten).get("Key0000")
    );
}
