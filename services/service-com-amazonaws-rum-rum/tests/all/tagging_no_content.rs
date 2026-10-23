// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ANSWER CloudWatch RUM's tag writes give.
//!
//! Two rules, over the whole class of requests each one covers rather than over one
//! request:
//!
//! 1. A tag write whose modelled output declares no member — `TagResource` and
//!    `UntagResource`, the two such operations of this service — answers HTTP 204 No
//!    Content on success, with nothing in the body, whatever the request wrote. A read of
//!    the same frontend (`ListTagsForResource`) reports a document and so keeps its 200,
//!    which is the other side of the same rule.
//! 2. `UntagResource$TagKeys` targets `TagKeyList`, whose `@length` is `min: 0`: the EMPTY
//!    key list is a modelled value, a query-bound list spells it by binding no value at
//!    all, and such a request is served (204, removing nothing) rather than answered with
//!    an unmodelled status.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request, the `ResourceArn` percent-encoded into the URI the way an SDK
//! encodes a non-greedy `@httpLabel` — so routing, deserialization, the modelled
//! constraints, the handler and the response envelope are all exercised, and each write is
//! read back through an implemented reader so the answer is never the only evidence.
//!
//! The tables span what the rules generalise over: which of the two operations is called,
//! how many tags or keys one request carries (none, one, several, the published maximum,
//! one past it), the key's character classes, the value's script and byte length, whether
//! a key is new to the resource or already on it, whether a named key is carried at all,
//! and — for the empty key list — whether the request carries a query string at all.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    call, create, emulator, get_app_monitor, percent_encoded, Reply, ACCOUNT, REGION,
};

/// The status a successful tag write answers with: 204 No Content.
/// Source: the AWS CloudWatch RUM API reference, which gives `HTTP/1.1 204` as the
/// response syntax of both `TagResource` and `UntagResource`, and the recorded corpus,
/// where every response of either is a 204 with no content.
const NO_CONTENT: u16 = 204;

/// The status a read of the same frontend answers with, since its output carries a
/// document: `ListTagsForResource` reports `ResourceArn` and `Tags`.
/// Source: `smithy_query(query="operation", operation="ListTagsForResource")`.
const OK: u16 = 200;

/// The status a refused tag request answers with, and the wire code RUM's tagging
/// frontend puts on it.
const BAD_REQUEST: u16 = 400;
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// `UntagResource$TagKeys` targets `TagKeyList`, which is `@length(min: 0, max: 50)`: the
/// empty list is a value the model admits and 50 keys is the published maximum.
/// Source: `smithy_query(query="operation", operation="UntagResource")`.
const TAG_KEY_LIST_BOUNDS: (usize, usize) = (0, 50);

/// `TagValue` is `@length(min: 0, max: 256)` with no pattern.
/// Source: `smithy_query(query="shape", shape="TagValue")`.
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// One key per character class the modelled `TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`
/// admits: the ASCII letters, `_`, and every character from `+` (0x2B) to `=` (0x3D) —
/// which is what `+-=` spells inside a character class, so the digits, `,`, `-`, `.`,
/// `/`, `:`, `;`, `<` and `=` are admitted too.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
const TAG_KEY_CLASSES: [&str; 8] = [
    "Letters",
    "under_score",
    "plus+key",
    "comma,key",
    "dash-key",
    "dot.key",
    "path/key",
    "colon:key",
];

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives it.
fn arn_of(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}")
}

/// The URI the tagging operations are bound to: `/tags/{ResourceArn}`.
fn tags_uri(resource_arn: &str) -> String {
    format!("/tags/{}", percent_encoded(resource_arn))
}

/// `(key, value)` pairs as a sorted map.
fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A tag map as the JSON object a request body or a response carries.
fn as_json(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// A JSON tag map as a sorted Rust map. An omitted member reads as the empty map it
/// stands for: restJson1 draws no distinction between an absent map and an empty one.
fn as_map(case: &str, value: &Value) -> BTreeMap<String, String> {
    match value {
        Value::Null => BTreeMap::new(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: tag {key:?} is not a string: {value}")
                        })
                        .to_string(),
                )
            })
            .collect(),
        other => panic!("case {case:?}: expected a tag map, got {other}"),
    }
}

