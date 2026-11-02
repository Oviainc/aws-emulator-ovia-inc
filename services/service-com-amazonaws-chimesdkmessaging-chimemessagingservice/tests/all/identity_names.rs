// SPDX-License-Identifier: CC0-1.0

//! Every `Identity` this service puts on the wire reports the NAME of the identity its
//! `Arn` names.
//!
//! The rule under test: an `Identity` member of a response is the ARN of the
//! `AppInstanceUser` or `AppInstanceBot` it stands for TOGETHER with the name that
//! identity is registered under — the name Chime SDK Identity publishes in the family's
//! shared, ARN-keyed identity directory — so a participant whose name this emulator holds
//! is never served as an ARN alone, and an identity no name was registered for is served
//! with none (the member is modelled optional, so nothing is invented for it).
//!
//! The rule is not about one participant or one operation, so every case drives a TABLE:
//! the names span what the modelled `ResourceName` allows (an ordinary name, the empty
//! name its `min 0` length permits, a name at its 256-character maximum, non-ASCII text)
//! plus an identity with no published name, and the assertions walk every `Identity` the
//! membership, ban, moderator, channel and message projections carry — `Member`,
//! `InvitedBy`, `CreatedBy`, `Moderator`, `ChannelModerator` and `Sender` — on both the
//! single-resource reads and the list rows.
//!
//! The directory is seeded here the way the identity service writes it, because this
//! crate's suite can only reach its own half of the hand-off; the cross-service suite
//! `core/router/tests/all/chime_identity_name_projection.rs` drives the other half, where
//! the name travels from a real `CreateAppInstanceUser`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use service_common::chime_identity_names::ChimeIdentityNameDirectory;
use smithy_server_runtime::MultiServiceRouter;

// ONE percent-encoder per test binary: the `@httpLabel` encoding rule lives in
// `channel_messaging.rs`, which this suite shares rather than copying.
use crate::channel_messaging::encode;

/// The `AppInstanceArn` every channel below is created under.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";

/// The ARN of one `AppInstanceUser` under [`APP_INSTANCE`].
fn user(id: &str) -> String {
    format!("{APP_INSTANCE}/user/{id}")
}

/// The names the table spans, each with the case it stands for.
///
/// `Identity$Name` targets `ResourceName`, which the model bounds at `length 0..256`
/// (`smithy_query(query="shape",shape="Identity")`), so the rows are an ordinary name, the
/// EMPTY name the lower bound allows, a name AT the upper bound, and one of non-ASCII text
/// — the `ResourceName` pattern admits the whole printable Unicode range.
fn name_rows() -> Vec<(&'static str, String)> {
    vec![
        ("an ordinary name", "Alice Example".to_string()),
        ("the empty name the shape's lower bound allows", String::new()),
        ("a name at the shape's 256-character maximum", "n".repeat(256)),
        ("a name of non-ASCII text", "Ría Ünicode ⛵".to_string()),
    ]
}

/// One response, reduced to the status and the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

/// The emulator under test: this one service mounted in the production router, with the
/// shared engine kept so a case can publish an identity's name the way the identity
/// service does.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router
            .set_request_authentication(false)
            .register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Publish `name` as the name of `identity_arn` in the family's shared directory —
    /// what Chime SDK Identity does when a caller registers or renames an identity.
    fn publish_name(&self, identity_arn: &str, name: &str) {
        self.engine
            .state::<ChimeIdentityNameDirectory>()
            .lock()
            .register(identity_arn, name);
    }

    async fn call(&self, bearer: &str, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let bytes = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the request")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://messaging-chime.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "messaging-chime.us-east-1.amazonaws.com")
            // The SigV4 credential scope is what routes a request to its service.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/chime/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("content-type", "application/json")
            .header("x-amz-chime-bearer", bearer)
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// Create a channel as `creator` and answer its ARN.
    async fn channel(&self, creator: &str, label: &str, token: &str) -> String {
        let reply = self
            .call(
                creator,
                "POST",
                "/channels",
                Some(&json!({
                    "AppInstanceArn": APP_INSTANCE,
                    "Name": label,
                    "Mode": "UNRESTRICTED",
                    "Privacy": "PUBLIC",
                    "ClientRequestToken": token,
                })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case {label:?}: CreateChannel must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        reply.body["ChannelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateChannel must return a ChannelArn"))
            .to_string()
    }
}

/// The `Name` of the `Identity` at `pointer`, or `None` when the identity carries none.
///
/// `Option<Option<&str>>`: the outer level says whether the response carried an `Identity`
/// there at all, so a missing member is not read as a missing name.
fn identity_name<'a>(body: &'a Value, pointer: &str) -> Option<Option<&'a str>> {
    let identity = body.pointer(pointer)?;
    Some(identity.get("Name").and_then(Value::as_str))
}

