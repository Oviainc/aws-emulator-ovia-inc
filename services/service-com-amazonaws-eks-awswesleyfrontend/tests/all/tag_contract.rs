// SPDX-License-Identifier: CC0-1.0

//! The TAG CONTRACT the two tag-writing operations refuse a request against, and
//! the error channel they refuse it in.
//!
//! RULE: a request a declared input constraint refuses is answered in the error
//! channel the CALLED operation models — for `TagResource` and `UntagResource`, whose
//! model declares `BadRequestException` (400) and `NotFoundException` (404) and
//! nothing else, that is a 400 `BadRequestException`; for the operations that declare
//! `InvalidParameterException` it stays that. The one refusal this service states in
//! its own sentence rather than the generic constraint report is the number of tags a
//! resource may carry: fixed text with the limit as its only variable part, echoing
//! nothing the request carried — and the same sentence whether the ceiling is broken
//! by one request's map or by the accumulated map a merge would leave.
//!
//! Each suite asserts that over a TABLE rather than one recorded request: the rows
//! differ along which bound is broken (the entry count of `tags`, the element count
//! of `tagKeys`, a key's length, a value's length, an absent member), along each
//! published bound and one step past it, and along the operation. Every assertion
//! names its case and prints what came back.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`, `error_code`) is
//! the shared one in `tests/all/wire.rs`; the cluster builders are
//! `cluster_lifecycle`'s.

use std::collections::BTreeMap;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    tag_ceiling_message, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

pub(crate) type Tags = BTreeMap<String, String>;

/// The published character bounds of one tag key and one tag value:
/// `TagKey` is `@length {min: 1, max: 128}` and `TagValue` is
/// `@length {min: 0, max: 256}`
/// (`smithy_query(query="shape",shape="TagKey,TagValue",
/// model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")`).
const MIN_KEY_LENGTH: usize = 1;
/// See [`MIN_KEY_LENGTH`].
const MAX_KEY_LENGTH: usize = 128;
/// See [`MIN_KEY_LENGTH`].
const MAX_VALUE_LENGTH: usize = 256;

/// The error code the two tag-writing operations refuse a bad request with.
const BAD_REQUEST: &str = "BadRequestException";

/// `count` distinct tag keys, each with a value of its own.
fn distinct_tags(count: usize) -> Tags {
    (0..count)
        .map(|index| (format!("key-{index:04}"), format!("value-{index:04}")))
        .collect()
}

async fn tag(router: &MultiServiceRouter, arn: &str, body: &serde_json::Value) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        &body.to_string(),
    )
    .await
}

pub(crate) async fn tag_map(router: &MultiServiceRouter, arn: &str, map: &Tags) -> Answer {
    tag(router, arn, &serde_json::json!({ "tags": map })).await
}

async fn untag(router: &MultiServiceRouter, arn: &str, tag_keys: &[String]) -> Answer {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = if query.is_empty() {
        format!("/tags/{}", encode_label(arn))
    } else {
        format!("/tags/{}?{query}", encode_label(arn))
    };
    call(router, REGION, "DELETE", &uri, "").await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        "",
    )
    .await
}

/// A cluster to tag, and its ARN.
pub(crate) async fn taggable(router: &MultiServiceRouter, name: &str) -> String {
    let created = create(router, &minimal_request(name)).await;
    assert_eq!(
        created.status, 200,
        "setup create {name:?}: expected 200, got {created:?}"
    );
    created.cluster()["arn"]
        .as_str()
        .expect("a created cluster carries an arn")
        .to_owned()
}

/// The tag map a resource currently carries, read back through
/// `ListTagsForResource`.
pub(crate) async fn stored(router: &MultiServiceRouter, arn: &str) -> serde_json::Value {
    let listed = list_tags(router, arn).await;
    assert_eq!(
        listed.status, 200,
        "ListTagsForResource must answer 200, got {listed:?}"
    );
    listed.body["tags"].clone()
}

