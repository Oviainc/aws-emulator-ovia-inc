// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal an operation owes when it cannot resolve the CHANNEL
//! RESOURCE its request names for the calling `x-amz-chime-bearer`.
//!
//! RULE under test: resolving a channel resource for the bearer is an ACCESS decision, so
//! an ARN this service holds no channel for, a channel the bearer may not read, and a
//! membership the bearer cannot resolve are all the modelled `ForbiddenException` — HTTP
//! 403, `Code` `Forbidden`, `x-amzn-errortype` `ForbiddenException` — carrying the fixed
//! sentence the service words it with, which names no value from the request. No operation
//! answers an error its own model does not declare: all but one of the operations below
//! declare no `NotFoundException` at all, and the two that report a channel ATTRIBUTE a
//! resolved channel does not hold to a permitted reader (a ban, a moderator) answer the
//! modelled 404 with their own fixed sentence.
//!
//! Every case drives real HTTP through the production router — restJson1 on the wire both
//! ways — and the table spans what the rule generalises over: the operation (every one
//! this slice implements that resolves a channel resource: reads, lists, creates, updates
//! and deletes), the reason the resource cannot be resolved (never created, deleted, the
//! sub-resource absent, the channel unreadable, the caller banned), and the HTTP method.

use serde_json::{json, Value};

// ONE bearer-aware harness and ONE percent-encoder per test binary: the suites in
// `tests/all.rs` share `channel_membership.rs`'s `Emulator`/`Reply` and
// `channel_messaging.rs`'s `@httpLabel` encoder rather than copying either.
use crate::channel_membership::{
    Emulator, APP_INSTANCE, BANNED, CREATOR, INSUFFICIENT_ACCESS, MEMBER, OUTSIDER,
};
use crate::channel_messaging::encode;

/// The `Message` a read of a ban the channel does not hold carries.
const NOT_BANNED: &str = "App instance user is not banned from the specified Channel.";
/// The `Message` a read of a moderator the channel does not hold carries.
const NOT_A_MODERATOR: &str = "App instance user is not a moderator of the specified Channel.";

/// A message id nothing ever sent, used where the case is about another resource.
const ABSENT_MESSAGE_ID: &str = "no-such-message";

