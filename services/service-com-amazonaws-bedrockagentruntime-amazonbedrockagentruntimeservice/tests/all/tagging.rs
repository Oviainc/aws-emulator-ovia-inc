// SPDX-License-Identifier: CC0-1.0

//! The session tag plane — `TagResource`, `UntagResource`, `ListTagsForResource` — end to
//! end through the framework router.
//!
//! RULE: the only resource this service's `TaggableResourcesArn` admits is a SESSION
//! (`smithy_query(query="operation",operation="TagResource")` -> `resourceArn` is
//! `@length(20, 1011)` with the pattern
//! `(^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session)/<uuid>$)`), so a tagging
//! request addressed to a session ARN this emulator minted is SERVED against that
//! session's stored tags — tagging merges, untagging removes, listing reports the merge —
//! and a well-formed session ARN naming no stored session is the modelled 404
//! `ResourceNotFoundException`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice as runtime;

/// The region every request here signs for, and so the region segment of a minted
/// `sessionArn`.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(runtime::EmulatedService::new(engine));
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(serde_json::to_vec(&body).expect("the test body serializes"))
    };
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{REGION}/bedrock/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// The wire error code restJson1 puts in `__type`, or `""` on a success body.
fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// Percent-encode an ARN for a non-greedy `@httpLabel`, as an SDK does: a session ARN
/// carries `:` and `/`, which would otherwise split the path segment.
fn label(arn: &str) -> String {
    arn.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}

/// Create a session, optionally with tags, and return `(sessionId, sessionArn)`.
async fn create_session(router: &MultiServiceRouter, tags: Value) -> (String, String) {
    let body = if tags.is_null() {
        json!({})
    } else {
        json!({ "tags": tags })
    };
    let (status, out) = call(router, "PUT", "/sessions/", body).await;
    assert_eq!(status, 201, "CreateSession: expected 201, got {status} {out:?}");
    (
        out["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateSession: no sessionId in {out:?}"))
            .to_string(),
        out["sessionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateSession: no sessionArn in {out:?}"))
            .to_string(),
    )
}

/// The tags `ListTagsForResource` reports for a session ARN, as a JSON value (`Null` when
/// the member is absent, which is what a session carrying no tags reports — the model
/// bounds `tags` at `length {min: 1}`).
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, out) = call(router, "GET", &format!("/tags/{}", label(arn)), Value::Null).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource({arn}): expected 200, got {status} {out:?}"
    );
    out.get("tags").cloned().unwrap_or(Value::Null)
}

/// RULE applied over a TABLE of tag writes: `TagResource` MERGES the supplied map into
/// the session's stored tags — a new key is added, a key already stored takes the new
/// value, and no other stored key is touched — and `ListTagsForResource` reports exactly
/// the merge.
///
/// The rows differ along what the rule generalises over: how many keys one call carries,
/// whether a key is new or already stored, the value's form (ordinary, empty, and the
/// punctuation `TagValue`'s own pattern admits), and the starting point (tags supplied at
/// CreateSession).
#[tokio::test]
async fn tagging_a_session_merges_into_what_it_already_carries() {
    let router = router();
    let (_, session_arn) = create_session(&router, json!({ "origin": "create-session" })).await;

    // (case, the tags one TagResource call carries, the whole map expected afterwards)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "one new key",
            json!({ "team": "runtime" }),
            json!({ "origin": "create-session", "team": "runtime" }),
        ),
        (
            "two new keys at once",
            json!({ "env": "test", "owner": "nobody" }),
            json!({
                "origin": "create-session",
                "team": "runtime",
                "env": "test",
                "owner": "nobody",
            }),
        ),
        (
            "a stored key takes the new value",
            json!({ "env": "prod" }),
            json!({
                "origin": "create-session",
                "team": "runtime",
                "env": "prod",
                "owner": "nobody",
            }),
        ),
        (
            // `TagValue` is `length(0, 256)`, so the empty value is a value.
            "an empty value",
            json!({ "owner": "" }),
            json!({
                "origin": "create-session",
                "team": "runtime",
                "env": "prod",
                "owner": "",
            }),
        ),
        (
            // The punctuation `TagKey`/`TagValue`'s pattern admits:
            // `^[a-zA-Z0-9\s._:/=+@-]*$`
            // (`smithy_query(query="shape",shape="TagsMap")`).
            "a key and value of punctuation the pattern admits",
            json!({ "cost:center/2026": "a.b_c=d+e@f-g" }),
            json!({
                "origin": "create-session",
                "team": "runtime",
                "env": "prod",
                "owner": "",
                "cost:center/2026": "a.b_c=d+e@f-g",
            }),
        ),
    ];

    for (case, tags, expected) in rows {
        let (status, out) = call(
            &router,
            "POST",
            &format!("/tags/{}", label(&session_arn)),
            json!({ "tags": tags }),
        )
        .await;
        assert_eq!(
            (status, error_code(&out)),
            (200, ""),
            "case {case:?}: TagResource expected 200, got {status} {out:?}"
        );
        let listed = list_tags(&router, &session_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the merged map {expected}, got {listed}"
        );
    }
}

