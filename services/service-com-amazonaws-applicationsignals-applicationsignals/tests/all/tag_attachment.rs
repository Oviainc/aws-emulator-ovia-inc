// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `TagResource` over HTTP, read back through the
//! `ListTagsForResource` reader that reports what it attached.
//!
//! The rule under test: a resource's tag set is a key-to-value map owned by the STORED
//! resource, so an attachment is per KEY — a key the resource does not carry is added, a key
//! it already carries takes the new value, every other tag keeps its value — and the resulting
//! set may not carry the resource past the documented maximum of tags per resource, which is
//! the modelled `ServiceQuotaExceededException`. A `ResourceArn` naming no resource this
//! service holds is the modelled `ResourceNotFoundException`, and a refused request attaches
//! nothing. Every assertion below is made over a SET of tag sets rather than on one request.
//!
//! The request helpers are the ones `resource_tags` and `slo_lifecycle` already drive this
//! service's `restJson1` surface with, so every suite speaks to one harness.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{
    expected_map, list_tags_raw, reported_tags, slo_arn, tag, tag_list, untag,
};
use crate::slo_lifecycle::{arn_uri_segment, call, create, router, sli_config, ACCOUNT, REGION};

/// The most tags one resource whose API documents a maximum may carry. An SLO's `Tags` member
/// doc states it: "You can associate as many as 50 tags with an SLO" — a maximum the modelled
/// `TagList` bound (`smithy_query(query="shape",shape="TagList")`: `length 0..200`) is wider
/// than.
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most tags a resource whose own API documents NO maximum may carry: that same modelled
/// `TagList` maximum. The dynamic instrumentation configuration is such a resource — its `Tags`
/// doc
/// (`smithy_query(query="doc",shape="CreateInstrumentationConfigurationRequest",member="Tags")`)
/// states only that the list is optional.
const MAX_TAGS_PER_LIST: usize = 200;

/// The `ResourceType` a failure about an SLO reports: the resource's type NAME, capitalised.
const SLO_RESOURCE_TYPE: &str = "Slo";

/// `POST /tag-resource` with the given `ResourceArn` and `Tags`
/// (`smithy_query(query="operation",operation="TagResource")`: `POST /tag-resource`).
async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/tag-resource",
        Some(json!({ "ResourceArn": resource_arn, "Tags": tag_list(tags) })),
    )
    .await
}

/// The tags the SLO called `name` carries, as the reader reports them.
async fn list_tags(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    list_tags_raw(router, &arn_uri_segment(&slo_arn(name))).await
}

