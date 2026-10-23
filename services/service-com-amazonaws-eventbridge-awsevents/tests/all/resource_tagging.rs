// SPDX-License-Identifier: CC0-1.0

//! The EventBridge tagging slice end to end over the wire (`awsJson1_1`, `POST /`
//! with `X-Amz-Target: AWSEvents.<Operation>`), dispatched through the real
//! `MultiServiceRouter` — so every case exercises routing, deserialization, the
//! modelled-constraint validator, the handler, the persisted store and
//! serialization: the same path a client takes.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded shape:
//!
//! * a tag write persists the request's pairs onto the resource its `ResourceARN`
//!   names and a later tag read reports exactly that set — for EVERY resource kind
//!   this service tags and every ARN form it mints (the account's `default` bus, a
//!   custom bus, a rule on the `default` bus, a rule on a custom bus);
//! * a tag write MERGES: a key the request names takes the value it names, in the
//!   position that key already held, and a key it does not name keeps its value;
//! * `UntagResource` removes exactly the keys the request names, and a key the
//!   resource does not carry removes nothing;
//! * a well-formed ARN naming a resource no record carries is
//!   `ResourceNotFoundException` for every tagging operation, and creates nothing;
//! * text that is not the ARN of a taggable resource of this service is refused as
//!   a bad request, never as a missing resource and never as a 501;
//! * the per-resource tag quota is charged on the RESULT of a write, and the
//!   modelled key and value bounds are enforced on every bound and one past it;
//! * tags a CREATE names are stored, while tags an UPDATE names are ignored.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. Deliberately NOT the store's
/// fallback region, so an ARN that ignored the request's credential scope would
/// show up as a wrong region segment rather than passing by accident.
const REGION: &str = "eu-west-2";

/// Largest number of tags one resource carries: "You can associate as many as 50
/// tags with a resource" (`TagResource`'s documentation, via
/// `smithy_query(query="doc", shape="TagResource")`). Spelled out here, not
/// imported, so a wrong bound in the implementation cannot make a test pass.
const TAG_QUOTA: usize = 50;

/// `TagKey`'s modelled `@length(min: 1, max: 128)` and `TagValue`'s
/// `@length(min: 0, max: 256)`, from
/// `smithy_query(query="shape", shape="TagKey,TagValue")`.
const TAG_KEY_MIN: usize = 1;
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MIN: usize = 0;
const TAG_VALUE_MAX: usize = 256;

/// A tag set a case names: key/value pairs in the order they are written.
type TagSet = Vec<(&'static str, &'static str)>;

/// One row of the resource-kind table: the case, the ARN, and the tags written.
type ResourceCase = (&'static str, String, Vec<(&'static str, String)>);

/// One row of the merge table: the case, the writes in order, the set they leave.
type MergeCase = (&'static str, Vec<TagSet>, TagSet);

/// One row of the removal table: the case, the keys removed, the set left.
type RemovalCase = (&'static str, Vec<&'static str>, TagSet);

/// The emulator: one router carrying only this service over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    Emulator { router }
}

