// SPDX-License-Identifier: CC0-1.0

//! The MediaLive tag operations (`CreateTags`, `ListTagsForResource`,
//! `DeleteTags`), end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded `tags-lifecycle-roundtrip` case is ONE row
//! of each. The column that matters most is the RESOURCE KIND, because the rule
//! these operations follow is about the resource an ARN names, not about any one
//! resource type: a tag is stored on that resource, so every kind this emulator
//! holds is a row.

use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_RESOURCE_TAGS, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::percent_encoded;
use crate::wire::{call, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The URI all three tag operations are bound to, from
/// `smithy_query(query="operation", operation="CreateTags,DeleteTags,ListTagsForResource")`:
/// `POST`/`DELETE`/`GET /prod/tags/{ResourceArn}`.
const TAGS: &str = "/prod/tags";

/// Every resource kind this emulator stores, by the segment its ARN carries.
///
/// These are the resources whose create operations are implemented, which is
/// exactly the set an ARN can name here; each is created by
/// [`create_resource`]. The rule under test holds for a resource, so the rows of
/// every table below span this list rather than one chosen type.
const KINDS: [&str; 8] = [
    "channel",
    "input",
    "inputSecurityGroup",
    "sdiSource",
    "signal-map",
    "multiplex",
    "cloudwatch-alarm-template-group",
    "eventbridge-rule-template-group",
];

/// A case name as a resource name: `Name` targets `__stringMin1Max255PatternS`
/// on several of these resources, whose pattern admits no whitespace.
fn slug(case: &str) -> String {
    case.replace(' ', "-")
}

/// The tag-collection URI of one resource. `ResourceArn` is an `@httpLabel`, so
/// the ARN's `:` separators travel percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("{TAGS}/{}", percent_encoded(arn))
}

/// `CreateTags` on `arn` with `tags` as its body.
async fn create_tags(router: &MultiServiceRouter, arn: &str, body: Value) -> (u16, Value) {
    call(router, "POST", &tags_uri(arn), Some(body)).await
}

/// `ListTagsForResource` on `arn`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &tags_uri(arn), None).await
}

/// `DeleteTags` on `arn`. `TagKeys` is an `@httpQuery` list, so each key is one
/// repetition of the `tagKeys` parameter.
async fn delete_tags(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encoded(key)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = if query.is_empty() {
        tags_uri(arn)
    } else {
        format!("{}?{query}", tags_uri(arn))
    };
    call(router, "DELETE", &uri, None).await
}

/// The tag set a response reports, as a map.
///
/// An EMPTY tag set and an absent `Tags` member both mean "this resource carries
/// no tags", and which of the two the restJson1 codec writes for an empty map is
/// the serializer's choice, so both read here as the empty map.
fn tag_map(case: &str, body: &Value) -> Map<String, Value> {
    match body.get("tags") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(tags)) => tags.clone(),
        Some(other) => panic!("case {case:?}: expected a tag map, got {other:?}"),
    }
}

/// The tags `ListTagsForResource` reports for `arn`, with the read itself
/// asserted to have succeeded.
async fn listed_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Map<String, Value> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}): expected 200, got {status}: {body}"
    );
    tag_map(case, &body)
}

/// A `Tags` map as a request body carries it.
fn tags(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// The same pairs as the map a response must report.
fn expected(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), json!(value)))
        .collect()
}

