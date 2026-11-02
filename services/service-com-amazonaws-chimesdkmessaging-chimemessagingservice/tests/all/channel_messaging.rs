// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Chime SDK Messaging channel-messaging slice.
//!
//! Every case drives real HTTP through the production router — restJson1 request on the
//! wire in, restJson1 response out — so nothing here can pass by reading a handler's
//! Rust types directly. The rule under test throughout is that each operation computes
//! its answer from the SAME persisted channel store: what `CreateChannel` /
//! `SendChannelMessage` wrote is what `DescribeChannel` / `GetChannelMessage` /
//! `ListChannelMessages` / `ListChannels` report, and what `DeleteChannel` removed is
//! gone for all of them.
//!
//! The tables span the classes the rules generalise over — every `ChannelMode`,
//! `ChannelPrivacy`, `ChannelMessageType` and `ChannelMessagePersistenceType` wire
//! value, optional members present and absent, page sizes at and past the modelled
//! `MaxResults` bounds, and both `SortOrder` values — rather than re-running one
//! recorded request.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::state::{
    MAX_MESSAGES_PER_CHANNEL, MAX_MESSAGE_ATTRIBUTES, MAX_MESSAGE_ATTRIBUTE_VALUES,
    MAX_MESSAGE_BYTES, MAX_MESSAGE_BYTES_PER_CHANNEL, MAX_STORED_CHANNELS,
};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `AppInstanceArn` most cases create their channels under.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";
/// A second app instance, so the `ListChannels` filter has something to exclude.
const OTHER_APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai2";
/// The `x-amz-chime-bearer` identity every request is made as.
const BEARER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/alice";
// The `Message` this service answers an authorization refusal with is stated ONCE for
// this test binary, in `channel_membership.rs`, and read from there.
use crate::channel_membership::INSUFFICIENT_ACCESS;

/// One response, reduced to the two things the wire contract pins: the HTTP status and
/// the JSON body.
///
/// `pub(crate)` for the reason [`encode`] is: the sibling suites in this ONE test binary
/// address the same restJson1 surface, so they read one reply type rather than a copy
/// each.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Value,
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

/// The emulator under test: this one service mounted in the production router.
///
/// `pub(crate)` for the reason [`encode`] is.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
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

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> Reply {
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
            .header("x-amz-chime-bearer", BEARER)
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

    pub(crate) async fn create_channel(&self, request: &Value) -> Reply {
        self.call("POST", "/channels", Some(request)).await
    }

    pub(crate) async fn describe_channel(&self, channel_arn: &str) -> Reply {
        self.call("GET", &format!("/channels/{}", encode(channel_arn)), None)
            .await
    }

    async fn delete_channel(&self, channel_arn: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/channels/{}", encode(channel_arn)),
            None,
        )
        .await
    }

    pub(crate) async fn send_message(&self, channel_arn: &str, request: &Value) -> Reply {
        self.call(
            "POST",
            &format!("/channels/{}/messages", encode(channel_arn)),
            Some(request),
        )
        .await
    }

    pub(crate) async fn get_message(&self, channel_arn: &str, message_id: &str) -> Reply {
        self.call(
            "GET",
            &format!(
                "/channels/{}/messages/{}",
                encode(channel_arn),
                encode(message_id)
            ),
            None,
        )
        .await
    }

    async fn list_messages(&self, channel_arn: &str, params: &[(&str, String)]) -> Reply {
        self.call(
            "GET",
            &format!(
                "/channels/{}/messages{}",
                encode(channel_arn),
                query(params)
            ),
            None,
        )
        .await
    }

    async fn list_channels(&self, params: &[(&str, String)]) -> Reply {
        self.call("GET", &format!("/channels{}", query(params)), None)
            .await
    }

    /// Every message the channel still holds, oldest first, read through as many pages
    /// as it takes.
    async fn all_message_contents(&self, channel_arn: &str) -> Vec<String> {
        let mut contents = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![
                ("sort-order", "ASCENDING".to_string()),
                ("max-results", "50".to_string()),
            ];
            if let Some(token) = &token {
                params.push(("next-token", token.clone()));
            }
            let reply = self.list_messages(channel_arn, &params).await;
            assert_eq!(
                reply.status, 200,
                "ListChannelMessages must answer 200 while draining the channel, got {} {:?}",
                reply.status, reply.body
            );
            for summary in reply.body["ChannelMessages"]
                .as_array()
                .expect("ChannelMessages must be a list")
            {
                contents.push(
                    summary["Content"]
                        .as_str()
                        .expect("every summary carries its Content")
                        .to_string(),
                );
            }
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => return contents,
            }
        }
    }

    /// Create a channel with only the required members and return its ARN.
    pub(crate) async fn channel(&self, app_instance_arn: &str, name: &str, token: &str) -> String {
        let reply = self
            .create_channel(&json!({
                "AppInstanceArn": app_instance_arn,
                "Name": name,
                "ClientRequestToken": token,
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: CreateChannel must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        reply.body["ChannelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: CreateChannel must return a ChannelArn"))
            .to_string()
    }
}

/// Percent-encode a value for a URI path segment or query value, the way an AWS SDK
/// encodes an `@httpLabel`: everything outside RFC 3986's unreserved set, so an ARN's
/// `:` and `/` are escaped and the whole ARN stays inside one segment.
///
/// `pub(crate)` because the sibling suites in this ONE test binary (`tests/all.rs`) all
/// address the same restJson1 surface and must encode a label identically; a second copy
/// could drift from this one.
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => write!(out, "%{other:02X}").expect("write into a String"),
        }
    }
    out
}

/// Assemble a query string from already-decoded pairs. `pub(crate)` for the reason
/// [`encode`] is.
pub(crate) fn query(params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = params
        .iter()
        .map(|(key, value)| format!("{key}={}", encode(value)))
        .collect();
    format!("?{}", pairs.join("&"))
}

/// Now, formatted the way a `@httpQuery` timestamp arrives on restJson1: RFC 3339
/// `date-time`, which is the protocol's default format for a query-bound timestamp.
fn rfc3339_now() -> String {
    DateTime::from(SystemTime::now())
        .fmt(Format::DateTime)
        .expect("format the current time as RFC 3339")
}

/// Long enough that two instants taken either side of it differ in epoch milliseconds.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

// ---------------------------------------------------------------------------
// CreateChannel -> DescribeChannel
// ---------------------------------------------------------------------------

/// One row of the create/describe table.
#[derive(Debug)]
struct ChannelCase {
    label: &'static str,
    name: &'static str,
    mode: Option<&'static str>,
    privacy: Option<&'static str>,
    metadata: Option<&'static str>,
    channel_id: Option<&'static str>,
    expected_mode: &'static str,
    expected_privacy: &'static str,
}

