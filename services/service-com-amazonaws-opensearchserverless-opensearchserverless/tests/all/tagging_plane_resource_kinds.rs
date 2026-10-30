// SPDX-License-Identifier: CC0-1.0

//! The tagging plane addresses a RESOURCE by the KIND its ARN names, end to end over the
//! wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — through the harness `collection_lifecycle`
//! owns, so the boundary validator, the protocol codec and the handler all take part
//! exactly as they do for an SDK client.
//!
//! The rule under test: `resourceArn` is "the ARN of the resource"
//! (`smithy_query(query="shape", shape="TagResourceRequest")`, which names no resource
//! kind), so every resource kind this service both issues an ARN for and models `tags` on
//! is resolved by all three tagging operations, whatever call set those tags; the tag set a
//! tag read reports is the one the resource's own read reports; and the per-resource tag cap
//! is held for each kind, its refusal naming the resource the ARN addresses.
//!
//! The table spans what the rule generalises over — the resource KIND the ARN addresses,
//! and the CALL the tags under test came from (the create request, a later `TagResource`,
//! a later `UntagResource`) — rather than the one group the issue quoted. The tag sets
//! themselves differ along count, value bytes and the omitted member.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
    ARN_NAMESPACE, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// One TAGGABLE resource kind of this service: a kind whose create request models `tags`
/// and whose ARN this service hands the caller
/// (`smithy_query(query="shape", shape="CreateCollectionRequest,CreateCollectionGroupRequest")`
/// — the only two requests of this service that model `tags`).
struct Kind {
    /// How a row names this kind.
    case: &'static str,
    /// The prefix every resource this kind creates is named with, inside
    /// `CollectionGroupName`'s 3..32 bound, the narrower of the two names.
    name_prefix: &'static str,
    /// The create operation, and the members it needs beside `name` and `tags`.
    create: &'static str,
    create_extra: Value,
    /// The create response member carrying the created resource's detail.
    create_detail: &'static str,
    /// Whether that detail shape models `tags`: `CreateCollectionGroupDetail` does,
    /// `CreateCollectionDetail` does not
    /// (`smithy_query(query="shape", shape="CreateCollectionDetail,CreateCollectionGroupDetail")`).
    create_reports_tags: bool,
    /// The batch read of this kind, and the list member its details come back in.
    read: &'static str,
    read_details: &'static str,
    /// Whether the read's detail shape models `tags`: `CollectionGroupDetail` does,
    /// `CollectionDetail` does not
    /// (`smithy_query(query="shape", shape="CollectionDetail,CollectionGroupDetail")`).
    read_reports_tags: bool,
    /// The resource segment this kind's ARN carries, which is also the `resourceType` a
    /// quota refusal names.
    resource_type: &'static str,
}

/// Every taggable resource kind, in the order the suite exercises them.
fn taggable_kinds() -> Vec<Kind> {
    vec![
        Kind {
            case: "a collection",
            name_prefix: "tagged-coll",
            create: "CreateCollection",
            create_extra: json!({ "encryptionConfig": { "aWSOwnedKey": true } }),
            create_detail: "createCollectionDetail",
            create_reports_tags: false,
            read: "BatchGetCollection",
            read_details: "collectionDetails",
            read_reports_tags: false,
            resource_type: "collection",
        },
        Kind {
            case: "a collection group",
            name_prefix: "tagged-group",
            create: "CreateCollectionGroup",
            create_extra: json!({ "standbyReplicas": "ENABLED" }),
            create_detail: "createCollectionGroupDetail",
            create_reports_tags: true,
            read: "BatchGetCollectionGroup",
            read_details: "collectionGroupDetails",
            read_reports_tags: true,
            resource_type: "collection-group",
        },
    ]
}

/// A tag list as the map it is keyed by: `Tag$key` is what a tag is addressed by, so the
/// comparison does not depend on the order the tags were written in.
fn tag_map(tags: &Value) -> BTreeMap<String, String> {
    tags.as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| (text(tag, "key"), text(tag, "value")))
                .collect()
        })
        .unwrap_or_default()
}