/// Create one resource of `kind`, optionally carrying `tags` from birth, and
/// answer its ARN.
///
/// Each arm is the kind's own implemented create, so what the tag tests address
/// is a resource this store really holds. A signal map needs an existing
/// resource to discover from, so its arm creates the channel it maps.
async fn create_resource(
    router: &MultiServiceRouter,
    kind: &str,
    label: &str,
    tags: Option<Value>,
) -> String {
    let name = slug(&format!("{kind}-{label}"));
    let with_tags = |mut body: Value| {
        if let Some(tags) = tags.clone() {
            body["tags"] = tags;
        }
        body
    };

    let (uri, body, status, arn) = match kind {
        "channel" => (
            "/prod/channels".to_string(),
            with_tags(json!({ "name": name })),
            201,
            vec!["channel", "arn"],
        ),
        "input" => (
            "/prod/inputs".to_string(),
            with_tags(json!({
                "name": name,
                "type": "URL_PULL",
                "sources": [{ "url": "https://example.com/a.m3u8" }],
            })),
            201,
            vec!["input", "arn"],
        ),
        "inputSecurityGroup" => (
            "/prod/inputSecurityGroups".to_string(),
            with_tags(json!({ "whitelistRules": [{ "cidr": "192.0.2.0/24" }] })),
            201,
            vec!["securityGroup", "arn"],
        ),
        "sdiSource" => (
            "/prod/sdiSources".to_string(),
            with_tags(json!({ "name": name, "type": "SINGLE" })),
            201,
            vec!["sdiSource", "arn"],
        ),
        "signal-map" => {
            // A signal map discovers its graph from an existing resource, so its
            // entry point is a channel created right here — inline rather than
            // through this function, which an `async fn` may not call recursively.
            let (status, response) = call(
                router,
                "POST",
                "/prod/channels",
                Some(json!({ "name": format!("{name}-entry") })),
            )
            .await;
            assert_eq!(
                status, 201,
                "kind {kind:?} ({label}): the entry-point channel got {status}: {response}"
            );
            let entry = response["channel"]["arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("kind {kind:?} ({label}): the entry-point channel has no ARN: {response}")
                })
                .to_string();
            (
                "/prod/signal-maps".to_string(),
                with_tags(json!({ "name": name, "discoveryEntryPointArn": entry })),
                201,
                vec!["arn"],
            )
        }
        "multiplex" => (
            "/prod/multiplexes".to_string(),
            // `AvailabilityZones` and `MultiplexSettings` are required, with the
            // two zones the member documents and the low end of each ranged
            // setting's published range (see the multiplex suite).
            with_tags(json!({
                "name": name,
                // `RequestId` is required by the model for a multiplex create.
                "requestId": format!("{name}-request"),
                "availabilityZones": ["us-west-2a", "us-west-2c"],
                "multiplexSettings": {
                    "transportStreamBitrate": 1_000_000,
                    "transportStreamId": 0,
                },
            })),
            201,
            vec!["multiplex", "arn"],
        ),
        "cloudwatch-alarm-template-group" => (
            "/prod/cloudwatch-alarm-template-groups".to_string(),
            with_tags(json!({ "name": name })),
            201,
            vec!["arn"],
        ),
        "eventbridge-rule-template-group" => (
            "/prod/eventbridge-rule-template-groups".to_string(),
            with_tags(json!({ "name": name })),
            201,
            vec!["arn"],
        ),
        other => panic!("no create is wired up for resource kind {other:?}"),
    };

    let (got, response) = call(router, "POST", &uri, Some(body)).await;
    assert_eq!(
        got, status,
        "kind {kind:?} ({label}): create expected {status}, got {got}: {response}"
    );
    let reported = arn
        .iter()
        .fold(&response, |value, member| &value[*member])
        .as_str()
        .unwrap_or_else(|| {
            panic!("kind {kind:?} ({label}): create reported no ARN at {arn:?}: {response}")
        })
        .to_string();
    assert!(
        reported.starts_with(&format!("arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:")),
        "kind {kind:?} ({label}): expected a MediaLive ARN, got {reported:?}"
    );
    reported
}

/// The id segment of a MediaLive resource ARN: everything after the resource
/// type, which is what the resource's own reader is keyed by.
fn arn_id(arn: &str) -> String {
    arn.rsplit(':').next().unwrap_or_default().to_string()
}

