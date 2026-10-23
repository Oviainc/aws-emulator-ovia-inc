// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag surface a request names by ARN: `UntagResource`
//! (`DELETE /tags/{resourceArn}?tagKeys=…`), the `TagResource` write it removes from
//! (`POST /tags/{resourceArn}`) and the reader both are checked through,
//! `ListTagsForResource` (`GET /tags/{resourceArn}`) — the three URIs
//! `smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
//! binds them to.
//!
//! RULE: `UntagResource` removes from the resource its `resourceArn` names — a vector
//! bucket or a vector index — exactly the tag keys the request lists, leaving every
//! other key and value as it was; a key the resource does not carry is removed
//! idempotently rather than refused; an ARN naming a resource this store does not
//! hold answers the modelled `NotFoundException` (404), and a value that is neither a
//! vector bucket nor a vector index ARN answers `ValidationException` (400).
//! `TagResource` is the same resolution with the opposite write: it merges the tags it
//! names into that resource's map, replacing the value of a key already there.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! KIND of resource the ARN names (vector bucket, vector index), how many tags it
//! carries and of which key and value forms (the `TagKey` / `TagValue` bounds), how
//! many keys the request writes or removes and whether the resource carries them, and
//! which clause of the ARN form the request breaks. Every case drives real HTTP
//! through the production front door (`MultiServiceRouter::dispatch`), so what is
//! asserted is what an SDK client sees.

use serde_json::{json, Value};
use service_com_amazonaws_s3vectors_s3vectors::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    error_code, expected_bucket_arn, expected_index_arn, ok, router, AUTHORIZATION, REGION,
};

/// Percent-encode `value` for a NON-GREEDY `@httpLabel` or an `@httpQuery` entry the
/// way an AWS SDK does: every byte outside RFC 3986's unreserved set, so an ARN's own
/// `:` and `/` travel as ONE path segment instead of being read as path structure.
///
/// Spelled out here rather than pulled from a crate so this fix adds no dependency
/// edge to `Cargo.lock`, which every landing fix touches; the sibling tagging suites
/// of `codeguru-security` and `dlm` do the same.
fn label(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else {
            out.push('%');
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    out
}

/// One request on an explicit method and path, returning its status and JSON body.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(value) => serde_json::to_vec(&value).expect("the request body serializes"),
        None => Vec::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://s3vectors.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is a required
/// `@httpQuery` list, so it travels as one query entry per key.
async fn untag(router: &MultiServiceRouter, arn: &str, tag_keys: &[String]) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect::<Vec<_>>()
        .join("&");
    send(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", label(arn)),
        None,
    )
    .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    send(router, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// `POST /tags/{resourceArn}` with a `tags` body — `TagResource`.
async fn tag(router: &MultiServiceRouter, arn: &str, tags: &Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// The tag map `ListTagsForResource` reports for `arn`, which must succeed.
async fn reported_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}) must succeed, got {status} {body}"
    );
    body.get("tags").cloned().unwrap_or(Value::Null)
}

