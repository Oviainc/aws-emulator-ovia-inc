// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListTagsForResource`, driven over real HTTP through the same
//! `MultiServiceRouter` the emulator boots, so what is asserted is the wire behaviour.
//!
//! The rule under test: `ListTagsForResource` reports, in `Tags`, exactly the tag set the
//! stored resource its `ResourceArn` names currently carries — whatever the create stored
//! and whatever `TagResource`/`UntagResource` have since made of it — for either resource
//! type this service tags, and it REPORTS ONLY: calling it leaves that tag set, and the
//! rest of the record, exactly as it found them. An `ResourceArn` that is not a
//! MediaPackage channel or origin-endpoint ARN is `UnprocessableEntityException` (422)
//! keyed by the member; one naming a resource this service does not hold is
//! `NotFoundException` (404) keyed by the resource type — the same two refusals
//! `TagResource`/`UntagResource` answer for the identical member.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! resource type, how many tags the resource carries (none, one, several, the published
//! maximum), the keys' and values' character sets and lengths (including the published
//! per-tag maxima), and which tags a later request adds, replaces or removes.

use serde_json::{json, Value};
// AWS's published tagging limits, taken from the service's own constants rather than
// restated here, so the boundary rows below cannot diverge from the bounds the request
// validator enforces: at most `MAX_TAGS` tags per resource, a key of at most
// `MAX_TAG_KEY_CHARS` characters and a value of at most `MAX_TAG_VALUE_CHARS`.
use service_com_amazonaws_mediapackage_mediapackage::state::{
    MAX_TAGS, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

// The crate's shared test client, defined once in `tests/all/channel_lifecycle.rs`.
use crate::channel_lifecycle::{call, create_channel, emulator, error_code, ACCOUNT, REGION};

/// `arn:aws:mediapackage:<region>:<account>:channels/<id>`, as this service mints it.
fn channel_arn(id: &str) -> String {
    format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:channels/{id}")
}

/// `arn:aws:mediapackage:<region>:<account>:origin_endpoints/<id>`.
fn origin_endpoint_arn(id: &str) -> String {
    format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:origin_endpoints/{id}")
}

/// The URI the tagging operations bind. `ResourceArn` is a NON-greedy `@httpLabel`
/// (`smithy_query(query="operation",operation="ListTagsForResource")`), so an SDK
/// percent-encodes the ARN's reserved characters into one path segment and its `:` and
/// `/` never split the path.
fn tags_path(arn: &str) -> String {
    let label: String = arn
        .chars()
        .map(|character| match character {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect();
    format!("/tags/{label}")
}

/// `TagKeys` is an `@httpQuery` list, so each key travels as one `tagKeys=` parameter
/// with its reserved characters percent-encoded.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The two resource types this service tags, each as the create path, the create body a
/// fresh one needs, the ARN form and the read-back path — so every row of every table
/// below is asserted on both.
enum Kind {
    Channel,
    OriginEndpoint,
}

impl Kind {
    fn label(&self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::OriginEndpoint => "origin endpoint",
        }
    }

    fn arn(&self, id: &str) -> String {
        match self {
            Self::Channel => channel_arn(id),
            Self::OriginEndpoint => origin_endpoint_arn(id),
        }
    }

    fn describe_path(&self, id: &str) -> String {
        match self {
            Self::Channel => format!("/channels/{id}"),
            Self::OriginEndpoint => format!("/origin_endpoints/{id}"),
        }
    }
}

/// The channel every origin endpoint below is packaged from.
const PARENT_CHANNEL: &str = "tags-parent-channel";

/// Create a resource of `kind` with id `id` carrying `tags`, and return the record the
/// create stored.
async fn create_with_tags(
    router: &MultiServiceRouter,
    kind: &Kind,
    id: &str,
    tags: &Value,
) -> Value {
    let (path, body) = match kind {
        Kind::Channel => ("/channels", json!({ "id": id, "tags": tags })),
        Kind::OriginEndpoint => (
            "/origin_endpoints",
            json!({
                "channelId": PARENT_CHANNEL,
                "id": id,
                "hlsPackage": { "segmentDurationSeconds": 6 },
                "tags": tags
            }),
        ),
    };
    let (status, created) = call(router, "POST", path, Some(&body)).await;
    assert_eq!(
        status,
        200,
        "creating the fixture {} {id:?} expected 200, got {status} {created:?}",
        kind.label()
    );
    created
}

/// `ListTagsForResource` on `arn`: the status and the `Tags` map it reports, with an
/// absent or `null` member read as the empty map.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let (status, body) = call(router, "GET", &tags_path(arn), None).await;
    let tags = match body.get("tags") {
        None | Some(Value::Null) => json!({}),
        Some(value) => value.clone(),
    };
    (status, tags)
}

/// The `tags` member of a `Describe*` response, with an absent or `null` member read as
/// the empty map.
fn described_tags(record: &Value) -> Value {
    match record.get("tags") {
        None | Some(Value::Null) => json!({}),
        Some(value) => value.clone(),
    }
}

/// A tag map of `count` distinct entries, keyed so the generated set is stable and
/// ordering-independent.
fn generated_tags(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("key-{index:03}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

/// The tag sets a resource can carry. The rows differ along what the rule generalises
/// over: how many entries (none, one, a handful, the published `MAX_TAGS` maximum), the
/// keys' and values' character sets — AWS's tag character set includes letters, digits,
/// spaces and `_ . : / = + - @` — and their lengths, including a key at the published
/// `MAX_TAG_KEY_CHARS` maximum and a value at the published `MAX_TAG_VALUE_CHARS`
/// maximum.
fn tag_set_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("no tags at all", json!({})),
        ("one tag", json!({ "team": "video" })),
        (
            "several tags",
            json!({ "team": "video", "env": "staging", "cost-centre": "1234" }),
        ),
        (
            "keys and values across the AWS tag character set",
            json!({
                "aws:not:reserved_here": "a/b=c+d-e@f",
                "with space": "value with space",
                "dotted.key": "1.2.3",
                "UPPER_lower-123": ""
            }),
        ),
        (
            "a key at the MAX_TAG_KEY_CHARS maximum and a value at the MAX_TAG_VALUE_CHARS maximum",
            Value::Object(serde_json::Map::from_iter([(
                "k".repeat(MAX_TAG_KEY_CHARS),
                json!("v".repeat(MAX_TAG_VALUE_CHARS)),
            )])),
        ),
        ("the MAX_TAGS maximum", generated_tags(MAX_TAGS)),
    ]
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_tag_set_the_create_stored() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        for (index, (name, tags)) in tag_set_cases().into_iter().enumerate() {
            let case = format!("{} created with {name}", kind.label());
            let id = format!("tagged-{}-{index}", kind.label().replace(' ', "-"));
            let created = create_with_tags(&router, &kind, &id, &tags).await;
            let arn = kind.arn(&id);

            // The RAW body, before any normalisation: `Tags` is always present as an
            // object, so a resource carrying no tags reports an EMPTY map rather than an
            // absent member.
            let (status, raw) = call(&router, "GET", &tags_path(&arn), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {raw:?}"
            );
            assert!(
                raw.get("tags").and_then(Value::as_object).is_some(),
                "case {case:?}: expected a tags object in the body, got {raw:?}"
            );

            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed, tags,
                "case {case:?}: expected tags {tags:?}, got {listed:?}"
            );

            // The one stored tag set: what `ListTagsForResource` reports and what the
            // create and a `Describe*` of the same resource report cannot disagree.
            assert_eq!(
                listed,
                described_tags(&created),
                "case {case:?}: ListTagsForResource and the create disagree, got {listed:?} \
                 against {created:?}"
            );
            let (status, described) = call(&router, "GET", &kind.describe_path(&id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back expected 200, got {status} {described:?}"
            );
            assert_eq!(
                listed,
                described_tags(&described),
                "case {case:?}: ListTagsForResource and the Describe disagree, got {listed:?} \
                 against {described:?}"
            );

            // Reporting is not mutating: a second read answers the same tag set, and the
            // whole record is byte-for-byte the one the create stored.
            let (status, again) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the repeated read expected 200, got {status} {again:?}"
            );
            assert_eq!(
                again, listed,
                "case {case:?}: a read changed the tag set, expected {listed:?}, got {again:?}"
            );
            let (_, after) = call(&router, "GET", &kind.describe_path(&id), None).await;
            assert_eq!(
                after, described,
                "case {case:?}: a read changed the record, expected {described:?}, got {after:?}"
            );
        }
    }
}