/// Create the SLO called `name` carrying `tags`, and assert the create succeeded.
async fn create_tagged(
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
    tags: &[(String, String)],
) {
    let (status, created) = create(
        router,
        json!({
            "Name": name,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0),
            "Tags": tag_list(tags),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the setup create of {name:?} to succeed, got {status} with {created}"
    );
}

/// `count` distinct tags whose keys start at `from`, so a row can hold a tag set the test sizes
/// and two rows can hold disjoint key ranges.
fn generated_tags(from: usize, count: usize) -> Vec<(String, String)> {
    (from..from + count)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect()
}

/// One row of the maximum-per-resource table: the tags the resource is created with, the tags
/// the `TagResource` call attaches, and whether the RESULTING set is within the maximum (so
/// whether the call is accepted at all).
struct Quota {
    label: &'static str,
    created: Vec<(String, String)>,
    attach: Vec<(String, String)>,
    accepted: bool,
}

/// One row of the attachment table: the tags the resource is created with, the tags the
/// `TagResource` call attaches, and the tag set the resource must carry afterwards.
struct Case {
    label: &'static str,
    created: Vec<(String, String)>,
    attach: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// The tag sets the rule is asserted over.
///
/// The rows differ along what the rule generalises over: how many tags the resource already
/// carries, how many the request attaches, whether each attached key is new or already carried
/// (so whether it is added or re-valued), whether a key is named more than once in one request,
/// and the sizes the model admits for a key, a value and the list
/// (`smithy_query(query="shape",shape="Tag,TagKey,TagValue,TagList")`: `TagKey` has
/// `length 1..128`, `TagValue` `length 0..256`, `TagList` `length 0..200`).
fn cases() -> Vec<Case> {
    let twenty = generated_tags(0, 20);
    let thirty_more = generated_tags(20, 30);
    let filled: Vec<(String, String)> = twenty.iter().chain(thirty_more.iter()).cloned().collect();

    vec![
        Case {
            label: "a key the resource does not carry is added",
            created: vec![tag("env", "prod")],
            attach: vec![tag("team", "payments")],
            expected: vec![tag("env", "prod"), tag("team", "payments")],
        },
        Case {
            label: "a key the resource carries takes the new value",
            created: vec![tag("env", "prod"), tag("team", "payments")],
            attach: vec![tag("env", "staging")],
            expected: vec![tag("env", "staging"), tag("team", "payments")],
        },
        Case {
            label: "several keys at once, some new and some re-valued",
            created: vec![tag("env", "prod"), tag("team", "payments")],
            attach: vec![tag("env", "staging"), tag("tier", "gold")],
            expected: vec![
                tag("env", "staging"),
                tag("team", "payments"),
                tag("tier", "gold"),
            ],
        },
        Case {
            label: "an untagged resource gains its first tags",
            created: vec![],
            attach: vec![tag("env", "prod"), tag("team", "payments")],
            expected: vec![tag("env", "prod"), tag("team", "payments")],
        },
        Case {
            // `TagList` admits `length 0`, so an empty list names no tag to attach.
            label: "the empty tag list the model admits attaches nothing",
            created: vec![tag("env", "prod")],
            attach: vec![],
            expected: vec![tag("env", "prod")],
        },
        Case {
            label: "the empty tag list on a resource with no tags",
            created: vec![],
            attach: vec![],
            expected: vec![],
        },
        Case {
            label: "the same key twice in one request takes the last value given",
            created: vec![],
            attach: vec![tag("env", "first"), tag("env", "second")],
            expected: vec![tag("env", "second")],
        },
        Case {
            // The shortest key the modelled `TagKey` `length 1..128` admits.
            label: "shortest key the model admits",
            created: vec![tag("env", "prod")],
            attach: vec![tag("x", "1")],
            expected: vec![tag("env", "prod"), tag("x", "1")],
        },
        Case {
            // The longest key that bound admits.
            label: "longest key the model admits",
            created: vec![tag("env", "prod")],
            attach: vec![tag(&"k".repeat(128), "longest-key")],
            expected: vec![tag("env", "prod"), tag(&"k".repeat(128), "longest-key")],
        },
        Case {
            // The longest value the modelled `TagValue` `length 0..256` admits.
            label: "longest value the model admits",
            created: vec![],
            attach: vec![tag("env", &"v".repeat(256))],
            expected: vec![tag("env", &"v".repeat(256))],
        },
        Case {
            // `TagValue` admits `length 0`: the empty value is a value like any other.
            label: "an empty value",
            created: vec![tag("env", "prod")],
            attach: vec![tag("empty", "")],
            expected: vec![tag("env", "prod"), tag("empty", "")],
        },
        Case {
            label: "re-valuing a carried key to the empty value",
            created: vec![tag("env", "prod"), tag("team", "payments")],
            attach: vec![tag("env", "")],
            expected: vec![tag("env", ""), tag("team", "payments")],
        },
        Case {
            label: "the documented maximum of tags attached at once",
            created: vec![],
            attach: filled.clone(),
            expected: filled.clone(),
        },
        Case {
            label: "filling the documented maximum from a partly tagged resource",
            created: twenty,
            attach: thirty_more,
            expected: filled,
        },
    ]
}

/// An attachment adds the keys the resource does not carry, re-values the ones it does, leaves
/// every other tag alone and answers HTTP 200 with the empty body its modelled output (no
/// members) serialises to — for every shape of stored and attached tag set.
#[tokio::test]
async fn tag_attaches_the_named_keys_and_leaves_the_rest() {
    for case in cases() {
        let router = router();
        let name = "tagged-slo";
        create_tagged(&router, case.label, name, &case.created).await;

        // The reader reports what the create stored, so the attachment is measured against a
        // known starting point rather than an assumed one.
        let (status, before) = list_tags(&router, name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the pre-tag list, got {status} with {before}",
            case.label
        );
        assert_eq!(
            reported_tags(&before),
            expected_map(&case.created),
            "case {:?}: expected the created tags {:?}, got {:?}",
            case.label,
            expected_map(&case.created),
            reported_tags(&before)
        );

        let (status, body) = tag_resource(&router, &slo_arn(name), &case.attach).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the tag, got {status} with {body}",
            case.label
        );
        assert_eq!(
            body,
            json!({}),
            "case {:?}: expected the tag to answer the empty body {{}}, got {body}",
            case.label
        );

        let (status, after) = list_tags(&router, name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the post-tag list, got {status} with {after}",
            case.label
        );
        assert_eq!(
            reported_tags(&after),
            expected_map(&case.expected),
            "case {:?}: expected the resulting tags {:?}, got {:?}",
            case.label,
            expected_map(&case.expected),
            reported_tags(&after)
        );

        // The resource itself is untouched: only its tags were addressed.
        let (status, slo) = call(&router, "GET", &format!("/slo/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the SLO to still be readable, got {status} with {slo}",
            case.label
        );
    }
}

/// Attachment is per key to a given value, so repeating a tag changes nothing further: the
/// second and third calls answer exactly as the first did and the tag set stays put.
#[tokio::test]
async fn repeating_a_tag_changes_nothing_further() {
    for case in cases() {
        let router = router();
        let name = "repeat-slo";
        create_tagged(&router, case.label, name, &case.created).await;

        for attempt in 1..=3 {
            let (status, body) = tag_resource(&router, &slo_arn(name), &case.attach).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: expected HTTP 200, got {status} with {body}",
                case.label
            );

            let (status, after) = list_tags(&router, name).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: expected HTTP 200 from the list, got {status} with {after}",
                case.label
            );
            assert_eq!(
                reported_tags(&after),
                expected_map(&case.expected),
                "case {:?} attempt {attempt}: expected the resulting tags {:?}, got {:?}",
                case.label,
                expected_map(&case.expected),
                reported_tags(&after)
            );
        }
    }
}

