// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of Security Hub CSPM resource tagging: `TagResource` →
//! `ListTagsForResource` → `UntagResource` against the `hub/default` resource
//! `EnableSecurityHub` creates.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter` — the
//! `@httpLabel`-bound `ResourceArn`, the `@httpQuery`-bound `TagKeys` and the JSON
//! body — so a route, a status code or a serialized member that is wrong on the wire
//! fails here.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test signs for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// One emulator with the Security Hub service mounted, driven over HTTP.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request and return `(status, parsed body)`. An empty body parses
    /// as `Value::Null`, which is what an operation with no output members returns.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call_in(REGION, method, path, body).await
    }

    /// `call`, signed for `region`: the handler attributes the request to the Region
    /// in the SigV4 credential scope, exactly as a signed SDK request carries it.
    async fn call_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Subscribe the account/Region, so there is a resource to tag.
    async fn enable(&self, body: Option<Value>) -> (u16, Value) {
        self.call("POST", "/accounts", body.or(Some(json!({})))).await
    }

    /// Subscribe in `region`, so one emulator holds a hub in more than one Region.
    async fn enable_in(&self, region: &str) -> (u16, Value) {
        self.call_in(region, "POST", "/accounts", Some(json!({})))
            .await
    }

    /// The ARN `DescribeHub` reports for the subscription in `region`.
    async fn hub_arn_in(&self, region: &str) -> String {
        let (status, body) = self.call_in(region, "GET", "/accounts", None).await;
        assert_eq!(
            status, 200,
            "DescribeHub in {region} expected 200, got {status}"
        );
        body["HubArn"]
            .as_str()
            .unwrap_or_else(|| panic!("DescribeHub in {region} must report a HubArn, got {body}"))
            .to_string()
    }

    /// The ARN `DescribeHub` reports for the subscription — the ARN every tagging
    /// call below addresses, read back from the service rather than hardcoded.
    async fn hub_arn(&self) -> String {
        self.hub_arn_in(REGION).await
    }

    async fn tag(&self, resource_arn: &str, tags: Value) -> (u16, Value) {
        self.tag_in(REGION, resource_arn, tags).await
    }

    async fn tag_in(&self, region: &str, resource_arn: &str, tags: Value) -> (u16, Value) {
        self.call_in(
            region,
            "POST",
            &format!("/tags/{}", path_escape(resource_arn)),
            Some(json!({ "Tags": tags })),
        )
        .await
    }

    async fn list_tags(&self, resource_arn: &str) -> (u16, Value) {
        self.list_tags_in(REGION, resource_arn).await
    }

    async fn list_tags_in(&self, region: &str, resource_arn: &str) -> (u16, Value) {
        self.call_in(
            region,
            "GET",
            &format!("/tags/{}", path_escape(resource_arn)),
            None,
        )
        .await
    }

    async fn untag(&self, resource_arn: &str, tag_keys: &[String]) -> (u16, Value) {
        self.untag_in(REGION, resource_arn, tag_keys).await
    }

    async fn untag_in(
        &self,
        region: &str,
        resource_arn: &str,
        tag_keys: &[String],
    ) -> (u16, Value) {
        let query = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", query_escape(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.call_in(
            region,
            "DELETE",
            &format!("/tags/{}?{query}", path_escape(resource_arn)),
            None,
        )
        .await
    }
}

/// Escape an ARN for the single, non-greedy `{ResourceArn}` path segment: only `/`
/// (and a literal `%`) would otherwise end the segment, and the server
/// percent-decodes the label back to the ARN. `:` is legal in a path segment.
fn path_escape(arn: &str) -> String {
    arn.replace('%', "%25").replace('/', "%2F")
}

/// Escape a tag key for the `@httpQuery`-bound `tagKeys` parameter, the way an SDK
/// does: `+` must not reach the server raw, because query decoding reads it as a
/// space, and `&`/`=` would split the parameter list.
fn query_escape(key: &str) -> String {
    key.replace('%', "%25")
        .replace('+', "%2B")
        .replace('&', "%26")
        .replace('=', "%3D")
}

