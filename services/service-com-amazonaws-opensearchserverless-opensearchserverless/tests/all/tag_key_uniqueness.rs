// SPDX-License-Identifier: CC0-1.0

//! Tag-key uniqueness: a tagging request that names one tag key twice is refused, end to
//! end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: a tag set is a map keyed by tag key, so a request member that
//! carries tag keys may not name one key twice; such a request is the invalid input
//! `ValidationException` describes and nothing is created, written or removed. The model
//! states it on `TagResourceRequest$tags` and `UntagResourceRequest$tagKeys` ("All tag keys
//! in the request must be unique") and on the create requests' `tags` ("an arbitrary set of
//! tags (key–value pairs)"), while neither `Tags` nor `TagKeys` carries `@uniqueItems`
//! (`smithy_query(query="shape", shape="Tags,TagKeys")`), so no boundary constraint narrows
//! it.
//!
//! The table spans what the rule generalises over — which operation and which member
//! carries the keys, where in the list the repeat sits, how many times a key repeats, how
//! long the key is and which characters it holds, and whether the values beside the
//! repeated key agree — not the one request the issue quoted. Beside it sit the rows the
//! rule must still ADMIT: keys that differ, including keys differing only in case.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// The error code every refusal below carries
/// (`smithy_query(query="shape", shape="ValidationException")`).
const VALIDATION: &str = "ValidationException";

/// `TagKey`'s modelled maximum length (`smithy_query(query="shape", shape="TagKeys")` ->
/// the member's `length.max`), so the longest key the boundary admits is exercised too.
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The refusal this service words for a tagging member that names `key` twice: the clause
/// its boundary validator renders for a repeated element of a list, at the member that
/// carried it.
fn repeated_key_message(member: &str, key: &str) -> String {
    format!(
        "1 validation error detected: Value '{key}' at '{member}' failed to satisfy \
         constraint: Member must have unique values"
    )
}

/// One row of the acceptance table: a tagging request whose keys are all distinct, and the
/// tag set the resource must report after it.
struct AcceptedCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expected: Vec<(&'static str, &'static str)>,
}

/// One row of the refusal table.
struct RepeatCase {
    case: &'static str,
    operation: &'static str,
    /// The member path the refusal names, which is the member that carried the keys.
    member: &'static str,
    request: Value,
    /// The key the request names more than once — the FIRST such key when several do.
    repeated: String,
}

/// Create a collection encrypted with the AWS-owned key and answer its
/// `createCollectionDetail`.
async fn create_collection(router: &smithy_server_runtime::MultiServiceRouter, body: Value) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateCollection", body).await;
    assert_eq!(
        status, 200,
        "case CreateCollection({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionDetail").clone()
}

