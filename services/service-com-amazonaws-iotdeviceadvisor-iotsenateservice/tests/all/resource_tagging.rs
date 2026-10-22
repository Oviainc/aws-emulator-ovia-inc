// SPDX-License-Identifier: CC0-1.0

//! What `UntagResource` REMOVES, over the wire.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket, and reads the result back through `ListTagsForResource`, so what is
//! asserted is the service's wire behaviour and not a store internal.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded request:
//!
//! * a removal takes exactly the keys it names off the resource its `resourceArn` names
//!   and leaves every other tag in place — for BOTH taggable resource kinds (suite
//!   definition, suite run), over rows differing in how many of the resource's keys are
//!   named, in keys the resource does not carry, in a key named twice, in key length and
//!   script, and up to the modelled maximum of 50 tags;
//! * a resource untagged down to nothing keeps NO tag row at all, so it is
//!   indistinguishable from one that was never tagged;
//! * removal is idempotent: repeating the same call is answered the same way and changes
//!   nothing further;
//! * the call addresses ONE resource: resources carrying identical key names are
//!   independent.
//!
//! The refusals (`a resourceArn naming no resource`, `a value that is not one of this
//! service's ARNs`) are asserted for all three tagging operations together in
//! `tests/all/resource_tags.rs`.

use std::fmt::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;
use smithy_server_runtime::MultiServiceRouter;

/// `SuiteDefinitionConfiguration$devicePermissionRoleArn` is `len 20..2048`; this is the
/// role every row below points at.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DeviceAdvisorRole";

/// The thing ARN of the device a suite run names as its primary device.
const THING_ARN: &str = "arn:aws:iot:us-east-1:000000000000:thing/device-under-test";

/// The region every case signs for, so a minted ARN is predictable.
const REGION: &str = "us-east-1";

/// `TagMap` is `len 0..50` (source: `smithy_query(query="shape",shape="TagMap")`), so this
/// is how many tags one resource can hold at once.
const MAX_TAGS: usize = 50;

/// `TagMap`'s key target is `String128` (`len 1..128`) and its value target `String256`
/// (`len 1..256`) — the bounds the longest-key/longest-value row below sits on.
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// The two resource kinds an IoT Device Advisor `resourceArn` can name: every tagging
/// operation documents the same thing about it — "This can be SuiteDefinition ARN or
/// SuiteRun ARN".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    SuiteDefinition,
    SuiteRun,
}

const KINDS: [Kind; 2] = [Kind::SuiteDefinition, Kind::SuiteRun];

/// Percent-encode one URI path segment or query value, the way an SDK does: an ARN carries
/// `:` and `/`, `/tags/{resourceArn}` is a single (non-greedy) label, and a tag key is
/// free text — `String128` carries no pattern — so it can hold `&`, `=` or a non-ASCII
/// character. Everything but the unreserved characters is encoded, byte by byte, so the
/// value survives the server's percent-decode unchanged.
fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => {
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

/// One response, reduced to what a client branches on.
struct Answer {
    status: u16,
    /// The parsed JSON body, or `Value::Null` for an empty one.
    body: Value,
    /// The restJson1 error code (`x-amzn-errortype`), when the answer carries one.
    error_code: Option<String>,
}

impl Answer {
    fn string(&self, case: &str, member: &str) -> String {
        self.body[member]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: member {member} is not a string in {}", self.body)
            })
            .to_string()
    }
}