/// Create one app monitor carrying `tags`, and return its ARN.
async fn create_monitor(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    create(
        router,
        case,
        json!({"Name": name, "Domain": "example.com", "Tags": as_json(tags)}),
    )
    .await;
    arn_of(name)
}

/// What one row of the table sends to the tagging frontend.
enum Write {
    /// `POST /tags/{ResourceArn}` with a `Tags` body.
    Tag(BTreeMap<String, String>),
    /// `DELETE /tags/{ResourceArn}` with the key list bound to the query string the way a
    /// `@httpQuery` list binds: one `tagKeys=` value per element — and, for the empty
    /// list, no value at all, so the request carries no query string.
    Untag(Vec<String>),
    /// The empty key list again, in a request that DOES carry a query string: "no
    /// `tagKeys` value" is the empty list whether or not other parameters are present.
    /// `dryRun` is not a member of this input, so the framework ignores it.
    UntagEmptyBesideAnotherParameter,
}

impl Write {
    /// The request this write sends, as `(method, path and query, body)`.
    fn request(&self, resource_arn: &str) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Tag(tags) => (
                "POST",
                tags_uri(resource_arn),
                Some(json!({"Tags": as_json(tags)})),
            ),
            Self::Untag(keys) => {
                let query = keys
                    .iter()
                    .map(|key| format!("tagKeys={}", percent_encoded(key)))
                    .collect::<Vec<_>>()
                    .join("&");
                let uri = if query.is_empty() {
                    tags_uri(resource_arn)
                } else {
                    format!("{}?{query}", tags_uri(resource_arn))
                };
                ("DELETE", uri, None)
            }
            Self::UntagEmptyBesideAnotherParameter => (
                "DELETE",
                format!("{}?dryRun=false", tags_uri(resource_arn)),
                None,
            ),
        }
    }

    /// The operation this write calls, for an assertion message.
    fn operation(&self) -> &'static str {
        match self {
            Self::Tag(_) => "TagResource",
            Self::Untag(_) | Self::UntagEmptyBesideAnotherParameter => "UntagResource",
        }
    }
}

/// One row of the tag-write table.
struct WriteCase {
    case: &'static str,
    monitor: &'static str,
    /// The tags the monitor is created with.
    created_with: BTreeMap<String, String>,
    write: Write,
    /// The tag map the resource must hold once the write has been applied.
    expected: BTreeMap<String, String>,
}

