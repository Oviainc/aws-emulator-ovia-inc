// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` addressing: how the `ResourceArn` label reaches the handler,
//! and what an untagged rule reports.
//!
//! `ResourceArn` is bound to a NON-greedy `@httpLabel` (`GET /tags/{ResourceArn}`), so
//! an ARN travels as ONE path segment. Of its characters only `/` MUST be escaped for
//! that to hold; `:` is legal unencoded in a path segment (RFC 3986 `pchar`), and a
//! percent escape is case-insensitive. The rule under test: the ARN the service minted
//! resolves to its rule whichever of those forms a client sends, and a rule carrying no
//! tags reports `Tags` as a PRESENT empty array rather than omitting the member — a
//! distinction a map- or list-defaulting assertion cannot see.
//!
//! `tests/all/resource_tags.rs` covers the tag sets, the upsert/removal rules and the
//! quota; this suite covers only the addressing and the empty-list shape, over the
//! encodings an SDK or a hand-rolled client may produce.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_rbin_amazonrecyclebin::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the handler reads it from the SigV4 credential scope.
const REGION: &str = "eu-west-2";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDRBINTEST/20260101/{REGION}/rbin/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://rbin.{REGION}.amazonaws.com{path}"))
        .header("authorization", authorization());
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path} answered {status} with a non-JSON body ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Create a rule with the given tag list and return its `RuleArn`.
async fn rule_with_tags(router: &MultiServiceRouter, case: &str, tags: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/rules",
        Some(json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": { "RetentionPeriodValue": 7, "RetentionPeriodUnit": "DAYS" },
            "Description": case,
            "Tags": tags,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: fixture CreateRule returned {status} / {created:?}"
    );
    created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no RuleArn"))
        .to_string()
}

/// The `Tags` list of a response, as `(Key, Value)` pairs.
fn tag_pairs(body: &Value) -> Vec<(String, String)> {
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("expected Tags to be an array, got {:?}", body["Tags"]))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every escaping of the one path segment an ARN occupies that a client may send.
/// Each row differs in WHICH characters are escaped and in the case of the escapes.
fn encodings(rule_arn: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "`/` and `:` percent-encoded, as an SDK sends a non-greedy label",
            rule_arn.replace(':', "%3A").replace('/', "%2F"),
        ),
        (
            "only `/` percent-encoded (`:` is a legal path character)",
            rule_arn.replace('/', "%2F"),
        ),
        (
            "lowercase percent escapes",
            rule_arn.replace(':', "%3a").replace('/', "%2f"),
        ),
        (
            "mixed: `:` escaped, `/` escaped in lowercase",
            rule_arn.replace(':', "%3A").replace('/', "%2f"),
        ),
    ]
}

#[tokio::test]
async fn the_resource_arn_label_resolves_however_the_client_escaped_it() {
    let router = router();
    let rule_arn = rule_with_tags(
        &router,
        "addressed through every escaping",
        json!([{ "Key": "env", "Value": "prod" }, { "Key": "team", "Value": "storage" }]),
    )
    .await;
    let want = vec![
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "storage".to_string()),
    ];

    for (case, encoded) in encodings(&rule_arn) {
        let (status, listed) = call(&router, "GET", &format!("/tags/{encoded}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for /tags/{encoded}, got {status} / {listed:?}"
        );
        let mut got = tag_pairs(&listed);
        got.sort();
        assert_eq!(
            got, want,
            "case {case:?}: expected the stored tags, got {:?}",
            listed["Tags"]
        );
    }
}

#[tokio::test]
async fn an_untagged_rule_reports_tags_as_a_present_empty_array() {
    let router = router();
    // Both ways a rule can end up holding no tags: created with an empty list, and
    // created with tags that are then all unassigned.
    let empty_at_creation = rule_with_tags(&router, "created with no tags", json!([])).await;
    let emptied = rule_with_tags(
        &router,
        "created with a tag that is then unassigned",
        json!([{ "Key": "env", "Value": "dev" }]),
    )
    .await;
    let (status, body) = call(
        &router,
        "DELETE",
        &format!(
            "/tags/{}?tagKeys=env",
            emptied.replace(':', "%3A").replace('/', "%2F")
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "fixture UntagResource returned {status} / {body:?}"
    );

    for (case, rule_arn) in [
        ("created with no tags", empty_at_creation),
        ("emptied by UntagResource", emptied),
    ] {
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/tags/{}", rule_arn.replace(':', "%3A").replace('/', "%2F")),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} / {listed:?}"
        );
        // `Tags` is a modelled list, so it is PRESENT as an empty array — not omitted.
        assert!(
            listed["Tags"].is_array(),
            "case {case:?}: expected Tags to be reported as an array, got {:?}",
            listed["Tags"]
        );
        assert!(
            tag_pairs(&listed).is_empty(),
            "case {case:?}: expected no tags, got {:?}",
            listed["Tags"]
        );
    }
}