/// Assert that the `Identity` at `pointer` reports `arn` and `expected_name`.
fn assert_identity(case: &str, body: &Value, pointer: &str, arn: &str, expected_name: Option<&str>) {
    let actual_arn = body
        .pointer(pointer)
        .and_then(|identity| identity.get("Arn"))
        .and_then(Value::as_str);
    assert_eq!(
        actual_arn,
        Some(arn),
        "case {case:?}: expected the Identity at {pointer} to carry Arn {arn:?}, got \
         {actual_arn:?} in {body:?}"
    );
    let actual_name = identity_name(body, pointer);
    assert_eq!(
        actual_name,
        Some(expected_name),
        "case {case:?}: expected the Identity at {pointer} to carry Name {expected_name:?}, got \
         {actual_name:?} in {body:?}"
    );
}

#[tokio::test]
async fn every_membership_ban_and_moderator_identity_reports_the_published_name() {
    for (case, name) in name_rows() {
        let emulator = Emulator::new();
        let (creator, member, nameless) = (user("alice"), user("bob"), user("carol"));
        // The creator and the member have a published name; the third identity has none,
        // which is the row that proves the name is READ rather than derived from the ARN.
        emulator.publish_name(&creator, &format!("{name} (creator)"));
        emulator.publish_name(&member, &name);
        let creator_name = format!("{name} (creator)");

        let channel = emulator.channel(&creator, "names", "token-1").await;

        // CreateChannelMembership: `Member` carries the invited identity's name.
        let added = emulator
            .call(
                &creator,
                "POST",
                &format!("/channels/{}/memberships", encode(&channel)),
                Some(&json!({ "MemberArn": member, "Type": "DEFAULT" })),
            )
            .await;
        assert_eq!(
            added.status, 201,
            "case {case:?}: CreateChannelMembership must answer 201, got {} {:?}",
            added.status, added.body
        );
        assert_identity(
            case,
            &added.body,
            "/Member",
            &member,
            Some(name.as_str()),
        );

        // A member with no published name is added the same way and reports no name.
        let added_nameless = emulator
            .call(
                &creator,
                "POST",
                &format!("/channels/{}/memberships", encode(&channel)),
                Some(&json!({ "MemberArn": nameless, "Type": "DEFAULT" })),
            )
            .await;
        assert_eq!(
            added_nameless.status, 201,
            "case {case:?}: CreateChannelMembership for an unnamed identity must answer 201, got \
             {} {:?}",
            added_nameless.status, added_nameless.body
        );
        assert_identity(case, &added_nameless.body, "/Member", &nameless, None);

        // DescribeChannelMembership: BOTH identities on the membership.
        let described = emulator
            .call(
                &creator,
                "GET",
                &format!(
                    "/channels/{}/memberships/{}",
                    encode(&channel),
                    encode(&member)
                ),
                None,
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeChannelMembership must answer 200, got {} {:?}",
            described.status, described.body
        );
        assert_identity(
            case,
            &described.body,
            "/ChannelMembership/Member",
            &member,
            Some(name.as_str()),
        );
        assert_identity(
            case,
            &described.body,
            "/ChannelMembership/InvitedBy",
            &creator,
            Some(creator_name.as_str()),
        );

        // ListChannelMemberships: every row, including the creator's own membership and
        // the identity with no published name.
        let listed = emulator
            .call(
                &creator,
                "GET",
                &format!("/channels/{}/memberships", encode(&channel)),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListChannelMemberships must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let mut rows: Vec<(String, Option<String>)> = listed.body["ChannelMemberships"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ChannelMemberships must be a list"))
            .iter()
            .map(|row| {
                (
                    row["Member"]["Arn"].as_str().unwrap_or_default().to_string(),
                    row["Member"]["Name"].as_str().map(str::to_string),
                )
            })
            .collect();
        rows.sort();
        let mut expected = vec![
            (creator.clone(), Some(creator_name.clone())),
            (member.clone(), Some(name.clone())),
            (nameless.clone(), None),
        ];
        expected.sort();
        assert_eq!(
            rows, expected,
            "case {case:?}: expected every membership row to carry its published name, got \
             {rows:?}"
        );

        // CreateChannelModerator / DescribeChannelModerator / ListChannelModerators.
        let promoted = emulator
            .call(
                &creator,
                "POST",
                &format!("/channels/{}/moderators", encode(&channel)),
                Some(&json!({ "ChannelModeratorArn": member })),
            )
            .await;
        assert_eq!(
            promoted.status, 201,
            "case {case:?}: CreateChannelModerator must answer 201, got {} {:?}",
            promoted.status, promoted.body
        );
        assert_identity(
            case,
            &promoted.body,
            "/ChannelModerator",
            &member,
            Some(name.as_str()),
        );

        let described_moderator = emulator
            .call(
                &creator,
                "GET",
                &format!(
                    "/channels/{}/moderators/{}",
                    encode(&channel),
                    encode(&member)
                ),
                None,
            )
            .await;
        assert_eq!(
            described_moderator.status, 200,
            "case {case:?}: DescribeChannelModerator must answer 200, got {} {:?}",
            described_moderator.status, described_moderator.body
        );
        assert_identity(
            case,
            &described_moderator.body,
            "/ChannelModerator/Moderator",
            &member,
            Some(name.as_str()),
        );
        assert_identity(
            case,
            &described_moderator.body,
            "/ChannelModerator/CreatedBy",
            &creator,
            Some(creator_name.as_str()),
        );

        let listed_moderators = emulator
            .call(
                &creator,
                "GET",
                &format!("/channels/{}/moderators", encode(&channel)),
                None,
            )
            .await;
        let mut moderator_rows: Vec<(String, Option<String>)> = listed_moderators.body
            ["ChannelModerators"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ChannelModerators must be a list"))
            .iter()
            .map(|row| {
                (
                    row["Moderator"]["Arn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    row["Moderator"]["Name"].as_str().map(str::to_string),
                )
            })
            .collect();
        moderator_rows.sort();
        let mut expected_moderators = vec![
            (creator.clone(), Some(creator_name.clone())),
            (member.clone(), Some(name.clone())),
        ];
        expected_moderators.sort();
        assert_eq!(
            moderator_rows, expected_moderators,
            "case {case:?}: expected every moderator row to carry its published name, got \
             {moderator_rows:?}"
        );

        // CreateChannelBan / DescribeChannelBan / ListChannelBans, banning the identity
        // with no published name beside the named one.
        for (banned, banned_name) in [
            (member.clone(), Some(name.clone())),
            (nameless.clone(), None),
        ] {
            let banned_reply = emulator
                .call(
                    &creator,
                    "POST",
                    &format!("/channels/{}/bans", encode(&channel)),
                    Some(&json!({ "MemberArn": banned })),
                )
                .await;
            assert_eq!(
                banned_reply.status, 201,
                "case {case:?}: CreateChannelBan must answer 201, got {} {:?}",
                banned_reply.status, banned_reply.body
            );
            assert_identity(
                case,
                &banned_reply.body,
                "/Member",
                &banned,
                banned_name.as_deref(),
            );

            let described_ban = emulator
                .call(
                    &creator,
                    "GET",
                    &format!("/channels/{}/bans/{}", encode(&channel), encode(&banned)),
                    None,
                )
                .await;
            assert_eq!(
                described_ban.status, 200,
                "case {case:?}: DescribeChannelBan must answer 200, got {} {:?}",
                described_ban.status, described_ban.body
            );
            assert_identity(
                case,
                &described_ban.body,
                "/ChannelBan/Member",
                &banned,
                banned_name.as_deref(),
            );
            assert_identity(
                case,
                &described_ban.body,
                "/ChannelBan/CreatedBy",
                &creator,
                Some(creator_name.as_str()),
            );
        }

        let listed_bans = emulator
            .call(
                &creator,
                "GET",
                &format!("/channels/{}/bans", encode(&channel)),
                None,
            )
            .await;
        let mut ban_rows: Vec<(String, Option<String>)> = listed_bans.body["ChannelBans"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ChannelBans must be a list"))
            .iter()
            .map(|row| {
                (
                    row["Member"]["Arn"].as_str().unwrap_or_default().to_string(),
                    row["Member"]["Name"].as_str().map(str::to_string),
                )
            })
            .collect();
        ban_rows.sort();
        let mut expected_bans = vec![
            (member.clone(), Some(name.clone())),
            (nameless.clone(), None),
        ];
        expected_bans.sort();
        assert_eq!(
            ban_rows, expected_bans,
            "case {case:?}: expected every ban row to carry its published name, got {ban_rows:?}"
        );
    }
}

#[tokio::test]
async fn the_channel_and_message_identities_report_the_published_name() {
    for (case, name) in name_rows() {
        let emulator = Emulator::new();
        let (sender, nameless) = (user("alice"), user("carol"));
        emulator.publish_name(&sender, &name);

        for (index, (label, bearer, expected)) in [
            ("a sender with a published name", sender.clone(), Some(name.clone())),
            ("a sender with no published name", nameless.clone(), None),
        ]
        .into_iter()
        .enumerate()
        {
            let channel = emulator
                .channel(&bearer, "names", &format!("token-{index}"))
                .await;

            // DescribeChannel: `CreatedBy` is the creating identity.
            let described = emulator.call(&bearer, "GET", &format!("/channels/{}", encode(&channel)), None).await;
            assert_eq!(
                described.status, 200,
                "case {case:?} / {label:?}: DescribeChannel must answer 200, got {} {:?}",
                described.status, described.body
            );
            assert_identity(
                case,
                &described.body,
                "/Channel/CreatedBy",
                &bearer,
                expected.as_deref(),
            );

            // SendChannelMessage then GetChannelMessage / ListChannelMessages: `Sender`.
            let sent = emulator
                .call(
                    &bearer,
                    "POST",
                    &format!("/channels/{}/messages", encode(&channel)),
                    Some(&json!({
                        "Content": "hello",
                        "Type": "STANDARD",
                        "Persistence": "PERSISTENT",
                        "ClientRequestToken": "message-token",
                    })),
                )
                .await;
            assert_eq!(
                sent.status, 201,
                "case {case:?} / {label:?}: SendChannelMessage must answer 201, got {} {:?}",
                sent.status, sent.body
            );
            let message_id = sent.body["MessageId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?} / {label:?}: SendChannelMessage must return a MessageId")
                })
                .to_string();

            let got = emulator
                .call(
                    &bearer,
                    "GET",
                    &format!(
                        "/channels/{}/messages/{}",
                        encode(&channel),
                        encode(&message_id)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                got.status, 200,
                "case {case:?} / {label:?}: GetChannelMessage must answer 200, got {} {:?}",
                got.status, got.body
            );
            assert_identity(
                case,
                &got.body,
                "/ChannelMessage/Sender",
                &bearer,
                expected.as_deref(),
            );

            let listed = emulator
                .call(
                    &bearer,
                    "GET",
                    &format!("/channels/{}/messages", encode(&channel)),
                    None,
                )
                .await;
            let rows: Vec<(Option<&str>, Option<&str>)> = listed.body["ChannelMessages"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?} / {label:?}: ChannelMessages must be a list")
                })
                .iter()
                .map(|row| {
                    (
                        row["Sender"]["Arn"].as_str(),
                        row["Sender"]["Name"].as_str(),
                    )
                })
                .collect();
            assert_eq!(
                rows,
                vec![(Some(bearer.as_str()), expected.as_deref())],
                "case {case:?} / {label:?}: expected the listed message's Sender to carry its \
                 published name, got {rows:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_renamed_identity_is_reported_under_its_current_name() {
    // The name travels with the identity, not with the membership: the projection reads
    // the directory when the response is built, so a rename published after the
    // membership was written is what the next read reports.
    let emulator = Emulator::new();
    let (creator, member) = (user("alice"), user("bob"));
    emulator.publish_name(&creator, "Creator");
    emulator.publish_name(&member, "Before");

    let channel = emulator.channel(&creator, "renames", "token-1").await;
    let added = emulator
        .call(
            &creator,
            "POST",
            &format!("/channels/{}/memberships", encode(&channel)),
            Some(&json!({ "MemberArn": member, "Type": "DEFAULT" })),
        )
        .await;
    assert_identity("before the rename", &added.body, "/Member", &member, Some("Before"));

    // Rows: a rename, and then the identity's deletion — which drops the published name,
    // so the participant is reported with none rather than with a stale one.
    emulator.publish_name(&member, "After");
    let described = emulator
        .call(
            &creator,
            "GET",
            &format!(
                "/channels/{}/memberships/{}",
                encode(&channel),
                encode(&member)
            ),
            None,
        )
        .await;
    assert_identity(
        "after the rename",
        &described.body,
        "/ChannelMembership/Member",
        &member,
        Some("After"),
    );

    emulator
        .engine
        .state::<ChimeIdentityNameDirectory>()
        .lock()
        .forget(&member);
    let after_delete = emulator
        .call(
            &creator,
            "GET",
            &format!(
                "/channels/{}/memberships/{}",
                encode(&channel),
                encode(&member)
            ),
            None,
        )
        .await;
    assert_identity(
        "after the identity was deleted",
        &after_delete.body,
        "/ChannelMembership/Member",
        &member,
        None,
    );
}
