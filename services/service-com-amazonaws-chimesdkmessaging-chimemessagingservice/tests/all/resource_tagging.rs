// SPDX-License-Identifier: CC0-1.0

//! The generic tagging surface of this service, end to end over HTTP.
//!
//! The RULE pinned here: a resource's tags are ARN-keyed state the three generic tagging
//! routes share. `TagResource` MERGES the pairs it carries into what the resource already
//! has (a repeated key is re-valued, an untouched one kept), `UntagResource` removes
//! exactly the keys it names and nothing else, and `ListTagsForResource` reports back the
//! tags those two and the CREATE left — an empty list for a resource carrying none. A
//! tagging call keyed by a resource of this service that does not exist is refused and
//! writes nothing, as is one that would take a resource past the modelled maximum number
//! of tags per resource.
//!
//! The tables span what the rule generalises over: how many pairs one call carries (one,
//! several, the modelled maximum, one past it), the form of a key and a value (short,
//! non-ASCII, at the modelled length bounds), which resource is tagged (a channel and a
//! channel FLOW, each tagged at create time and afterwards), how many keys an untag names
//! (one, several, one the resource never carried), and the order the calls arrive in.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::state::MAX_TAGS_PER_RESOURCE;
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The restJson1 label/query encoder this binary's other suites already state: one copy
// for the whole test target, so two suites cannot encode an ARN differently.
use super::channel_messaging::encode;

/// The `AppInstanceArn` every resource below is created under.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";
/// The `x-amz-chime-bearer` identity every request is made as.
const BEARER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/alice";
/// A processor Lambda ARN inside the modelled `LambdaFunctionArn` pattern, for the
/// channel flows a row tags.
const PROCESSOR_LAMBDA: &str = "arn:aws:lambda:us-east-1:111122223333:function:redact";

/// The emulator under test: this one service mounted in the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        // ONE shared engine, exactly as `router/src/lib.rs` builds it, so every request
        // in a case observes the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router
            .set_request_authentication(false)
            .register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> (u16, Value) {
        let bytes = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the request")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://messaging-chime.us-east-1.amazonaws.com{uri}"))
            .header("host", "messaging-chime.us-east-1.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/chime/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("content-type", "application/json")
            .header("x-amz-chime-bearer", BEARER)
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// Create a channel with only the required members and return its ARN.
    async fn channel(&self, name: &str, token: &str, tags: &[(String, String)]) -> String {
        let mut request = json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": name,
            "ClientRequestToken": token,
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self.call("POST", "/channels", Some(&request)).await;
        assert_eq!(
            status, 201,
            "CreateChannel({name}) must answer 201, got {status} {body:?}"
        );
        body["ChannelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateChannel({name}) returned no ChannelArn: {body:?}"))
            .to_string()
    }

    /// Create a channel flow with one processor and return its ARN.
    async fn channel_flow(&self, name: &str, token: &str, tags: &[(String, String)]) -> String {
        let mut request = json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": name,
            "ClientRequestToken": token,
            "Processors": [{
                "Name": "p1",
                "Configuration": { "Lambda": {
                    "ResourceArn": PROCESSOR_LAMBDA,
                    "InvocationType": "ASYNC",
                } },
                "ExecutionOrder": 1,
                "FallbackAction": "CONTINUE",
            }],
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self.call("POST", "/channel-flows", Some(&request)).await;
        assert_eq!(
            status, 201,
            "CreateChannelFlow({name}) must answer 201, got {status} {body:?}"
        );
        body["ChannelFlowArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("CreateChannelFlow({name}) returned no ChannelFlowArn: {body:?}")
            })
            .to_string()
    }

    /// `POST /tags?operation=tag-resource` with the pairs this call applies.
    async fn tag(&self, arn: &str, pairs: &[(String, String)]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=tag-resource",
            Some(&json!({ "ResourceARN": arn, "Tags": wire_tags(pairs) })),
        )
        .await
    }

    /// `POST /tags?operation=untag-resource` with the keys this call removes.
    async fn untag(&self, arn: &str, tag_keys: &[String]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=untag-resource",
            Some(&json!({ "ResourceARN": arn, "TagKeys": tag_keys })),
        )
        .await
    }

    /// `GET /tags?arn=…`, raw.
    async fn list_raw(&self, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags?arn={}", encode(arn)), None)
            .await
    }

    /// `GET /tags?arn=…`, as the map a client reads the answer as.
    async fn list(&self, arn: &str) -> (u16, BTreeMap<String, String>) {
        let (status, body) = self.list_raw(arn).await;
        (status, tag_map(&body))
    }
}

