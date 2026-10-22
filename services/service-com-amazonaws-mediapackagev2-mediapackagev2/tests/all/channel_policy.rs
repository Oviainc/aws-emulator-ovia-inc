// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the MediaPackageV2 channel resource policy
//! (`PutChannelPolicy` / `GetChannelPolicy`), driven over HTTP through the same router
//! the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. `GetChannelPolicy` reports the document the last `PutChannelPolicy` stored on the
//!    addressed channel, byte for byte, with that channel's group and channel names.
//! 2. A channel holds at most ONE policy: a second put replaces the first.
//! 3. A policy belongs to the full channel address — attaching one changes no other
//!    channel's policy, in the same group or in another.
//! 4. `GetChannelPolicy` answers `ResourceNotFoundException` (404) whenever the thing
//!    addressed is absent: an unknown channel group, an unknown channel, a channel that
//!    has never had a policy attached, and a channel re-created under a name whose
//!    earlier incarnation had one.
//! 5. `PutChannelPolicy` resolves the parent chain before storing anything, so a put
//!    naming an absent group or channel is a 404 and leaves nothing behind.
//! 6. A `Policy` that is not a policy document is refused with `ValidationException`
//!    (400) and `ValidationExceptionType` `INVALID_POLICY`, leaving any policy already
//!    attached in place.
//! 7. `PolicyText` is bounded: `smithy_query(query="shape",shape="PolicyText")` gives
//!    `length {max: 6144}`, so a document at the bound is accepted and one past it is
//!    refused with `ValidationException` (400).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. It travels in the SigV4
/// credential scope, which is also what the service reads its region from.
const REGION: &str = "us-west-2";

/// The documented maximum length of `PolicyText`, from
/// `smithy_query(query="shape",shape="PolicyText")`: `length {max: 6144}`.
const POLICY_TEXT_MAX_LENGTH: usize = 6144;

/// A MediaPackageV2 emulator plus a tiny restJson1 client for it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite signs nothing: it exercises handler behaviour, not authentication.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for an
    /// empty body).
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("mediapackagev2.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/mediapackagev2/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON: {e}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Create a channel group, panicking unless it was created: this is suite setup, not
    /// what is under test.
    async fn given_group(&self, name: &str) {
        let (status, body) = self
            .call("POST", "/channelGroup", Some(json!({"ChannelGroupName": name})))
            .await;
        assert_eq!(status, 200, "setup: creating group {name:?} got {body}");
    }

    /// Create a channel in `group`, panicking unless it was created.
    async fn given_channel(&self, group: &str, name: &str) {
        let (status, body) = self
            .call(
                "POST",
                &format!("/channelGroup/{group}/channel"),
                Some(json!({"ChannelName": name})),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: creating channel {name:?} in {group:?} got {body}"
        );
    }

    async fn delete_channel(&self, group: &str, channel: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/channelGroup/{group}/channel/{channel}/"),
            None,
        )
        .await
    }

    async fn put_policy(&self, group: &str, channel: &str, policy: &str) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/channelGroup/{group}/channel/{channel}/policy"),
            Some(json!({"Policy": policy})),
        )
        .await
    }

    async fn get_policy(&self, group: &str, channel: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/channelGroup/{group}/channel/{channel}/policy"),
            None,
        )
        .await
    }
}

