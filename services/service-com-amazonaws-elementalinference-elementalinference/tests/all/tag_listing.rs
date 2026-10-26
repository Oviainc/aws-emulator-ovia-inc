// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, end to end over the wire: which tag set it reports, and what
//! it refuses.
//!
//! Every case drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the request, the handler runs, and
//! the response is serialized as `restJson1` — so what these tests assert is what a
//! client sees. The `resourceArn` is an `@httpLabel`, so each request carries it
//! percent-encoded in the path exactly as a client sends it ([`tags_path`]).
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * the reported map IS the tag set stored on the resource — the one record `GetFeed` /
//!   `GetDictionary` report and `TagResource` / `UntagResource` write — so a listing
//!   agrees with the resource's own reader and follows every write and delete, on either
//!   kind of resource;
//! * the tag set is a property every resource HAS, so a resource carrying no tag reports
//!   an EMPTY map rather than an absent member;
//! * the operation is `@readonly`: listing a resource's tags changes nothing a later
//!   caller sees, so the resource reads back identically afterwards and a repeated
//!   listing reports the same map;
//! * a `resourceArn` identifies a resource WHOLE — kind, id, region and account — so any
//!   other well-formed ARN, including one naming another Region's resource, names no
//!   resource of the endpoint the request reached and earns a 404 carrying the error code
//!   `NotFoundException` (the code the recorded refusal of this route carries) and this
//!   service's one not-found sentence;
//! * a resource whose deletion has been ACCEPTED is still a resource the store holds, so
//!   its tags are still reported, right up to the instant its removal falls due — from
//!   when its ARN names nothing.

use crate::harness::{
    call, call_in_region, create_tagged_resource, create_tagged_resource_in, expected_tags, pair,
    past_the_transition_span, read_resource, read_resource_in, router, tag_object, tags_of,
    tags_path, unresolvable_arns, untag_resource, TAGGABLE_KINDS,
};
use crate::region_isolation::REGIONS;
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::ops::list_tags_for_resource::NOT_FOUND_MESSAGE;
use service_com_amazonaws_elementalinference_elementalinference::state::{
    MAX_TAGS_PER_RESOURCE, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

/// The error code a not-found refusal of this route carries — the code recorded for it,
/// which the model declares on no shape, so it is named here rather than read off a
/// generated shape.
const NOT_FOUND_CODE: &str = "NotFoundException";

/// `GET /v1/tags/{resourceArn}` — the one route this suite lists tags through.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(router, "GET", &tags_path(resource_arn), None).await
}

/// As [`list_tags`], through `region`'s endpoint — for the cases that assert which
/// Region owns the resource a listing names.
async fn list_tags_in(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
) -> (u16, Value) {
    call_in_region(router, region, "GET", &tags_path(resource_arn), None).await
}

/// `POST /v1/tags/{resourceArn}` carrying `tags`: the write whose result a listing has
/// to report.
async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &tags_path(resource_arn),
        Some(json!({"tags": tag_object(tags)})),
    )
    .await
}

/// The map a listing reports, asserted to be a 200 first.
async fn listed_tags(
    router: &MultiServiceRouter,
    resource_arn: &str,
    label: &str,
) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "case {label:?}: ListTagsForResource answers its modelled 200, got {status} with {body}"
    );
    tags_of(&body)
}

/// One row of the tag-set table: what the resource carries, and the label the assertions
/// name the row by.
struct TagSetCase {
    label: &'static str,
    tags: Vec<(String, String)>,
}

/// Rows spanning what "report the stored tag set" generalises over: how many tags the
/// resource carries (none, one, several, the published cap's worth), and the shape of a
/// key and a value.
///
/// The key and value lengths come from the model, not from a recorded request: `TagMap`'s
/// key is a `TagKey` of `@length` 1..128 and its value a `TagValue` of `@length` max 256
/// (`smithy_query(query="shape",shape="TagMap")`), so the rows below include both ends of
/// each; the count's bound is this service's own [`MAX_TAGS_PER_RESOURCE`].
fn tag_set_cases() -> Vec<TagSetCase> {
    vec![
        TagSetCase {
            label: "a resource carrying no tag at all",
            tags: vec![],
        },
        TagSetCase {
            label: "a resource carrying one tag",
            tags: vec![pair("env", "prod")],
        },
        TagSetCase {
            label: "a resource carrying several tags",
            tags: vec![pair("env", "dev"), pair("team", "video"), pair("owner", "ops")],
        },
        TagSetCase {
            label: "the shortest key and an empty value the model admits",
            tags: vec![pair(&"k".repeat(TAG_KEY_MIN_LENGTH), "")],
        },
        TagSetCase {
            label: "the longest key and value the model admits",
            tags: vec![("k".repeat(TAG_KEY_MAX_LENGTH), "v".repeat(256))],
        },
        TagSetCase {
            label: "a punctuated key and a non-ASCII value",
            tags: vec![pair("project:alpha.1_x", "München / 東京")],
        },
        TagSetCase {
            label: "the published cap's worth of tags",
            tags: (0..MAX_TAGS_PER_RESOURCE)
                .map(|index| pair(&format!("key{index}"), &format!("value{index}")))
                .collect(),
        },
    ]
}

