// SPDX-License-Identifier: CC0-1.0

//! How this service words a boundary refusal of a COLLECTION whose `@length` a request
//! broke, asserted end-to-end over the wire.
//!
//! THE RULE UNDER TEST: a list or a map outside its `@length` bound is refused with NO
//! value in the clause —
//! `N validation error(s) detected: Value at '<member path>' failed to satisfy constraint:
//! <requirement>` — while a SCALAR member outside its own bound states the value it
//! submitted (`Value '<value>' at '<member path>' …`). A collection has no scalar
//! rendering, so none is invented for it; the member path, the requirement and the
//! counting envelope are the framework's either way.
//!
//! Each row drives a real `restJson1` request through the `MultiServiceRouter` the
//! production emulator serves with (the harness in [`crate::sessions`]), so what is pinned
//! is the status, the error code and the `message` a Bedrock Agent Runtime client reads.
//! The rows span what the rule generalises over: both bounds (a minimum and a maximum),
//! both collection kinds (list and map), where the collection is bound (a request body
//! and an `@httpQuery`), which operation carries it, and — as the contrast the rule turns
//! on — the scalar classes that keep their value.

use serde_json::{json, Value};

use crate::sessions::{call, error_code, router, ACCOUNT, REGION};

/// The status and code the model binds to a constraint refusal of every operation here
/// (`smithy_query(query="operation",operation="Rerank,TagResource,UntagResource")` ->
/// `ValidationException` httpError 400).
const INVALID: (u16, &str) = (400, "ValidationException");

/// A reranker model id the modelled `BedrockModelArn` pattern admits, so no row is refused
/// for the configuration it carries.
const MODEL: &str = "amazon.rerank-v1:0";

/// The `@length` bounds of the collections under test, from the model
/// (`smithy_query(query="shape",shape="RerankQueriesList,RerankSourcesList,TagsMap")` and
/// `smithy_query(query="operation",operation="UntagResource")` -> `tagKeys`):
/// `queries` 1..=1, `sources` 1..=1000, `tags` 1..=200 entries, `tagKeys` 1..=200.
const QUERIES_MAX: usize = 1;
const SOURCES_MAX: usize = 1000;
const TAGS_MIN: usize = 1;
const TAG_KEYS_MAX: usize = 200;

/// The `message` member of an error body, or `""` when there is none.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// The single-violation envelope the framework counts with.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// THE RULE: the clause a COLLECTION's `@length` violation states — the member and the
/// requirement, and no value.
fn collection_clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The contrast: the clause a SCALAR member's violation states, quoting what it submitted.
fn scalar_clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The framework's wording of a `@length` maximum.
fn at_most(bound: usize) -> String {
    format!("Member must have length less than or equal to {bound}")
}

/// The framework's wording of a `@length` minimum.
fn at_least(bound: usize) -> String {
    format!("Member must have length greater than or equal to {bound}")
}

/// One `TEXT` query carrying `text`.
fn text_query(text: &str) -> Value {
    json!({ "type": "TEXT", "textQuery": { "text": text } })
}

/// One INLINE `TEXT` source carrying `text`.
fn text_source(text: &str) -> Value {
    json!({
        "type": "INLINE",
        "inlineDocumentSource": { "type": "TEXT", "textDocument": { "text": text } },
    })
}

/// A `Rerank` body over `queries` and `sources`, with the reranking configuration every
/// row needs to get as far as the collection it is about.
fn rerank_body(queries: Value, sources: Value) -> Value {
    json!({
        "queries": queries,
        "sources": sources,
        "rerankingConfiguration": {
            "type": "BEDROCK_RERANKING_MODEL",
            "bedrockRerankingConfiguration": { "modelConfiguration": { "modelArn": MODEL } },
        },
    })
}

/// `count` distinct `TEXT` queries.
fn queries(count: usize) -> Value {
    Value::Array((0..count).map(|i| text_query(&format!("query {i}"))).collect())
}

/// `count` distinct INLINE `TEXT` sources.
fn sources(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|i| text_source(&format!("document {i}")))
            .collect(),
    )
}

/// A well-formed session ARN for `uuid`, which this service's `TaggableResourcesArn`
/// pattern admits: the tagging rows are refused by the boundary walk, so the session it
/// names need not exist.
fn session_arn(uuid: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/{uuid}")
}