/// A tagging sequence: the tags the resource is created with, the `TagResource` body
/// applied next, the `UntagResource` keys applied after that, and the tag set
/// `ListTagsForResource` must then report. The rows differ along whether a request adds a
/// new key, replaces an existing key's value, removes one of several keys, removes a key
/// the resource never carried, or empties the set entirely.
#[allow(clippy::type_complexity)]
fn tagging_sequence_cases() -> Vec<(&'static str, Value, Value, Vec<&'static str>, Value)> {
    vec![
        (
            "adds the first tag to an untagged resource",
            json!({}),
            json!({ "team": "video" }),
            vec![],
            json!({ "team": "video" }),
        ),
        (
            "adds a key beside the ones the create stored",
            json!({ "team": "video" }),
            json!({ "env": "prod" }),
            vec![],
            json!({ "team": "video", "env": "prod" }),
        ),
        (
            "replaces the value of a key already carried and leaves the others",
            json!({ "team": "video", "env": "staging" }),
            json!({ "env": "prod" }),
            vec![],
            json!({ "team": "video", "env": "prod" }),
        ),
        (
            "removes only the named key",
            json!({ "team": "video", "env": "staging", "owner": "ops" }),
            json!({}),
            vec!["env"],
            json!({ "team": "video", "owner": "ops" }),
        ),
        (
            "removes several named keys",
            json!({ "a": "1", "b": "2", "c": "3", "d": "4" }),
            json!({}),
            vec!["a", "c"],
            json!({ "b": "2", "d": "4" }),
        ),
        (
            "removing a key the resource never carried changes nothing",
            json!({ "team": "video" }),
            json!({}),
            vec!["absent-key"],
            json!({ "team": "video" }),
        ),
        (
            "removing every key leaves an empty tag set",
            json!({ "team": "video", "env": "staging" }),
            json!({}),
            vec!["team", "env"],
            json!({}),
        ),
        (
            "a key added then removed is gone, the one added beside it stays",
            json!({}),
            json!({ "temporary": "yes", "kept": "1" }),
            vec!["temporary"],
            json!({ "kept": "1" }),
        ),
        (
            "a key whose characters need percent-encoding in the untag query",
            json!({ "path/key": "x", "kept": "1" }),
            json!({}),
            vec!["path/key"],
            json!({ "kept": "1" }),
        ),
    ]
}