fn wire_tags(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

fn tag_map(body: &Value) -> BTreeMap<String, String> {
    body["Tags"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| {
                    Some((
                        tag["Key"].as_str()?.to_string(),
                        tag["Value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Owned pairs, so a row can carry a value it builds (a key at the modelled length
/// bound) beside the literals.
fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn as_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One row of the round-trip table: the pairs each successive tagging call applies, the
/// keys untagged afterwards, and the tags the resource is left with.
struct TagCase {
    case: &'static str,
    calls: Vec<Vec<(String, String)>>,
    removed: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tagging merges, untagging removes only the keys it names, and the listing
/// reports exactly the tags the writes left — for every resource kind this service mints.
#[tokio::test]
async fn tags_round_trip_through_the_shared_tagging_routes() {
    let emulator = Emulator::new();

    // The pairs below stay inside the model's own bounds: `TagKey` is len 1..128 and
    // `TagValue` len 1..256 (`smithy_query(query="shape",shape="Tag")`).
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let at_bounds = owned(&[
        (long_key.as_str(), long_value.as_str()),
        ("équipe", "ノート"),
    ]);

    let rows: Vec<TagCase> = vec![
        TagCase {
            case: "one pair, kept",
            calls: vec![owned(&[("env", "dev")])],
            removed: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "several pairs in one call, one of them removed again",
            calls: vec![owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")])],
            removed: keys(&["team"]),
            left: owned(&[("env", "dev"), ("tier", "gold")]),
        },
        TagCase {
            case: "two calls merging, the repeated key re-valued",
            calls: vec![
                owned(&[("env", "dev"), ("team", "core")]),
                owned(&[("env", "prod"), ("tier", "gold")]),
            ],
            removed: Vec::new(),
            left: owned(&[("env", "prod"), ("team", "core"), ("tier", "gold")]),
        },
        TagCase {
            case: "a key the resource never carried is removed as a no-op",
            calls: vec![owned(&[("env", "dev")])],
            removed: keys(&["absent", "neither"]),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "every key removed, leaving no tags at all",
            calls: vec![owned(&[("env", "dev"), ("team", "core")])],
            removed: keys(&["env", "team"]),
            left: Vec::new(),
        },
        TagCase {
            case: "keys and values at the modelled length bounds, and non-ASCII text",
            calls: vec![at_bounds.clone()],
            removed: Vec::new(),
            left: at_bounds,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let TagCase {
            case,
            calls,
            removed,
            left,
        } = row;
        // Both resource kinds this service mints, so the rule is pinned over the resource
        // as well as over the tags.
        let channel = emulator
            .channel(&format!("c{index}"), &format!("tok-tag-{index}"), &[])
            .await;
        let flow = emulator
            .channel_flow(&format!("f{index}"), &format!("tok-tag-{index}-f"), &[])
            .await;

        for arn in [channel.as_str(), flow.as_str()] {
            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, tags.len()),
                (200, 0),
                "case {case:?} ({arn}): an untagged resource reports no tags, got {tags:?}"
            );

            for pairs in &calls {
                let (status, body) = emulator.tag(arn, pairs).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected TagResource to answer 204, got \
                     {status} {body:?}"
                );
            }
            if !removed.is_empty() {
                let (status, body) = emulator.untag(arn, &removed).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected UntagResource to answer 204, got \
                     {status} {body:?}"
                );
            }

            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, &tags),
                (200, &as_map(&left)),
                "case {case:?} ({arn}): expected exactly the tags the writes left"
            );

            // A read does not change what the next reader sees.
            let (status, again) = emulator.list(arn).await;
            assert_eq!(
                (status, &again),
                (200, &tags),
                "case {case:?} ({arn}): a second listing differed from the first"
            );
        }
    }
}

/// One row of the create-time table: the tags the create carries, what the later calls do
/// to them, and the tags the resource is left with.
struct CreateTagCase {
    case: &'static str,
    created_with: Vec<(String, String)>,
    then_tagged: Vec<(String, String)>,
    untagged: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tags given at CREATE time are the same state the tagging routes read and write,
/// so a created resource lists them, the tagging routes change them, and deleting the
/// resource takes them with it.
#[tokio::test]
async fn tags_supplied_at_create_time_are_listed_and_can_be_changed() {
    let emulator = Emulator::new();

    let rows: Vec<CreateTagCase> = vec![
        CreateTagCase {
            case: "one create-time tag, left alone",
            created_with: owned(&[("env", "dev")]),
            then_tagged: Vec::new(),
            untagged: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        CreateTagCase {
            case: "three create-time tags, one re-valued and one dropped",
            created_with: owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")]),
            then_tagged: owned(&[("env", "prod")]),
            untagged: keys(&["tier"]),
            left: owned(&[("env", "prod"), ("team", "core")]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let CreateTagCase {
            case,
            created_with,
            then_tagged,
            untagged,
            left,
        } = row;
        let channel = emulator
            .channel(
                &format!("ct{index}"),
                &format!("tok-create-tag-{index}"),
                &created_with,
            )
            .await;
        let flow = emulator
            .channel_flow(
                &format!("ft{index}"),
                &format!("tok-create-tag-{index}-f"),
                &created_with,
            )
            .await;

        for arn in [channel.as_str(), flow.as_str()] {
            let (status, listed) = emulator.list(arn).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&created_with)),
                "case {case:?} ({arn}): expected the create-time tags to be listed"
            );

            if !then_tagged.is_empty() {
                emulator.tag(arn, &then_tagged).await;
            }
            if !untagged.is_empty() {
                emulator.untag(arn, &untagged).await;
            }

            let (status, listed) = emulator.list(arn).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&left)),
                "case {case:?} ({arn}): expected the create-time tags as the later calls left them"
            );
        }

        // The resource's tags go with the resource: once the channel is deleted, its ARN
        // is not a resource of this service any more and the listing refuses it.
        let (status, body) = emulator
            .call("DELETE", &format!("/channels/{}", encode(&channel)), None)
            .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected DeleteChannel to answer 204, got {status} {body:?}"
        );
        let (status, body) = emulator.list_raw(&channel).await;
        assert_eq!(
            (status, body["Code"].as_str()),
            (400, Some("NotFound")),
            "case {case:?}: expected a deleted resource's tags to be refused, got {body:?}"
        );
    }
}

/// RULE: a tagging call that names a resource of this service that does not exist is
/// refused with the modelled client error carrying the `NotFound` code and writes
/// nothing, and a call past the modelled maximum number of tags per resource is refused,
/// leaving the resource's tags as they were.
#[tokio::test]
async fn a_tagging_call_this_service_cannot_serve_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let channel = emulator.channel("refusals", "tok-tag-refuse", &[]).await;
    let flow = emulator
        .channel_flow("refusals", "tok-tag-refuse-f", &[])
        .await;

    // Rows: each kind of ARN of THIS service that names nothing — a channel, a channel
    // flow — plus an ARN outside this family's ARN namespace altogether, which names no
    // resource this service serves tagging for.
    let absent: Vec<(&str, String)> = vec![
        ("a channel that never existed", format!("{channel}x")),
        ("a channel flow that never existed", format!("{flow}x")),
        (
            "a channel of an app instance with no resources",
            format!("{APP_INSTANCE}2/channel/c9"),
        ),
        (
            "another vendor's resource ARN",
            "arn:aws:sqs:us-east-1:000000000000:queue/q1".to_string(),
        ),
    ];

    for (case, arn) in absent {
        let refusals = [
            ("TagResource", emulator.tag(&arn, &owned(&[("k", "v")])).await),
            ("UntagResource", emulator.untag(&arn, &keys(&["k"])).await),
            ("ListTagsForResource", emulator.list_raw(&arn).await),
        ];
        for (operation, (status, payload)) in refusals {
            assert_eq!(
                (status, payload["Code"].as_str()),
                (400, Some("NotFound")),
                "case {case:?} ({operation}): expected 400 NotFound for {arn}, got \
                 {status} {payload:?}"
            );
        }
    }

    // The tag count is bounded by the model's own `TagList` maximum: a call that would
    // take the resource past it is refused and writes nothing.
    let at_maximum: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("k{index:02}"), "v".to_string()))
        .collect();
    let (status, body) = emulator.tag(&channel, &at_maximum).await;
    assert_eq!(
        status, 204,
        "case the modelled maximum of tags: expected it accepted, got {status} {body:?}"
    );

    let (status, body) = emulator.tag(&channel, &owned(&[("one-too-many", "v")])).await;
    assert_eq!(
        (status, body["Code"].as_str()),
        (400, Some("ResourceLimitExceeded")),
        "case one tag past the maximum: expected the modelled resource-limit refusal, got \
         {status} {body:?}"
    );
    let (status, listed) = emulator.list(&channel).await;
    assert_eq!(
        (status, listed.len()),
        (200, at_maximum.len()),
        "case one tag past the maximum: expected the refused call to write nothing"
    );

    // Re-valuing a key the resource already carries adds none, so it is accepted even at
    // the maximum.
    let (status, body) = emulator
        .tag(&channel, &owned(&[("k00", "re-valued")]))
        .await;
    assert_eq!(
        status, 204,
        "case re-valuing a key at the maximum: expected it accepted, got {status} {body:?}"
    );
    let (status, listed) = emulator.list(&channel).await;
    assert_eq!(
        (status, listed.get("k00").map(String::as_str)),
        (200, Some("re-valued")),
        "case re-valuing a key at the maximum: expected the new value, got {listed:?}"
    );
}
