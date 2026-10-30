// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two operations that EDIT an existing Chime SDK Messaging
//! channel: `UpdateChannel` and `PutChannelExpirationSettings`.
//!
//! Every case drives real HTTP through the production router — restJson1 request on the
//! wire in, restJson1 response out — so nothing here can pass by reading a handler's Rust
//! types directly. The rule under test throughout is read-after-write: what an accepted
//! edit wrote onto the stored channel is what `DescribeChannel` and `ListChannels`
//! report afterwards, and what a REFUSED edit asked for is reported by neither.
//!
//! The tables span what those rules generalise over — each updatable member supplied and
//! omitted, both `ChannelMode` wire values in both directions, `Metadata` set, replaced,
//! emptied and left alone, the modelled `Name` / `Metadata` length maxima, both
//! `ExpirationCriterion` wire values, `ExpirationDays` at each modelled bound and one
//! past each of them, and every role a caller can hold on the channel.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// ONE percent-encoder per test binary: the `@httpLabel` encoding rule lives in
// `channel_messaging.rs`, which this suite shares rather than copying.
use crate::channel_messaging::encode;

/// The `AppInstanceArn` every channel below is created under.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";
/// The identity that creates every channel: its first member and first moderator.
const CREATOR: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/alice";
/// An identity the creator adds as a plain member of a channel.
const MEMBER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/bob";
/// An identity that belongs to no channel.
const OUTSIDER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/dave";

/// `NonEmptyResourceName`'s modelled maximum length, which `Name` is tested at
/// (`smithy_query(query="operation",operation="UpdateChannel")` reports `len 1..256`).
const NAME_MAX: usize = 256;
/// `Metadata`'s modelled maximum length, from the same signature (`len 0..1024`).
const METADATA_MAX: usize = 1024;
/// `ExpirationDays`' modelled bounds, from
/// `smithy_query(query="shape",shape="ExpirationSettings")` (`range 1..5475`).
const EXPIRATION_DAYS_MIN: i64 = 1;
const EXPIRATION_DAYS_MAX: i64 = 5475;
/// Every value `ExpirationCriterion` accepts on the wire, from
/// `smithy_query(query="shape",shape="ExpirationCriterion")`.
const EXPIRATION_CRITERIA: [&str; 2] = ["CREATED_TIMESTAMP", "LAST_MESSAGE_TIMESTAMP"];

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
}

