// SPDX-License-Identifier: CC0-1.0

//! The NAME of an app instance's identity across TWO services: it is registered through
//! Chime SDK Identity and it is REPORTED by Chime SDK Messaging.
//!
//! The rule under test: every `Identity` Chime SDK Messaging puts on the wire — a channel
//! member, the member that invited it, a banned member, a moderator, the caller that
//! created either, a channel's creator, a message's sender — carries the name the
//! `AppInstanceUser` or `AppInstanceBot` its `Arn` names is registered under in Chime SDK
//! Identity, and reports no name for an identity nothing registered. A rename through
//! `UpdateAppInstanceUser` is what later reads report, and deleting the identity leaves no
//! name behind.
//!
//! This suite needs BOTH services on one engine: the name is written by one crate and
//! reported by another, and the point under test is that the two agree about what an
//! identity is called. Each crate's own suite can only reach its half — the messaging
//! crate's `identity_names` suite drives every projection against a published name, and
//! the identity crate's suites pin the registration itself — so neither can prove the
//! hand-off.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name, from the
/// router suite that already states that rule — one `Authorization` builder per test
/// binary. Every Chime SDK service signs as `chime`.
use crate::rds_docdb_neptune_signing_name_routing::authorization;

// Percent-encoding for one URI path segment, as an AWS SDK encodes an `@httpLabel`:
// `all/cloudfront_key_value_store_handshake.rs` states that codec once for this test
// binary, so this suite shares it rather than carrying another copy.
use crate::cloudfront_key_value_store_handshake::segment as encode;

/// The Chime SDK Identity endpoint host.
const IDENTITY_HOST: &str = "identity-chime.us-east-1.amazonaws.com";
/// The Chime SDK Messaging endpoint host.
const MESSAGING_HOST: &str = "messaging-chime.us-east-1.amazonaws.com";

/// Chime SDK Identity and Chime SDK Messaging, mounted on ONE engine, exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_identity_and_messaging_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_chimesdkidentity_chimeidentityservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One answer, reduced to the status and the JSON body.
struct Reply {
    status: u16,
    body: Value,
}

/// Dispatch one request to `host`, optionally as the `x-amz-chime-bearer` identity.
async fn call(
    router: &MultiServiceRouter,
    host: &str,
    bearer: Option<&str>,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = match body {
        Some(value) => Bytes::from(serde_json::to_vec(&value).expect("serialize the body")),
        None => Bytes::new(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path_and_query}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization("chime"));
    if let Some(bearer) = bearer {
        builder = builder.header("x-amz-chime-bearer", bearer);
    }
    let request = builder.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Reply { status, body }
}

/// The `Name` the `Identity` at `pointer` carries, or `None` when it carries none.
fn identity_name<'a>(body: &'a Value, pointer: &str) -> Option<&'a str> {
    body.pointer(pointer)
        .and_then(|identity| identity.get("Name"))
        .and_then(Value::as_str)
}

/// Assert the `Identity` at `pointer` reports `arn` and `expected`.
fn assert_identity(case: &str, body: &Value, pointer: &str, arn: &str, expected: Option<&str>) {
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
    let actual = identity_name(body, pointer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the Identity at {pointer} to carry Name {expected:?}, got \
         {actual:?} in {body:?}"
    );
}