/// The `message` member of an error body, whichever casing the protocol rendered.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A JSON object from key/value pairs, for comparing a reported tag map whole.
fn tag_object(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// Owned key/value pairs from literals, so a table row can mix literal and generated
/// keys.
fn pairs(literal: &[(&str, &str)]) -> Vec<(String, String)> {
    literal
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Owned tag keys from literals.
fn keys(literal: &[&str]) -> Vec<String> {
    literal.iter().map(|key| (*key).to_string()).collect()
}

/// Which kind of resource a `resourceArn` names. Both are tagged resources: "the
/// tagged resource can be a vector bucket or a vector index".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bucket,
    Index,
}

/// The bucket and index names every fixture in this suite uses.
const BUCKET: &str = "tagged-bucket";
const INDEX: &str = "tagged-index";

/// Create a vector bucket (and, for [`Kind::Index`], an index in it) carrying `tags`,
/// and answer with the ARN of the resource the tags are on.
async fn resource_with_tags(
    router: &MultiServiceRouter,
    kind: Kind,
    tags: &[(String, String)],
) -> String {
    let tag_map = tag_object(tags);
    let mut bucket_request = json!({ "vectorBucketName": BUCKET });
    let mut index_request = json!({
        "vectorBucketName": BUCKET,
        "indexName": INDEX,
        "dataType": "float32",
        "dimension": 2,
        "distanceMetric": "cosine",
    });
    // The tags go on the resource under test; the resource that merely holds it is
    // created untagged.
    match kind {
        Kind::Bucket => bucket_request["tags"] = tag_map,
        Kind::Index => index_request["tags"] = tag_map,
    }
    let created = ok(router, "CreateVectorBucket", &bucket_request).await;
    assert_eq!(
        created["vectorBucketArn"].as_str(),
        Some(expected_bucket_arn(BUCKET).as_str()),
        "CreateVectorBucket must mint the modelled bucket ARN, got {created}"
    );
    match kind {
        Kind::Bucket => expected_bucket_arn(BUCKET),
        Kind::Index => {
            let created = ok(router, "CreateIndex", &index_request).await;
            assert_eq!(
                created["indexArn"].as_str(),
                Some(expected_index_arn(BUCKET, INDEX).as_str()),
                "CreateIndex must mint the modelled index ARN, got {created}"
            );
            expected_index_arn(BUCKET, INDEX)
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a removal takes exactly the keys it names
// ---------------------------------------------------------------------------

/// One removal row: the resource kind, the tags it is created with, the keys the
/// request removes, and the tag map that must remain.
#[derive(Debug)]
struct RemovalCase {
    case: &'static str,
    kind: Kind,
    stored: Vec<(String, String)>,
    remove: Vec<String>,
    remaining: Vec<(String, String)>,
}

/// A `TagKey` at its modelled maximum length: `TagKey` is `length 1..128`
/// (`smithy_query(query="shape",shape="TagKey")`).
fn longest_key() -> String {
    "k".repeat(128)
}

/// A `TagKey` carrying every punctuation character the shape's `@pattern`
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits beside letters, digits and separators.
const PUNCTUATION_KEY: &str = "cost_center.unit:scope/sub=v+1-2@aws";

/// The rows. They differ along the resource kind, how many tags are stored, how many
/// keys are removed, whether those keys are present, and the key FORM (shortest,
/// longest and full-punctuation keys, from `TagKey`'s own bounds and pattern). The
/// issue cited none of these: it only reported that the operation answered 501.
fn removal_cases() -> Vec<RemovalCase> {
    let three = pairs(&[("team", "web"), ("env", "test"), ("owner", "nobody")]);
    let dozen: Vec<(String, String)> = (0..12)
        .map(|n| (format!("key-{n:02}"), format!("value-{n}")))
        .collect();
    vec![
        RemovalCase {
            case: "one key of three, on a vector bucket",
            kind: Kind::Bucket,
            stored: three.clone(),
            remove: keys(&["env"]),
            remaining: pairs(&[("team", "web"), ("owner", "nobody")]),
        },
        RemovalCase {
            case: "two keys of three, on a vector bucket",
            kind: Kind::Bucket,
            stored: three.clone(),
            remove: keys(&["team", "owner"]),
            remaining: pairs(&[("env", "test")]),
        },
        RemovalCase {
            case: "every key, leaving the bucket with no tags",
            kind: Kind::Bucket,
            stored: three.clone(),
            remove: keys(&["team", "env", "owner"]),
            remaining: Vec::new(),
        },
        RemovalCase {
            case: "the only key a bucket carries",
            kind: Kind::Bucket,
            stored: pairs(&[("team", "web")]),
            remove: keys(&["team"]),
            remaining: Vec::new(),
        },
        RemovalCase {
            case: "a key the resource does not carry",
            kind: Kind::Bucket,
            stored: pairs(&[("team", "web"), ("env", "test")]),
            remove: keys(&["absent"]),
            remaining: pairs(&[("team", "web"), ("env", "test")]),
        },
        RemovalCase {
            case: "a present key beside an absent one",
            kind: Kind::Bucket,
            stored: pairs(&[("team", "web"), ("env", "test")]),
            remove: keys(&["env", "absent"]),
            remaining: pairs(&[("team", "web")]),
        },
        RemovalCase {
            case: "the same key named twice in one request",
            kind: Kind::Bucket,
            stored: pairs(&[("team", "web"), ("env", "test")]),
            remove: keys(&["team", "team"]),
            remaining: pairs(&[("env", "test")]),
        },
        RemovalCase {
            // A tag key is matched exactly: the stored map is keyed by the key string
            // the write carried, so a key differing only in case is a different key.
            case: "a key differing from a stored one only in case",
            kind: Kind::Bucket,
            stored: pairs(&[("team", "web")]),
            remove: keys(&["Team"]),
            remaining: pairs(&[("team", "web")]),
        },
        RemovalCase {
            case: "half of a dozen keys, on a vector bucket",
            kind: Kind::Bucket,
            stored: dozen.clone(),
            remove: dozen
                .iter()
                .enumerate()
                .filter(|(n, _)| n % 2 == 0)
                .map(|(_, (key, _))| key.clone())
                .collect(),
            remaining: dozen
                .iter()
                .enumerate()
                .filter(|(n, _)| n % 2 == 1)
                .map(|(_, pair)| pair.clone())
                .collect(),
        },
        RemovalCase {
            case: "the longest modelled key, beside a one-character key",
            kind: Kind::Bucket,
            stored: vec![
                ("a".to_string(), "short".to_string()),
                (longest_key(), "long".to_string()),
            ],
            remove: vec![longest_key()],
            remaining: vec![("a".to_string(), "short".to_string())],
        },
        RemovalCase {
            case: "a one-character key, leaving the longest one",
            kind: Kind::Bucket,
            stored: vec![
                ("a".to_string(), "short".to_string()),
                (longest_key(), "long".to_string()),
            ],
            remove: keys(&["a"]),
            remaining: vec![(longest_key(), "long".to_string())],
        },
        RemovalCase {
            case: "a key carrying every punctuation character the pattern admits",
            kind: Kind::Bucket,
            stored: vec![
                (PUNCTUATION_KEY.to_string(), "yes".to_string()),
                ("plain".to_string(), "kept".to_string()),
            ],
            remove: vec![PUNCTUATION_KEY.to_string()],
            remaining: vec![("plain".to_string(), "kept".to_string())],
        },
        RemovalCase {
            case: "one key of three, on a vector index",
            kind: Kind::Index,
            stored: three.clone(),
            remove: keys(&["owner"]),
            remaining: pairs(&[("team", "web"), ("env", "test")]),
        },
        RemovalCase {
            case: "every key, leaving the index with no tags",
            kind: Kind::Index,
            stored: three,
            remove: keys(&["team", "env", "owner"]),
            remaining: Vec::new(),
        },
        RemovalCase {
            case: "a key on an index that carries no tags at all",
            kind: Kind::Index,
            stored: Vec::new(),
            remove: keys(&["team"]),
            remaining: Vec::new(),
        },
    ]
}

/// `UntagResource` removes exactly the named keys from the resource the `resourceArn`
/// names, leaves every other key and value alone, and is idempotent: naming a key the
/// resource does not carry — including one a previous call already removed — succeeds
/// and changes nothing.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    for row in removal_cases() {
        let RemovalCase {
            case,
            kind,
            stored,
            remove,
            remaining,
        } = row;
        let router = router();
        let arn = resource_with_tags(&router, kind, &stored).await;

        // The tags the create wrote are what the reader reports before any removal.
        let before = reported_tags(&router, case, &arn).await;
        assert_eq!(
            before,
            tag_object(&stored),
            "case {case:?}: the create's tags must be what the reader reports, got {before}"
        );

        let (status, body) = untag(&router, &arn, &remove).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource({remove:?}) must succeed, got {status} {body}"
        );

        let expected = tag_object(&remaining);
        let actual = reported_tags(&router, case, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: after removing {remove:?} the resource must carry {expected}, got {actual}"
        );

        // Idempotence: the same removal again is accepted and leaves the same map, so
        // every key it names is now absent.
        let (status, body) = untag(&router, &arn, &remove).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating UntagResource({remove:?}) must succeed, got {status} {body}"
        );
        let actual = reported_tags(&router, case, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: repeating the removal must change nothing, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the resourceArn must resolve to a stored vector bucket or vector index
// ---------------------------------------------------------------------------

/// One `resourceArn` row: the ARN, and the status and error code every tagging route
/// must answer it with.
#[derive(Debug)]
struct ResourceArnCase {
    case: &'static str,
    arn: String,
    status: u16,
    /// The wire error code, or `""` for a row that must succeed.
    code: &'static str,
}

/// The canonical account id every emulated ARN carries.
fn account() -> &'static str {
    engine::EMULATOR_ACCOUNT_ID
}

/// A vector bucket that was created and then deleted, and an index of the live bucket
/// that was created and then deleted: the recorded unit this suite answers untags a
/// DELETED bucket.
const GONE_BUCKET: &str = "gone-bucket";
const GONE_INDEX: &str = "gone-index";

/// The fixture every `resourceArn` row runs against: a tagged vector bucket holding a
/// tagged vector index, plus a deleted bucket and a deleted index.
async fn arn_fixture() -> MultiServiceRouter {
    let router = router();
    let tags = pairs(&[("team", "web"), ("env", "test")]);
    resource_with_tags(&router, Kind::Index, &tags).await;
    // The live bucket's own tags, so a bucket row has something to remove.
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": GONE_INDEX,
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
            "tags": { "team": "web" },
        }),
    )
    .await;
    ok(
        &router,
        "DeleteIndex",
        &json!({ "vectorBucketName": BUCKET, "indexName": GONE_INDEX }),
    )
    .await;
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": GONE_BUCKET, "tags": { "team": "web" } }),
    )
    .await;
    ok(
        &router,
        "DeleteVectorBucket",
        &json!({ "vectorBucketName": GONE_BUCKET }),
    )
    .await;
    router
}