/// The `__type` discriminator of an error envelope.
fn error_type(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A member of an error envelope, or `""` when it is absent.
fn member<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A policy document of exactly `length` bytes: a real policy whose `Sid` is padded out
/// to the requested total, so the length rule can be probed at and past its bound.
fn policy_of_length(length: usize) -> String {
    let skeleton =
        json!({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Sid": ""}]}).to_string();
    let padding = length
        .checked_sub(skeleton.len())
        .expect("the skeleton must fit inside the requested length");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Sid": "A".repeat(padding)}],
    })
    .to_string();
    assert_eq!(policy.len(), length, "padding must hit the exact length");
    policy
}

/// Policy documents spanning the class the round-trip rule covers: they differ in member
/// set, statement count, nesting depth, whitespace, non-ASCII content and size.
fn policy_cases() -> Vec<(&'static str, String)> {
    vec![
        (
            "one statement, no version",
            json!({"Statement": [{"Effect": "Allow", "Action": "mediapackagev2:GetObject"}]})
                .to_string(),
        ),
        (
            "versioned, two statements",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "AllowRead",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                    },
                    {
                        "Sid": "DenyHarvest",
                        "Effect": "Deny",
                        "Principal": {"AWS": "arn:aws:iam::000000000000:root"},
                        "Action": ["mediapackagev2:GetHeadObject"],
                    },
                ],
            })
            .to_string(),
        ),
        (
            "nested condition",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": "mediapackagev2:GetObject",
                    "Condition": {"StringEquals": {"aws:SourceVpc": ["vpc-1", "vpc-2"]}},
                }],
            })
            .to_string(),
        ),
        (
            "statement as an object rather than a list",
            json!({"Statement": {"Effect": "Deny", "Action": "*"}}).to_string(),
        ),
        (
            "pretty-printed with newlines and tabs",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\"Effect\": \"Allow\"}\n\t]\n}"
                .to_string(),
        ),
        (
            "non-ascii Sid",
            json!({"Statement": [{"Sid": "ポリシー-π", "Effect": "Allow"}]}).to_string(),
        ),
        (
            "at the PolicyText length bound",
            policy_of_length(POLICY_TEXT_MAX_LENGTH),
        ),
    ]
}

/// RULE 1 — every policy in the table round-trips byte for byte, under the address it
/// was attached to.
#[tokio::test]
async fn a_stored_policy_is_reported_back_unchanged() {
    let emulator = Emulator::new();
    emulator.given_group("group-1").await;

    for (index, (label, policy)) in policy_cases().into_iter().enumerate() {
        let channel = format!("channel-{index}");
        emulator.given_channel("group-1", &channel).await;

        let (status, body) = emulator.put_policy("group-1", &channel, &policy).await;
        assert_eq!(status, 200, "case {label:?}: put expected 200, got {body}");

        let (status, body) = emulator.get_policy("group-1", &channel).await;
        assert_eq!(status, 200, "case {label:?}: get expected 200, got {body}");
        assert_eq!(
            member(&body, "Policy"),
            policy,
            "case {label:?}: expected the stored document verbatim, got {body}"
        );
        assert_eq!(
            member(&body, "ChannelGroupName"),
            "group-1",
            "case {label:?}: expected the addressed group, got {body}"
        );
        assert_eq!(
            member(&body, "ChannelName"),
            channel,
            "case {label:?}: expected the addressed channel, got {body}"
        );
    }
}

/// RULE 2 — a channel holds one policy; the newest put is what a read reports.
#[tokio::test]
async fn a_second_put_replaces_the_policy_the_first_stored() {
    let emulator = Emulator::new();
    emulator.given_group("group-1").await;
    emulator.given_channel("group-1", "channel-1").await;

    let mut previous: Option<String> = None;
    for (label, policy) in policy_cases() {
        let (status, body) = emulator.put_policy("group-1", "channel-1", &policy).await;
        assert_eq!(status, 200, "case {label:?}: put expected 200, got {body}");

        let (status, body) = emulator.get_policy("group-1", "channel-1").await;
        assert_eq!(status, 200, "case {label:?}: get expected 200, got {body}");
        assert_eq!(
            member(&body, "Policy"),
            policy,
            "case {label:?}: expected the newest document, got {body}"
        );
        if let Some(previous) = previous.as_deref() {
            assert_ne!(
                member(&body, "Policy"),
                previous,
                "case {label:?}: the replaced document must be gone, got {body}"
            );
        }
        previous = Some(policy);
    }
}