// ---------------------------------------------------------------------------
// RULE: the number of tags one request may carry is the model's `TagMap`
// `@length` ceiling, and breaking it is a 400 `BadRequestException` carrying this
// service's tagging sentence.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_tag_count_ceiling_is_a_bad_request_with_the_tagging_sentence() {
    let router = emulator();
    let arn = taggable(&router, "ceiling-target").await;

    // Rows span the bound: both ends of the accepted range, the first count past it,
    // and counts far past it.
    let accepted: [usize; 4] = [1, 2, MAX_TAGS_PER_RESOURCE - 1, MAX_TAGS_PER_RESOURCE];
    let refused: [usize; 4] = [
        MAX_TAGS_PER_RESOURCE + 1,
        MAX_TAGS_PER_RESOURCE + 2,
        MAX_TAGS_PER_RESOURCE * 2,
        MAX_TAGS_PER_RESOURCE * 4,
    ];

    for count in refused {
        // A fresh resource per row, so each refusal is measured against an empty map.
        let target = taggable(&router, &format!("over-{count}")).await;
        let answer = tag_map(&router, &target, &distinct_tags(count)).await;
        assert_eq!(
            answer.status, 400,
            "case {count} tags: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            BAD_REQUEST,
            "case {count} tags: expected {BAD_REQUEST}, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(tag_ceiling_message(MAX_TAGS_PER_RESOURCE).as_str()),
            "case {count} tags: expected the tagging sentence, got {:?}",
            answer.body["message"]
        );
        // The refusal is TOTAL: nothing of the request reached the record.
        assert_eq!(
            stored(&router, &target).await,
            serde_json::json!({}),
            "case {count} tags: a refused request must write nothing"
        );
    }

    for count in accepted {
        let map = distinct_tags(count);
        let answer = tag_map(&router, &arn, &map).await;
        assert_eq!(
            answer.status, 200,
            "case {count} tags: expected HTTP 200, got {answer:?}"
        );
        let expected = serde_json::to_value(&map).expect("tags serialize");
        assert_eq!(
            stored(&router, &arn).await,
            expected,
            "case {count} tags: expected the map to be stored"
        );
        // Clear the map again so the next row starts from the same place.
        let keys: Vec<String> = map.keys().cloned().collect();
        let cleared = untag(&router, &arn, &keys).await;
        assert_eq!(
            cleared.status, 200,
            "case {count} tags: setup untag must answer 200, got {cleared:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE (the same ceiling, counted after the merge): the tags a request would LEAVE
// on the resource are what the ceiling bounds, and breaking it earns the same
// sentence.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_accumulated_ceiling_earns_the_same_sentence() {
    // (case, how many tags are stored first, how many fresh keys are added, whether
    // the merge is accepted) — rows differ along where the two requests split the
    // ceiling, including the row where the merge lands exactly on it.
    let rows: [(&str, usize, usize, bool); 5] = [
        ("a merge landing exactly on the ceiling", 25, 25, true),
        ("a merge one past the ceiling", 25, 26, false),
        ("one fresh key past a full map", MAX_TAGS_PER_RESOURCE, 1, false),
        ("a full map's last free slot", MAX_TAGS_PER_RESOURCE - 1, 1, true),
        ("a second full map onto a full one", 40, 40, false),
    ];

    for (index, (case, first, second, accepted)) in rows.iter().enumerate() {
        let router = emulator();
        let arn = taggable(&router, &format!("merge-{index}")).await;

        let stored_first = distinct_tags(*first);
        let seeded = tag_map(&router, &arn, &stored_first).await;
        assert_eq!(
            seeded.status, 200,
            "case {case:?}: setup tag of {first} keys must answer 200, got {seeded:?}"
        );

        // Fresh keys, none of them already on the record, so the merge GROWS the map.
        let added: Tags = (0..*second)
            .map(|offset| (format!("extra-{offset:04}"), "v".to_string()))
            .collect();
        let answer = tag_map(&router, &arn, &added).await;

        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {answer:?}"
            );
            let mut both = stored_first.clone();
            both.extend(added.clone());
            assert_eq!(
                stored(&router, &arn).await,
                serde_json::to_value(&both).expect("tags serialize"),
                "case {case:?}: expected both maps to be stored"
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(tag_ceiling_message(MAX_TAGS_PER_RESOURCE).as_str()),
            "case {case:?}: expected the tagging sentence, got {:?}",
            answer.body["message"]
        );
        assert_eq!(
            stored(&router, &arn).await,
            serde_json::to_value(&stored_first).expect("tags serialize"),
            "case {case:?}: the refused merge must leave the stored map alone"
        );
    }

    // A map at the ceiling can still be RE-tagged: a key already on the record is
    // overwritten rather than added, so the merge does not grow and is accepted.
    let router = emulator();
    let arn = taggable(&router, "merge-overwrite").await;
    let full = distinct_tags(MAX_TAGS_PER_RESOURCE);
    let seeded = tag_map(&router, &arn, &full).await;
    assert_eq!(seeded.status, 200, "setup tag: got {seeded:?}");
    let overwrite: Tags = full
        .keys()
        .take(3)
        .map(|key| (key.clone(), "rewritten".to_string()))
        .collect();
    let answer = tag_map(&router, &arn, &overwrite).await;
    assert_eq!(
        answer.status, 200,
        "overwriting keys of a full map must answer 200, got {answer:?}"
    );
    let mut expected = full.clone();
    expected.extend(overwrite.clone());
    assert_eq!(
        stored(&router, &arn).await,
        serde_json::to_value(&expected).expect("tags serialize"),
        "the overwritten values must be stored"
    );
}