#[tokio::test]
async fn list_tags_for_resource_reports_what_tag_and_untag_left_behind() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        for (index, (name, created_tags, added, removed, expected)) in
            tagging_sequence_cases().into_iter().enumerate()
        {
            let case = format!("{}: {name}", kind.label());
            let id = format!("seq-{}-{index}", kind.label().replace(' ', "-"));
            create_with_tags(&router, &kind, &id, &created_tags).await;
            let arn = kind.arn(&id);

            if !added.as_object().expect("an object of tags").is_empty() {
                let (status, body) =
                    call(&router, "POST", &tags_path(&arn), Some(&json!({ "tags": added }))).await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: TagResource expected 204, got {status} {body:?}"
                );
            }
            if !removed.is_empty() {
                let query = removed
                    .iter()
                    .map(|key| format!("tagKeys={}", urlencode(key)))
                    .collect::<Vec<String>>()
                    .join("&");
                let (status, body) = call(
                    &router,
                    "DELETE",
                    &format!("{}?{query}", tags_path(&arn)),
                    None,
                )
                .await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: UntagResource expected 204, got {status} {body:?}"
                );
            }

            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed, expected,
                "case {case:?}: expected tags {expected:?}, got {listed:?}"
            );

            // The same record, read the other way: the two reads of one resource's tags
            // are rendered from the same stored map.
            let (_, described) = call(&router, "GET", &kind.describe_path(&id), None).await;
            assert_eq!(
                described_tags(&described),
                expected,
                "case {case:?}: the Describe expected tags {expected:?}, got {described:?}"
            );
        }
    }
}

