// SPDX-License-Identifier: CC0-1.0

//! The encoder-configuration surface over the wire: `CreateEncoderConfiguration`,
//! `GetEncoderConfiguration` and `DeleteEncoderConfiguration`.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. DELETE REMOVES ONE RESOURCE. `DeleteEncoderConfiguration` removes exactly the
//!    encoder configuration its `arn` names from this service's state: that ARN then
//!    names nothing (every later reference to it is the modelled
//!    `ResourceNotFoundException`, HTTP 404, including a second delete), every other
//!    configuration reads back unchanged, and the tagging surface no longer finds it.
//! 2. An ARN this store holds no configuration for is that same 404; a value outside
//!    the member's modelled `@pattern`/`@length` is the modelled `ValidationException`
//!    (HTTP 400), which is a different answer from "not found".
//! 3. ROUND TRIP. `CreateEncoderConfiguration` persists what the request describes and
//!    mints the ARN, and `GetEncoderConfiguration` reports that SAME configuration back
//!    for its ARN — member for member, on every read, with the model's documented
//!    default for every `Video` member the request left out.
//! 4. The two DOCUMENTED `Video` rules the model carries no trait for are enforced: a
//!    resolution is even, and `width` times `height` stays within the documented maximum
//!    total pixels.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// `EncoderConfigurationArn`'s `@pattern`, from
/// `smithy_query(query="shape",shape="EncoderConfiguration")` — member `arn`.
const ENCODER_CONFIGURATION_ARN_PATTERN: &str =
    "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:encoder-configuration/[a-zA-Z0-9-]+$";

/// The modelled `@length` maximum of `EncoderConfigurationArn`.
const ENCODER_CONFIGURATION_ARN_MAX_LENGTH: usize = 128;

// The documented defaults of `Video`, each from that member's own doc in
// `smithy_query(query="shape",shape="Video")`:
//   width     "Video-resolution width. … Default: 1280."
//   height    "Video-resolution height. … Default: 720."
//   framerate "Video frame rate, in fps. Default: 30."
//   bitrate   "Bitrate for generated output, in bps. Default: 2500000."
const DEFAULT_WIDTH: i64 = 1280;
const DEFAULT_HEIGHT: i64 = 720;
const DEFAULT_FRAMERATE: f64 = 30.0;
const DEFAULT_BITRATE: i64 = 2_500_000;

/// The documented maximum total pixels, from `Video$width` and `Video$height`: "the
/// maximum value is determined by `width` times `height`, such that the maximum total
/// pixels is 2073600 (1920x1080 or 1080x1920)".
const MAX_VIDEO_PIXELS: i64 = 2_073_600;

