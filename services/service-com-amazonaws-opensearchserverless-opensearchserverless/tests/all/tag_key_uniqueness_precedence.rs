// SPDX-License-Identifier: CC0-1.0

//! Tag-key uniqueness is decided from the REQUEST, before anything the store holds is
//! measured, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test is the one `tag_key_uniqueness` pins — a request member that carries
//! tag keys may not name one key twice, and such a request is the invalid input
//! `ValidationException` describes — read for WHEN it is decided: the repeat is a property
//! of the request alone, so it is refused whatever else the request would have been
//! answered with, and the request is never folded into the stored tag set first. That
//! matters because the fold is what the per-resource tag cap is counted over and what the
//! merge of an already-carried key happens in: a request naming one key twice must not be
//! measured against the cap, resolved against the store, or merged at all.
//!
//! The table spans what that generalises over — the answer the SAME request shape gets with
//! distinct keys instead (the tag cap, a resource that is not there, a name already taken,
//! or plain acceptance), which member carries the keys, and whether the repeated key is one
//! the resource already carries — so each row shows the refusal the uniqueness rule takes
//! precedence over, not just that some refusal arrives.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_TAGS_PER_RESOURCE;

use crate::collection_lifecycle::{call, text, ACCOUNT, CLIENT_ERROR};
use crate::tag_key_uniqueness::{create_collection, listed_tags, repeated_key_message, VALIDATION};

/// One row: one request shape sent TWICE — once naming a tag key twice, once with distinct
/// keys — so the row records both the uniqueness refusal and the answer the request would
/// otherwise have been given.
struct PrecedenceCase {
    case: &'static str,
    operation: &'static str,
    /// The member path the uniqueness refusal names, which is the member carrying the keys.
    member: &'static str,
    /// The request naming one key twice, and the key it names twice.
    repeated_request: Value,
    repeated: &'static str,
    /// The same request with distinct keys, and the error code it is answered with —
    /// `None` when that request is accepted.
    control_request: Value,
    control_code: Option<&'static str>,
    /// The resource whose stored tag set the REFUSED request must leave exactly as it was,
    /// read before and after that request, so each row carries its own post-condition and
    /// does not depend on the rows around it. `None` where no read can report it (an ARN
    /// that resolves to nothing).
    unchanged: Option<String>,
}

