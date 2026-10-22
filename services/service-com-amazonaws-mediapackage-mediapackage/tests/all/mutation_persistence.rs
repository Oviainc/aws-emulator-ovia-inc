// SPDX-License-Identifier: CC0-1.0

//! A MediaPackage mutation is visible to the next read of the resource it changed.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//! The client, the region and the account label are the ones
//! `tests/all/channel_lifecycle.rs` defines, so both suites address the same emulator in
//! the same way.
//!
//! The rule under test, for every operation whose own documentation says it CHANGES a
//! channel or an origin endpoint (`UpdateChannel`, `ConfigureLogs`, `UpdateOriginEndpoint`,
//! `TagResource`, `UntagResource`): the request's effect is written into the stored
//! record, so the operation's own response and every later read of that resource
//! (`DescribeChannel`, `DescribeOriginEndpoint`) render the UPDATED record, while the
//! members the request does not bind — the id, the ARN, `CreatedAt`, the ingest
//! endpoints, the channel a packaged endpoint belongs to — keep the values the create
//! stored. A mutation naming a resource the service does not hold writes nothing and is
//! `NotFoundException` (404).
//!
//! Each suite is a table whose rows differ along what the rule generalises over: whether
//! a member is already set, whether the request sends one, its value, its length, its
//! character set, how many entries it carries, and (for a packaged endpoint) which of the
//! four packaging formats it names.

use serde_json::{json, Value};
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
/// (`smithy_query(query="operation",operation="TagResource")`), so an SDK percent-encodes
/// the ARN's reserved characters into one path segment and its `:` and `/` never split
/// the path.
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

/// A response member at a JSON pointer, with an absent member, an explicit `null`, an
/// empty list and an empty map all read as the same thing: "the stored record carries
/// nothing there". The table rows below spell that expectation as `null`.
fn member(body: &Value, pointer: &str) -> Value {
    match body.pointer(pointer) {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Array(items)) if items.is_empty() => Value::Null,
        Some(Value::Object(map)) if map.is_empty() => Value::Null,
        Some(value) => value.clone(),
    }
}

/// Every member of `expected` — keyed by JSON pointer — as the stored record must report
/// it; see [`member`]. Only the members a row names are compared, so a member the service
/// server-fills elsewhere does not have to be restated in every row.
fn assert_members(case: &str, actual: &Value, expected: &Value) {
    for (pointer, want) in expected.as_object().expect("an object of expectations") {
        let got = member(actual, pointer);
        assert_eq!(
            &got, want,
            "case {case:?}: expected {pointer} {want:?}, got {got:?} in {actual:?}"
        );
    }
}