/// The modelled `@range` of `Width` and `Height` (`2..1920`), from
/// `smithy_query(query="shape",shape="Video")`.
const RESOLUTION_RANGE: (i64, i64) = (2, 1920);

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// [`Self::call_in`] against the region every test that does not vary it uses.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    /// `ListTagsForResource`, bound to `GET /tags/{resourceArn}`, carrying the ARN the
    /// way a client does: percent-encoded into one path segment.
    async fn list_tags(&self, arn: &str) -> (u16, Value) {
        self.send(
            "us-east-1",
            "GET",
            &format!("/tags/{}", encode_path_label(arn)),
            None,
        )
        .await
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an encoder configuration in `region` and return the
    /// `encoderConfiguration` the service reported for it.
    async fn create_in(&self, case: &str, region: &str, body: &Value) -> Value {
        let (status, response) = self
            .call_in(region, "CreateEncoderConfiguration", body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration {body} in {region} must answer HTTP 200, \
             got {status} {response}"
        );
        response["encoderConfiguration"].clone()
    }

    /// Read `arn` back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetEncoderConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEncoderConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["encoderConfiguration"].clone()
    }

    /// Delete `arn`, asserting the delete succeeded with the empty body its modelled
    /// output carries.
    async fn delete(&self, case: &str, arn: &str) {
        let (status, response) = self
            .call("DeleteEncoderConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEncoderConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        assert!(
            response.is_null() || response == json!({}),
            "case {case:?}: DeleteEncoderConfiguration on {arn} must answer an empty body, \
             got {response}"
        );
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Does `value` satisfy the `EncoderConfigurationArn` `@pattern` quoted above?
///
/// The pattern is simple enough to check directly, which keeps this crate's dependency
/// set exactly what the service itself needs.
fn matches_encoder_configuration_arn(value: &str) -> bool {
    // ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:encoder-configuration/[a-zA-Z0-9-]+$
    let Some(rest) = value.strip_prefix("arn:aws:ivs:") else {
        return false;
    };
    let Some((region, rest)) = rest.split_once(':') else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    let Some(id) = resource.strip_prefix("encoder-configuration/") else {
        return false;
    };
    let region_like = !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let id_like = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    region_like && !account.is_empty() && account.bytes().all(|b| b.is_ascii_digit()) && id_like
}

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// An integer member of a reported `Video`, read as `None` when the wire omits it.
fn integer(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// The create requests this suite spans the class of inputs with: each row differs from
/// the others in what the rules generalise over — whether a video configuration is
/// supplied at all, which of its members the request names, the resolution, the frame
/// rate and bitrate, and whether a name and tags are carried.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the smallest request the model allows",
            json!({}),
        ),
        (
            "a name and tags, no video",
            json!({ "name": "studio-720p", "tags": { "env": "test", "team": "video" } }),
        ),
        (
            "every video member supplied",
            json!({
                "name": "full",
                "video": { "width": 854, "height": 480, "framerate": 24, "bitrate": 1_200_000 },
            }),
        ),
        (
            "only the width supplied, the rest defaulted",
            json!({ "video": { "width": 640 } }),
        ),
        (
            "only the framerate supplied, the rest defaulted",
            json!({ "video": { "framerate": 60 } }),
        ),
        (
            "an empty video object: every member defaulted",
            json!({ "name": "", "video": {} }),
        ),
        (
            // The documented landscape maximum: 1920x1080 is exactly MAX_VIDEO_PIXELS.
            "the documented landscape maximum resolution",
            json!({ "name": "landscape-max", "video": { "width": 1920, "height": 1080 } }),
        ),
        (
            // Its portrait twin, the other form the same doc names.
            "the documented portrait maximum resolution",
            json!({ "name": "portrait-max", "video": { "width": 1080, "height": 1920 } }),
        ),
        (
            "the modelled minimum resolution with the minimum bitrate and framerate",
            json!({
                "video": {
                    "width": RESOLUTION_RANGE.0,
                    "height": RESOLUTION_RANGE.0,
                    "framerate": 1,
                    "bitrate": 1,
                },
            }),
        ),
        (
            // `Bitrate`'s modelled `@range` maximum (1..8500000).
            "the modelled maximum bitrate",
            json!({ "video": { "bitrate": 8_500_000 } }),
        ),
    ]
}

/// The `Video` a request's row must read back, member for member: what the request asked
/// for, with the documented default for everything it left out.
fn expected_video(request: &Value) -> (i64, i64, f64, i64) {
    let requested = request.get("video").cloned().unwrap_or(json!({}));
    (
        integer(&requested, "width").unwrap_or(DEFAULT_WIDTH),
        integer(&requested, "height").unwrap_or(DEFAULT_HEIGHT),
        requested
            .get("framerate")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_FRAMERATE),
        integer(&requested, "bitrate").unwrap_or(DEFAULT_BITRATE),
    )
}

/// Assert that `reported` is the video configuration `request` asks for.
fn assert_video(case: &str, request: &Value, reported: &Value) {
    let (width, height, framerate, bitrate) = expected_video(request);
    let video = reported["video"].clone();
    assert_eq!(
        integer(&video, "width"),
        Some(width),
        "case {case:?}: expected video.width {width}, got {video}"
    );
    assert_eq!(
        integer(&video, "height"),
        Some(height),
        "case {case:?}: expected video.height {height}, got {video}"
    );
    let actual_framerate = video.get("framerate").and_then(Value::as_f64);
    assert!(
        actual_framerate.is_some_and(|actual| (actual - framerate).abs() < 1e-6),
        "case {case:?}: expected video.framerate {framerate}, got {video}"
    );
    assert_eq!(
        integer(&video, "bitrate"),
        Some(bitrate),
        "case {case:?}: expected video.bitrate {bitrate}, got {video}"
    );
}