/// Create a resource of `kind` named `name`, carrying `tags` when the row names a tag list
/// at all; answer the create's own detail.
async fn create(
    router: &MultiServiceRouter,
    kind: &Kind,
    name: &str,
    tags: Option<&Value>,
) -> Value {
    let mut body = kind.create_extra.clone();
    body["name"] = json!(name);
    if let Some(tags) = tags {
        body["tags"] = tags.clone();
    }
    let request = body.to_string();
    let operation = kind.create;
    let (status, code, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}({request}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, kind.create_detail).clone()
}

/// The tag set `ListTagsForResource` reports for `arn`.
async fn listed(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, code, answer) =
        call(router, "ListTagsForResource", json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource({case}): expected 200, got {status} {code} with {answer}"
    );
    tag_map(at(&answer, "tags"))
}

/// The tag set the RESOURCE's own read reports for `id` — the read whose answer a tag read
/// must agree with.
async fn reported(
    router: &MultiServiceRouter,
    kind: &Kind,
    id: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let operation = kind.read;
    let (status, code, answer) = call(router, operation, json!({ "ids": [id] })).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    let detail = at(&answer, kind.read_details)
        .as_array()
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null);
    tag_map(at(&detail, "tags"))
}

/// Send `operation` against `arn` with `members`, asserting it was accepted.
async fn accepted(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    case: &str,
    members: Value,
) {
    let mut request = members;
    request["resourceArn"] = json!(arn);
    let (status, code, answer) = call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
}

