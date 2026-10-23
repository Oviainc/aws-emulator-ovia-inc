// SPDX-License-Identifier: CC0-1.0

//! Comprehend's tagging facility: `TagResource`, `ListTagsForResource` and
//! `UntagResource`.
//!
//! RULE: every Comprehend resource carries ONE key-value tag map, held on the record
//! the request's `ResourceArn` names. `TagResource` writes a key's value — replacing
//! the value of a key the resource already holds, adding a key it does not —
//! `ListTagsForResource` reports exactly the map the resource holds (the tags its
//! creating operation's `Tags` persisted as much as the ones a later `TagResource`
//! associated), and `UntagResource` removes exactly the keys `TagKeys` names. A
//! `ResourceArn` that names no resource this service holds is refused by all three
//! with `ResourceNotFoundException` (HTTP 404). The map is bounded at 50 tags per
//! resource counting BOTH the tags a resource already holds and the ones the request
//! brings (`TooManyTagsException`, HTTP 400), and a removal naming more than 50 tag
//! keys is refused with `TooManyTagKeysException` (HTTP 400).
//!
//! Each suite asserts that over a SET of inputs: the rows differ along the kind of
//! resource tagged (a job of each implemented `Start*DetectionJob` family, a document
//! classifier, a classifier version), along the tags themselves (a value at the
//! modelled bound, a tag with no value at all, keys already held versus new ones, how
//! many), and along the keys a removal names. The recorded unit's single nonexistent
//! resource ARN is ONE row of the not-found table.
//!
//! Because the map is keyed by the tag key, the order the tags were associated in is
//! not part of it: a set reached by any order of requests is reported as the same
//! key-ordered list, and reporting it leaves it alone. The rows of
//! [`the_reported_map_is_one_key_order_whatever_order_the_tags_were_associated_in`]
//! differ along exactly that — the order, the number of requests, and the number of
//! tags.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// A resource's tag map as these suites compare it: `(key, value)` pairs in key
/// order, with `None` for a `Tag` carrying no `Value` member (which `Tag$Value`
/// models as optional).
type TagMap = Vec<(String, Option<String>)>;

/// The most tags a Comprehend resource can hold, as `TooManyTagsException` states:
/// "The request contains more tags than can be associated with a resource (50 tags
/// per resource). The maximum number of tags includes both existing tags and those
/// included in your current request."
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The one sentence Comprehend answers for a resource ARN it holds no resource
/// under, spelled out here rather than read from the crate, so the assertion is
/// independent of the constant the handlers carry.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "RESOURCE_NOT_FOUND: Could not find specified resource.";

/// One kind of resource this emulator persists, and therefore one kind of resource
/// that can be tagged: the call that creates one, and the output member carrying the
/// ARN that names it.
struct ResourceKind {
    case: &'static str,
    create: &'static str,
    arn_member: &'static str,
    /// The creation body, built around `label` so no two rows collide in one store.
    /// Its `Tags` — present on some rows, absent on others — are the resource's
    /// first tags.
    body: fn(&str) -> Value,
}

/// Every kind of resource this service holds records for. The rule holds for each.
///
/// A resource type whose creating operation is still a scaffold stub (an endpoint, a
/// flywheel, a dataset, an entity recognizer) has no row here because the store can
/// hold none of them; its ARN is a row of [`absent_arns`] instead.
const RESOURCE_KINDS: &[ResourceKind] = &[
    ResourceKind {
        case: "sentiment detection job, created carrying two tags",
        create: "StartSentimentDetectionJob",
        arn_member: "JobArn",
        body: sentiment_submission,
    },
    ResourceKind {
        case: "key-phrases detection job, created carrying no tags at all",
        create: "StartKeyPhrasesDetectionJob",
        arn_member: "JobArn",
        body: key_phrases_submission,
    },
    ResourceKind {
        case: "PII entities detection job, created carrying one tag",
        create: "StartPiiEntitiesDetectionJob",
        arn_member: "JobArn",
        body: pii_submission,
    },
    ResourceKind {
        case: "document classifier, created carrying no tags at all",
        create: "CreateDocumentClassifier",
        arn_member: "DocumentClassifierArn",
        body: classifier_creation,
    },
    ResourceKind {
        case: "document classifier version, created carrying one tag",
        create: "CreateDocumentClassifier",
        arn_member: "DocumentClassifierArn",
        body: classifier_version_creation,
    },
];