// ---------------------------------------------------------------------------
// CreateEncoderConfiguration, then GetEncoderConfiguration
// ---------------------------------------------------------------------------

/// What a create persists is what a read reports: for every row of the input class, the
/// configuration `GetEncoderConfiguration` answers is the one
/// `CreateEncoderConfiguration` reported, member for member, and every member is the one
/// the request asked for or the documented default for what it left out.
#[tokio::test]
async fn a_created_encoder_configuration_is_read_back_exactly() {
    let emulator = RealTime::new();

    for (case, request) in create_cases() {
        let created = emulator.create_in(case, "us-east-1", &request).await;
        let configuration_arn = member(&created, "arn");
        let read = emulator.get(case, &configuration_arn).await;

        assert_eq!(
            read, created,
            "case {case:?}: GetEncoderConfiguration must report the created configuration, \
             expected {created}, got {read}"
        );

        // The minted ARN: the modelled form, within the modelled length.
        assert!(
            matches_encoder_configuration_arn(&configuration_arn),
            "case {case:?}: expected an arn matching {ENCODER_CONFIGURATION_ARN_PATTERN}, \
             got {configuration_arn:?}"
        );
        assert!(
            configuration_arn.len() <= ENCODER_CONFIGURATION_ARN_MAX_LENGTH,
            "case {case:?}: expected an arn of at most {ENCODER_CONFIGURATION_ARN_MAX_LENGTH} \
             characters, got {} in {configuration_arn:?}",
            configuration_arn.len()
        );

        // What the request asked for, reported back.
        let expected_name = member(&request, "name");
        let actual_name = member(&read, "name");
        assert_eq!(
            actual_name, expected_name,
            "case {case:?}: expected name {expected_name:?}, got {actual_name:?}"
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        let actual_tags = read["tags"].clone();
        assert_eq!(
            actual_tags, expected_tags,
            "case {case:?}: expected tags {expected_tags}, got {actual_tags}"
        );
        assert_video(case, &request, &read);

        // A read changes nothing a later caller sees: the second read answers the first.
        let again = emulator.get(case, &configuration_arn).await;
        assert_eq!(
            again, read,
            "case {case:?}: a second GetEncoderConfiguration must answer the first, \
             expected {read}, got {again}"
        );
    }
}

/// The minted ARN names the region the creating request was signed for, and no two
/// configurations share an ARN.
#[tokio::test]
async fn each_minted_encoder_configuration_arn_is_distinct_and_names_its_region() {
    let emulator = RealTime::new();
    let mut seen: Vec<String> = Vec::new();

    for region in ["us-east-1", "eu-west-1", "ap-northeast-3"] {
        for (case, request) in create_cases() {
            let created = emulator.create_in(case, region, &request).await;
            let configuration_arn = member(&created, "arn");
            assert!(
                configuration_arn.starts_with(&format!("arn:aws:ivs:{region}:")),
                "case {case:?}: expected an arn naming the signed region {region}, \
                 got {configuration_arn:?}"
            );
            assert!(
                !seen.contains(&configuration_arn),
                "case {case:?}: expected a fresh arn, got the already-issued \
                 {configuration_arn:?}"
            );
            seen.push(configuration_arn);
        }
    }
}

// ---------------------------------------------------------------------------
// DeleteEncoderConfiguration
// ---------------------------------------------------------------------------

/// Deleting an encoder configuration removes exactly the one its ARN names: that ARN
/// then names nothing (a read, a second delete and the tagging surface all answer the
/// modelled 404), and every other configuration still reads back exactly as it did.
#[tokio::test]
async fn deleting_an_encoder_configuration_removes_only_the_one_it_names() {
    let emulator = RealTime::new();

    // One configuration per row of the input class, so the delete is exercised over the
    // whole class rather than on one shape of configuration.
    let cases = create_cases();
    let mut live: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in &cases {
        let created = emulator.create_in(case, "us-east-1", request).await;
        live.push((case, member(&created, "arn"), created));
    }

    while let Some((case, doomed, _)) = live.pop() {
        emulator.delete(case, &doomed).await;

        // Every later reference to the deleted configuration.
        for (operation, request) in [
            ("GetEncoderConfiguration", json!({ "arn": doomed })),
            ("DeleteEncoderConfiguration", json!({ "arn": doomed })),
        ] {
            let (status, response) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} after delete must answer HTTP 404, \
                 got {status} {response}"
            );
            let code = error_code(&response);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: {operation} after delete must answer \
                 ResourceNotFoundException, got {code:?} in {response}"
            );
        }
        let (status, response) = emulator.list_tags(&doomed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListTagsForResource after delete must answer HTTP 404, \
             got {status} {response}"
        );

        // Everything the delete did not name is untouched.
        for (surviving_case, surviving_arn, created) in &live {
            let read = emulator.get(surviving_case, surviving_arn).await;
            assert_eq!(
                &read, created,
                "case {case:?}: deleting {doomed} must leave {surviving_arn} as created, \
                 expected {created}, got {read}"
            );
        }
    }
}