/// The rows. The resolving ones differ in the kind of resource named; the 404 ones in
/// HOW the resource is missing (never created, deleted, or a live bucket's missing
/// index); the 400 ones in which clause of the ARN form they break. `ResourceARN`
/// carries no `@pattern` of its own
/// (`smithy_query(query="shape",shape="ResourceARN")`), so the form is the one
/// `VectorBucketArn` and `IndexArn` declare.
fn resource_arn_cases() -> Vec<ResourceArnCase> {
    let account = account();
    vec![
        ResourceArnCase {
            case: "a vector bucket that exists",
            arn: expected_bucket_arn(BUCKET),
            status: 200,
            code: "",
        },
        ResourceArnCase {
            case: "a vector index that exists",
            arn: expected_index_arn(BUCKET, INDEX),
            status: 200,
            code: "",
        },
        ResourceArnCase {
            case: "a vector bucket that was never created",
            arn: expected_bucket_arn("never-created"),
            status: 404,
            code: "NotFoundException",
        },
        ResourceArnCase {
            case: "a vector bucket that was deleted",
            arn: expected_bucket_arn(GONE_BUCKET),
            status: 404,
            code: "NotFoundException",
        },
        ResourceArnCase {
            case: "a vector index that was deleted",
            arn: expected_index_arn(BUCKET, GONE_INDEX),
            status: 404,
            code: "NotFoundException",
        },
        ResourceArnCase {
            case: "an index name the live bucket does not hold",
            arn: expected_index_arn(BUCKET, "never-created"),
            status: 404,
            code: "NotFoundException",
        },
        ResourceArnCase {
            case: "an index of a bucket that does not exist",
            arn: expected_index_arn("never-created", INDEX),
            status: 404,
            code: "NotFoundException",
        },
        ResourceArnCase {
            case: "not an ARN at all",
            arn: "not-an-arn".to_string(),
            status: 400,
            code: "ValidationException",
        },
        ResourceArnCase {
            case: "the bucket name on its own",
            arn: BUCKET.to_string(),
            status: 400,
            code: "ValidationException",
        },
        ResourceArnCase {
            case: "an ARN of another service",
            arn: format!("arn:aws:s3:{REGION}:{account}:bucket/{BUCKET}"),
            status: 400,
            code: "ValidationException",
        },
        ResourceArnCase {
            case: "an ARN that stops before the resource",
            arn: format!("arn:aws:s3vectors:{REGION}:{account}"),
            status: 400,
            code: "ValidationException",
        },
        ResourceArnCase {
            case: "a resource segment that is not a bucket",
            arn: format!("arn:aws:s3vectors:{REGION}:{account}:index/{BUCKET}"),
            status: 400,
            code: "ValidationException",
        },
        ResourceArnCase {
            case: "an empty bucket segment",
            arn: format!("arn:aws:s3vectors:{REGION}:{account}:bucket/"),
            status: 400,
            code: "ValidationException",
        },
        ResourceArnCase {
            case: "an empty index segment",
            arn: format!("arn:aws:s3vectors:{REGION}:{account}:bucket/{BUCKET}/index/"),
            status: 400,
            code: "ValidationException",
        },
        ResourceArnCase {
            case: "a path past the index segment",
            arn: format!(
                "arn:aws:s3vectors:{REGION}:{account}:bucket/{BUCKET}/index/{INDEX}/vectors"
            ),
            status: 400,
            code: "ValidationException",
        },
    ]
}