/// Rule: a tag is stored on the resource its `ResourceArn` names — `CreateTags`
/// adds the keys it carries and overwrites the values of keys the resource
/// already has, leaving every other key alone; `ListTagsForResource` reports that
/// stored set; `DeleteTags` removes exactly the keys it names.
///
/// The rows span every resource kind this emulator stores ([`KINDS`]), because
/// the rule is about the resource an ARN names rather than any one type, and each
/// row walks the whole lifecycle: nothing, added, overwritten, partly deleted,
/// emptied.
#[tokio::test]
async fn tags_live_on_the_resource_their_arn_names() {
    for kind in KINDS {
        let router = medialive();
        let case = format!("{kind} lifecycle");
        let arn = create_resource(&router, kind, "lifecycle", None).await;

        // A resource created without tags carries none.
        assert_eq!(
            listed_tags(&router, &case, &arn).await,
            Map::new(),
            "case {case:?}: expected a resource created without tags to carry none"
        );

        // CreateTags adds the keys it carries, and answers the modelled 204 with
        // no body of its own.
        let (status, body) = create_tags(
            &router,
            &arn,
            json!({ "tags": tags(&[("owner", "video"), ("env", "test")]) }),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: CreateTags: expected 204, got {status}: {body}"
        );
        assert_eq!(
            listed_tags(&router, &case, &arn).await,
            expected(&[("owner", "video"), ("env", "test")]),
            "case {case:?}: expected the two created tags"
        );

        // A second CreateTags overwrites the keys it names and adds the ones it
        // introduces; the key it does not name keeps its value.
        let (status, body) = create_tags(
            &router,
            &arn,
            json!({ "tags": tags(&[("env", "prod"), ("cost-centre", "1234")]) }),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: CreateTags (overwrite): expected 204, got {status}: {body}"
        );
        assert_eq!(
            listed_tags(&router, &case, &arn).await,
            expected(&[("owner", "video"), ("env", "prod"), ("cost-centre", "1234")]),
            "case {case:?}: expected the overwritten value and the untouched key"
        );

        // A CreateTags carrying no tags at all changes nothing.
        let (status, body) = create_tags(&router, &arn, json!({})).await;
        assert_eq!(
            status, 204,
            "case {case:?}: CreateTags (no tags): expected 204, got {status}: {body}"
        );
        assert_eq!(
            listed_tags(&router, &case, &arn).await,
            expected(&[("owner", "video"), ("env", "prod"), ("cost-centre", "1234")]),
            "case {case:?}: expected a tagless CreateTags to change nothing"
        );

        // DeleteTags removes exactly the keys it names; a key the resource does
        // not carry is already in the state the caller asked for.
        let (status, body) = delete_tags(&router, &arn, &["env", "no-such-key"]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: DeleteTags: expected 204, got {status}: {body}"
        );
        assert_eq!(
            listed_tags(&router, &case, &arn).await,
            expected(&[("owner", "video"), ("cost-centre", "1234")]),
            "case {case:?}: expected only the named key to be removed"
        );

        // And the resource can be emptied of tags again.
        let (status, body) = delete_tags(&router, &arn, &["owner", "cost-centre"]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: DeleteTags (rest): expected 204, got {status}: {body}"
        );
        assert_eq!(
            listed_tags(&router, &case, &arn).await,
            Map::new(),
            "case {case:?}: expected every tag to be gone"
        );
    }
}

/// One row of the verbatim table: the case name, the resource kind it runs on,
/// and the tag set written as key/value pairs.
type VerbatimRow<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

/// Rule: the tag set is stored and reported verbatim, whatever its size, count or
/// character content — `ListTagsForResource` answers with the keys and values that
/// were written, not with anything derived from the reading request.
///
/// The rows differ along what the rule generalises over: the number of tags, the
/// length of a key and a value, non-ASCII text, an empty value and an empty tag
/// set. The recorded case is the single-tag row.
#[tokio::test]
async fn a_stored_tag_set_is_reported_verbatim() {
    let many: Vec<(String, String)> = (0..20)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let many_pairs: Vec<(&str, &str)> = many
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    // The published maxima of a tag key and value (see the bounds suite below),
    // which a verbatim read must carry unchanged.
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);

    // (case, resource kind the row runs on, the tag set written)
    //
    // The kind is not what this rule varies over, so each row names one
    // explicitly and the rows walk [`KINDS`] round-robin: no single resource type
    // is what makes the assertions pass.
    let rows: Vec<VerbatimRow<'_>> = vec![
        ("no tags at all", KINDS[0], vec![]),
        ("one tag", KINDS[1], vec![("owner", "video")]),
        (
            "three tags",
            KINDS[2],
            vec![("owner", "video"), ("env", "test"), ("cost-centre", "1234")],
        ),
        ("twenty tags", KINDS[3], many_pairs),
        (
            "a long key and a long value",
            KINDS[4],
            vec![(long_key.as_str(), long_value.as_str())],
        ),        (
            "non-ascii key and value",
            KINDS[5],
            vec![("所有者", "ライブ"), ("ключ", "значение")],
        ),
        ("an empty value", KINDS[6], vec![("placeholder", "")]),
        (
            "values that need escaping",
            KINDS[0],
            vec![("path", "a/b c:d"), ("json", "{\"nested\": true}")],
        ),
    ];

    for (case, kind, pairs) in rows {
        let router = medialive();
        let arn = create_resource(&router, kind, "verbatim", None).await;

        let (status, body) = create_tags(&router, &arn, json!({ "tags": tags(&pairs) })).await;
        assert_eq!(
            status, 204,
            "case {case:?} on {kind}: CreateTags: expected 204, got {status}: {body}"
        );
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            expected(&pairs),
            "case {case:?} on {kind}: expected the tag set that was written"
        );

        // A row that wrote tags must have them REPORTED, not merely absent: the
        // read carries a `tags` object.
        if !pairs.is_empty() {
            let (status, body) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} on {kind}: ListTagsForResource: expected 200, got {status}: {body}"
            );
            assert!(
                body.get("tags").is_some_and(Value::is_object),
                "case {case:?} on {kind}: expected a reported tags object, got {body}"
            );
        }

        // Reading does not change what the next reader sees.
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            expected(&pairs),
            "case {case:?} on {kind}: expected a second read to report the same tags"
        );
    }
}

