// SPDX-License-Identifier: CC0-1.0

//! `TagResource`, end to end over the wire: what tagging a feed or a dictionary leaves
//! behind, and what it refuses.
//!
//! Every case drives the real request path — the router picks the service from the SigV4
//! credential scope, the framework deserializes and validates the body, the handler
//! runs, and the response is serialized as `restJson1` — so what these tests assert is
//! what a client sees. The `resourceArn` is an `@httpLabel`, so each request carries it
//! percent-encoded in the path exactly as a client sends it ([`tags_path`]).
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a tag write MERGES into the tag set stored on the resource: every key the request
//!   carries takes the value it carries, and "if existing tags on a resource are not
//!   specified in the request parameters, they are not changed" — so the same request
//!   twice leaves the same set (the operation is `@idempotent`), and the next read of
//!   the resource reports it, because the tags live on the one record `GetFeed` /
//!   `GetDictionary` report;
//! * a `resourceArn` identifies a resource WHOLE — kind, id, region and account — so
//!   any other well-formed ARN names no resource of this store and earns the modelled
//!   `ResourceNotFoundException` (HTTP 404) and no write;
//! * a resource whose deletion has been accepted takes no tag: its tags go with it when
//!   the removal falls due, so the write is refused with the modelled
//!   `ConflictException` (HTTP 409) while the deletion is pending and with
//!   `ResourceNotFoundException` once it has completed;
//! * a tag set is BOUNDED at [`MAX_TAGS_PER_RESOURCE`]: a request that would take the
//!   resource past it is refused with the modelled `ValidationException` (HTTP 400),
//!   while one that only re-sets keys the resource already holds is accepted at the
//!   bound;
//! * a key or value outside the bounds `TagMap` models is refused with that same
//!   `ValidationException` and writes nothing — the VALUE by the framework's boundary
//!   walk, the KEY by the handler, since that walk reads a map entry's key as the path
//!   its value is reported under rather than checking it.

use crate::harness::{
    call, create_tagged_resource, expected_tags, pair, past_the_transition_span, read_resource,
    router, tag_object, tags_of, tags_path, unresolvable_arns, TAGGABLE_KINDS,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    invalid_tag_key_message, too_many_tags_message, MAX_TAGS_PER_RESOURCE, NOT_FOUND_MESSAGE,
    TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};
use smithy_server_runtime::MultiServiceRouter;

/// `POST /v1/tags/{resourceArn}` carrying `tags`.
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