/// RULE: `DescribeChannel` reports exactly what `CreateChannel` stored, with `Mode`
/// defaulting to `UNRESTRICTED` and `Privacy` to `PUBLIC`, the ARN derived from
/// `AppInstanceArn` + `/channel/` + the channel's id, and `CreatedBy` the calling
/// bearer.
///
/// The rows span both `ChannelMode` and both `ChannelPrivacy` wire values (the catalogs
/// the model declares for those enums), `Metadata` absent / empty / set, and a
/// caller-supplied `ChannelId` versus a generated one.
#[tokio::test]
async fn describe_channel_reports_what_create_channel_stored() {
    let cases = [
        ChannelCase {
            label: "required members only — both defaults apply",
            name: "demo",
            mode: None,
            privacy: None,
            metadata: None,
            channel_id: None,
            expected_mode: "UNRESTRICTED",
            expected_privacy: "PUBLIC",
        },
        ChannelCase {
            label: "restricted + private + metadata",
            name: "ops-room",
            mode: Some("RESTRICTED"),
            privacy: Some("PRIVATE"),
            metadata: Some("{\"topic\":\"ops\"}"),
            channel_id: None,
            expected_mode: "RESTRICTED",
            expected_privacy: "PRIVATE",
        },
        ChannelCase {
            label: "unrestricted + private",
            name: "hallway",
            mode: Some("UNRESTRICTED"),
            privacy: Some("PRIVATE"),
            metadata: None,
            channel_id: None,
            expected_mode: "UNRESTRICTED",
            expected_privacy: "PRIVATE",
        },
        ChannelCase {
            label: "restricted + public + empty metadata (Metadata is len 0..1024)",
            name: "announcements",
            mode: Some("RESTRICTED"),
            privacy: Some("PUBLIC"),
            metadata: Some(""),
            channel_id: None,
            expected_mode: "RESTRICTED",
            expected_privacy: "PUBLIC",
        },
        ChannelCase {
            label: "caller-supplied ChannelId",
            name: "fixed-id",
            mode: None,
            privacy: None,
            metadata: None,
            channel_id: Some("my-channel.1"),
            expected_mode: "UNRESTRICTED",
            expected_privacy: "PUBLIC",
        },
        ChannelCase {
            label: "name with punctuation and non-ASCII text",
            name: "équipe — #1 (général)",
            mode: None,
            privacy: None,
            metadata: None,
            channel_id: None,
            expected_mode: "UNRESTRICTED",
            expected_privacy: "PUBLIC",
        },
    ];

    let emulator = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let mut request = json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": case.name,
            "ClientRequestToken": format!("create-token-{index}"),
        });
        for (member, value) in [
            ("Mode", case.mode),
            ("Privacy", case.privacy),
            ("Metadata", case.metadata),
            ("ChannelId", case.channel_id),
        ] {
            if let Some(value) = value {
                request[member] = json!(value);
            }
        }

        let created = emulator.create_channel(&request).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateChannel must answer 201, got {} {:?}",
            case.label, created.status, created.body
        );
        let arn = created.body["ChannelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no ChannelArn in {:?}", case.label, created.body))
            .to_string();
        if let Some(channel_id) = case.channel_id {
            assert_eq!(
                arn,
                format!("{APP_INSTANCE}/channel/{channel_id}"),
                "case {:?}: the ARN must carry the ChannelId the caller chose, got {arn:?}",
                case.label
            );
        } else {
            let prefix = format!("{APP_INSTANCE}/channel/");
            assert!(
                arn.starts_with(&prefix) && arn.len() > prefix.len(),
                "case {:?}: the ARN must be the AppInstanceArn plus a generated channel id, got {arn:?}",
                case.label
            );
        }

        let described = emulator.describe_channel(&arn).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeChannel must answer 200, got {} {:?}",
            case.label, described.status, described.body
        );
        let channel = &described.body["Channel"];
        assert_eq!(
            channel["ChannelArn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: wrong ChannelArn, got {:?}",
            case.label,
            channel["ChannelArn"]
        );
        assert_eq!(
            channel["Name"].as_str(),
            Some(case.name),
            "case {:?}: wrong Name, got {:?}",
            case.label,
            channel["Name"]
        );
        assert_eq!(
            channel["Mode"].as_str(),
            Some(case.expected_mode),
            "case {:?}: wrong Mode, got {:?}",
            case.label,
            channel["Mode"]
        );
        assert_eq!(
            channel["Privacy"].as_str(),
            Some(case.expected_privacy),
            "case {:?}: wrong Privacy, got {:?}",
            case.label,
            channel["Privacy"]
        );
        assert_eq!(
            channel.get("Metadata").and_then(Value::as_str),
            case.metadata,
            "case {:?}: wrong Metadata, got {:?}",
            case.label,
            channel.get("Metadata")
        );
        assert_eq!(
            channel["CreatedBy"]["Arn"].as_str(),
            Some(BEARER),
            "case {:?}: CreatedBy must be the x-amz-chime-bearer identity, got {:?}",
            case.label,
            channel["CreatedBy"]
        );
        assert!(
            channel["CreatedTimestamp"].is_number(),
            "case {:?}: CreatedTimestamp must be set, got {:?}",
            case.label,
            channel["CreatedTimestamp"]
        );
        assert!(
            channel["LastUpdatedTimestamp"].is_number(),
            "case {:?}: LastUpdatedTimestamp must be set, got {:?}",
            case.label,
            channel["LastUpdatedTimestamp"]
        );
        assert!(
            channel.get("LastMessageTimestamp").is_none(),
            "case {:?}: a channel with no message must carry no LastMessageTimestamp, got {:?}",
            case.label,
            channel.get("LastMessageTimestamp")
        );
    }
}