/// An attachment that would leave one resource carrying more tags than the maximum its kind
/// documents is refused with the modelled `ServiceQuotaExceededException` (HTTP 402) and
/// attaches nothing, while an attachment whose RESULTING set is within the maximum is accepted
/// — so the bound is on the union of the stored and attached keys, not on either alone.
#[tokio::test]
async fn an_attachment_over_the_documented_maximum_is_refused_and_attaches_nothing() {
    let full = generated_tags(0, MAX_TAGS_PER_RESOURCE);
    let revalued: Vec<(String, String)> = full
        .iter()
        .map(|(key, _)| tag(key, "revalued"))
        .collect();

    let rows = vec![
        Quota {
            label: "one key past a resource already at the maximum",
            created: full.clone(),
            attach: vec![tag("one-too-many", "x")],
            accepted: false,
        },
        Quota {
            label: "a list one past the maximum onto an untagged resource",
            created: vec![],
            attach: generated_tags(0, MAX_TAGS_PER_RESOURCE + 1),
            accepted: false,
        },
        Quota {
            // The modelled `TagList` admits 200 entries, so this request passes the boundary
            // validator and is refused by the resource's own maximum instead.
            label: "the whole list length the model admits",
            created: vec![],
            attach: generated_tags(0, 200),
            accepted: false,
        },
        Quota {
            label: "a few new keys that would cross the maximum",
            created: generated_tags(0, MAX_TAGS_PER_RESOURCE - 2),
            attach: generated_tags(MAX_TAGS_PER_RESOURCE - 2, 5),
            accepted: false,
        },
        Quota {
            // Re-valuing keys the resource already carries adds no key, so a resource at the
            // maximum still accepts it.
            label: "re-valuing every key of a resource at the maximum",
            created: full.clone(),
            attach: revalued.clone(),
            accepted: true,
        },
        Quota {
            label: "exactly filling the maximum",
            created: generated_tags(0, MAX_TAGS_PER_RESOURCE - 5),
            attach: generated_tags(MAX_TAGS_PER_RESOURCE - 5, 5),
            accepted: true,
        },
        Quota {
            label: "the last key the maximum admits",
            created: generated_tags(0, MAX_TAGS_PER_RESOURCE - 1),
            attach: vec![tag("last", "x")],
            accepted: true,
        },
    ];

    for Quota {
        label,
        created,
        attach,
        accepted,
    } in rows
    {
        let router = router();
        let name = "quota-slo";
        create_tagged(&router, label, name, &created).await;

        let (status, body) = tag_resource(&router, &slo_arn(name), &attach).await;
        let (_status, after) = list_tags(&router, name).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200 for a resulting set within the maximum, got {status} with {body}"
            );
            let mut expected = expected_map(&created);
            expected.extend(expected_map(&attach));
            assert_eq!(
                reported_tags(&after),
                expected,
                "case {label:?}: expected the resulting tags {expected:?}, got {:?}",
                reported_tags(&after)
            );
            assert!(
                reported_tags(&after).len() <= MAX_TAGS_PER_RESOURCE,
                "case {label:?}: expected at most {MAX_TAGS_PER_RESOURCE} tags, got {}",
                reported_tags(&after).len()
            );
        } else {
            assert_eq!(
                status, 402,
                "case {label:?}: expected HTTP 402 for a resulting set over the maximum, got {status} with {body}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("ServiceQuotaExceededException"),
                "case {label:?}: expected a ServiceQuotaExceededException, got {body}"
            );
            assert_eq!(
                reported_tags(&after),
                expected_map(&created),
                "case {label:?}: expected a refused attachment to attach nothing, got {:?}",
                reported_tags(&after)
            );
        }
    }
}