/// One row of the merge table: what the resource already carried, what the request
/// carries, and the set the merge must leave behind.
struct MergeCase {
    label: &'static str,
    initial: Vec<(String, String)>,
    request: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// Rows spanning what the merge rule generalises over: how many tags the resource
/// already held, how many the request carries, whether the request's keys are new or
/// already present, and the shape of the key and the value.
///
/// The key and value lengths come from the model, not from a recorded request:
/// `TagMap`'s key is a `TagKey` of `@length` 1..128 and its value a `TagValue` of
/// `@length` max 256 (`smithy_query(query="shape",shape="TagMap")`), so the rows below
/// include both ends of each.
fn merge_cases() -> Vec<MergeCase> {
    let longest_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let longest_value = "v".repeat(256);
    vec![
        MergeCase {
            label: "an untagged resource takes its first tag",
            initial: vec![],
            request: vec![pair("env", "prod")],
            expected: vec![pair("env", "prod")],
        },
        MergeCase {
            label: "a key the resource already holds is overwritten",
            initial: vec![pair("env", "dev")],
            request: vec![pair("env", "prod")],
            expected: vec![pair("env", "prod")],
        },
        MergeCase {
            label: "keys the request does not name are left alone",
            initial: vec![pair("env", "dev"), pair("team", "video")],
            request: vec![pair("owner", "ops")],
            expected: vec![pair("env", "dev"), pair("team", "video"), pair("owner", "ops")],
        },
        MergeCase {
            label: "several keys at once, one of them an overwrite",
            initial: vec![pair("a", "1")],
            request: vec![pair("a", "2"), pair("b", "3"), pair("c", "4")],
            expected: vec![pair("a", "2"), pair("b", "3"), pair("c", "4")],
        },
        MergeCase {
            label: "a request carrying no tag at all changes nothing",
            initial: vec![pair("env", "dev")],
            request: vec![],
            expected: vec![pair("env", "dev")],
        },
        MergeCase {
            label: "an empty value is a value (TagValue declares no minimum length)",
            initial: vec![],
            request: vec![pair("blank", "")],
            expected: vec![pair("blank", "")],
        },
        MergeCase {
            label: "the shortest key the model admits",
            initial: vec![pair("env", "dev")],
            request: vec![pair("k", "v")],
            expected: vec![pair("env", "dev"), pair("k", "v")],
        },
        MergeCase {
            label: "the longest key and value the model admits",
            initial: vec![],
            request: vec![(longest_key.clone(), longest_value.clone())],
            expected: vec![(longest_key, longest_value)],
        },
        MergeCase {
            label: "a punctuated key and a non-ASCII value",
            initial: vec![pair("env", "dev")],
            request: vec![pair("project:alpha.1_x", "München / 東京")],
            expected: vec![pair("env", "dev"), pair("project:alpha.1_x", "München / 東京")],
        },
    ]
}

/// A tag write merges into the stored set, is visible to the next read of the resource,
/// and is idempotent — on either kind of resource.
#[tokio::test]
async fn tags_merge_into_the_resource_and_are_visible_to_the_next_read() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        for (index, case) in merge_cases().into_iter().enumerate() {
            let label = format!("{} on a {kind}", case.label);
            let (id, arn) =
                create_tagged_resource(&router, kind, &format!("tag{kind}{index}"), &case.initial)
                    .await;

            let (status, body) = tag_resource(&router, &arn, &case.request).await;
            assert_eq!(
                status, 204,
                "case {label:?}: TagResource answers its modelled 204, got {status} with {body}"
            );
            assert_eq!(
                body,
                Value::Null,
                "case {label:?}: a 204 carries no body, got {body}"
            );

            let (status, stored) = read_resource(&router, kind, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the tagged {kind} still reads back, got {status} with {stored}"
            );
            assert_eq!(
                tags_of(&stored),
                expected_tags(&case.expected),
                "case {label:?}: the merged tag set is what the next read reports, got {stored}"
            );

            // `@idempotent`: the same request again leaves the same set.
            let (status, body) = tag_resource(&router, &arn, &case.request).await;
            assert_eq!(
                status, 204,
                "case {label:?}: repeating the tag write answers 204, got {status} with {body}"
            );
            let (_, repeated) = read_resource(&router, kind, &id).await;
            assert_eq!(
                tags_of(&repeated),
                expected_tags(&case.expected),
                "case {label:?}: repeating the tag write changes nothing, got {repeated}"
            );
        }
    }
}

/// A `resourceArn` that is not the ARN a stored resource carries names nothing, and
/// nothing is written.
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_not_found() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let initial = vec![pair("env", "dev")];
        let (id, arn) =
            create_tagged_resource(&router, kind, &format!("miss{kind}"), &initial).await;

        for case in unresolvable_arns(kind, &arn) {
            let label = format!("{} ({kind})", case.label);
            let (status, error) = tag_resource(&router, &case.arn, &[pair("added", "yes")]).await;
            assert_eq!(
                status, 404,
                "case {label:?}: an ARN naming no stored resource is refused 404, got {status} \
                 with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ResourceNotFoundException"),
                "case {label:?}: the refusal is the modelled ResourceNotFoundException, got \
                 {error}"
            );
            assert_eq!(
                error["message"],
                json!(NOT_FOUND_MESSAGE),
                "case {label:?}: the refusal carries this service's one not-found message, got \
                 {error}"
            );

            let (_, stored) = read_resource(&router, kind, &id).await;
            assert_eq!(
                tags_of(&stored),
                expected_tags(&initial),
                "case {label:?}: a refused tag write leaves the real resource untouched, got \
                 {stored}"
            );
        }
    }
}

