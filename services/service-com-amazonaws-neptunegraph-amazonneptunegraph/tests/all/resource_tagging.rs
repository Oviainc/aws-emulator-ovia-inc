// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tagging surface — `ListTagsForResource`, `TagResource` and
//! `UntagResource` — driven over the wire (`restJson1`) through the same router the
//! emulator serves from.
//!
//! The rule every assertion here is about: the tag set a resource carries is persisted
//! ON that resource and reported back exactly as stored, whether it was supplied at
//! create time or written later; tagging merges the keys it is sent and untagging
//! removes only the keys it names; and an ARN that names no stored resource is refused
//! `ResourceNotFoundException` (404) by all three operations.
//!
//! One further rule is about which tags a request may supply at all: neither half of a
//! caller-supplied entry may open with the prefix AWS keeps for the tags it applies
//! itself, so a request naming one is refused `ValidationException` (400) and writes
//! nothing, whichever operation carried the tags.
//!
//! So the tables below span what that rule generalises over — entry count, key and
//! value form, the length and entry bounds the model publishes, which call wrote the
//! tag, and which keys a later call mentions — rather than re-running one recorded
//! request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    MAX_TAGS, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names, so the ARNs the service mints
/// are predictable.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Percent-encode one path segment or query value.
///
/// `resourceArn` is bound to a NON-greedy `{resourceArn}` label (`/tags/{resourceArn}`),
/// so the `/` inside an ARN has to arrive encoded or it would split the URI into another
/// path segment; a tag key goes into the `tagKeys` query string, which is encoded for the
/// same reason. Only the RFC 3986 unreserved set is left as-is.
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `POST /graphs` with `tags`, as the service answered it.
async fn create_graph_answer(
    router: &MultiServiceRouter,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let body = json!({
        "graphName": name,
        "provisionedMemory": MIN_PROVISIONED_MEMORY,
        "tags": tags,
    });
    call(router, "POST", "/graphs", Some(body)).await
}

/// `POST /graphs` with `tags`, returning the created graph's ARN.
async fn create_graph(
    router: &MultiServiceRouter,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let (status, created) = create_graph_answer(router, name, tags).await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no arn in {created}"))
        .to_string()
}

/// `GET /tags/{resourceArn}`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", encode(arn)), None).await
}