/// A `Tags` entry outside the modelled `Tag` bounds, or a list longer than `TagList`'s
/// `length 0..200`, is no tag list the API accepts: the request is refused with a 400
/// `ValidationException` and attaches nothing. Each published bound is asserted one past it
/// here, and at the bound by the accepted rows of the attachment table above.
#[tokio::test]
async fn a_tag_list_outside_the_modelled_bounds_is_refused_and_attaches_nothing() {
    let router = router();
    let name = "bounded-slo";
    let kept = vec![tag("env", "prod")];
    create_tagged(&router, "bounds", name, &kept).await;

    let refused: Vec<(&str, Vec<(String, String)>)> = vec![
        ("key one character past the maximum", vec![tag(&"k".repeat(129), "x")]),
        ("empty key, one below the minimum", vec![tag("", "x")]),
        ("value one character past the maximum", vec![tag("env", &"v".repeat(257))]),
        ("201 tags, one past the list maximum", generated_tags(0, 201)),
    ];

    for (label, tags) in &refused {
        let (status, error) = tag_resource(&router, &slo_arn(name), tags).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a tag list the model refuses, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {error}"
        );

        let (_status, after) = list_tags(&router, name).await;
        assert_eq!(
            reported_tags(&after),
            expected_map(&kept),
            "case {label:?}: expected a refused tag to attach nothing, got {:?}",
            reported_tags(&after)
        );
    }
}