/// The emulator under test: this one service mounted in the production router. Every
/// call names the bearer the case is made as, because two of the rules under test are
/// about WHO may edit a channel.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        // ONE shared engine, exactly as `router/src/lib.rs` builds it, so every request
        // in a case observes the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised
        // elsewhere; these cases are about the service's behaviour.
        router
            .set_request_authentication(false)
            .register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(
        &self,
        bearer: Option<&str>,
        method: &str,
        uri: &str,
        body: Option<&Value>,
    ) -> Reply {
        let bytes = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the request")),
            None => Bytes::new(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "messaging-chime.us-east-1.amazonaws.com")
            // The SigV4 credential scope is what routes a request to its service.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/chime/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("content-type", "application/json");
        if let Some(bearer) = bearer {
            request = request.header("x-amz-chime-bearer", bearer);
        }
        let response = self
            .router
            .dispatch(request.body(bytes).expect("build the request"))
            .await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// Create a channel as [`CREATOR`] and return its ARN.
    async fn channel(
        &self,
        label: &str,
        token: &str,
        name: &str,
        mode: &str,
        metadata: Option<&str>,
    ) -> String {
        let mut request = json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": name,
            "Mode": mode,
            "ClientRequestToken": token,
        });
        if let Some(metadata) = metadata {
            request["Metadata"] = json!(metadata);
        }
        let reply = self
            .call(Some(CREATOR), "POST", "/channels", Some(&request))
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

    /// Add `member_arn` to the channel as a plain (non-moderator) member.
    async fn add_member(&self, label: &str, channel_arn: &str, member_arn: &str) {
        let reply = self
            .call(
                Some(CREATOR),
                "POST",
                &format!("/channels/{}/memberships", encode(channel_arn)),
                Some(&json!({ "MemberArn": member_arn, "Type": "DEFAULT" })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case {label:?}: CreateChannelMembership must answer 201, got {} {:?}",
            reply.status, reply.body
        );
    }

    async fn update_channel(&self, bearer: Option<&str>, channel_arn: &str, body: &Value) -> Reply {
        self.call(
            bearer,
            "PUT",
            &format!("/channels/{}", encode(channel_arn)),
            Some(body),
        )
        .await
    }

    async fn put_expiration_settings(
        &self,
        bearer: Option<&str>,
        channel_arn: &str,
        body: &Value,
    ) -> Reply {
        self.call(
            bearer,
            "PUT",
            &format!("/channels/{}/expiration-settings", encode(channel_arn)),
            Some(body),
        )
        .await
    }

    async fn describe_channel(&self, bearer: Option<&str>, channel_arn: &str) -> Reply {
        self.call(
            bearer,
            "GET",
            &format!("/channels/{}", encode(channel_arn)),
            None,
        )
        .await
    }

    /// The `Channel` `DescribeChannel` reports for `channel_arn`, read as [`CREATOR`].
    async fn channel_body(&self, label: &str, channel_arn: &str) -> Value {
        let reply = self.describe_channel(Some(CREATOR), channel_arn).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: DescribeChannel must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        reply.body["Channel"].clone()
    }

    /// Every `ChannelSummary` `ListChannels` reports for [`APP_INSTANCE`], keyed by ARN.
    async fn listed_summaries(&self) -> Vec<Value> {
        let reply = self
            .call(
                Some(CREATOR),
                "GET",
                &format!(
                    "/channels?app-instance-arn={}&max-results=50",
                    encode(APP_INSTANCE)
                ),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "ListChannels must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        reply.body["Channels"]
            .as_array()
            .expect("Channels must be a list")
            .clone()
    }
}

/// Long enough that two instants taken either side of it differ in epoch milliseconds.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

/// The `Metadata` member of a reported channel, as `Option`: absent and present are
/// different answers.
fn metadata_of(channel: &Value) -> Option<&str> {
    channel.get("Metadata").and_then(Value::as_str)
}

// ---------------------------------------------------------------------------
// UpdateChannel -> DescribeChannel / ListChannels
// ---------------------------------------------------------------------------

/// One row of the update table: a channel as created, the update applied to it, and the
/// channel the next read must report.
#[derive(Debug)]
struct UpdateCase {
    label: &'static str,
    created_name: String,
    created_mode: &'static str,
    created_metadata: Option<&'static str>,
    update: Value,
    expected_name: String,
    expected_mode: &'static str,
    expected_metadata: Option<String>,
}

/// RULE: an accepted `UpdateChannel` replaces each of `Name`, `Mode` and `Metadata` the
/// request SUPPLIES on the stored channel, leaves a member it omits as it was, and makes
/// the call's instant the channel's `LastUpdatedTimestamp` — so every later report of
/// that channel is the updated one.
///
/// The rows span each updatable member supplied alone and all three together, both
/// `ChannelMode` wire values in both directions, `Metadata` set / replaced / emptied /
/// untouched, the modelled `Name` and `Metadata` maxima, and a request that supplies no
/// updatable member at all.
#[tokio::test]
async fn update_channel_applies_the_members_it_supplies() {
    let long_name = "n".repeat(NAME_MAX);
    let long_metadata = "m".repeat(METADATA_MAX);
    let cases = [
        UpdateCase {
            label: "all three members supplied, RESTRICTED -> UNRESTRICTED",
            created_name: "created-one".to_string(),
            created_mode: "RESTRICTED",
            created_metadata: None,
            update: json!({
                "Name": "renamed-one",
                "Mode": "UNRESTRICTED",
                "Metadata": "{\"topic\":\"ops\"}",
            }),
            expected_name: "renamed-one".to_string(),
            expected_mode: "UNRESTRICTED",
            expected_metadata: Some("{\"topic\":\"ops\"}".to_string()),
        },
        UpdateCase {
            label: "all three members supplied, UNRESTRICTED -> RESTRICTED, metadata replaced",
            created_name: "created-two".to_string(),
            created_mode: "UNRESTRICTED",
            created_metadata: Some("{\"topic\":\"old\"}"),
            update: json!({
                "Name": "renamed-two",
                "Mode": "RESTRICTED",
                "Metadata": "{\"topic\":\"new\"}",
            }),
            expected_name: "renamed-two".to_string(),
            expected_mode: "RESTRICTED",
            expected_metadata: Some("{\"topic\":\"new\"}".to_string()),
        },
        UpdateCase {
            label: "Name alone — Mode and Metadata keep the stored values",
            created_name: "created-three".to_string(),
            created_mode: "RESTRICTED",
            created_metadata: Some("keep-me"),
            update: json!({ "Name": "renamed-three" }),
            expected_name: "renamed-three".to_string(),
            expected_mode: "RESTRICTED",
            expected_metadata: Some("keep-me".to_string()),
        },
        UpdateCase {
            label: "Mode alone — Name and Metadata keep the stored values",
            created_name: "created-four".to_string(),
            created_mode: "RESTRICTED",
            created_metadata: Some("also-keep-me"),
            update: json!({ "Mode": "UNRESTRICTED" }),
            expected_name: "created-four".to_string(),
            expected_mode: "UNRESTRICTED",
            expected_metadata: Some("also-keep-me".to_string()),
        },
        UpdateCase {
            label: "Metadata set to the empty string, which Metadata's len 0..1024 allows",
            created_name: "created-five".to_string(),
            created_mode: "UNRESTRICTED",
            created_metadata: Some("about-to-go"),
            update: json!({ "Metadata": "" }),
            expected_name: "created-five".to_string(),
            expected_mode: "UNRESTRICTED",
            expected_metadata: Some(String::new()),
        },
        UpdateCase {
            label: "Name and Metadata at their modelled maxima",
            created_name: "created-six".to_string(),
            created_mode: "RESTRICTED",
            created_metadata: None,
            update: json!({ "Name": long_name, "Metadata": long_metadata }),
            expected_name: long_name.clone(),
            expected_mode: "RESTRICTED",
            expected_metadata: Some(long_metadata.clone()),
        },
        UpdateCase {
            label: "no updatable member supplied — the channel keeps every value",
            created_name: "created-seven".to_string(),
            created_mode: "RESTRICTED",
            created_metadata: Some("untouched"),
            update: json!({}),
            expected_name: "created-seven".to_string(),
            expected_mode: "RESTRICTED",
            expected_metadata: Some("untouched".to_string()),
        },
    ];

    let emulator = Emulator::new();
    // The ARN of each row's channel, so the ListChannels pass below can check the
    // summary projection agrees with what DescribeChannel reported.
    let mut arns: Vec<String> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let token = format!("update-token-{index}");
        let arn = emulator
            .channel(
                case.label,
                &token,
                &case.created_name,
                case.created_mode,
                case.created_metadata,
            )
            .await;
        let before = emulator.channel_body(case.label, &arn).await;
        let created_at = before["CreatedTimestamp"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: CreatedTimestamp must be a number", case.label));
        tick().await;

        let reply = emulator
            .update_channel(Some(CREATOR), &arn, &case.update)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: UpdateChannel must answer 200, got {} {:?}",
            case.label, reply.status, reply.body
        );
        assert_eq!(
            reply.body["ChannelArn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: UpdateChannel must answer with the updated channel's ARN, got {:?}",
            case.label,
            reply.body["ChannelArn"]
        );

        let channel = emulator.channel_body(case.label, &arn).await;
        assert_eq!(
            channel["Name"].as_str(),
            Some(case.expected_name.as_str()),
            "case {:?}: wrong Name after the update, got {:?}",
            case.label,
            channel["Name"]
        );
        assert_eq!(
            channel["Mode"].as_str(),
            Some(case.expected_mode),
            "case {:?}: wrong Mode after the update, got {:?}",
            case.label,
            channel["Mode"]
        );
        assert_eq!(
            metadata_of(&channel),
            case.expected_metadata.as_deref(),
            "case {:?}: wrong Metadata after the update, got {:?}",
            case.label,
            channel.get("Metadata")
        );
        // An update cannot change a channel's privacy: the operation takes no `Privacy`
        // member, which is how the model states that restriction.
        assert_eq!(
            channel["Privacy"].as_str(),
            before["Privacy"].as_str(),
            "case {:?}: Privacy must survive the update, got {:?}",
            case.label,
            channel["Privacy"]
        );
        let updated_at = channel["LastUpdatedTimestamp"]
            .as_f64()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: LastUpdatedTimestamp must be a number",
                    case.label
                )
            });
        assert!(
            updated_at > created_at,
            "case {:?}: the update must advance LastUpdatedTimestamp past {created_at}, got {updated_at}",
            case.label
        );
        assert_eq!(
            channel["CreatedTimestamp"].as_f64(),
            Some(created_at),
            "case {:?}: CreatedTimestamp must survive the update, got {:?}",
            case.label,
            channel["CreatedTimestamp"]
        );
        arns.push(arn);
    }

    // The summary projection reads the SAME stored channel, so `ListChannels` reports
    // every row's updated name, mode and metadata too.
    let summaries = emulator.listed_summaries().await;
    for (case, arn) in cases.iter().zip(&arns) {
        let summary = summaries
            .iter()
            .find(|summary| summary["ChannelArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: ListChannels must report the channel at {arn}",
                    case.label
                )
            });
        assert_eq!(
            summary["Name"].as_str(),
            Some(case.expected_name.as_str()),
            "case {:?}: ListChannels reports a stale Name, got {:?}",
            case.label,
            summary["Name"]
        );
        assert_eq!(
            summary["Mode"].as_str(),
            Some(case.expected_mode),
            "case {:?}: ListChannels reports a stale Mode, got {:?}",
            case.label,
            summary["Mode"]
        );
        assert_eq!(
            metadata_of(summary),
            case.expected_metadata.as_deref(),
            "case {:?}: ListChannels reports stale Metadata, got {:?}",
            case.label,
            summary.get("Metadata")
        );
    }
}