/// RULE 3 — a policy is keyed by the whole channel address, so siblings and namesakes in
/// another group are unaffected.
#[tokio::test]
async fn a_policy_belongs_only_to_the_channel_it_was_attached_to() {
    let emulator = Emulator::new();
    // The same channel name in two groups, plus a sibling in the first: three distinct
    // addresses, two of which collide on one component each.
    let addresses = [
        ("group-a", "channel-1"),
        ("group-a", "channel-2"),
        ("group-b", "channel-1"),
    ];
    for group in ["group-a", "group-b"] {
        emulator.given_group(group).await;
    }
    for (group, channel) in addresses {
        emulator.given_channel(group, channel).await;
    }

    // Attach a distinct policy to one address at a time and check that only that
    // address reports one.
    for (attached_at, &(group, channel)) in addresses.iter().enumerate() {
        let policy =
            json!({"Statement": [{"Sid": format!("{group}-{channel}"), "Effect": "Allow"}]})
                .to_string();
        let (status, body) = emulator.put_policy(group, channel, &policy).await;
        assert_eq!(
            status, 200,
            "case {group:?}/{channel:?}: put expected 200, got {body}"
        );

        for (index, &(other_group, other_channel)) in addresses.iter().enumerate() {
            let (status, body) = emulator.get_policy(other_group, other_channel).await;
            if index <= attached_at {
                let expected = json!({
                    "Statement": [{"Sid": format!("{other_group}-{other_channel}"), "Effect": "Allow"}],
                })
                .to_string();
                assert_eq!(
                    status, 200,
                    "case {other_group:?}/{other_channel:?}: expected 200, got {body}"
                );
                assert_eq!(
                    member(&body, "Policy"),
                    expected,
                    "case {other_group:?}/{other_channel:?}: expected its own policy, got {body}"
                );
            } else {
                assert_eq!(
                    status, 404,
                    "case {other_group:?}/{other_channel:?}: expected 404 while unattached, got {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "ResourceNotFoundException",
                    "case {other_group:?}/{other_channel:?}: expected ResourceNotFoundException, got {body}"
                );
            }
        }
    }
}