/// Percent-encode an ARN for the non-greedy `@httpLabel` the tagging routes bind: a
/// session ARN carries `:` and `/`, which would otherwise split the path segment.
fn label(arn: &str) -> String {
    arn.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// THE RULE over a TABLE: every collection outside its `@length` is refused with a
/// valueless clause, and every scalar outside its own bound keeps the value it sent.
#[tokio::test]
async fn a_collection_outside_its_length_is_refused_without_a_value() {
    let arn = session_arn("3f6c0a52-7b1d-4a9e-8c21-55d0f2a7b914");
    let tag_keys: Vec<String> = (0..=TAG_KEYS_MAX).map(|i| format!("key{i}")).collect();

    // (case, method, path, body, the clause the refusal must state)
    let rows: Vec<(&str, &str, String, Value, String)> = vec![
        (
            "queries one entry over its maximum",
            "POST",
            "/rerank".to_string(),
            rerank_body(queries(QUERIES_MAX + 1), sources(1)),
            collection_clause("queries", &at_most(QUERIES_MAX)),
        ),
        (
            "an empty queries list, under its minimum",
            "POST",
            "/rerank".to_string(),
            rerank_body(json!([]), sources(1)),
            collection_clause("queries", &at_least(1)),
        ),
        (
            "an empty sources list, under its minimum",
            "POST",
            "/rerank".to_string(),
            rerank_body(queries(1), json!([])),
            collection_clause("sources", &at_least(1)),
        ),
        (
            "sources one entry over a much larger maximum",
            "POST",
            "/rerank".to_string(),
            rerank_body(queries(1), sources(SOURCES_MAX + 1)),
            collection_clause("sources", &at_most(SOURCES_MAX)),
        ),
        (
            "an empty tags map, under its minimum",
            "POST",
            format!("/tags/{}", label(&arn)),
            json!({ "tags": {} }),
            collection_clause("tags", &at_least(TAGS_MIN)),
        ),
        (
            "a query-bound tagKeys list one entry over its maximum",
            "DELETE",
            format!("/tags/{}?tagKeys={}", label(&arn), tag_keys.join("&tagKeys=")),
            Value::Null,
            collection_clause("tagKeys", &at_most(TAG_KEYS_MAX)),
        ),
        (
            "a scalar below its @range minimum keeps its value",
            "POST",
            "/rerank".to_string(),
            json!({
                "queries": queries(1),
                "sources": sources(1),
                "rerankingConfiguration": {
                    "type": "BEDROCK_RERANKING_MODEL",
                    "bedrockRerankingConfiguration": {
                        "modelConfiguration": { "modelArn": MODEL },
                        "numberOfResults": 0,
                    },
                },
            }),
            scalar_clause(
                "0",
                "rerankingConfiguration.bedrockRerankingConfiguration.numberOfResults",
                "Member must have value greater than or equal to 1",
            ),
        ),
        (
            "a scalar string under its @length minimum keeps its value",
            "POST",
            "/rerank".to_string(),
            json!({
                "queries": queries(1),
                "sources": sources(1),
                "rerankingConfiguration": {
                    "type": "BEDROCK_RERANKING_MODEL",
                    "bedrockRerankingConfiguration": { "modelConfiguration": { "modelArn": MODEL } },
                },
                "nextToken": "",
            }),
            scalar_clause("", "nextToken", &at_least(1)),
        ),
    ];

    let router = router();
    for (case, method, path, body, clause) in &rows {
        let (status, refused) = call(&router, method, path, body.clone()).await;
        assert_eq!(
            (status, error_code(&refused)),
            INVALID,
            "case {case:?}: expected {INVALID:?}, got {status} {refused:?}"
        );
        let expected = one_error(clause);
        let stated = message(&refused);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected message {expected:?}, got {stated:?}"
        );
    }
}

/// THE RULE for a request breaking a COLLECTION's bound AND a scalar's at once: the
/// envelope counts both clauses, the collection's carries no value and the scalar's
/// carries its own.
#[tokio::test]
async fn an_envelope_over_two_violations_drops_only_the_collections_value() {
    let router = router();
    let (status, refused) = call(
        &router,
        "POST",
        "/rerank",
        json!({
            "queries": queries(QUERIES_MAX + 1),
            "sources": sources(1),
            "rerankingConfiguration": {
                "type": "BEDROCK_RERANKING_MODEL",
                "bedrockRerankingConfiguration": {
                    "modelConfiguration": { "modelArn": MODEL },
                    "numberOfResults": 0,
                },
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        INVALID,
        "two violations: expected {INVALID:?}, got {status} {refused:?}"
    );
    let stated = message(&refused);
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "two violations: expected the two-error envelope, got {stated:?}"
    );
    let list_clause = collection_clause("queries", &at_most(QUERIES_MAX));
    assert!(
        stated.contains(&list_clause),
        "two violations: expected the clause {list_clause:?}, got {stated:?}"
    );
    let range_clause = scalar_clause(
        "0",
        "rerankingConfiguration.bedrockRerankingConfiguration.numberOfResults",
        "Member must have value greater than or equal to 1",
    );
    assert!(
        stated.contains(&range_clause),
        "two violations: expected the clause {range_clause:?}, got {stated:?}"
    );
    assert!(
        !stated.contains("items"),
        "two violations: a collection's clause must state no value, got {stated:?}"
    );
}