#[tokio::test]
async fn a_repeated_tag_key_is_refused_before_the_request_is_measured_against_the_store() {
    let (router, capped, fresh) = emulator_with_two_collections().await;

    // A well-formed ARN of this account and Region addressing a collection nobody created:
    // `CollectionId` is `[a-z0-9]{3,40}`, so this is an id the service could have minted.
    let absent = format!(
        "arn:aws:aoss:us-east-1:{ACCOUNT}:collection/{}",
        "0".repeat(20)
    );

    // A list that is nothing but repeats of ONE key: were the request folded into the
    // stored set first it would add a single tag, well inside the cap.
    let one_key_many_times: Vec<Value> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| json!({ "key": "dup", "value": format!("v{index}") }))
        .collect();
    // The same count of DISTINCT keys, which fills the fresh collection to the cap.
    let distinct_to_the_cap: Vec<Value> = (0..MAX_TAGS_PER_RESOURCE - 1)
        .map(|index| json!({ "key": format!("fill-{index}"), "value": "v" }))
        .collect();

    let cases = vec![
        PrecedenceCase {
            case: "a repeat of a key that would take the resource past the tag cap",
            operation: "TagResource",
            member: "tags",
            repeated_request: json!({
                "resourceArn": &capped,
                "tags": [
                    { "key": "beyond-the-cap", "value": "1" },
                    { "key": "beyond-the-cap", "value": "2" },
                ],
            }),
            repeated: "beyond-the-cap",
            control_request: json!({
                "resourceArn": &capped,
                "tags": [{ "key": "beyond-the-cap", "value": "1" }],
            }),
            control_code: Some("ServiceQuotaExceededException"),
            unchanged: Some(capped.clone()),
        },
        PrecedenceCase {
            case: "a repeat of a key the resource already carries, at the cap",
            operation: "TagResource",
            member: "tags",
            repeated_request: json!({
                "resourceArn": &capped,
                "tags": [
                    { "key": "cap-0", "value": "rewritten" },
                    { "key": "cap-0", "value": "rewritten again" },
                ],
            }),
            repeated: "cap-0",
            control_request: json!({
                "resourceArn": &capped,
                "tags": [{ "key": "cap-0", "value": "rewritten" }],
            }),
            control_code: None,
            unchanged: Some(capped.clone()),
        },
        PrecedenceCase {
            case: "a whole list naming one key, which would merge to a single tag",
            operation: "TagResource",
            member: "tags",
            repeated_request: json!({ "resourceArn": &fresh, "tags": one_key_many_times }),
            repeated: "dup",
            control_request: json!({ "resourceArn": &fresh, "tags": distinct_to_the_cap }),
            control_code: None,
            unchanged: Some(fresh.clone()),
        },
        PrecedenceCase {
            case: "a repeat addressed at a resource that is not there",
            operation: "TagResource",
            member: "tags",
            repeated_request: json!({
                "resourceArn": &absent,
                "tags": [
                    { "key": "env", "value": "prod" },
                    { "key": "env", "value": "staging" },
                ],
            }),
            repeated: "env",
            control_request: json!({
                "resourceArn": &absent,
                "tags": [{ "key": "env", "value": "prod" }],
            }),
            control_code: Some("ResourceNotFoundException"),
            unchanged: None,
        },
        PrecedenceCase {
            case: "a repeated key to remove from a resource that is not there",
            operation: "UntagResource",
            member: "tagKeys",
            repeated_request: json!({ "resourceArn": &absent, "tagKeys": ["env", "env"] }),
            repeated: "env",
            control_request: json!({ "resourceArn": &absent, "tagKeys": ["env"] }),
            control_code: Some("ResourceNotFoundException"),
            unchanged: None,
        },
        PrecedenceCase {
            case: "a repeat on a create whose name is already taken",
            operation: "CreateCollection",
            member: "tags",
            repeated_request: json!({
                "name": "precedence-fresh",
                "encryptionConfig": { "aWSOwnedKey": true },
                "tags": [
                    { "key": "env", "value": "prod" },
                    { "key": "env", "value": "staging" },
                ],
            }),
            repeated: "env",
            control_request: json!({
                "name": "precedence-fresh",
                "encryptionConfig": { "aWSOwnedKey": true },
                "tags": [{ "key": "env", "value": "prod" }],
            }),
            control_code: Some("ConflictException"),
            // The create names the collection that already holds the name, so its tag set
            // is what a create that got past the uniqueness rule would have disturbed.
            unchanged: Some(fresh.clone()),
        },
        // Last, because its control is accepted and takes the capped collection one tag
        // below the cap.
        PrecedenceCase {
            case: "a repeated key to remove, naming a key the resource carries",
            operation: "UntagResource",
            member: "tagKeys",
            repeated_request: json!({ "resourceArn": &capped, "tagKeys": ["cap-1", "cap-1"] }),
            repeated: "cap-1",
            control_request: json!({ "resourceArn": &capped, "tagKeys": ["cap-1"] }),
            control_code: None,
            unchanged: Some(capped.clone()),
        },
    ];

    for PrecedenceCase {
        case,
        operation,
        member,
        repeated_request,
        repeated,
        control_request,
        control_code,
        unchanged,
    } in cases
    {
        let before = match &unchanged {
            Some(arn) => Some(listed_tags(&router, arn).await),
            None => None,
        };

        let (status, code, answer) = call(&router, operation, repeated_request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, VALIDATION),
            "case {operation}({case}): expected {CLIENT_ERROR} {VALIDATION}, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            repeated_key_message(member, repeated),
            "case {operation}({case}): expected the refusal to name the repeated key at {member:?}, got {answer}"
        );

        // The refused request stored nothing: this row's own post-condition, whatever the
        // rows before it left behind.
        if let (Some(arn), Some(before)) = (&unchanged, before) {
            let after = listed_tags(&router, arn).await;
            assert_eq!(
                after, before,
                "case {operation}({case}): expected the refused request to leave the stored \
                 tags at {before:?}, got {after:?}"
            );
        }

        // The same request with distinct keys is answered by whatever the uniqueness rule
        // took precedence over, which is what makes the row above a precedence row.
        let (status, code, answer) = call(&router, operation, control_request).await;
        match control_code {
            Some(expected) => assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, expected),
                "case {operation}({case}, distinct keys): expected {CLIENT_ERROR} {expected}, got {status} {code} with {answer}"
            ),
            None => assert_eq!(
                status, 200,
                "case {operation}({case}, distinct keys): expected 200, got {status} {code} with {answer}"
            ),
        }
    }

    // Nothing a refused request asked for reached the store: the capped collection carries
    // the tags its fill wrote, with the one value its accepted control rewrote and without
    // the one key its accepted control removed, and no key a refused request named.
    let capped_tags = listed_tags(&router, &capped).await;
    assert_eq!(
        capped_tags.len(),
        MAX_TAGS_PER_RESOURCE - 1,
        "case ListTagsForResource(the capped collection): expected {} tags after the one \
         accepted removal, got {capped_tags:?}",
        MAX_TAGS_PER_RESOURCE - 1
    );
    assert!(
        capped_tags.contains(&("cap-0".to_string(), "rewritten".to_string())),
        "case ListTagsForResource(the capped collection): expected the accepted write's \
         value at cap-0, got {capped_tags:?}"
    );
    assert!(
        !capped_tags
            .iter()
            .any(|(key, _)| key == "cap-1" || key == "beyond-the-cap"),
        "case ListTagsForResource(the capped collection): expected no key a refused request \
         named and none the accepted removal took, got {capped_tags:?}"
    );

    // The fresh collection carries its create's tag and the distinct fill its accepted
    // control wrote — and not the key the refused all-repeats list named, nor the key the
    // refused create carried.
    let fresh_tags = listed_tags(&router, &fresh).await;
    assert_eq!(
        fresh_tags.len(),
        MAX_TAGS_PER_RESOURCE,
        "case ListTagsForResource(the fresh collection): expected {MAX_TAGS_PER_RESOURCE} \
         tags, got {fresh_tags:?}"
    );
    assert!(
        !fresh_tags
            .iter()
            .any(|(key, _)| key == "dup" || key == "env"),
        "case ListTagsForResource(the fresh collection): expected no key a refused request \
         named, got {fresh_tags:?}"
    );

    // The ARN no resource answers to is still unresolvable, so neither refused tagging call
    // brought a tag set into being.
    let (status, code, answer) =
        call(&router, "ListTagsForResource", json!({ "resourceArn": &absent })).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ResourceNotFoundException"),
        "case ListTagsForResource(the absent ARN): expected {CLIENT_ERROR} \
         ResourceNotFoundException, got {status} {code} with {answer}"
    );
}

