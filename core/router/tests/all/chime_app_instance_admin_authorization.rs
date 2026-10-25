// SPDX-License-Identifier: CC0-1.0

//! The `AppInstanceAdmin` role across TWO services: it is granted by Chime SDK Identity
//! and it is READ by Chime SDK Messaging.
//!
//! The rule under test: Chime SDK Messaging reserves two actions for an administrator of
//! the app instance — listing an account's PRIVATE channels (`ListChannels` with
//! `privacy=PRIVATE`, which the model says only an `AppInstanceAdmin` may ask for) and
//! deleting a channel message (`DeleteChannelMessage`, which the model says only admins
//! can perform) — and the only identities that hold that role are the ones Chime SDK
//! Identity promoted with `CreateAppInstanceAdmin`. A channel's creator, which is its
//! first member and its first moderator, does not hold it, and neither does an identity
//! with no role at all.
//!
//! This suite needs BOTH services on one engine: the promotion is written by one crate
//! and judged by another, and the point under test is that the two agree about who
//! administers the app instance. Each crate's own suite can only reach its half — the
//! identity crate's `app_instance_admin` suite pins the promotion and its read-back, and
//! the messaging crate's `channel_membership` suite pins the refusal every unpromoted
//! role gets — so neither can prove the hand-off.

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

impl Reply {
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
    }
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

/// The Chime SDK Identity endpoint host.
const IDENTITY_HOST: &str = "identity-chime.us-east-1.amazonaws.com";
/// The Chime SDK Messaging endpoint host.
const MESSAGING_HOST: &str = "messaging-chime.us-east-1.amazonaws.com";