/// The tags `ListTagsForResource` reports for `arn`, sorted by key.
async fn listed_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> Vec<(String, String)> {
    let (status, code, answer) =
        call(router, "ListTagsForResource", json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource({arn}): expected 200, got {status} {code} with {answer}"
    );
    let mut tags: Vec<(String, String)> = at(&answer, "tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| (text(tag, "key"), text(tag, "value")))
        .collect();
    tags.sort();
    tags
}

#[tokio::test]
async fn a_repeated_tag_key_is_refused_wherever_a_request_carries_tag_keys() {
    let router = emulator();

    let detail = create_collection(
        &router,
        json!({
            "name": "unique-keys-collection",
            "encryptionConfig": { "aWSOwnedKey": true },
            "tags": [{ "key": "team", "value": "search" }],
        }),
    )
    .await;
    let arn = text(&detail, "arn");
    let stored_before = listed_tags(&router, &arn).await;

    let long_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let cases = vec![
        RepeatCase {
            case: "the same key twice, side by side",
            operation: "UntagResource",
            member: "tagKeys",
            request: json!({ "resourceArn": arn, "tagKeys": ["env", "env"] }),
            repeated: "env".to_string(),
        },
        RepeatCase {
            case: "a repeat separated by another key",
            operation: "UntagResource",
            member: "tagKeys",
            request: json!({ "resourceArn": arn, "tagKeys": ["team", "env", "team"] }),
            repeated: "team".to_string(),
        },
        RepeatCase {
            case: "one key three times, after a distinct one",
            operation: "UntagResource",
            member: "tagKeys",
            request: json!({ "resourceArn": arn, "tagKeys": ["a", "b", "b", "b"] }),
            repeated: "b".to_string(),
        },
        RepeatCase {
            case: "a key of the maximum modelled length, repeated",
            operation: "UntagResource",
            member: "tagKeys",
            request: json!({ "resourceArn": arn, "tagKeys": [long_key, long_key] }),
            repeated: long_key.clone(),
        },
        RepeatCase {
            case: "a repeat beside a key differing only in case",
            operation: "UntagResource",
            member: "tagKeys",
            request: json!({ "resourceArn": arn, "tagKeys": ["Env", "env", "Env"] }),
            repeated: "Env".to_string(),
        },
        RepeatCase {
            case: "two tags naming one key with different values",
            operation: "TagResource",
            member: "tags",
            request: json!({
                "resourceArn": arn,
                "tags": [{ "key": "env", "value": "prod" }, { "key": "env", "value": "staging" }],
            }),
            repeated: "env".to_string(),
        },
        RepeatCase {
            case: "two tags naming one key with the SAME value",
            operation: "TagResource",
            member: "tags",
            request: json!({
                "resourceArn": arn,
                "tags": [
                    { "key": "owner", "value": "data" },
                    { "key": "env", "value": "prod" },
                    { "key": "owner", "value": "data" },
                ],
            }),
            repeated: "owner".to_string(),
        },
        RepeatCase {
            case: "a non-ASCII key, repeated",
            operation: "TagResource",
            member: "tags",
            request: json!({
                "resourceArn": arn,
                "tags": [{ "key": "ключ", "value": "a" }, { "key": "ключ", "value": "b" }],
            }),
            repeated: "ключ".to_string(),
        },
        RepeatCase {
            case: "a create whose own tag set names one key twice",
            operation: "CreateCollection",
            member: "tags",
            request: json!({
                "name": "repeated-tag-collection",
                "encryptionConfig": { "aWSOwnedKey": true },
                "tags": [{ "key": "env", "value": "prod" }, { "key": "env", "value": "dev" }],
            }),
            repeated: "env".to_string(),
        },
        RepeatCase {
            case: "a collection-group create whose tag set names one key twice",
            operation: "CreateCollectionGroup",
            member: "tags",
            request: json!({
                "name": "repeated-tag-group",
                "standbyReplicas": "ENABLED",
                "tags": [
                    { "key": "cost-centre", "value": "1" },
                    { "key": "cost-centre", "value": "2" },
                ],
            }),
            repeated: "cost-centre".to_string(),
        },
    ];

    for RepeatCase {
        case,
        operation,
        member,
        request,
        repeated,
    } in cases
    {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, VALIDATION),
            "case {operation}({case}): expected {CLIENT_ERROR} {VALIDATION}, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            repeated_key_message(member, &repeated),
            "case {operation}({case}): expected the refusal to name the repeated key at {member:?}, got {answer}"
        );
    }

    // Nothing the refused requests asked for happened: the tag set is the one the create
    // left, and neither refused create left a resource behind.
    assert_eq!(
        listed_tags(&router, &arn).await,
        stored_before,
        "case ListTagsForResource(after the refusals): expected the stored tags to be unchanged at {stored_before:?}"
    );
    let (status, code, read) = call(
        &router,
        "BatchGetCollection",
        json!({ "names": ["repeated-tag-collection"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection(the refused create): expected 200, got {status} {code} with {read}"
    );
    assert!(
        at(&read, "collectionDetails")
            .as_array()
            .is_none_or(|details| details.is_empty()),
        "case BatchGetCollection(the refused create): expected no collection, got {read}"
    );
    let (status, code, read) = call(
        &router,
        "BatchGetCollectionGroup",
        json!({ "names": ["repeated-tag-group"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetCollectionGroup(the refused create): expected 200, got {status} {code} with {read}"
    );
    assert!(
        at(&read, "collectionGroupDetails")
            .as_array()
            .is_none_or(|details| details.is_empty()),
        "case BatchGetCollectionGroup(the refused create): expected no group, got {read}"
    );
}

#[tokio::test]
async fn distinct_tag_keys_are_accepted_however_they_differ() {
    let router = emulator();

    let detail = create_collection(
        &router,
        json!({
            "name": "distinct-keys-collection",
            "encryptionConfig": { "aWSOwnedKey": true },
            // Keys differing only in case are DIFFERENT keys: a tag key is case-sensitive.
            "tags": [{ "key": "env", "value": "prod" }, { "key": "Env", "value": "staging" }],
        }),
    )
    .await;
    let arn = text(&detail, "arn");
    assert_eq!(
        listed_tags(&router, &arn).await,
        vec![
            ("Env".to_string(), "staging".to_string()),
            ("env".to_string(), "prod".to_string()),
        ],
        "case CreateCollection(keys differing only in case): both keys are stored"
    );

    // One row per shape of tagging request whose keys are all distinct: the rule refuses a
    // repeat and nothing else.
    let accepted = vec![
        AcceptedCase {
            case: "an empty key list",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": [] }),
            expected: vec![("Env", "staging"), ("env", "prod")],
        },
        AcceptedCase {
            case: "an empty tag list",
            operation: "TagResource",
            request: json!({ "resourceArn": arn, "tags": [] }),
            expected: vec![("Env", "staging"), ("env", "prod")],
        },
        AcceptedCase {
            case: "distinct keys, one of them already carried",
            operation: "TagResource",
            request: json!({
                "resourceArn": arn,
                "tags": [{ "key": "env", "value": "dev" }, { "key": "team", "value": "search" }],
            }),
            expected: vec![("Env", "staging"), ("env", "dev"), ("team", "search")],
        },
        AcceptedCase {
            case: "keys differing only in case",
            operation: "TagResource",
            request: json!({
                "resourceArn": arn,
                "tags": [{ "key": "owner", "value": "a" }, { "key": "OWNER", "value": "b" }],
            }),
            expected: vec![
                ("Env", "staging"),
                ("OWNER", "b"),
                ("env", "dev"),
                ("owner", "a"),
                ("team", "search"),
            ],
        },
        AcceptedCase {
            case: "distinct keys removed in one call, one of them never carried",
            operation: "UntagResource",
            request: json!({ "resourceArn": arn, "tagKeys": ["OWNER", "owner", "never-set"] }),
            expected: vec![("Env", "staging"), ("env", "dev"), ("team", "search")],
        },
    ];

    for AcceptedCase {
        case,
        operation,
        request,
        expected,
    } in accepted
    {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {operation}({case}): expected 200, got {status} {code} with {answer}"
        );
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let got = listed_tags(&router, &arn).await;
        assert_eq!(
            got, expected,
            "case {operation}({case}): expected the stored tags {expected:?}, got {got:?}"
        );
    }
}