// ---------------------------------------------------------------------------
// RULE: the per-entry bounds the model declares on a tag key and a tag value are
// refused in the same channel, and the refusal names the constraint that was broken.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_key_and_value_bounds_are_refused_as_a_bad_request() {
    let router = emulator();

    // (case, key length, value length, accepted) — each published bound and one step
    // past it, plus lengths far past them.
    let rows: [(&str, usize, usize, bool); 9] = [
        ("the low end of both bounds", MIN_KEY_LENGTH, 0, true),
        ("the high end of both bounds", MAX_KEY_LENGTH, MAX_VALUE_LENGTH, true),
        ("a mid-range key and value", 12, 40, true),
        ("a key one past its high bound", MAX_KEY_LENGTH + 1, 4, false),
        ("a key far past its high bound", MAX_KEY_LENGTH * 8, 4, false),
        ("an empty key, one under its low bound", 0, 4, false),
        ("a value one past its high bound", 6, MAX_VALUE_LENGTH + 1, false),
        ("a value far past its high bound", 6, MAX_VALUE_LENGTH * 8, false),
        (
            "a key and a value both past theirs",
            MAX_KEY_LENGTH + 1,
            MAX_VALUE_LENGTH + 1,
            false,
        ),
    ];

    for (index, (case, key_length, value_length, accepted)) in rows.iter().enumerate() {
        let arn = taggable(&router, &format!("bounds-{index}")).await;
        let mut map = Tags::new();
        map.insert("k".repeat(*key_length), "v".repeat(*value_length));
        let answer = tag_map(&router, &arn, &map).await;

        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {answer:?}"
            );
            assert_eq!(
                stored(&router, &arn).await,
                serde_json::to_value(&map).expect("tags serialize"),
                "case {case:?}: expected the entry to be stored"
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {answer:?}"
        );
        // The WORDING of a per-entry refusal is the tagging front end's
        // map-constraint document, pinned entry by entry in
        // `tests/all/tag_map_constraint_message.rs`; here it only has to be that
        // document, naming the member the front end reports the added tags under.
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("validation error") && message.contains("addedTags"),
            "case {case:?}: expected a constraint report naming the member, got {message:?}"
        );
        assert_eq!(
            stored(&router, &arn).await,
            serde_json::json!({}),
            "case {case:?}: a refused request must write nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a `tags` member the request does not supply at all, or supplies empty, is
// the same channel again — the model requires it and bounds it at 1..50.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_or_empty_tag_map_is_a_bad_request() {
    let router = emulator();
    let arn = taggable(&router, "absent-map").await;

    let rows: [(&str, serde_json::Value); 3] = [
        ("an empty map", serde_json::json!({ "tags": {} })),
        ("no tags member at all", serde_json::json!({})),
        ("a null tags member", serde_json::json!({ "tags": null })),
    ];

    for (case, body) in rows {
        let answer = tag(&router, &arn, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {answer:?}"
        );
        assert_eq!(
            stored(&router, &arn).await,
            serde_json::json!({}),
            "case {case:?}: a refused request must write nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `UntagResource` declares the same two errors, so its own declared bounds —
// the 1..50 element count of `tagKeys` and each element's `TagKey` length — are
// refused in the same channel.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn untag_refuses_its_declared_bounds_in_the_same_channel() {
    let router = emulator();
    let arn = taggable(&router, "untag-bounds").await;
    let stored_map = distinct_tags(MAX_TAGS_PER_RESOURCE);
    let seeded = tag_map(&router, &arn, &stored_map).await;
    assert_eq!(seeded.status, 200, "setup tag: got {seeded:?}");

    let every_key: Vec<String> = stored_map.keys().cloned().collect();
    let too_many: Vec<String> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("key-{index:04}"))
        .collect();
    let far_too_many: Vec<String> = (0..MAX_TAGS_PER_RESOURCE * 3)
        .map(|index| format!("key-{index:04}"))
        .collect();

    // (case, the keys to delete, accepted) — rows span the element count's high bound
    // and one past it, and an element past the key length bound. A request carrying NO
    // `tagKeys` at all is deliberately absent: the member is `@required` +
    // `@httpQuery`, so the REST router rejects the route itself when it is missing and
    // no operation is ever named for such a request.
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        ("one key", vec![every_key[0].clone()], true),
        ("every key of a map at the ceiling", every_key.clone(), true),
        ("one key past the element ceiling", too_many, false),
        ("far past the element ceiling", far_too_many, false),
        (
            "an element past the key length bound",
            vec!["k".repeat(MAX_KEY_LENGTH + 1)],
            false,
        ),
        (
            "a long element beside a well-formed one",
            vec![every_key[1].clone(), "k".repeat(MAX_KEY_LENGTH * 4)],
            false,
        ),
    ];

    for (case, keys, accepted) in &rows {
        // Re-seed, so every row deletes from the same stored map.
        let reseeded = tag_map(&router, &arn, &stored_map).await;
        assert_eq!(
            reseeded.status, 200,
            "case {case:?}: setup re-tag must answer 200, got {reseeded:?}"
        );

        let answer = untag(&router, &arn, keys).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {answer:?}"
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {answer:?}"
        );
        assert_eq!(
            stored(&router, &arn).await,
            serde_json::to_value(&stored_map).expect("tags serialize"),
            "case {case:?}: a refused delete must leave the stored map alone"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE (the other half of it): an operation that models `InvalidParameterException`
// keeps answering that for the same class of refusal — the channel follows the
// OPERATION, not the member.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_operation_that_models_invalid_parameter_keeps_it() {
    let router = emulator();

    // `CreateCluster` carries the very same `TagMap` member, bounded the same way,
    // and declares `InvalidParameterException` rather than `BadRequestException`
    // (`smithy_query(query="operation",operation="CreateCluster",…)`), so the same
    // broken bound is reported in ITS channel. The second row breaks a different
    // declared bound on the same operation.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "a tag map past the entry ceiling",
            {
                let mut request = minimal_request("invalid-parameter-tags");
                request["tags"] =
                    serde_json::to_value(distinct_tags(MAX_TAGS_PER_RESOURCE + 1))
                        .expect("tags serialize");
                request
            },
        ),
        ("a name outside its declared pattern", {
            let mut request = minimal_request("-leading-hyphen");
            request["tags"] = serde_json::json!({ "env": "dev" });
            request
        }),
    ];

    for (case, request) in &rows {
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
}