/// A `StartSentimentDetectionJob` body whose submission carries two tags, one of them
/// with no `Value` at all.
fn sentiment_submission(label: &str) -> Value {
    json!({
        "JobName": format!("{label}-sentiment"),
        "LanguageCode": "en",
        "InputDataConfig": { "S3Uri": "s3://bucket/in/" },
        "OutputDataConfig": { "S3Uri": "s3://bucket/out/" },
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        "Tags": [{ "Key": "team", "Value": "research" }, { "Key": "cost-centre" }],
    })
}

/// A `StartKeyPhrasesDetectionJob` body naming no `Tags` member at all.
fn key_phrases_submission(label: &str) -> Value {
    json!({
        "JobName": format!("{label}-key-phrases"),
        "LanguageCode": "de",
        "InputDataConfig": { "S3Uri": "s3://other-bucket/in/" },
        "OutputDataConfig": { "S3Uri": "s3://other-bucket/out/" },
        "DataAccessRoleArn": "arn:aws:iam::210987654321:role/path/to/OtherRole",
    })
}

/// A `StartPiiEntitiesDetectionJob` body carrying one tag.
fn pii_submission(label: &str) -> Value {
    json!({
        "JobName": format!("{label}-pii"),
        "LanguageCode": "en",
        "Mode": "ONLY_OFFSETS",
        "InputDataConfig": { "S3Uri": "s3://bucket/in/", "InputFormat": "ONE_DOC_PER_LINE" },
        "OutputDataConfig": { "S3Uri": "s3://bucket/out/" },
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        "Tags": [{ "Key": "team", "Value": "nlp" }],
    })
}

/// A `CreateDocumentClassifier` body naming no version and no `Tags`.
fn classifier_creation(label: &str) -> Value {
    json!({
        "DocumentClassifierName": format!("{label}-classifier"),
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        "LanguageCode": "en",
        "InputDataConfig": { "S3Uri": "s3://bucket/training/" },
    })
}

/// A `CreateDocumentClassifier` body naming a version, carrying one tag whose value
/// is AT the modelled `TagValue` bound (`len 0..256`).
fn classifier_version_creation(label: &str) -> Value {
    let mut body = classifier_creation(&format!("{label}-versioned"));
    body["VersionName"] = json!("v1");
    body["Tags"] = json!([{ "Key": "release", "Value": "x".repeat(256) }]);
    body
}

/// Create one resource of `kind`, returning the ARN that names it and the tags its
/// creation persisted.
async fn create(comprehend: &Comprehend, kind: &ResourceKind, label: &str) -> (String, TagMap) {
    let body = (kind.body)(label);
    let output = comprehend.ok(kind.create, &body).await;
    let arn = output[kind.arn_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: {} reported no {}: {output}",
                kind.case, kind.create, kind.arn_member
            )
        })
        .to_string();
    (arn, tag_pairs(body.get("Tags")))
}