/// RULE 4 — every absence on the read path is the same modeled 404.
#[tokio::test]
async fn a_policy_that_is_not_there_is_reported_as_not_found() {
    let emulator = Emulator::new();
    emulator.given_group("group-1").await;
    emulator.given_channel("group-1", "channel-1").await;
    emulator.given_channel("group-1", "recreated").await;
    let policy = json!({"Statement": [{"Effect": "Allow"}]}).to_string();
    let (status, body) = emulator.put_policy("group-1", "recreated", &policy).await;
    assert_eq!(status, 200, "setup: put expected 200, got {body}");
    // Deleting the channel takes its policy with it; a namesake created afterwards is a
    // different resource and starts with none.
    let (status, body) = emulator.delete_channel("group-1", "recreated").await;
    assert!(
        (200..300).contains(&status),
        "setup: deleting the channel expected 2xx, got {status} {body}"
    );
    emulator.given_channel("group-1", "recreated").await;

    let cases = [
        ("unknown channel group", "absent-group", "channel-1"),
        ("unknown channel", "group-1", "absent-channel"),
        ("channel with no policy ever attached", "group-1", "channel-1"),
        ("channel re-created under a used name", "group-1", "recreated"),
    ];
    for (label, group, channel) in cases {
        let (status, body) = emulator.get_policy(group, channel).await;
        assert_eq!(status, 404, "case {label:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        // The not-found text is fixed per resource type and carries none of the
        // caller's identifiers.
        let message = member(&body, "Message");
        for identifier in [group, channel] {
            assert!(
                !message.contains(identifier),
                "case {label:?}: the message must not echo {identifier:?}, got {message:?}"
            );
        }
    }
}

/// RULE 5 — a put whose parent chain does not resolve is a 404 and stores nothing.
#[tokio::test]
async fn a_put_against_an_unresolvable_address_stores_nothing() {
    let emulator = Emulator::new();
    emulator.given_group("group-1").await;
    emulator.given_channel("group-1", "channel-1").await;
    let policy = json!({"Statement": [{"Effect": "Allow"}]}).to_string();

    let cases = [
        ("unknown channel group", "absent-group", "channel-1"),
        ("unknown channel", "group-1", "absent-channel"),
        ("unknown channel in an unknown group", "absent-group", "absent-channel"),
    ];
    for (label, group, channel) in cases {
        let (status, body) = emulator.put_policy(group, channel, &policy).await;
        assert_eq!(status, 404, "case {label:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        let (status, body) = emulator.get_policy(group, channel).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the refused put must have stored nothing, got {status} {body}"
        );
    }
    // The one address that does resolve still has no policy: the refusals above wrote
    // nowhere, not merely not where they were addressed.
    let (status, body) = emulator.get_policy("group-1", "channel-1").await;
    assert_eq!(
        status, 404,
        "the resolvable channel must still carry no policy, got {status} {body}"
    );
}

/// RULE 6 — a document that is not a policy is refused with the modeled validation
/// error, and whatever was attached stays attached.
#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused() {
    let emulator = Emulator::new();
    emulator.given_group("group-1").await;
    emulator.given_channel("group-1", "channel-1").await;
    let attached = json!({"Statement": [{"Sid": "Keep", "Effect": "Allow"}]}).to_string();
    let (status, body) = emulator.put_policy("group-1", "channel-1", &attached).await;
    assert_eq!(status, 200, "setup: put expected 200, got {body}");

    // Rows differ in HOW the document fails to be a policy: not JSON at all, truncated
    // JSON, a JSON value of the wrong type, and a well-formed object with no statement.
    let cases = [
        ("empty", String::new()),
        ("not json at all", "this is not a policy".to_string()),
        ("truncated object", r#"{"Statement":[{"Effect":"Allow"}]"#.to_string()),
        ("json array", r#"[{"Statement":[{"Effect":"Allow"}]}]"#.to_string()),
        ("json string", json!("{\"Statement\":[]}").to_string()),
        ("json number", "17".to_string()),
        ("object without a statement", json!({"Version": "2012-10-17"}).to_string()),
    ];
    for (label, policy) in cases {
        let (status, body) = emulator.put_policy("group-1", "channel-1", &policy).await;
        assert_eq!(status, 400, "case {label:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            member(&body, "ValidationExceptionType"),
            "INVALID_POLICY",
            "case {label:?}: expected ValidationExceptionType INVALID_POLICY, got {body}"
        );

        let (status, body) = emulator.get_policy("group-1", "channel-1").await;
        assert_eq!(status, 200, "case {label:?}: expected 200, got {body}");
        assert_eq!(
            member(&body, "Policy"),
            attached,
            "case {label:?}: the refused put must leave the attached policy, got {body}"
        );
    }
}

/// RULE 7 — the `PolicyText` length bound: accepted at the maximum, refused past it.
#[tokio::test]
async fn the_policy_length_bound_is_enforced() {
    let emulator = Emulator::new();
    emulator.given_group("group-1").await;
    emulator.given_channel("group-1", "channel-1").await;

    for length in [POLICY_TEXT_MAX_LENGTH - 1, POLICY_TEXT_MAX_LENGTH] {
        let policy = policy_of_length(length);
        let (status, body) = emulator.put_policy("group-1", "channel-1", &policy).await;
        assert_eq!(
            status, 200,
            "case {length} bytes: expected 200 at or below the bound, got {status} {body}"
        );
        let (status, body) = emulator.get_policy("group-1", "channel-1").await;
        assert_eq!(status, 200, "case {length} bytes: get expected 200, got {body}");
        assert_eq!(
            member(&body, "Policy").len(),
            length,
            "case {length} bytes: expected the document back whole, got {} bytes",
            member(&body, "Policy").len()
        );
    }

    let too_long = policy_of_length(POLICY_TEXT_MAX_LENGTH + 1);
    let (status, body) = emulator.put_policy("group-1", "channel-1", &too_long).await;
    assert_eq!(
        status, 400,
        "case {} bytes: expected 400 past the bound, got {status} {body}",
        POLICY_TEXT_MAX_LENGTH + 1
    );
    assert_eq!(
        error_type(&body),
        "ValidationException",
        "case {} bytes: expected ValidationException, got {body}",
        POLICY_TEXT_MAX_LENGTH + 1
    );
}