/// RULE: a channel id that already names a channel is a conflict, not a second channel.
#[tokio::test]
async fn creating_a_channel_twice_under_one_id_conflicts() {
    let emulator = Emulator::new();
    let request = json!({
        "AppInstanceArn": APP_INSTANCE,
        "Name": "duplicate",
        "ChannelId": "taken",
        "ClientRequestToken": "first-token",
    });
    let first = emulator.create_channel(&request).await;
    assert_eq!(
        first.status, 201,
        "the first CreateChannel must answer 201, got {} {:?}",
        first.status, first.body
    );

    let mut second = request.clone();
    second["ClientRequestToken"] = json!("a-different-token");
    let reply = emulator.create_channel(&second).await;
    assert_eq!(
        reply.status, 409,
        "case {:?}: a taken ChannelId must answer 409, got {} {:?}",
        "same ChannelId, different token", reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ConflictException",
        "case {:?}: wrong error code, body {:?}",
        "same ChannelId, different token",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// SendChannelMessage -> GetChannelMessage
// ---------------------------------------------------------------------------

/// One row of the send/get table.
#[derive(Debug)]
struct MessageCase {
    label: &'static str,
    content: String,
    kind: &'static str,
    persistence: &'static str,
    metadata: Option<&'static str>,
    content_type: Option<&'static str>,
    attributes: Option<Value>,
}

/// RULE: `GetChannelMessage` returns the message `SendChannelMessage` appended to the
/// channel — its content, type, persistence, metadata, content type and attributes as
/// sent, its `Sender` the calling bearer, `Redacted` false and `Status.Value` `SENT` —
/// for a PERSISTENT message. A NON_PERSISTENT message is accepted and NOT persisted, so
/// nothing reports it afterwards and it does not advance the channel's
/// `LastMessageTimestamp`.
///
/// The rows cover every combination of the `ChannelMessageType` and
/// `ChannelMessagePersistenceType` catalogs the model declares, with the optional
/// members present and absent and content of several sizes and shapes.
#[tokio::test]
async fn get_channel_message_returns_the_message_that_was_sent() {
    let cases = [
        MessageCase {
            label: "standard + persistent, nothing optional",
            content: "hello".to_string(),
            kind: "STANDARD",
            persistence: "PERSISTENT",
            metadata: None,
            content_type: None,
            attributes: None,
        },
        MessageCase {
            label: "control + persistent, with metadata",
            content: "ping".to_string(),
            kind: "CONTROL",
            persistence: "PERSISTENT",
            metadata: Some("{\"kind\":\"ping\"}"),
            content_type: None,
            attributes: None,
        },
        MessageCase {
            label: "standard + non-persistent, 1024-character content",
            content: "x".repeat(1024),
            kind: "STANDARD",
            persistence: "NON_PERSISTENT",
            metadata: None,
            content_type: None,
            attributes: None,
        },
        MessageCase {
            label: "control + non-persistent, content type and attributes",
            content: "{}".to_string(),
            kind: "CONTROL",
            persistence: "NON_PERSISTENT",
            metadata: None,
            content_type: Some("application/json"),
            attributes: Some(json!({"priority": {"StringValues": ["high", "urgent"]}})),
        },
        MessageCase {
            label: "standard + persistent, multi-line non-ASCII content",
            content: "première ligne\nseconde ligne\t— fin".to_string(),
            kind: "STANDARD",
            persistence: "PERSISTENT",
            metadata: Some(""),
            content_type: Some(""),
            attributes: Some(json!({"a": {"StringValues": []}})),
        },
    ];

    let emulator = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let channel = emulator
            .channel(APP_INSTANCE, case.label, &format!("chan-{index}"))
            .await;

        let mut request = json!({
            "Content": case.content,
            "Type": case.kind,
            "Persistence": case.persistence,
            "ClientRequestToken": format!("msg-token-{index}"),
        });
        if let Some(metadata) = case.metadata {
            request["Metadata"] = json!(metadata);
        }
        if let Some(content_type) = case.content_type {
            request["ContentType"] = json!(content_type);
        }
        if let Some(attributes) = &case.attributes {
            request["MessageAttributes"] = attributes.clone();
        }

        let sent = emulator.send_message(&channel, &request).await;
        assert_eq!(
            sent.status, 201,
            "case {:?}: SendChannelMessage must answer 201, got {} {:?}",
            case.label, sent.status, sent.body
        );
        assert_eq!(
            sent.body["ChannelArn"].as_str(),
            Some(channel.as_str()),
            "case {:?}: wrong ChannelArn in the send response, got {:?}",
            case.label,
            sent.body["ChannelArn"]
        );
        let message_id = sent.body["MessageId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no MessageId in {:?}", case.label, sent.body))
            .to_string();
        // `MessageId` is modelled `len 1..128, pattern ^[-_a-zA-Z0-9]*$`.
        assert!(
            !message_id.is_empty()
                && message_id.len() <= 128
                && message_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "case {:?}: MessageId must match ^[-_a-zA-Z0-9]{{1,128}}$, got {message_id:?}",
            case.label
        );
        assert_eq!(
            sent.body["Status"]["Value"].as_str(),
            Some("SENT"),
            "case {:?}: wrong send Status, got {:?}",
            case.label,
            sent.body["Status"]
        );

        // `Persistence` is what the model says controls whether a message is persisted on
        // the back end, so a NON_PERSISTENT message is not there to be read back.
        if case.persistence == "NON_PERSISTENT" {
            let got = emulator.get_message(&channel, &message_id).await;
            assert_eq!(
                got.status, 403,
                "case {:?}: a NON_PERSISTENT message must not be readable, got {} {:?}",
                case.label, got.status, got.body
            );
            assert_eq!(
                got.error_code(),
                "ForbiddenException",
                "case {:?}: wrong error code for a NON_PERSISTENT read-back, body {:?}",
                case.label,
                got.body
            );
            let listed = emulator.list_messages(&channel, &[]).await;
            assert_eq!(
                listed.body["ChannelMessages"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                0,
                "case {:?}: a NON_PERSISTENT message must not be listed, got {:?}",
                case.label,
                listed.body
            );
            let described = emulator.describe_channel(&channel).await;
            let last_message = described.body["Channel"].get("LastMessageTimestamp");
            assert!(
                last_message.is_none(),
                "case {:?}: a NON_PERSISTENT send must not advance LastMessageTimestamp, got {last_message:?}",
                case.label
            );
            continue;
        }

        let got = emulator.get_message(&channel, &message_id).await;
        assert_eq!(
            got.status, 200,
            "case {:?}: GetChannelMessage must answer 200, got {} {:?}",
            case.label, got.status, got.body
        );
        let message = &got.body["ChannelMessage"];
        assert_eq!(
            message["ChannelArn"].as_str(),
            Some(channel.as_str()),
            "case {:?}: wrong ChannelArn, got {:?}",
            case.label,
            message["ChannelArn"]
        );
        assert_eq!(
            message["MessageId"].as_str(),
            Some(message_id.as_str()),
            "case {:?}: wrong MessageId, got {:?}",
            case.label,
            message["MessageId"]
        );
        assert_eq!(
            message["Content"].as_str(),
            Some(case.content.as_str()),
            "case {:?}: wrong Content, got {:?}",
            case.label,
            message["Content"]
        );
        assert_eq!(
            message["Type"].as_str(),
            Some(case.kind),
            "case {:?}: wrong Type, got {:?}",
            case.label,
            message["Type"]
        );
        assert_eq!(
            message["Persistence"].as_str(),
            Some(case.persistence),
            "case {:?}: wrong Persistence, got {:?}",
            case.label,
            message["Persistence"]
        );
        assert_eq!(
            message.get("Metadata").and_then(Value::as_str),
            case.metadata,
            "case {:?}: wrong Metadata, got {:?}",
            case.label,
            message.get("Metadata")
        );
        assert_eq!(
            message.get("ContentType").and_then(Value::as_str),
            case.content_type,
            "case {:?}: wrong ContentType, got {:?}",
            case.label,
            message.get("ContentType")
        );
        assert_eq!(
            message.get("MessageAttributes"),
            case.attributes.as_ref(),
            "case {:?}: wrong MessageAttributes, got {:?}",
            case.label,
            message.get("MessageAttributes")
        );
        assert_eq!(
            message["Sender"]["Arn"].as_str(),
            Some(BEARER),
            "case {:?}: Sender must be the x-amz-chime-bearer identity, got {:?}",
            case.label,
            message["Sender"]
        );
        assert_eq!(
            message["Redacted"].as_bool(),
            Some(false),
            "case {:?}: a message nobody redacted must report Redacted=false, got {:?}",
            case.label,
            message["Redacted"]
        );
        assert_eq!(
            message["Status"]["Value"].as_str(),
            Some("SENT"),
            "case {:?}: wrong Status, got {:?}",
            case.label,
            message["Status"]
        );
        assert!(
            message["CreatedTimestamp"].is_number(),
            "case {:?}: CreatedTimestamp must be set, got {:?}",
            case.label,
            message["CreatedTimestamp"]
        );
        assert!(
            message.get("LastEditedTimestamp").is_none(),
            "case {:?}: a message nobody edited must carry no LastEditedTimestamp, got {:?}",
            case.label,
            message.get("LastEditedTimestamp")
        );

        // Only a PERSISTENT message is kept for later readers, so only a PERSISTENT
        // message moves the channel's last-message time.
        let described = emulator.describe_channel(&channel).await;
        let last_message = described.body["Channel"].get("LastMessageTimestamp");
        assert!(
            last_message.is_some_and(Value::is_number),
            "case {:?}: a PERSISTENT send must advance LastMessageTimestamp, got {last_message:?}",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// RULE: replaying a request that carries an `@idempotencyToken` answers with the result
/// of the first one instead of creating a second resource — for both `CreateChannel` and
/// `SendChannelMessage`, and whatever else the replay repeats.
#[tokio::test]
async fn replaying_an_idempotency_token_returns_the_first_result() {
    let emulator = Emulator::new();

    for (label, request) in [
        (
            "required members only",
            json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": "idem-a",
                "ClientRequestToken": "tok-1",
            }),
        ),
        (
            "with an explicit mode and metadata",
            json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": "idem-b",
                "Mode": "RESTRICTED",
                "Metadata": "m",
                "ClientRequestToken": "tok-2",
            }),
        ),
    ] {
        let first = emulator.create_channel(&request).await;
        let second = emulator.create_channel(&request).await;
        assert_eq!(
            first.body["ChannelArn"], second.body["ChannelArn"],
            "case {label:?}: a replayed CreateChannel must return the first ChannelArn, got {:?} then {:?}",
            first.body, second.body
        );

        let listed = emulator
            .list_channels(&[("app-instance-arn", APP_INSTANCE.to_string())])
            .await;
        let names: Vec<&str> = listed.body["Channels"]
            .as_array()
            .expect("Channels must be a list")
            .iter()
            .filter_map(|c| c["Name"].as_str())
            .collect();
        let name = request["Name"].as_str().expect("the case has a Name");
        assert_eq!(
            names.iter().filter(|n| **n == name).count(),
            1,
            "case {label:?}: the replay must not create a second channel, got {names:?}"
        );
    }

    let channel = emulator.channel(APP_INSTANCE, "idem-msgs", "chan-idem").await;
    let send = json!({
        "Content": "once",
        "Type": "STANDARD",
        "Persistence": "PERSISTENT",
        "ClientRequestToken": "send-tok-1",
    });
    let first = emulator.send_message(&channel, &send).await;
    let second = emulator.send_message(&channel, &send).await;
    assert_eq!(
        first.body["MessageId"], second.body["MessageId"],
        "a replayed SendChannelMessage must return the first MessageId, got {:?} then {:?}",
        first.body, second.body
    );
    let listed = emulator.list_messages(&channel, &[]).await;
    assert_eq!(
        listed.body["ChannelMessages"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "a replayed SendChannelMessage must not append a second message, got {:?}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// ListChannelMessages
// ---------------------------------------------------------------------------

/// One row of the message-pagination table.
#[derive(Debug)]
struct PageCase {
    label: &'static str,
    total: usize,
    max_results: Option<i32>,
    sort_order: Option<&'static str>,
}

/// RULE: `ListChannelMessages` pages over the channel's stored messages ordered by
/// `CreatedTimestamp` — descending unless `sort-order` says otherwise — returning at
/// most `max-results` per page and a `NextToken` that resumes exactly where the page
/// stopped, so the pages together are every message, once each, in that order.
///
/// The rows vary the message count, the page size (including the modelled `MaxResults`
/// bounds 1 and 50, and an omitted one) and both `SortOrder` wire values.
#[tokio::test]
async fn list_channel_messages_pages_the_whole_ordered_view_exactly_once() {
    let cases = [
        PageCase {
            label: "one message, page size 1",
            total: 1,
            max_results: Some(1),
            sort_order: Some("DESCENDING"),
        },
        PageCase {
            label: "three messages, page size 1, default order",
            total: 3,
            max_results: Some(1),
            sort_order: None,
        },
        PageCase {
            label: "three messages, page size 2, ascending",
            total: 3,
            max_results: Some(2),
            sort_order: Some("ASCENDING"),
        },
        PageCase {
            label: "five messages, page size equal to the count",
            total: 5,
            max_results: Some(5),
            sort_order: Some("ASCENDING"),
        },
        PageCase {
            label: "seven messages, page size 3, descending",
            total: 7,
            max_results: Some(3),
            sort_order: Some("DESCENDING"),
        },
        PageCase {
            label: "four messages, page size at the modelled maximum of 50",
            total: 4,
            max_results: Some(50),
            sort_order: Some("ASCENDING"),
        },
        PageCase {
            label: "six messages, no page size given",
            total: 6,
            max_results: None,
            sort_order: Some("DESCENDING"),
        },
        PageCase {
            label: "no messages at all",
            total: 0,
            max_results: Some(2),
            sort_order: Some("ASCENDING"),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let channel = emulator
            .channel(APP_INSTANCE, case.label, &format!("page-chan-{index}"))
            .await;
        let contents: Vec<String> = (0..case.total).map(|n| format!("m{n}")).collect();
        for (n, content) in contents.iter().enumerate() {
            let reply = emulator
                .send_message(
                    &channel,
                    &json!({
                        "Content": content,
                        "Type": "STANDARD",
                        "Persistence": "PERSISTENT",
                        "ClientRequestToken": format!("page-msg-{n}"),
                    }),
                )
                .await;
            assert_eq!(
                reply.status, 201,
                "case {:?}: seeding message {n} must answer 201, got {} {:?}",
                case.label, reply.status, reply.body
            );
        }

        // Descending by creation time is the documented default, so an omitted
        // `sort-order` must page newest-first.
        let descending = case.sort_order.unwrap_or("DESCENDING") == "DESCENDING";
        let expected: Vec<String> = if descending {
            contents.iter().rev().cloned().collect()
        } else {
            contents.clone()
        };
        let page_limit = usize::try_from(case.max_results.unwrap_or(50)).expect("a positive limit");

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= case.total + 2,
                "case {:?}: paging did not terminate after {pages} pages",
                case.label
            );
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(sort_order) = case.sort_order {
                params.push(("sort-order", sort_order.to_string()));
            }
            if let Some(max_results) = case.max_results {
                params.push(("max-results", max_results.to_string()));
            }
            if let Some(token) = &token {
                params.push(("next-token", token.clone()));
            }
            let reply = emulator.list_messages(&channel, &params).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: ListChannelMessages must answer 200, got {} {:?}",
                case.label, reply.status, reply.body
            );
            assert_eq!(
                reply.body["ChannelArn"].as_str(),
                Some(channel.as_str()),
                "case {:?}: wrong ChannelArn, got {:?}",
                case.label,
                reply.body["ChannelArn"]
            );
            let page = reply.body["ChannelMessages"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: ChannelMessages must be a list, got {:?}",
                        case.label, reply.body
                    )
                })
                .clone();
            assert!(
                page.len() <= page_limit,
                "case {:?}: a page must hold at most {page_limit} messages, got {}",
                case.label,
                page.len()
            );
            for summary in &page {
                assert!(
                    summary["MessageId"].is_string(),
                    "case {:?}: every summary must carry a MessageId, got {summary:?}",
                    case.label
                );
                assert_eq!(
                    summary["Sender"]["Arn"].as_str(),
                    Some(BEARER),
                    "case {:?}: every summary must carry its Sender, got {summary:?}",
                    case.label
                );
                seen.push(
                    summary["Content"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!(
                                "case {:?}: every summary must carry its Content, got {summary:?}",
                                case.label
                            )
                        })
                        .to_string(),
                );
            }
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        seen.len() < expected.len(),
                        "case {:?}: a NextToken was returned after every message was listed ({seen:?})",
                        case.label
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {:?}: the pages together must be every message once, in order",
            case.label
        );
        let unique: HashSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {:?}: the pages must not repeat a message, got {seen:?}",
            case.label
        );
    }
}