/// RULE: `UntagResource` removes exactly the keys it names and leaves every other stored
/// key; a key the session does not carry is a no-op, because the request asks for that key
/// to be absent and it is. A session left with no tags reports the member as ABSENT.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    let router = router();
    let (_, session_arn) = create_session(
        &router,
        json!({ "a": "1", "b": "2", "c": "3", "d": "4" }),
    )
    .await;

    // (case, the tagKeys one UntagResource call carries, the whole map expected afterwards)
    let rows: Vec<(&str, Vec<&str>, Value)> = vec![
        (
            "one stored key",
            vec!["a"],
            json!({ "b": "2", "c": "3", "d": "4" }),
        ),
        (
            "a key the session does not carry: a no-op",
            vec!["never-set"],
            json!({ "b": "2", "c": "3", "d": "4" }),
        ),
        (
            "the same key twice in one call",
            vec!["b", "b"],
            json!({ "c": "3", "d": "4" }),
        ),
        (
            "a stored key beside an unknown one",
            vec!["c", "never-set"],
            json!({ "d": "4" }),
        ),
        ("the last stored key", vec!["d"], Value::Null),
        (
            "every key again, on a resource with none left",
            vec!["a", "b", "c", "d"],
            Value::Null,
        ),
    ];

    for (case, tag_keys, expected) in rows {
        let query: String = tag_keys
            .iter()
            .map(|key| format!("tagKeys={key}"))
            .collect::<Vec<String>>()
            .join("&");
        let (status, out) = call(
            &router,
            "DELETE",
            &format!("/tags/{}?{query}", label(&session_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, error_code(&out)),
            (200, ""),
            "case {case:?}: UntagResource expected 200, got {status} {out:?}"
        );
        let listed = list_tags(&router, &session_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected} after removing {tag_keys:?}, got {listed}"
        );
    }
}

/// RULE: a well-formed session ARN that names no stored session is the modelled 404
/// `ResourceNotFoundException` on every one of the three tagging operations — the ARN is
/// resolved against the store, not merely parsed.
///
/// The rows are the three operations, each against two ARNs the model admits but this
/// store does not hold: one this emulator could have minted, and one from another
/// account (which names another account's resource and so resolves to nothing here).
#[tokio::test]
async fn tagging_an_arn_that_names_no_session_is_not_found() {
    let router = router();
    // Create one session, so the store is non-empty and the refusal is about the ARN.
    let (_, live_arn) = create_session(&router, json!({ "k": "v" })).await;

    let absent =
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/0cabde93-58b4-4d68-b230-90df328e1acb");
    let other_account =
        "arn:aws:bedrock:us-east-1:210987654321:session/11111111-2222-3333-4444-555555555555"
            .to_string();

    // (operation, method, the path builder for one ARN, body)
    type Operation = (&'static str, &'static str, fn(&str) -> String, Value);
    let operations: Vec<Operation> = vec![
        (
            "TagResource",
            "POST",
            |arn| format!("/tags/{}", label(arn)),
            json!({ "tags": { "k": "v" } }),
        ),
        (
            "UntagResource",
            "DELETE",
            |arn| format!("/tags/{}?tagKeys=k", label(arn)),
            Value::Null,
        ),
        (
            "ListTagsForResource",
            "GET",
            |arn| format!("/tags/{}", label(arn)),
            Value::Null,
        ),
    ];

    for (operation, method, path_of, body) in operations {
        for (case, arn) in [
            ("an ARN this emulator could have minted", &absent),
            ("another account's session ARN", &other_account),
        ] {
            let (status, refused) = call(&router, method, &path_of(arn), body.clone()).await;
            assert_eq!(
                (status, error_code(&refused)),
                (404, "ResourceNotFoundException"),
                "case {operation} / {case:?}: expected 404 ResourceNotFoundException for \
                 {arn}, got {status} {refused:?}"
            );
        }
    }

    // The live session's own tags are untouched by any of those refusals.
    let listed = list_tags(&router, &live_arn).await;
    assert_eq!(
        listed,
        json!({ "k": "v" }),
        "the stored session's tags must be untouched, got {listed}"
    );
}

/// The ONE sentence the tag plane states when its `resourceArn` names no stored
/// resource: entirely fixed, trailing period included, naming the resource kind in words
/// and carrying no identifier. The only resource a `TaggableResourcesArn` admits is a
/// session (`smithy_query(query="operation",operation="TagResource")` -> `resourceArn`).
const TAG_NOT_FOUND_MESSAGE: &str = "Session resource not found.";

