// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Chime SDK Messaging MEMBERSHIP slice: who belongs to a
//! channel, who moderates it, who is banned from it, and what each of those roles may do.
//!
//! Every case drives real HTTP through the production router — restJson1 request on the
//! wire in, restJson1 response out — so nothing here can pass by reading a handler's Rust
//! types directly, and every answer is computed from the one persisted channel store:
//! what `CreateChannelMembership`, `CreateChannelModerator` and `CreateChannelBan` wrote
//! is what `DescribeChannelMembership`, `ListChannelMemberships`,
//! `DescribeChannelModerator`, `ListChannelModerators`, `DescribeChannelBan`,
//! `ListChannelBans`, `SearchChannels` and the send/read rules report.
//!
//! Each test states the RULE it pins and exercises a TABLE spanning what that rule
//! generalises over — both `ChannelMode` values, both `ChannelPrivacy` values, both
//! `ChannelMembershipType` values, both `SearchFieldOperator` values, and every role a
//! caller can hold on a channel (creator, member, moderator, banned member, outsider) —
//! so a hardcoded answer cannot pass.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use bytes::Bytes;

// ONE percent-encoder per test binary: the `@httpLabel` encoding rule lives in
// `channel_messaging.rs`, which this suite shares rather than copying.
use crate::channel_messaging::encode;

/// The `AppInstanceArn` every channel below is created under.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";

/// The identity that creates the channels: its first member and first moderator.
const CREATOR: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/alice";
/// An identity the creator adds to a channel.
const MEMBER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/bob";
/// A second identity the creator adds, so a membership set has more than one entry.
const OTHER_MEMBER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/carol";
/// An identity that belongs to no channel.
const OUTSIDER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/dave";

/// One response, reduced to the two things the wire contract pins: the HTTP status and
/// the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The modelled error code, from the restJson1 `__type` discriminator.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
    }

    /// The `Message` the refusal carries.
    fn error_message(&self) -> &str {
        self.body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or("<no Message>")
    }
}

/// The emulator under test: this one service mounted in the production router. Unlike
/// `channel_messaging.rs`'s harness, every call here carries the bearer the case names,
/// because the rules under test are about WHO is calling.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        // ONE shared engine, exactly as `router/src/lib.rs` builds it, so every request
        // in a case observes the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router
            .set_request_authentication(false)
            .register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, bearer: &str, method: &str, uri: &str, body: Option<&Value>) -> Reply {
        let bytes = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the request")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
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

    /// Create a channel as [`CREATOR`] and return its ARN.
    async fn channel(&self, label: &str, token: &str, mode: &str, privacy: &str) -> String {
        let reply = self
            .call(
                CREATOR,
                "POST",
                "/channels",
                Some(&json!({
                    "AppInstanceArn": APP_INSTANCE,
                    "Name": label,
                    "Mode": mode,
                    "Privacy": privacy,
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

    async fn add_member(&self, bearer: &str, channel: &str, member: &str, kind: &str) -> Reply {
        self.call(
            bearer,
            "POST",
            &format!("/channels/{}/memberships", encode(channel)),
            Some(&json!({ "MemberArn": member, "Type": kind })),
        )
        .await
    }

    async fn describe_membership(&self, bearer: &str, channel: &str, member: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!(
                "/channels/{}/memberships/{}",
                encode(channel),
                encode(member)
            ),
            None,
        )
        .await
    }

    async fn list_memberships(&self, bearer: &str, channel: &str, kind: Option<&str>) -> Reply {
        let query = kind.map(|kind| format!("?type={kind}")).unwrap_or_default();
        self.call(
            bearer,
            "GET",
            &format!("/channels/{}/memberships{query}", encode(channel)),
            None,
        )
        .await
    }

    async fn remove_member(&self, bearer: &str, channel: &str, member: &str) -> Reply {
        self.call(
            bearer,
            "DELETE",
            &format!(
                "/channels/{}/memberships/{}",
                encode(channel),
                encode(member)
            ),
            None,
        )
        .await
    }

    async fn add_moderator(&self, bearer: &str, channel: &str, moderator: &str) -> Reply {
        self.call(
            bearer,
            "POST",
            &format!("/channels/{}/moderators", encode(channel)),
            Some(&json!({ "ChannelModeratorArn": moderator })),
        )
        .await
    }

    async fn list_moderators(&self, bearer: &str, channel: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!("/channels/{}/moderators", encode(channel)),
            None,
        )
        .await
    }

    async fn describe_moderator(&self, bearer: &str, channel: &str, moderator: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!(
                "/channels/{}/moderators/{}",
                encode(channel),
                encode(moderator)
            ),
            None,
        )
        .await
    }

    async fn remove_moderator(&self, bearer: &str, channel: &str, moderator: &str) -> Reply {
        self.call(
            bearer,
            "DELETE",
            &format!(
                "/channels/{}/moderators/{}",
                encode(channel),
                encode(moderator)
            ),
            None,
        )
        .await
    }

    async fn ban(&self, bearer: &str, channel: &str, member: &str) -> Reply {
        self.call(
            bearer,
            "POST",
            &format!("/channels/{}/bans", encode(channel)),
            Some(&json!({ "MemberArn": member })),
        )
        .await
    }

    async fn describe_ban(&self, bearer: &str, channel: &str, member: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!("/channels/{}/bans/{}", encode(channel), encode(member)),
            None,
        )
        .await
    }

    async fn list_bans(&self, bearer: &str, channel: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!("/channels/{}/bans", encode(channel)),
            None,
        )
        .await
    }

    async fn unban(&self, bearer: &str, channel: &str, member: &str) -> Reply {
        self.call(
            bearer,
            "DELETE",
            &format!("/channels/{}/bans/{}", encode(channel), encode(member)),
            None,
        )
        .await
    }

    async fn send(&self, bearer: &str, channel: &str, content: &str, token: &str) -> Reply {
        self.call(
            bearer,
            "POST",
            &format!("/channels/{}/messages", encode(channel)),
            Some(&json!({
                "Content": content,
                "Type": "STANDARD",
                "Persistence": "PERSISTENT",
                "ClientRequestToken": token,
            })),
        )
        .await
    }

    async fn get_message(&self, bearer: &str, channel: &str, message_id: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!(
                "/channels/{}/messages/{}",
                encode(channel),
                encode(message_id)
            ),
            None,
        )
        .await
    }

    async fn list_messages(&self, bearer: &str, channel: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!("/channels/{}/messages", encode(channel)),
            None,
        )
        .await
    }

    async fn describe_channel(&self, bearer: &str, channel: &str) -> Reply {
        self.call(bearer, "GET", &format!("/channels/{}", encode(channel)), None)
            .await
    }

    async fn update_message(
        &self,
        bearer: &str,
        channel: &str,
        message_id: &str,
        content: &str,
    ) -> Reply {
        self.call(
            bearer,
            "PUT",
            &format!(
                "/channels/{}/messages/{}",
                encode(channel),
                encode(message_id)
            ),
            Some(&json!({ "Content": content, "Metadata": "edited" })),
        )
        .await
    }

    async fn redact(&self, bearer: &str, channel: &str, message_id: &str) -> Reply {
        self.call(
            bearer,
            "POST",
            &format!(
                "/channels/{}/messages/{}?operation=redact",
                encode(channel),
                encode(message_id)
            ),
            Some(&json!({})),
        )
        .await
    }

    async fn delete_message(&self, bearer: &str, channel: &str, message_id: &str) -> Reply {
        self.call(
            bearer,
            "DELETE",
            &format!(
                "/channels/{}/messages/{}",
                encode(channel),
                encode(message_id)
            ),
            None,
        )
        .await
    }

    async fn search(&self, bearer: &str, operator: &str, values: &[&str]) -> Reply {
        self.call(
            bearer,
            "POST",
            "/channels?operation=search",
            Some(&json!({
                "Fields": [{ "Key": "MEMBERS", "Values": values, "Operator": operator }],
            })),
        )
        .await
    }

    /// Send one message as the creator and return its id.
    async fn seeded_message(&self, channel: &str, token: &str) -> String {
        let sent = self.send(CREATOR, channel, "hello", token).await;
        assert_eq!(
            sent.status, 201,
            "seeding a message must answer 201, got {} {:?}",
            sent.status, sent.body
        );
        sent.body["MessageId"]
            .as_str()
            .expect("the send returns a MessageId")
            .to_string()
    }
}