/// `POST /tags/{resourceArn}` with a `tags` map.
async fn tag(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode(arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…`, one query entry per key.
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[String]) -> (u16, Value) {
    let query: Vec<String> = keys.iter().map(|key| format!("tagKeys={}", encode(key))).collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", encode(arn), query.join("&")),
        None,
    )
    .await
}

/// The tag set `ListTagsForResource` reported, as a map.
///
/// A resource with no tags carries an EMPTY set; whether the envelope renders that as
/// `{}` or omits the member is a serialization detail, so both read as "no tags" here.
/// The answer that distinguishes a resource is the 404 a missing one gets.
async fn stored_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {arn:?}: expected 200 from ListTagsForResource, got {status}: {body}"
    );
    match body.get("tags") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("case {arn:?}: tag {key:?} is not a string"))
                        .to_string(),
                )
            })
            .collect(),
        Some(other) => panic!("case {arn:?}: expected a tags object, got {other}"),
    }
}

/// A tag map from `(key, value)` pairs.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct tags, for the entry-count rows.
fn many_tags(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect()
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_tag_set_stored_at_create_time() {
    let router = emulator();

    // (case, the tags the graph was created with). The rows differ along entry count,
    // key form, value form and the bounds the model publishes for each:
    // `smithy_query(query="shape",shape="TagMap,TagKey,TagValue")` ->
    // TagMap @length(0..50), TagKey @length(1..128) @pattern("^[a-zA-Z+-=._:/]+$"),
    // TagValue @length(0..256).
    let cases: Vec<(&str, BTreeMap<String, String>)> = vec![
        ("no tags at all", BTreeMap::new()),
        ("one tag", tags(&[("env", "prod")])),
        (
            "several tags",
            tags(&[("env", "prod"), ("team", "graphs"), ("tier", "gold")]),
        ),
        (
            "every punctuation character the modelled TagKey pattern allows",
            tags(&[
                ("a.b", "dot"),
                ("a_b", "underscore"),
                ("a:b", "colon"),
                ("a/b", "slash"),
                ("a+b", "plus"),
                ("a-b", "hyphen"),
                ("a=b", "equals"),
            ]),
        ),
        (
            "digits in the key (inside the pattern's +-= range)",
            tags(&[("key0", "zero"), ("9", "nine")]),
        ),
        ("an empty value (TagValue @length min 0)", tags(&[("empty", "")])),
        (
            "a key at the modelled length bound",
            tags(&[(&"k".repeat(MAX_TAG_KEY_CHARS), "at the key bound")]),
        ),
        (
            "a value at the modelled length bound",
            tags(&[("long", &"v".repeat(MAX_TAG_VALUE_CHARS))]),
        ),
        (
            "a value of characters outside ASCII",
            tags(&[("unicode", "tāg-vàlüe-✓-\u{1f600}")]),
        ),
        ("at the modelled entry bound", many_tags(MAX_TAGS)),
    ];

    for (index, (case, expected)) in cases.iter().enumerate() {
        let arn = create_graph(&router, &format!("tagged-graph-{index}"), expected).await;
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn tagging_merges_the_keys_it_is_sent_and_untagging_removes_only_those_it_names() {
    let router = emulator();

    // (case, the tags the graph was created with, the TagResource map, the UntagResource
    // keys, the tag set the resource must then carry). The rows differ along which call
    // wrote a key, whether a key already existed, and whether a mentioned key is present.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(
        &str,
        BTreeMap<String, String>,
        BTreeMap<String, String>,
        Vec<String>,
        BTreeMap<String, String>,
    )> = vec![
        (
            "tagging a resource that carries no tags",
            BTreeMap::new(),
            tags(&[("env", "prod")]),
            Vec::new(),
            tags(&[("env", "prod")]),
        ),
        (
            "adding a key beside the ones already stored",
            tags(&[("env", "prod")]),
            tags(&[("team", "graphs")]),
            Vec::new(),
            tags(&[("env", "prod"), ("team", "graphs")]),
        ),
        (
            "replacing the value of a key already stored",
            tags(&[("env", "dev"), ("team", "graphs")]),
            tags(&[("env", "prod")]),
            Vec::new(),
            tags(&[("env", "prod"), ("team", "graphs")]),
        ),
        (
            "an empty tag map changes nothing",
            tags(&[("env", "prod")]),
            BTreeMap::new(),
            Vec::new(),
            tags(&[("env", "prod")]),
        ),
        (
            "removing one key of several",
            tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            BTreeMap::new(),
            vec!["b".to_string()],
            tags(&[("a", "1"), ("c", "3")]),
        ),
        (
            "removing a key the resource does not carry",
            tags(&[("env", "prod")]),
            BTreeMap::new(),
            vec!["absent".to_string()],
            tags(&[("env", "prod")]),
        ),
        (
            "removing every key",
            tags(&[("a", "1"), ("b", "2")]),
            BTreeMap::new(),
            vec!["a".to_string(), "b".to_string()],
            BTreeMap::new(),
        ),
        (
            "removing a key the same round added",
            BTreeMap::new(),
            tags(&[("x", "1"), ("y", "2")]),
            vec!["x".to_string()],
            tags(&[("y", "2")]),
        ),
        (
            "a key written by TagResource at the modelled length bound",
            BTreeMap::new(),
            tags(&[(&"k".repeat(MAX_TAG_KEY_CHARS), &"v".repeat(MAX_TAG_VALUE_CHARS))]),
            Vec::new(),
            tags(&[(&"k".repeat(MAX_TAG_KEY_CHARS), &"v".repeat(MAX_TAG_VALUE_CHARS))]),
        ),
        (
            "filling a resource to the modelled entry bound",
            many_tags(MAX_TAGS / 2),
            many_tags(MAX_TAGS),
            Vec::new(),
            many_tags(MAX_TAGS),
        ),
    ];

    for (index, (case, created_with, added, removed, expected)) in cases.iter().enumerate() {
        let arn = create_graph(&router, &format!("retagged-graph-{index}"), created_with).await;

        // Both calls are `@idempotent`, so the whole round is applied TWICE and must
        // leave the same tag set as once.
        for round in 1..=2 {
            if !added.is_empty() || round == 1 {
                let (status, body) = tag(&router, &arn, added).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} round {round}: expected 200 from TagResource, \
                     got {status}: {body}"
                );
            }
            if !removed.is_empty() {
                let (status, body) = untag(&router, &arn, removed).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} round {round}: expected 200 from UntagResource, \
                     got {status}: {body}"
                );
            }
            let actual = stored_tags(&router, &arn).await;
            assert_eq!(
                actual, *expected,
                "case {case:?} round {round}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_that_names_no_stored_resource() {
    let router = emulator();
    let stored = tags(&[("env", "prod")]);
    let arn = create_graph(&router, "only-graph", &stored).await;
    let id = arn.rsplit('/').next().expect("the ARN names a graph id").to_string();

    // (case, an ARN no stored resource has). Each row differs in exactly one ARN
    // component, because an ARN names one resource in one region of one account.
    let cases: Vec<(&str, String)> = vec![
        ("another graph id", format!("arn:aws:neptune-graph:{REGION}:000000000000:graph/g-0000000000")),
        ("another region", arn.replace(REGION, "eu-west-1")),
        (
            "another account",
            format!("arn:aws:neptune-graph:{REGION}:999999999999:graph/{id}"),
        ),
        (
            "another resource type",
            format!("arn:aws:neptune-graph:{REGION}:000000000000:graph-snapshot/gs-0123456789"),
        ),
        ("another service", format!("arn:aws:s3:{REGION}:000000000000:bucket/{id}")),
        ("another partition", arn.replace("arn:aws:", "arn:aws-cn:")),
    ];

    for (case, missing) in &cases {
        let (list_status, list_body) = list_tags(&router, missing).await;
        assert_eq!(
            list_status, 404,
            "case {case:?}: expected 404 from ListTagsForResource, \
             got {list_status}: {list_body}"
        );
        assert_eq!(
            error_code(&list_body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {list_body}"
        );

        let (tag_status, tag_body) = tag(&router, missing, &tags(&[("late", "tag")])).await;
        assert_eq!(
            tag_status, 404,
            "case {case:?}: expected 404 from TagResource, got {tag_status}: {tag_body}"
        );
        assert_eq!(
            error_code(&tag_body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {tag_body}"
        );

        let (untag_status, untag_body) = untag(&router, missing, &["env".to_string()]).await;
        assert_eq!(
            untag_status, 404,
            "case {case:?}: expected 404 from UntagResource, got {untag_status}: {untag_body}"
        );
        assert_eq!(
            error_code(&untag_body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {untag_body}"
        );
    }

    // A refused call wrote nothing: the one stored resource still carries what it did.
    let actual = stored_tags(&router, &arn).await;
    assert_eq!(
        actual, stored,
        "case refusals leave the store alone: expected {stored:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn the_entry_bound_is_on_what_the_resource_holds_after_the_call() {
    let router = emulator();
    let full = many_tags(MAX_TAGS);
    let arn = create_graph(&router, "fully-tagged-graph", &full).await;

    // A key the resource already carries only replaces a value, so it is accepted at
    // the bound.
    let replacement = tags(&[("key0", "replaced")]);
    let (status, body) = tag(&router, &arn, &replacement).await;
    assert_eq!(
        status, 200,
        "case replacing a key at the entry bound: expected 200, got {status}: {body}"
    );
    let mut expected = full.clone();
    expected.insert("key0".to_string(), "replaced".to_string());
    let actual = stored_tags(&router, &arn).await;
    assert_eq!(
        actual, expected,
        "case replacing a key at the entry bound: expected {expected:?}, got {actual:?}"
    );

    // A key it does not carry would take the record past the bound.
    let (status, body) = tag(&router, &arn, &tags(&[("oneTooMany", "v")])).await;
    assert_eq!(
        status, 400,
        "case one key past the entry bound: expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case one key past the entry bound: expected ValidationException, got {body}"
    );
    let actual = stored_tags(&router, &arn).await;
    assert_eq!(
        actual, expected,
        "case one key past the entry bound: the refused call must write nothing, \
         expected {expected:?}, got {actual:?}"
    );

    // Freeing an entry makes room for exactly one more key.
    let (status, body) = untag(&router, &arn, &["key0".to_string()]).await;
    assert_eq!(
        status, 200,
        "case freeing one entry: expected 200, got {status}: {body}"
    );
    let (status, body) = tag(&router, &arn, &tags(&[("oneTooMany", "v")])).await;
    assert_eq!(
        status, 200,
        "case the freed entry: expected 200, got {status}: {body}"
    );
    expected.remove("key0");
    expected.insert("oneTooMany".to_string(), "v".to_string());
    let actual = stored_tags(&router, &arn).await;
    assert_eq!(
        actual, expected,
        "case the freed entry: expected {expected:?}, got {actual:?}"
    );
    assert_eq!(
        actual.len(),
        MAX_TAGS,
        "case the freed entry: expected {MAX_TAGS} entries, got {}",
        actual.len()
    );
}

#[tokio::test]
async fn each_resource_carries_its_own_tag_set() {
    let router = emulator();

    // Two graphs, each tagged differently, so a lookup by ARN cannot be answered from
    // the wrong record.
    let first = tags(&[("env", "prod"), ("shared", "first")]);
    let second = tags(&[("shared", "second")]);
    let first_arn = create_graph(&router, "first-graph", &first).await;
    let second_arn = create_graph(&router, "second-graph", &second).await;

    let (status, body) = tag(&router, &first_arn, &tags(&[("only-on-first", "yes")])).await;
    assert_eq!(status, 200, "case tagging the first graph: got {status}: {body}");
    let (status, body) = untag(&router, &second_arn, &["shared".to_string()]).await;
    assert_eq!(status, 200, "case untagging the second graph: got {status}: {body}");

    let mut expected_first = first.clone();
    expected_first.insert("only-on-first".to_string(), "yes".to_string());
    let actual_first = stored_tags(&router, &first_arn).await;
    assert_eq!(
        actual_first, expected_first,
        "case the first graph: expected {expected_first:?}, got {actual_first:?}"
    );
    let actual_second = stored_tags(&router, &second_arn).await;
    assert_eq!(
        actual_second,
        BTreeMap::new(),
        "case the second graph: expected no tags, got {actual_second:?}"
    );
}

/// The prefix AWS keeps for the tags it applies itself, restated here independently of
/// the service: the member documentation says a key and a value alike "can't be prefixed
/// with `aws:`" (`smithy_query(query="doc",shape="TagResourceInput",member="tags")`).
const RESERVED_PREFIX: &str = "aws:";

/// Whether one half of a tag entry lies in the reserved namespace — the rule restated
/// for this suite, so the expectations below do not come from the service's own helper.
/// The prefix is matched however it is cased, since a cased spelling of it still reads as
/// a tag AWS applied itself.
fn is_reserved(text: &str) -> bool {
    text.to_lowercase().starts_with(RESERVED_PREFIX)
}

/// (case, the tags a request carries, whether the service may store them).
///
/// The rows differ along which half of an entry carries the reserved prefix, how that
/// prefix is cased, where in the text it sits, and whether allowed entries travel beside
/// a reserved one — which is what the rule generalises over. Every key here is inside
/// `TagKey`'s modelled alphabet (`smithy_query(query="shape",shape="TagKey")`:
/// `^[a-zA-Z+-=._:/]+$`), so no row can be refused by the boundary walk instead.
fn reserved_namespace_cases() -> Vec<(&'static str, BTreeMap<String, String>, bool)> {
    vec![
        ("neither half in the reserved namespace", tags(&[("owner", "finance")]), true),
        ("a key in the reserved namespace", tags(&[("aws:owner", "finance")]), false),
        ("a value in the reserved namespace", tags(&[("owner", "aws:finance")]), false),
        (
            "both halves in the reserved namespace",
            tags(&[("aws:owner", "aws:finance")]),
            false,
        ),
        ("the key's prefix upper-cased", tags(&[("AWS:owner", "finance")]), false),
        ("the key's prefix mixed-cased", tags(&[("Aws:owner", "finance")]), false),
        ("the value's prefix upper-cased", tags(&[("owner", "AWS:finance")]), false),
        ("a key that is exactly the prefix", tags(&[("aws:", "finance")]), false),
        (
            "a reserved key beside allowed ones",
            tags(&[("env", "prod"), ("aws:cloudformation:stack-name", "s"), ("tier", "gold")]),
            false,
        ),
        (
            "a reserved value beside allowed ones",
            tags(&[("env", "prod"), ("owner", "aws:finance")]),
            false,
        ),
        ("the prefix later in the key", tags(&[("my:aws:owner", "finance")]), true),
        ("the prefix later in the value", tags(&[("owner", "my-aws:finance")]), true),
        ("the prefix's letters without the colon", tags(&[("awsome", "awsome")]), true),
        ("the prefix's letters as the whole value", tags(&[("team", "aws")]), true),
        (
            "a colon in a key outside the reserved namespace",
            tags(&[("team:graphs", "owned")]),
            true,
        ),
        ("a key shorter than the prefix", tags(&[("a", "b")]), true),
    ]
}

/// Assert a refusal of a reserved tag: the modelled 400 `ValidationException`, whose
/// message is the whole clause the rule calls for, inside the envelope this service
/// reports a broken constraint in. The expectation is built here from `supplied` and the
/// rule restated above, not read out of the service.
fn assert_reserved_refusal(
    case: &str,
    operation: &str,
    supplied: &BTreeMap<String, String>,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status}: {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException from {operation}, got {body}"
    );
    let requirement = format!("Member must not be prefixed with {RESERVED_PREFIX}");
    // A reserved KEY is refused before a reserved value, and a key's clause sits at the
    // member the map itself occupies, because a map's keys reach no clause of their own;
    // a value's clause sits at its entry's path. `BTreeMap` iterates in key order, which
    // is the order the refusal picks its offending entry in.
    let clause = if let Some(key) = supplied.keys().find(|key| is_reserved(key)) {
        format!(
            "Value '{key}' at 'tags' failed to satisfy constraint: Map keys must satisfy \
             constraint: [{requirement}]"
        )
    } else {
        let (key, value) = supplied
            .iter()
            .find(|(_, value)| is_reserved(value))
            .unwrap_or_else(|| panic!("case {case:?}: no reserved half in {supplied:?}"));
        format!("Value '{value}' at 'tags.{key}' failed to satisfy constraint: {requirement}")
    };
    let expected = format!("1 validation error detected: {clause}");
    let actual = message(body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the {operation} refusal to read {expected:?}, \
         got {actual:?}"
    );
}

#[tokio::test]
async fn tag_resource_refuses_a_tag_in_the_reserved_namespace_and_writes_nothing() {
    let router = emulator();

    for (index, (case, supplied, storable)) in reserved_namespace_cases().iter().enumerate() {
        // The graph starts with one tag of its own, so a refused call is seen to leave
        // the stored set exactly as it was rather than merely leaving it empty.
        let stored = tags(&[("created-with", "a tag")]);
        let arn = create_graph(&router, &format!("reserved-tag-graph-{index}"), &stored).await;

        let (status, body) = tag(&router, &arn, supplied).await;
        let mut expected = stored.clone();
        if *storable {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from TagResource, got {status}: {body}"
            );
            expected.extend(supplied.clone());
        } else {
            assert_reserved_refusal(case, "TagResource", supplied, status, &body);
        }
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} after TagResource, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn create_graph_refuses_a_tag_in_the_reserved_namespace_and_creates_nothing() {
    let router = emulator();

    for (index, (case, supplied, storable)) in reserved_namespace_cases().iter().enumerate() {
        let name = format!("reserved-tag-create-{index}");
        let (status, body) = create_graph_answer(&router, &name, supplied).await;
        if *storable {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateGraph, got {status}: {body}"
            );
            let arn = body["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no arn in {body}"))
                .to_string();
            let actual = stored_tags(&router, &arn).await;
            assert_eq!(
                actual, *supplied,
                "case {case:?}: expected {supplied:?} on the new graph, got {actual:?}"
            );
            continue;
        }
        assert_reserved_refusal(case, "CreateGraph", supplied, status, &body);

        // The refused create stored nothing: the name it asked for is still free, which
        // a second create of the same name — with tags this service may store — proves
        // by being accepted rather than conflicting.
        let kept = tags(&[("env", "prod")]);
        let arn = create_graph(&router, &name, &kept).await;
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, kept,
            "case {case:?}: expected {kept:?} on the graph created after the refusal, \
             got {actual:?}"
        );
    }
}