/// A `TagMap` JSON object from a list of pairs, so a case's expected tags and the
/// request that sets them come from one value.
fn tag_object<'a>(tags: impl IntoIterator<Item = &'a (String, String)>) -> Value {
    let map: Map<String, Value> = tags
        .into_iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    Value::Object(map)
}

/// `count` distinct tags, for the cases that vary only in how many tags there are.
fn numbered_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|i| (format!("key-{i:02}"), format!("value-{i}")))
        .collect()
}

/// A subscribed hub and its ARN.
async fn subscribed() -> (Harness, String) {
    let harness = Harness::new();
    let (status, body) = harness.enable(None).await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub expected 200, got {status} with {body}"
    );
    let arn = harness.hub_arn().await;
    (harness, arn)
}

/// RULE: `TagResource` merges the submitted tag set into the addressed resource's
/// tags, `ListTagsForResource` reports exactly the set the resource currently
/// carries, and `UntagResource` removes exactly the named keys — for any tag set the
/// model admits, whatever its count, its key form or its value length.
///
/// The rows differ along what the rule generalises over: the number of tags, the
/// length of a key or a value (the published `TagKey` 1..128, `TagValue` 0..256 and
/// `TagMap` 1..50 bounds from `smithy_query(query="shape", shape="TagMap")`), and
/// which of the characters `TagKey`'s pattern admits a key is built from.
#[tokio::test]
async fn tag_list_untag_round_trips_for_every_shape_of_tag_set() {
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("one tag", vec![("Env".to_string(), "prod".to_string())]),
        (
            "keys built from the punctuation TagKey's pattern admits",
            vec![
                ("cost-center".to_string(), "1234".to_string()),
                ("team:owner".to_string(), "security".to_string()),
                ("path/to/thing".to_string(), "nested".to_string()),
                ("a+b=c.d_e".to_string(), "punctuated".to_string()),
            ],
        ),
        (
            "a value at the published minimum length of 0",
            vec![("Blank".to_string(), String::new())],
        ),
        (
            "a value at the published maximum length of 256",
            vec![("Long".to_string(), "v".repeat(256))],
        ),
        (
            "a key at the published maximum length of 128",
            vec![("k".repeat(128), "value".to_string())],
        ),
        ("ten tags", numbered_tags(10)),
        (
            "fifty tags, the published TagMap maximum",
            numbered_tags(50),
        ),
    ];

    for (case, tags) in cases {
        let (harness, arn) = subscribed().await;

        // A resource nobody has tagged carries no tags.
        let (status, body) = harness.list_tags(&arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Tags"],
            json!({}),
            "case {case:?}: an untagged resource must report an empty tag set, got {body}"
        );

        let (status, body) = harness.tag(&arn, tag_object(&tags)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource expected 200, got {status} with {body}"
        );

        let (status, after_tag) = harness.list_tags(&arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} with {after_tag}"
        );
        assert_eq!(
            after_tag["Tags"],
            tag_object(&tags),
            "case {case:?}: the reported tags must be the submitted set, got {after_tag}"
        );

        // Reporting the tags does not change what the next reader sees.
        let (_, again) = harness.list_tags(&arn).await;
        assert_eq!(
            again, after_tag,
            "case {case:?}: a second ListTagsForResource must report the same tags, got {again}"
        );

        // Removing a subset leaves exactly the rest.
        let keys: Vec<String> = tags.iter().map(|(key, _)| key.clone()).collect();
        let removed: Vec<String> = keys[..keys.len().div_ceil(2)].to_vec();
        let (status, body) = harness.untag(&arn, &removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource expected 200, got {status} with {body}"
        );
        let expected = tag_object(tags.iter().filter(|(key, _)| !removed.contains(key)));
        let (_, body) = harness.list_tags(&arn).await;
        assert_eq!(
            body["Tags"], expected,
            "case {case:?}: removing {removed:?} must leave the other tags, got {body}"
        );

        // Removing the remaining keys, together with a key the resource never
        // carried, empties the set: removal is idempotent.
        let mut rest: Vec<String> = keys
            .iter()
            .filter(|key| !removed.contains(key))
            .cloned()
            .collect();
        rest.push("never-set".to_string());
        let (status, body) = harness.untag(&arn, &rest).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource of {rest:?} expected 200, got {status} with {body}"
        );
        let (_, body) = harness.list_tags(&arn).await;
        assert_eq!(
            body["Tags"],
            json!({}),
            "case {case:?}: removing every key must leave an empty tag set, got {body}"
        );
    }
}