/// One row of the table: the request, and the refusal the rule owes it.
struct Case {
    label: &'static str,
    bearer: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    error_type: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn a_channel_resource_the_bearer_cannot_resolve_is_refused_as_forbidden() {
    let emulator = Emulator::new();

    // A live PUBLIC channel with one member, so a case can ask for a sub-resource the
    // channel does not hold while the channel itself resolves.
    let live = emulator
        .channel("live", "live-token", "RESTRICTED", "PUBLIC")
        .await;
    let added = emulator
        .call(
            CREATOR,
            "POST",
            &format!("/channels/{}/memberships", encode(&live)),
            Some(&json!({ "MemberArn": MEMBER, "Type": "DEFAULT" })),
        )
        .await;
    assert_eq!(
        added.status, 201,
        "seeding the member must answer 201, got {} {:?}",
        added.status, added.body
    );

    // A PRIVATE channel the outsider is not in, so the gate's "unreadable" leg is covered.
    let private = emulator
        .channel("private", "private-token", "RESTRICTED", "PRIVATE")
        .await;

    // A channel created and then deleted, so the gate's "no longer held" leg is covered
    // with an ARN this service itself minted.
    let deleted = emulator
        .channel("deleted", "deleted-token", "RESTRICTED", "PUBLIC")
        .await;
    let removed = emulator
        .call(
            CREATOR,
            "DELETE",
            &format!("/channels/{}", encode(&deleted)),
            None,
        )
        .await;
    assert_eq!(
        removed.status, 204,
        "seeding the deleted channel: DeleteChannel must answer 204, got {} {:?}",
        removed.status, removed.body
    );

    // A channel this service never minted at all.
    let never = format!("{APP_INSTANCE}/channel/never-created");

    // A channel whose other member is BANNED, so the ban's own sentence is covered.
    let banning = emulator
        .channel("banning", "ban-token", "RESTRICTED", "PUBLIC")
        .await;
    for (what, reply) in [
        (
            "membership",
            emulator
                .call(
                    CREATOR,
                    "POST",
                    &format!("/channels/{}/memberships", encode(&banning)),
                    Some(&json!({ "MemberArn": MEMBER, "Type": "DEFAULT" })),
                )
                .await,
        ),
        (
            "ban",
            emulator
                .call(
                    CREATOR,
                    "POST",
                    &format!("/channels/{}/bans", encode(&banning)),
                    Some(&json!({ "MemberArn": MEMBER })),
                )
                .await,
        ),
    ] {
        assert_eq!(
            reply.status, 201,
            "seeding the {what} must answer 201, got {} {:?}",
            reply.status, reply.body
        );
    }

    let send_body = json!({
        "Content": "into the void",
        "Type": "STANDARD",
        "Persistence": "PERSISTENT",
        "ClientRequestToken": "void-token",
    });
    let cases = vec![
        // ---- the channel ARN resolves to no channel this service holds ----
        Case {
            label: "DescribeChannel, never created",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}", encode(&never)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DescribeChannel, deleted",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}", encode(&deleted)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DescribeChannelMembership, deleted channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!(
                "/channels/{}/memberships/{}",
                encode(&deleted),
                encode(MEMBER)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DescribeChannelModerator, never-created channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!(
                "/channels/{}/moderators/{}",
                encode(&never),
                encode(CREATOR)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DescribeChannelBan, deleted channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}/bans/{}", encode(&deleted), encode(MEMBER)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "GetChannelMessage, deleted channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!(
                "/channels/{}/messages/{}",
                encode(&deleted),
                encode(ABSENT_MESSAGE_ID)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "ListChannelMessages, deleted channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}/messages", encode(&deleted)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "ListChannelMemberships, never-created channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}/memberships", encode(&never)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "ListChannelModerators, never-created channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}/moderators", encode(&never)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "ListChannelBans, never-created channel",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}/bans", encode(&never)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannel, already deleted",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!("/channels/{}", encode(&deleted)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "SendChannelMessage, deleted channel",
            bearer: CREATOR,
            method: "POST",
            uri: format!("/channels/{}/messages", encode(&deleted)),
            body: Some(send_body),
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "CreateChannelBan, never-created channel",
            bearer: CREATOR,
            method: "POST",
            uri: format!("/channels/{}/bans", encode(&never)),
            body: Some(json!({ "MemberArn": MEMBER })),
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "CreateChannelModerator, never-created channel",
            bearer: CREATOR,
            method: "POST",
            uri: format!("/channels/{}/moderators", encode(&never)),
            body: Some(json!({ "ChannelModeratorArn": MEMBER })),
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannelMembership, deleted channel",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!(
                "/channels/{}/memberships/{}",
                encode(&deleted),
                encode(MEMBER)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannelModerator, deleted channel",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!(
                "/channels/{}/moderators/{}",
                encode(&deleted),
                encode(CREATOR)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannelBan, deleted channel",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!("/channels/{}/bans/{}", encode(&deleted), encode(MEMBER)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "RedactChannelMessage, deleted channel",
            bearer: CREATOR,
            method: "POST",
            uri: format!(
                "/channels/{}/messages/{}?operation=redact",
                encode(&deleted),
                encode(ABSENT_MESSAGE_ID)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "UpdateChannelMessage, deleted channel",
            bearer: CREATOR,
            method: "PUT",
            uri: format!(
                "/channels/{}/messages/{}",
                encode(&deleted),
                encode(ABSENT_MESSAGE_ID)
            ),
            body: Some(json!({ "Content": "rewritten" })),
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannelMessage, deleted channel",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!(
                "/channels/{}/messages/{}",
                encode(&deleted),
                encode(ABSENT_MESSAGE_ID)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        // ---- the channel resolves, but the bearer may not read it ----
        Case {
            label: "DescribeChannel, PRIVATE channel the caller is not in",
            bearer: OUTSIDER,
            method: "GET",
            uri: format!("/channels/{}", encode(&private)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "ListChannelMessages, PRIVATE channel the caller is not in",
            bearer: OUTSIDER,
            method: "GET",
            uri: format!("/channels/{}/messages", encode(&private)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DescribeChannel, a BANNED caller",
            bearer: MEMBER,
            method: "GET",
            uri: format!("/channels/{}", encode(&banning)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: BANNED,
        },
        // ---- the channel resolves, but the SUB-RESOURCE the bearer asked for does not,
        // ---- on an operation that declares no `NotFoundException` for it ----
        Case {
            label: "DescribeChannelMembership, an identity that is no member",
            bearer: CREATOR,
            method: "GET",
            uri: format!(
                "/channels/{}/memberships/{}",
                encode(&live),
                encode(OUTSIDER)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannelMembership, an identity that is no member",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!(
                "/channels/{}/memberships/{}",
                encode(&live),
                encode(OUTSIDER)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannelModerator, an identity that does not moderate",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!("/channels/{}/moderators/{}", encode(&live), encode(MEMBER)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "DeleteChannelBan, an identity that is not banned",
            bearer: CREATOR,
            method: "DELETE",
            uri: format!("/channels/{}/bans/{}", encode(&live), encode(MEMBER)),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        Case {
            label: "GetChannelMessage, a message the channel does not hold",
            bearer: CREATOR,
            method: "GET",
            uri: format!(
                "/channels/{}/messages/{}",
                encode(&live),
                encode(ABSENT_MESSAGE_ID)
            ),
            body: None,
            status: 403,
            code: "Forbidden",
            error_type: "ForbiddenException",
            message: INSUFFICIENT_ACCESS,
        },
        // ---- the two attributes a RESOLVED channel reports absent, each with the
        // ---- modelled 404 its own error list declares and its own fixed sentence ----
        Case {
            label: "DescribeChannelModerator, an identity that does not moderate",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}/moderators/{}", encode(&live), encode(MEMBER)),
            body: None,
            status: 404,
            code: "NotFound",
            error_type: "NotFoundException",
            message: NOT_A_MODERATOR,
        },
        Case {
            label: "DescribeChannelBan, an identity that is not banned",
            bearer: CREATOR,
            method: "GET",
            uri: format!("/channels/{}/bans/{}", encode(&live), encode(MEMBER)),
            body: None,
            status: 404,
            code: "NotFound",
            error_type: "NotFoundException",
            message: NOT_BANNED,
        },
    ];

    for case in &cases {
        let reply = emulator
            .call(case.bearer, case.method, &case.uri, case.body.as_ref())
            .await;
        let label = case.label;
        assert_eq!(
            reply.status, case.status,
            "case {label:?}: expected status {}, got {} {:?}",
            case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, case.error_type,
            "case {label:?}: expected x-amzn-errortype {:?}, got {:?}",
            case.error_type, reply.error_type
        );
        assert_eq!(
            reply.body["Code"].as_str(),
            Some(case.code),
            "case {label:?}: expected Code {:?}, got {:?}",
            case.code,
            reply.body["Code"]
        );
        assert_eq!(
            reply.body["Message"].as_str(),
            Some(case.message),
            "case {label:?}: expected Message {:?}, got {:?}",
            case.message,
            reply.body["Message"]
        );
    }
}

/// RULE's other side: the gate refuses only what it cannot resolve — every channel
/// resource the bearer CAN reach is still served from the store, so the refusal above is a
/// decision and not a blanket answer.
///
/// The rows are every reporting operation of the table above, each asked for a resource
/// the store holds.
#[tokio::test]
async fn a_channel_resource_the_bearer_can_resolve_is_still_served() {
    let emulator = Emulator::new();
    let channel = emulator
        .channel("served", "served-token", "RESTRICTED", "PUBLIC")
        .await;
    for (what, reply) in [
        (
            "membership",
            emulator
                .call(
                    CREATOR,
                    "POST",
                    &format!("/channels/{}/memberships", encode(&channel)),
                    Some(&json!({ "MemberArn": MEMBER, "Type": "DEFAULT" })),
                )
                .await,
        ),
        (
            "moderator",
            emulator
                .call(
                    CREATOR,
                    "POST",
                    &format!("/channels/{}/moderators", encode(&channel)),
                    Some(&json!({ "ChannelModeratorArn": MEMBER })),
                )
                .await,
        ),
        (
            "ban",
            emulator
                .call(
                    CREATOR,
                    "POST",
                    &format!("/channels/{}/bans", encode(&channel)),
                    Some(&json!({ "MemberArn": OUTSIDER })),
                )
                .await,
        ),
    ] {
        assert_eq!(
            reply.status, 201,
            "seeding the {what} must answer 201, got {} {:?}",
            reply.status, reply.body
        );
    }
    let sent = emulator
        .call(
            CREATOR,
            "POST",
            &format!("/channels/{}/messages", encode(&channel)),
            Some(&json!({
                "Content": "hello",
                "Type": "STANDARD",
                "Persistence": "PERSISTENT",
                "ClientRequestToken": "served-msg",
            })),
        )
        .await;
    assert_eq!(
        sent.status, 201,
        "seeding the message must answer 201, got {} {:?}",
        sent.status, sent.body
    );
    let message_id = sent.body["MessageId"]
        .as_str()
        .unwrap_or_else(|| panic!("the seeded message must have an id, got {:?}", sent.body))
        .to_string();

    let served: Vec<(&str, String, &str)> = vec![
        (
            "DescribeChannel",
            format!("/channels/{}", encode(&channel)),
            "Channel",
        ),
        (
            "DescribeChannelMembership",
            format!(
                "/channels/{}/memberships/{}",
                encode(&channel),
                encode(MEMBER)
            ),
            "ChannelMembership",
        ),
        (
            "DescribeChannelModerator",
            format!(
                "/channels/{}/moderators/{}",
                encode(&channel),
                encode(MEMBER)
            ),
            "ChannelModerator",
        ),
        (
            "DescribeChannelBan",
            format!("/channels/{}/bans/{}", encode(&channel), encode(OUTSIDER)),
            "ChannelBan",
        ),
        (
            "GetChannelMessage",
            format!(
                "/channels/{}/messages/{}",
                encode(&channel),
                encode(&message_id)
            ),
            "ChannelMessage",
        ),
        (
            "ListChannelMemberships",
            format!("/channels/{}/memberships", encode(&channel)),
            "ChannelMemberships",
        ),
        (
            "ListChannelModerators",
            format!("/channels/{}/moderators", encode(&channel)),
            "ChannelModerators",
        ),
        (
            "ListChannelBans",
            format!("/channels/{}/bans", encode(&channel)),
            "ChannelBans",
        ),
        (
            "ListChannelMessages",
            format!("/channels/{}/messages", encode(&channel)),
            "ChannelMessages",
        ),
    ];

    for (label, uri, member) in &served {
        let reply = emulator.call(CREATOR, "GET", uri, None).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: a resolvable resource must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.body[member].is_null(),
            "case {label:?}: the answer must carry {member}, body {:?}",
            reply.body
        );
    }
}