/// An emulator holding two collections: one filled to the per-resource tag cap, and one
/// carrying a single tag. Answers the router and the two ARNs.
async fn emulator_with_two_collections() -> (smithy_server_runtime::MultiServiceRouter, String, String)
{
    let router = crate::collection_lifecycle::emulator();

    let capped = text(
        &create_collection(
            &router,
            json!({
                "name": "precedence-capped",
                "encryptionConfig": { "aWSOwnedKey": true },
            }),
        )
        .await,
        "arn",
    );
    // `Tags` admits up to 50 entries (`smithy_query(query="shape", shape="Tags")` ->
    // `length.max`), which is the per-resource cap, so one call fills the resource.
    let fill: Vec<Value> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| json!({ "key": format!("cap-{index}"), "value": "v" }))
        .collect();
    let (status, code, answer) = call(
        &router,
        "TagResource",
        json!({ "resourceArn": &capped, "tags": fill }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case TagResource(filling the collection to the cap): expected 200, got {status} \
         {code} with {answer}"
    );
    let at_cap = listed_tags(&router, &capped).await;
    assert_eq!(
        at_cap.len(),
        MAX_TAGS_PER_RESOURCE,
        "case TagResource(filling the collection to the cap): expected \
         {MAX_TAGS_PER_RESOURCE} tags, got {at_cap:?}"
    );

    let fresh = text(
        &create_collection(
            &router,
            json!({
                "name": "precedence-fresh",
                "encryptionConfig": { "aWSOwnedKey": true },
                "tags": [{ "key": "team", "value": "search" }],
            }),
        )
        .await,
        "arn",
    );

    (router, capped, fresh)
}
