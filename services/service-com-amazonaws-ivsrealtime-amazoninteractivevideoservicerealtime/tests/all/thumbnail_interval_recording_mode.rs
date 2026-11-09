// SPDX-License-Identifier: CC0-1.0

//! The thumbnail-generation interval of a stage's individual-participant recording, and
//! the thumbnail recording mode it may be configured in, over the wire.
//!
//! One rule, exercised on both operations that write a stage's recording configuration
//! (`CreateStage`, `UpdateStage`) and over a SET of inputs rather than one request: a
//! member the model documents as configurable only while a sibling member carries a
//! particular value is refused with the operation's modelled `ValidationException`
//! (HTTP 400) when the request configures it while that sibling's EFFECTIVE value is
//! anything else, and nothing is created or changed.
//!
//! The member this applies to here is
//! `autoParticipantRecordingConfiguration.thumbnailConfiguration.targetIntervalSeconds`
//! (`smithy_query(query="shape",shape="ParticipantThumbnailConfiguration")`: "The targeted
//! thumbnail-generation interval in seconds. This is configurable only if `recordingMode`
//! is `INTERVAL`. Default: 60."), whose sibling `recordingMode` is documented "Thumbnail
//! recording mode. Default: `DISABLED`." — so a request that configures an interval while
//! `recordingMode` is `DISABLED`, or leaves `recordingMode` out and so takes that
//! documented default, is refused, while one that configures it in `INTERVAL` is accepted
//! and reported back.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code, the message and the JSON
//! body — never an internal call.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request of this suite is signed for.
const REGION: &str = "us-east-1";

/// The two values the modelled `ThumbnailRecordingMode` enum admits, from
/// `smithy_query(query="shape",shape="ThumbnailRecordingMode")`: `["INTERVAL",
/// "DISABLED"]`. The first is the one the interval is documented as configurable in; every
/// other value of the enum is a mode it is not.
const THUMBNAIL_RECORDING_MODES: [&str; 2] = ["INTERVAL", "DISABLED"];

/// The mode `targetIntervalSeconds` is documented as configurable in.
const CONFIGURABLE_MODE: &str = THUMBNAIL_RECORDING_MODES[0];

/// The published bounds of `ThumbnailIntervalSeconds`, from
/// `smithy_query(query="shape",shape="ParticipantThumbnailConfiguration")` — member
/// `targetIntervalSeconds`, `@range 1..86400`.
const MIN_THUMBNAIL_INTERVAL_SECONDS: i64 = 1;
const MAX_THUMBNAIL_INTERVAL_SECONDS: i64 = 86_400;

/// The interval a stage reports when its thumbnail configuration sets none, from the same
/// member's doc: "Default: 60."
const DEFAULT_THUMBNAIL_INTERVAL_SECONDS: i64 = 60;