/// Rule: the tags a create request supplied are the resource's tags, so the tag
/// operations and the resource's own reader see ONE set — a tag written by
/// `CreateTags` shows up in the resource's description, and a tag the create
/// supplied shows up in `ListTagsForResource`.
///
/// The rows are the resources whose own output shape carries a `Tags` member, and
/// the reader of each is its implemented describe/get.
#[tokio::test]
async fn the_resource_and_the_tag_operations_report_one_tag_set() {
    // (kind, the reader's URI prefix, the reader's response path to `Tags`)
    let rows: [(&str, &str, &[&str]); 4] = [
        ("channel", "/prod/channels", &["tags"]),
        ("input", "/prod/inputs", &["tags"]),
        (
            "cloudwatch-alarm-template-group",
            "/prod/cloudwatch-alarm-template-groups",
            &["tags"],
        ),
        ("signal-map", "/prod/signal-maps", &["tags"]),
    ];

    for (kind, prefix, path) in rows {
        let router = medialive();
        let case = format!("{kind} single tag set");
        // Born with one tag, which the tag reader must already see.
        let arn = create_resource(
            &router,
            kind,
            "shared",
            Some(tags(&[("born", "at-create")])),
        )
        .await;
        assert_eq!(
            listed_tags(&router, &case, &arn).await,
            expected(&[("born", "at-create")]),
            "case {case:?}: expected the create's own tags to be listed"
        );

        // A tag written through CreateTags joins them.
        let (status, body) = create_tags(
            &router,
            &arn,
            json!({ "tags": tags(&[("added", "by-create-tags")]) }),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: CreateTags: expected 204, got {status}: {body}"
        );

        // The resource's OWN reader reports both: there is one stored map, not a
        // tag store beside the resource.
        let id = arn_id(&arn);
        let (status, described) =
            call(&router, "GET", &format!("{prefix}/{}", percent_encoded(&id)), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading {prefix}/{id}: expected 200, got {status}: {described}"
        );
        let reported = path
            .iter()
            .fold(&described, |value, member| &value[*member]);
        assert_eq!(
            reported,
            &Value::Object(expected(&[("born", "at-create"), ("added", "by-create-tags")])),
            "case {case:?}: expected the resource's own reader to report both tags, got {reported:?}"
        );

        // And removing one removes it from the resource too.
        let (status, body) = delete_tags(&router, &arn, &["born"]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: DeleteTags: expected 204, got {status}: {body}"
        );
        let (status, described) =
            call(&router, "GET", &format!("{prefix}/{}", percent_encoded(&id)), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-reading {prefix}/{id}: expected 200, got {status}: {described}"
        );
        let reported = path
            .iter()
            .fold(&described, |value, member| &value[*member]);
        assert_eq!(
            reported,
            &Value::Object(expected(&[("added", "by-create-tags")])),
            "case {case:?}: expected the deleted tag to be gone from the resource, got {reported:?}"
        );
    }
}