/// The `(key, value)` pairs of a modelled `TagList` value, in the order given.
fn tag_pairs(tags: Option<&Value>) -> TagMap {
    tags.and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or("<no Key>").to_string(),
                        tag.get("Value").and_then(Value::as_str).map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tags `ListTagsForResource` reports for `arn`, in the order served, once it has
/// echoed the ARN it was asked about.
async fn list_tags(comprehend: &Comprehend, arn: &str, case: &str) -> TagMap {
    let output = comprehend
        .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    assert_eq!(
        output["ResourceArn"].as_str(),
        Some(arn),
        "case {case:?}: ListTagsForResource must report the ResourceArn it was asked \
         about, got {output}"
    );
    tag_pairs(output.get("Tags"))
}

/// The map a resource holding `held` holds once `incoming` is associated with it, by
/// the rule: a key already held takes the new value, a key not held is added. In key
/// order, which is the order `ListTagsForResource` serves a tag set in.
fn merged(held: &TagMap, incoming: &Value) -> TagMap {
    let mut map: BTreeMap<String, Option<String>> = held.iter().cloned().collect();
    for (key, value) in tag_pairs(Some(incoming)) {
        map.insert(key, value);
    }
    map.into_iter().collect()
}

/// A `TagList` of `count` distinct tags keyed `<prefix>0..<prefix>count`.
fn tag_list(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("{prefix}{index}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// ARNs matching the modelled `ComprehendArn` pattern that name no resource this
/// store holds: job ids nothing minted, a classifier and a classifier version never
/// created, and resource types whose creating operation is still a scaffold stub, so
/// no store can hold one.
fn absent_arns() -> Vec<(&'static str, String)> {
    let arn = |resource: &str| format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:{resource}");
    vec![
        (
            "a sentiment job id nothing minted",
            arn("sentiment-detection-job/0123456789abcdef0123456789abcdef"),
        ),
        (
            "a key-phrases job id nothing minted",
            arn("key-phrases-detection-job/ffffffffffffffffffffffffffffffff"),
        ),
        (
            "a document classifier that was never created",
            arn("document-classifier/absent-classifier"),
        ),
        (
            "a classifier version that was never created",
            arn("document-classifier/absent-classifier/version/v1"),
        ),
        (
            "an endpoint, whose creating operation is still a stub",
            arn("inference-endpoint/absent-endpoint"),
        ),
        (
            "a flywheel dataset, whose creating operation is still a stub",
            arn("flywheel/absent-flywheel/dataset/absent-dataset"),
        ),
    ]
}

#[tokio::test]
async fn a_tag_map_round_trips_through_every_kind_of_resource_this_service_holds() {
    // What every row associates after creation: one key the row may already hold (a
    // replacement where it does, an addition where it does not), one new key, and one
    // key with no value at all.
    let incoming = json!([
        { "Key": "team", "Value": "replaced" },
        { "Key": "env", "Value": "staging" },
        { "Key": "no-value" },
    ]);

    for kind in RESOURCE_KINDS {
        let case = kind.case;
        let comprehend = Comprehend::new();
        let (arn, created) = create(&comprehend, kind, "roundtrip").await;

        // The tags the creation persisted are the tags the read reports.
        let reported = list_tags(&comprehend, &arn, case).await;
        let expected = merged(&created, &json!([]));
        assert_eq!(
            reported, expected,
            "case {case:?}: ListTagsForResource must report the tags the creation \
             persisted ({expected:?}), got {reported:?}"
        );

        comprehend
            .ok(
                "TagResource",
                &json!({ "ResourceArn": &arn, "Tags": &incoming }),
            )
            .await;
        let after_tag = list_tags(&comprehend, &arn, case).await;
        let expected = merged(&created, &incoming);
        assert_eq!(
            after_tag, expected,
            "case {case:?}: after TagResource the resource must hold {expected:?}, \
             got {after_tag:?}"
        );

        // The map is keyed by the tag key, so associating the same tags again leaves
        // the same map.
        comprehend
            .ok(
                "TagResource",
                &json!({ "ResourceArn": &arn, "Tags": &incoming }),
            )
            .await;
        let after_repeat = list_tags(&comprehend, &arn, case).await;
        assert_eq!(
            after_repeat, after_tag,
            "case {case:?}: a repeated TagResource must leave the map unchanged \
             ({after_tag:?}), got {after_repeat:?}"
        );

        // A removal takes exactly the keys it names; a key the resource does not hold
        // removes nothing.
        comprehend
            .ok(
                "UntagResource",
                &json!({ "ResourceArn": &arn, "TagKeys": ["team", "absent-key"] }),
            )
            .await;
        let after_untag = list_tags(&comprehend, &arn, case).await;
        let expected: TagMap = after_tag
            .iter()
            .filter(|(key, _)| key != "team")
            .cloned()
            .collect();
        assert_eq!(
            after_untag, expected,
            "case {case:?}: after UntagResource the resource must hold {expected:?}, \
             got {after_untag:?}"
        );
    }
}

#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_naming_no_resource() {
    let comprehend = Comprehend::new();
    // One real resource, so a refusal is about the ARN and not about an empty store.
    let (held, held_tags) = create(&comprehend, &RESOURCE_KINDS[0], "present").await;

    for (case, arn) in absent_arns() {
        for (operation, body) in [
            (
                "TagResource",
                json!({ "ResourceArn": &arn, "Tags": [{ "Key": "team", "Value": "research" }] }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": &arn, "TagKeys": ["team"] }),
            ),
            ("ListTagsForResource", json!({ "ResourceArn": &arn })),
        ] {
            let (status, response) = comprehend.call(operation, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?} / {operation}: expected 404, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?} / {operation}: expected ResourceNotFoundException, \
                 got {response}"
            );
            // One sentence for every unknown ARN, whichever operation was called and
            // whatever the ARN named: the refusal reports that no such resource is
            // held, and echoes nothing back from the request.
            assert_eq!(
                error_message(&response),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?} / {operation}: expected the one not-found sentence, \
                 got {response}"
            );
        }
    }

    // None of those refusals touched the resource that does exist.
    let reported = list_tags(&comprehend, &held, "an ARN that does name a resource").await;
    let expected = merged(&held_tags, &json!([]));
    assert_eq!(
        reported, expected,
        "case \"an ARN that does name a resource\": the refusals must leave it holding \
         {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn the_tag_bound_counts_the_tags_the_resource_already_holds() {
    // Rows: the bound reached from both sides, one past it from each, and a request
    // whose keys the resource already holds — which adds nothing to the count, so it
    // is accepted at the bound.
    let rows: &[(&str, Value, Value, bool)] = &[
        ("no tags on either side", tag_list("h", 0), tag_list("t", 0), true),
        ("one tag onto none", tag_list("h", 0), tag_list("t", 1), true),
        ("one short of the bound", tag_list("h", 10), tag_list("t", 39), true),
        ("exactly at the bound", tag_list("h", 10), tag_list("t", 40), true),
        ("one past the bound", tag_list("h", 10), tag_list("t", 41), false),
        ("the bound reached by the request alone", tag_list("h", 0), tag_list("t", 50), true),
        (
            "one past the bound by the request alone",
            tag_list("h", 0),
            tag_list("t", 51),
            false,
        ),
        (
            "every key already held, at the bound",
            tag_list("h", 50),
            tag_list("h", 50),
            true,
        ),
    ];

    for (case, held, incoming, accepted) in rows {
        let comprehend = Comprehend::new();
        let (arn, _) = create(&comprehend, &RESOURCE_KINDS[1], "bound").await;
        if !tag_pairs(Some(held)).is_empty() {
            comprehend
                .ok("TagResource", &json!({ "ResourceArn": &arn, "Tags": held }))
                .await;
        }
        let before = list_tags(&comprehend, &arn, case).await;

        let (status, response) = comprehend
            .call(
                "TagResource",
                &json!({ "ResourceArn": &arn, "Tags": incoming }),
            )
            .await;
        let reported = list_tags(&comprehend, &arn, case).await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
            let expected = merged(&before, incoming);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the resource to hold {} tags, got {}",
                expected.len(),
                reported.len()
            );
            assert!(
                reported.len() <= MAX_TAGS_PER_RESOURCE,
                "case {case:?}: an accepted request must leave the resource within the \
                 bound, got {} tags",
                reported.len()
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TooManyTagsException",
                "case {case:?}: expected TooManyTagsException, got {response}"
            );
            assert_eq!(
                reported, before,
                "case {case:?}: a refused request must change nothing, expected \
                 {before:?}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_removal_naming_more_tag_keys_than_a_resource_can_hold_is_refused() {
    // `TooManyTagKeysException`: "The request contains more tag keys than can be
    // associated with a resource (50 tag keys per resource)." At the bound, one past
    // it, one key, and the empty list, which names nothing to remove.
    let held = 3usize;
    for count in [0usize, 1, MAX_TAGS_PER_RESOURCE, MAX_TAGS_PER_RESOURCE + 1] {
        let case = format!("{count} tag keys");
        let comprehend = Comprehend::new();
        let (arn, _) = create(&comprehend, &RESOURCE_KINDS[3], "keybound").await;
        comprehend
            .ok(
                "TagResource",
                &json!({ "ResourceArn": &arn, "Tags": tag_list("k", held) }),
            )
            .await;

        let keys: Vec<String> = (0..count).map(|index| format!("k{index}")).collect();
        let (status, response) = comprehend
            .call(
                "UntagResource",
                &json!({ "ResourceArn": &arn, "TagKeys": keys }),
            )
            .await;
        let remaining = list_tags(&comprehend, &arn, &case).await;

        if count > MAX_TAGS_PER_RESOURCE {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TooManyTagKeysException",
                "case {case:?}: expected TooManyTagKeysException, got {response}"
            );
            assert_eq!(
                remaining.len(),
                held,
                "case {case:?}: a refused removal must remove nothing, got {remaining:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
            let expected = held - count.min(held);
            assert_eq!(
                remaining.len(),
                expected,
                "case {case:?}: expected {expected} tags left, got {remaining:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_reported_map_is_one_key_order_whatever_order_the_tags_were_associated_in() {
    // Rows: the SAME three-key tag set reached by different association orders and by
    // different numbers of `TagResource` calls, a set whose keys sort the other way
    // round from the order they were given in, a single tag, and no tags at all. The
    // tag set is a key-value MAP, so every row that reaches the same map must be
    // reported identically — in key order — and a read must report it again unchanged.
    let rows: &[(&str, &[Value])] = &[
        ("no tags at all", &[]),
        (
            "one tag",
            &[json!([{ "Key": "team", "Value": "research" }])],
        ),
        (
            "three tags in one request, keys ascending",
            &[json!([
                { "Key": "a-env", "Value": "staging" },
                { "Key": "m-owner", "Value": "nlp" },
                { "Key": "z-team", "Value": "research" },
            ])],
        ),
        (
            "the same three tags in one request, keys descending",
            &[json!([
                { "Key": "z-team", "Value": "research" },
                { "Key": "m-owner", "Value": "nlp" },
                { "Key": "a-env", "Value": "staging" },
            ])],
        ),
        (
            "the same three tags over three requests, in a third order",
            &[
                json!([{ "Key": "m-owner", "Value": "nlp" }]),
                json!([{ "Key": "z-team", "Value": "research" }]),
                json!([{ "Key": "a-env", "Value": "staging" }]),
            ],
        ),
        (
            "a key associated twice, the later value winning",
            &[
                json!([{ "Key": "z-team", "Value": "superseded" }]),
                json!([
                    { "Key": "a-env", "Value": "staging" },
                    { "Key": "z-team", "Value": "research" },
                ]),
                json!([{ "Key": "m-owner", "Value": "nlp" }]),
            ],
        ),
        (
            // `key0..key11`: given in numeric order, these sort "key0", "key1",
            // "key10", "key11", "key2", … so a reported key order cannot be the
            // insertion order.
            "twelve tags whose key order differs from the order they were given in",
            &[tag_list("key", 12)],
        ),
    ];

    for (case, batches) in rows {
        let comprehend = Comprehend::new();
        // A resource whose creation carried no `Tags`, so the reported map is exactly
        // what these requests associated.
        let (arn, created) = create(&comprehend, &RESOURCE_KINDS[1], "order").await;
        assert!(
            created.is_empty(),
            "case {case:?}: this row's resource must start with no tags, got {created:?}"
        );

        let mut expected: TagMap = Vec::new();
        for batch in *batches {
            comprehend
                .ok("TagResource", &json!({ "ResourceArn": &arn, "Tags": batch }))
                .await;
            expected = merged(&expected, batch);
        }

        let reported = list_tags(&comprehend, &arn, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the key-ordered map {expected:?}, got {reported:?}"
        );
        let keys: Vec<&String> = reported.iter().map(|(key, _)| key).collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: the reported tags must be a set in strictly ascending key \
             order, got {keys:?}"
        );

        // The read reports, it does not change: reading again reports the same map,
        // and a later removal still finds every key the first read reported.
        let again = list_tags(&comprehend, &arn, case).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a repeated ListTagsForResource must report {reported:?}, \
             got {again:?}"
        );
        let all_keys: Vec<String> = reported.iter().map(|(key, _)| key.clone()).collect();
        comprehend
            .ok(
                "UntagResource",
                &json!({ "ResourceArn": &arn, "TagKeys": &all_keys }),
            )
            .await;
        let emptied = list_tags(&comprehend, &arn, case).await;
        assert!(
            emptied.is_empty(),
            "case {case:?}: removing every key the read reported must leave no tags, \
             got {emptied:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_belongs_to_the_one_resource_it_was_associated_with() {
    let comprehend = Comprehend::new();
    let mut resources: Vec<(&str, String, TagMap)> = Vec::new();
    for (index, kind) in RESOURCE_KINDS.iter().enumerate() {
        let (arn, created) = create(&comprehend, kind, &format!("isolated{index}")).await;
        resources.push((kind.case, arn, created));
    }

    // Every resource gets a tag under the SAME key, carrying a value that names only
    // itself.
    for (index, (_, arn, _)) in resources.iter().enumerate() {
        comprehend
            .ok(
                "TagResource",
                &json!({
                    "ResourceArn": arn,
                    "Tags": [{ "Key": "owner", "Value": format!("resource-{index}") }],
                }),
            )
            .await;
    }

    for (index, (case, arn, created)) in resources.iter().enumerate() {
        let reported = list_tags(&comprehend, arn, case).await;
        let expected = merged(
            created,
            &json!([{ "Key": "owner", "Value": format!("resource-{index}") }]),
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // And a removal from one resource leaves the others' tags in place.
    let (first_case, first_arn, first_created) = resources[0].clone();
    comprehend
        .ok(
            "UntagResource",
            &json!({ "ResourceArn": &first_arn, "TagKeys": ["owner"] }),
        )
        .await;
    let reported = list_tags(&comprehend, &first_arn, first_case).await;
    let expected = merged(&first_created, &json!([]));
    assert_eq!(
        reported, expected,
        "case {first_case:?}: expected {expected:?} after the removal, got {reported:?}"
    );
    for (index, (case, arn, created)) in resources.iter().enumerate().skip(1) {
        let reported = list_tags(&comprehend, arn, case).await;
        let expected = merged(
            created,
            &json!([{ "Key": "owner", "Value": format!("resource-{index}") }]),
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: another resource's removal must leave it holding \
             {expected:?}, got {reported:?}"
        );
    }
}