/// A tag set belongs to ONE resource: tagging one resource leaves every other resource's
/// identically-keyed tags exactly as they were, and an untag of one is not undone by a tag of
/// another.
#[tokio::test]
async fn tagging_one_resource_leaves_another_resources_tags_alone() {
    let router = router();
    let shared = vec![tag("env", "prod"), tag("team", "payments")];
    for name in ["first-slo", "second-slo", "third-slo"] {
        create_tagged(&router, "three resources", name, &shared).await;
    }

    let (status, body) = tag_resource(
        &router,
        &slo_arn("second-slo"),
        &[tag("env", "staging"), tag("tier", "gold")],
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200 from the tag, got {status} with {body}");

    for name in ["first-slo", "third-slo"] {
        let (status, after) = list_tags(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the list, got {status} with {after}"
        );
        assert_eq!(
            reported_tags(&after),
            expected_map(&shared),
            "case {name:?}: expected an untouched resource to keep {:?}, got {:?}",
            expected_map(&shared),
            reported_tags(&after)
        );
    }

    let (status, after) = list_tags(&router, "second-slo").await;
    assert_eq!(
        status, 200,
        "case second-slo: expected HTTP 200 from the list, got {status} with {after}"
    );
    assert_eq!(
        reported_tags(&after),
        expected_map(&[tag("env", "staging"), tag("team", "payments"), tag("tier", "gold")]),
        "case second-slo: expected the attached and re-valued tags, got {:?}",
        reported_tags(&after)
    );

    // Attaching and detaching address the same per-resource map: a key removed stays removed
    // until it is attached again, and attaching one key does not restore another.
    let (status, body) = untag(&router, &slo_arn("second-slo"), &["team".to_string()]).await;
    assert_eq!(status, 200, "expected HTTP 200 from the untag, got {status} with {body}");
    let (status, body) = tag_resource(&router, &slo_arn("second-slo"), &[tag("tier", "silver")]).await;
    assert_eq!(status, 200, "expected HTTP 200 from the second tag, got {status} with {body}");
    let (_status, after) = list_tags(&router, "second-slo").await;
    assert_eq!(
        reported_tags(&after),
        expected_map(&[tag("env", "staging"), tag("tier", "silver")]),
        "expected the detached key to stay detached, got {:?}",
        reported_tags(&after)
    );
}

/// A `ResourceArn` that names no resource this service holds is refused with the modelled
/// `ResourceNotFoundException` (HTTP 404), identifying the resource by the ARN the request
/// named, and the refused attachment persists nothing — so a resource later created under that
/// name starts with no tags.
///
/// The rows differ along WHY the ARN names nothing: a resource that was never created, one
/// created and then deleted, a bare resource name (which is not an ARN at all, and
/// `ResourceArn` is modelled as an `AmazonResourceName`), another service's ARN, an ARN of this
/// service whose resource segment is not a taggable kind, and another account's ARN.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_and_attaches_nothing() {
    let router = router();
    let kept = vec![tag("env", "prod"), tag("team", "payments")];
    create_tagged(&router, "missing resource", "live-slo", &kept).await;

    create_tagged(&router, "missing resource", "gone-slo", &kept).await;
    let (status, deleted) = call(&router, "DELETE", "/slo/gone-slo", None).await;
    assert_eq!(
        status, 200,
        "expected the setup delete to succeed, got {status} with {deleted}"
    );

    let rows = [
        ("never created", slo_arn("absent-slo")),
        ("created then deleted", slo_arn("gone-slo")),
        ("a bare resource name, not an ARN", "live-slo".to_string()),
        (
            "another service's resource",
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:alarm/my-alarm"),
        ),
        (
            "an ARN with no taggable resource segment",
            format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:service/checkout"),
        ),
        (
            "another account's ARN for the same name",
            format!("arn:aws:application-signals:{REGION}:123456789012:slo/live-slo"),
        ),
    ];

    for (label, resource_arn) in &rows {
        let (status, error) = tag_resource(&router, resource_arn, &[tag("env", "staging")]).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 for an ARN naming no resource, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected a ResourceNotFoundException, got {error}"
        );
        assert_eq!(
            error["ResourceId"].as_str(),
            Some(resource_arn.as_str()),
            "case {label:?}: expected the failure to identify the resource by the ARN the request named ({resource_arn:?}), got {error}"
        );

        // The reader refuses the same ARN identically: both operations address a resource.
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(resource_arn)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 from the read, got {status} with {error}"
        );
    }

    // Nothing the refusals touched: the stored resource still carries exactly its own tags.
    let (status, after) = list_tags(&router, "live-slo").await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the list, got {status} with {after}"
    );
    assert_eq!(
        reported_tags(&after),
        expected_map(&kept),
        "expected a refused tag to change nothing, got {:?}",
        reported_tags(&after)
    );
    assert_eq!(
        error_type(&router, "absent-slo").await,
        Some(SLO_RESOURCE_TYPE.to_string()),
        "expected the refusal about an absent SLO to report the SLO type name"
    );

    // A resource created under a name a refused attachment named starts with no tags: the
    // refusal persisted nothing under that ARN.
    create_tagged(&router, "recreated", "absent-slo", &[]).await;
    let (_status, after) = list_tags(&router, "absent-slo").await;
    assert!(
        reported_tags(&after).is_empty(),
        "expected a newly created resource to carry no tags, got {:?}",
        reported_tags(&after)
    );
}

/// The `ResourceType` the refusal about the SLO called `name` reports.
async fn error_type(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (_status, error) = tag_resource(router, &slo_arn(name), &[tag("env", "staging")]).await;
    error["ResourceType"].as_str().map(ToString::to_string)
}

/// The rule holds for every taggable resource KIND of this service, not only its primary one:
/// an instrumentation configuration is tagged by the ARN its create minted, and the reader
/// reports the merge of the tags the create carried and the ones the attachment added.
#[tokio::test]
async fn tags_attach_to_an_instrumentation_configuration_by_its_own_arn() {
    let router = router();
    let arn = create_configuration(
        &router,
        "one kind beside the other",
        142,
        &[tag("team", "checkout"), tag("env", "prod")],
    )
    .await;

    // One new key and one the create already carried, so both halves of the per-key rule are
    // exercised on this resource kind too.
    let (status, body) = tag_resource(&router, &arn, &[tag("tier", "gold"), tag("env", "staging")]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 tagging {arn}, got {status} with {body}"
    );

    let (status, after) = list_tags_raw(&router, &arn_uri_segment(&arn)).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 reading the configuration's tags, got {status} with {after}"
    );
    let expected: BTreeMap<String, String> = expected_map(&[
        tag("env", "staging"),
        tag("team", "checkout"),
        tag("tier", "gold"),
    ]);
    assert_eq!(
        reported_tags(&after),
        expected,
        "expected the merged tags {expected:?}, got {:?}",
        reported_tags(&after)
    );
}