/// An `ResourceArn` this service cannot resolve to one of its own resources, and the
/// status and error code that form must answer with. The rows differ along the way the
/// ARN fails: a well-formed ARN of each resource type naming nothing stored is a 404,
/// while anything that is not a MediaPackage channel or origin-endpoint ARN at all — a
/// foreign service, a resource type this service does not have, a missing id, a
/// non-ARN string — is a 422 about the member.
fn unresolvable_arn_cases() -> Vec<(&'static str, String, u16, &'static str)> {
    vec![
        (
            "a channel ARN naming no stored channel",
            channel_arn("no-such-channel"),
            404,
            "NotFoundException",
        ),
        (
            "an origin-endpoint ARN naming no stored endpoint",
            origin_endpoint_arn("no-such-endpoint"),
            404,
            "NotFoundException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channels/{PARENT_CHANNEL}"),
            422,
            "UnprocessableEntityException",
        ),
        (
            "an ARN of a resource type this service does not tag",
            format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:harvest_jobs/j1"),
            422,
            "UnprocessableEntityException",
        ),
        (
            "an ARN with no resource id",
            format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:channels/"),
            422,
            "UnprocessableEntityException",
        ),
        (
            "a string that is not an ARN at all",
            PARENT_CHANNEL.to_string(),
            422,
            "UnprocessableEntityException",
        ),
    ]
}

#[tokio::test]
async fn listing_the_tags_of_an_arn_this_service_cannot_resolve_is_refused() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;
    let (status, tagged) = call(
        &router,
        "POST",
        &tags_path(&channel_arn(PARENT_CHANNEL)),
        Some(&json!({ "tags": { "team": "video" } })),
    )
    .await;
    assert_eq!(
        status, 204,
        "the fixture tagging expected 204, got {status} {tagged:?}"
    );

    for (case, arn, expected_status, expected_code) in unresolvable_arn_cases() {
        let (status, error) = call(&router, "GET", &tags_path(&arn), None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            expected_code,
            "case {case:?}: expected {expected_code}, got {error:?}"
        );

        // A refusal carries this service's whole envelope: the sentence and the `errors`
        // map keyed by the thing that was wrong.
        assert!(
            error.get("message").and_then(Value::as_str).is_some(),
            "case {case:?}: expected a message, got {error:?}"
        );
        assert!(
            error
                .get("errors")
                .and_then(Value::as_object)
                .is_some_and(|errors| !errors.is_empty()),
            "case {case:?}: expected a non-empty errors map, got {error:?}"
        );

        // And it writes nothing: the tag set of the resource this service DOES hold is
        // untouched by a refused read of another ARN.
        let (status, listed) = list_tags(&router, &channel_arn(PARENT_CHANNEL)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the held resource expected 200, got {status} {listed:?}"
        );
        assert_eq!(
            listed,
            json!({ "team": "video" }),
            "case {case:?}: the held resource's tags changed, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_resource_has_no_tag_set_to_report() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for (index, kind) in [Kind::Channel, Kind::OriginEndpoint].into_iter().enumerate() {
        let case = format!("a deleted {}", kind.label());
        let id = format!("deleted-{index}");
        create_with_tags(&router, &kind, &id, &json!({ "team": "video" })).await;
        let arn = kind.arn(&id);

        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: before the delete expected 200, got {status} {listed:?}"
        );

        // `DeleteChannel` and `DeleteOriginEndpoint` are modelled 202.
        let (status, body) = call(&router, "DELETE", &kind.describe_path(&id), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the delete expected 202, got {status} {body:?}"
        );

        let (status, error) = call(&router, "GET", &tags_path(&arn), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: after the delete expected 404, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {error:?}"
        );
    }
}