/// The reported map is the tag set the resource carries — and the same map the
/// resource's own reader reports.
#[tokio::test]
async fn the_listing_reports_the_tag_set_stored_on_the_resource() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        for (index, case) in tag_set_cases().into_iter().enumerate() {
            let label = format!("{} ({kind})", case.label);
            let (id, arn) =
                create_tagged_resource(&router, kind, &format!("listtags{kind}{index}"), &case.tags)
                    .await;

            let listed = listed_tags(&router, &arn, &label).await;
            assert_eq!(
                listed,
                expected_tags(&case.tags),
                "case {label:?}: the listing reports exactly the stored tag set"
            );

            let (status, stored) = read_resource(&router, kind, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the resource reads back, got {status} with {stored}"
            );
            assert_eq!(
                listed,
                tags_of(&stored),
                "case {label:?}: the listing and the resource's own reader report one tag set, \
                 got {stored}"
            );
        }
    }
}

/// One row of the write table: the write to make, and the tag set the listing must
/// report afterwards.
struct WriteCase {
    label: &'static str,
    set: Vec<(String, String)>,
    delete: Vec<&'static str>,
    expected: Vec<(String, String)>,
}

/// Rows spanning the writes a tag set can take, starting from the set
/// [`initial_tags`] returns: a new key, an overwritten key, several keys at once, a delete of a key the resource holds, a
/// delete of one it does not, and both at once.
fn write_cases() -> Vec<WriteCase> {
    vec![
        WriteCase {
            label: "a new key is added",
            set: vec![pair("owner", "ops")],
            delete: vec![],
            expected: vec![pair("env", "dev"), pair("team", "video"), pair("owner", "ops")],
        },
        WriteCase {
            label: "an existing key is overwritten",
            set: vec![pair("env", "prod")],
            delete: vec![],
            expected: vec![pair("env", "prod"), pair("team", "video")],
        },
        WriteCase {
            label: "several keys at once",
            set: vec![pair("a", "1"), pair("b", "2")],
            delete: vec![],
            expected: vec![
                pair("env", "dev"),
                pair("team", "video"),
                pair("a", "1"),
                pair("b", "2"),
            ],
        },
        WriteCase {
            label: "a key the resource holds is deleted",
            set: vec![],
            delete: vec!["team"],
            expected: vec![pair("env", "dev")],
        },
        WriteCase {
            label: "a key the resource does not hold is deleted",
            set: vec![],
            delete: vec!["absent"],
            expected: vec![pair("env", "dev"), pair("team", "video")],
        },
        WriteCase {
            label: "a key is added and another deleted",
            set: vec![pair("owner", "ops")],
            delete: vec!["env"],
            expected: vec![pair("team", "video"), pair("owner", "ops")],
        },
        WriteCase {
            label: "every key the resource holds is deleted",
            set: vec![],
            delete: vec!["env", "team"],
            expected: vec![],
        },
    ]
}

/// The tag set every row of [`write_cases`] starts from.
fn initial_tags() -> Vec<(String, String)> {
    vec![pair("env", "dev"), pair("team", "video")]
}

/// The listing follows every tag write and delete, because it reports the set those
/// operations wrote.
#[tokio::test]
async fn the_listing_follows_every_tag_write_and_delete() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        for (index, case) in write_cases().into_iter().enumerate() {
            let label = format!("{} ({kind})", case.label);
            let (_, arn) = create_tagged_resource(
                &router,
                kind,
                &format!("writetags{kind}{index}"),
                &initial_tags(),
            )
            .await;

            if !case.set.is_empty() {
                let (status, body) = tag_resource(&router, &arn, &case.set).await;
                assert_eq!(
                    status, 204,
                    "case {label:?}: the tag write is accepted, got {status} with {body}"
                );
            }
            if !case.delete.is_empty() {
                let keys: Vec<String> = case.delete.iter().map(|key| (*key).to_string()).collect();
                let (status, body) = untag_resource(&router, &arn, &keys).await;
                assert_eq!(
                    status, 204,
                    "case {label:?}: the tag delete is accepted, got {status} with {body}"
                );
            }

            assert_eq!(
                listed_tags(&router, &arn, &label).await,
                expected_tags(&case.expected),
                "case {label:?}: the listing reports the set the writes left behind"
            );
        }
    }
}

