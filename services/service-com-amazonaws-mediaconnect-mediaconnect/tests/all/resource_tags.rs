// SPDX-License-Identifier: CC0-1.0

//! The tagging slice, end to end over HTTP: `TagResource`, `ListTagsForResource` and
//! `UntagResource`.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the same `MultiServiceRouter`
//! the emulator's front door uses, so what these tests assert is what a MediaConnect
//! client sees on the wire. The rules under test:
//!
//! * `TagResource` MERGES the pairs it names into the addressed resource's stored tag
//!   map — a key it names is set or overwritten, a key it does not name keeps the value
//!   it had ("if existing tags on a resource are not specified in the request parameters,
//!   they are not changed") — and no other resource's tags move;
//! * `ListTagsForResource` reports that same stored map, whatever its size, keys or
//!   values, and changes nothing a later caller sees;
//! * `UntagResource` removes exactly the keys it names, and removing a key the resource
//!   does not carry is a no-op success (the operation is `@idempotent`);
//! * a `ResourceArn` that addresses no resource this service holds is HTTP 404
//!   `NotFoundException` on all three — never a 501 and never a success;
//! * a tag pair outside the bounds the model's own member documentation states, and a
//!   write that would push a resource past the tags-per-resource bound, are refused with
//!   HTTP 400 `BadRequestException` and store nothing.
//!
//! The tables span the class each rule covers rather than one tag set: the pair count
//! (none, one, several, the per-resource maximum), new keys and keys that overwrite,
//! empty, non-ASCII and URI-significant values, and the documented key/value length
//! bounds with one character past each.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

/// A SigV4 `Authorization` header whose credential scope names this service and the
/// Region every ARN below is minted in. It is the routing signal the front door reads.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLEEXAMPLE/20260101/us-east-1/mediaconnect/aws4_request, \
     SignedHeaders=host, Signature=deadbeef";

/// The Region of [`AUTHORIZATION`].
const REGION: &str = "us-east-1";

