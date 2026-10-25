// SPDX-License-Identifier: CC0-1.0

//! The reserved `aws:` tag-key namespace, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! Rule under test: a tag key in the reserved `aws:` namespace (matched without
//! regard to case) is AWS's own, so wherever a request asks this service to
//! create, overwrite or remove such a key it is refused with that operation's
//! modelled `BadRequestException` (400) and the resource's stored tags are left
//! exactly as they were. The tables span what the rule generalises over: the
//! spelling of the prefix (its case, what follows it, where in the key it sits),
//! the operation the key arrives through (`CreateTags`, `DeleteTags`, a create's
//! `Tags`, an update's `Tags`), and the resource kind being created.

use serde_json::{json, Map, Value};

use crate::input_security_group::update_group;
use crate::resource_tags::{create_resource, create_tags, delete_tags, expected, listed_tags, tags};
use crate::wire::{call, error_code, error_message, medialive};

/// Tag keys that lie IN the reserved namespace, so every write or removal of one
/// is refused.
///
/// The rows differ along what the rule generalises over: the case of the prefix,
/// what follows it (a sub-namespace, nothing at all, a non-ASCII character), and
/// the recorded `aws:cloudformation:stack-name` key is one row among them.
const RESERVED_KEYS: [(&str, &str); 6] = [
    ("the recorded CloudFormation system key", "aws:cloudformation:stack-name"),
    ("a short reserved key", "aws:foo"),
    ("the bare prefix", "aws:"),
    ("the prefix in capitals", "AWS:owner"),
    ("the prefix mixed-case", "Aws:Created-By"),
    ("a non-ASCII remainder behind the prefix", "aws:équipe"),
];

/// Tag keys that merely LOOK like the reserved ones and are a caller's to use,
/// so each must be stored.
///
/// The reserved namespace is a prefix of the whole key, so a key that only
/// contains the prefix, stops short of its colon, or starts with another
/// character entirely is outside it.
const ALLOWED_KEYS: [(&str, &str); 6] = [
    ("the prefix without its colon", "aws"),
    ("a word starting with the prefix letters", "awsome:thing"),
    ("the prefix inside the key rather than at its start", "my-aws:key"),
    ("an underscore where the colon would be", "aws_cost-centre"),
    ("a non-ASCII character before the prefix", "áws:key"),
    ("a key unrelated to the prefix", "owner"),
];

/// One create per resource kind whose create carries `Tags`, as `(kind, URI,
/// body without tags)`.
///
/// These are the kinds whose create needs no parent resource; the nested creates
/// (a node, the two monitor templates, a signal map, a partner input) carry the
/// same `Tags` member through the same guard, and the accepted path of all of
/// them is covered by the `resource_tags` suite. Each body carries exactly the
/// members its create requires, as the `resource_tags` suite reads them off the
/// model.
fn creates() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("channel", "/prod/channels", json!({ "name": "reserved-tag" })),
        (
            "input",
            "/prod/inputs",
            json!({
                "name": "reserved-tag",
                "type": "URL_PULL",
                "sources": [{ "url": "https://example.com/a.m3u8" }],
            }),
        ),
        (
            "inputSecurityGroup",
            "/prod/inputSecurityGroups",
            json!({ "whitelistRules": [{ "cidr": "192.0.2.0/24" }] }),
        ),
        (
            "sdiSource",
            "/prod/sdiSources",
            json!({ "name": "reserved-tag", "type": "SINGLE" }),
        ),
        (
            "network",
            "/prod/networks",
            json!({ "name": "reserved-tag", "ipPools": [{ "cidr": "192.0.2.0/24" }] }),
        ),
        (
            "multiplex",
            "/prod/multiplexes",
            json!({
                "name": "reserved-tag",
                "requestId": "reserved-tag-request",
                "availabilityZones": ["us-west-2a", "us-west-2c"],
                "multiplexSettings": {
                    "transportStreamBitrate": 1_000_000,
                    "transportStreamId": 0,
                },
            }),
        ),
        (
            "cluster",
            "/prod/clusters",
            json!({ "name": "reserved-tag", "clusterType": "ON_PREMISES" }),
        ),
        (
            "cloudwatch-alarm-template-group",
            "/prod/cloudwatch-alarm-template-groups",
            json!({ "name": "reserved-tag" }),
        ),
        (
            "eventbridge-rule-template-group",
            "/prod/eventbridge-rule-template-groups",
            json!({ "name": "reserved-tag" }),
        ),
    ]
}