/// The emulator, assembled in-process with authentication off: this suite asserts
/// behaviour, and signature verification is the framework's own concern.
struct Emulator {
    router: MultiServiceRouter,
    /// The one shared engine the handlers write through, kept so a case can assert an
    /// invariant of the STORE itself — that a resource untagged down to nothing keeps no
    /// tag row behind — and not only of the responses.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(iotdeviceadvisor::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Send one request, signed (in scope only — authentication is off) for [`REGION`], so
    /// the handler resolves the same region a real SDK client would.
    async fn call(&self, method: &str, path_and_query: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path_and_query)
            .header("host", format!("api.iotdeviceadvisor.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iotdeviceadvisor/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    /// Create a resource of `kind` carrying exactly `tags`, and answer its ARN.
    ///
    /// The tags go on the resource under test only: a run's definition is created
    /// untagged, so a row about a run cannot pass by reading its definition's tags.
    async fn tagged_resource(&self, case: &str, kind: Kind, tags: &Value) -> String {
        let definition_tags = match kind {
            Kind::SuiteDefinition => tags.clone(),
            Kind::SuiteRun => json!({}),
        };
        let mut body = json!({
            "suiteDefinitionConfiguration": {
                "suiteDefinitionName": "untag-suite",
                "rootGroup": "group",
                "devicePermissionRoleArn": ROLE_ARN,
            }
        });
        if !definition_tags
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            body["tags"] = definition_tags;
        }
        let created = self.call("POST", "/suiteDefinitions", Some(body)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
            created.status, created.body
        );

        match kind {
            Kind::SuiteDefinition => created.string(case, "suiteDefinitionArn"),
            Kind::SuiteRun => {
                let definition_id = created.string(case, "suiteDefinitionId");
                let mut body = json!({
                    "suiteRunConfiguration": { "primaryDevice": { "thingArn": THING_ARN } }
                });
                if !tags.as_object().is_some_and(serde_json::Map::is_empty) {
                    body["tags"] = tags.clone();
                }
                let started = self
                    .call(
                        "POST",
                        &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                        Some(body),
                    )
                    .await;
                assert_eq!(
                    started.status, 200,
                    "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
                    started.status, started.body
                );
                started.string(case, "suiteRunArn")
            }
        }
    }

    /// `UntagResource` for `arn`: `DELETE /tags/{resourceArn}` with one `tagKeys` query
    /// entry per key, as an SDK serializes the modelled `@httpQuery` list.
    async fn untag(&self, arn: &str, tag_keys: &[String]) -> Answer {
        let query = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", encode(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.call(
            "DELETE",
            &format!("/tags/{}?{query}", encode(arn)),
            None,
        )
        .await
    }

    /// The tags `ListTagsForResource` reports for `arn`, as a JSON object.
    async fn reported_tags(&self, case: &str, arn: &str) -> Value {
        let listed = self.call("GET", &format!("/tags/{}", encode(arn)), None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} {}",
            listed.status, listed.body
        );
        listed.body["tags"].clone()
    }

    /// How many tag keys the STORE holds for `arn`; `None` when it holds no row for it.
    fn stored_tag_count(&self, arn: &str) -> Option<usize> {
        let state = iotdeviceadvisor::state::service_state(&*self.engine)
            .expect("the service's store is reachable from the shared engine");
        let store = state.lock();
        store.tags.get(arn).map(std::collections::BTreeMap::len)
    }
}

/// Tag keys as the rows name them.
fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// A tag map of `count` distinct keys.
fn tag_map(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

/// `tags` without every key in `removed` — what the resource must carry after the call.
fn without_keys(tags: &Value, removed: &[String]) -> Value {
    let mut map = tags.as_object().expect("a tag map is an object").clone();
    map.retain(|key, _| !removed.contains(key));
    Value::Object(map)
}

/// The rows the removal is asserted over: `(case, the tags the resource is created with,
/// the keys the call names)`.
///
/// They differ along what the rule generalises over — how many of the resource's keys are
/// named (one of several, all of them, none it carries), a list naming the same key twice,
/// a mixed present/absent list, both `String128` key-length bounds, a non-ASCII key, a key
/// carrying URI-significant characters, a resource carrying no tags at all, and the
/// modelled `TagMap` maximum of 50 tags with half of them removed in one call.
fn untag_rows() -> Vec<(String, Value, Vec<String>)> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let full = tag_map(MAX_TAGS);
    let every_other: Vec<String> = full
        .as_object()
        .expect("a tag map is an object")
        .keys()
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(_, key)| key.clone())
        .collect();

    vec![
        (
            "one key of three".to_string(),
            json!({ "env": "prod", "team": "iot", "owner": "qa" }),
            keys(&["team"]),
        ),
        (
            "every key the resource carries".to_string(),
            json!({ "env": "prod", "team": "iot" }),
            keys(&["env", "team"]),
        ),
        (
            "a key the resource does not carry".to_string(),
            json!({ "env": "prod" }),
            keys(&["never-attached"]),
        ),
        (
            "present and absent keys in one list".to_string(),
            json!({ "env": "prod", "owner": "qa" }),
            keys(&["owner", "never-attached"]),
        ),
        (
            "the same key named twice".to_string(),
            json!({ "env": "prod", "keep": "yes" }),
            keys(&["env", "env"]),
        ),
        (
            // `String128`'s lower length bound.
            "shortest legal key, one character".to_string(),
            json!({ "k": "v", "other": "x" }),
            keys(&["k"]),
        ),
        (
            // `String128`'s upper bound, with the longest legal `String256` value.
            "longest legal key and value".to_string(),
            {
                let mut map = serde_json::Map::new();
                map.insert(long_key.clone(), json!("v".repeat(MAX_TAG_VALUE_LEN)));
                map.insert("other".to_string(), json!("x"));
                Value::Object(map)
            },
            keys(&[long_key.as_str()]),
        ),
        (
            "a non-ASCII key".to_string(),
            json!({ "проект": "устройство", "plain": "value" }),
            keys(&["проект"]),
        ),
        (
            "a key carrying URI-significant characters".to_string(),
            json!({ "a&b=c d": "v", "plain": "value" }),
            keys(&["a&b=c d"]),
        ),
        (
            "a resource carrying no tags at all".to_string(),
            json!({}),
            keys(&["never-attached"]),
        ),
        (
            format!("half of the modelled maximum of {MAX_TAGS} tags"),
            full,
            every_other,
        ),
    ]
}