/// RULE: an `UpdateChannel` the service refuses changes nothing about the stored channel
/// — a caller that does not moderate it may not edit it, a `Mode` outside the enum's
/// catalog is refused as invalid, and an ARN naming no channel is not found.
#[tokio::test]
async fn a_refused_update_channel_leaves_the_stored_channel_alone() {
    let emulator = Emulator::new();
    let arn = emulator
        .channel(
            "refusals",
            "refusal-token",
            "guarded",
            "RESTRICTED",
            Some("original"),
        )
        .await;
    emulator.add_member("refusals", &arn, MEMBER).await;
    let before = emulator.channel_body("refusals", &arn).await;

    let edit = json!({ "Name": "hijacked", "Mode": "UNRESTRICTED", "Metadata": "rewritten" });
    // Each row is a request that must be refused, with the status and — where the
    // service's own handler is what refuses it — the modelled error code. A `Mode`
    // outside the `ChannelMode` catalog is refused by the modelled-constraint boundary
    // ahead of the handler, so only its status is pinned here.
    let cases: [(&str, Reply, u16, Option<&str>); 4] = [
        (
            "a plain member of the channel",
            emulator.update_channel(Some(MEMBER), &arn, &edit).await,
            403,
            Some("ForbiddenException"),
        ),
        (
            "a caller that belongs to no channel",
            emulator.update_channel(Some(OUTSIDER), &arn, &edit).await,
            403,
            Some("ForbiddenException"),
        ),
        (
            "a Mode outside the ChannelMode catalog",
            emulator
                .update_channel(Some(CREATOR), &arn, &json!({ "Mode": "MODERATED" }))
                .await,
            400,
            None,
        ),
        (
            "an ARN naming no channel",
            emulator
                .update_channel(
                    Some(CREATOR),
                    &format!("{APP_INSTANCE}/channel/never-created"),
                    &edit,
                )
                .await,
            404,
            Some("NotFoundException"),
        ),
    ];

    for (label, reply, expected_status, expected_code) in &cases {
        assert_eq!(
            reply.status, *expected_status,
            "case {label:?}: UpdateChannel must answer {expected_status}, got {} {:?}",
            reply.status, reply.body
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                reply.error_code(),
                *expected_code,
                "case {label:?}: wrong error code, body {:?}",
                reply.body
            );
        }
    }

    let after = emulator.channel_body("refusals", &arn).await;
    assert_eq!(
        after["Name"].as_str(),
        before["Name"].as_str(),
        "case {:?}: a refused update must not rename the channel, got {:?}",
        "every refusal above",
        after["Name"]
    );
    assert_eq!(
        after["Mode"].as_str(),
        before["Mode"].as_str(),
        "case {:?}: a refused update must not change the mode, got {:?}",
        "every refusal above",
        after["Mode"]
    );
    assert_eq!(
        metadata_of(&after),
        metadata_of(&before),
        "case {:?}: a refused update must not change the metadata, got {:?}",
        "every refusal above",
        after.get("Metadata")
    );
}