#[tokio::test]
async fn tags_of_every_taggable_kind_are_read_written_and_removed_through_the_tagging_plane() {
    let router = emulator();

    // The tag sets a create may carry: one key, several keys with multi-byte values that
    // must survive byte for byte, the empty list the model's `@length(min: 0)` admits, a
    // value at that same empty-string minimum, and no `tags` member at all.
    let created_with: Vec<(&str, Option<Value>)> = vec![
        ("one tag", Some(json!([{ "key": "team", "value": "search" }]))),
        (
            "two tags, one with a multi-byte value",
            Some(json!([
                { "key": "team", "value": "search" },
                { "key": "owner", "value": "ベクトル検索 🌐 — naïve" },
            ])),
        ),
        ("an empty tag list", Some(json!([]))),
        (
            "a value at the model's empty-string minimum",
            Some(json!([{ "key": "team", "value": "" }])),
        ),
        ("no tags member at all", None),
    ];

    for kind in taggable_kinds() {
        for (index, (row, tags)) in created_with.iter().enumerate() {
            let case = format!("{} created with {row}", kind.case);
            let name = format!("{}-{index}", kind.name_prefix);
            let detail = create(&router, &kind, &name, tags.as_ref()).await;
            let arn = text(&detail, "arn");
            let id = text(&detail, "id");
            assert!(
                arn.ends_with(&format!(":{}/{id}", kind.resource_type)),
                "case {case}: the ARN the create returns must address the resource by its \
                 kind and id, got {arn:?}"
            );

            // The tags the CREATE stored are addressable through the tagging plane: the
            // read answers them, and the resource's own read answers the same set.
            let mut expected = tag_map(tags.as_ref().unwrap_or(&Value::Null));
            if kind.create_reports_tags {
                let echoed = tag_map(at(&detail, "tags"));
                assert_eq!(
                    echoed, expected,
                    "case {case}: the create must echo the tags it stored, got {echoed:?}"
                );
            }
            let got = listed(&router, &arn, &case).await;
            assert_eq!(
                got, expected,
                "case {case}: ListTagsForResource must report the create-time tags, got {got:?}"
            );
            if kind.read_reports_tags {
                let read = reported(&router, &kind, &id, &case).await;
                assert_eq!(
                    read, expected,
                    "case {case}: the resource's own read must report the same tag set as the \
                     tag read {expected:?}, got {read:?}"
                );
            }

            // A create-time key's value is CHANGEABLE, and a key the resource never
            // carried is added by the same write.
            let mut written: Vec<Value> = expected
                .keys()
                .take(1)
                .map(|key| json!({ "key": key, "value": "relevance" }))
                .collect();
            written.push(json!({ "key": "stage", "value": "prod" }));
            accepted(
                &router,
                "TagResource",
                &arn,
                &case,
                json!({ "tags": written }),
            )
            .await;
            for tag in &written {
                expected.insert(text(tag, "key"), text(tag, "value"));
            }
            let got = listed(&router, &arn, &case).await;
            assert_eq!(
                got, expected,
                "case {case}: a later TagResource must leave the merged tag set {expected:?}, \
                 got {got:?}"
            );
            if kind.read_reports_tags {
                let read = reported(&router, &kind, &id, &case).await;
                assert_eq!(
                    read, expected,
                    "case {case}: after the write the resource's own read must still agree \
                     with the tag read {expected:?}, got {read:?}"
                );
            }

            // And every key the resource now carries is REMOVABLE, including the one the
            // create put there.
            let keys: Vec<String> = expected.keys().cloned().collect();
            accepted(
                &router,
                "UntagResource",
                &arn,
                &case,
                json!({ "tagKeys": keys }),
            )
            .await;
            expected.clear();
            let got = listed(&router, &arn, &case).await;
            assert_eq!(
                got, expected,
                "case {case}: UntagResource must remove every key named, got {got:?}"
            );
            if kind.read_reports_tags {
                let read = reported(&router, &kind, &id, &case).await;
                assert_eq!(
                    read, expected,
                    "case {case}: after the removal the resource's own read must report no \
                     tags, got {read:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_tag_cap_is_held_for_every_taggable_kind_and_names_the_resource_it_refused() {
    let router = emulator();

    for kind in taggable_kinds() {
        let case = kind.case;
        let detail = create(
            &router,
            &kind,
            &format!("{}-capped", kind.name_prefix),
            None,
        )
        .await;
        let arn = text(&detail, "arn");

        // Filled in several writes, so the cap is held over the ACCUMULATED set rather than
        // over one request's list.
        for chunk in 0..(MAX_TAGS_PER_RESOURCE / 10) {
            let tags: Vec<Value> = (0..10)
                .map(|index| json!({ "key": format!("key-{chunk}-{index}"), "value": "v" }))
                .collect();
            accepted(
                &router,
                "TagResource",
                &arn,
                &format!("{case}, chunk {chunk} of the cap"),
                json!({ "tags": tags }),
            )
            .await;
        }
        let at_cap = listed(&router, &arn, case).await;
        assert_eq!(
            at_cap.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case} at the cap: the resource must carry exactly \
             {MAX_TAGS_PER_RESOURCE} tags, got {at_cap:?}"
        );

        // One past the cap is the modelled quota refusal, and it names the resource the ARN
        // addresses and the KIND its segment carries.
        let (status, code, answer) = call(
            &router,
            "TagResource",
            json!({ "resourceArn": &arn, "tags": [{ "key": "one-tag-too-many", "value": "v" }] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ServiceQuotaExceededException"),
            "case {case} one past the cap: expected {CLIENT_ERROR} \
             ServiceQuotaExceededException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            format!("Maximum number of tags exceeded: {MAX_TAGS_PER_RESOURCE}"),
            "case {case} one past the cap: the refusal must name the cap, got {answer}"
        );
        assert_eq!(
            text(&answer, "resourceId"),
            arn,
            "case {case} one past the cap: the refusal must name the resource it refused, \
             got {answer}"
        );
        assert_eq!(
            text(&answer, "resourceType"),
            kind.resource_type,
            "case {case} one past the cap: the refusal must name the kind the ARN addresses, \
             got {answer}"
        );
        assert_eq!(
            text(&answer, "serviceCode"),
            ARN_NAMESPACE,
            "case {case} one past the cap: the refusal must name this service, got {answer}"
        );

        // A refused write stores nothing.
        let after = listed(&router, &arn, case).await;
        assert_eq!(
            after, at_cap,
            "case {case} one past the cap: the refused write must store nothing, got {after:?}"
        );
    }
}