/// The one account id a generated emulator presents (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// Percent-encode one path segment or query value: everything outside RFC 3986 §2.3's
/// unreserved set is escaped, which is what an SDK client sends for an ARN bound to a
/// NON-greedy `@httpLabel` (`/tags/{ResourceArn}`) and for each `tagKeys` query pair.
fn pct(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A deterministic string of `length` ASCII characters, for exercising a documented
/// length bound rather than one hand-picked spelling.
fn filler(length: usize, seed: u8) -> String {
    (0..length)
        .map(|index| char::from(b'a' + ((index as u8).wrapping_add(seed) % 26)))
        .collect()
}

/// A JSON tag map from `(key, value)` pairs.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// One answer from the wire: `(status, error code, body)`.
type Reply = (u16, Option<String>, Value);

/// One emulator: a router with only this service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject here; the requests below carry a scope, not a
        // real signature.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediaconnect_mediaconnect::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Send one request and return `(status, error code, body)`. The error code is the
    /// restJson1 `x-amzn-errortype` header, absent on success.
    async fn send(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Option<String>, Value) {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", "mediaconnect.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, code, parsed)
    }

    /// Create a flow and return its minted ARN, the identifier every tagging call below
    /// addresses it by.
    async fn create_flow(&self, name: &str, flow_tags: Option<Value>) -> String {
        let mut body = json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        });
        if let Some(tags) = flow_tags {
            body["flowTags"] = tags;
        }
        let (status, code, created) = self.send("POST", "/v1/flows", Some(body)).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "creating the fixture flow {name:?}: expected 201, got {status} {code:?}: {created}"
        );
        created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// `POST /tags/{ResourceArn}` with the pairs in `tags`.
    async fn tag(&self, resource_arn: &str, tags: Value) -> (u16, Option<String>, Value) {
        self.send(
            "POST",
            &format!("/tags/{}", pct(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }

    /// `GET /tags/{ResourceArn}`.
    async fn list_tags(&self, resource_arn: &str) -> (u16, Option<String>, Value) {
        self.send("GET", &format!("/tags/{}", pct(resource_arn)), None)
            .await
    }

    /// `DELETE /tags/{ResourceArn}?tagKeys=..&tagKeys=..` — `TagKeys` is bound to
    /// `@httpQuery("tagKeys")`, so each key travels as its own query pair.
    async fn untag(&self, resource_arn: &str, keys: &[&str]) -> (u16, Option<String>, Value) {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", pct(key)))
            .collect::<Vec<_>>()
            .join("&");
        let path = format!("/tags/{}", pct(resource_arn));
        let uri = if query.is_empty() {
            path
        } else {
            format!("{path}?{query}")
        };
        self.send("DELETE", &uri, None).await
    }

    /// The tags `ListTagsForResource` reports for `resource_arn`, asserted to be a 200.
    async fn reported_tags(&self, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
        let (status, code, body) = self.list_tags(resource_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: list tags expected 200, got {status} {code:?}: {body}"
        );
        body["tags"]
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
            .collect()
    }
}

/// TagResource merges the pairs it names into the resource's stored tags, and
/// ListTagsForResource reports exactly that merged map. The expected map is maintained
/// by the rule itself (insert the named keys, touch nothing else), so each row asserts
/// the rule rather than a recorded answer.
#[tokio::test]
async fn tag_resource_merges_the_named_keys_into_the_stored_tags() {
    let emulator = Emulator::new();
    let tagged = emulator
        .create_flow(
            "tagged-flow",
            Some(json!({ "env": "prod", "team": "video" })),
        )
        .await;
    // A second flow, tagged differently, to pin that a write addresses ONE resource.
    let other = emulator
        .create_flow("other-flow", Some(json!({ "env": "dev" })))
        .await;

    let max_key = filler(mc::MAX_TAG_KEY_LENGTH, 3);
    let max_value = filler(mc::MAX_TAG_VALUE_LENGTH, 7);

    // Each row differs from the others along what the merge rule generalises over: how
    // many pairs the request carries, whether a key is new or overwrites, and the form
    // of the key and the value.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no pairs at all", vec![]),
        ("one new key", vec![("owner".into(), "ops".into())]),
        (
            "overwrite a key the flow already carries",
            vec![("env".into(), "staging".into())],
        ),
        (
            "two pairs at once: one overwrite, one new key",
            vec![
                ("team".into(), "video-eng".into()),
                ("cost-center".into(), "cc-42".into()),
            ],
        ),
        (
            "non-ASCII key and value",
            vec![("проект".into(), "видео-поток".into())],
        ),
        ("an empty value", vec![("blank".into(), String::new())]),
        (
            "key and value at the documented maximum character lengths",
            vec![(max_key.clone(), max_value.clone())],
        ),
        (
            "URI-significant characters in the key and the value",
            vec![("path/key".into(), "a:b c&d=e".into())],
        ),
        (
            "re-stating a key with the value it already has",
            vec![("owner".into(), "ops".into())],
        ),
        (
            "three pairs, all new",
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "2".into()),
                ("c".into(), "3".into()),
            ],
        ),
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::from([
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "video".to_string()),
    ]);
    for (case, pairs) in rows {
        let (status, code, body) = emulator.tag(&tagged, tag_map(&pairs)).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: tag expected 204 with no error, got {status} {code:?}: {body}"
        );

        for (key, value) in pairs {
            expected.insert(key, value);
        }
        let reported = emulator.reported_tags(case, &tagged).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: list must report the merged tag map, got {reported:?}"
        );
        // Reading the tags changed nothing: the same read answers the same map.
        let again = emulator.reported_tags(case, &tagged).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second list must report the same map, got {again:?}"
        );
        // The other flow's tags are untouched by every write above.
        let untouched = emulator.reported_tags(case, &other).await;
        assert_eq!(
            untouched,
            BTreeMap::from([("env".to_string(), "dev".to_string())]),
            "case {case:?}: another flow's tags must not move, got {untouched:?}"
        );
    }
}