/// The `message` member of an error body, or `""` when there is none.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// RULE: a tagging operation whose `resourceArn` resolves to no stored resource states
/// ONE constant sentence — the resource kind in words, trailing period included — and
/// interpolates nothing from the request: no ARN, and no segment of one.
///
/// The rows span the three tagging operations and what the rule generalises over in the
/// ARN: which account, which partition, which region, which session uuid, and how the
/// ARN came to name nothing (never minted, or minted and then deleted). The sentence is
/// the same in every cell, which is the point — a client matches one rule.
#[tokio::test]
async fn a_tagging_arn_that_names_nothing_states_one_constant_sentence() {
    let router = router();
    // A session created and then deleted: its ARN is well-formed, this emulator minted
    // it, and it now names nothing.
    let (deleted_id, deleted_arn) = create_session(&router, json!({ "k": "v" })).await;
    for (method, path) in [
        ("PATCH", format!("/sessions/{deleted_id}")),
        ("DELETE", format!("/sessions/{deleted_id}")),
    ] {
        let (status, out) = call(&router, method, &path, Value::Null).await;
        assert_eq!(
            status, 200,
            "releasing the session: {method} {path} expected 200, got {status} {out:?}"
        );
    }

    // (case, an ARN the model admits that names no stored session)
    let arns: Vec<(&str, String)> = vec![
        (
            "a deleted session's own ARN",
            deleted_arn,
        ),
        (
            "an ARN this emulator could have minted, never minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/0cabde93-58b4-4d68-b230-90df328e1acb"),
        ),
        (
            "a different unknown session uuid, same account and region",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ),
        (
            "another region",
            format!("arn:aws:bedrock:eu-central-1:{ACCOUNT}:session/11111111-2222-3333-4444-555555555555"),
        ),
        (
            "another account",
            "arn:aws:bedrock:us-east-1:210987654321:session/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:bedrock:us-gov-west-1:{ACCOUNT}:session/7a9c0d11-2e3f-4a5b-8c7d-9e0f1a2b3c4d"),
        ),
    ];

    // (operation, method, the path builder for one ARN, body)
    type Operation = (&'static str, &'static str, fn(&str) -> String, Value);
    let operations: Vec<Operation> = vec![
        (
            "TagResource",
            "POST",
            |arn| format!("/tags/{}", label(arn)),
            json!({ "tags": { "k": "v" } }),
        ),
        (
            "UntagResource",
            "DELETE",
            |arn| format!("/tags/{}?tagKeys=k", label(arn)),
            Value::Null,
        ),
        (
            "ListTagsForResource",
            "GET",
            |arn| format!("/tags/{}", label(arn)),
            Value::Null,
        ),
    ];

    for (operation, method, path_of, body) in &operations {
        for (form, arn) in &arns {
            let case = format!("{operation} addressed by {form}");
            let (status, refused) = call(&router, method, &path_of(arn), body.clone()).await;
            assert_eq!(
                (status, error_code(&refused)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {status} {refused:?}"
            );
            let stated = message(&refused);
            assert_eq!(
                stated, TAG_NOT_FOUND_MESSAGE,
                "case {case:?}: expected message {TAG_NOT_FOUND_MESSAGE:?}, got {stated:?}"
            );
            // No identifier at all: neither the whole ARN nor any segment of it — the
            // account, the region, the partition or the session uuid — appears.
            for segment in arn.split([':', '/']).filter(|part| !part.is_empty()) {
                assert!(
                    !stated.contains(segment),
                    "case {case:?}: the refusal must carry no part of the request ARN, but \
                     {stated:?} contains {segment:?}"
                );
            }
        }
    }
}

/// RULE: the tags a session carries are ONE map, whichever operation wrote them —
/// `CreateSession`'s `tags` member and `TagResource` write the same store, and
/// `ListTagsForResource` reads it, addressed by the session ARN `CreateSession` minted.
#[tokio::test]
async fn create_session_tags_and_tag_resource_write_one_map() {
    let router = router();
    let (session_id, session_arn) =
        create_session(&router, json!({ "from": "create-session" })).await;
    assert_eq!(
        session_arn,
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/{session_id}"),
        "the minted sessionArn must name the minted session, got {session_arn:?}"
    );

    assert_eq!(
        list_tags(&router, &session_arn).await,
        json!({ "from": "create-session" }),
        "CreateSession's tags must be what ListTagsForResource reports"
    );

    let (status, out) = call(
        &router,
        "POST",
        &format!("/tags/{}", label(&session_arn)),
        json!({ "tags": { "from": "tag-resource", "extra": "yes" } }),
    )
    .await;
    assert_eq!(status, 200, "TagResource: expected 200, got {status} {out:?}");
    assert_eq!(
        list_tags(&router, &session_arn).await,
        json!({ "from": "tag-resource", "extra": "yes" }),
        "TagResource must write the same map CreateSession seeded"
    );

    // Deleting the session releases its tags with it: the ARN then names nothing.
    let (status, deleted) = call(
        &router,
        "PATCH",
        &format!("/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "EndSession: expected 200, got {status} {deleted:?}");
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "DeleteSession: expected 200, got {status} {deleted:?}");
    let (status, refused) = call(
        &router,
        "GET",
        &format!("/tags/{}", label(&session_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "a deleted session's ARN must name nothing, got {status} {refused:?}"
    );
}
