// SPDX-License-Identifier: CC0-1.0

//! What a tag KEY and a tag VALUE may be, over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.<Operation>`), dispatched through the same
//! multi-service router the production emulator mounts.
//!
//! THE RULE UNDER TEST: a tag is an opaque key/value pair of strings held on the resource
//! itself — the service stores what the request carried and reports it back byte for byte,
//! keys are distinguished as whole strings (so two keys differing only in case are two
//! tags), and the set lives and dies with the resource record rather than with the ARN
//! that names it.
//!
//! The round-trip rows below differ along what a string can differ along inside the
//! charset the model allows: which script its letters are from, whether it carries digits,
//! each punctuation mark the charset admits, a separator, emptiness, and each published
//! length bound. The recycled-identity rows differ along what either life of one identity
//! carried: tags attached by the create, tags added afterwards, both, and neither.
//!
//! The collection suite owns the router harness and the dispatch helpers, and the tagging
//! suite owns the create-a-taggable-resource helpers and the tag reader, so nothing here
//! builds a second one.

use std::collections::BTreeMap;

use serde_json::json;

use crate::face_collections::{call, harness};
use crate::resource_tags::{
    as_json, assert_not_found_by_every_tagging_operation, collection_resource, project_resource,
    tags_of,
};

/// The published bounds of a tag key and a tag value, from this service's model
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`: `TagKey` `length 1..128`,
/// `TagValue` `length 0..256`, both over the charset
/// `[\p{L}\p{Z}\p{N}_.:/=+\-@]`).
const TAG_KEY_MIN: usize = 1;
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MAX: usize = 256;

/// One row of the tag-form table: a label for failure messages, and the key and value the
/// request carries.
struct TagForm {
    case: &'static str,
    key: String,
    value: String,
}

/// Every form a tag pair takes inside the charset the model allows, one row each.
///
/// The punctuation row names every mark the `TagKey` / `TagValue` pattern admits, the
/// non-ASCII rows are letters (`\p{L}`) from scripts the issue cited none of, the spaced
/// row is a separator (`\p{Z}`), and the last three rows sit exactly on the published
/// length bounds.
fn tag_forms() -> Vec<TagForm> {
    vec![
        TagForm {
            case: "an ascii word",
            key: "env".to_string(),
            value: "production".to_string(),
        },
        TagForm {
            case: "digits only",
            key: "1234".to_string(),
            value: "5678".to_string(),
        },
        TagForm {
            case: "every punctuation mark the charset admits",
            key: "key_.:/=+-@".to_string(),
            value: "value_.:/=+-@".to_string(),
        },
        TagForm {
            case: "a separator inside the key and the value",
            key: "cost centre".to_string(),
            value: "research and development".to_string(),
        },
        TagForm {
            case: "letters outside ascii",
            key: "propriétaire".to_string(),
            value: "équipe-ключ-キー".to_string(),
        },
        TagForm {
            case: "an empty value",
            key: "no-value".to_string(),
            // The published `TagValue` minimum length is zero, so the empty string is a
            // value a resource can carry.
            value: String::new(),
        },
        TagForm {
            case: "the shortest key the model allows",
            key: "k".repeat(TAG_KEY_MIN),
            value: "one-character-key".to_string(),
        },
        TagForm {
            case: "the longest key the model allows",
            key: "k".repeat(TAG_KEY_MAX),
            value: "longest-key".to_string(),
        },
        TagForm {
            case: "the longest value the model allows",
            key: "longest-value".to_string(),
            value: "v".repeat(TAG_VALUE_MAX),
        },
    ]
}

#[tokio::test]
async fn every_form_of_tag_pair_the_model_allows_round_trips_byte_for_byte() {
    let router = harness();
    let resources = vec![
        collection_resource(&router, "a collection", "c1", "verbatim", &[]).await,
        project_resource(&router, "a project", "p1", "verbatim-project", &[]).await,
    ];

    let forms = tag_forms();
    let written: BTreeMap<String, String> = forms
        .iter()
        .map(|form| (form.key.clone(), form.value.clone()))
        .collect();
    assert_eq!(
        written.len(),
        forms.len(),
        "the table's keys must be distinct, got {written:?}"
    );

    for resource in &resources {
        let arn = &resource.arns[0];
        let tagged = call(
            &router,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": as_json(&written) }),
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {:?}: tagging every form at once must succeed, got {:?}",
            resource.case, tagged.body
        );

        // Reported through every ARN form that names the resource, so a value survives
        // the round trip whichever way the resource is read back.
        for read_by in &resource.arns {
            let reported = tags_of(&router, resource.case, read_by).await;
            for form in &forms {
                let value = reported.get(&form.key).and_then(|value| value.as_str());
                assert_eq!(
                    value,
                    Some(form.value.as_str()),
                    "case {:?}/{} read by {read_by}: the key must report the value it was \
                     tagged with, got {value:?}",
                    form.case,
                    resource.case
                );
            }
            assert_eq!(
                reported,
                as_json(&written),
                "case {:?} read by {read_by}: the resource reports exactly the pairs it \
                 was tagged with",
                resource.case
            );
        }
    }
}

#[tokio::test]
async fn two_keys_differing_only_in_case_are_two_tags() {
    let router = harness();
    let resources = vec![
        collection_resource(&router, "a collection", "c1", "cased", &[]).await,
        project_resource(&router, "a project", "p1", "cased-project", &[]).await,
    ];

    // Rows: the same letters in four casings, each with its own value.
    let cased: BTreeMap<String, String> = [
        ("env", "lower"),
        ("Env", "capitalised"),
        ("ENV", "upper"),
        ("eNv", "mixed"),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
    .collect();

    for resource in &resources {
        let case = resource.case;
        let arn = &resource.arns[0];
        let tagged = call(
            &router,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": as_json(&cased) }),
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: four casings of one word are four keys, got {:?}",
            tagged.body
        );
        assert_eq!(
            tags_of(&router, case, arn).await,
            as_json(&cased),
            "case {case:?}: every casing must be carried as its own tag"
        );

        // Removing one casing leaves the others, and overwriting one casing leaves the
        // others' values alone.
        let untagged = call(
            &router,
            "UntagResource",
            json!({ "ResourceArn": arn, "TagKeys": ["env"] }),
        )
        .await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: removing one casing must succeed, got {:?}",
            untagged.body
        );
        let overwritten = call(
            &router,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": { "ENV": "upper-again" } }),
        )
        .await;
        assert_eq!(
            overwritten.status, 200,
            "case {case:?}: overwriting one casing must succeed, got {:?}",
            overwritten.body
        );
        let mut survived = cased.clone();
        survived.remove("env");
        survived.insert("ENV".to_string(), "upper-again".to_string());
        assert_eq!(
            tags_of(&router, case, arn).await,
            as_json(&survived),
            "case {case:?}: only the casing the request named may change"
        );
    }
}

/// One row of the recycled-identity table: what the resource's first life carried, and
/// what the create of its second life attaches.
struct Recycled {
    case: &'static str,
    collection_id: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    added: &'static [(&'static str, &'static str)],
    recreated_with: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_tag_set_belongs_to_the_live_resource_and_not_to_its_arn() {
    let router = harness();

    // Rows differ along what either life of the identity carries: tags attached by the
    // create, tags added afterwards, both, and neither.
    let table = [
        Recycled {
            case: "tagged at create and tagged again, recreated with tags",
            collection_id: "recycled-both",
            created_with: &[("env", "first-life")],
            added: &[("owner", "first-life")],
            recreated_with: &[("env", "second-life")],
        },
        Recycled {
            case: "tagged only after the create, recreated bare",
            collection_id: "recycled-added",
            created_with: &[],
            added: &[("owner", "first-life"), ("team", "first-life")],
            recreated_with: &[],
        },
        Recycled {
            case: "never tagged at all, recreated with tags",
            collection_id: "recycled-bare",
            created_with: &[],
            added: &[],
            recreated_with: &[("env", "second-life"), ("owner", "second-life")],
        },
    ];

    for row in &table {
        let case = row.case;
        let first = collection_resource(
            &router,
            "the first life",
            "first",
            row.collection_id,
            row.created_with,
        )
        .await;
        let added: BTreeMap<String, String> = row
            .added
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        if !added.is_empty() {
            let tagged = call(
                &router,
                "TagResource",
                json!({ "ResourceArn": &first.arns[0], "Tags": as_json(&added) }),
            )
            .await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: tagging the first life must succeed, got {:?}",
                tagged.body
            );
        }

        // What the first life carries before it is deleted: what its create attached,
        // plus what was tagged onto it — so each row's setup is load-bearing rather than
        // assumed.
        let mut first_life: BTreeMap<String, String> = row
            .created_with
            .iter()
            .map(|(key, value)| ((*key).to_string(), format!("{value}-first")))
            .collect();
        first_life.extend(added.clone());
        assert_eq!(
            tags_of(&router, case, &first.arns[0]).await,
            as_json(&first_life),
            "case {case:?}: the first life must carry what its create and its tagging put \
             on it"
        );

        let deleted = call(
            &router,
            "DeleteCollection",
            json!({ "CollectionId": row.collection_id }),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting the collection must succeed, got {:?}",
            deleted.body
        );

        // The ARN now names no live resource, in either of its forms, so the tagging
        // operations do not find it — the tag set went with the record.
        for arn in &first.arns {
            assert_not_found_by_every_tagging_operation(&router, case, arn).await;
        }

        // The same identity created again is a NEW resource under the same ARN: it
        // carries what its own create attached and nothing its predecessor carried.
        let second = collection_resource(
            &router,
            "the second life",
            "second",
            row.collection_id,
            row.recreated_with,
        )
        .await;
        let expected: BTreeMap<String, String> = row
            .recreated_with
            .iter()
            .map(|(key, value)| ((*key).to_string(), format!("{value}-second")))
            .collect();
        for arn in &second.arns {
            let reported = tags_of(&router, case, arn).await;
            assert_eq!(
                reported,
                as_json(&expected),
                "case {case:?}: {arn} must report only what the new create attached, got \
                 {reported:?}"
            );
        }
    }
}