/// UntagResource removes exactly the keys it names and leaves every other key alone;
/// naming a key the resource does not carry removes nothing and is still a success.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_flow(
            "untagged-flow",
            Some(json!({
                "env": "prod",
                "team": "video",
                "owner": "ops",
                "cost-center": "cc-42",
                "проект": "видео-поток",
                "path/key": "a:b c"
            })),
        )
        .await;

    // Rows differ along the count of keys removed, whether a named key exists, and the
    // form of the key. `TagKeys` is a REQUIRED `@httpQuery` list, so every row names at
    // least one key: a `DELETE /tags/{ResourceArn}` with no `tagKeys` pair is not a
    // request this operation has.
    let rows: Vec<(&str, &[&str])> = vec![
        ("one key the flow carries", &["env"]),
        ("a key the flow does not carry", &["absent"]),
        (
            "the same absent key again — removal is idempotent",
            &["absent"],
        ),
        (
            "two keys, one carried and one absent",
            &["team", "also-absent"],
        ),
        ("a non-ASCII key", &["проект"]),
        ("a key with URI-significant characters", &["path/key"]),
        (
            "every remaining key at once",
            &["owner", "cost-center"],
        ),
        ("an already empty tag map", &["owner"]),
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::from([
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "video".to_string()),
        ("owner".to_string(), "ops".to_string()),
        ("cost-center".to_string(), "cc-42".to_string()),
        ("проект".to_string(), "видео-поток".to_string()),
        ("path/key".to_string(), "a:b c".to_string()),
    ]);
    for (case, keys) in rows {
        let (status, code, body) = emulator.untag(&arn, keys).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: untag expected 204 with no error, got {status} {code:?}: {body}"
        );

        for key in keys {
            expected.remove(*key);
        }
        let reported = emulator.reported_tags(case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: list must report the keys that are left, got {reported:?}"
        );
    }

    // And a tag write after the removals starts from what is left, not from what was
    // once there.
    let (status, code, body) = emulator.tag(&arn, json!({ "env": "back" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "re-tagging an emptied flow expected 204, got {status} {code:?}: {body}"
    );
    let reported = emulator.reported_tags("re-tag after removal", &arn).await;
    assert_eq!(
        reported,
        BTreeMap::from([("env".to_string(), "back".to_string())]),
        "re-tagging an emptied flow leaves only the new pair, got {reported:?}"
    );
}

/// Every tagging call resolves its `ResourceArn` against the resources this service
/// holds, and refuses one that addresses nothing with the modelled `NotFoundException`
/// (404) — never a 501 and never a success. The rows are the ways an ARN can address
/// nothing: a flow that was deleted, a flow that never existed, another MediaConnect
/// resource type, another service's resource, and a string that is no ARN at all.
#[tokio::test]
async fn tagging_an_arn_that_addresses_nothing_is_not_found() {
    let emulator = Emulator::new();
    let deleted = emulator.create_flow("doomed-flow", None).await;
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{deleted}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "deleting the fixture flow expected 202, got {status} {code:?}: {body}"
    );

    let arns = [
        ("a flow ARN whose flow was deleted", deleted),
        (
            "a flow ARN this service never minted",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000beef:never-made"),
        ),
        (
            "a bridge ARN — bridges are not resources this emulator holds",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:bridge:1-0000beef:some-bridge"),
        ),
        (
            "a gateway ARN — gateways are not resources this emulator holds",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:gateway:1-0000beef:some-gateway"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channel:1234567"),
        ),
        ("a string that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in arns {
        let calls: Vec<(&str, Reply)> = vec![
            ("TagResource", emulator.tag(&arn, json!({ "k": "v" })).await),
            ("ListTagsForResource", emulator.list_tags(&arn).await),
            ("UntagResource", emulator.untag(&arn, &["k"]).await),
        ];
        for (operation, (status, code, body)) in calls {
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?} / {operation}: expected 404 NotFoundException, got {status} \
                 {code:?}: {body}"
            );
        }
    }

    // Precedence, pinned so it cannot drift: a pair that breaks the tag-pair bound is
    // refused on the pair (400) rather than on the ARN (404), because the request is
    // checked before any resource is resolved.
    let unknown = format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000beef:never-made");
    let (status, code, body) = emulator
        .tag(
            &unknown,
            tag_map(&[(filler(mc::MAX_TAG_KEY_LENGTH + 1, 9), "v".to_string())]),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "an over-long key on an unknown ARN is refused on the pair, got {status} {code:?}: {body}"
    );
}