/// RULE: tagging is a MERGE — a key the resource does not carry is added, a key it
/// already carries takes the submitted value, and no other key is touched; the tags
/// a resource was created with take part in the same merge.
#[tokio::test]
async fn tagging_merges_with_the_tags_the_resource_already_carries() {
    let harness = Harness::new();
    // `EnableSecurityHub` carries its own `Tags`, so the resource exists already
    // tagged — the first row proves those are the tags tagging merges into.
    let (status, body) = harness
        .enable(Some(json!({ "Tags": { "Origin": "enable", "Keep": "untouched" } })))
        .await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub expected 200, got {status} with {body}"
    );
    let arn = harness.hub_arn().await;

    // Each row is one TagResource call and the whole tag set it must leave behind.
    let rows: [(&str, Value, Value); 4] = [
        (
            "the tags the resource was created with",
            json!(null),
            json!({ "Origin": "enable", "Keep": "untouched" }),
        ),
        (
            "adding keys the resource does not carry",
            json!({ "Env": "prod", "Tier": "1" }),
            json!({ "Origin": "enable", "Keep": "untouched", "Env": "prod", "Tier": "1" }),
        ),
        (
            "overwriting one existing key",
            json!({ "Env": "staging" }),
            json!({ "Origin": "enable", "Keep": "untouched", "Env": "staging", "Tier": "1" }),
        ),
        (
            "overwriting a created-with key and adding another at once",
            json!({ "Origin": "tag-resource", "Extra": "" }),
            json!({
                "Origin": "tag-resource",
                "Keep": "untouched",
                "Env": "staging",
                "Tier": "1",
                "Extra": "",
            }),
        ),
    ];

    for (case, tags, expected) in rows {
        if !tags.is_null() {
            let (status, body) = harness.tag(&arn, tags.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: TagResource expected 200, got {status} with {body}"
            );
        }
        let (status, body) = harness.list_tags(&arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Tags"], expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// RULE: a resource's tag set is a `TagMap`, which the model bounds at 50 entries, so
/// a `TagResource` call whose new keys would take the resource past that is refused
/// with `InvalidInputException` (400) and writes nothing — while a call that only
/// overwrites keys the resource already carries stays within the bound and succeeds.
#[tokio::test]
async fn tagging_past_the_published_fifty_tag_maximum_is_refused() {
    let (harness, arn) = subscribed().await;
    let fifty = numbered_tags(50);
    let (status, body) = harness.tag(&arn, tag_object(&fifty)).await;
    assert_eq!(
        status, 200,
        "fifty tags, the published maximum, expected 200, got {status} with {body}"
    );

    // One past the bound: a single new key on a resource that already carries 50.
    let (status, body) = harness
        .tag(&arn, json!({ "one-too-many": "51" }))
        .await;
    assert_eq!(
        status, 400,
        "a 51st tag: expected 400, got {status} with {body}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some("InvalidInputException"),
        "a 51st tag must be refused as InvalidInputException, got {body}"
    );
    let (_, body) = harness.list_tags(&arn).await;
    assert_eq!(
        body["Tags"],
        tag_object(&fifty),
        "a refused TagResource must write nothing, got {body}"
    );

    // At the bound, overwriting an existing key adds no entry and is accepted.
    let (status, body) = harness
        .tag(&arn, json!({ "key-00": "replaced" }))
        .await;
    assert_eq!(
        status, 200,
        "overwriting a key at the bound: expected 200, got {status} with {body}"
    );
    let (_, body) = harness.list_tags(&arn).await;
    assert_eq!(
        body["Tags"]["key-00"].as_str(),
        Some("replaced"),
        "the overwritten value must be reported, got {body}"
    );
    assert_eq!(
        body["Tags"].as_object().map(Map::len),
        Some(50),
        "overwriting a key must not change how many tags the resource carries, got {body}"
    );

    // Freeing a key makes room again.
    let (status, body) = harness.untag(&arn, &["key-00".to_string()]).await;
    assert_eq!(
        status, 200,
        "UntagResource expected 200, got {status} with {body}"
    );
    let (status, body) = harness
        .tag(&arn, json!({ "one-too-many": "51" }))
        .await;
    assert_eq!(
        status, 200,
        "a new key once one was removed: expected 200, got {status} with {body}"
    );
}

/// RULE: a tagging call addresses a resource by ARN — an ARN that is well formed but
/// names no resource the caller's account and Region has is
/// `ResourceNotFoundException` (404), and an ARN that is not a well-formed ARN at all
/// is `InvalidInputException` (400). Both hold for every one of the three tagging
/// operations, and a refused call changes nothing.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused() {
    let (harness, arn) = subscribed().await;
    let (status, body) = harness.tag(&arn, json!({ "Env": "prod" })).await;
    assert_eq!(
        status, 200,
        "TagResource on the real resource expected 200, got {status} with {body}"
    );

    // Well formed, but no resource carries it: another Region, another hub name,
    // and resource types this service models but has not created.
    let missing = [
        "arn:aws:securityhub:eu-west-1:000000000000:hub/default",
        "arn:aws:securityhub:us-east-1:000000000000:hub/other",
        "arn:aws:securityhub:us-east-1:999999999999:hub/default",
        "arn:aws:securityhub:us-east-1:000000000000:insight/000000000000/custom/0fbd6c9a",
        "arn:aws:securityhub:us-east-1:000000000000:action/custom/escalate",
    ];
    // Prefixed with `arn:aws:securityhub:`, as the modelled `ResourceArn` pattern
    // demands, but not the six fields of an ARN.
    let malformed = [
        "arn:aws:securityhub:",
        "arn:aws:securityhub:us-east-1",
        "arn:aws:securityhub:us-east-1:000000000000",
        "arn:aws:securityhub:us-east-1:000000000000:",
    ];

    for (case, expected_status, expected_type) in missing
        .iter()
        .map(|arn| (arn, 404, "ResourceNotFoundException"))
        .chain(
            malformed
                .iter()
                .map(|arn| (arn, 400, "InvalidInputException")),
        )
    {
        let probes: [(&str, (u16, Value)); 3] = [
            ("TagResource", harness.tag(case, json!({ "Env": "prod" })).await),
            ("ListTagsForResource", harness.list_tags(case).await),
            (
                "UntagResource",
                harness.untag(case, &["Env".to_string()]).await,
            ),
        ];
        for (operation, (status, body)) in probes {
            assert_eq!(
                status, expected_status,
                "{operation} on {case:?}: expected {expected_status}, got {status} with {body}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some(expected_type),
                "{operation} on {case:?}: expected {expected_type}, got {body}"
            );
        }
    }

    // None of the refusals touched the resource that does exist.
    let (_, body) = harness.list_tags(&arn).await;
    assert_eq!(
        body["Tags"],
        json!({ "Env": "prod" }),
        "a refused tagging call must leave the real resource alone, got {body}"
    );
}

/// RULE: tagging addresses a resource that exists — before `EnableSecurityHub` there
/// is no hub to tag, so every tagging call on its ARN is `ResourceNotFoundException`
/// (404), and the same holds again once `DisableSecurityHub` removes it.
#[tokio::test]
async fn tagging_a_hub_that_is_not_subscribed_is_not_found() {
    let harness = Harness::new();
    let arn = "arn:aws:securityhub:us-east-1:000000000000:hub/default";

    for stage in ["before enable", "after disable"] {
        let probes: [(&str, (u16, Value)); 3] = [
            ("TagResource", harness.tag(arn, json!({ "Env": "prod" })).await),
            ("ListTagsForResource", harness.list_tags(arn).await),
            (
                "UntagResource",
                harness.untag(arn, &["Env".to_string()]).await,
            ),
        ];
        for (operation, (status, body)) in probes {
            assert_eq!(
                status, 404,
                "{operation} {stage}: expected 404, got {status} with {body}"
            );
        }

        if stage == "before enable" {
            let (status, body) = harness.enable(None).await;
            assert_eq!(
                status, 200,
                "EnableSecurityHub expected 200, got {status} with {body}"
            );
            let reported = harness.hub_arn().await;
            assert_eq!(
                reported, arn,
                "the subscription's ARN must be the one tagged here, got {reported}"
            );
            let (status, body) = harness.tag(arn, json!({ "Env": "prod" })).await;
            assert_eq!(
                status, 200,
                "TagResource once subscribed: expected 200, got {status} with {body}"
            );
            let (status, body) = harness.call("DELETE", "/accounts", None).await;
            assert_eq!(
                status, 200,
                "DisableSecurityHub expected 200, got {status} with {body}"
            );
        }
    }
}

/// RULE: Security Hub CSPM is subscribed per account AND Region behind per-Region
/// endpoints, so a tagging call reaches only the resources of the account and Region
/// its credential scope names: a resource ARN belonging to another Region is
/// `ResourceNotFoundException` (404) even though that resource exists, and each
/// Region's tag set is independent.
#[tokio::test]
async fn tagging_reaches_only_the_credential_scope_region() {
    let harness = Harness::new();

    // Two Regions subscribed on ONE emulator, each tagged through its own scope.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let mut hubs: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let (status, body) = harness.enable_in(region).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub in {region}: expected 200, got {status} with {body}"
        );
        let arn = harness.hub_arn_in(region).await;
        let (status, body) = harness
            .tag_in(region, &arn, json!({ "Region": region }))
            .await;
        assert_eq!(
            status, 200,
            "TagResource in {region}: expected 200, got {status} with {body}"
        );
        hubs.push((region, arn));
    }

    for (region, arn) in &hubs {
        // In its own Region the resource reports its own tag, and nothing else.
        let (status, body) = harness.list_tags_in(region, arn).await;
        assert_eq!(
            status, 200,
            "ListTagsForResource in {region}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Tags"],
            json!({ "Region": region }),
            "{region}: each Region's hub must carry only its own tags, got {body}"
        );

        // From any other Region's scope the same ARN names no resource.
        for (other, _) in hubs.iter().filter(|(other, _)| other != region) {
            let probes: [(&str, (u16, Value)); 3] = [
                (
                    "TagResource",
                    harness.tag_in(other, arn, json!({ "Leaked": "yes" })).await,
                ),
                ("ListTagsForResource", harness.list_tags_in(other, arn).await),
                (
                    "UntagResource",
                    harness.untag_in(other, arn, &["Region".to_string()]).await,
                ),
            ];
            for (operation, (status, body)) in probes {
                assert_eq!(
                    status, 404,
                    "{operation} on the {region} hub from {other}: expected 404, got {status} with {body}"
                );
                assert_eq!(
                    body["__type"].as_str(),
                    Some("ResourceNotFoundException"),
                    "{operation} on the {region} hub from {other}: expected ResourceNotFoundException, got {body}"
                );
            }
        }
    }

    // Every cross-Region call left every Region's tags exactly as they were.
    for (region, arn) in &hubs {
        let (_, body) = harness.list_tags_in(region, arn).await;
        assert_eq!(
            body["Tags"],
            json!({ "Region": region }),
            "{region}: a cross-Region call must change nothing, got {body}"
        );
    }
}