/// One emulator holding only this service, driven through the router that serves
/// production traffic.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a storage configuration and return the ARN the service minted for it: every
    /// row that configures thumbnails records into one.
    async fn create_storage_configuration(&self, bucket: &str) -> String {
        let request = json!({ "s3": { "bucketName": bucket }, "name": bucket });
        let (status, response) = self.call("CreateStorageConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "CreateStorageConfiguration {request}: expected 200, got {status} {response}"
        );
        response["storageConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, body: &Value) -> String {
        let (status, response) = self.call("CreateStage", body).await;
        assert_eq!(
            status, 200,
            "CreateStage {body}: expected 200, got {status} {response}"
        );
        response["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The stage `arn`, as `GetStage` reports it.
    async fn get_stage(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {arn} expected 200, got {status} {response}"
        );
        response["stage"].clone()
    }

    /// How many stages this account holds, as `ListStages` reports it.
    async fn stage_count(&self, case: &str) -> usize {
        let (status, response) = self.call("ListStages", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListStages expected 200, got {status} {response}"
        );
        response["stages"].as_array().map_or(0, Vec::len)
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The failure's text, under the `message` key an SDK error parser reads.
fn error_message(response: &Value) -> &str {
    response["message"].as_str().unwrap_or("<no message>")
}

/// Thumbnail configurations that CONFIGURE the interval in a mode it is not configurable
/// in, each row differing from the others along what the rule generalises over: the
/// interval's value (each published bound, one in between), how the mode arrives
/// (explicitly, or left out so the documented `DISABLED` default decides), and what the
/// configuration carries beside the two members.
fn interval_in_a_mode_it_is_not_configurable_in() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = Vec::new();

    for interval in [
        MIN_THUMBNAIL_INTERVAL_SECONDS,
        DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
        MAX_THUMBNAIL_INTERVAL_SECONDS,
    ] {
        // Every mode of the enum that is not the one the member is configurable in.
        for mode in THUMBNAIL_RECORDING_MODES
            .iter()
            .filter(|mode| **mode != CONFIGURABLE_MODE)
        {
            rows.push((
                format!("an interval of {interval} with recordingMode {mode}"),
                json!({ "targetIntervalSeconds": interval, "recordingMode": mode }),
            ));
        }
        rows.push((
            format!("an interval of {interval} with no recordingMode at all"),
            json!({ "targetIntervalSeconds": interval }),
        ));
    }

    // The same pairing with the configuration's other member supplied beside it, so the
    // refusal is about the pairing and not about a sparse object.
    rows.push((
        "an interval in a mode it is not configurable in, in both storage formats".to_string(),
        json!({
            "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
            "storage": ["SEQUENTIAL", "LATEST"],
            "recordingMode": "DISABLED",
        }),
    ));

    rows
}

/// Thumbnail configurations that are accepted, each paired with the thumbnail
/// configuration the stage must then report: either the interval is configured in the one
/// mode it is configurable in, or it is not configured at all — and a member left out is
/// reported at the default the model documents for it.
fn accepted_thumbnail_configurations() -> Vec<(String, Value, Value)> {
    let mut rows: Vec<(String, Value, Value)> = Vec::new();

    // Configured in the mode it is configurable in: each published bound, and one in
    // between.
    for interval in [
        MIN_THUMBNAIL_INTERVAL_SECONDS,
        DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
        MAX_THUMBNAIL_INTERVAL_SECONDS,
    ] {
        rows.push((
            format!("an interval of {interval} with recordingMode {CONFIGURABLE_MODE}"),
            json!({
                "targetIntervalSeconds": interval,
                "recordingMode": CONFIGURABLE_MODE,
            }),
            json!({
                "targetIntervalSeconds": interval,
                "storage": ["SEQUENTIAL"],
                "recordingMode": CONFIGURABLE_MODE,
            }),
        ));
    }

    // Not configured at all: every mode of the enum admits that, and the stage reports the
    // documented default interval.
    for mode in THUMBNAIL_RECORDING_MODES {
        rows.push((
            format!("no interval configured, recordingMode {mode}"),
            json!({ "recordingMode": mode }),
            json!({
                "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
                "storage": ["SEQUENTIAL"],
                "recordingMode": mode,
            }),
        ));
    }

    // Neither member configured, and a sibling member configured without either: the
    // documented defaults for both.
    rows.push((
        "an empty thumbnail configuration".to_string(),
        json!({}),
        json!({
            "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
            "storage": ["SEQUENTIAL"],
            "recordingMode": "DISABLED",
        }),
    ));
    rows.push((
        "a storage format only".to_string(),
        json!({ "storage": ["LATEST"] }),
        json!({
            "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
            "storage": ["LATEST"],
            "recordingMode": "DISABLED",
        }),
    ));

    rows
}

/// A refusal of the pairing states the member that carried the value and the mode the
/// configuration is in, so a client can tell it from a refusal of some other member.
fn assert_names_the_member_and_the_mode(
    case: &str,
    operation: &str,
    mode: &str,
    response: &Value,
) {
    let message = error_message(response);
    assert!(
        message.contains(
            "autoParticipantRecordingConfiguration.thumbnailConfiguration.\
             targetIntervalSeconds"
        ),
        "case {case:?}: expected the {operation} refusal to name the member that carried \
         the interval, got {message:?}"
    );
    assert!(
        message.contains(CONFIGURABLE_MODE),
        "case {case:?}: expected the {operation} refusal to name the mode the interval is \
         configurable in, got {message:?}"
    );
    assert!(
        message.contains(mode),
        "case {case:?}: expected the {operation} refusal to name the mode {mode:?} the \
         configuration is in, got {message:?}"
    );
}

/// The mode a thumbnail configuration is EFFECTIVELY in: the one it names, else the
/// documented `DISABLED` default.
fn effective_mode(configuration: &Value) -> &str {
    configuration["recordingMode"].as_str().unwrap_or("DISABLED")
}

/// `CreateStage` refuses every configuration that sets the interval in a mode it is not
/// configurable in, and creates no stage for it.
#[tokio::test]
async fn create_stage_refuses_a_thumbnail_interval_outside_the_mode_it_is_configurable_in() {
    let emulator = Emulator::new();
    let storage = emulator.create_storage_configuration("thumbs").await;

    for (case, thumbnails) in interval_in_a_mode_it_is_not_configurable_in() {
        let before = emulator.stage_count(&case).await;
        let request = json!({
            "name": "thumbs",
            "autoParticipantRecordingConfiguration": {
                "storageConfigurationArn": storage.as_str(),
                "thumbnailConfiguration": thumbnails.clone(),
            },
        });
        let (status, response) = emulator.call("CreateStage", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {request}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert_names_the_member_and_the_mode(
            &case,
            "CreateStage",
            effective_mode(&thumbnails),
            &response,
        );

        let after = emulator.stage_count(&case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused create to leave {before} stages, got {after}"
        );
    }
}

/// `UpdateStage` applies the same rule, and a refused update leaves the stage exactly as
/// it was — including a stage whose thumbnails are already configured at an interval.
#[tokio::test]
async fn update_stage_refuses_a_thumbnail_interval_outside_the_mode_it_is_configurable_in() {
    let emulator = Emulator::new();
    let storage = emulator.create_storage_configuration("thumbs").await;

    // Rows differ in what the stage being updated already carries: no recording
    // configuration at all, and one whose thumbnails are generated at an interval.
    let stages: Vec<(&str, Value)> = vec![
        ("a stage with no recording configured", json!({ "name": "plain" })),
        (
            "a stage already generating thumbnails at an interval",
            json!({
                "name": "recorded",
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": storage.as_str(),
                    "thumbnailConfiguration": {
                        "targetIntervalSeconds": 120,
                        "recordingMode": CONFIGURABLE_MODE,
                    },
                },
            }),
        ),
    ];

    for (stage_case, request) in stages {
        let stage_arn = emulator.create_stage(&request).await;
        let before = emulator.get_stage(stage_case, &stage_arn).await;

        for (row_case, thumbnails) in interval_in_a_mode_it_is_not_configurable_in() {
            let case = format!("{stage_case}, {row_case}");
            let request = json!({
                "arn": stage_arn.as_str(),
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": storage.as_str(),
                    "thumbnailConfiguration": thumbnails.clone(),
                },
            });
            let (status, response) = emulator.call("UpdateStage", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for {request}, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response}"
            );
            assert_names_the_member_and_the_mode(
                &case,
                "UpdateStage",
                effective_mode(&thumbnails),
                &response,
            );

            let after = emulator.get_stage(&case, &stage_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused update to leave the stage as it was \
                 ({before}), got {after}"
            );
        }
    }
}