/// Both `/tags/{resourceArn}` routes resolve the ARN by the same rule: a stored vector
/// bucket or vector index, or a modelled refusal. A removal on a resource this store
/// does not hold is the 404 the model declares — never a success that silently writes
/// nothing.
#[tokio::test]
async fn the_resource_arn_a_tagging_call_names_must_resolve_to_a_stored_resource() {
    for row in resource_arn_cases() {
        for route in ["POST", "DELETE", "GET"] {
            // A fresh fixture per route: the write routes of a resolving ARN really do
            // change the tag map, which the GET row must not see.
            let router = arn_fixture().await;
            let (status, body) = match route {
                "POST" => tag(&router, &row.arn, &json!({ "team": "web" })).await,
                "DELETE" => untag(&router, &row.arn, &keys(&["team"])).await,
                _ => list_tags(&router, &row.arn).await,
            };
            let case = row.case;
            let arn = &row.arn;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (row.status, row.code),
                "case {case:?} on {route} /tags/{arn}: got {status} {body}"
            );
            if row.status == 200 && route == "GET" {
                assert!(
                    body.get("tags").is_some_and(Value::is_object),
                    "case {case:?}: ListTagsForResource must report a tag map, got {body}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The modelled tagKeys bounds, and that one resource's removal is only its own
// ---------------------------------------------------------------------------

/// The sentence the real service answered for a missing vector bucket, as the
/// recorded response carried it. A LITERAL, not the handler's own constant: a suite
/// asserting the constant would agree with any wording the handler chose.
const RECORDED_BUCKET_NOT_FOUND: &str = "The specified vector bucket could not be found";

/// The same sentence for the other resource kind.
const EXPECTED_INDEX_NOT_FOUND: &str = "The specified vector index could not be found";

/// The `NotFoundException` a tagging call answers for a resource this account does not
/// hold carries ONE fixed sentence per resource KIND: it names the kind of resource
/// that is missing and no value the request carried, so two different missing
/// resources of the same kind are reported identically, and the two kinds differ.
#[tokio::test]
async fn the_not_found_message_names_the_resource_kind_and_no_request_value() {
    // (case, arn, the expected message). The rows differ in the kind of resource and
    // in HOW it is missing; the recorded unit this suite answers is the deleted
    // bucket row.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a bucket that was never created",
            expected_bucket_arn("never-created"),
            RECORDED_BUCKET_NOT_FOUND,
        ),
        (
            "another bucket that was never created",
            expected_bucket_arn("also-never-created"),
            RECORDED_BUCKET_NOT_FOUND,
        ),
        (
            "a bucket that was deleted",
            expected_bucket_arn(GONE_BUCKET),
            RECORDED_BUCKET_NOT_FOUND,
        ),
        (
            "an index that was deleted",
            expected_index_arn(BUCKET, GONE_INDEX),
            EXPECTED_INDEX_NOT_FOUND,
        ),
        (
            "an index the live bucket does not hold",
            expected_index_arn(BUCKET, "never-created"),
            EXPECTED_INDEX_NOT_FOUND,
        ),
        (
            // The bucket is resolved first, so the missing resource is the BUCKET.
            "an index of a bucket that does not exist",
            expected_index_arn("never-created", INDEX),
            RECORDED_BUCKET_NOT_FOUND,
        ),
    ];
    assert_ne!(
        RECORDED_BUCKET_NOT_FOUND, EXPECTED_INDEX_NOT_FOUND,
        "the two resource kinds must be reported with different sentences"
    );

    let router = arn_fixture().await;
    for (case, arn, expected) in &cases {
        for route in ["POST", "DELETE", "GET"] {
            let (status, body) = match route {
                "POST" => tag(&router, arn, &json!({ "team": "web" })).await,
                "DELETE" => untag(&router, arn, &keys(&["team"])).await,
                _ => list_tags(&router, arn).await,
            };
            let message = error_message(&body);
            assert_eq!(
                (status, error_code(&body).as_str(), message.as_str()),
                (404, "NotFoundException", *expected),
                "case {case:?} on {route} /tags/{arn}: got {status} {body}"
            );
            assert!(
                !message.contains("never-created") && !message.contains(GONE_BUCKET),
                "case {case:?}: the message must carry no value from the request, got {message:?}"
            );
        }
    }
}

/// `tagKeys` is a required `@httpQuery` list of `TagKey` (`length 1..128`, pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), so a request whose keys fall outside those
/// bounds is refused before the handler runs — a 400, never the operation's 404 and
/// never a success.
#[tokio::test]
async fn untag_resource_refuses_tag_keys_outside_the_modelled_bounds() {
    // (case, the query string after `?`, accepted). One row per clause of the
    // member's rules, with the longest accepted key as the row that must pass.
    let longest = longest_key();
    let cases: Vec<(&str, String, bool)> = vec![
        ("the longest accepted key", format!("tagKeys={longest}"), true),
        (
            "one character past the maximum",
            format!("tagKeys={}", "k".repeat(129)),
            false,
        ),
        ("an empty key", "tagKeys=".to_string(), false),
        (
            "a character outside the pattern",
            format!("tagKeys={}", label("no!")),
            false,
        ),
    ];

    for (case, query, accepted) in &cases {
        let router = router();
        let arn = resource_with_tags(&router, Kind::Bucket, &pairs(&[("team", "web")])).await;
        let path = format!("/tags/{}?{query}", label(&arn));
        let (status, body) = send(&router, "DELETE", &path, None).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {query:?} must be accepted, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {query:?} must be refused with 400, got {status} {body}"
            );
        }
        // Either way the stored tag — which no row names — is still there.
        assert_eq!(
            reported_tags(&router, case, &arn).await,
            tag_object(&pairs(&[("team", "web")])),
            "case {case:?}: a key the request never named must survive"
        );
    }
}