/// Listing tags is a REPORT: it changes nothing a later caller sees, and repeating it
/// reports the same map.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let tags = initial_tags();
        let (id, arn) =
            create_tagged_resource(&router, kind, &format!("readonly{kind}"), &tags).await;
        // Past the span a clock-owed lifecycle move takes, so the record this case
        // compares is a SETTLED one: what changes across the listings below is then only
        // what the listings themselves did.
        past_the_transition_span();

        let (status, before) = read_resource(&router, kind, &id).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the resource reads back before the listing, got {status} with {before}"
        );

        let first = listed_tags(&router, &arn, kind).await;
        let second = listed_tags(&router, &arn, kind).await;
        assert_eq!(
            (&first, &second),
            (&expected_tags(&tags), &expected_tags(&tags)),
            "case {kind:?}: both listings report the stored tag set"
        );

        let (status, after) = read_resource(&router, kind, &id).await;
        assert_eq!(
            (status, &after),
            (200, &before),
            "case {kind:?}: the resource reads back unchanged after being listed, got {after}"
        );
    }
}

/// A `resourceArn` that is not the ARN a stored resource of this endpoint carries names
/// nothing.
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_not_found() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let tags = initial_tags();
        let (_, arn) = create_tagged_resource(&router, kind, &format!("misstags{kind}"), &tags).await;

        for case in unresolvable_arns(kind, &arn) {
            let label = format!("{} ({kind})", case.label);
            let (status, error) = list_tags(&router, &case.arn).await;
            assert_eq!(
                status, 404,
                "case {label:?}: an ARN naming no stored resource is refused 404, got {status} \
                 with {error}"
            );
            assert_eq!(
                error["__type"],
                json!(NOT_FOUND_CODE),
                "case {label:?}: the refusal carries this route's not-found error code, got \
                 {error}"
            );
            assert_eq!(
                error["message"],
                json!(NOT_FOUND_MESSAGE),
                "case {label:?}: the refusal carries this route's not-found sentence, got \
                 {error}"
            );
        }

        // The refusals are about the ARN, not about the route: the real ARN still lists.
        assert_eq!(
            listed_tags(&router, &arn, kind).await,
            expected_tags(&tags),
            "case {kind:?}: the resource's own ARN still lists its tags"
        );
    }
}

/// A listing reports the tags of a resource of the endpoint it reached, and of no other
/// Region's resource.
#[tokio::test]
async fn a_listing_reports_only_the_resources_of_the_signed_for_region() {
    let router = router();

    for owner in REGIONS {
        for kind in TAGGABLE_KINDS {
            let tags = vec![pair("owner", owner)];
            let (id, arn) = create_tagged_resource_in(
                &router,
                owner,
                kind,
                &format!("scopetags{kind}"),
                &tags,
            )
            .await;

            for other in REGIONS.iter().filter(|region| **region != owner) {
                let label = format!("{kind} of {owner}, listed through {other}");
                let (status, error) = list_tags_in(&router, other, &arn).await;
                assert_eq!(
                    (status, &error["__type"]),
                    (404, &json!(NOT_FOUND_CODE)),
                    "case {label:?}: another Region's endpoint reaches no resource of this one, \
                     got {status} with {error}"
                );
            }

            let (status, body) = list_tags_in(&router, owner, &arn).await;
            assert_eq!(
                status, 200,
                "case {kind} of {owner:?}: its own endpoint lists its tags, got {status} with \
                 {body}"
            );
            assert_eq!(
                tags_of(&body),
                expected_tags(&tags),
                "case {kind} of {owner:?}: the listing reports the set it was created with, got \
                 {body}"
            );

            let (_, stored) = read_resource_in(&router, owner, kind, &id).await;
            assert_eq!(
                tags_of(&stored),
                expected_tags(&tags),
                "case {kind} of {owner:?}: no foreign listing disturbed the record, got {stored}"
            );
        }
    }
}

/// A resource whose deletion has been accepted still lists its tags; once the removal
/// falls due its ARN names nothing.
#[tokio::test]
async fn a_pending_deletion_still_lists_its_tags_until_the_removal_falls_due() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let tags = initial_tags();
        let (id, arn) =
            create_tagged_resource(&router, kind, &format!("gonetags{kind}"), &tags).await;
        let (status, accepted) = call(&router, "DELETE", &format!("/v1/{kind}/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {kind:?}: the deletion is accepted with 202, got {status} with {accepted}"
        );

        assert_eq!(
            listed_tags(&router, &arn, kind).await,
            expected_tags(&tags),
            "case {kind:?}: a resource on its way out still reports its tags"
        );

        past_the_transition_span();
        let (status, error) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, &error["__type"]),
            (404, &json!(NOT_FOUND_CODE)),
            "case {kind:?}: a removed resource's ARN names nothing, got {status} with {error}"
        );
    }
}