/// The pairing is read off the request's OWN members, before the reset the model documents
/// for an empty `storageConfigurationArn` ("other fields in this object will get reset to
/// their defaults when sending `\"\"`"): a request that configures the interval in a mode
/// it is not configurable in is refused whatever the configuration's storage member
/// carries, while the same request without the interval keeps the documented reset.
#[tokio::test]
async fn the_pairing_is_decided_by_the_members_the_request_sent() {
    let emulator = Emulator::new();

    for (case, thumbnails) in interval_in_a_mode_it_is_not_configurable_in() {
        let request = json!({
            "autoParticipantRecordingConfiguration": {
                "storageConfigurationArn": "",
                "thumbnailConfiguration": thumbnails.clone(),
            },
        });
        let (status, response) = emulator.call("CreateStage", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {request}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }

    // No interval configured: the documented reset applies and every member of the
    // configuration is reported at its default.
    let request = json!({
        "autoParticipantRecordingConfiguration": {
            "storageConfigurationArn": "",
            "thumbnailConfiguration": { "recordingMode": CONFIGURABLE_MODE },
        },
    });
    let (status, created) = emulator.call("CreateStage", &request).await;
    assert_eq!(
        status, 200,
        "the reset case: expected 200 for {request}, got {status} {created}"
    );
    let actual = &created["stage"]["autoParticipantRecordingConfiguration"]
        ["thumbnailConfiguration"];
    assert_eq!(
        actual,
        &json!({
            "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
            "storage": ["SEQUENTIAL"],
            "recordingMode": "DISABLED",
        }),
        "the reset case: expected the documented defaults, got {created}"
    );
}

/// A configuration that configures the interval in the one mode it is configurable in —
/// and one that does not configure it at all — is accepted by both writers and reported
/// back by a read, so the rule refuses only what the model says cannot be configured.
#[tokio::test]
async fn a_thumbnail_interval_in_the_mode_it_is_configurable_in_is_accepted_and_read_back() {
    let emulator = Emulator::new();
    let storage = emulator.create_storage_configuration("thumbs").await;

    for (case, thumbnails, expected) in accepted_thumbnail_configurations() {
        let configuration = json!({
            "storageConfigurationArn": storage.as_str(),
            "thumbnailConfiguration": thumbnails,
        });
        let (status, created) = emulator
            .call(
                "CreateStage",
                &json!({ "autoParticipantRecordingConfiguration": configuration.clone() }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage expected 200, got {status} {created}"
        );
        let actual = &created["stage"]["autoParticipantRecordingConfiguration"]
            ["thumbnailConfiguration"];
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected CreateStage to report {expected}, got {created}"
        );

        // The create response is what a client caches, so a read must report exactly it.
        let stage_arn = created["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = emulator.get_stage(&case, &stage_arn).await;
        assert_eq!(
            read, created["stage"],
            "case {case:?}: expected GetStage to report the created stage, got {read}"
        );

        // And the same configuration attached by an update reports the same members.
        let other = emulator.create_stage(&json!({ "name": "updated" })).await;
        let (status, updated) = emulator
            .call(
                "UpdateStage",
                &json!({
                    "arn": other.as_str(),
                    "autoParticipantRecordingConfiguration": configuration,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateStage expected 200, got {status} {updated}"
        );
        let actual = &updated["stage"]["autoParticipantRecordingConfiguration"]
            ["thumbnailConfiguration"];
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected UpdateStage to report {expected}, got {updated}"
        );
        let read = emulator.get_stage(&case, &other).await;
        assert_eq!(
            read, updated["stage"],
            "case {case:?}: expected GetStage to report the update, got {read}"
        );
    }
}