// ---------------------------------------------------------------------------
// PutChannelExpirationSettings -> DescribeChannel
// ---------------------------------------------------------------------------

/// One row of the expiration table: the settings put, or none at all.
#[derive(Debug)]
struct ExpirationCase {
    label: &'static str,
    settings: Option<(i64, &'static str)>,
}

/// RULE: the `ExpirationSettings` an accepted `PutChannelExpirationSettings` carries
/// become the settings in force on that channel — echoed by the call and reported by
/// `DescribeChannel` through `Channel.ExpirationSettings` — and a call carrying none
/// leaves the channel with no expiration settings at all.
///
/// The rows are applied in turn to ONE channel, so each also pins that a later put
/// REPLACES the settings an earlier one left: they span both `ExpirationCriterion` wire
/// values, `ExpirationDays` at each modelled bound and between them, and the absent
/// member.
#[tokio::test]
async fn put_channel_expiration_settings_is_reported_by_describe_channel() {
    let cases = [
        ExpirationCase {
            label: "the modelled minimum day count, CREATED_TIMESTAMP",
            settings: Some((EXPIRATION_DAYS_MIN, EXPIRATION_CRITERIA[0])),
        },
        ExpirationCase {
            label: "a day count between the bounds, LAST_MESSAGE_TIMESTAMP",
            settings: Some((90, EXPIRATION_CRITERIA[1])),
        },
        ExpirationCase {
            label: "the modelled maximum day count, LAST_MESSAGE_TIMESTAMP",
            settings: Some((EXPIRATION_DAYS_MAX, EXPIRATION_CRITERIA[1])),
        },
        ExpirationCase {
            label: "replaced again, back to CREATED_TIMESTAMP",
            settings: Some((30, EXPIRATION_CRITERIA[0])),
        },
        ExpirationCase {
            label: "no settings at all — the channel keeps no expiry",
            settings: None,
        },
    ];

    let emulator = Emulator::new();
    let arn = emulator
        .channel("expiry", "expiry-token", "retained", "RESTRICTED", None)
        .await;
    let fresh = emulator.channel_body("expiry", &arn).await;
    assert!(
        fresh.get("ExpirationSettings").is_none(),
        "case {:?}: a channel created without ExpirationSettings must report none, got {:?}",
        "freshly created",
        fresh.get("ExpirationSettings")
    );

    for case in &cases {
        let request = match case.settings {
            Some((days, criterion)) => json!({
                "ExpirationSettings": {
                    "ExpirationDays": days,
                    "ExpirationCriterion": criterion,
                }
            }),
            None => json!({}),
        };
        let reply = emulator
            .put_expiration_settings(Some(CREATOR), &arn, &request)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: PutChannelExpirationSettings must answer 200, got {} {:?}",
            case.label, reply.status, reply.body
        );
        assert_eq!(
            reply.body["ChannelArn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: the call must answer with the channel's ARN, got {:?}",
            case.label,
            reply.body["ChannelArn"]
        );
        let expected = case.settings.map(|(days, criterion)| {
            json!({ "ExpirationDays": days, "ExpirationCriterion": criterion })
        });
        assert_eq!(
            reply.body.get("ExpirationSettings"),
            expected.as_ref(),
            "case {:?}: the call must echo the settings now in force, got {:?}",
            case.label,
            reply.body.get("ExpirationSettings")
        );

        let channel = emulator.channel_body(case.label, &arn).await;
        assert_eq!(
            channel.get("ExpirationSettings"),
            expected.as_ref(),
            "case {:?}: DescribeChannel must report the settings in force, got {:?}",
            case.label,
            channel.get("ExpirationSettings")
        );
    }
}

/// RULE: a `PutChannelExpirationSettings` the service refuses leaves the settings in
/// force untouched — a day count outside the modelled range and a criterion outside the
/// enum's catalog are refused as invalid, a caller that does not moderate the channel may
/// not set its expiry, and an ARN naming no channel is not found.
#[tokio::test]
async fn a_refused_put_channel_expiration_settings_leaves_the_settings_alone() {
    let emulator = Emulator::new();
    let arn = emulator
        .channel("expiry refusals", "expiry-refusal", "kept", "RESTRICTED", None)
        .await;
    emulator.add_member("expiry refusals", &arn, MEMBER).await;
    let accepted = json!({
        "ExpirationSettings": { "ExpirationDays": 7, "ExpirationCriterion": EXPIRATION_CRITERIA[0] }
    });
    let reply = emulator
        .put_expiration_settings(Some(CREATOR), &arn, &accepted)
        .await;
    assert_eq!(
        reply.status, 200,
        "the settings under test must be accepted first, got {} {:?}",
        reply.status, reply.body
    );
    let in_force = emulator
        .channel_body("expiry refusals", &arn)
        .await
        .get("ExpirationSettings")
        .cloned();

    // Each row is a request that must be refused, with the status and — where the
    // service's own handler is what refuses it — the modelled error code. A day count
    // outside `range 1..5475` and a criterion outside the catalog are refused by the
    // modelled-constraint boundary ahead of the handler, so only their status is pinned
    // here.
    let day_counts = [EXPIRATION_DAYS_MIN - 1, EXPIRATION_DAYS_MAX + 1];
    let mut refusals: Vec<(String, Reply, u16, Option<&str>)> = Vec::new();
    for days in day_counts {
        let reply = emulator
            .put_expiration_settings(
                Some(CREATOR),
                &arn,
                &json!({
                    "ExpirationSettings": {
                        "ExpirationDays": days,
                        "ExpirationCriterion": EXPIRATION_CRITERIA[0],
                    }
                }),
            )
            .await;
        let label = format!("ExpirationDays {days}, one past a modelled bound");
        refusals.push((label, reply, 400, None));
    }
    refusals.push((
        "an ExpirationCriterion outside the catalog".to_string(),
        emulator
            .put_expiration_settings(
                Some(CREATOR),
                &arn,
                &json!({
                    "ExpirationSettings": {
                        "ExpirationDays": 7,
                        "ExpirationCriterion": "DELETED_TIMESTAMP",
                    }
                }),
            )
            .await,
        400,
        None,
    ));
    refusals.push((
        "a plain member of the channel".to_string(),
        emulator
            .put_expiration_settings(Some(MEMBER), &arn, &accepted)
            .await,
        403,
        Some("ForbiddenException"),
    ));
    refusals.push((
        "a caller that belongs to no channel".to_string(),
        emulator
            .put_expiration_settings(Some(OUTSIDER), &arn, &accepted)
            .await,
        403,
        Some("ForbiddenException"),
    ));
    refusals.push((
        "no x-amz-chime-bearer header".to_string(),
        emulator.put_expiration_settings(None, &arn, &accepted).await,
        400,
        Some("BadRequestException"),
    ));
    refusals.push((
        "an ARN naming no channel".to_string(),
        emulator
            .put_expiration_settings(
                Some(CREATOR),
                &format!("{APP_INSTANCE}/channel/never-created"),
                &accepted,
            )
            .await,
        404,
        Some("NotFoundException"),
    ));

    for (label, reply, expected_status, expected_code) in &refusals {
        assert_eq!(
            reply.status, *expected_status,
            "case {label:?}: PutChannelExpirationSettings must answer {expected_status}, got {} {:?}",
            reply.status, reply.body
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                reply.error_code(),
                *expected_code,
                "case {label:?}: wrong error code, body {:?}",
                reply.body
            );
        }
    }

    let after = emulator
        .channel_body("expiry refusals", &arn)
        .await
        .get("ExpirationSettings")
        .cloned();
    assert_eq!(
        after, in_force,
        "case {:?}: a refused put must leave the settings in force alone, got {after:?}",
        "every refusal above"
    );
}