/// One row of the bounds table: the pairs a TagResource call carries, and the status and
/// error code the service answers.
struct BoundCase {
    case: &'static str,
    pairs: Vec<(String, String)>,
    status: u16,
    code: Option<&'static str>,
}

/// A tag pair outside the bounds the model's own member documentation states — "tag keys
/// can have a maximum character length of 128 characters, and tag values can have a
/// maximum length of 256 characters" — is refused with `BadRequestException` (400), and a
/// pair AT either bound is stored. The lengths count characters, so a 256-character
/// multi-byte value is within the bound.
#[tokio::test]
async fn tag_pairs_outside_the_documented_bounds_are_refused() {
    let emulator = Emulator::new();
    let arn = emulator.create_flow("bounded-flow", None).await;

    let cases = vec![
        BoundCase {
            case: "a key at the documented maximum length",
            pairs: vec![(filler(mc::MAX_TAG_KEY_LENGTH, 1), "v".into())],
            status: 204,
            code: None,
        },
        BoundCase {
            case: "a key one character past the maximum length",
            pairs: vec![(filler(mc::MAX_TAG_KEY_LENGTH + 1, 1), "v".into())],
            status: 400,
            code: Some("BadRequestException"),
        },
        BoundCase {
            case: "a value at the documented maximum length",
            pairs: vec![("at-value-bound".into(), filler(mc::MAX_TAG_VALUE_LENGTH, 2))],
            status: 204,
            code: None,
        },
        BoundCase {
            case: "a value one character past the maximum length",
            pairs: vec![(
                "past-value-bound".into(),
                filler(mc::MAX_TAG_VALUE_LENGTH + 1, 2),
            )],
            status: 400,
            code: Some("BadRequestException"),
        },
        BoundCase {
            case: "a value of 256 multi-byte characters — the bound counts characters",
            pairs: vec![(
                "multibyte-value".into(),
                "é".repeat(mc::MAX_TAG_VALUE_LENGTH),
            )],
            status: 204,
            code: None,
        },
        BoundCase {
            case: "an empty key names no tag",
            pairs: vec![(String::new(), "v".into())],
            status: 400,
            code: Some("BadRequestException"),
        },
        BoundCase {
            case: "one good pair beside an over-long key: neither is stored",
            pairs: vec![
                ("would-be-stored".into(), "v".into()),
                (filler(mc::MAX_TAG_KEY_LENGTH + 1, 5), "v".into()),
            ],
            status: 400,
            code: Some("BadRequestException"),
        },
    ];

    // The expected map is grown only by the rows the rule accepts, so a refused row is
    // also asserted to have stored nothing.
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for BoundCase {
        case,
        pairs,
        status: want_status,
        code: want_code,
    } in cases
    {
        let (status, code, body) = emulator.tag(&arn, tag_map(&pairs)).await;
        assert_eq!(
            (status, code.as_deref()),
            (want_status, want_code),
            "case {case:?}: expected {want_status} {want_code:?}, got {status} {code:?}: {body}"
        );
        if want_status == 204 {
            for (key, value) in pairs {
                expected.insert(key, value);
            }
        }
        let reported = emulator.reported_tags(case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: only accepted pairs are stored, got {} keys",
            reported.len()
        );
    }
}