/// A resource whose deletion has been accepted takes no tag: a conflict while the
/// removal is pending, and not found once it has fallen due.
#[tokio::test]
async fn a_resource_being_deleted_takes_no_tag() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let initial = vec![pair("env", "dev")];
        let (id, arn) =
            create_tagged_resource(&router, kind, &format!("gone{kind}"), &initial).await;
        let (status, deleted) = call(&router, "DELETE", &format!("/v1/{kind}/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {kind:?}: the deletion is accepted with 202, got {status} with {deleted}"
        );

        let (status, error) = tag_resource(&router, &arn, &[pair("added", "yes")]).await;
        assert_eq!(
            status, 409,
            "case {kind:?}: tagging a resource whose deletion is pending conflicts, got {status} \
             with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ConflictException"),
            "case {kind:?}: the refusal is the modelled ConflictException, got {error}"
        );

        let (_, pending) = read_resource(&router, kind, &id).await;
        assert_eq!(
            tags_of(&pending),
            expected_tags(&initial),
            "case {kind:?}: the refused write left the tag set alone, got {pending}"
        );

        // Once the removal falls due the resource is gone, so its ARN names nothing.
        past_the_transition_span();
        let (status, error) = tag_resource(&router, &arn, &[pair("added", "yes")]).await;
        assert_eq!(
            (status, &error["__type"]),
            (404, &json!("ResourceNotFoundException")),
            "case {kind:?}: a removed resource's ARN names nothing, got {status} with {error}"
        );
    }
}

/// The tag set is bounded: a request is accepted up to the cap, a request that would
/// take the resource past it is refused, and an overwrite at the cap still succeeds.
#[tokio::test]
async fn the_tag_set_is_bounded_at_the_published_cap() {
    let router = router();
    let tag = |index: usize| pair(&format!("key{index}"), &format!("value{index}"));

    for kind in TAGGABLE_KINDS {
        // One short of the cap, so the next single tag lands exactly on it.
        let initial: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE - 1).map(tag).collect();
        let (id, arn) =
            create_tagged_resource(&router, kind, &format!("cap{kind}"), &initial).await;

        let at_the_cap = vec![tag(MAX_TAGS_PER_RESOURCE - 1)];
        let (status, body) = tag_resource(&router, &arn, &at_the_cap).await;
        assert_eq!(
            status, 204,
            "case {kind:?}: a tag write landing exactly on the cap is accepted, got {status} \
             with {body}"
        );
        let (_, stored) = read_resource(&router, kind, &id).await;
        assert_eq!(
            tags_of(&stored).len(),
            MAX_TAGS_PER_RESOURCE,
            "case {kind:?}: the resource now carries the cap's worth of tags, got {stored}"
        );

        // One key past it: refused, and the stored set is unchanged.
        let past_the_cap = vec![tag(MAX_TAGS_PER_RESOURCE)];
        let (status, error) = tag_resource(&router, &arn, &past_the_cap).await;
        assert_eq!(
            status, 400,
            "case {kind:?}: a tag write past the cap is refused 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {kind:?}: the refusal is the modelled ValidationException, got {error}"
        );
        assert_eq!(
            error["message"],
            json!(too_many_tags_message(MAX_TAGS_PER_RESOURCE + 1)),
            "case {kind:?}: the refusal states the count and the cap, got {error}"
        );

        // An overwrite of a key the resource already holds adds nothing to the count,
        // so it is accepted even with the set full.
        let overwrite = vec![pair("key0", "rewritten")];
        let (status, body) = tag_resource(&router, &arn, &overwrite).await;
        assert_eq!(
            status, 204,
            "case {kind:?}: re-setting an existing key at the cap is accepted, got {status} \
             with {body}"
        );
        let (_, stored) = read_resource(&router, kind, &id).await;
        let tags = tags_of(&stored);
        assert_eq!(
            (tags.len(), tags.get("key0").map(String::as_str)),
            (MAX_TAGS_PER_RESOURCE, Some("rewritten")),
            "case {kind:?}: the overwrite replaced a value without growing the set, got {stored}"
        );
    }
}

/// One row of the modelled-bounds table: a tag key/value pair, whether `TagMap`'s own
/// `@length` constraints admit it, and the refusal message where the rule that produces
/// it is this service's own.
struct BoundCase {
    label: &'static str,
    key: String,
    value: String,
    admitted: bool,
    message: Option<String>,
}