/// The rule: `UntagResource` takes exactly the keys it names off the resource its ARN
/// names, leaves every other tag in place, and the removal is what every later read
/// answers. A resource left with no tags keeps no tag row at all.
#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_for_either_resource_kind() {
    let emulator = Emulator::new();

    for kind in KINDS {
        for (row, tags, removed) in untag_rows() {
            let case = format!("{kind:?}: {row}");
            let arn = emulator.tagged_resource(&case, kind, &tags).await;

            let answer = emulator.untag(&arn, &removed).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from UntagResource, got {} {} ({:?})",
                answer.status, answer.body, answer.error_code
            );
            assert!(
                answer.error_code.is_none(),
                "case {case:?}: a successful UntagResource carries no error code, got {:?}",
                answer.error_code
            );

            let expected = without_keys(&tags, &removed);
            let reported = emulator.reported_tags(&case, &arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the remaining tags {expected}, got {reported}"
            );

            let stored = emulator.stored_tag_count(&arn);
            let remaining = expected.as_object().expect("a tag map is an object").len();
            if remaining == 0 {
                assert!(
                    stored.is_none(),
                    "case {case:?}: a resource with no tags left keeps no tag row, got {stored:?}"
                );
            } else {
                assert_eq!(
                    stored,
                    Some(remaining),
                    "case {case:?}: the store must hold exactly the {remaining} remaining tags, \
                     got {stored:?}"
                );
            }
        }
    }
}

/// Removal states what the resource must NOT carry afterwards, so repeating the same call
/// is answered the same way and changes nothing further.
#[tokio::test]
async fn untagging_the_same_keys_again_is_idempotent() {
    let emulator = Emulator::new();

    for kind in KINDS {
        for (row, tags, removed) in [
            (
                "one key of three",
                json!({ "a": "1", "b": "2", "c": "3" }),
                keys(&["b"]),
            ),
            (
                "every key the resource carries",
                json!({ "a": "1", "b": "2" }),
                keys(&["a", "b"]),
            ),
        ] {
            let case = format!("{kind:?}: {row}");
            let arn = emulator.tagged_resource(&case, kind, &tags).await;
            let expected = without_keys(&tags, &removed);

            for attempt in 1..=3 {
                let answer = emulator.untag(&arn, &removed).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} attempt {attempt}: expected HTTP 200, got {} {}",
                    answer.status, answer.body
                );
                let reported = emulator.reported_tags(&case, &arn).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?} attempt {attempt}: expected the remaining tags {expected}, \
                     got {reported}"
                );
            }
        }
    }
}

/// The call addresses ONE resource: resources carrying the same key names are independent,
/// so untagging one leaves every other one exactly as it was.
#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resource_alone() {
    let emulator = Emulator::new();

    let shared = json!({ "env": "prod", "team": "iot", "owner": "qa" });
    let removed = keys(&["team"]);
    let mut resources: Vec<(String, String)> = Vec::new();
    for (index, kind) in [
        Kind::SuiteDefinition,
        Kind::SuiteDefinition,
        Kind::SuiteRun,
        Kind::SuiteRun,
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("{kind:?} #{index}");
        let arn = emulator.tagged_resource(&case, kind, &shared).await;
        resources.push((case, arn));
    }

    for target in 0..resources.len() {
        let (case, arn) = &resources[target];
        let answer = emulator.untag(arn, &removed).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );

        for (other, (other_case, other_arn)) in resources.iter().enumerate() {
            // Only the resources already untagged have lost the key; the rest still carry
            // every tag they were created with.
            let expected = if other <= target {
                without_keys(&shared, &removed)
            } else {
                shared.clone()
            };
            let reported = emulator.reported_tags(other_case, other_arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: after untagging resource {target}, resource {other} \
                 ({other_case:?}) must carry {expected}, got {reported}"
            );
        }
    }
}
