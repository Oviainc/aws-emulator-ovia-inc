// SPDX-License-Identifier: CC0-1.0

//! One counting rule for one documented tag maximum, asserted across the two paths that
//! attach a tag list to one resource: the create that carries `Tags` and the later
//! `TagResource` of the same list.
//!
//! The rule under test: a resource's tag set is a key-to-value map, so the documented maximum
//! of tags per resource bounds the DISTINCT-KEY set the resource will carry after the call —
//! a key named more than once in one tag list is one stored tag, holding the last value given
//! for it — and the entry count of the request's list bounds nothing beyond the modelled
//! `TagList` length. A list is therefore accepted by the create exactly where it is accepted
//! by the attachment, and both leave the resource carrying the same set.
//!
//! The rows differ along what the rule generalises over: how many entries the list carries,
//! how many DISTINCT keys those entries name, how often a key repeats, and whether the
//! resulting set is within the maximum or past it.
//!
//! The request helpers are the ones `slo_lifecycle`, `resource_tags` and `tag_attachment`
//! already drive this service's `restJson1` surface with, so every suite speaks to one
//! harness.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{expected_map, list_tags_raw, reported_tags, slo_arn, tag, tag_list};
use crate::slo_lifecycle::{arn_uri_segment, call, create, name_path_segment, router, sli_config};
use crate::tag_attachment::tag_resource;

/// The most tags one SLO may carry. Its `Tags` member doc states it
/// (`smithy_query(query="doc",shape="CreateServiceLevelObjectiveInput",member="Tags")`): "You
/// can associate as many as 50 tags with an SLO".
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The largest `Tags` list the MODEL admits
/// (`smithy_query(query="shape",shape="TagList")`: `length 0..200`), which is wider than the
/// documented per-resource maximum — so a list between the two reaches the handler.
const MODELLED_MAX_TAGS: usize = 200;

/// The refusal a create answers for a tag set past [`MAX_TAGS_PER_RESOURCE`]: the member path
/// and the requirement, in the envelope every constraint violation of this service uses.
fn tag_count_message() -> String {
    format!(
        "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
         Member must have length less than or equal to {MAX_TAGS_PER_RESOURCE}"
    )
}

/// `count` tags with DISTINCT keys, numbered from `from`, each carrying its own value.
fn distinct_tags(from: usize, count: usize) -> Vec<(String, String)> {
    (from..from + count)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect()
}

/// `repeats` entries that all name the key `key`, each with a different value, so the last one
/// is the value the stored tag must carry.
fn repeated_key(key: &str, repeats: usize) -> Vec<(String, String)> {
    (0..repeats)
        .map(|index| tag(key, &format!("value-{index:03}")))
        .collect()
}

/// One row of the table: a `Tags` list, named.
struct Case {
    label: &'static str,
    tags: Vec<(String, String)>,
}

/// The tag lists the rule is asserted over: lists within the maximum and past it, each with
/// and without keys named more than once, up to the modelled list length.
fn cases() -> Vec<Case> {
    // 50 distinct keys with one of them named a second time: 51 entries, 50 stored tags — the
    // documented maximum.
    let mut at_maximum_with_repeat = distinct_tags(0, MAX_TAGS_PER_RESOURCE);
    at_maximum_with_repeat.push(tag("key-000", "named-again"));

    // Every key of a full set named twice: twice the entries, the same stored set.
    let doubled: Vec<(String, String)> = distinct_tags(0, MAX_TAGS_PER_RESOURCE)
        .into_iter()
        .flat_map(|(key, value)| {
            vec![
                (key.clone(), format!("{value}-first")),
                (key, format!("{value}-second")),
            ]
        })
        .collect();

    // One key past the maximum, with repeats padding the list out: still 51 stored tags.
    let mut past_maximum_with_repeats = distinct_tags(0, MAX_TAGS_PER_RESOURCE + 1);
    past_maximum_with_repeats.extend(repeated_key("key-000", 9));

    vec![
        Case {
            label: "the empty list the model admits",
            tags: Vec::new(),
        },
        Case {
            label: "one tag",
            tags: distinct_tags(0, 1),
        },
        Case {
            label: "one key named twice",
            tags: repeated_key("env", 2),
        },
        Case {
            label: "the whole modelled list length on a single key",
            tags: repeated_key("churn", MODELLED_MAX_TAGS),
        },
        Case {
            label: "the documented maximum in distinct keys",
            tags: distinct_tags(0, MAX_TAGS_PER_RESOURCE),
        },
        Case {
            label: "the documented maximum with one key named twice",
            tags: at_maximum_with_repeat,
        },
        Case {
            label: "the documented maximum with every key named twice",
            tags: doubled,
        },
        Case {
            label: "one distinct key past the documented maximum",
            tags: distinct_tags(0, MAX_TAGS_PER_RESOURCE + 1),
        },
        Case {
            label: "one distinct key past the documented maximum, padded with repeats",
            tags: past_maximum_with_repeats,
        },
        Case {
            label: "the whole modelled list length in distinct keys",
            tags: distinct_tags(0, MODELLED_MAX_TAGS),
        },
    ]
}

