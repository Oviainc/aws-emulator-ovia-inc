// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of every `float` member this service reports.
//!
//! RULE: a member whose model target is a Smithy `float` is reported as the decimal ITS OWN
//! binary32 value spells — the shortest decimal that round-trips as an `f32`, carrying a
//! fractional part — so the decimal a client configured is the decimal it reads back, and
//! never the binary64 expansion of the binary32 bits (`29.969999313354492` for the value
//! nearest `29.97`). The rule is about the MEMBER, not about any one operation: it holds for
//! every `float` member of every shape this service serves, at every nesting depth.
//!
//! Where the catalogue comes from: `Video$framerate` targets `Framerate`, declared `float`
//! with `@range(min: 1, max: 60)`
//! (`smithy_query(query="shape",shape="Video,Framerate")`), and it is the only `float` the
//! `com.amazonaws.ivsrealtime` model declares — the two operations whose output carries a
//! `Video` are `CreateEncoderConfiguration` and `GetEncoderConfiguration`
//! (`smithy_query(query="shape",shape="EncoderConfiguration")`), and both are driven here.
//!
//! Three checks, because they can fail apart:
//!
//! * the TEXT on the wire, read off the response bytes before anything parses them, for a
//!   table of frame rates differing along what the rule generalises over — whether the value
//!   is whole, how many fractional digits it has, its magnitude, and the two published
//!   bounds of the member's own range;
//! * the VALUE a client reads back, compared against the value the request sent, on the
//!   create response and on the later read: the member round-trips exactly;
//! * the member's `@range` is still refused at both published bounds, which is what says the
//!   output-side rendering did not cost the INPUT its declared constraint.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The member under test, as it goes onto the wire.
const FRAMERATE: &str = "framerate";

/// The frame rate an encoder configuration takes when the request names none, from
/// `Video$framerate`'s own doc ("Video frame rate, in fps. Default: 30").
const DEFAULT_FRAMERATE: f64 = 30.0;

/// The published bounds of `Framerate`'s `@range`, from
/// `smithy_query(query="shape",shape="Framerate")`.
const FRAMERATE_RANGE: (f64, f64) = (1.0, 60.0);

/// The wire error code a refused request states.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One answer from the emulator: the status, the response bytes exactly as they went onto
/// the wire, and the parsed body.
struct Answer {
    status: u16,
    text: String,
    body: Value,
}

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
    async fn call(&self, operation: &str, body: &Value) -> Answer {
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
        let text = String::from_utf8_lossy(&bytes).to_string();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer { status, text, body }
    }
}

/// The error code `answer` states, or `"<absent>"` so a failing assertion names what came
/// back rather than panicking inside the reader.
fn error_code(answer: &Answer) -> &str {
    answer.body["__type"].as_str().unwrap_or("<absent>")
}

/// The reported frame rate of `answer`'s configuration, or `None` when the member is absent.
fn reported_framerate(answer: &Answer) -> Option<f64> {
    answer.body["encoderConfiguration"]["video"][FRAMERATE].as_f64()
}

/// The rows the wire-form table spans: the frame rate a request sends, and the decimal text
/// the response must carry for it.
///
/// Each row differs from the others in what the rule generalises over — whether the value is
/// whole, how many fractional digits it carries, its magnitude — and the two bounds of the
/// member's own published range are rows of their own. The fractional broadcast rates
/// (`29.97`, `59.94`, `23.976`) are the values whose binary32 and binary64 renderings differ.
fn wire_form_rows() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        ("the published minimum", json!(FRAMERATE_RANGE.0), "1.0"),
        ("the published maximum", json!(FRAMERATE_RANGE.1), "60.0"),
        ("a whole rate", json!(50), "50.0"),
        ("the documented default, sent outright", json!(30), "30.0"),
        ("one fractional digit", json!(29.5), "29.5"),
        ("the NTSC rate", json!(29.97), "29.97"),
        ("the NTSC progressive rate", json!(59.94), "59.94"),
        ("the film-to-video rate", json!(23.976), "23.976"),
        ("a rate at the bottom of the range", json!(1.5), "1.5"),
    ]
}