/// A row per published bound of `TagMap`'s key and value, and one past each: the key is
/// a `TagKey` of `@length` 1..128 and the value a `TagValue` of `@length` max 256
/// (`smithy_query(query="shape",shape="TagMap")`).
///
/// A key's bounds are the handler's to enforce — the boundary walk captures a map entry's
/// key as the path its VALUE is reported under rather than checking it — so those rows
/// pin the message this service builds. A value past its maximum is refused by the
/// boundary, whose envelope echoes the value, so that row pins only the status and the
/// code.
fn bound_cases() -> Vec<BoundCase> {
    vec![
        BoundCase {
            label: "a key at the minimum length",
            key: "k".repeat(TAG_KEY_MIN_LENGTH),
            value: "v".to_string(),
            admitted: true,
            message: None,
        },
        BoundCase {
            label: "a key at the maximum length",
            key: "k".repeat(TAG_KEY_MAX_LENGTH),
            value: "v".to_string(),
            admitted: true,
            message: None,
        },
        BoundCase {
            label: "a key one character past the maximum",
            key: "k".repeat(TAG_KEY_MAX_LENGTH + 1),
            value: "v".to_string(),
            admitted: false,
            message: Some(invalid_tag_key_message(TAG_KEY_MAX_LENGTH + 1)),
        },
        BoundCase {
            label: "an empty key, one short of the minimum",
            key: String::new(),
            value: "v".to_string(),
            admitted: false,
            message: Some(invalid_tag_key_message(TAG_KEY_MIN_LENGTH - 1)),
        },
        BoundCase {
            label: "a non-ASCII key at the maximum length, counted in characters",
            // `@length` counts characters, not bytes: 128 three-byte characters are a
            // key the model admits, and 129 of them are not.
            key: "東".repeat(TAG_KEY_MAX_LENGTH),
            value: "v".to_string(),
            admitted: true,
            message: None,
        },
        BoundCase {
            label: "a non-ASCII key one character past the maximum",
            key: "東".repeat(TAG_KEY_MAX_LENGTH + 1),
            value: "v".to_string(),
            admitted: false,
            message: Some(invalid_tag_key_message(TAG_KEY_MAX_LENGTH + 1)),
        },
        BoundCase {
            label: "a value at the maximum length",
            key: "boundedvalue".to_string(),
            value: "v".repeat(256),
            admitted: true,
            message: None,
        },
        BoundCase {
            label: "a value one character past the maximum",
            // Its own key, so "nothing was written" is about THIS row's pair.
            key: "oversizevalue".to_string(),
            value: "v".repeat(257),
            admitted: false,
            message: None,
        },
    ]
}

/// A key or value outside the bounds the model declares is refused, and writes nothing.
#[tokio::test]
async fn tag_keys_and_values_follow_the_modelled_bounds() {
    let router = router();

    for kind in TAGGABLE_KINDS {
        let (id, arn) = create_tagged_resource(&router, kind, &format!("bound{kind}"), &[]).await;

        for case in bound_cases() {
            let label = format!("{} on a {kind}", case.label);
            let request = vec![(case.key.clone(), case.value.clone())];
            let (status, body) = tag_resource(&router, &arn, &request).await;
            let (_, stored) = read_resource(&router, kind, &id).await;
            let tags = tags_of(&stored);

            if case.admitted {
                assert_eq!(
                    status, 204,
                    "case {label:?}: a pair inside the modelled bounds is accepted, got {status} \
                     with {body}"
                );
                assert_eq!(
                    tags.get(&case.key).map(String::as_str),
                    Some(case.value.as_str()),
                    "case {label:?}: the accepted pair is stored, got {stored}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {label:?}: a pair outside the modelled bounds is refused 400, got \
                     {status} with {body}"
                );
                assert_eq!(
                    body["__type"],
                    json!("ValidationException"),
                    "case {label:?}: that 400 is a ValidationException, got {body}"
                );
                if let Some(message) = &case.message {
                    assert_eq!(
                        body["message"],
                        json!(message),
                        "case {label:?}: expected {message:?}, got {body}"
                    );
                }
                assert!(
                    !tags.contains_key(&case.key),
                    "case {label:?}: a refused pair writes nothing, got {stored}"
                );
            }
        }
    }
}