/// The maximum an attachment enforces is the maximum that resource KIND documents, which is
/// the maximum its CREATE admits: an instrumentation configuration documents none of its own,
/// so it may be created carrying more tags than the SLO maximum and an attachment to it is
/// still accepted — bounded instead by the modelled `TagList` maximum, past which it is the
/// modelled `ServiceQuotaExceededException` (402) and attaches nothing.
#[tokio::test]
async fn a_configuration_is_bounded_by_the_maximum_its_own_kind_documents() {
    let router = router();
    // More tags than the SLO maximum, which this kind's API does not document.
    let created = generated_tags(0, MAX_TAGS_PER_RESOURCE + 10);
    let arn = create_configuration(&router, "above the SLO maximum", 7, &created).await;
    let escaped = arn_uri_segment(&arn);

    let (status, before) = list_tags_raw(&router, &escaped).await;
    assert_eq!(
        status, 200,
        "expected the create to store every tag it carried, got {status} with {before}"
    );
    assert_eq!(
        reported_tags(&before).len(),
        created.len(),
        "expected the configuration to carry {} tags, got {}",
        created.len(),
        reported_tags(&before).len()
    );

    // Re-valuing a key it already carries adds no key, so it is accepted even though the count
    // is already past the maximum the OTHER kind documents.
    let revalued = created
        .first()
        .map(|(key, _)| vec![tag(key, "revalued")])
        .expect("the created set holds at least one tag");
    let (status, body) = tag_resource(&router, &arn, &revalued).await;
    assert_eq!(
        status, 200,
        "case re-valuing a carried key: expected HTTP 200, got {status} with {body}"
    );

    // Filling the modelled maximum exactly is accepted…
    let fill = generated_tags(created.len(), MAX_TAGS_PER_LIST - created.len());
    let (status, body) = tag_resource(&router, &arn, &fill).await;
    assert_eq!(
        status, 200,
        "case filling the modelled maximum: expected HTTP 200, got {status} with {body}"
    );
    let (_status, after) = list_tags_raw(&router, &escaped).await;
    assert_eq!(
        reported_tags(&after).len(),
        MAX_TAGS_PER_LIST,
        "case filling the modelled maximum: expected {MAX_TAGS_PER_LIST} tags, got {}",
        reported_tags(&after).len()
    );

    // …and one key past it is refused, attaching nothing.
    let (status, error) = tag_resource(&router, &arn, &[tag("one-too-many", "x")]).await;
    assert_eq!(
        status, 402,
        "case one key past the modelled maximum: expected HTTP 402, got {status} with {error}"
    );
    assert_eq!(
        error["__type"].as_str(),
        Some("ServiceQuotaExceededException"),
        "case one key past the modelled maximum: expected a ServiceQuotaExceededException, got {error}"
    );
    let (_status, unchanged) = list_tags_raw(&router, &escaped).await;
    assert_eq!(
        reported_tags(&unchanged),
        reported_tags(&after),
        "case one key past the modelled maximum: expected a refused attachment to attach nothing, got {:?}",
        reported_tags(&unchanged)
    );
}

/// Create one instrumentation configuration carrying `tags` at `line_number` (the location is
/// part of a configuration's identity, so each row instruments a different line), and hand back
/// the ARN the create minted.
async fn create_configuration(
    router: &MultiServiceRouter,
    label: &str,
    line_number: u32,
    tags: &[(String, String)],
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(json!({
            "InstrumentationType": "PROBE",
            "Service": "checkout",
            "Environment": "eks:cluster-prod/namespace",
            "SignalType": "SNAPSHOT",
            "Location": {
                "CodeLocation": {
                    "Language": "Java",
                    "FilePath": "src/main/java/com/example/CartService.java",
                    "LineNumber": line_number,
                }
            },
            "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } },
            "Tags": tag_list(tags),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the setup create to succeed, got {status} with {created}"
    );
    let arn = created["ARN"].as_str().unwrap_or_default().to_string();
    assert!(
        !arn.is_empty(),
        "case {label:?}: expected the create to answer the configuration's ARN, got {created}"
    );
    arn
}