/// RULE, first and second readings: for every row of the class, the frame rate on the wire
/// is the decimal its own binary32 value spells, and the value a client reads back — on the
/// create response and on the later read — is the value the request sent.
#[tokio::test]
async fn a_float_member_is_reported_as_the_decimal_its_own_value_spells() {
    let emulator = RealTime::new();

    for (case, requested, expected_text) in wire_form_rows() {
        let created = emulator
            .call(
                "CreateEncoderConfiguration",
                &json!({ "video": { FRAMERATE: requested } }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateEncoderConfiguration must answer HTTP 200, got {} {}",
            created.status, created.body
        );

        let wire_member = format!("\"{FRAMERATE}\":{expected_text}");
        assert!(
            created.text.contains(&wire_member),
            "case {case:?}: expected the create response to carry {wire_member:?}, \
             got {:?}",
            created.text
        );

        let sent = requested.as_f64().expect("the row sends a number");
        assert_eq!(
            reported_framerate(&created),
            Some(sent),
            "case {case:?}: expected the create to report the frame rate {sent} it was sent, \
             got {}",
            created.body
        );

        let arn = created.body["encoderConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: expected a minted ARN, got {}",
            created.body
        );

        let read = emulator
            .call("GetEncoderConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetEncoderConfiguration must answer HTTP 200, got {} {}",
            read.status, read.body
        );
        assert!(
            read.text.contains(&wire_member),
            "case {case:?}: expected the read to carry {wire_member:?}, got {:?}",
            read.text
        );
        assert_eq!(
            reported_framerate(&read),
            Some(sent),
            "case {case:?}: expected the read to report the frame rate {sent} the create was \
             sent, got {}",
            read.body
        );
    }
}

/// RULE, first reading, on the value no request sends: a configuration created with no
/// `video` at all reports the documented default frame rate in the same decimal form.
#[tokio::test]
async fn the_defaulted_frame_rate_is_reported_in_the_same_form() {
    let emulator = RealTime::new();
    let case = "no video configuration at all";

    let created = emulator.call("CreateEncoderConfiguration", &json!({})).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateEncoderConfiguration must answer HTTP 200, got {} {}",
        created.status, created.body
    );
    let wire_member = format!("\"{FRAMERATE}\":{DEFAULT_FRAMERATE:.1}");
    assert!(
        created.text.contains(&wire_member),
        "case {case:?}: expected the response to carry {wire_member:?}, got {:?}",
        created.text
    );
    assert_eq!(
        reported_framerate(&created),
        Some(DEFAULT_FRAMERATE),
        "case {case:?}: expected the documented default {DEFAULT_FRAMERATE}, got {}",
        created.body
    );
}

/// RULE, third reading: the member's own `@range` is still enforced on the INPUT. One step
/// past each published bound is the modelled `ValidationException` (HTTP 400) carrying no
/// configuration, so rendering the output from the binary32 value did not cost the request
/// its declared constraint.
#[tokio::test]
async fn a_frame_rate_outside_the_published_range_is_still_refused() {
    let emulator = RealTime::new();

    // Each row is one step past a published bound, on both sides and in both forms a client
    // sends a number in — a whole value and a fractional one.
    let refused = [
        ("just below the minimum", json!(0.9)),
        ("a whole value below the minimum", json!(0)),
        ("just above the maximum", json!(60.1)),
        ("a whole value above the maximum", json!(61)),
    ];

    for (case, requested) in refused {
        let answer = emulator
            .call(
                "CreateEncoderConfiguration",
                &json!({ "video": { FRAMERATE: requested } }),
            )
            .await;
        assert_eq!(
            (answer.status, error_code(&answer)),
            (400, VALIDATION_EXCEPTION),
            "case {case:?}: expected HTTP 400 {VALIDATION_EXCEPTION}, got {} {}",
            answer.status,
            answer.body
        );
        assert!(
            reported_framerate(&answer).is_none(),
            "case {case:?}: a refused create must report no configuration, got {}",
            answer.body
        );
    }

    // Both bounds themselves are accepted, so the refusals above are the range's edges and
    // not a blanket rejection.
    for (case, requested) in [
        ("the published minimum", FRAMERATE_RANGE.0),
        ("the published maximum", FRAMERATE_RANGE.1),
    ] {
        let answer = emulator
            .call(
                "CreateEncoderConfiguration",
                &json!({ "video": { FRAMERATE: requested } }),
            )
            .await;
        assert_eq!(
            (answer.status, reported_framerate(&answer)),
            (200, Some(requested)),
            "case {case:?}: expected HTTP 200 reporting {requested}, got {} {}",
            answer.status,
            answer.body
        );
    }
}