/// One row of a list-filter table: what the request asks for, and the values the
/// response must carry back in order.
#[derive(Debug)]
struct FilterCase {
    label: &'static str,
    params: Vec<(&'static str, String)>,
    expected: Vec<&'static str>,
}

/// RULE: `not-before` and `not-after` bound the listed messages by `CreatedTimestamp` —
/// the starting and ending instant of the requested range.
#[tokio::test]
async fn list_channel_messages_honours_the_not_before_and_not_after_bounds() {
    let emulator = Emulator::new();
    let channel = emulator.channel(APP_INSTANCE, "ranged", "range-chan").await;

    let before_all = rfc3339_now();
    tick().await;
    for n in 0..3 {
        let reply = emulator
            .send_message(
                &channel,
                &json!({
                    "Content": format!("m{n}"),
                    "Type": "STANDARD",
                    "Persistence": "PERSISTENT",
                    "ClientRequestToken": format!("range-msg-{n}"),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "seeding message {n} must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        tick().await;
    }
    let after_all = rfc3339_now();

    // A bound taken between the second and the third send, so each half is non-empty.
    tick().await;
    let late_marker = rfc3339_now();
    tick().await;
    let reply = emulator
        .send_message(
            &channel,
            &json!({
                "Content": "m3",
                "Type": "STANDARD",
                "Persistence": "PERSISTENT",
                "ClientRequestToken": "range-msg-3",
            }),
        )
        .await;
    assert_eq!(
        reply.status, 201,
        "seeding the late message must answer 201, got {} {:?}",
        reply.status, reply.body
    );

    let cases = [
        FilterCase {
            label: "no bounds: every message",
            params: vec![("sort-order", "ASCENDING".to_string())],
            expected: vec!["m0", "m1", "m2", "m3"],
        },
        FilterCase {
            label: "not-before an instant before every send: every message",
            params: vec![
                ("sort-order", "ASCENDING".to_string()),
                ("not-before", before_all.clone()),
            ],
            expected: vec!["m0", "m1", "m2", "m3"],
        },
        FilterCase {
            label: "not-before the late marker: only the message sent after it",
            params: vec![
                ("sort-order", "ASCENDING".to_string()),
                ("not-before", late_marker.clone()),
            ],
            expected: vec!["m3"],
        },
        FilterCase {
            label: "not-after the late marker: only the messages sent before it",
            params: vec![
                ("sort-order", "ASCENDING".to_string()),
                ("not-after", late_marker.clone()),
            ],
            expected: vec!["m0", "m1", "m2"],
        },
        FilterCase {
            label: "both bounds, an empty window: nothing",
            params: vec![
                ("sort-order", "ASCENDING".to_string()),
                ("not-before", after_all.clone()),
                ("not-after", late_marker.clone()),
            ],
            expected: vec![],
        },
    ];

    for case in &cases {
        let label = case.label;
        let reply = emulator.list_messages(&channel, &case.params).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: ListChannelMessages must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let listed: Vec<&str> = reply.body["ChannelMessages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: ChannelMessages must be a list"))
            .iter()
            .filter_map(|m| m["Content"].as_str())
            .collect();
        assert_eq!(
            listed, case.expected,
            "case {label:?}: wrong messages inside the requested range"
        );
    }
}

// ---------------------------------------------------------------------------
// ListChannels
// ---------------------------------------------------------------------------

/// RULE: `ListChannels` returns the stored PUBLIC channels of the requested
/// `AppInstanceArn` only, paged by `max-results` with a `NextToken` that resumes the same
/// ordered view — and a request for `privacy=PRIVATE`, which the model reserves for an
/// `AppInstanceAdmin`, is refused with the modelled 403 rather than served.
#[tokio::test]
async fn list_channels_filters_by_app_instance_and_privacy_and_pages() {
    let emulator = Emulator::new();
    for (index, (app_instance, name, privacy)) in [
        (APP_INSTANCE, "pub-a", "PUBLIC"),
        (APP_INSTANCE, "pub-b", "PUBLIC"),
        (APP_INSTANCE, "pub-c", "PUBLIC"),
        (APP_INSTANCE, "priv-a", "PRIVATE"),
        (OTHER_APP_INSTANCE, "other", "PUBLIC"),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = emulator
            .create_channel(&json!({
                "AppInstanceArn": app_instance,
                "Name": name,
                "Privacy": privacy,
                "ClientRequestToken": format!("list-chan-{index}"),
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: CreateChannel must answer 201, got {} {:?}",
            reply.status, reply.body
        );
    }

    let cases = [
        FilterCase {
            label: "one app instance's channels",
            params: vec![("app-instance-arn", APP_INSTANCE.to_string())],
            expected: vec!["pub-a", "pub-b", "pub-c"],
        },
        FilterCase {
            label: "the other app instance's channels",
            params: vec![("app-instance-arn", OTHER_APP_INSTANCE.to_string())],
            expected: vec!["other"],
        },
        FilterCase {
            label: "narrowed to PUBLIC",
            params: vec![
                ("app-instance-arn", APP_INSTANCE.to_string()),
                ("privacy", "PUBLIC".to_string()),
            ],
            expected: vec!["pub-a", "pub-b", "pub-c"],
        },
    ];

    for case in &cases {
        let label = case.label;
        let reply = emulator.list_channels(&case.params).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: ListChannels must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let mut names: Vec<&str> = reply.body["Channels"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: Channels must be a list"))
            .iter()
            .filter_map(|c| c["Name"].as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names, case.expected,
            "case {label:?}: wrong channels listed"
        );
    }

    // Only an administrator may ask for the private channels, and this service holds
    // none: the request is refused rather than answered with the private channel it
    // stores.
    let private = emulator
        .list_channels(&[
            ("app-instance-arn", APP_INSTANCE.to_string()),
            ("privacy", "PRIVATE".to_string()),
        ])
        .await;
    assert_eq!(
        private.status, 403,
        "case {:?}: must be refused with 403, got {} {:?}",
        "narrowed to PRIVATE", private.status, private.body
    );
    assert_eq!(
        private.error_code(),
        "ForbiddenException",
        "case {:?}: wrong error code, body {:?}",
        "narrowed to PRIVATE",
        private.body
    );

    // Paged one at a time, the same three channels arrive once each.
    for page_size in [1, 2, 3] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![
                ("app-instance-arn", APP_INSTANCE.to_string()),
                ("max-results", page_size.to_string()),
            ];
            if let Some(token) = &token {
                params.push(("next-token", token.clone()));
            }
            let reply = emulator.list_channels(&params).await;
            assert_eq!(
                reply.status, 200,
                "case {page_size}-per-page: ListChannels must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = reply.body["Channels"]
                .as_array()
                .unwrap_or_else(|| panic!("case {page_size}-per-page: Channels must be a list"))
                .clone();
            assert!(
                page.len() <= page_size,
                "case {page_size}-per-page: a page must hold at most {page_size} channels, got {}",
                page.len()
            );
            for summary in &page {
                seen.push(
                    summary["ChannelArn"]
                        .as_str()
                        .expect("every summary carries a ChannelArn")
                        .to_string(),
                );
            }
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: HashSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            3,
            "case {page_size}-per-page: paging must yield each channel exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            3,
            "case {page_size}-per-page: paging must not repeat a channel, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteChannel
// ---------------------------------------------------------------------------

/// RULE: a deleted channel is gone for every later reader — `DescribeChannel`,
/// `GetChannelMessage` and `ListChannelMessages` refuse it and `ListChannels` no longer
/// lists it — and its messages go with it. A channel ARN this service cannot resolve is an
/// ACCESS decision, so each of those refusals is the modelled 403 `ForbiddenException`
/// carrying the service's fixed insufficient-access sentence, which is also the only
/// terminal client error those operations model.
#[tokio::test]
async fn deleting_a_channel_is_observable_to_every_later_read() {
    let emulator = Emulator::new();
    let kept = emulator.channel(APP_INSTANCE, "kept", "keep-chan").await;
    let doomed = emulator.channel(APP_INSTANCE, "doomed", "doom-chan").await;
    let sent = emulator
        .send_message(
            &doomed,
            &json!({
                "Content": "last words",
                "Type": "STANDARD",
                "Persistence": "PERSISTENT",
                "ClientRequestToken": "doom-msg",
            }),
        )
        .await;
    let message_id = sent.body["MessageId"]
        .as_str()
        .expect("the seeded message has an id")
        .to_string();

    let deleted = emulator.delete_channel(&doomed).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteChannel must answer 204, got {} {:?}",
        deleted.status, deleted.body
    );

    for (label, reply) in [
        ("DescribeChannel", emulator.describe_channel(&doomed).await),
        (
            "GetChannelMessage",
            emulator.get_message(&doomed, &message_id).await,
        ),
        (
            "ListChannelMessages",
            emulator.list_messages(&doomed, &[]).await,
        ),
        ("DeleteChannel again", emulator.delete_channel(&doomed).await),
    ] {
        assert_eq!(
            reply.status, 403,
            "case {label:?}: a deleted channel must answer 403, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ForbiddenException",
            "case {label:?}: wrong error code, body {:?}",
            reply.body
        );
        assert_eq!(
            reply.body["Message"].as_str(),
            Some(INSUFFICIENT_ACCESS),
            "case {label:?}: wrong refusal message, body {:?}",
            reply.body
        );
    }

    let listed = emulator
        .list_channels(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    let arns: Vec<&str> = listed.body["Channels"]
        .as_array()
        .expect("Channels must be a list")
        .iter()
        .filter_map(|c| c["ChannelArn"].as_str())
        .collect();
    assert_eq!(
        arns,
        vec![kept.as_str()],
        "ListChannels must list only the channel that was not deleted"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request naming a channel resource this service cannot resolve is refused with
/// an error the operation's own model declares, and the one it declares for a resource the
/// caller cannot reach is `ForbiddenException` (HTTP 403, `Code` `Forbidden`) carrying the
/// service's fixed insufficient-access sentence — for the CHANNEL (none of these
/// operations models `NotFoundException`) as much as for a MESSAGE the channel does not
/// hold, and never an empty success.
#[tokio::test]
async fn reads_of_a_resource_that_does_not_exist_are_refused() {
    let emulator = Emulator::new();
    let missing = format!("{APP_INSTANCE}/channel/does-not-exist");
    let channel = emulator.channel(APP_INSTANCE, "present", "present-chan").await;

    let absent_channel = [
        ("DescribeChannel", emulator.describe_channel(&missing).await),
        (
            "SendChannelMessage",
            emulator
                .send_message(
                    &missing,
                    &json!({
                        "Content": "into the void",
                        "Type": "STANDARD",
                        "Persistence": "PERSISTENT",
                        "ClientRequestToken": "void-token",
                    }),
                )
                .await,
        ),
        (
            "ListChannelMessages",
            emulator.list_messages(&missing, &[]).await,
        ),
        (
            "GetChannelMessage, unknown channel",
            emulator.get_message(&missing, "whatever").await,
        ),
        ("DeleteChannel", emulator.delete_channel(&missing).await),
    ];

    for (label, reply) in absent_channel {
        assert_eq!(
            reply.status, 403,
            "case {label:?}: must answer 403, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ForbiddenException",
            "case {label:?}: wrong error code, body {:?}",
            reply.body
        );
        assert_eq!(
            reply.body["Message"].as_str(),
            Some(INSUFFICIENT_ACCESS),
            "case {label:?}: wrong refusal message, body {:?}",
            reply.body
        );
    }

    let absent_message = [(
        "GetChannelMessage, unknown message in a known channel",
        emulator.get_message(&channel, "no-such-message").await,
    )];
    for (label, reply) in absent_message {
        assert_eq!(
            reply.status, 403,
            "case {label:?}: must answer 403, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ForbiddenException",
            "case {label:?}: wrong error code, body {:?}",
            reply.body
        );
    }
}

/// RULE: a request outside a modelled constraint or catalog is refused with HTTP 400,
/// never served. The rows walk each enum catalog with a value it does not contain, both
/// published `MaxResults` bounds and one past each, and a `NextToken` this service did
/// not issue.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    let channel = emulator.channel(APP_INSTANCE, "refusals", "refuse-chan").await;
    emulator
        .send_message(
            &channel,
            &json!({
                "Content": "seed",
                "Type": "STANDARD",
                "Persistence": "PERSISTENT",
                "ClientRequestToken": "refuse-msg",
            }),
        )
        .await;

    // A token whose cursor decodes but names nothing in the view.
    let stale_token = hex::encode(b"no-such-message-id");

    let refused = [
        (
            "Mode outside the ChannelMode catalog",
            emulator
                .create_channel(&json!({
                    "AppInstanceArn": APP_INSTANCE,
                    "Name": "bad-mode",
                    "Mode": "SIDEWAYS",
                    "ClientRequestToken": "bad-mode-token",
                }))
                .await,
        ),
        (
            "Privacy outside the ChannelPrivacy catalog",
            emulator
                .create_channel(&json!({
                    "AppInstanceArn": APP_INSTANCE,
                    "Name": "bad-privacy",
                    "Privacy": "TRANSLUCENT",
                    "ClientRequestToken": "bad-privacy-token",
                }))
                .await,
        ),
        (
            "Type outside the ChannelMessageType catalog",
            emulator
                .send_message(
                    &channel,
                    &json!({
                        "Content": "x",
                        "Type": "SHOUT",
                        "Persistence": "PERSISTENT",
                        "ClientRequestToken": "bad-type-token",
                    }),
                )
                .await,
        ),
        (
            "Persistence outside the ChannelMessagePersistenceType catalog",
            emulator
                .send_message(
                    &channel,
                    &json!({
                        "Content": "x",
                        "Type": "STANDARD",
                        "Persistence": "SOMETIMES",
                        "ClientRequestToken": "bad-persistence-token",
                    }),
                )
                .await,
        ),
        (
            "SubChannelId, which only an elastic channel could route",
            emulator
                .send_message(
                    &channel,
                    &json!({
                        "Content": "x",
                        "Type": "STANDARD",
                        "Persistence": "PERSISTENT",
                        "SubChannelId": "sub-1",
                        "ClientRequestToken": "sub-channel-token",
                    }),
                )
                .await,
        ),
        (
            "more MessageAttributes entries than the emulator stores",
            emulator
                .send_message(
                    &channel,
                    &json!({
                        "Content": "x",
                        "Type": "STANDARD",
                        "Persistence": "PERSISTENT",
                        "MessageAttributes": (0..MAX_MESSAGE_ATTRIBUTES + 1)
                            .map(|n| (format!("a{n}"), json!({"StringValues": ["v"]})))
                            .collect::<serde_json::Map<String, Value>>(),
                        "ClientRequestToken": "too-many-attributes",
                    }),
                )
                .await,
        ),
        (
            "more StringValues in one attribute than the emulator stores, all empty",
            emulator
                .send_message(
                    &channel,
                    &json!({
                        "Content": "x",
                        "Type": "STANDARD",
                        "Persistence": "PERSISTENT",
                        "MessageAttributes": {
                            "a": {
                                "StringValues": vec![""; MAX_MESSAGE_ATTRIBUTE_VALUES + 1],
                            },
                        },
                        "ClientRequestToken": "too-many-values",
                    }),
                )
                .await,
        ),
        (
            "sort-order outside the SortOrder catalog",
            emulator
                .list_messages(&channel, &[("sort-order", "SIDEWAYS".to_string())])
                .await,
        ),
        (
            "max-results one below the modelled minimum of 1",
            emulator
                .list_messages(&channel, &[("max-results", "0".to_string())])
                .await,
        ),
        (
            "max-results one above the modelled maximum of 50",
            emulator
                .list_messages(&channel, &[("max-results", "51".to_string())])
                .await,
        ),
        (
            "ListChannels max-results one above the modelled maximum of 50",
            emulator
                .list_channels(&[
                    ("app-instance-arn", APP_INSTANCE.to_string()),
                    ("max-results", "51".to_string()),
                ])
                .await,
        ),
        (
            "a next-token this service never issued",
            emulator
                .list_messages(&channel, &[("next-token", "not-a-token".to_string())])
                .await,
        ),
        (
            "a next-token naming a message that is not in the view",
            emulator
                .list_messages(&channel, &[("next-token", stale_token)])
                .await,
        ),
    ];

    for (label, reply) in refused {
        assert_eq!(
            reply.status, 400,
            "case {label:?}: must be refused with 400, got {} {:?}",
            reply.status, reply.body
        );
    }

    // A refused CreateChannel must not have left a channel behind.
    let listed = emulator
        .list_channels(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    let names: Vec<&str> = listed.body["Channels"]
        .as_array()
        .expect("Channels must be a list")
        .iter()
        .filter_map(|c| c["Name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["refusals"],
        "a refused CreateChannel must not persist a channel, got {names:?}"
    );

    // A refused SendChannelMessage must not have appended a message.
    let listed = emulator.list_messages(&channel, &[]).await;
    assert_eq!(
        listed.body["ChannelMessages"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "a refused SendChannelMessage must not append a message, got {:?}",
        listed.body
    );

    // Both published `MaxResults` bounds are accepted.
    for max_results in [1, 50] {
        let reply = emulator
            .list_messages(&channel, &[("max-results", max_results.to_string())])
            .await;
        assert_eq!(
            reply.status, 200,
            "case max-results={max_results}: a value at a published bound must be served, got {} {:?}",
            reply.status, reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// The caps on caller-driven state
// ---------------------------------------------------------------------------

/// RULE: the channel store is capped, so a caller that never deletes cannot grow it
/// without limit: past `MAX_STORED_CHANNELS` live channels `CreateChannel` answers the
/// modelled `ResourceLimitExceededException`, and deleting a channel frees a slot again.
#[tokio::test]
async fn the_channel_store_is_capped_and_a_delete_frees_a_slot() {
    let emulator = Emulator::new();
    for n in 0..MAX_STORED_CHANNELS {
        let reply = emulator
            .create_channel(&json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": format!("capped-{n}"),
                "ChannelId": format!("capped-{n}"),
                "ClientRequestToken": format!("cap-chan-{n}"),
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case channel {n} of the cap: CreateChannel must answer 201, got {} {:?}",
            reply.status, reply.body
        );
    }

    let over = json!({
        "AppInstanceArn": APP_INSTANCE,
        "Name": "one-too-many",
        "ChannelId": "one-too-many",
        "ClientRequestToken": "cap-chan-over",
    });
    let refused = emulator.create_channel(&over).await;
    assert_eq!(
        refused.status, 400,
        "case one past the cap: CreateChannel must be refused, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ResourceLimitExceededException",
        "case one past the cap: wrong error code, body {:?}",
        refused.body
    );

    let freed = format!("{APP_INSTANCE}/channel/capped-0");
    let deleted = emulator.delete_channel(&freed).await;
    assert_eq!(
        deleted.status, 204,
        "case freeing a slot: DeleteChannel must answer 204, got {} {:?}",
        deleted.status, deleted.body
    );
    let mut retry = over.clone();
    retry["ClientRequestToken"] = json!("cap-chan-over-2");
    let accepted = emulator.create_channel(&retry).await;
    assert_eq!(
        accepted.status, 201,
        "case after a delete freed a slot: CreateChannel must answer 201, got {} {:?}",
        accepted.status, accepted.body
    );
}

/// RULE: a channel keeps at most `MAX_MESSAGES_PER_CHANNEL` messages; past that the
/// OLDEST is dropped (and is then simply not there for `GetChannelMessage`), so a loop
/// of sends cannot grow the store without limit.
#[tokio::test]
async fn a_channel_keeps_at_most_the_capped_number_of_messages() {
    let emulator = Emulator::new();
    let channel = emulator.channel(APP_INSTANCE, "capped", "cap-msg-chan").await;

    let total = MAX_MESSAGES_PER_CHANNEL + 2;
    let mut first_message_id = String::new();
    for n in 0..total {
        let reply = emulator
            .send_message(
                &channel,
                &json!({
                    "Content": format!("m{n}"),
                    "Type": "STANDARD",
                    "Persistence": "PERSISTENT",
                    "ClientRequestToken": format!("cap-msg-{n}"),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case message {n} of {total}: SendChannelMessage must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        if n == 0 {
            first_message_id = reply.body["MessageId"]
                .as_str()
                .expect("the first send returns a MessageId")
                .to_string();
        }
    }

    let kept = emulator.all_message_contents(&channel).await;
    assert_eq!(
        kept.len(),
        MAX_MESSAGES_PER_CHANNEL,
        "case {total} sends: the channel must keep exactly {MAX_MESSAGES_PER_CHANNEL} messages, kept {}",
        kept.len()
    );
    assert_eq!(
        kept.first().map(String::as_str),
        Some("m2"),
        "case {total} sends: the two oldest messages must have been evicted, oldest kept is {:?}",
        kept.first()
    );
    assert_eq!(
        kept.last().map(String::as_str),
        Some(format!("m{}", total - 1)).as_deref(),
        "case {total} sends: the newest message must be kept, newest is {:?}",
        kept.last()
    );

    let evicted = emulator.get_message(&channel, &first_message_id).await;
    assert_eq!(
        evicted.status, 403,
        "case the evicted message: GetChannelMessage must refuse a message the channel no longer holds, got {} {:?}",
        evicted.status, evicted.body
    );
}

/// RULE: a channel keeps at most `MAX_MESSAGE_BYTES_PER_CHANNEL` of caller-supplied
/// message bytes — `Content` carries no modelled maximum, so a count cap alone would
/// bound nothing — and a message larger than `MAX_MESSAGE_BYTES` is refused outright
/// rather than stored, so no retention path escapes the budget.
#[tokio::test]
async fn a_channel_keeps_at_most_the_capped_number_of_message_bytes() {
    let emulator = Emulator::new();
    let channel = emulator.channel(APP_INSTANCE, "big", "cap-bytes-chan").await;

    // Each send carries the largest message the emulator accepts, and together they
    // carry more than one whole per-channel budget.
    let chunk = MAX_MESSAGE_BYTES;
    let sends = (MAX_MESSAGE_BYTES_PER_CHANNEL / chunk) + 4;
    let mut first_message_id = String::new();
    for n in 0..sends {
        let content = format!("{n:08}{}", "x".repeat(chunk - 8));
        let reply = emulator
            .send_message(
                &channel,
                &json!({
                    "Content": content,
                    "Type": "STANDARD",
                    "Persistence": "PERSISTENT",
                    "ClientRequestToken": format!("cap-bytes-{n}"),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "case big message {n}: a message at the accepted maximum must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        if n == 0 {
            first_message_id = reply.body["MessageId"]
                .as_str()
                .expect("the first send returns a MessageId")
                .to_string();
        }
    }

    let kept = emulator.all_message_contents(&channel).await;
    let kept_bytes: usize = kept.iter().map(String::len).sum();
    assert!(
        kept_bytes <= MAX_MESSAGE_BYTES_PER_CHANNEL,
        "case {sends} x {chunk}-byte sends: the channel must keep at most {MAX_MESSAGE_BYTES_PER_CHANNEL} bytes, kept {kept_bytes}"
    );
    assert!(
        kept.len() < sends,
        "case {sends} x {chunk}-byte sends: the oldest messages must have been evicted, kept {}",
        kept.len()
    );
    assert_eq!(
        kept.last().map(|content| &content[..8]),
        Some(&*format!("{:08}", sends - 1)),
        "case {sends} x {chunk}-byte sends: the newest message must be kept, newest starts {:?}",
        kept.last().map(|content| &content[..8])
    );
    let evicted = emulator.get_message(&channel, &first_message_id).await;
    assert_eq!(
        evicted.status, 403,
        "case the evicted big message: GetChannelMessage must refuse a message the channel no longer holds, got {} {:?}",
        evicted.status, evicted.body
    );

    // One byte past the accepted maximum is refused, and nothing is stored for it; the
    // maximum itself is served, and is readable back in full.
    let lone = emulator.channel(APP_INSTANCE, "lone", "cap-lone-chan").await;
    let oversized = emulator
        .send_message(
            &lone,
            &json!({
                "Content": "y".repeat(MAX_MESSAGE_BYTES + 1),
                "Type": "STANDARD",
                "Persistence": "PERSISTENT",
                "ClientRequestToken": "cap-lone-over",
            }),
        )
        .await;
    assert_eq!(
        oversized.status, 400,
        "case one byte past the accepted maximum: SendChannelMessage must be refused, got {} {:?}",
        oversized.status, oversized.body
    );
    assert_eq!(
        oversized.error_code(),
        "BadRequestException",
        "case one byte past the accepted maximum: wrong error code, body {:?}",
        oversized.body
    );
    assert!(
        emulator.all_message_contents(&lone).await.is_empty(),
        "case one byte past the accepted maximum: nothing may be stored for a refused send"
    );

    let at_maximum = "z".repeat(MAX_MESSAGE_BYTES);
    let accepted = emulator
        .send_message(
            &lone,
            &json!({
                "Content": at_maximum,
                "Type": "STANDARD",
                "Persistence": "PERSISTENT",
                "ClientRequestToken": "cap-lone-at-max",
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 201,
        "case exactly the accepted maximum: SendChannelMessage must answer 201, got {} {:?}",
        accepted.status, accepted.body
    );
    let message_id = accepted.body["MessageId"]
        .as_str()
        .expect("the send returns a MessageId");
    let got = emulator.get_message(&lone, message_id).await;
    assert_eq!(
        got.status, 200,
        "case exactly the accepted maximum: GetChannelMessage must answer 200, got {} {:?}",
        got.status, got.body
    );
    assert_eq!(
        got.body["ChannelMessage"]["Content"].as_str().map(str::len),
        Some(at_maximum.len()),
        "case exactly the accepted maximum: the whole Content must be readable back"
    );
}