/// The tag-write table. Rows differ along what the rules generalise over: the operation,
/// the number of tags or keys the request carries (none, one, several, the published
/// maximum), the key's character classes, the value's script and byte length, whether a
/// key is new or already on the resource, whether a named key is carried at all, and
/// whether the empty key list travels with a query string or without one.
fn write_cases() -> Vec<WriteCase> {
    let unicode_value = "équipe-naïve-日本語-🙂-Ωμέγα";
    let max_value = "v".repeat(MAX_TAG_VALUE_LENGTH);
    let class_tags: BTreeMap<String, String> = TAG_KEY_CLASSES
        .iter()
        .enumerate()
        .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
        .collect();
    let max_keys: BTreeMap<String, String> = (0..TAG_KEY_LIST_BOUNDS.1)
        .map(|index| (format!("key{index:02}"), format!("value-{index}")))
        .collect();

    vec![
        WriteCase {
            case: "TagResource, one ASCII tag onto an untagged monitor",
            monitor: "nc-tag-one",
            created_with: BTreeMap::new(),
            write: Write::Tag(map_of(&[("Team", "web")])),
            expected: map_of(&[("Team", "web")]),
        },
        WriteCase {
            case: "TagResource, a value in several non-ASCII scripts",
            monitor: "nc-tag-unicode",
            created_with: BTreeMap::new(),
            write: Write::Tag(map_of(&[("Owner", unicode_value)])),
            expected: map_of(&[("Owner", unicode_value)]),
        },
        WriteCase {
            case: "TagResource, the empty string as a value",
            monitor: "nc-tag-empty-value",
            created_with: map_of(&[("Keep", "kept")]),
            write: Write::Tag(map_of(&[("Blank", "")])),
            expected: map_of(&[("Keep", "kept"), ("Blank", "")]),
        },
        WriteCase {
            case: "TagResource, a value at TagValue's 256-character maximum",
            monitor: "nc-tag-max-value",
            created_with: BTreeMap::new(),
            write: Write::Tag(map_of(&[("Long", max_value.as_str())])),
            expected: map_of(&[("Long", max_value.as_str())]),
        },
        WriteCase {
            case: "TagResource, a key already on the resource (a replacement, not an add)",
            monitor: "nc-tag-replace",
            created_with: map_of(&[("Env", "dev"), ("Keep", "yes")]),
            write: Write::Tag(map_of(&[("Env", "prod")])),
            expected: map_of(&[("Env", "prod"), ("Keep", "yes")]),
        },
        WriteCase {
            case: "TagResource, one key per character class TagKey admits",
            monitor: "nc-tag-key-classes",
            created_with: BTreeMap::new(),
            write: Write::Tag(class_tags.clone()),
            expected: class_tags.clone(),
        },
        WriteCase {
            case: "TagResource, the documented fifty-tag maximum in one call",
            monitor: "nc-tag-max-keys",
            created_with: BTreeMap::new(),
            write: Write::Tag(max_keys.clone()),
            expected: max_keys,
        },
        WriteCase {
            case: "UntagResource, one present key",
            monitor: "nc-untag-one",
            created_with: map_of(&[("Env", "dev"), ("Team", "web")]),
            write: Write::Untag(vec!["Env".to_string()]),
            expected: map_of(&[("Team", "web")]),
        },
        WriteCase {
            case: "UntagResource, several present keys",
            monitor: "nc-untag-several",
            created_with: map_of(&[("A", "1"), ("B", "2"), ("C", "3")]),
            write: Write::Untag(vec!["A".to_string(), "C".to_string()]),
            expected: map_of(&[("B", "2")]),
        },
        WriteCase {
            case: "UntagResource, every key the resource holds",
            monitor: "nc-untag-all",
            created_with: map_of(&[("A", "1"), ("B", "2")]),
            write: Write::Untag(vec!["A".to_string(), "B".to_string()]),
            expected: BTreeMap::new(),
        },
        WriteCase {
            case: "UntagResource, a key the resource does not hold",
            monitor: "nc-untag-absent",
            created_with: map_of(&[("Env", "dev")]),
            write: Write::Untag(vec!["never-set".to_string()]),
            expected: map_of(&[("Env", "dev")]),
        },
        WriteCase {
            case: "UntagResource, one key per character class TagKey admits",
            monitor: "nc-untag-key-classes",
            created_with: class_tags.clone(),
            write: Write::Untag(TAG_KEY_CLASSES.iter().map(|key| (*key).to_string()).collect()),
            expected: BTreeMap::new(),
        },
        WriteCase {
            case: "UntagResource, the EMPTY key list: no tagKeys value and no query string",
            monitor: "nc-untag-empty",
            created_with: map_of(&[("Env", "dev"), ("Team", "web")]),
            write: Write::Untag(Vec::new()),
            expected: map_of(&[("Env", "dev"), ("Team", "web")]),
        },
        WriteCase {
            case: "UntagResource, the EMPTY key list beside another query parameter",
            monitor: "nc-untag-empty-with-query",
            created_with: map_of(&[("Env", "dev")]),
            write: Write::UntagEmptyBesideAnotherParameter,
            expected: map_of(&[("Env", "dev")]),
        },
        WriteCase {
            case: "UntagResource, the EMPTY key list on a resource that carries no tags",
            monitor: "nc-untag-empty-untagged",
            created_with: BTreeMap::new(),
            write: Write::Untag(Vec::new()),
            expected: BTreeMap::new(),
        },
    ]
}