/// A removal reaches ONE resource: the tags of the sibling bucket and of the index the
/// bucket holds are untouched, and reading a tag map never changes what a later caller
/// sees.
#[tokio::test]
async fn a_removal_reaches_only_the_resource_its_arn_names() {
    let router = router();
    let shared = pairs(&[("team", "web"), ("env", "test")]);
    let bucket_arn = resource_with_tags(&router, Kind::Bucket, &shared).await;
    let index_arn = expected_index_arn(BUCKET, INDEX);
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
            "tags": { "team": "web", "env": "test" },
        }),
    )
    .await;
    let sibling = "sibling-bucket";
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": sibling, "tags": { "team": "web", "env": "test" } }),
    )
    .await;
    let sibling_arn = expected_bucket_arn(sibling);

    let (status, body) = untag(&router, &bucket_arn, &keys(&["team"])).await;
    assert_eq!(
        status, 200,
        "untagging the bucket must succeed, got {status} {body}"
    );
    assert_eq!(
        reported_tags(&router, "the untagged bucket", &bucket_arn).await,
        tag_object(&pairs(&[("env", "test")])),
        "the bucket must have lost exactly `team`"
    );
    for (case, arn) in [("the sibling bucket", &sibling_arn), ("the index", &index_arn)] {
        assert_eq!(
            reported_tags(&router, case, arn).await,
            tag_object(&shared),
            "case {case:?}: a removal on another resource must not touch its tags"
        );
    }

    // The index's own removal is likewise only its own.
    let (status, body) = untag(&router, &index_arn, &keys(&["env"])).await;
    assert_eq!(
        status, 200,
        "untagging the index must succeed, got {status} {body}"
    );
    assert_eq!(
        reported_tags(&router, "the untagged index", &index_arn).await,
        tag_object(&pairs(&[("team", "web")])),
        "the index must have lost exactly `env`"
    );
    assert_eq!(
        reported_tags(&router, "the bucket after the index removal", &bucket_arn).await,
        tag_object(&pairs(&[("env", "test")])),
        "the bucket's tags must be unchanged by a removal on its index"
    );

    // A read does not change what a later caller sees.
    let first = reported_tags(&router, "the sibling bucket, first read", &sibling_arn).await;
    let second = reported_tags(&router, "the sibling bucket, second read", &sibling_arn).await;
    assert_eq!(
        (&first, &second),
        (&tag_object(&shared), &tag_object(&shared)),
        "reading a tag map must report the same map every time, got {first} then {second}"
    );
}