#[tokio::test]
async fn only_an_identity_promoted_by_chime_sdk_identity_administers_a_messaging_app_instance() {
    let router = build_identity_and_messaging_router();

    // Chime SDK Identity: one app instance and three identities under it.
    let created = call(
        &router,
        IDENTITY_HOST,
        None,
        "POST",
        "/app-instances",
        Some(json!({ "Name": "admin-crossing", "ClientRequestToken": "instance-token" })),
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

    let mut users = Vec::new();
    for user_id in ["admin", "creator", "outsider"] {
        let user = call(
            &router,
            IDENTITY_HOST,
            None,
            "POST",
            "/app-instance-users",
            Some(json!({
                "AppInstanceArn": app_instance,
                "AppInstanceUserId": user_id,
                "Name": user_id,
                "ClientRequestToken": format!("user-token-{user_id}"),
            })),
        )
        .await;
        assert_eq!(
            user.status, 201,
            "CreateAppInstanceUser({user_id}) must answer 201, got {} {:?}",
            user.status, user.body
        );
        users.push(
            user.body["AppInstanceUserArn"]
                .as_str()
                .expect("CreateAppInstanceUser returns an AppInstanceUserArn")
                .to_string(),
        );
    }
    let (admin, creator, outsider) = (users[0].clone(), users[1].clone(), users[2].clone());

    // Chime SDK Messaging: a private channel and a public one with a message in it, both
    // created by `creator` — which makes it their first member and first moderator.
    let mut channels = Vec::new();
    for (name, privacy) in [("secret", "PRIVATE"), ("open", "PUBLIC")] {
        let channel = call(
            &router,
            MESSAGING_HOST,
            Some(&creator),
            "POST",
            "/channels",
            Some(json!({
                "AppInstanceArn": app_instance,
                "Name": name,
                "Privacy": privacy,
                "ClientRequestToken": format!("channel-token-{name}"),
            })),
        )
        .await;
        assert_eq!(
            channel.status, 201,
            "CreateChannel({name}) must answer 201, got {} {:?}",
            channel.status, channel.body
        );
        channels.push(
            channel.body["ChannelArn"]
                .as_str()
                .expect("CreateChannel returns a ChannelArn")
                .to_string(),
        );
    }
    let (private_channel, public_channel) = (channels[0].clone(), channels[1].clone());

    let sent = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "POST",
        &format!("/channels/{}/messages", encode(&public_channel)),
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

    // Before any promotion, NO identity administers the app instance, so both
    // administrator-only actions are refused for every role on the channel.
    let list_private = |bearer: String| {
        let router = &router;
        let app_instance = app_instance.clone();
        async move {
            call(
                router,
                MESSAGING_HOST,
                Some(&bearer),
                "GET",
                &format!(
                    "/channels?app-instance-arn={}&privacy=PRIVATE",
                    encode(&app_instance)
                ),
                None,
            )
            .await
        }
    };
    let delete_message = |bearer: String| {
        let router = &router;
        let public_channel = public_channel.clone();
        let message_id = message_id.clone();
        async move {
            call(
                router,
                MESSAGING_HOST,
                Some(&bearer),
                "DELETE",
                &format!(
                    "/channels/{}/messages/{}",
                    encode(&public_channel),
                    encode(&message_id)
                ),
                None,
            )
            .await
        }
    };

    for (label, bearer) in [
        ("the channel's creator, its first moderator", creator.clone()),
        ("an identity with no role at all", outsider.clone()),
        ("the identity about to be promoted", admin.clone()),
    ] {
        let listed = list_private(bearer.clone()).await;
        assert_eq!(
            listed.status, 403,
            "case {label:?}: listing PRIVATE channels before any promotion must answer 403, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.error_code(),
            "ForbiddenException",
            "case {label:?}: wrong error code, body {:?}",
            listed.body
        );
        let deleted = delete_message(bearer).await;
        assert_eq!(
            deleted.status, 403,
            "case {label:?}: deleting a message before any promotion must answer 403, got {} {:?}",
            deleted.status, deleted.body
        );
    }

    // Chime SDK Identity promotes ONE of them.
    let promoted = call(
        &router,
        IDENTITY_HOST,
        None,
        "POST",
        &format!("/app-instances/{}/admins", encode(&app_instance)),
        Some(json!({ "AppInstanceAdminArn": admin })),
    )
    .await;
    assert_eq!(
        promoted.status, 201,
        "CreateAppInstanceAdmin must answer 201, got {} {:?}",
        promoted.status, promoted.body
    );

    // The promoted identity now holds both administrator-only actions, and nobody else
    // does.
    let as_admin = list_private(admin.clone()).await;
    assert_eq!(
        as_admin.status, 200,
        "case {:?}: an administrator must be served the PRIVATE listing, got {} {:?}",
        "promoted identity", as_admin.status, as_admin.body
    );
    let listed_arns: Vec<&str> = as_admin.body["Channels"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["ChannelArn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed_arns,
        vec![private_channel.as_str()],
        "case {:?}: the PRIVATE listing must report the private channel, body {:?}",
        "promoted identity",
        as_admin.body
    );

    for (label, bearer) in [
        ("the channel's creator", creator.clone()),
        ("an identity with no role at all", outsider.clone()),
    ] {
        let listed = list_private(bearer).await;
        assert_eq!(
            listed.status, 403,
            "case {label:?}: a promotion of ANOTHER identity must not serve this one, got {} {:?}",
            listed.status, listed.body
        );
    }

    let deleted_by_admin = delete_message(admin.clone()).await;
    assert_eq!(
        deleted_by_admin.status, 204,
        "case {:?}: an administrator must be able to delete a channel message, got {} {:?}",
        "promoted identity", deleted_by_admin.status, deleted_by_admin.body
    );
    // Deletion makes the message inaccessible immediately, for every later reader.
    let read_back = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "GET",
        &format!(
            "/channels/{}/messages/{}",
            encode(&public_channel),
            encode(&message_id)
        ),
        None,
    )
    .await;
    assert_eq!(
        read_back.status, 403,
        "case {:?}: a deleted message must not be readable, got {} {:?}",
        "after the delete", read_back.status, read_back.body
    );
    let listed_messages = call(
        &router,
        MESSAGING_HOST,
        Some(&creator),
        "GET",
        &format!("/channels/{}/messages", encode(&public_channel)),
        None,
    )
    .await;
    assert_eq!(
        listed_messages.body["ChannelMessages"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case {:?}: a deleted message must not be listed, body {:?}",
        "after the delete",
        listed_messages.body
    );
}