/// The id segment of a MediaLive resource ARN, which is what a resource's own
/// operations are keyed by.
fn arn_id(arn: &str) -> String {
    arn.rsplit(':').next().unwrap_or_default().to_string()
}

/// Assert that `status`/`body` is the modelled refusal, naming `key`.
fn assert_refused(case: &str, key: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the reserved key {key:?} to be refused with 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "BadRequestException",
        "case {case:?}: expected BadRequestException for {key:?}, got {body}"
    );
    let message = error_message(body);
    assert!(
        message.contains(key),
        "case {case:?}: expected the refusal to name the key {key:?}, got {message:?}"
    );
}

/// Rule: `CreateTags` refuses a key in the reserved namespace with
/// `BadRequestException` (400) and stores nothing, while a key that merely looks
/// like one is stored like any other.
#[tokio::test]
async fn create_tags_refuses_the_reserved_namespace() {
    for (case, key) in RESERVED_KEYS {
        let router = medialive();
        let arn = create_resource(&router, "channel", "reserved-create", None).await;
        // A tag that must survive the refusal.
        let (status, body) =
            create_tags(&router, &arn, json!({ "tags": tags(&[("keep", "me")]) })).await;
        assert_eq!(status, 204, "case {case:?}: the fixture tag got {status}: {body}");

        let (status, body) = create_tags(&router, &arn, json!({ "tags": tags(&[(key, "x")]) })).await;
        assert_refused(case, key, status, &body);
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            expected(&[("keep", "me")]),
            "case {case:?}: expected the refused write to leave the stored tags alone"
        );

        // The refusal is of the whole request: a reserved key beside an
        // acceptable one stores neither.
        let (status, body) = create_tags(
            &router,
            &arn,
            json!({ "tags": tags(&[(key, "x"), ("env", "test")]) }),
        )
        .await;
        assert_refused(case, key, status, &body);
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            expected(&[("keep", "me")]),
            "case {case:?}: expected a request carrying one reserved key to store none of its tags"
        );
    }

    // A key outside the namespace is a caller's own, so it is stored and read
    // back through `ListTagsForResource` and through the resource's own describe.
    let router = medialive();
    let arn = create_resource(&router, "channel", "allowed-create", None).await;
    for (case, key) in ALLOWED_KEYS {
        let (status, body) =
            create_tags(&router, &arn, json!({ "tags": tags(&[(key, "value")]) })).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected {key:?} to be accepted, got {status}: {body}"
        );
        let listed = listed_tags(&router, case, &arn).await;
        assert_eq!(
            listed.get(key),
            Some(&json!("value")),
            "case {case:?}: expected {key:?} among the stored tags, got {listed:?}"
        );
    }

    let (status, described) = call(&router, "GET", &format!("/prod/channels/{}", arn_id(&arn)), None).await;
    assert_eq!(
        status, 200,
        "DescribeChannel: expected 200, got {status}: {described}"
    );
    let described_tags = match described.get("tags") {
        Some(Value::Object(map)) => map.clone(),
        other => panic!("DescribeChannel reported no tag map: {other:?}"),
    };
    for (case, key) in ALLOWED_KEYS {
        assert_eq!(
            described_tags.get(key),
            Some(&json!("value")),
            "case {case:?}: expected the channel's own describe to report {key:?}, got {described_tags:?}"
        );
    }
}

/// Rule: a system tag is not the caller's to remove either, so `DeleteTags`
/// refuses a key in the reserved namespace with `BadRequestException` (400) and
/// removes nothing — including the other keys the same request named.
#[tokio::test]
async fn delete_tags_refuses_the_reserved_namespace() {
    for (case, key) in RESERVED_KEYS {
        let router = medialive();
        let arn = create_resource(&router, "input", "reserved-delete", None).await;
        let (status, body) = create_tags(
            &router,
            &arn,
            json!({ "tags": tags(&[("owner", "video"), ("env", "test")]) }),
        )
        .await;
        assert_eq!(status, 204, "case {case:?}: the fixture tags got {status}: {body}");
        let before = listed_tags(&router, case, &arn).await;

        let (status, body) = delete_tags(&router, &arn, &[key]).await;
        assert_refused(case, key, status, &body);
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            before,
            "case {case:?}: expected a refused removal to leave the stored tags alone"
        );

        let (status, body) = delete_tags(&router, &arn, &["owner", key]).await;
        assert_refused(case, key, status, &body);
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            before,
            "case {case:?}: expected a removal naming one reserved key to remove nothing"
        );
    }

    // A key outside the namespace is removed as ever.
    let router = medialive();
    let arn = create_resource(&router, "input", "allowed-delete", None).await;
    let (status, body) = create_tags(&router, &arn, json!({ "tags": tags(&[("aws", "near-miss")]) })).await;
    assert_eq!(status, 204, "the near-miss tag got {status}: {body}");
    let (status, body) = delete_tags(&router, &arn, &["aws"]).await;
    assert_eq!(status, 204, "DeleteTags on a near-miss key got {status}: {body}");
    assert_eq!(
        listed_tags(&router, "near miss", &arn).await,
        Map::new(),
        "expected the near-miss key to have been removed"
    );
}