#[tokio::test]
async fn a_messaging_participants_name_is_the_one_chime_sdk_identity_registered() {
    let router = build_identity_and_messaging_router();

    // Chime SDK Identity: one app instance.
    let created = call(
        &router,
        IDENTITY_HOST,
        None,
        "POST",
        "/app-instances",
        Some(json!({ "Name": "name-crossing", "ClientRequestToken": "instance-token" })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateAppInstance must answer 201, got {} {:?}",
        created.status, created.body
    );
    let app_instance = created.body["AppInstanceArn"]
        .as_str()
        .expect("CreateAppInstance returns an AppInstanceArn")
        .to_string();

    // Three identities, spanning what `CreateAppInstanceUser`'s `Name` (modelled `UserName`,
    // `length 1..100`, pattern `\S` — `smithy_query(query="operation",
    // operation="CreateAppInstanceUser")`) allows: an ordinary name, one at that
    // 100-character maximum, and one of non-ASCII text.
    let rows: [(&str, &str, String); 3] = [
        ("an ordinary registered name", "creator", "Ada Creator".to_string()),
        ("a name at the modelled 100-character maximum", "member", "m".repeat(100)),
        ("a registered name of non-ASCII text", "watcher", "Ría Ünicode ⛵".to_string()),
    ];
    let mut arns = Vec::new();
    for (case, user_id, name) in &rows {
        let user = call(
            &router,
            IDENTITY_HOST,
            None,
            "POST",
            "/app-instance-users",
            Some(json!({
                "AppInstanceArn": app_instance,
                "AppInstanceUserId": user_id,
                "Name": name,
                "ClientRequestToken": format!("user-token-{user_id}"),
            })),
        )
        .await;
        assert_eq!(
            user.status, 201,
            "case {case:?}: CreateAppInstanceUser must answer 201, got {} {:?}",
            user.status, user.body
        );
        arns.push(
            user.body["AppInstanceUserArn"]
                .as_str()
                .expect("CreateAppInstanceUser returns an AppInstanceUserArn")
                .to_string(),
        );
    }
    let (creator, member, watcher) = (arns[0].clone(), arns[1].clone(), arns[2].clone());
    let (creator_name, member_name, watcher_name) = (
        rows[0].2.clone(),
        rows[1].2.clone(),
        rows[2].2.clone(),
    );

    // One `AppInstanceBot`, whose `Name` is modelled `ResourceName` rather than `UserName`
    // — so a bot is the identity kind whose name may be as long as `Identity$Name` itself
    // allows (`length 0..256`).
    let bot_name = "b".repeat(256);
    let bot = call(
        &router,
        IDENTITY_HOST,
        None,
        "POST",
        "/app-instance-bots",
        Some(json!({
            "AppInstanceArn": app_instance,
            "Name": bot_name,
            "ClientRequestToken": "bot-token",
            "Configuration": {
                "Lex": {
                    "LexBotAliasArn": "arn:aws:lex:us-east-1:111122223333:bot-alias/AB1CD2EFGH/IJ3KL4MNOP",
                    "LocaleId": "en_US",
                    "RespondsTo": "STANDARD_MESSAGES",
                }
            },
        })),
    )
    .await;
    assert_eq!(
        bot.status, 201,
        "CreateAppInstanceBot must answer 201, got {} {:?}",
        bot.status, bot.body
    );
    let bot_arn = bot.body["AppInstanceBotArn"]
        .as_str()
        .expect("CreateAppInstanceBot returns an AppInstanceBotArn")
        .to_string();

    // An identity Chime SDK Identity never registered: the row that proves the name is
    // READ from the registration rather than derived from the ARN.
    let unregistered = format!("{app_instance}/user/never-registered");

    // Chime SDK Messaging: a channel created by `creator`, with the other identities added.
    let channel = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "POST",
        "/channels",
        Some(json!({
            "AppInstanceArn": app_instance,
            "Name": "named-participants",
            "Privacy": "PUBLIC",
            "ClientRequestToken": "channel-token",
        })),
    )
    .await;
    assert_eq!(
        channel.status, 201,
        "CreateChannel must answer 201, got {} {:?}",
        channel.status, channel.body
    );
    let channel_arn = channel.body["ChannelArn"]
        .as_str()
        .expect("CreateChannel returns a ChannelArn")
        .to_string();

    // DescribeChannel reports the creator's registered name.
    let described = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "GET",
        &format!("/channels/{}", encode(&channel_arn)),
        None,
    )
    .await;
    assert_identity(
        "the channel's creator",
        &described.body,
        "/Channel/CreatedBy",
        &creator,
        Some(creator_name.as_str()),
    );

    // Each added member's `Member` carries that identity's registered name, and the one
    // nothing registered carries none.
    for (case, arn, expected) in [
        ("a user registered with a name", member.clone(), Some(member_name.clone())),
        ("a bot registered with a name", bot_arn.clone(), Some(bot_name.clone())),
        ("a user with a name of non-ASCII text", watcher.clone(), Some(watcher_name.clone())),
        ("an identity nothing registered", unregistered.clone(), None),
    ] {
        let added = call(
            &router,
            MESSAGING_HOST,
            Some(&creator),
            "POST",
            &format!("/channels/{}/memberships", encode(&channel_arn)),
            Some(json!({ "MemberArn": arn, "Type": "DEFAULT" })),
        )
        .await;
        assert_eq!(
            added.status, 201,
            "case {case:?}: CreateChannelMembership must answer 201, got {} {:?}",
            added.status, added.body
        );
        assert_identity(case, &added.body, "/Member", &arn, expected.as_deref());

        // And the read-back through DescribeChannelMembership, where `InvitedBy` is the
        // creator — a second `Identity` on the same response.
        let membership = call(
            &router,
            MESSAGING_HOST,
            Some(&creator),
            "GET",
            &format!(
                "/channels/{}/memberships/{}",
                encode(&channel_arn),
                encode(&arn)
            ),
            None,
        )
        .await;
        assert_eq!(
            membership.status, 200,
            "case {case:?}: DescribeChannelMembership must answer 200, got {} {:?}",
            membership.status, membership.body
        );
        assert_identity(
            case,
            &membership.body,
            "/ChannelMembership/Member",
            &arn,
            expected.as_deref(),
        );
        assert_identity(
            case,
            &membership.body,
            "/ChannelMembership/InvitedBy",
            &creator,
            Some(creator_name.as_str()),
        );
    }

    // A message's `Sender` is the same join, on the message projections.
    let sent = call(
        &router,
        MESSAGING_HOST,
        Some(&member),
        "POST",
        &format!("/channels/{}/messages", encode(&channel_arn)),
        Some(json!({
            "Content": "hello",
            "Type": "STANDARD",
            "Persistence": "PERSISTENT",
            "ClientRequestToken": "message-token",
        })),
    )
    .await;
    assert_eq!(
        sent.status, 201,
        "SendChannelMessage must answer 201, got {} {:?}",
        sent.status, sent.body
    );
    let message_id = sent.body["MessageId"]
        .as_str()
        .expect("SendChannelMessage returns a MessageId")
        .to_string();
    let message = call(
        &router,
        MESSAGING_HOST,
        Some(&member),
        "GET",
        &format!(
            "/channels/{}/messages/{}",
            encode(&channel_arn),
            encode(&message_id)
        ),
        None,
    )
    .await;
    assert_identity(
        "a message's sender",
        &message.body,
        "/ChannelMessage/Sender",
        &member,
        Some(member_name.as_str()),
    );

    // A RENAME through Chime SDK Identity is what the next messaging read reports: the name
    // belongs to the identity, not to the membership that was written earlier.
    let renamed = call(
        &router,
        IDENTITY_HOST,
        None,
        "PUT",
        &format!("/app-instance-users/{}", encode(&member)),
        Some(json!({ "Name": "Renamed Member", "Metadata": "" })),
    )
    .await;
    assert_eq!(
        renamed.status, 200,
        "UpdateAppInstanceUser must answer 200, got {} {:?}",
        renamed.status, renamed.body
    );
    let after_rename = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "GET",
        &format!("/channels/{}/memberships", encode(&channel_arn)),
        None,
    )
    .await;
    let renamed_row = after_rename.body["ChannelMemberships"]
        .as_array()
        .expect("ChannelMemberships must be a list")
        .iter()
        .find(|row| row["Member"]["Arn"].as_str() == Some(member.as_str()))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        renamed_row["Member"]["Name"].as_str(),
        Some("Renamed Member"),
        "case {:?}: expected the listed membership to report the new name, got {:?}",
        "after the rename",
        renamed_row
    );

    // Promoting a moderator and banning a member put the same join on two more shapes.
    let promoted = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "POST",
        &format!("/channels/{}/moderators", encode(&channel_arn)),
        Some(json!({ "ChannelModeratorArn": watcher })),
    )
    .await;
    assert_identity(
        "a promoted moderator",
        &promoted.body,
        "/ChannelModerator",
        &watcher,
        Some(watcher_name.as_str()),
    );
    let moderator = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "GET",
        &format!(
            "/channels/{}/moderators/{}",
            encode(&channel_arn),
            encode(&watcher)
        ),
        None,
    )
    .await;
    assert_identity(
        "a described moderator",
        &moderator.body,
        "/ChannelModerator/Moderator",
        &watcher,
        Some(watcher_name.as_str()),
    );
    assert_identity(
        "the caller that promoted the moderator",
        &moderator.body,
        "/ChannelModerator/CreatedBy",
        &creator,
        Some(creator_name.as_str()),
    );

    let banned = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "POST",
        &format!("/channels/{}/bans", encode(&channel_arn)),
        Some(json!({ "MemberArn": bot_arn })),
    )
    .await;
    assert_identity(
        "a banned bot",
        &banned.body,
        "/Member",
        &bot_arn,
        Some(bot_name.as_str()),
    );
    let ban = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "GET",
        &format!("/channels/{}/bans/{}", encode(&channel_arn), encode(&bot_arn)),
        None,
    )
    .await;
    assert_identity(
        "a described ban's member",
        &ban.body,
        "/ChannelBan/Member",
        &bot_arn,
        Some(bot_name.as_str()),
    );
    assert_identity(
        "the caller that created the ban",
        &ban.body,
        "/ChannelBan/CreatedBy",
        &creator,
        Some(creator_name.as_str()),
    );

    // DELETING the identity through Chime SDK Identity leaves no name behind: the
    // participant is still on the channel and is reported with its ARN alone.
    let deleted = call(
        &router,
        IDENTITY_HOST,
        None,
        "DELETE",
        &format!("/app-instance-users/{}", encode(&watcher)),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "DeleteAppInstanceUser must answer 204, got {} {:?}",
        deleted.status, deleted.body
    );
    let after_delete = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "GET",
        &format!(
            "/channels/{}/moderators/{}",
            encode(&channel_arn),
            encode(&watcher)
        ),
        None,
    )
    .await;
    assert_identity(
        "after the identity was deleted",
        &after_delete.body,
        "/ChannelModerator/Moderator",
        &watcher,
        None,
    );
}
