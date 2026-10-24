// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, end to end over HTTP.
//!
//! Three rules are asserted here, each over a TABLE or a class of inputs rather than
//! over the single request that exposed the defect:
//!
//! 1. listing a resource's tags reports the tag map that resource HOLDS — every pair
//!    persisted for it and nothing else, an untagged resource as an empty map — and
//!    reports it without changing it, so a second listing (and the resource's own
//!    `tags` member read through `GetGraphqlApi`) answers the same map. The table
//!    spans what the rule generalises over: the tag COUNT from one to the modelled
//!    maximum of 50, key length on both modelled bounds, value length on both
//!    modelled bounds (the empty value included), the character classes `TagValue`
//!    admits, and a resource created with no `tags` member at all;
//! 2. tags belong to the resource they were set on: each of several resources in one
//!    store lists its own map, so no listing leaks another resource's tags;
//! 3. a well-formed resource ARN this account holds no resource for is a 404
//!    `NotFoundException` — asserted over a never-created ARN, a deleted resource's
//!    ARN, and the ARN of a resource that exists under a different Region or account.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, the `@httpLabel` ARN binding, restJson1 (de)serialization, the
//! handler and the shared engine together. The suite reads the error-type header,
//! which the lifecycle suite's helpers do not carry, so it keeps its own `call` for
//! the tag route and reuses that suite's router and create helper.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::graphql_api_lifecycle::{call, create_api, router, REGION};

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The modelled maximum number of entries a `TagMap` holds. Source:
/// `smithy_query(query="shape", shape="TagMap")` — `@length` min 1, max 50.
const TAG_COUNT_MAX: usize = 50;

/// `TagKey`'s modelled maximum length. Source: `smithy_query(query="shape",
/// shape="TagKey")` — `@length` min 1, max 128.
const TAG_KEY_MAX_LEN: usize = 128;

/// `TagValue`'s modelled maximum length. Source: `smithy_query(query="shape",
/// shape="TagValue")` — `@length` min 0, max 256, so the empty string is a value.
const TAG_VALUE_MAX_LEN: usize = 256;

/// An `apiId`-shaped id (`[0-9A-Za-z_-]{26}`) that no create ever mints here, used to
/// build the ARN of a resource that does not exist.
const ABSENT_API_ID: &str = "abcdefghijklmnopqrstuvwxyz";

/// One answer from the emulator: the parts a client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    error_type: String,
    body: Value,
}

/// `GET /v1/tags/{resourceArn}` for `arn`, as a client issues it.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://appsync.{REGION}.amazonaws.com/v1/tags/{}",
            encode_label(arn)
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::new())
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// `value` percent-encoded the way an AWS SDK encodes a non-greedy `@httpLabel`:
/// every character outside the unreserved set (`A-Za-z0-9-._~`) becomes `%XX`, so an
/// ARN's `:` and `/` stay inside the one path segment the label binds.
fn encode_label(value: &str) -> String {
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

/// One row of the tag table: a label, and the tag pairs the resource is created with
/// (`None` for a resource created with no `tags` member at all).
#[derive(Debug)]
struct Case {
    name: String,
    tags: Option<Vec<(String, String)>>,
}

impl Case {
    fn new(name: &str, tags: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            tags: Some(
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            ),
        }
    }

    /// The tag map this row expects a listing to report: the pairs it was created
    /// with, or an empty map when it carried no `tags` member.
    fn expected(&self) -> Value {
        let mut map = Map::new();
        for (key, value) in self.tags.iter().flatten() {
            map.insert(key.clone(), Value::String(value.clone()));
        }
        Value::Object(map)
    }

    /// The `CreateGraphqlApi` body this row creates its resource with.
    fn create_body(&self) -> Value {
        let mut body = json!({ "name": "tagged", "authenticationType": "API_KEY" });
        if self.tags.is_some() {
            body["tags"] = self.expected();
        }
        body
    }
}