/// A delete of an ARN this store holds no configuration for is the modelled 404, and an
/// `arn` outside the member's modelled constraints is the modelled 400 — a different
/// answer, so a client can tell a bad request from a missing resource.
#[tokio::test]
async fn deleting_an_encoder_configuration_the_store_does_not_hold_is_refused() {
    let emulator = RealTime::new();
    // One live configuration, so "not found" is not just "the store is empty", and one
    // deleted one, so a stale ARN is covered too.
    let kept = member(
        &emulator
            .create_in("a surviving configuration", "us-east-1", &json!({}))
            .await,
        "arn",
    );
    let deleted = member(
        &emulator
            .create_in("a deleted configuration", "us-east-1", &json!({}))
            .await,
        "arn",
    );
    emulator.delete("a deleted configuration", &deleted).await;

    let not_found = [
        (
            "an id this store never minted",
            arn("us-east-1", "encoder-configuration", "abcdef123456"),
        ),
        ("an already-deleted configuration", deleted.clone()),
        (
            "the id of a live configuration in another region",
            arn(
                "eu-west-1",
                "encoder-configuration",
                kept.rsplit('/').next().unwrap_or("x"),
            ),
        ),
    ];
    for (case, bad_arn) in not_found {
        let (status, response) = emulator
            .call("DeleteEncoderConfiguration", &json!({ "arn": bad_arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: DeleteEncoderConfiguration on {bad_arn} must answer HTTP 404, \
             got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?} in {response}"
        );
    }

    let refused = [
        // Each row is outside the member's modelled `@pattern` or `@length`.
        ("the empty string", String::new()),
        ("not an arn at all", "encoder-configuration/abc".to_string()),
        (
            "an arn of another resource type",
            arn("us-east-1", "stage", "abcdef123456"),
        ),
        (
            "an arn of another service",
            format!("arn:aws:ivschat:us-east-1:{ACCOUNT}:encoder-configuration/abcdef123456"),
        ),
        (
            "an id longer than the modelled arn length",
            arn("us-east-1", "encoder-configuration", &"a".repeat(128)),
        ),
    ];
    for (case, bad_arn) in refused {
        let (status, response) = emulator
            .call("DeleteEncoderConfiguration", &json!({ "arn": bad_arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: DeleteEncoderConfiguration on {bad_arn:?} must answer HTTP 400, \
             got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {response}"
        );
    }

    // The configuration the refused calls named nothing of is still there.
    let read = emulator.get("a surviving configuration", &kept).await;
    assert_eq!(
        member(&read, "arn"),
        kept,
        "the surviving configuration must still read back, got {read}"
    );
}

/// A delete removes the configuration from the state the WHOLE service reads, so the
/// tagging surface stops finding it too — and until it is deleted, the tags the create
/// attached are what that surface reports.
#[tokio::test]
async fn a_deleted_encoder_configuration_leaves_the_tagging_surface_nothing() {
    let emulator = RealTime::new();
    let cases = [
        ("no tags", json!({})),
        ("one tag", json!({ "tags": { "env": "test" } })),
        (
            "several tags",
            json!({ "tags": { "env": "test", "team": "video", "tier": "gold" } }),
        ),
    ];

    for (case, request) in cases {
        let created = emulator.create_in(case, "us-east-1", &request).await;
        let configuration_arn = member(&created, "arn");

        let (status, listed) = emulator.list_tags(&configuration_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource on {configuration_arn} must answer HTTP 200, \
             got {status} {listed}"
        );
        let expected: HashMap<String, String> =
            serde_json::from_value(request.get("tags").cloned().unwrap_or(json!({})))
                .unwrap_or_default();
        let actual: HashMap<String, String> =
            serde_json::from_value(listed["tags"].clone()).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected tags {expected:?}, got {actual:?}"
        );

        emulator.delete(case, &configuration_arn).await;
        let (status, response) = emulator.list_tags(&configuration_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListTagsForResource after delete must answer HTTP 404, \
             got {status} {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// The documented `Video` rules
// ---------------------------------------------------------------------------

/// The two rules the model documents on `Video$width`/`$height` and carries no trait
/// for: a resolution member is an even number, and `width` times `height` stays within
/// the documented maximum total pixels. Each is the modelled `ValidationException`
/// (HTTP 400), and nothing is persisted for a refused request.
#[tokio::test]
async fn the_documented_video_rules_are_enforced() {
    let emulator = RealTime::new();

    let refused = [
        ("an odd width", json!({ "video": { "width": 1919 } })),
        ("an odd height", json!({ "video": { "height": 719 } })),
        (
            "an odd width and an odd height",
            json!({ "video": { "width": 3, "height": 5 } }),
        ),
        (
            "one pixel past the documented maximum, by width",
            // 1922 is past the modelled @range, so the pair must stay inside it: the
            // smallest even pair over MAX_VIDEO_PIXELS that both bounds admit.
            json!({ "video": { "width": 1920, "height": 1082 } }),
        ),
        (
            "the largest resolution each member's own range admits",
            json!({ "video": { "width": 1920, "height": 1920 } }),
        ),
    ];
    for (case, request) in refused {
        let (status, response) = emulator.call("CreateEncoderConfiguration", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateEncoderConfiguration {request} must answer HTTP 400, \
             got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {response}"
        );
    }

    // Each resolution member's own modelled `@range`, one step past each bound.
    let out_of_range = [
        (
            "a width below the modelled minimum",
            json!({ "video": { "width": RESOLUTION_RANGE.0 - 2 } }),
        ),
        (
            "a width above the modelled maximum",
            json!({ "video": { "width": RESOLUTION_RANGE.1 + 2 } }),
        ),
        (
            "a height below the modelled minimum",
            json!({ "video": { "height": RESOLUTION_RANGE.0 - 2 } }),
        ),
        (
            "a height above the modelled maximum",
            json!({ "video": { "height": RESOLUTION_RANGE.1 + 2 } }),
        ),
    ];
    for (case, request) in out_of_range {
        let (status, response) = emulator.call("CreateEncoderConfiguration", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateEncoderConfiguration {request} must answer HTTP 400, \
             got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {response}"
        );
    }

    // Both published forms of the documented maximum are ACCEPTED, and persist the
    // resolution they asked for.
    for (case, width, height) in [
        ("the landscape maximum", 1920, 1080),
        ("the portrait maximum", 1080, 1920),
    ] {
        assert_eq!(
            i64::from(width) * i64::from(height),
            MAX_VIDEO_PIXELS,
            "case {case:?}: this row must be exactly the documented maximum total pixels"
        );
        let request = json!({ "video": { "width": width, "height": height } });
        let created = emulator.create_in(case, "us-east-1", &request).await;
        assert_video(case, &request, &created);
    }
}