/// `ListTagsForResource`'s answer for `resource_arn`, asserted to be the 200 its document
/// output needs — the reader that proves what the write left behind.
async fn listed_tags(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let reply = call(router, "GET", &tags_uri(resource_arn), None).await;
    assert_eq!(
        reply.status,
        OK,
        "case {case:?}: expected ListTagsForResource to answer {OK}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    as_map(case, &reply.json(case)["Tags"])
}

/// What the answer to a tag write carries, as text: `TagResourceResponse` and
/// `UntagResourceResponse` model no member, so a 204 reports nothing — an empty body, or
/// the empty JSON object the framework renders a member-less output as. Either way the
/// write tells the caller nothing it did not already know.
fn reported_content(reply: &Reply) -> String {
    String::from_utf8_lossy(&reply.body).trim().to_string()
}

fn assert_no_content(case: &str, operation: &str, reply: &Reply) {
    assert_eq!(
        reply.status,
        NO_CONTENT,
        "case {case:?}: expected {operation} to answer {NO_CONTENT}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let content = reported_content(reply);
    assert!(
        content.is_empty() || content == "{}",
        "case {case:?}: expected {operation} to answer with no content, got {content:?}"
    );
}

#[tokio::test]
async fn every_tag_write_answers_204_no_content_and_leaves_the_tags_it_wrote() {
    let (router, _engine) = emulator();
    for row in write_cases() {
        let WriteCase {
            case,
            monitor,
            created_with,
            write,
            expected,
        } = row;
        let arn = create_monitor(&router, case, monitor, &created_with).await;

        let (method, path_and_query, body) = write.request(&arn);
        let reply = call(&router, method, &path_and_query, body.clone()).await;
        assert_no_content(case, write.operation(), &reply);

        let listed = listed_tags(&router, case, &arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the resource to hold {expected:?}, got {listed:?}"
        );
        // The same stored map through the second implemented reader, so the write is
        // asserted on persisted state rather than on the write's own (empty) answer.
        let reported = as_map(case, &get_app_monitor(&router, case, monitor).await["Tags"]);
        assert_eq!(
            reported, expected,
            "case {case:?}: GetAppMonitor must report the same map {expected:?}, got {reported:?}"
        );

        // Repeating the write answers the same way: both operations are `@idempotent`.
        let (method, path_and_query, body) = write.request(&arn);
        let again = call(&router, method, &path_and_query, body).await;
        assert_no_content(case, write.operation(), &again);
        let listed_again = listed_tags(&router, case, &arn).await;
        assert_eq!(
            listed_again, expected,
            "case {case:?}: repeating the write must leave {expected:?}, got {listed_again:?}"
        );
    }
}

#[tokio::test]
async fn the_modelled_tag_key_list_bounds_hold_at_each_bound_and_one_past_the_maximum() {
    // `TagKeyList` is `@length(min: 0, max: 50)`, so the published bounds are 0 keys and
    // 50 keys; one past the maximum is 51. There is no "one below the minimum": a request
    // cannot bind fewer values than none.
    let (router, _engine) = emulator();
    let (min_keys, max_keys) = TAG_KEY_LIST_BOUNDS;

    let carried: BTreeMap<String, String> = (0..max_keys)
        .map(|index| (format!("key{index:02}"), format!("value-{index}")))
        .collect();

    for (case, count) in [
        ("the minimum: no keys at all", min_keys),
        ("one key", 1),
        ("the maximum: fifty keys", max_keys),
    ] {
        let monitor = format!("nc-bound-{count}");
        let arn = create_monitor(&router, case, &monitor, &carried).await;
        let keys: Vec<String> = carried.keys().take(count).cloned().collect();
        let (method, path_and_query, body) = Write::Untag(keys.clone()).request(&arn);
        let reply = call(&router, method, &path_and_query, body).await;
        assert_no_content(case, "UntagResource", &reply);

        let expected: BTreeMap<String, String> = carried
            .iter()
            .filter(|(key, _)| !keys.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let listed = listed_tags(&router, case, &arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: removing {count} of the resource's {max_keys} keys must leave \
             {} of them, got {}",
            expected.len(),
            listed.len()
        );
    }

    // One past the maximum: refused on the modelled bound, and nothing is removed.
    let case = "one past the maximum: fifty-one keys";
    let arn = create_monitor(&router, case, "nc-bound-past", &carried).await;
    let too_many: Vec<String> = (0..=max_keys).map(|index| format!("key{index:02}")).collect();
    let (method, path_and_query, body) = Write::Untag(too_many).request(&arn);
    let reply = call(&router, method, &path_and_query, body).await;
    assert_eq!(
        reply.status,
        BAD_REQUEST,
        "case {case:?}: expected HTTP {BAD_REQUEST}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        BAD_REQUEST_CODE,
        "case {case:?}: expected {BAD_REQUEST_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let listed = listed_tags(&router, case, &arn).await;
    assert_eq!(
        listed, carried,
        "case {case:?}: a refused removal must remove nothing, got {} of {max_keys} keys",
        listed.len()
    );
}

#[tokio::test]
async fn the_empty_key_list_reaches_the_operation_rather_than_an_unmodelled_answer() {
    // A `DELETE /tags/{ResourceArn}` that binds no `tagKeys` value is the same operation
    // as one that binds keys: it is served by `UntagResource`, so it answers that
    // operation's own refusals for a bad request rather than a routing failure. Each row
    // is a `ResourceArn` that fails `Arn`'s modelled pattern
    // `arn:[^:]*:[^:]*:[^:]*:[^:]*:.*` in a different way, and each one is refused with
    // the 400 the tagging frontend words a bad request with — never the 501 an unrouted
    // request would answer.
    let (router, _engine) = emulator();
    for (case, resource_arn) in [
        ("no ARN syntax at all", "not-an-arn-at-all"),
        ("too few fields", "arn:aws:rum"),
        ("the account field missing", "arn:aws:rum:us-east-1"),
        ("the wrong prefix", "urn:aws:rum:us-east-1:000000000000:appmonitor/x"),
    ] {
        let (method, path_and_query, body) = Write::Untag(Vec::new()).request(resource_arn);
        let reply = call(&router, method, &path_and_query, body).await;
        assert_eq!(
            reply.status,
            BAD_REQUEST,
            "case {case:?}: expected HTTP {BAD_REQUEST} for {resource_arn:?}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            BAD_REQUEST_CODE,
            "case {case:?}: expected {BAD_REQUEST_CODE}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }

    // And an ARN that is well-formed but names no resource is the operation's modelled
    // 404, with or without keys — the empty list changes nothing about which resource the
    // request addresses.
    let unknown = arn_of("nc-no-such-monitor");
    for (case, write) in [
        ("the empty key list", Write::Untag(Vec::new())),
        ("one key", Write::Untag(vec!["Env".to_string()])),
    ] {
        let (method, path_and_query, body) = write.request(&unknown);
        let reply = call(&router, method, &path_and_query, body).await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected HTTP 404 for an ARN naming no monitor, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

#[tokio::test]
async fn a_read_of_the_same_frontend_still_answers_200_with_its_document() {
    // The 204 is the answer of a write whose output declares NO member; the frontend's
    // read declares `ResourceArn` and `Tags`, so it answers 200 with that document. The
    // rule is about the output shape, not about the URI.
    let (router, _engine) = emulator();
    let case = "a read beside the writes";
    let tags = map_of(&[("Env", "dev"), ("Team", "web")]);
    let arn = create_monitor(&router, case, "nc-read", &tags).await;

    let reply = call(&router, "GET", &tags_uri(&arn), None).await;
    assert_eq!(
        reply.status,
        OK,
        "case {case:?}: expected ListTagsForResource to answer {OK}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let document = reply.json(case);
    assert_eq!(
        document["ResourceArn"].as_str(),
        Some(arn.as_str()),
        "case {case:?}: the read must name the resource it was asked about, got {:?}",
        document["ResourceArn"]
    );
    assert_eq!(
        as_map(case, &document["Tags"]),
        tags,
        "case {case:?}: the read must report the stored map {tags:?}, got {:?}",
        document["Tags"]
    );
}