// ---------------------------------------------------------------------------
// The write a removal removes from: TagResource -> ListTagsForResource -> UntagResource
// ---------------------------------------------------------------------------

/// One write row: the resource kind, the tags its create carried, the tags the
/// `TagResource` call applies, and the map the resource must then carry.
#[derive(Debug)]
struct WriteCase {
    case: &'static str,
    kind: Kind,
    created: Vec<(String, String)>,
    applied: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// A `TagValue` at its modelled maximum length: `TagValue` is `length 0..256`
/// (`smithy_query(query="shape",shape="TagValue")`), so the empty string is a legal
/// value too.
fn longest_value() -> String {
    "v".repeat(256)
}

/// The rows. They differ along the resource kind, whether the applied keys are new or
/// already present, how many are applied, and the key/value FORM (the `TagKey` and
/// `TagValue` bounds, including an empty value).
fn write_cases() -> Vec<WriteCase> {
    vec![
        WriteCase {
            case: "one tag onto an untagged bucket",
            kind: Kind::Bucket,
            created: Vec::new(),
            applied: pairs(&[("team", "web")]),
            expected: pairs(&[("team", "web")]),
        },
        WriteCase {
            case: "three tags onto an untagged bucket",
            kind: Kind::Bucket,
            created: Vec::new(),
            applied: pairs(&[("team", "web"), ("env", "test"), ("owner", "nobody")]),
            expected: pairs(&[("team", "web"), ("env", "test"), ("owner", "nobody")]),
        },
        WriteCase {
            case: "a new key beside one the create wrote",
            kind: Kind::Bucket,
            created: pairs(&[("team", "web")]),
            applied: pairs(&[("env", "test")]),
            expected: pairs(&[("team", "web"), ("env", "test")]),
        },
        WriteCase {
            case: "a key the resource already carries, with a new value",
            kind: Kind::Bucket,
            created: pairs(&[("team", "web")]),
            applied: pairs(&[("team", "infra")]),
            expected: pairs(&[("team", "infra")]),
        },
        WriteCase {
            case: "an update and an addition in one request",
            kind: Kind::Bucket,
            created: pairs(&[("team", "web"), ("env", "test")]),
            applied: pairs(&[("team", "infra"), ("owner", "nobody")]),
            expected: pairs(&[("team", "infra"), ("env", "test"), ("owner", "nobody")]),
        },
        WriteCase {
            case: "the same key and value the create wrote",
            kind: Kind::Bucket,
            created: pairs(&[("team", "web")]),
            applied: pairs(&[("team", "web")]),
            expected: pairs(&[("team", "web")]),
        },
        WriteCase {
            case: "no tags at all",
            kind: Kind::Bucket,
            created: pairs(&[("team", "web")]),
            applied: Vec::new(),
            expected: pairs(&[("team", "web")]),
        },
        WriteCase {
            case: "the shortest legal value, the empty string",
            kind: Kind::Bucket,
            created: Vec::new(),
            applied: pairs(&[("team", "")]),
            expected: pairs(&[("team", "")]),
        },
        WriteCase {
            case: "the longest modelled key and value",
            kind: Kind::Bucket,
            created: Vec::new(),
            applied: vec![(longest_key(), longest_value())],
            expected: vec![(longest_key(), longest_value())],
        },
        WriteCase {
            case: "two tags onto an untagged index",
            kind: Kind::Index,
            created: Vec::new(),
            applied: pairs(&[("team", "web"), ("env", "test")]),
            expected: pairs(&[("team", "web"), ("env", "test")]),
        },
        WriteCase {
            case: "an update on an index the create tagged",
            kind: Kind::Index,
            created: pairs(&[("team", "web"), ("env", "test")]),
            applied: pairs(&[("env", "prod")]),
            expected: pairs(&[("team", "web"), ("env", "prod")]),
        },
    ]
}

/// `TagResource` merges the tags it names into the resource's map — replacing the
/// value of a key already there and leaving every unnamed key alone — and that map is
/// exactly what the reader reports and what a removal removes from.
#[tokio::test]
async fn tag_resource_writes_the_map_the_reader_reports_and_a_removal_removes_from() {
    for row in write_cases() {
        let WriteCase {
            case,
            kind,
            created,
            applied,
            expected,
        } = row;
        let router = router();
        let arn = resource_with_tags(&router, kind, &created).await;

        let (status, body) = tag(&router, &arn, &tag_object(&applied)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must succeed, got {status} {body}"
        );
        let actual = reported_tags(&router, case, &arn).await;
        let expected_map = tag_object(&expected);
        assert_eq!(
            actual, expected_map,
            "case {case:?}: the resource must carry {expected_map}, got {actual}"
        );

        // Writing the same request again changes nothing: the merge is idempotent.
        let (status, body) = tag(&router, &arn, &tag_object(&applied)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating TagResource must succeed, got {status} {body}"
        );
        assert_eq!(
            reported_tags(&router, case, &arn).await,
            expected_map,
            "case {case:?}: repeating the write must leave the same map"
        );

        // And the removal takes exactly the keys the write named back out, leaving the
        // keys the create wrote that the write did not name. `tagKeys` is a required
        // member, so a request naming no key cannot be expressed on the wire at all.
        let applied_keys: Vec<String> = applied.iter().map(|(key, _)| key.clone()).collect();
        if applied_keys.is_empty() {
            continue;
        }
        let (status, body) = untag(&router, &arn, &applied_keys).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource({applied_keys:?}) must succeed, got {status} {body}"
        );
        let survivors: Vec<(String, String)> = created
            .iter()
            .filter(|(key, _)| !applied_keys.contains(key))
            .cloned()
            .collect();
        let actual = reported_tags(&router, case, &arn).await;
        assert_eq!(
            actual,
            tag_object(&survivors),
            "case {case:?}: removing {applied_keys:?} must leave {:?}, got {actual}",
            tag_object(&survivors)
        );
    }
}