/// The table: every class the listing rule generalises over.
fn cases() -> Vec<Case> {
    let long_key = "k".repeat(TAG_KEY_MAX_LEN);
    let long_value = "v".repeat(TAG_VALUE_MAX_LEN);
    let mut cases = vec![
        Case::new("one pair", &[("env", "prod")]),
        Case::new("two pairs", &[("env", "prod"), ("team", "search")]),
        // `TagKey`'s modelled length bounds.
        Case::new("key at the minimum length of 1", &[("k", "v")]),
        Case::new(
            "key at the maximum length of 128",
            &[(long_key.as_str(), "v")],
        ),
        // `TagValue`'s modelled length bounds — the minimum IS the empty string.
        Case::new("value at the minimum length of 0", &[("empty", "")]),
        Case::new(
            "value at the maximum length of 256",
            &[("long", long_value.as_str())],
        ),
        // The character classes `TagValue`'s pattern admits (`^[\s\w+-=\.:/@]*$`).
        Case::new(
            "value spanning the admitted character classes",
            &[("punctuation", "a b+c-d=e.f:g/h@i_1")],
        ),
        Case {
            name: "no tags member at all".to_string(),
            tags: None,
        },
    ];
    // The modelled maximum tag count.
    let full: Vec<(String, String)> = (0..TAG_COUNT_MAX)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    cases.push(Case {
        name: format!("the maximum of {TAG_COUNT_MAX} pairs"),
        tags: Some(full),
    });
    cases
}

#[tokio::test]
async fn a_listing_reports_the_tag_map_the_resource_holds() {
    for case in cases() {
        let name = case.name.clone();
        let router = router();
        let created = create_api(&router, case.create_body()).await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: the created API carries an ARN: {created:?}"))
            .to_string();

        let answer = list_tags(&router, &arn).await;
        assert_eq!(
            answer.status, 200,
            "case {name} ({case:?}): expected ListTagsForResource 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["tags"],
            case.expected(),
            "case {name}: expected the stored tag map, got {:?}",
            answer.body
        );

        // A read may not change what a later caller sees: the same listing, and the
        // resource's own `tags` member, still report the same map.
        let again = list_tags(&router, &arn).await;
        assert_eq!(
            again.body["tags"],
            case.expected(),
            "case {name}: expected a second listing to report the same map, got {:?}",
            again.body
        );
        let api_id = created["apiId"].as_str().unwrap_or_default();
        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {name}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["graphqlApi"]["tags"],
            case.expected(),
            "case {name}: expected the API to still carry the same tags, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn each_resource_lists_its_own_tags() {
    let router = router();
    let rows = [
        ("first", json!({ "env": "prod", "team": "search" })),
        ("second", json!({ "env": "staging" })),
        ("third", json!({ "owner": "platform", "cost.center": "42" })),
    ];

    let mut arns = Vec::new();
    for (name, tags) in &rows {
        let created = create_api(
            &router,
            json!({ "name": name, "authenticationType": "AWS_IAM", "tags": tags }),
        )
        .await;
        arns.push(
            created["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name}: the created API carries an ARN"))
                .to_string(),
        );
    }

    for ((name, tags), arn) in rows.iter().zip(&arns) {
        let answer = list_tags(&router, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected ListTagsForResource 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            &answer.body["tags"], tags,
            "case {name}: expected only this resource's tags, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_is_not_found() {
    let router = router();
    let created = create_api(
        &router,
        json!({ "name": "tagged", "authenticationType": "API_KEY", "tags": { "env": "prod" } }),
    )
    .await;
    let arn = created["arn"].as_str().expect("the created API carries an ARN");
    let api_id = created["apiId"].as_str().expect("the created API carries an id");

    // An API created and then deleted: its ARN was valid and now names nothing.
    let deleted = create_api(
        &router,
        json!({ "name": "gone", "authenticationType": "API_KEY", "tags": { "env": "dev" } }),
    )
    .await;
    let deleted_arn = deleted["arn"].as_str().expect("the created API carries an ARN").to_string();
    let deleted_id = deleted["apiId"].as_str().expect("the created API carries an id");
    let (status, body) = call(&router, "DELETE", &format!("/v1/apis/{deleted_id}"), None).await;
    assert_eq!(status, 200, "deleting the API answers 200, got {status} {body:?}");

    let cases = [
        (
            "an id no create ever minted",
            format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
        ),
        ("a deleted API's ARN", deleted_arn),
        (
            "the API's id under another Region",
            format!("arn:aws:appsync:us-east-1:{ACCOUNT}:apis/{api_id}"),
        ),
        (
            "the API's id under another account",
            format!("arn:aws:appsync:{REGION}:123456789012:apis/{api_id}"),
        ),
    ];

    for (name, candidate) in &cases {
        let answer = list_tags(&router, candidate).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "NotFoundException"),
            "case {name} ({candidate}): expected 404 NotFoundException, got {} {} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }

    // The resource that DOES exist is still listed, so the refusals above are about
    // the ARN that named nothing rather than about the route.
    let answer = list_tags(&router, arn).await;
    assert_eq!(
        answer.status, 200,
        "case the stored API: expected ListTagsForResource 200, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["tags"],
        json!({ "env": "prod" }),
        "case the stored API: expected its stored tags, got {:?}",
        answer.body
    );
}