/// Rule: a `ResourceArn` that is not an ARN at all is a bad request
/// (`BadRequestException`, 400), a well-formed ARN naming no stored resource is
/// a missing one (`NotFoundException`, 404), and neither stores or removes
/// anything.
///
/// The rows differ along how the ARN fails: its syntax (too few segments, empty
/// partition, not an ARN at all) and its referent (a resource type this store
/// holds but with an id nothing has, a type it does not hold at all, another
/// service's ARN, another account's, another region's).
#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused() {
    let router = medialive();
    // One real resource, to prove a refusal leaves the store alone.
    let bystander = create_resource(&router, "channel", "bystander", None).await;
    let (status, body) = create_tags(
        &router,
        &bystander,
        json!({ "tags": tags(&[("keep", "me")]) }),
    )
    .await;
    assert_eq!(status, 204, "the bystander's tag got {status}: {body}");

    // (case, ResourceArn, expected status, expected error code)
    let rows: [(&str, &str, u16, &str); 10] = [
        ("not an ARN at all", "just-a-name", 400, "BadRequestException"),
        ("too few segments", "arn:aws:medialive:us-west-2:channel", 400, "BadRequestException"),
        (
            "empty partition",
            "arn::medialive:us-west-2:000000000000:channel:1234567",
            400,
            "BadRequestException",
        ),
        (
            "empty resource",
            "arn:aws:medialive:us-west-2:000000000000:",
            400,
            "BadRequestException",
        ),
        (
            "a channel id nothing has",
            "arn:aws:medialive:us-west-2:000000000000:channel:9999999",
            404,
            "NotFoundException",
        ),
        (
            "a resource type this store does not hold",
            "arn:aws:medialive:us-west-2:000000000000:cluster:1234567",
            404,
            "NotFoundException",
        ),
        (
            "a multiplex id nothing has",
            "arn:aws:medialive:us-west-2:000000000000:multiplex:1234567",
            404,
            "NotFoundException",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket",
            404,
            "NotFoundException",
        ),
        (
            "another account's channel",
            "arn:aws:medialive:us-west-2:111122223333:channel:1234567",
            404,
            "NotFoundException",
        ),
        (
            "another region's channel",
            "arn:aws:medialive:eu-west-1:000000000000:channel:1234567",
            404,
            "NotFoundException",
        ),
    ];

    for (case, arn, status, code) in rows {
        for (operation, (got, body)) in [
            ("CreateTags", create_tags(&router, arn, json!({ "tags": tags(&[("a", "b")]) })).await),
            ("ListTagsForResource", list_tags(&router, arn).await),
            ("DeleteTags", delete_tags(&router, arn, &["a"]).await),
        ] {
            assert_eq!(
                got, status,
                "case {case:?} ({operation}): expected {status}, got {got}: {body}"
            );
            assert_eq!(
                error_code(got, &body),
                code,
                "case {case:?} ({operation}): expected {code}, got {body}"
            );
            // The refusal names the ARN the request asked about.
            assert!(
                body["message"].as_str().unwrap_or_default().contains(arn),
                "case {case:?} ({operation}): expected the refusal to name {arn:?}, got {body}"
            );
        }
    }

    // Every refusal above stored nothing and removed nothing.
    assert_eq!(
        listed_tags(&router, "bystander", &bystander).await,
        expected(&[("keep", "me")]),
        "expected the refused calls to leave the real resource's tags alone"
    );
}

/// A tag set of `count` distinct keys, each key and value sized as asked.
fn sized_tags(count: usize, key_len: usize, value_len: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| {
                let suffix = format!("-{index:03}");
                let key = format!(
                    "{}{suffix}",
                    "k".repeat(key_len.saturating_sub(suffix.len()))
                );
                (key, json!("v".repeat(value_len)))
            })
            .collect(),
    )
}