/// This emulator's account id — the account segment of every ARN it mints.
fn account() -> &'static str {
    engine::emulator_account_id()
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>`, built here rather than
/// read from the implementation.
fn bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

/// The ARN of a rule on the account's `default` bus, which carries no bus segment.
fn default_bus_rule_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:rule/{name}", account())
}

/// The ARN of a rule on a custom bus, which names that bus.
fn rule_arn(bus: &str, name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:rule/{bus}/{name}", account())
}

/// One `Tag` on the wire.
fn tag(key: &str, value: &str) -> Value {
    json!({ "Key": key, "Value": value })
}

/// `TagResource`, as `(status, error code)` plus the body.
async fn tag_resource(emulator: &Emulator, arn: &str, tags: Vec<Value>) -> (u16, Value) {
    call(
        emulator,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": tags }),
    )
    .await
}

/// `UntagResource`.
async fn untag_resource(emulator: &Emulator, arn: &str, keys: Vec<&str>) -> (u16, Value) {
    call(
        emulator,
        "UntagResource",
        &json!({ "ResourceARN": arn, "TagKeys": keys }),
    )
    .await
}

/// The tags `ListTagsForResource` reports for `arn`, in the order it reports them.
/// Panics unless the read succeeded, so a caller asserting a tag set never reads a
/// failure as "no tags".
async fn stored_tags(emulator: &Emulator, arn: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        emulator,
        "ListTagsForResource",
        &json!({ "ResourceARN": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the tags of {arn}: expected 200, got {status} {body:?}"
    );
    let reported = body
        .get("Tags")
        .unwrap_or_else(|| panic!("ListTagsForResource({arn}) carries no Tags member: {body:?}"))
        .as_array()
        .unwrap_or_else(|| panic!("ListTagsForResource({arn}) Tags is not a list: {body:?}"))
        .clone();
    reported
        .iter()
        .map(|entry| {
            (
                entry
                    .get("Key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry
                    .get("Value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Create the bus `name`, failing the test if it was not created.
async fn create_bus(emulator: &Emulator, name: &str) {
    let (status, body) = call(emulator, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "creating bus {name}: expected 200, got {status} {body:?}"
    );
}

/// Create the rule `name` on `bus` (the `default` bus when `bus` is `None`).
async fn create_rule(emulator: &Emulator, bus: Option<&str>, name: &str) {
    let mut request = json!({
        "Name": name,
        "EventPattern": json!({ "source": ["aws.s3"] }).to_string(),
    });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    let (status, body) = call(emulator, "PutRule", &request).await;
    assert_eq!(
        status, 200,
        "creating rule {name}: expected 200, got {status} {body:?}"
    );
}

/// A tag write is reported back by a tag read, for every resource kind this
/// service tags and every ARN form it mints — and tags written to one resource
/// stay on that resource.
#[tokio::test]
async fn a_tag_write_is_reported_back_for_every_taggable_resource_kind() {
    let emulator = emulator();
    create_bus(&emulator, "orders-bus").await;
    create_rule(&emulator, None, "on-default").await;
    create_rule(&emulator, Some("orders-bus"), "on-orders").await;

    // One row per resource kind and ARN form, differing also in how many tags are
    // written and in the form of the values (plain, empty, the modelled maximum).
    let long_value = "v".repeat(TAG_VALUE_MAX);
    let cases: Vec<ResourceCase> = vec![
        (
            "the account's default bus",
            bus_arn("default"),
            vec![("team", "platform".to_string())],
        ),
        (
            "a custom bus",
            bus_arn("orders-bus"),
            vec![
                ("team", "orders".to_string()),
                ("env", "staging".to_string()),
            ],
        ),
        (
            "a rule on the default bus",
            default_bus_rule_arn("on-default"),
            vec![
                ("team", "platform".to_string()),
                ("env", String::new()),
                ("note", long_value.clone()),
            ],
        ),
        (
            "a rule on a custom bus",
            rule_arn("orders-bus", "on-orders"),
            vec![("owner", "invoicing".to_string())],
        ),
    ];

    for (case, arn, tags) in &cases {
        let (status, body) = tag_resource(
            &emulator,
            arn,
            tags.iter()
                .map(|(key, value)| tag(key, value))
                .collect::<Vec<_>>(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: tagging {arn} expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: TagResource answers an empty body, got {body:?}"
        );
    }

    // Every resource reports back exactly its own set, so one write did not reach
    // another record.
    for (case, arn, tags) in &cases {
        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        let actual = stored_tags(&emulator, arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} on {arn}, got {actual:?}"
        );
    }
}

/// A tag write merges into the set the resource already carries: a key the request
/// names takes the value it names, in the position that key already held; a key it
/// does not name keeps the value it had; and a request repeating a key leaves the
/// value it names last.
#[tokio::test]
async fn a_tag_write_merges_with_the_set_already_stored() {
    // Rows differ along what the rule generalises over: how many pairs one request
    // carries, whether a key is new or already stored, the order the writes arrive
    // in, and the value's form.
    let cases: Vec<MergeCase> = vec![
        (
            "one write of one pair",
            vec![vec![("a", "1")]],
            vec![("a", "1")],
        ),
        (
            "two writes of new keys append in write order",
            vec![vec![("a", "1")], vec![("b", "2"), ("c", "3")]],
            vec![("a", "1"), ("b", "2"), ("c", "3")],
        ),
        (
            "a key already stored takes the new value in its own position",
            vec![vec![("a", "1"), ("b", "2")], vec![("a", "9")]],
            vec![("a", "9"), ("b", "2")],
        ),
        (
            "a write naming no stored key leaves the stored ones alone",
            vec![vec![("a", "1"), ("b", "2")], vec![("c", "3")]],
            vec![("a", "1"), ("b", "2"), ("c", "3")],
        ),
        (
            "a request repeating a key leaves the value it names last",
            vec![vec![("a", "1"), ("a", "2"), ("a", "3")]],
            vec![("a", "3")],
        ),
        (
            "an empty tag list changes nothing",
            vec![vec![("a", "1")], vec![]],
            vec![("a", "1")],
        ),
        (
            "a value may be empty, and may be re-set to a non-empty one",
            vec![vec![("a", "")], vec![("a", "1")]],
            vec![("a", "1")],
        ),
    ];

    for (case, writes, expected) in cases {
        let emulator = emulator();
        create_rule(&emulator, None, "merged").await;
        let arn = default_bus_rule_arn("merged");
        for write in &writes {
            let (status, body) = tag_resource(
                &emulator,
                &arn,
                write.iter().map(|(key, value)| tag(key, value)).collect(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: writing {write:?} expected 200, got {status} {body:?}"
            );
        }
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let actual = stored_tags(&emulator, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// `UntagResource` removes exactly the keys the request names, leaving the order of
/// the ones that stay; a key the resource does not carry removes nothing.
#[tokio::test]
async fn removing_tags_leaves_every_key_the_request_did_not_name() {
    let stored = [("a", "1"), ("b", "2"), ("c", "3")];
    // Rows differ in how many keys are removed, whether they are stored at all, and
    // where in the set they sit.
    let cases: Vec<RemovalCase> = vec![
        (
            "no key at all removes nothing",
            vec![],
            vec![("a", "1"), ("b", "2"), ("c", "3")],
        ),
        (
            "the first key",
            vec!["a"],
            vec![("b", "2"), ("c", "3")],
        ),
        (
            "a key in the middle",
            vec!["b"],
            vec![("a", "1"), ("c", "3")],
        ),
        (
            "two of three keys",
            vec!["a", "c"],
            vec![("b", "2")],
        ),
        ("every key", vec!["a", "b", "c"], vec![]),
        (
            "a key the resource does not carry",
            vec!["zz"],
            vec![("a", "1"), ("b", "2"), ("c", "3")],
        ),
        (
            "a stored key beside one that is not stored",
            vec!["zz", "b"],
            vec![("a", "1"), ("c", "3")],
        ),
    ];

    for (case, removed, expected) in cases {
        let emulator = emulator();
        create_bus(&emulator, "untagged-bus").await;
        let arn = bus_arn("untagged-bus");
        let (status, body) = tag_resource(
            &emulator,
            &arn,
            stored.iter().map(|(key, value)| tag(key, value)).collect(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the setup write expected 200, got {status} {body:?}"
        );

        let (status, body) = untag_resource(&emulator, &arn, removed.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing {removed:?} expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: UntagResource answers an empty body, got {body:?}"
        );

        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let actual = stored_tags(&emulator, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A well-formed ARN naming a resource no record carries is the modelled
/// `ResourceNotFoundException` for every tagging operation — never an empty
/// success, and never a resource this call brings into being.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_resource_not_found() {
    let emulator = emulator();
    create_bus(&emulator, "orders-bus").await;
    create_rule(&emulator, None, "on-default").await;

    // Rows differ in which resource kind is missing and in which segment of the ARN
    // is the one that names nothing.
    let cases: Vec<(&str, String)> = vec![
        ("a rule never created on the default bus", default_bus_rule_arn("absent")),
        ("a bus never created", bus_arn("absent-bus")),
        ("a rule on a bus that exists but was never created", rule_arn("orders-bus", "absent")),
        ("a rule on a bus that does not exist", rule_arn("absent-bus", "on-default")),
        // The rule exists on the DEFAULT bus, so the same name under a custom bus
        // names a different resource — one nothing created.
        ("a stored rule name under the wrong bus", rule_arn("orders-bus", "on-default")),
    ];

    for (case, arn) in &cases {
        for (operation, body) in [
            ("TagResource", json!({ "ResourceARN": arn, "Tags": [tag("a", "1")] })),
            ("UntagResource", json!({ "ResourceARN": arn, "TagKeys": ["a"] })),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ] {
            let (status, answered) = call(&emulator, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} on {arn} expected 400, got {status} {answered:?}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: {operation} on {arn} expected ResourceNotFoundException, got {code:?} {answered:?}"
            );
        }
    }

    // Nothing above created a record: the rule that was never created is still not
    // there, and the bus that was never created still is not either.
    let (status, body) = call(&emulator, "DescribeRule", &json!({ "Name": "absent" })).await;
    assert_eq!(
        status, 400,
        "a refused tag write must not create the rule it named, got {status} {body:?}"
    );
    let (status, body) = call(
        &emulator,
        "DescribeEventBus",
        &json!({ "Name": "absent-bus" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a refused tag write must not create the bus it named, got {status} {body:?}"
    );
}

/// Text that is not the ARN of a taggable resource of this service is a BAD
/// REQUEST — `Arn` is modelled as a plain string, so nothing refuses it before the
/// handler runs, and the answer is never a 501 and never a missing resource.
#[tokio::test]
async fn text_naming_no_taggable_resource_of_this_service_is_refused() {
    let emulator = emulator();

    // Rows differ in HOW the text fails to name a taggable EventBridge resource.
    let cases: Vec<(&str, String)> = vec![
        ("not an ARN at all", "on-default".to_string()),
        ("an ARN missing its fields", "arn:aws:events".to_string()),
        ("an ARN with no resource segment", format!("arn:aws:events:{REGION}:{}:", account())),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{REGION}:{}:orders-queue", account()),
        ),
        (
            // "In EventBridge, rules and event buses can be tagged"
            // (`TagResource`): an archive is neither.
            "an EventBridge resource kind that is not taggable",
            format!("arn:aws:events:{REGION}:{}:archive/nightly", account()),
        ),
        (
            "an event-bus ARN naming no bus",
            format!("arn:aws:events:{REGION}:{}:event-bus/", account()),
        ),
        (
            "a rule ARN naming no rule",
            format!("arn:aws:events:{REGION}:{}:rule/", account()),
        ),
    ];

    for (case, arn) in &cases {
        for (operation, body) in [
            ("TagResource", json!({ "ResourceARN": arn, "Tags": [tag("a", "1")] })),
            ("UntagResource", json!({ "ResourceARN": arn, "TagKeys": ["a"] })),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ] {
            let (status, answered) = call(&emulator, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} on {arn:?} expected 400, got {status} {answered:?}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: {operation} on {arn:?} expected ValidationException, got {code:?} {answered:?}"
            );
        }
    }
}

/// The per-resource tag quota is charged on the RESULT of a write: a write that
/// would leave the resource past it is refused whole and changes nothing, while
/// re-setting a key the resource already carries is accepted at the quota.
#[tokio::test]
async fn the_per_resource_tag_quota_is_charged_on_the_merged_set() {
    let emulator = emulator();
    create_rule(&emulator, None, "at-the-quota").await;
    let arn = default_bus_rule_arn("at-the-quota");

    // The quota itself, in one request: accepted.
    let full: Vec<Value> = (0..TAG_QUOTA)
        .map(|index| tag(&format!("k{index}"), &format!("v{index}")))
        .collect();
    let (status, body) = tag_resource(&emulator, &arn, full.clone()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} {body:?}",
        format!("{TAG_QUOTA} tags in one request")
    );
    let actual = stored_tags(&emulator, &arn).await;
    assert_eq!(
        actual.len(),
        TAG_QUOTA,
        "case \"at the quota\": expected {TAG_QUOTA} stored tags, got {}",
        actual.len()
    );

    // One key PAST the quota: refused, and the stored set is exactly what it was.
    let (status, body) = tag_resource(&emulator, &arn, vec![tag("one-too-many", "x")]).await;
    assert_eq!(
        status, 400,
        "case \"one key past the quota\": expected 400, got {status} {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationException",
        "case \"one key past the quota\": expected ValidationException, got {code:?} {body:?}"
    );
    let after = stored_tags(&emulator, &arn).await;
    assert_eq!(
        after, actual,
        "case \"one key past the quota\": a refused write changes nothing, expected {actual:?}, got {after:?}"
    );

    // Re-setting a key the resource already carries does not grow the set, so it is
    // accepted at the quota.
    let (status, body) = tag_resource(&emulator, &arn, vec![tag("k0", "rewritten")]).await;
    assert_eq!(
        status, 200,
        "case \"re-setting a stored key at the quota\": expected 200, got {status} {body:?}"
    );
    let after = stored_tags(&emulator, &arn).await;
    assert_eq!(
        after.len(),
        TAG_QUOTA,
        "case \"re-setting a stored key at the quota\": expected {TAG_QUOTA} tags, got {}",
        after.len()
    );
    assert_eq!(
        after.first().map(|(key, value)| (key.as_str(), value.as_str())),
        Some(("k0", "rewritten")),
        "case \"re-setting a stored key at the quota\": expected k0 rewritten in place, got {after:?}"
    );

    // One request already past the quota, against a resource carrying no tags.
    create_rule(&emulator, None, "fresh").await;
    let fresh = default_bus_rule_arn("fresh");
    let too_many: Vec<Value> = (0..=TAG_QUOTA)
        .map(|index| tag(&format!("k{index}"), "v"))
        .collect();
    let (status, body) = tag_resource(&emulator, &fresh, too_many).await;
    assert_eq!(
        status, 400,
        "case {:?}: expected 400, got {status} {body:?}",
        format!("{} tags in one request", TAG_QUOTA + 1)
    );
    let actual = stored_tags(&emulator, &fresh).await;
    assert!(
        actual.is_empty(),
        "case \"a refused oversized write\": expected no stored tags, got {actual:?}"
    );

    // The same quota is charged on the CREATE paths that name tags, and a refused
    // create stores no resource at all.
    let oversized: Vec<Value> = (0..=TAG_QUOTA)
        .map(|index| tag(&format!("k{index}"), "v"))
        .collect();
    let (status, body) = call(
        &emulator,
        "CreateEventBus",
        &json!({ "Name": "oversized-bus", "Tags": oversized.clone() }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"CreateEventBus past the tag quota\": expected 400, got {status} {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationException",
        "case \"CreateEventBus past the tag quota\": expected ValidationException, got {code:?} {body:?}"
    );
    let (status, body) = call(
        &emulator,
        "DescribeEventBus",
        &json!({ "Name": "oversized-bus" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"CreateEventBus past the tag quota\": the bus must not exist, got {status} {body:?}"
    );

    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({
            "Name": "oversized-rule",
            "EventPattern": json!({ "source": ["aws.s3"] }).to_string(),
            "Tags": oversized,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"PutRule past the tag quota\": expected 400, got {status} {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationException",
        "case \"PutRule past the tag quota\": expected ValidationException, got {code:?} {body:?}"
    );
    let (status, body) = call(
        &emulator,
        "DescribeRule",
        &json!({ "Name": "oversized-rule" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"PutRule past the tag quota\": the rule must not exist, got {status} {body:?}"
    );
}

/// The modelled key and value bounds hold on every published bound and one past
/// it: a value inside them is stored, a value outside is refused as invalid.
#[tokio::test]
async fn the_modelled_tag_key_and_value_bounds_are_enforced() {
    // Rows: each published bound, and one character past it.
    let cases: Vec<(&str, usize, usize, bool)> = vec![
        ("the shortest key", TAG_KEY_MIN, 1, true),
        ("the longest key", TAG_KEY_MAX, 1, true),
        ("one character past the longest key", TAG_KEY_MAX + 1, 1, false),
        ("the shortest value", 3, TAG_VALUE_MIN, true),
        ("the longest value", 3, TAG_VALUE_MAX, true),
        (
            "one character past the longest value",
            3,
            TAG_VALUE_MAX + 1,
            false,
        ),
    ];

    for (case, key_len, value_len, accepted) in cases {
        let emulator = emulator();
        create_rule(&emulator, None, "bounded").await;
        let arn = default_bus_rule_arn("bounded");
        let key = "k".repeat(key_len);
        let value = "v".repeat(value_len);
        let (status, body) = tag_resource(&emulator, &arn, vec![tag(&key, &value)]).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: key of {key_len} and value of {value_len} expected 200, \
                 got {status} {body:?}"
            );
            let actual = stored_tags(&emulator, &arn).await;
            assert_eq!(
                actual,
                vec![(key.clone(), value.clone())],
                "case {case:?}: expected the stored pair back, got {actual:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: key of {key_len} and value of {value_len} expected 400, \
                 got {status} {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected ValidationException, got {code:?} {body:?}"
            );
            let actual = stored_tags(&emulator, &arn).await;
            assert!(
                actual.is_empty(),
                "case {case:?}: a refused write stores nothing, got {actual:?}"
            );
        }
    }
}

/// Tags a CREATE names are stored and reported by the tag read; tags an UPDATE
/// names are ignored, so a stored rule keeps exactly what the tagging operations
/// left it.
#[tokio::test]
async fn tags_a_create_names_are_stored_and_an_update_leaves_them_alone() {
    let emulator = emulator();

    // `CreateEventBus` names tags: they are the bus's tag set.
    let (status, body) = call(
        &emulator,
        "CreateEventBus",
        &json!({
            "Name": "tagged-bus",
            "Tags": [tag("team", "orders"), tag("env", "staging")],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"CreateEventBus with tags\": expected 200, got {status} {body:?}"
    );
    let actual = stored_tags(&emulator, &bus_arn("tagged-bus")).await;
    assert_eq!(
        actual,
        vec![
            ("team".to_string(), "orders".to_string()),
            ("env".to_string(), "staging".to_string()),
        ],
        "case \"CreateEventBus with tags\": expected the created set, got {actual:?}"
    );

    // `PutRule` names tags on the call that CREATES the rule: they are stored.
    let create = json!({
        "Name": "tagged-rule",
        "EventPattern": json!({ "source": ["aws.s3"] }).to_string(),
        "Description": "first",
        "Tags": [tag("team", "platform")],
    });
    let (status, body) = call(&emulator, "PutRule", &create).await;
    assert_eq!(
        status, 200,
        "case \"PutRule create with tags\": expected 200, got {status} {body:?}"
    );
    let arn = default_bus_rule_arn("tagged-rule");
    let actual = stored_tags(&emulator, &arn).await;
    assert_eq!(
        actual,
        vec![("team".to_string(), "platform".to_string())],
        "case \"PutRule create with tags\": expected the created set, got {actual:?}"
    );

    // A tag write adds to it.
    let (status, body) = tag_resource(&emulator, &arn, vec![tag("env", "prod")]).await;
    assert_eq!(
        status, 200,
        "case \"tagging a created rule\": expected 200, got {status} {body:?}"
    );

    // "If you are updating an existing rule, any tags you specify in the PutRule
    // operation are ignored" (`PutRule`): the update replaces the configuration and
    // leaves the tag set exactly as the tagging operations left it.
    let update = json!({
        "Name": "tagged-rule",
        "EventPattern": json!({ "source": ["aws.ec2"] }).to_string(),
        "Description": "second",
        "Tags": [tag("team", "ignored"), tag("new", "ignored")],
    });
    let (status, body) = call(&emulator, "PutRule", &update).await;
    assert_eq!(
        status, 200,
        "case \"PutRule update with tags\": expected 200, got {status} {body:?}"
    );
    let actual = stored_tags(&emulator, &arn).await;
    assert_eq!(
        actual,
        vec![
            ("team".to_string(), "platform".to_string()),
            ("env".to_string(), "prod".to_string()),
        ],
        "case \"PutRule update with tags\": an update ignores the tags it names, got {actual:?}"
    );

    // The update DID replace the configuration, so the tag set surviving is not a
    // rule that was left untouched.
    let (status, described) = call(&emulator, "DescribeRule", &json!({ "Name": "tagged-rule" })).await;
    assert_eq!(
        status, 200,
        "case \"PutRule update with tags\": describing the rule expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described.get("Description").and_then(Value::as_str),
        Some("second"),
        "case \"PutRule update with tags\": expected the updated description, got {described:?}"
    );
}