/// The members a mutation never changes, as the create reported them: the id, the ARN and
/// the instant the resource was created.
fn assert_identity_preserved(case: &str, created: &Value, after: &Value) {
    for key in ["id", "arn", "createdAt"] {
        assert_eq!(
            after.get(key),
            created.get(key),
            "case {case:?}: {key} must be the one the create stored, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateChannel
// ---------------------------------------------------------------------------

/// An `UpdateChannel` case: the description the channel is created with, the update body
/// (the id is added per case), and the description the stored channel must report
/// afterwards. The rows differ along whether the stored value is set, whether the request
/// sends one, and the value's length and character set.
fn update_channel_cases() -> Vec<(
    &'static str,
    Option<&'static str>,
    Value,
    Option<&'static str>,
)> {
    vec![
        (
            "sets a description on a channel created without one",
            None,
            json!({ "description": "first words" }),
            Some("first words"),
        ),
        (
            "replaces the description the create stored",
            Some("initial description"),
            json!({ "description": "updated description" }),
            Some("updated description"),
        ),
        (
            "stores an empty description",
            Some("initial description"),
            json!({ "description": "" }),
            Some(""),
        ),
        (
            "clears the description when the request sends none",
            Some("initial description"),
            json!({}),
            None,
        ),
        (
            "stores a long non-ASCII description",
            Some("short"),
            json!({ "description": "日本語の説明 — ingest för kanalen, 24/7 · ★" }),
            Some("日本語の説明 — ingest för kanalen, 24/7 · ★"),
        ),
        (
            "re-sending the stored description changes nothing",
            Some("steady"),
            json!({ "description": "steady" }),
            Some("steady"),
        ),
    ]
}

#[tokio::test]
async fn update_channel_writes_the_description_into_the_stored_channel() {
    let router = emulator();

    for (index, (case, created_description, update, expected)) in
        update_channel_cases().into_iter().enumerate()
    {
        let id = format!("update-channel-{index}");
        let mut create = json!({ "id": id, "tags": { "owner": "probe" } });
        if let Some(description) = created_description {
            create["description"] = json!(description);
        }
        let (status, created) = call(&router, "POST", "/channels", Some(&create)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating the channel expected 200, got {status} {created:?}"
        );

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/channels/{id}"),
            Some(&with_id(&update, &id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannel expected 200, got {status} {updated:?}"
        );
        assert_eq!(
            updated.get("description").and_then(Value::as_str),
            expected,
            "case {case:?}: expected description {expected:?}, got {updated:?}"
        );

        // The read after the write is the point of the rule: it must render the UPDATED
        // record, byte for byte what the update answered.
        let (status, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, updated,
            "case {case:?}: DescribeChannel must render the updated channel"
        );

        // `Description` is the only member `UpdateChannelRequest` binds, so everything
        // else the create stored survives the update.
        assert_identity_preserved(case, &created, &described);
        for key in ["hlsIngest", "tags", "ingressAccessLogs", "egressAccessLogs"] {
            assert_eq!(
                described.get(key),
                created.get(key),
                "case {case:?}: {key} is not this operation's input and must be unchanged, \
                 got {described:?}"
            );
        }
    }
}

/// The update body with the path's id added, which is how the schema server fills the
/// `@httpLabel` member.
fn with_id(body: &Value, id: &str) -> Value {
    let mut body = body.clone();
    body["id"] = json!(id);
    body
}

// ---------------------------------------------------------------------------
// ConfigureLogs
// ---------------------------------------------------------------------------

/// A `ConfigureLogs` case: the request body (the id is added per case) and the ingress and
/// egress log groups the stored channel must report afterwards. Every case starts from a
/// channel that already has BOTH subscriptions, so a row that drops one is observable.
fn configure_logs_cases() -> Vec<(&'static str, Value, Option<&'static str>, Option<&'static str>)>
{
    vec![
        (
            "subscribes both directions",
            json!({
                "ingressAccessLogs": { "logGroupName": "/aws/MediaPackage/probe-ingress" },
                "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/probe-egress" }
            }),
            Some("/aws/MediaPackage/probe-ingress"),
            Some("/aws/MediaPackage/probe-egress"),
        ),
        (
            "subscribes ingress only, dropping the stored egress subscription",
            json!({ "ingressAccessLogs": { "logGroupName": "/aws/MediaPackage/only-ingress" } }),
            Some("/aws/MediaPackage/only-ingress"),
            None,
        ),
        (
            "subscribes egress only, dropping the stored ingress subscription",
            json!({ "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/only-egress" } }),
            None,
            Some("/aws/MediaPackage/only-egress"),
        ),
        (
            "unsubscribes both when the request carries neither member",
            json!({}),
            None,
            None,
        ),
        (
            "unsubscribes both when the members carry no log group name",
            json!({ "ingressAccessLogs": {}, "egressAccessLogs": {} }),
            None,
            None,
        ),
        (
            "accepts a log group name of the caller's own choosing",
            json!({
                "ingressAccessLogs": { "logGroupName": "my-own-group" },
                "egressAccessLogs": { "logGroupName": "a/deeply/nested/group-name_1" }
            }),
            Some("my-own-group"),
            Some("a/deeply/nested/group-name_1"),
        ),
    ]
}

#[tokio::test]
async fn configure_logs_writes_the_log_subscription_into_the_stored_channel() {
    let router = emulator();

    for (index, (case, request, expected_ingress, expected_egress)) in
        configure_logs_cases().into_iter().enumerate()
    {
        let id = format!("configure-logs-{index}");
        let create = json!({
            "id": id,
            "description": "logged channel",
            "tags": { "owner": "probe" }
        });
        let (status, created) = call(&router, "POST", "/channels", Some(&create)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating the channel expected 200, got {status} {created:?}"
        );

        // Every case starts from both directions subscribed, so a row that leaves one out
        // is asserted to UNSUBSCRIBE it rather than to leave a stale value in place.
        let seed = json!({
            "id": id,
            "ingressAccessLogs": { "logGroupName": "/aws/MediaPackage/seed-ingress" },
            "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/seed-egress" }
        });
        let (status, seeded) = call(
            &router,
            "PUT",
            &format!("/channels/{id}/configure_logs"),
            Some(&seed),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the seeding ConfigureLogs expected 200, got {status} {seeded:?}"
        );
        assert_eq!(
            seeded.pointer("/ingressAccessLogs/logGroupName").cloned(),
            Some(json!("/aws/MediaPackage/seed-ingress")),
            "case {case:?}: the seeding ConfigureLogs must report what it subscribed, \
             got {seeded:?}"
        );

        let (status, configured) = call(
            &router,
            "PUT",
            &format!("/channels/{id}/configure_logs"),
            Some(&with_id(&request, &id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ConfigureLogs expected 200, got {status} {configured:?}"
        );

        let (status, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, configured,
            "case {case:?}: DescribeChannel must render the configured channel"
        );
        for (pointer, expected) in [
            ("/ingressAccessLogs/logGroupName", expected_ingress),
            ("/egressAccessLogs/logGroupName", expected_egress),
        ] {
            let got = described.pointer(pointer).and_then(Value::as_str);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {pointer} {expected:?}, got {got:?} in {described:?}"
            );
        }

        // `ConfigureLogs` binds only the two log-subscription members.
        assert_identity_preserved(case, &created, &described);
        for key in ["description", "hlsIngest", "tags"] {
            assert_eq!(
                described.get(key),
                created.get(key),
                "case {case:?}: {key} is not this operation's input and must be unchanged, \
                 got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// TagResource / UntagResource
// ---------------------------------------------------------------------------

/// A tagging case: the tag maps to apply in order, the keys to remove afterwards, and the
/// tag set the stored resource must then report. The rows differ along how many calls are
/// made, how many entries each carries, whether a key is re-used, and the characters a
/// key or value is made of.
fn tagging_cases() -> Vec<(&'static str, Vec<Value>, Vec<&'static str>, Value)> {
    vec![
        (
            "one tag",
            vec![json!({ "owner": "probe" })],
            vec![],
            json!({ "owner": "probe" }),
        ),
        (
            "several tags in one call",
            vec![json!({ "a": "1", "b": "2", "c": "3" })],
            vec![],
            json!({ "a": "1", "b": "2", "c": "3" }),
        ),
        (
            "a second call adds without dropping the first call's tags",
            vec![json!({ "a": "1" }), json!({ "b": "2" })],
            vec![],
            json!({ "a": "1", "b": "2" }),
        ),
        (
            "re-tagging a key replaces that key's value only",
            vec![json!({ "a": "1", "b": "2" }), json!({ "a": "9" })],
            vec![],
            json!({ "a": "9", "b": "2" }),
        ),
        (
            "untagging removes only the named keys",
            vec![json!({ "a": "1", "b": "2", "c": "3" })],
            vec!["a", "c"],
            json!({ "b": "2" }),
        ),
        (
            "untagging a key the resource does not carry changes nothing",
            vec![json!({ "a": "1" })],
            vec!["not-a-key"],
            json!({ "a": "1" }),
        ),
        (
            "untagging every key leaves no tags",
            vec![json!({ "a": "1", "b": "2" })],
            vec!["a", "b"],
            json!({}),
        ),
        (
            "a key and a value of their own alphabet",
            vec![json!({ "cost:center": "video-€/eu", "Ägare": "staffan" })],
            vec![],
            json!({ "cost:center": "video-€/eu", "Ägare": "staffan" }),
        ),
    ]
}

#[tokio::test]
async fn tagging_writes_the_tags_into_the_stored_channel_or_origin_endpoint() {
    let router = emulator();
    create_channel(&router, "tagged-channel").await;

    for (index, (case, batches, removed, expected)) in tagging_cases().into_iter().enumerate() {
        // The two resource kinds this service holds, each addressed by its own ARN form.
        for kind in ["channel", "origin_endpoint"] {
            let case = format!("{kind}: {case}");
            let id = format!("tagging-{kind}-{index}");
            let (arn, read_path, created) = if kind == "channel" {
                let created = create_described_channel(&router, &id).await;
                (channel_arn(&id), format!("/channels/{id}"), created)
            } else {
                let created = create_origin_endpoint(&router, "tagged-channel", &id, &json!({}))
                    .await;
                (
                    origin_endpoint_arn(&id),
                    format!("/origin_endpoints/{id}"),
                    created,
                )
            };

            for batch in &batches {
                let (status, body) = call(
                    &router,
                    "POST",
                    &tags_path(&arn),
                    Some(&json!({ "tags": batch })),
                )
                .await;
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

            let (status, described) = call(&router, "GET", &read_path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back expected 200, got {status} {described:?}"
            );
            let got = described.get("tags").cloned().unwrap_or(json!({}));
            assert_eq!(
                got, expected,
                "case {case:?}: expected tags {expected:?}, got {got:?}"
            );
            assert_identity_preserved(&case, &created, &described);
        }
    }
}

/// `TagKeys` is an `@httpQuery` list, so each key is one `tagKeys=` parameter with its
/// reserved characters percent-encoded.
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

#[tokio::test]
async fn tagging_an_arn_this_service_does_not_hold_writes_nothing() {
    let router = emulator();
    create_channel(&router, "untouched-channel").await;
    let (_, before) = call(&router, "GET", "/channels/untouched-channel", None).await;

    // An ARN this service cannot resolve to one of its own resources, and the status and
    // error code each form must answer with: a malformed or foreign ARN is an invalid
    // parameter (422), a well-formed one naming no stored resource is a 404.
    let cases: Vec<(&str, String, u16, &str)> = vec![
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
            format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channels/untouched-channel"),
            422,
            "UnprocessableEntityException",
        ),
        (
            "an ARN of a resource type this service does not have",
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
            "untouched-channel".to_string(),
            422,
            "UnprocessableEntityException",
        ),
    ];

    for (case, arn, expected_status, expected_code) in cases {
        for (method, body) in [
            ("POST", Some(json!({ "tags": { "owner": "probe" } }))),
            ("DELETE", None),
        ] {
            let path = if method == "DELETE" {
                format!("{}?tagKeys=owner", tags_path(&arn))
            } else {
                tags_path(&arn)
            };
            let (status, error) = call(&router, method, &path, body.as_ref()).await;
            assert_eq!(
                status, expected_status,
                "case {case:?} ({method}): expected {expected_status}, got {status} {error:?}"
            );
            assert_eq!(
                error_code(&error),
                expected_code,
                "case {case:?} ({method}): expected {expected_code}, got {error:?}"
            );
        }
    }

    let (_, after) = call(&router, "GET", "/channels/untouched-channel", None).await;
    assert_eq!(
        after, before,
        "a refused tagging request must leave every stored resource alone, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// UpdateOriginEndpoint
// ---------------------------------------------------------------------------

/// The packaging and endpoint configuration every `UpdateOriginEndpoint` case starts from,
/// so each row's assertion is about what the UPDATE left behind. `Origination` is created
/// as `DENY`, the value that is NOT this member's server default, so a row that omits it
/// can tell "the create's value was kept" from "the member was written afresh".
fn endpoint_create_body() -> Value {
    json!({
        "description": "initial endpoint",
        "startoverWindowSeconds": 300,
        "timeDelaySeconds": 5,
        "origination": "DENY",
        "whitelist": ["10.0.0.0/8"],
        "tags": { "owner": "probe" },
        "hlsPackage": { "segmentDurationSeconds": 6, "playlistWindowSeconds": 60 }
    })
}

/// An `UpdateOriginEndpoint` case: the request body (the id is added per case), the
/// members the stored endpoint must report afterwards — keyed by JSON pointer, `null`
/// meaning "carries nothing there", see [`member`] — and the manifest its playback URL
/// must end in. The rows differ along which members the request binds, which of the four
/// packaging formats it names, the values' signs and magnitudes, and the manifest name the
/// URL is derived from.
///
/// An update is a write of the endpoint, so a member it leaves unset takes that member's
/// server default (`state::apply_origin_endpoint_defaults`) rather than keeping the
/// create's value — which is what several rows below assert.
///
/// `Origination`'s two values (`ALLOW`, `DENY`) and the four packaging members come from
/// `smithy_query(query="operation",operation="UpdateOriginEndpoint",
/// model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`.
fn update_origin_endpoint_cases() -> Vec<(&'static str, Value, Value, &'static str)> {
    vec![
        (
            "changes the description, the startover window and the HLS packaging",
            json!({
                "description": "updated endpoint",
                "startoverWindowSeconds": 600,
                "hlsPackage": { "segmentDurationSeconds": 10, "playlistWindowSeconds": 120 }
            }),
            json!({
                "/description": "updated endpoint",
                "/startoverWindowSeconds": 600,
                "/hlsPackage/segmentDurationSeconds": 10,
                "/hlsPackage/playlistWindowSeconds": 120,
                // Bound by the request? No — so written afresh, which for these two means
                // the server default, not the 5 and the DENY the create stored.
                "/timeDelaySeconds": 0,
                "/origination": "ALLOW",
                "/whitelist": null
            }),
            "index.m3u8",
        ),
        (
            "writes every member the request does not bind afresh, not the create's value",
            json!({ "hlsPackage": { "segmentDurationSeconds": 6 } }),
            json!({
                "/description": null,
                "/startoverWindowSeconds": 0,
                "/timeDelaySeconds": 0,
                "/origination": "ALLOW",
                "/whitelist": null,
                "/hlsPackage/segmentDurationSeconds": 6,
                // The replaced packaging is server-filled too, so the stored endpoint
                // carries the same member set as a created one.
                "/hlsPackage/playlistWindowSeconds": 60,
                "/hlsPackage/adMarkers": "NONE"
            }),
            "index.m3u8",
        ),
        (
            "stores a zero the request sends",
            json!({
                "startoverWindowSeconds": 0,
                "timeDelaySeconds": 0,
                "hlsPackage": { "segmentDurationSeconds": 1 }
            }),
            json!({
                "/startoverWindowSeconds": 0,
                "/timeDelaySeconds": 0,
                "/hlsPackage/segmentDurationSeconds": 1
            }),
            "index.m3u8",
        ),
        (
            "stores the Origination value that is not the default, and a longer whitelist",
            json!({
                "origination": "DENY",
                "whitelist": ["10.0.0.0/8", "192.168.0.0/16", "172.16.0.0/12"],
                "hlsPackage": { "segmentDurationSeconds": 6 }
            }),
            json!({
                "/origination": "DENY",
                "/whitelist": ["10.0.0.0/8", "192.168.0.0/16", "172.16.0.0/12"]
            }),
            "index.m3u8",
        ),
        (
            "replaces the HLS packaging with a DASH packaging",
            json!({
                "description": "now dash",
                "dashPackage": { "segmentDurationSeconds": 2, "manifestWindowSeconds": 30 }
            }),
            json!({
                "/description": "now dash",
                "/hlsPackage": null,
                "/dashPackage/segmentDurationSeconds": 2,
                "/dashPackage/manifestWindowSeconds": 30,
                // The new packaging's own server-filled members.
                "/dashPackage/profile": "NONE",
                "/dashPackage/minBufferTimeSeconds": 30
            }),
            "index.mpd",
        ),
        (
            "replaces the HLS packaging with an MSS packaging",
            json!({ "mssPackage": { "segmentDurationSeconds": 2, "manifestWindowSeconds": 60 } }),
            json!({
                "/hlsPackage": null,
                "/mssPackage/segmentDurationSeconds": 2,
                "/mssPackage/manifestWindowSeconds": 60
            }),
            "index.ism/Manifest",
        ),
        (
            "replaces the HLS packaging with a CMAF packaging",
            json!({
                "cmafPackage": {
                    "segmentDurationSeconds": 4,
                    "hlsManifests": [
                        { "id": "primary", "manifestName": "master" },
                        { "id": "secondary" }
                    ]
                }
            }),
            json!({
                "/hlsPackage": null,
                "/cmafPackage/segmentDurationSeconds": 4,
                // `SegmentPrefix`' documented default is the ChannelId, and each child
                // manifest is server-filled like a parent HLS manifest.
                "/cmafPackage/segmentPrefix": "packaged-channel",
                "/cmafPackage/hlsManifests/0/manifestName": "master",
                "/cmafPackage/hlsManifests/0/adMarkers": "NONE",
                "/cmafPackage/hlsManifests/1/manifestName": "index"
            }),
            "index/index.m3u8",
        ),
        (
            "keeps the stored packaging when the request names none",
            json!({ "description": "configuration only" }),
            json!({
                "/description": "configuration only",
                "/hlsPackage/segmentDurationSeconds": 6,
                "/hlsPackage/playlistWindowSeconds": 60
            }),
            "index.m3u8",
        ),
        (
            "re-derives the playback URL from a new manifest name",
            json!({
                "manifestName": "stream",
                "hlsPackage": { "segmentDurationSeconds": 6 }
            }),
            json!({ "/manifestName": "stream" }),
            "stream.m3u8",
        ),
    ]
}

#[tokio::test]
async fn update_origin_endpoint_writes_every_member_it_binds_into_the_stored_endpoint() {
    let router = emulator();
    create_channel(&router, "packaged-channel").await;

    for (index, (case, update, expected, manifest)) in
        update_origin_endpoint_cases().into_iter().enumerate()
    {
        let id = format!("update-endpoint-{index}");
        let created =
            create_origin_endpoint(&router, "packaged-channel", &id, &endpoint_create_body()).await;

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&with_id(&update, &id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateOriginEndpoint expected 200, got {status} {updated:?}"
        );
        assert_members(case, &updated, &expected);

        // The playback URL is DERIVED from the endpoint's manifest name and its packaging
        // format, so an update that changes either re-derives it — and it stays in the
        // endpoint's own region.
        let url = updated.get("url").and_then(Value::as_str).unwrap_or("");
        assert!(
            url.starts_with("https://") && url.contains(REGION) && url.ends_with(manifest),
            "case {case:?}: expected a playback url in {REGION} ending in {manifest:?}, \
             got {url:?}"
        );

        // Each child HLS manifest of a CMAF package carries a URL the SERVICE derives for
        // it, so an update that stores a CMAF packaging must mint one per manifest.
        if let Some(manifests) = updated
            .pointer("/cmafPackage/hlsManifests")
            .and_then(Value::as_array)
        {
            assert!(
                !manifests.is_empty(),
                "case {case:?}: a stored CMAF package must keep its child manifests, \
                 got {updated:?}"
            );
            for child in manifests {
                let name = child
                    .get("manifestName")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let child_url = child.get("url").and_then(Value::as_str).unwrap_or_default();
                assert!(
                    child_url.starts_with("https://")
                        && child_url.contains(REGION)
                        && child_url.contains(name)
                        && child_url.ends_with("/index.m3u8"),
                    "case {case:?}: CMAF manifest {child:?} must carry a derived url naming \
                     {name:?} in {REGION}, got {child_url:?}"
                );
            }
        }

        // The read after the write: the stored endpoint, byte for byte what the update
        // answered.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, updated,
            "case {case:?}: DescribeOriginEndpoint must render the updated endpoint"
        );

        // `ChannelId` and `Tags` are not `UpdateOriginEndpointRequest` members, so the
        // update leaves them as the create stored them.
        assert_identity_preserved(case, &created, &described);
        for key in ["channelId", "tags"] {
            assert_eq!(
                described.get(key),
                created.get(key),
                "case {case:?}: {key} is not this operation's input and must be unchanged, \
                 got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_naming_several_packaging_configurations_is_refused_and_writes_nothing() {
    let router = emulator();
    create_channel(&router, "packaged-channel").await;
    let created =
        create_origin_endpoint(&router, "packaged-channel", "refused", &endpoint_create_body())
            .await;

    let (status, error) = call(
        &router,
        "PUT",
        "/origin_endpoints/refused",
        Some(&json!({
            "id": "refused",
            "description": "two packagings",
            "hlsPackage": { "segmentDurationSeconds": 6 },
            "dashPackage": { "segmentDurationSeconds": 2 }
        })),
    )
    .await;
    assert_eq!(
        status, 422,
        "expected 422 for two packaging configurations, got {status} {error:?}"
    );
    assert_eq!(
        error_code(&error),
        "UnprocessableEntityException",
        "expected UnprocessableEntityException, got {error:?}"
    );

    let (_, described) = call(&router, "GET", "/origin_endpoints/refused", None).await;
    assert_eq!(
        described, created,
        "a refused update must leave the stored endpoint alone, got {described:?}"
    );
}

#[tokio::test]
async fn a_mutation_of_a_resource_the_service_does_not_hold_is_not_found() {
    let router = emulator();

    // Every mutating operation that addresses a resource by id, with the request body it
    // needs; none of these resources exists.
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "UpdateChannel",
            "PUT",
            "/channels/missing",
            json!({ "id": "missing", "description": "no such channel" }),
        ),
        (
            "ConfigureLogs",
            "PUT",
            "/channels/missing/configure_logs",
            json!({
                "id": "missing",
                "ingressAccessLogs": { "logGroupName": "/aws/MediaPackage/missing" }
            }),
        ),
        (
            "UpdateOriginEndpoint",
            "PUT",
            "/origin_endpoints/missing",
            json!({
                "id": "missing",
                "hlsPackage": { "segmentDurationSeconds": 6 }
            }),
        ),
        // The target is resolved BEFORE the request is judged, so an id the service does
        // not hold is reported as not found even when the request is invalid as well.
        (
            "UpdateOriginEndpoint with two packaging configurations",
            "PUT",
            "/origin_endpoints/missing",
            json!({
                "id": "missing",
                "hlsPackage": { "segmentDurationSeconds": 6 },
                "dashPackage": { "segmentDurationSeconds": 2 }
            }),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, error) = call(&router, method, path, Some(&body)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {error:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Create a channel and return the record the create stored.
async fn create_described_channel(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, created) = call(
        router,
        "POST",
        "/channels",
        Some(&json!({ "id": id, "description": "tagged channel" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the fixture channel {id:?} expected 200, got {status} {created:?}"
    );
    created
}

/// Create an origin endpoint on `channel_id` from `body` (an HLS packaging is supplied
/// when `body` names no packaging configuration, since exactly one is required), and
/// return the record the create stored.
async fn create_origin_endpoint(
    router: &MultiServiceRouter,
    channel_id: &str,
    id: &str,
    body: &Value,
) -> Value {
    let mut request = json!({ "channelId": channel_id, "id": id });
    for (key, value) in body.as_object().expect("an object body") {
        request[key] = value.clone();
    }
    if !["hlsPackage", "dashPackage", "cmafPackage", "mssPackage"]
        .iter()
        .any(|key| request.get(*key).is_some())
    {
        request["hlsPackage"] = json!({ "segmentDurationSeconds": 6 });
    }
    let (status, created) = call(router, "POST", "/origin_endpoints", Some(&request)).await;
    assert_eq!(
        status, 200,
        "creating the fixture endpoint {id:?} expected 200, got {status} {created:?}"
    );
    created
}