/// Rule: a create that carries `Tags` is held to the same namespace, so a
/// request asking to be born with a reserved key is refused with
/// `BadRequestException` (400) and creates nothing.
///
/// The rows span the resource kinds whose create carries `Tags` ([`creates`]),
/// because the rule is about the tag key rather than about any one resource
/// type, and the key column is the reserved-spelling table.
#[tokio::test]
async fn a_create_refuses_the_reserved_namespace() {
    for (kind, uri, body) in creates() {
        for (spelling, key) in RESERVED_KEYS {
            let router = medialive();
            let case = format!("{kind} created with {spelling}");
            let mut request = body.clone();
            request["tags"] = tags(&[(key, "x")]);
            let (status, response) = call(&router, "POST", uri, Some(request)).await;
            assert_refused(&case, key, status, &response);
        }

        // The same create, with the same tag key outside the namespace, is
        // accepted — so what the rows above refuse is the namespace and not the
        // request body.
        let router = medialive();
        let case = format!("{kind} created with an acceptable key");
        let mut request = body.clone();
        request["tags"] = tags(&[("owner", "video")]);
        let (status, response) = call(&router, "POST", uri, Some(request)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the create to be accepted, got {status}: {response}"
        );
    }

    // A refused create stores nothing: the channel collection is still empty
    // afterwards.
    let router = medialive();
    let mut request = json!({ "name": "refused" });
    request["tags"] = tags(&[("aws:foo", "bar")]);
    let (status, response) = call(&router, "POST", "/prod/channels", Some(request)).await;
    assert_refused("a refused create stores nothing", "aws:foo", status, &response);
    let (status, listed) = call(&router, "GET", "/prod/channels", None).await;
    assert_eq!(status, 200, "ListChannels: expected 200, got {status}: {listed}");
    let channels = listed["channels"].as_array().cloned().unwrap_or_default();
    assert!(
        channels.is_empty(),
        "expected a refused create to have created no channel, got {channels:?}"
    );
}

/// Rule: an update that carries `Tags` is held to the same namespace, so
/// `UpdateInputSecurityGroup` refuses a reserved key with
/// `BadRequestException` (400) and leaves the group — its tags AND the whitelist
/// the same request asked to change — exactly as it was.
#[tokio::test]
async fn an_update_refuses_the_reserved_namespace() {
    for (case, key) in RESERVED_KEYS {
        let router = medialive();
        let arn = create_resource(&router, "inputSecurityGroup", "reserved-update", None).await;
        let id = arn_id(&arn);
        let (status, body) =
            create_tags(&router, &arn, json!({ "tags": tags(&[("owner", "video")]) })).await;
        assert_eq!(status, 204, "case {case:?}: the fixture tag got {status}: {body}");

        let (status, body) = update_group(
            &router,
            &id,
            json!({
                "tags": tags(&[(key, "x")]),
                "whitelistRules": [{ "cidr": "198.51.100.0/24" }],
            }),
        )
        .await;
        assert_refused(case, key, status, &body);
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            expected(&[("owner", "video")]),
            "case {case:?}: expected the refused update to leave the group's tags alone"
        );

        let (status, described) =
            call(&router, "GET", &format!("/prod/inputSecurityGroups/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInputSecurityGroup: expected 200, got {status}: {described}"
        );
        let rules = described["whitelistRules"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            rules,
            vec![json!({ "cidr": "192.0.2.0/24" })],
            "case {case:?}: expected the refused update to leave the whitelist alone, got {rules:?}"
        );
    }

    // The same update with an acceptable key lands.
    let router = medialive();
    let arn = create_resource(&router, "inputSecurityGroup", "allowed-update", None).await;
    let id = arn_id(&arn);
    let (status, body) = update_group(&router, &id, json!({ "tags": tags(&[("aws", "near-miss")]) })).await;
    assert_eq!(status, 200, "UpdateInputSecurityGroup got {status}: {body}");
    assert_eq!(
        listed_tags(&router, "near miss", &arn).await,
        expected(&[("aws", "near-miss")]),
        "expected the near-miss key to have been written by the update"
    );
}