/// Rule: a resource's tag set is bounded — at most [`MAX_RESOURCE_TAGS`] keys,
/// each key at most [`MAX_TAG_KEY_LEN`] characters and each value at most
/// [`MAX_TAG_VALUE_LEN`] (AWS's published tag restrictions for a MediaLive
/// resource) — so a request that would carry a resource past a bound is refused
/// with `BadRequestException` and the stored tags are left exactly as they were.
///
/// The rows sit ON each published bound and ONE PAST it, which is what a bound
/// generalises over, and the overwrite row shows the count is of the merged set
/// rather than of the request.
#[tokio::test]
async fn a_resource_tag_set_is_bounded() {
    // (case, tags written in one call, accepted?)
    //
    // Each row runs against a resource that already carries one tag, so the
    // count rows write one fewer than the bound to sit exactly ON it.
    let rows: [(&str, Value, bool); 6] = [
        ("one tag", sized_tags(1, 8, 8), true),
        (
            "fills up to exactly the tag maximum",
            sized_tags(MAX_RESOURCE_TAGS - 1, 8, 8),
            true,
        ),
        ("one tag past the maximum", sized_tags(MAX_RESOURCE_TAGS, 8, 8), false),
        ("a key at its maximum", sized_tags(1, MAX_TAG_KEY_LEN, 8), true),
        ("a key one past its maximum", sized_tags(1, MAX_TAG_KEY_LEN + 1, 8), false),
        ("a value one past its maximum", sized_tags(1, 8, MAX_TAG_VALUE_LEN + 1), false),
    ];

    for (case, written, accepted) in rows {
        let router = medialive();
        let arn = create_resource(&router, "channel", "bounded", None).await;
        // A tag that must survive a refusal.
        let (status, body) = create_tags(&router, &arn, json!({ "tags": tags(&[("keep", "me")]) })).await;
        assert_eq!(
            status, 204,
            "case {case:?}: the fixture tag got {status}: {body}"
        );

        let (status, body) = create_tags(&router, &arn, json!({ "tags": written.clone() })).await;
        if accepted {
            assert_eq!(
                status, 204,
                "case {case:?}: expected the write to be accepted, got {status}: {body}"
            );
            let listed = listed_tags(&router, case, &arn).await;
            let wrote = written.as_object().expect("a tag map");
            assert_eq!(
                listed.len(),
                wrote.len() + 1,
                "case {case:?}: expected {} tags, got {listed:?}",
                wrote.len() + 1
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the write to be refused, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            assert_eq!(
                listed_tags(&router, case, &arn).await,
                expected(&[("keep", "me")]),
                "case {case:?}: expected a refused write to store nothing"
            );
        }
    }

    // The bound is on the MERGED set, so re-tagging a full resource's own keys is
    // accepted while one more new key is not.
    let router = medialive();
    let arn = create_resource(&router, "channel", "full", None).await;
    let full = sized_tags(MAX_RESOURCE_TAGS, 8, 8);
    let (status, body) = create_tags(&router, &arn, json!({ "tags": full.clone() })).await;
    assert_eq!(status, 204, "filling the resource got {status}: {body}");

    let first_key = full
        .as_object()
        .and_then(|map| map.keys().next().cloned())
        .expect("a filled tag map has keys");
    let (status, body) = create_tags(
        &router,
        &arn,
        json!({ "tags": { first_key.clone(): "overwritten" } }),
    )
    .await;
    assert_eq!(
        status, 204,
        "overwriting a key of a full resource got {status}: {body}"
    );
    let listed = listed_tags(&router, "full", &arn).await;
    assert_eq!(
        listed.len(),
        MAX_RESOURCE_TAGS,
        "expected a full resource to stay at {MAX_RESOURCE_TAGS} tags, got {listed:?}"
    );
    assert_eq!(
        listed.get(&first_key),
        Some(&json!("overwritten")),
        "expected the overwrite to land, got {listed:?}"
    );

    let (status, body) = create_tags(&router, &arn, json!({ "tags": tags(&[("one", "more")]) })).await;
    assert_eq!(
        status, 400,
        "expected one more key on a full resource to be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "BadRequestException",
        "expected BadRequestException, got {body}"
    );
    assert_eq!(
        listed_tags(&router, "full", &arn).await,
        listed,
        "expected the refused write to leave a full resource's tags alone"
    );

    // Deleting a key makes room again: the bound is on what is stored, not a
    // one-way ratchet.
    let (status, body) = delete_tags(&router, &arn, &[first_key.as_str()]).await;
    assert_eq!(status, 204, "DeleteTags got {status}: {body}");
    let (status, body) = create_tags(&router, &arn, json!({ "tags": tags(&[("one", "more")]) })).await;
    assert_eq!(
        status, 204,
        "expected the freed slot to accept a new key, got {status}: {body}"
    );
    let listed = listed_tags(&router, "full", &arn).await;
    assert_eq!(
        listed.len(),
        MAX_RESOURCE_TAGS,
        "expected the resource to be full again, got {listed:?}"
    );
}