/// One ceiling row: how many tags the resource is created with, how many NEW keys the
/// write adds, whether it also re-writes the keys already there, and whether the write
/// is accepted.
#[derive(Debug)]
struct CeilingCase {
    case: &'static str,
    created: usize,
    added: usize,
    updates_existing: bool,
    accepted: bool,
}

/// `count` tags named `{prefix}-NNN`, all carrying `value`.
fn tag_set(prefix: &str, count: usize, value: &str) -> Vec<(String, String)> {
    (0..count)
        .map(|n| (format!("{prefix}-{n:03}"), value.to_string()))
        .collect()
}

/// A resource carries at most 50 tags — "you can add up to 50 tags for each resource"
/// — counted over what the write would LEAVE it with: a write past the ceiling is
/// refused with 400 and stores nothing, while a write that only updates keys the
/// resource already carries is accepted however full it is.
#[tokio::test]
async fn a_tag_write_past_the_documented_ceiling_is_refused() {
    // The ceiling itself and one key past it are both rows, reached in one write and
    // in two.
    let cases = [
        CeilingCase {
            case: "exactly the ceiling in one write",
            created: 0,
            added: MAX_TAGS_PER_RESOURCE,
            updates_existing: false,
            accepted: true,
        },
        CeilingCase {
            case: "one below the ceiling",
            created: 0,
            added: MAX_TAGS_PER_RESOURCE - 1,
            updates_existing: false,
            accepted: true,
        },
        CeilingCase {
            case: "one past the ceiling in one write",
            created: 0,
            added: MAX_TAGS_PER_RESOURCE + 1,
            updates_existing: false,
            accepted: false,
        },
        CeilingCase {
            case: "the ceiling reached in two writes",
            created: MAX_TAGS_PER_RESOURCE - 1,
            added: 1,
            updates_existing: false,
            accepted: true,
        },
        CeilingCase {
            case: "a full resource, one more key",
            created: MAX_TAGS_PER_RESOURCE,
            added: 1,
            updates_existing: true,
            accepted: false,
        },
        CeilingCase {
            case: "a full resource, only updates",
            created: MAX_TAGS_PER_RESOURCE,
            added: 0,
            updates_existing: true,
            accepted: true,
        },
    ];

    for row in &cases {
        let CeilingCase {
            case,
            created,
            added,
            updates_existing,
            accepted,
        } = *row;
        let router = router();
        let stored = tag_set("key", created, "created");
        let arn = resource_with_tags(&router, Kind::Bucket, &stored).await;

        let updated = tag_set("key", created, "updated");
        let additions = tag_set("added", added, "new");
        let mut write: Vec<(String, String)> = Vec::new();
        if updates_existing {
            write.extend(updated.iter().cloned());
        }
        write.extend(additions.iter().cloned());

        let (status, body) = tag(&router, &arn, &tag_object(&write)).await;
        let reported = reported_tags(&router, case, &arn).await;
        if accepted {
            assert_eq!(
                status,
                200,
                "case {case:?}: a write leaving {} tag(s) must be accepted, got {status} {body}",
                created + added
            );
            let mut expected = if updates_existing {
                updated.clone()
            } else {
                stored.clone()
            };
            expected.extend(additions.iter().cloned());
            assert_eq!(
                reported,
                tag_object(&expected),
                "case {case:?}: the resource must carry every written tag, got {reported}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a write leaving {} tag(s) must be refused, got {status} {body}",
                created + added
            );
            assert_eq!(
                reported,
                tag_object(&stored),
                "case {case:?}: a refused write must store nothing, got {reported}"
            );
        }
    }
}