/// Read a stored SLO back by name.
async fn get_slo(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/slo/{}", name_path_segment(name)),
        None,
    )
    .await
}

/// The tags the SLO called `name` carries, as the reader reports them.
async fn tags_of(router: &MultiServiceRouter, name: &str) -> BTreeMap<String, String> {
    let (_status, listed) = list_tags_raw(router, &arn_uri_segment(&slo_arn(name))).await;
    reported_tags(&listed)
}

/// The documented maximum bounds the distinct-key SET a tag list leaves the resource carrying,
/// through BOTH paths that attach one: the create's `Tags` and a later `TagResource` of the
/// identical list are accepted for the same lists and store the same set, and a refusal on
/// either path attaches nothing.
#[tokio::test]
async fn one_tag_list_counts_the_same_through_a_create_and_an_attachment() {
    for Case { label, tags } in cases() {
        let router = router();

        // The set the list would store: one tag per KEY, carrying the last value the list
        // gives for it, which is what `put_tags` writes.
        let expected = expected_map(&tags);
        let accepted = expected.len() <= MAX_TAGS_PER_RESOURCE;

        // Path 1: the create carries the list.
        let created = "created-with-tags";
        let (status, answer) = create(
            &router,
            json!({
                "Name": created,
                "SliConfig": sli_config("checkout", "GET /cart", 100.0),
                "Tags": tag_list(&tags),
            }),
        )
        .await;

        // Path 2: the identical list is attached to an SLO created without tags.
        let attached = "attach-target";
        let (setup_status, setup) = create(
            &router,
            json!({
                "Name": attached,
                "SliConfig": sli_config("checkout", "GET /cart", 100.0),
            }),
        )
        .await;
        assert_eq!(
            setup_status, 200,
            "case {label:?}: expected the untagged setup create to succeed, \
             got {setup_status} with {setup}"
        );
        let (attach_status, attach_answer) = tag_resource(&router, &slo_arn(attached), &tags).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200 for a tag set of {} tags, \
                 got {status} with {answer}",
                expected.len()
            );
            assert_eq!(
                attach_status, 200,
                "case {label:?}: expected the attachment of the same list to be accepted too, \
                 got {attach_status} with {attach_answer}"
            );

            // Round trip: both paths leave the resource carrying the same set, one tag per key.
            let created_tags = tags_of(&router, created).await;
            let attached_tags = tags_of(&router, attached).await;
            assert_eq!(
                created_tags, expected,
                "case {label:?}: expected the created SLO to carry {expected:?}, \
                 got {created_tags:?}"
            );
            assert_eq!(
                attached_tags, expected,
                "case {label:?}: expected the attachment to store {expected:?}, \
                 got {attached_tags:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400 for a tag set of {} tags, \
                 got {status} with {answer}",
                expected.len()
            );
            assert_eq!(
                answer["__type"].as_str(),
                Some("ValidationException"),
                "case {label:?}: expected a ValidationException, got {answer}"
            );
            assert_eq!(
                answer["message"].as_str(),
                Some(tag_count_message().as_str()),
                "case {label:?}: expected the message {:?}, got {:?}",
                tag_count_message(),
                answer["message"]
            );
            let (fetched_status, fetched) = get_slo(&router, created).await;
            assert_eq!(
                fetched_status, 404,
                "case {label:?}: expected a refused create to store nothing, \
                 got {fetched_status} with {fetched}"
            );

            assert_eq!(
                attach_status, 402,
                "case {label:?}: expected the attachment of the same list to be refused too, \
                 got {attach_status} with {attach_answer}"
            );
            assert_eq!(
                attach_answer["__type"].as_str(),
                Some("ServiceQuotaExceededException"),
                "case {label:?}: expected a ServiceQuotaExceededException, got {attach_answer}"
            );
            let attached_tags = tags_of(&router, attached).await;
            assert!(
                attached_tags.is_empty(),
                "case {label:?}: expected a refused attachment to attach nothing, \
                 got {attached_tags:?}"
            );
        }
    }
}