/// The tags-per-resource bound applies to the MERGED set a write would leave on the
/// resource: a write that fills it is accepted, one that would add a key past it is
/// refused with `BadRequestException` (400) and stores nothing, and a write that only
/// re-states keys the resource already carries is accepted at the bound.
#[tokio::test]
async fn a_resource_keeps_at_most_the_bounded_number_of_tags() {
    const LIMIT: usize = mc::MAX_TAGS_PER_RESOURCE;

    let emulator = Emulator::new();
    let arn = emulator.create_flow("full-of-tags", None).await;

    let at_bound: Vec<(String, String)> = (0..LIMIT)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let (status, code, body) = emulator.tag(&arn, tag_map(&at_bound)).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "a write that fills the bound ({LIMIT}) expected 204, got {status} {code:?}: {body}"
    );
    let expected: BTreeMap<String, String> = at_bound.iter().cloned().collect();
    let reported = emulator.reported_tags("at the bound", &arn).await;
    assert_eq!(
        reported, expected,
        "at the bound the flow carries every written pair, got {} keys",
        reported.len()
    );

    // Re-stating keys it already carries does not grow the set, so it is accepted.
    let (status, code, body) = emulator
        .tag(&arn, json!({ "key-0": "rewritten" }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "overwriting a key at the bound expected 204, got {status} {code:?}: {body}"
    );

    // One key past the bound is refused, and the refusal stores neither pair it carried.
    let (status, code, body) = emulator
        .tag(
            &arn,
            json!({ "one-too-many": "v", "another-too-many": "v" }),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a write past the bound ({LIMIT}) expected 400 BadRequestException, got {status} \
         {code:?}: {body}"
    );
    let mut still_expected = expected.clone();
    still_expected.insert("key-0".to_string(), "rewritten".to_string());
    let reported = emulator.reported_tags("past the bound", &arn).await;
    assert_eq!(
        reported, still_expected,
        "a refused write stores nothing, got {} keys",
        reported.len()
    );

    // Freeing a slot makes room again: the bound is on the stored set, not on history.
    let (status, code, body) = emulator.untag(&arn, &["key-0"]).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "freeing a slot expected 204, got {status} {code:?}: {body}"
    );
    let (status, code, body) = emulator.tag(&arn, json!({ "one-too-many": "v" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "after freeing a slot the write expected 204, got {status} {code:?}: {body}"
    );
    let reported = emulator.reported_tags("after freeing a slot", &arn).await;
    assert_eq!(
        reported.len(),
        LIMIT,
        "the flow is back at the bound ({LIMIT}), got {} keys",
        reported.len()
    );
}

/// The tags a flow is created with are the tags the tagging slice reads, and DeleteFlow
/// takes them with the flow: "when a resource is deleted, the tags associated with that
/// resource are deleted as well". Recreating the same flow name mints a new flow with
/// only the tags its own create named.
#[tokio::test]
async fn a_deleted_flow_takes_its_tags_with_it() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_flow("recycled-flow", Some(json!({ "env": "prod" })))
        .await;
    let reported = emulator.reported_tags("created with flowTags", &arn).await;
    assert_eq!(
        reported,
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        "CreateFlow's flowTags are the tags the tagging slice reads, got {reported:?}"
    );

    let (status, code, body) = emulator.tag(&arn, json!({ "team": "video" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "tagging the flow expected 204, got {status} {code:?}: {body}"
    );
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "deleting the flow expected 202, got {status} {code:?}: {body}"
    );

    let (status, code, body) = emulator.list_tags(&arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "the deleted flow's tags are gone with it, got {status} {code:?}: {body}"
    );

    // A new flow with the same name is a new resource with a new ARN and only its own
    // tags.
    let recreated = emulator.create_flow("recycled-flow", None).await;
    assert_ne!(
        recreated, arn,
        "recreating the name mints a new flow ARN, got {recreated}"
    );
    let reported = emulator.reported_tags("recreated flow", &recreated).await;
    assert!(
        reported.is_empty(),
        "a recreated flow carries no tags of the deleted one, got {reported:?}"
    );
}
