// SPDX-License-Identifier: CC0-1.0

//! The members this service MATERIALISES when the creating request leaves them out, and the
//! projections that report them.
//!
//! RULE: an optional string member a created resource materialises is reported by EVERY
//! projection of that resource — the create response, the single-resource read, the update
//! response and a listing's summary — carrying the value the creating request gave, or the
//! empty string where it gave none. A projection never decides whether to carry such a
//! member, so a client reading it finds a value rather than having to tell "no value" apart
//! from "the member is not served".
//!
//! Which members: an encoder configuration's `name` and an ingest configuration's `userId`.
//! Each targets a string whose `@length` admits the empty value
//! (`smithy_query(query="shape",shape="EncoderConfigurationName,UserId")` reports
//! `@length(0..128)` on both), so the empty string is a value the member can hold; the
//! recorded responses of both resources report the member as empty rather than omitting it
//! when the creating request named nothing. The same reading is already applied to the two
//! members whose own documentation states the empty default outright — a stage's
//! `activeSessionId` and an ingest configuration's `stageArn` — which
//! `tests/all/documented_member_defaults.rs` pins.
//!
//! The rows of each table differ along what the rule generalises over — whether the member
//! was sent at all, and the value sent (the empty string the member's length admits, a short
//! value, and one at the published maximum length) — and each row is checked on every
//! projection of its resource, never on the create alone.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The published maximum length of `EncoderConfigurationName` and of `UserId`, from
/// `smithy_query(query="shape",shape="EncoderConfigurationName,UserId")`.
const MAX_MEMBER_LENGTH: usize = 128;

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `operation` with `request`, asserted to have answered HTTP 200, reporting the body.
    async fn succeed(&self, case: &str, operation: &str, request: &Value) -> Value {
        let (status, body) = self.call(operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer HTTP 200, got {status} {body}"
        );
        body
    }
}

/// The resource `value` reports under `key`, or `Value::Null` when the projection carries
/// none — so a failing assertion names what came back rather than panicking in the reader.
fn resource(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

/// Assert `projection` carries `member` as a STRING equal to `expected`.
///
/// The member's PRESENCE is the half of the rule a value comparison alone would miss: an
/// absent member and one carrying the empty string are not the same answer to a client.
fn assert_member(case: &str, projection_name: &str, projection: &Value, member: &str, expected: &str) {
    let found = projection.get(member);
    assert!(
        found.is_some_and(Value::is_string),
        "case {case:?}: expected {projection_name} to carry {member} as a string, \
         got {found:?} in {projection}"
    );
    let actual = found.and_then(Value::as_str).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {projection_name} to report {member} {expected:?}, \
         got {actual:?} in {projection}"
    );
}

/// The one summary of `arn` in a listing, or `Value::Null` when the page holds none.
fn summary_of(page: &Value, key: &str, arn: &str) -> Value {
    page[key]
        .as_array()
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| summary["arn"].as_str() == Some(arn))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// The rows both tables span: whether the member is sent, and the value a projection must
/// report for it.
///
/// `None` is the row the recording is about — the member left out entirely — and the rest
/// are the values the member's own `@length(0..128)` admits: the empty string, a short
/// value, and one at the published maximum.
fn member_rows() -> Vec<(&'static str, Option<String>, String)> {
    vec![
        ("the member left out entirely", None, String::new()),
        (
            "the empty string, which the member's length admits",
            Some(String::new()),
            String::new(),
        ),
        (
            "a short value",
            Some("studio-1".to_string()),
            "studio-1".to_string(),
        ),
        (
            "a value at the published maximum length",
            Some("a".repeat(MAX_MEMBER_LENGTH)),
            "a".repeat(MAX_MEMBER_LENGTH),
        ),
    ]
}

/// RULE on an encoder configuration's `name`: every projection of the configuration — the
/// create response, the read, and the listing's summary — reports the member, with the value
/// the create was given or the empty string where it was given none.
#[tokio::test]
async fn an_encoder_configurations_name_is_reported_by_every_projection() {
    let emulator = RealTime::new();

    for (case, sent, expected) in member_rows() {
        let request = match &sent {
            Some(name) => json!({ "name": name }),
            None => json!({}),
        };
        let created = emulator
            .succeed(case, "CreateEncoderConfiguration", &request)
            .await;
        let configuration = resource(&created, "encoderConfiguration");
        assert_member(case, "the create response", &configuration, "name", &expected);

        let arn = configuration["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: expected a minted ARN, got {configuration}"
        );

        let read = emulator
            .succeed(
                case,
                "GetEncoderConfiguration",
                &json!({ "arn": arn.clone() }),
            )
            .await;
        assert_member(
            case,
            "the read",
            &resource(&read, "encoderConfiguration"),
            "name",
            &expected,
        );

        let page = emulator
            .succeed(case, "ListEncoderConfigurations", &json!({}))
            .await;
        let summary = summary_of(&page, "encoderConfigurations", &arn);
        assert_member(case, "the listing's summary", &summary, "name", &expected);
    }
}

/// RULE on an ingest configuration's `userId`: every projection of the configuration — the
/// create response, the read, the update response and the listing's summary — reports the
/// member, with the value the create was given or the empty string where it was given none.
#[tokio::test]
async fn an_ingest_configurations_user_id_is_reported_by_every_projection() {
    let emulator = RealTime::new();

    for (case, sent, expected) in member_rows() {
        let mut request = json!({ "ingestProtocol": "RTMPS" });
        if let Some(user_id) = &sent {
            request["userId"] = json!(user_id);
        }
        let created = emulator
            .succeed(case, "CreateIngestConfiguration", &request)
            .await;
        let configuration = resource(&created, "ingestConfiguration");
        assert_member(
            case,
            "the create response",
            &configuration,
            "userId",
            &expected,
        );

        let arn = configuration["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: expected a minted ARN, got {configuration}"
        );

        let read = emulator
            .succeed(
                case,
                "GetIngestConfiguration",
                &json!({ "arn": arn.clone() }),
            )
            .await;
        assert_member(
            case,
            "the read",
            &resource(&read, "ingestConfiguration"),
            "userId",
            &expected,
        );

        // An update that changes nothing about the identity still reports it: the member is
        // the configuration's, not the request's.
        let updated = emulator
            .succeed(
                case,
                "UpdateIngestConfiguration",
                &json!({ "arn": arn.clone() }),
            )
            .await;
        assert_member(
            case,
            "the update response",
            &resource(&updated, "ingestConfiguration"),
            "userId",
            &expected,
        );

        let page = emulator
            .succeed(case, "ListIngestConfigurations", &json!({}))
            .await;
        let summary = summary_of(&page, "ingestConfigurations", &arn);
        assert_member(case, "the listing's summary", &summary, "userId", &expected);
    }
}