/// The `Message` every authorization refusal of this service carries.
const INSUFFICIENT_ACCESS: &str = "You do not have sufficient access to perform this action.";
/// The `Message` a refusal caused by a BAN carries instead.
const BANNED: &str = "App instance user is banned from Channel.";

/// The member ARNs a membership listing reports, sorted.
fn listed_members(reply: &Reply) -> Vec<String> {
    let mut members: Vec<String> = reply.body["ChannelMemberships"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["Member"]["Arn"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    members.sort();
    members
}

// ---------------------------------------------------------------------------
// CreateChannelMembership -> DescribeChannelMembership / ListChannelMemberships
// ---------------------------------------------------------------------------

/// RULE: the identity that creates a channel is its first member and its first moderator,
/// and `CreateChannelMembership` adds a member the channel then reports through
/// `DescribeChannelMembership` and `ListChannelMemberships` — a DEFAULT member always, a
/// HIDDEN member only to a listing that asks for that type.
///
/// The rows span both `ChannelMembershipType` values, both `ChannelMode` values and both
/// `ChannelPrivacy` values: the membership set does not depend on either of the latter.
#[tokio::test]
async fn a_membership_is_readable_through_every_reader_that_reports_it() {
    let cases = [
        ("unrestricted public, DEFAULT member", "UNRESTRICTED", "PUBLIC", "DEFAULT"),
        ("unrestricted private, DEFAULT member", "UNRESTRICTED", "PRIVATE", "DEFAULT"),
        ("restricted public, HIDDEN member", "RESTRICTED", "PUBLIC", "HIDDEN"),
        ("restricted private, HIDDEN member", "RESTRICTED", "PRIVATE", "HIDDEN"),
    ];

    let emulator = Emulator::new();
    for (index, (label, mode, privacy, kind)) in cases.into_iter().enumerate() {
        let channel = emulator
            .channel(label, &format!("member-chan-{index}"), mode, privacy)
            .await;

        // The creator is a member and a moderator of its own channel, with no call of
        // its own.
        let creator_membership = emulator
            .describe_membership(CREATOR, &channel, CREATOR)
            .await;
        assert_eq!(
            creator_membership.status, 200,
            "case {label:?}: the creator must be a member of its own channel, got {} {:?}",
            creator_membership.status, creator_membership.body
        );
        let creator_moderation = emulator.describe_moderator(CREATOR, &channel, CREATOR).await;
        assert_eq!(
            creator_moderation.status, 200,
            "case {label:?}: the creator must moderate its own channel, got {} {:?}",
            creator_moderation.status, creator_moderation.body
        );

        let added = emulator.add_member(CREATOR, &channel, MEMBER, kind).await;
        assert_eq!(
            added.status, 201,
            "case {label:?}: CreateChannelMembership must answer 201, got {} {:?}",
            added.status, added.body
        );
        assert_eq!(
            added.body["ChannelArn"].as_str(),
            Some(channel.as_str()),
            "case {label:?}: wrong ChannelArn, got {:?}",
            added.body["ChannelArn"]
        );
        assert_eq!(
            added.body["Member"]["Arn"].as_str(),
            Some(MEMBER),
            "case {label:?}: wrong Member, got {:?}",
            added.body["Member"]
        );

        let described = emulator.describe_membership(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: DescribeChannelMembership must answer 200, got {} {:?}",
            described.status, described.body
        );
        let membership = &described.body["ChannelMembership"];
        assert_eq!(
            membership["Member"]["Arn"].as_str(),
            Some(MEMBER),
            "case {label:?}: wrong Member, got {membership:?}"
        );
        assert_eq!(
            membership["Type"].as_str(),
            Some(kind),
            "case {label:?}: wrong membership Type, got {membership:?}"
        );
        assert_eq!(
            membership["InvitedBy"]["Arn"].as_str(),
            Some(CREATOR),
            "case {label:?}: InvitedBy must be the caller that added the member, got {membership:?}"
        );
        assert_eq!(
            membership["ChannelArn"].as_str(),
            Some(channel.as_str()),
            "case {label:?}: wrong ChannelArn, got {membership:?}"
        );
        assert!(
            membership["CreatedTimestamp"].is_number()
                && membership["LastUpdatedTimestamp"].is_number(),
            "case {label:?}: both membership timestamps must be set, got {membership:?}"
        );

        // A DEFAULT member is always listed; a HIDDEN one only when the filter asks for
        // that type.
        let default_listing = emulator.list_memberships(CREATOR, &channel, None).await;
        assert_eq!(
            default_listing.status, 200,
            "case {label:?}: ListChannelMemberships must answer 200, got {} {:?}",
            default_listing.status, default_listing.body
        );
        let hidden_listing = emulator
            .list_memberships(CREATOR, &channel, Some("HIDDEN"))
            .await;
        let (expected_default, expected_hidden): (Vec<String>, Vec<String>) = if kind == "HIDDEN" {
            (
                vec![CREATOR.to_string()],
                vec![MEMBER.to_string()],
            )
        } else {
            (
                vec![CREATOR.to_string(), MEMBER.to_string()],
                Vec::new(),
            )
        };
        let mut expected_default = expected_default;
        expected_default.sort();
        assert_eq!(
            listed_members(&default_listing),
            expected_default,
            "case {label:?}: wrong DEFAULT membership listing, body {:?}",
            default_listing.body
        );
        assert_eq!(
            listed_members(&hidden_listing),
            expected_hidden,
            "case {label:?}: wrong HIDDEN membership listing, body {:?}",
            hidden_listing.body
        );

        // A member removed is gone for every reader.
        let removed = emulator.remove_member(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            removed.status, 204,
            "case {label:?}: DeleteChannelMembership must answer 204, got {} {:?}",
            removed.status, removed.body
        );
        let after = emulator.describe_membership(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            after.status, 404,
            "case {label:?}: a removed membership must not be readable, got {} {:?}",
            after.status, after.body
        );
    }
}

/// RULE: who may add a member is the channel's `Mode`: moderators and channel members add
/// members to an UNRESTRICTED channel, and only moderators to a RESTRICTED one; a HIDDEN
/// membership is a moderator's alone. Every refusal is the modelled 403.
///
/// The rows are the cross product of both `ChannelMode` values with every role a caller
/// can hold — moderator (the creator), plain member, outsider — plus the HIDDEN type.
#[tokio::test]
async fn the_channel_mode_decides_who_may_add_a_member() {
    struct Case {
        label: &'static str,
        mode: &'static str,
        caller: &'static str,
        kind: &'static str,
        expected_status: u16,
    }

    let cases = [
        Case {
            label: "unrestricted, added by a moderator",
            mode: "UNRESTRICTED",
            caller: CREATOR,
            kind: "DEFAULT",
            expected_status: 201,
        },
        Case {
            label: "unrestricted, added by a plain member",
            mode: "UNRESTRICTED",
            caller: MEMBER,
            kind: "DEFAULT",
            expected_status: 201,
        },
        Case {
            label: "unrestricted, added by an outsider",
            mode: "UNRESTRICTED",
            caller: OUTSIDER,
            kind: "DEFAULT",
            expected_status: 403,
        },
        Case {
            label: "restricted, added by a moderator",
            mode: "RESTRICTED",
            caller: CREATOR,
            kind: "DEFAULT",
            expected_status: 201,
        },
        Case {
            label: "restricted, added by a plain member",
            mode: "RESTRICTED",
            caller: MEMBER,
            kind: "DEFAULT",
            expected_status: 403,
        },
        Case {
            label: "restricted, added by an outsider",
            mode: "RESTRICTED",
            caller: OUTSIDER,
            kind: "DEFAULT",
            expected_status: 403,
        },
        Case {
            label: "unrestricted, HIDDEN member added by a plain member",
            mode: "UNRESTRICTED",
            caller: MEMBER,
            kind: "HIDDEN",
            expected_status: 403,
        },
        Case {
            label: "unrestricted, HIDDEN member added by a moderator",
            mode: "UNRESTRICTED",
            caller: CREATOR,
            kind: "HIDDEN",
            expected_status: 201,
        },
    ];

    let emulator = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let channel = emulator
            .channel(
                case.label,
                &format!("mode-chan-{index}"),
                case.mode,
                "PUBLIC",
            )
            .await;
        // `MEMBER` is a plain member of every channel in this table: a moderator adds it,
        // whatever the mode.
        let seeded = emulator.add_member(CREATOR, &channel, MEMBER, "DEFAULT").await;
        assert_eq!(
            seeded.status, 201,
            "case {:?}: seeding the plain member must answer 201, got {} {:?}",
            case.label, seeded.status, seeded.body
        );

        let reply = emulator
            .add_member(case.caller, &channel, OTHER_MEMBER, case.kind)
            .await;
        assert_eq!(
            reply.status, case.expected_status,
            "case {:?}: expected {}, got {} {:?}",
            case.label, case.expected_status, reply.status, reply.body
        );
        if case.expected_status == 403 {
            assert_eq!(
                reply.error_code(),
                "ForbiddenException",
                "case {:?}: wrong error code, body {:?}",
                case.label,
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                INSUFFICIENT_ACCESS,
                "case {:?}: wrong refusal message, body {:?}",
                case.label,
                reply.body
            );
            // A refused add changes nothing a later reader sees.
            let described = emulator
                .describe_membership(CREATOR, &channel, OTHER_MEMBER)
                .await;
            assert_eq!(
                described.status, 404,
                "case {:?}: the refused member must not be in the channel, got {} {:?}",
                case.label, described.status, described.body
            );
        } else {
            let described = emulator
                .describe_membership(CREATOR, &channel, OTHER_MEMBER)
                .await;
            assert_eq!(
                described.status, 200,
                "case {:?}: the added member must be in the channel, got {} {:?}",
                case.label, described.status, described.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CreateChannelBan
// ---------------------------------------------------------------------------

/// RULE: a ban is a moderator's action; it removes the banned identity's membership, keeps
/// it out of the channel — it can neither be added back nor send nor read — and
/// `DeleteChannelBan` is what lets it back in. `DescribeChannelBan` and `ListChannelBans`
/// report the ban while it stands and refuse once it is gone.
#[tokio::test]
async fn a_ban_keeps_a_member_out_until_it_is_deleted() {
    let emulator = Emulator::new();
    for (index, (label, mode, privacy)) in [
        ("unrestricted public", "UNRESTRICTED", "PUBLIC"),
        ("restricted private", "RESTRICTED", "PRIVATE"),
    ]
    .into_iter()
    .enumerate()
    {
        let channel = emulator
            .channel(label, &format!("ban-chan-{index}"), mode, privacy)
            .await;
        let added = emulator.add_member(CREATOR, &channel, MEMBER, "DEFAULT").await;
        assert_eq!(
            added.status, 201,
            "case {label:?}: seeding the member must answer 201, got {} {:?}",
            added.status, added.body
        );

        // A plain member cannot ban anyone.
        let refused = emulator.ban(MEMBER, &channel, OTHER_MEMBER).await;
        assert_eq!(
            refused.status, 403,
            "case {label:?}: only a moderator may ban, got {} {:?}",
            refused.status, refused.body
        );

        let banned = emulator.ban(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            banned.status, 201,
            "case {label:?}: CreateChannelBan must answer 201, got {} {:?}",
            banned.status, banned.body
        );
        assert_eq!(
            banned.body["Member"]["Arn"].as_str(),
            Some(MEMBER),
            "case {label:?}: wrong banned Member, got {:?}",
            banned.body["Member"]
        );

        // The ban took the membership with it.
        let membership = emulator.describe_membership(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            membership.status, 404,
            "case {label:?}: a banned identity must not be a member, got {} {:?}",
            membership.status, membership.body
        );
        assert_eq!(
            listed_members(&emulator.list_memberships(CREATOR, &channel, None).await),
            vec![CREATOR.to_string()],
            "case {label:?}: only the creator must remain a member"
        );

        // The ban is readable, and every action the banned identity attempts names it.
        let described = emulator.describe_ban(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: DescribeChannelBan must answer 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["ChannelBan"]["CreatedBy"]["Arn"].as_str(),
            Some(CREATOR),
            "case {label:?}: wrong CreatedBy, got {:?}",
            described.body["ChannelBan"]
        );
        let listed = emulator.list_bans(CREATOR, &channel).await;
        let bans: Vec<&str> = listed.body["ChannelBans"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry["Member"]["Arn"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            bans,
            vec![MEMBER],
            "case {label:?}: wrong ban listing, body {:?}",
            listed.body
        );

        for (what, reply) in [
            (
                "SendChannelMessage",
                emulator
                    .send(MEMBER, &channel, "let me in", &format!("ban-msg-{index}"))
                    .await,
            ),
            (
                "ListChannelMessages",
                emulator.list_messages(MEMBER, &channel).await,
            ),
            (
                "CreateChannelMembership",
                emulator.add_member(CREATOR, &channel, MEMBER, "DEFAULT").await,
            ),
        ] {
            assert_eq!(
                reply.status, 403,
                "case {label:?}: {what} by a banned identity must answer 403, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_message(),
                BANNED,
                "case {label:?}: {what} must name the ban, body {:?}",
                reply.body
            );
        }

        // Deleting the ban is what lets the identity back in.
        let refused_unban = emulator.unban(OUTSIDER, &channel, MEMBER).await;
        assert_eq!(
            refused_unban.status, 403,
            "case {label:?}: only a moderator may delete a ban, got {} {:?}",
            refused_unban.status, refused_unban.body
        );
        let unbanned = emulator.unban(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            unbanned.status, 204,
            "case {label:?}: DeleteChannelBan must answer 204, got {} {:?}",
            unbanned.status, unbanned.body
        );
        let gone = emulator.describe_ban(CREATOR, &channel, MEMBER).await;
        assert_eq!(
            gone.status, 404,
            "case {label:?}: a deleted ban must not be readable, got {} {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.error_message(),
            "App instance user is not banned from the specified Channel.",
            "case {label:?}: wrong message for a ban the channel does not hold, body {:?}",
            gone.body
        );
        let rejoined = emulator.add_member(CREATOR, &channel, MEMBER, "DEFAULT").await;
        assert_eq!(
            rejoined.status, 201,
            "case {label:?}: an unbanned identity must be addable again, got {} {:?}",
            rejoined.status, rejoined.body
        );
    }
}

// ---------------------------------------------------------------------------
// CreateChannelModerator
// ---------------------------------------------------------------------------

/// RULE: moderation is a moderator's to grant and to withdraw: only a moderator adds or
/// removes one, a moderator added is reported by `DescribeChannelModerator` and
/// `ListChannelModerators`, and the capability follows the record — a new moderator may
/// add members to a RESTRICTED channel, and may not once it is removed.
#[tokio::test]
async fn moderation_is_granted_by_a_moderator_and_carries_its_capabilities() {
    let emulator = Emulator::new();
    let channel = emulator
        .channel("moderators", "mod-chan", "RESTRICTED", "PUBLIC")
        .await;
    let added = emulator.add_member(CREATOR, &channel, MEMBER, "DEFAULT").await;
    assert_eq!(
        added.status, 201,
        "seeding the member must answer 201, got {} {:?}",
        added.status, added.body
    );

    // A plain member cannot promote anyone, and nothing is written for the attempt.
    let refused = emulator.add_moderator(MEMBER, &channel, MEMBER).await;
    assert_eq!(
        refused.status, 403,
        "case {:?}: only a moderator may add a moderator, got {} {:?}",
        "added by a plain member", refused.status, refused.body
    );
    assert_eq!(
        emulator
            .describe_moderator(CREATOR, &channel, MEMBER)
            .await
            .status,
        404,
        "case {:?}: the refused moderator must not be recorded",
        "added by a plain member"
    );

    let promoted = emulator.add_moderator(CREATOR, &channel, MEMBER).await;
    assert_eq!(
        promoted.status, 201,
        "case {:?}: CreateChannelModerator must answer 201, got {} {:?}",
        "added by a moderator", promoted.status, promoted.body
    );
    assert_eq!(
        promoted.body["ChannelModerator"]["Arn"].as_str(),
        Some(MEMBER),
        "case {:?}: wrong ChannelModerator, got {:?}",
        "added by a moderator",
        promoted.body["ChannelModerator"]
    );

    let described = emulator.describe_moderator(CREATOR, &channel, MEMBER).await;
    assert_eq!(
        described.status, 200,
        "case {:?}: DescribeChannelModerator must answer 200, got {} {:?}",
        "read back", described.status, described.body
    );
    assert_eq!(
        described.body["ChannelModerator"]["CreatedBy"]["Arn"].as_str(),
        Some(CREATOR),
        "case {:?}: wrong CreatedBy, got {:?}",
        "read back",
        described.body["ChannelModerator"]
    );
    let listed = emulator.list_moderators(CREATOR, &channel).await;
    let mut moderators: Vec<&str> = listed.body["ChannelModerators"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["Moderator"]["Arn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    moderators.sort_unstable();
    let mut expected = vec![CREATOR, MEMBER];
    expected.sort_unstable();
    assert_eq!(
        moderators, expected,
        "case {:?}: wrong moderator listing, body {:?}",
        "read back", listed.body
    );

    // The capability follows the record: the new moderator may now add a member to this
    // RESTRICTED channel, and may not once the moderation is withdrawn.
    let as_moderator = emulator
        .add_member(MEMBER, &channel, OTHER_MEMBER, "DEFAULT")
        .await;
    assert_eq!(
        as_moderator.status, 201,
        "case {:?}: a moderator must be able to add a member to a RESTRICTED channel, got {} {:?}",
        "capability granted", as_moderator.status, as_moderator.body
    );
    let withdrawn = emulator.remove_moderator(CREATOR, &channel, MEMBER).await;
    assert_eq!(
        withdrawn.status, 204,
        "case {:?}: DeleteChannelModerator must answer 204, got {} {:?}",
        "capability withdrawn", withdrawn.status, withdrawn.body
    );
    let after = emulator.add_member(MEMBER, &channel, OUTSIDER, "DEFAULT").await;
    assert_eq!(
        after.status, 403,
        "case {:?}: a demoted moderator must not add a member to a RESTRICTED channel, got {} {:?}",
        "capability withdrawn", after.status, after.body
    );
}

// ---------------------------------------------------------------------------
// Privacy and the send rule
// ---------------------------------------------------------------------------

/// RULE: a PUBLIC channel's messages can be read without a membership and a PRIVATE
/// channel's only by its members and moderators, while SENDING always takes a membership —
/// the rule the model states on `CreateChannelMembership`. Every refusal is the modelled
/// 403.
///
/// The rows are the cross product of both `ChannelPrivacy` values with each role, over the
/// three readers and the send.
#[tokio::test]
async fn privacy_decides_who_may_read_and_membership_decides_who_may_send() {
    struct Case {
        label: &'static str,
        privacy: &'static str,
        caller: &'static str,
        read_status: u16,
        send_status: u16,
    }

    let cases = [
        Case {
            label: "public channel, its creator",
            privacy: "PUBLIC",
            caller: CREATOR,
            read_status: 200,
            send_status: 201,
        },
        Case {
            label: "public channel, a member",
            privacy: "PUBLIC",
            caller: MEMBER,
            read_status: 200,
            send_status: 201,
        },
        Case {
            label: "public channel, an outsider",
            privacy: "PUBLIC",
            caller: OUTSIDER,
            read_status: 200,
            send_status: 403,
        },
        Case {
            label: "private channel, its creator",
            privacy: "PRIVATE",
            caller: CREATOR,
            read_status: 200,
            send_status: 201,
        },
        Case {
            label: "private channel, a member",
            privacy: "PRIVATE",
            caller: MEMBER,
            read_status: 200,
            send_status: 201,
        },
        Case {
            label: "private channel, an outsider",
            privacy: "PRIVATE",
            caller: OUTSIDER,
            read_status: 403,
            send_status: 403,
        },
    ];

    let emulator = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let channel = emulator
            .channel(
                case.label,
                &format!("privacy-chan-{index}"),
                "UNRESTRICTED",
                case.privacy,
            )
            .await;
        let seeded = emulator.add_member(CREATOR, &channel, MEMBER, "DEFAULT").await;
        assert_eq!(
            seeded.status, 201,
            "case {:?}: seeding the member must answer 201, got {} {:?}",
            case.label, seeded.status, seeded.body
        );
        let message_id = emulator
            .seeded_message(&channel, &format!("privacy-msg-{index}"))
            .await;

        for (what, reply) in [
            (
                "DescribeChannel",
                emulator.describe_channel(case.caller, &channel).await,
            ),
            (
                "GetChannelMessage",
                emulator
                    .get_message(case.caller, &channel, &message_id)
                    .await,
            ),
            (
                "ListChannelMessages",
                emulator.list_messages(case.caller, &channel).await,
            ),
        ] {
            assert_eq!(
                reply.status, case.read_status,
                "case {:?}: {what} must answer {}, got {} {:?}",
                case.label, case.read_status, reply.status, reply.body
            );
            if case.read_status == 403 {
                assert_eq!(
                    reply.error_message(),
                    INSUFFICIENT_ACCESS,
                    "case {:?}: {what} must carry the service's refusal message, body {:?}",
                    case.label,
                    reply.body
                );
            }
        }

        let sent = emulator
            .send(
                case.caller,
                &channel,
                "from the table",
                &format!("privacy-send-{index}"),
            )
            .await;
        assert_eq!(
            sent.status, case.send_status,
            "case {:?}: SendChannelMessage must answer {}, got {} {:?}",
            case.label, case.send_status, sent.status, sent.body
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateChannelMessage / RedactChannelMessage / DeleteChannelMessage
// ---------------------------------------------------------------------------

/// RULE: a message's SENDER edits it (and nobody else), a sender or a moderator REDACTS it
/// — which removes its content and metadata and keeps the message, with `Redacted` true —
/// and nobody deletes it without administering the app instance, which is what the model
/// reserves `DeleteChannelMessage` to.
#[tokio::test]
async fn editing_redacting_and_deleting_a_message_follow_the_callers_role() {
    let emulator = Emulator::new();
    let channel = emulator
        .channel("messages", "msg-role-chan", "UNRESTRICTED", "PUBLIC")
        .await;
    let seeded = emulator.add_member(CREATOR, &channel, MEMBER, "DEFAULT").await;
    assert_eq!(
        seeded.status, 201,
        "seeding the member must answer 201, got {} {:?}",
        seeded.status, seeded.body
    );
    let message_id = emulator.seeded_message(&channel, "msg-role-token").await;

    // Only the sender edits.
    for (label, caller, expected) in [
        ("edited by another member", MEMBER, 403),
        ("edited by an outsider", OUTSIDER, 403),
        ("edited by its sender", CREATOR, 200),
    ] {
        let reply = emulator
            .update_message(caller, &channel, &message_id, "edited content")
            .await;
        assert_eq!(
            reply.status, expected,
            "case {label:?}: UpdateChannelMessage must answer {expected}, got {} {:?}",
            reply.status, reply.body
        );
    }
    let edited = emulator.get_message(CREATOR, &channel, &message_id).await;
    assert_eq!(
        edited.body["ChannelMessage"]["Content"].as_str(),
        Some("edited content"),
        "case {:?}: the edit must be what every later reader sees, got {:?}",
        "edited by its sender",
        edited.body["ChannelMessage"]
    );
    assert!(
        edited.body["ChannelMessage"]["LastEditedTimestamp"].is_number(),
        "case {:?}: an edited message must carry a LastEditedTimestamp, got {:?}",
        "edited by its sender",
        edited.body["ChannelMessage"]
    );

    // Nobody deletes a message: deletion takes an app instance administrator, and no
    // role on the channel is one.
    for (label, caller) in [
        ("deleted by its sender, who also moderates", CREATOR),
        ("deleted by a member", MEMBER),
        ("deleted by an outsider", OUTSIDER),
    ] {
        let reply = emulator.delete_message(caller, &channel, &message_id).await;
        assert_eq!(
            reply.status, 403,
            "case {label:?}: DeleteChannelMessage must answer 403, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_message(),
            INSUFFICIENT_ACCESS,
            "case {label:?}: wrong refusal message, body {:?}",
            reply.body
        );
    }
    assert_eq!(
        emulator
            .get_message(CREATOR, &channel, &message_id)
            .await
            .status,
        200,
        "case {:?}: a refused delete must leave the message readable",
        "deleted by a member"
    );

    // An outsider cannot redact; the sender can, and the message stays in the channel
    // with its content and metadata gone.
    let refused = emulator.redact(OUTSIDER, &channel, &message_id).await;
    assert_eq!(
        refused.status, 403,
        "case {:?}: RedactChannelMessage must answer 403, got {} {:?}",
        "redacted by an outsider", refused.status, refused.body
    );
    let redacted = emulator.redact(CREATOR, &channel, &message_id).await;
    assert_eq!(
        redacted.status, 200,
        "case {:?}: RedactChannelMessage must answer 200, got {} {:?}",
        "redacted by its sender", redacted.status, redacted.body
    );
    let after = emulator.get_message(CREATOR, &channel, &message_id).await;
    let message = &after.body["ChannelMessage"];
    assert_eq!(
        after.status, 200,
        "case {:?}: a redacted message must still be readable, got {} {:?}",
        "redacted by its sender", after.status, after.body
    );
    assert_eq!(
        message["Redacted"].as_bool(),
        Some(true),
        "case {:?}: the message must report Redacted=true, got {message:?}",
        "redacted by its sender"
    );
    assert!(
        message.get("Content").is_none() && message.get("Metadata").is_none(),
        "case {:?}: redaction must remove the content and the metadata, got {message:?}",
        "redacted by its sender"
    );
    assert_eq!(
        message["MessageId"].as_str(),
        Some(message_id.as_str()),
        "case {:?}: the message itself must stay in the channel, got {message:?}",
        "redacted by its sender"
    );
    // A redacted message has no content left to edit.
    let edit_after = emulator
        .update_message(CREATOR, &channel, &message_id, "back again")
        .await;
    assert_eq!(
        edit_after.status, 409,
        "case {:?}: updating a redacted message must answer 409, got {} {:?}",
        "edited after redaction", edit_after.status, edit_after.body
    );
}

// ---------------------------------------------------------------------------
// SearchChannels
// ---------------------------------------------------------------------------

/// RULE: `SearchChannels` searches the channels the `ChimeBearer` BELONGS to by their
/// membership: `EQUALS` matches a channel whose member set is exactly the requested ARNs
/// and `INCLUDES` one whose member set contains them all, so a channel a membership was
/// just added to is found and one the membership was removed from is not.
///
/// The rows span both `SearchFieldOperator` values and membership sets of one, two and
/// three identities.
#[tokio::test]
async fn searching_by_membership_reports_the_channels_the_caller_belongs_to() {
    let emulator = Emulator::new();
    let alone = emulator
        .channel("alone", "search-chan-0", "UNRESTRICTED", "PUBLIC")
        .await;
    let pair = emulator
        .channel("pair", "search-chan-1", "UNRESTRICTED", "PUBLIC")
        .await;
    let trio = emulator
        .channel("trio", "search-chan-2", "UNRESTRICTED", "PRIVATE")
        .await;
    for (channel, members) in [
        (&pair, vec![MEMBER]),
        (&trio, vec![MEMBER, OTHER_MEMBER]),
    ] {
        for member in members {
            let reply = emulator.add_member(CREATOR, channel, member, "DEFAULT").await;
            assert_eq!(
                reply.status, 201,
                "seeding {member} into a channel must answer 201, got {} {:?}",
                reply.status, reply.body
            );
        }
    }

    /// The channel ARNs a search answer reports, sorted.
    fn found(reply: &Reply) -> Vec<String> {
        let mut arns: Vec<String> = reply.body["Channels"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry["ChannelArn"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        arns.sort();
        arns
    }

    let cases: [(&str, &str, Vec<&str>, Vec<String>); 6] = [
        (
            "EQUALS a one-member set",
            "EQUALS",
            vec![CREATOR],
            vec![alone.clone()],
        ),
        (
            "EQUALS a two-member set",
            "EQUALS",
            vec![CREATOR, MEMBER],
            vec![pair.clone()],
        ),
        (
            "EQUALS a three-member set",
            "EQUALS",
            vec![CREATOR, MEMBER, OTHER_MEMBER],
            vec![trio.clone()],
        ),
        (
            "INCLUDES one identity",
            "INCLUDES",
            vec![MEMBER],
            {
                let mut expected = vec![pair.clone(), trio.clone()];
                expected.sort();
                expected
            },
        ),
        (
            "INCLUDES two identities",
            "INCLUDES",
            vec![MEMBER, OTHER_MEMBER],
            vec![trio.clone()],
        ),
        (
            "INCLUDES an identity in no channel",
            "INCLUDES",
            vec![OUTSIDER],
            Vec::new(),
        ),
    ];

    for (label, operator, values, expected) in cases {
        let reply = emulator.search(CREATOR, operator, &values).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: SearchChannels must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            found(&reply),
            expected,
            "case {label:?}: wrong channels found, body {:?}",
            reply.body
        );
    }

    // The search is over the channels the BEARER belongs to, so an outsider finds
    // nothing, and a membership removed takes its channel out of the answer.
    let outsider = emulator.search(OUTSIDER, "INCLUDES", &[CREATOR]).await;
    assert_eq!(
        found(&outsider),
        Vec::<String>::new(),
        "case {:?}: an identity that belongs to no channel must find none, body {:?}",
        "searched by an outsider",
        outsider.body
    );
    let removed = emulator.remove_member(CREATOR, &pair, MEMBER).await;
    assert_eq!(
        removed.status, 204,
        "removing the member must answer 204, got {} {:?}",
        removed.status, removed.body
    );
    let after = emulator.search(CREATOR, "INCLUDES", &[MEMBER]).await;
    assert_eq!(
        found(&after),
        vec![trio.clone()],
        "case {:?}: a channel the membership left must drop out of the answer, body {:?}",
        "searched after a removal",
        after.body
    );
}

// ---------------------------------------------------------------------------
// Idempotency conflicts
// ---------------------------------------------------------------------------

/// RULE: one `@idempotencyToken` stands for ONE request: replaying it answers with the
/// resource the first request produced, and reusing it for a DIFFERENT request is the
/// modelled 409 rather than a second resource — on `CreateChannel` as much as on
/// `SendChannelMessage`.
#[tokio::test]
async fn reusing_an_idempotency_token_for_a_different_request_conflicts() {
    let emulator = Emulator::new();

    // CreateChannel: the same token with the same request replays, with a different
    // request conflicts.
    let create = json!({
        "AppInstanceArn": APP_INSTANCE,
        "Name": "token-a",
        "ClientRequestToken": "shared-token",
    });
    let first = emulator.call(CREATOR, "POST", "/channels", Some(&create)).await;
    assert_eq!(
        first.status, 201,
        "case {:?}: the first CreateChannel must answer 201, got {} {:?}",
        "first create", first.status, first.body
    );
    let replay = emulator.call(CREATOR, "POST", "/channels", Some(&create)).await;
    assert_eq!(
        replay.body["ChannelArn"], first.body["ChannelArn"],
        "case {:?}: a replay must answer with the first ARN, got {:?}",
        "replayed create", replay.body
    );
    for (label, member, value) in [
        ("a different Name", "Name", json!("token-b")),
        ("a different Mode", "Mode", json!("RESTRICTED")),
        ("a different Privacy", "Privacy", json!("PRIVATE")),
        ("a different Metadata", "Metadata", json!("m")),
    ] {
        let mut different = create.clone();
        different[member] = value;
        let reply = emulator
            .call(CREATOR, "POST", "/channels", Some(&different))
            .await;
        assert_eq!(
            reply.status, 409,
            "case {label:?}: the reused token must answer 409, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {label:?}: wrong error code, body {:?}",
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            "The provided ClientRequestToken is already associated with an existing resource.",
            "case {label:?}: wrong conflict message, body {:?}",
            reply.body
        );
    }

    // SendChannelMessage: the same token with the same content replays, with different
    // content conflicts.
    let channel = emulator
        .channel("token-msgs", "token-chan", "UNRESTRICTED", "PUBLIC")
        .await;
    let sent = emulator.send(CREATOR, &channel, "once", "msg-token").await;
    assert_eq!(
        sent.status, 201,
        "case {:?}: the first send must answer 201, got {} {:?}",
        "first send", sent.status, sent.body
    );
    let replayed = emulator.send(CREATOR, &channel, "once", "msg-token").await;
    assert_eq!(
        replayed.body["MessageId"], sent.body["MessageId"],
        "case {:?}: a replay must answer with the first MessageId, got {:?}",
        "replayed send", replayed.body
    );
    for (label, content) in [
        ("different content", "twice"),
        ("content that differs only in case", "Once"),
        ("empty-ish content", " "),
    ] {
        let reply = emulator.send(CREATOR, &channel, content, "msg-token").await;
        assert_eq!(
            reply.status, 409,
            "case {label:?}: the reused token must answer 409, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_message(),
            "The provided ClientRequestToken is already associated with an existing resource.",
            "case {label:?}: wrong conflict message, body {:?}",
            reply.body
        );
    }
    let listed = emulator.list_messages(CREATOR, &channel).await;
    assert_eq!(
        listed.body["ChannelMessages"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case {:?}: no conflict may append a second message, body {:?}",
        "after the conflicts",
        listed.body
    );
}
