// SPDX-License-Identifier: CC0-1.0

//! The default a response reports for a member the model declares a Smithy `@default`
//! for, over the wire.
//!
//! THE RULE: a member has ONE default — the one its own documentation states for a
//! request that leaves it out — and a response reports that default whatever the
//! member's siblings carry, while a value the request DID carry is reported as it
//! arrived. Where the model also declares a Smithy `@default` for such a member, its
//! shape keeps the member's presence, so "left out" and "sent at the declared default"
//! stay distinguishable and both halves of the rule hold.
//!
//! The two members of this service whose documented default differs from the `@default`
//! the model declares for them, each from `smithy_query(query="shape",
//! shape="AutoParticipantRecordingConfiguration,GridConfiguration,PipConfiguration")`:
//!
//! * `AutoParticipantRecordingConfiguration$recordParticipantReplicas` — `@default
//!   false`, documented "Optional field to disable replica participant recording. … 
//!   Default: `true`." Attached to a stage by `CreateStage` and `UpdateStage`, reported
//!   by both and by `GetStage`.
//! * `GridConfiguration$gridGap` — `@default 0`, documented "Specifies the spacing
//!   between participant tiles in pixels. Default: `2`." Attached to a composition by
//!   `StartComposition` and reported by it and by `GetComposition`.
//!
//! The members whose documented default IS the `@default` the model declares are here
//! too, as the rows that must NOT move: `recordingReconnectWindowSeconds` ("The default
//! value is 0"), `GridConfiguration$omitStoppedVideo` ("Default: `false`"),
//! `PipConfiguration$gridGap` ("Default: `0`") and `PipConfiguration$pipOffset`
//! ("Default: `0`") — the same `GridGap` shape, with its own default per member.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every request of this suite is signed for.
const REGION: &str = "us-east-1";

/// `AutoParticipantRecordingConfiguration$recordParticipantReplicas`: "Default: `true`".
const DEFAULT_RECORD_PARTICIPANT_REPLICAS: bool = true;

/// `AutoParticipantRecordingConfiguration$recordingReconnectWindowSeconds`: "The default
/// value is 0, which disables merging" — the same value the model declares as its
/// `@default`, so it is the row that must not move.
const DEFAULT_RECONNECT_WINDOW_SECONDS: i64 = 0;

/// `GridConfiguration$gridGap`: "Default: `2`".
const DEFAULT_GRID_GAP: i64 = 2;

/// `PipConfiguration$gridGap`: "Default: `0`" — the same member name on the sibling
/// layout shape, with its own documented default.
const DEFAULT_PIP_GRID_GAP: i64 = 0;

/// `PipConfiguration$pipOffset`: "Default: `0`".
const DEFAULT_PIP_OFFSET: i64 = 0;

/// One IVS RealTime emulator over one shared engine, driven through the router that
/// serves production traffic.
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
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
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

    /// `operation`, asserting the modelled success and answering with the member of the
    /// response body `key` names.
    async fn succeed(&self, case: &str, operation: &str, body: &Value, key: &str) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must answer HTTP 200, got {status} {response}"
        );
        response[key].clone()
    }

    /// Create a storage configuration and answer with the ARN the service minted — the
    /// only value `storageConfigurationArn` can name, since it is a reference to a
    /// `StorageConfiguration` of this service.
    async fn storage_configuration(&self, case: &str, bucket: &str) -> String {
        let request = json!({ "name": bucket, "s3": { "bucketName": bucket } });
        let created = self
            .succeed(case, "CreateStorageConfiguration", &request, "storageConfiguration")
            .await;
        created["arn"].as_str().unwrap_or_default().to_string()
    }

    /// Create a stage and answer with the stage the service reported.
    async fn create_stage(&self, case: &str, body: &Value) -> Value {
        self.succeed(case, "CreateStage", body, "stage").await
    }

    /// The stage `arn`, as `GetStage` reports it.
    async fn get_stage(&self, case: &str, arn: &str) -> Value {
        self.succeed(case, "GetStage", &json!({ "arn": arn }), "stage")
            .await
    }

    /// The composition `arn`, as `GetComposition` reports it.
    async fn get_composition(&self, case: &str, arn: &str) -> Value {
        self.succeed(case, "GetComposition", &json!({ "arn": arn }), "composition")
            .await
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The ARN member of a resource the service just reported.
fn arn_of(case: &str, resource: &Value) -> String {
    resource["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted arn, got {resource}"))
        .to_string()
}

/// One row of the recording-configuration class: the `autoParticipantRecordingConfiguration`
/// a request carries (`Value::Null` for a request that carries none), and what
/// `recordParticipantReplicas` and `recordingReconnectWindowSeconds` must then report.
struct RecordingRow {
    case: &'static str,
    configuration: Value,
    replicas: bool,
    reconnect_window: i64,
}

/// The rows the recording class is spanned with. They differ along what the rule
/// generalises over: whether the request carries the configuration at all, whether it
/// configures storage (the sibling the two defaults used to disagree over), how many
/// other members it carries, and whether it names `recordParticipantReplicas` itself —
/// including at the `false` the model declares as the member's `@default`, which is the
/// row that separates "the documented default for an absent member" from "a constant".
///
/// `storageConfigurationArn` is `@required` on the shape, so every row that carries a
/// configuration names it — with the documented "no storage configuration is specified"
/// empty string, or with one of the two configurations this account holds.
fn recording_rows(first_storage: &str, second_storage: &str) -> Vec<RecordingRow> {
    vec![
        RecordingRow {
            case: "no recording configuration at all",
            configuration: Value::Null,
            replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
            reconnect_window: DEFAULT_RECONNECT_WINDOW_SECONDS,
        },
        RecordingRow {
            case: "the documented empty string, which configures no storage",
            configuration: json!({ "storageConfigurationArn": "" }),
            replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
            reconnect_window: DEFAULT_RECONNECT_WINDOW_SECONDS,
        },
        RecordingRow {
            case: "a storage configuration this account holds, and nothing else",
            configuration: json!({ "storageConfigurationArn": first_storage }),
            replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
            reconnect_window: DEFAULT_RECONNECT_WINDOW_SECONDS,
        },
        RecordingRow {
            case: "another storage configuration, with the media types named",
            configuration: json!({
                "storageConfigurationArn": second_storage,
                "mediaTypes": ["AUDIO_ONLY"],
            }),
            replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
            reconnect_window: DEFAULT_RECONNECT_WINDOW_SECONDS,
        },
        RecordingRow {
            case: "storage configured and the reconnect window at its maximum",
            configuration: json!({
                "storageConfigurationArn": first_storage,
                "recordingReconnectWindowSeconds": 300,
            }),
            replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
            reconnect_window: 300,
        },
        RecordingRow {
            case: "every sibling member supplied, the member itself left out",
            configuration: json!({
                "storageConfigurationArn": second_storage,
                "mediaTypes": ["NONE"],
                "recordingReconnectWindowSeconds": 1,
                "thumbnailConfiguration": {
                    "targetIntervalSeconds": 120,
                    "storage": ["LATEST"],
                    "recordingMode": "INTERVAL",
                },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 10 },
            }),
            replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
            reconnect_window: 1,
        },
        RecordingRow {
            case: "the member named explicitly, beside storage",
            configuration: json!({
                "storageConfigurationArn": first_storage,
                "recordParticipantReplicas": true,
            }),
            replicas: true,
            reconnect_window: DEFAULT_RECONNECT_WINDOW_SECONDS,
        },
        RecordingRow {
            // The member's documented purpose: "Optional field to disable replica
            // participant recording. If this is set to `false` … replica participants
            // are not recorded." `false` is also the `@default` the model declares, so
            // this row is what proves an explicitly sent value is not read as absence.
            case: "the member disabled explicitly, beside storage",
            configuration: json!({
                "storageConfigurationArn": first_storage,
                "recordParticipantReplicas": false,
            }),
            replicas: false,
            reconnect_window: DEFAULT_RECONNECT_WINDOW_SECONDS,
        },
        RecordingRow {
            case: "the member disabled explicitly beside every other member",
            configuration: json!({
                "storageConfigurationArn": second_storage,
                "mediaTypes": ["AUDIO_VIDEO"],
                "recordingReconnectWindowSeconds": 300,
                "recordParticipantReplicas": false,
                "thumbnailConfiguration": { "recordingMode": "INTERVAL" },
                "hlsConfiguration": { "targetSegmentDurationSeconds": 2 },
            }),
            replicas: false,
            reconnect_window: 300,
        },
        RecordingRow {
            // Documented on `storageConfigurationArn`: "other fields in this object will
            // get reset to their defaults when sending `\"\"`" — so the reset wins over
            // the value sent beside it, on this member as on every other.
            case: "the member disabled explicitly, with no storage configured",
            configuration: json!({
                "storageConfigurationArn": "",
                "recordParticipantReplicas": false,
            }),
            replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
            reconnect_window: DEFAULT_RECONNECT_WINDOW_SECONDS,
        },
    ]
}

/// What a stage reports for the two members under test.
fn reported(stage: &Value) -> (Value, Value) {
    let configuration = &stage["autoParticipantRecordingConfiguration"];
    (
        configuration["recordParticipantReplicas"].clone(),
        configuration["recordingReconnectWindowSeconds"].clone(),
    )
}

/// `recordParticipantReplicas` reports its ONE documented default for every request that
/// leaves it out — whatever the siblings in the same object carry, and whether the stage
/// was created with the configuration or updated into it — while
/// `recordingReconnectWindowSeconds`, whose documented default IS its `@default`, reports
/// exactly what the request sent.
#[tokio::test]
async fn record_participant_replicas_reports_its_documented_default_whatever_the_siblings_carry() {
    let service = RealTime::new();
    let first = service.storage_configuration("storage", "replicas-first").await;
    let second = service
        .storage_configuration("storage", "replicas-second")
        .await;

    for row in recording_rows(&first, &second) {
        let case = row.case;
        let expected = (json!(row.replicas), json!(row.reconnect_window));

        // CreateStage attaches the configuration.
        let mut request = json!({ "name": "created" });
        if !row.configuration.is_null() {
            request["autoParticipantRecordingConfiguration"] = row.configuration.clone();
        }
        let created = service.create_stage(case, &request).await;
        assert_eq!(
            reported(&created),
            expected,
            "case {case:?}: expected CreateStage to report ({}, {}), got {created}",
            expected.0,
            expected.1
        );

        // ROUND TRIP: the create response is what a client caches, so a read reports it.
        let created_arn = arn_of(case, &created);
        let read = service.get_stage(case, &created_arn).await;
        assert_eq!(
            read, created,
            "case {case:?}: expected GetStage to report the created stage {created}, got {read}"
        );

        // UpdateStage attaches the same configuration to a stage created without one, so
        // an updated stage reports the same defaults a created one does.
        if row.configuration.is_null() {
            continue;
        }
        let other = service.create_stage(case, &json!({ "name": "updated" })).await;
        let other_arn = arn_of(case, &other);
        let updated = service
            .succeed(
                case,
                "UpdateStage",
                &json!({
                    "arn": other_arn.as_str(),
                    "autoParticipantRecordingConfiguration": row.configuration.clone(),
                }),
                "stage",
            )
            .await;
        assert_eq!(
            reported(&updated),
            expected,
            "case {case:?}: expected UpdateStage to report ({}, {}), got {updated}",
            expected.0,
            expected.1
        );
        let read = service.get_stage(case, &other_arn).await;
        assert_eq!(
            read, updated,
            "case {case:?}: expected GetStage to report the updated stage {updated}, got {read}"
        );
    }
}

/// A started composition's `layout`, as the wire reports it, for a request carrying
/// `layout`.
async fn started_layout(service: &RealTime, case: &str, stage_arn: &str, layout: &Value) -> Value {
    // One channel destination is the smallest start request the model allows; the channel
    // lives in another service, so any well-formed channel ARN serves.
    let mut request = json!({
        "stageArn": stage_arn,
        "destinations": [{ "channel": {
            "channelArn": format!("arn:aws:ivs:{REGION}:000000000000:channel/abcdABCD1234"),
        }}],
    });
    if !layout.is_null() {
        request["layout"] = layout.clone();
    }
    let started = service
        .succeed(case, "StartComposition", &request, "composition")
        .await;

    // ROUND TRIP: the layout was PERSISTED with its defaults, not recomputed on report.
    let arn = arn_of(case, &started);
    let read = service.get_composition(case, &arn).await;
    assert_eq!(
        read["layout"], started["layout"],
        "case {case:?}: expected GetComposition to report the started layout {}, got {}",
        started["layout"], read["layout"]
    );
    started["layout"].clone()
}

/// `GridConfiguration$gridGap` reports its documented default for every request that does
/// not size it, and the sibling `PipConfiguration` members whose documented default is
/// the `@default` the model declares report what the request sent.
#[tokio::test]
async fn grid_gap_reports_its_documented_default_and_the_pip_layout_keeps_its_own() {
    let service = RealTime::new();
    let stage = service.create_stage("layout", &json!({ "name": "composited" })).await;
    let stage_arn = arn_of("layout", &stage);

    // Rows differ along what the rule generalises over: whether the request carries a
    // layout at all, which of the two layouts it configures, whether it sizes the gap,
    // and the value it sizes it with. `GridGap` is `@range(min 0)` with no maximum
    // (`smithy_query(query="shape",shape="GridConfiguration")`), so the published bound
    // and a value far above it both appear; one past the bound is refused below.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no layout at all: the documented grid fallback",
            Value::Null,
            json!(DEFAULT_GRID_GAP),
        ),
        ("an empty grid", json!({ "grid": {} }), json!(DEFAULT_GRID_GAP)),
        (
            "a grid sizing another member, not the gap",
            json!({ "grid": { "videoAspectRatio": "PORTRAIT" } }),
            json!(DEFAULT_GRID_GAP),
        ),
        (
            "a grid sizing the gap at the published minimum",
            json!({ "grid": { "gridGap": 0 } }),
            json!(0),
        ),
        (
            "a grid sizing the gap at one pixel",
            json!({ "grid": { "gridGap": 1 } }),
            json!(1),
        ),
        (
            "a grid sizing the gap far above the published minimum",
            json!({ "grid": { "gridGap": 4_096 } }),
            json!(4_096),
        ),
    ];

    for (case, layout, expected) in cases {
        let reported = started_layout(&service, case, &stage_arn, &layout).await;
        assert_eq!(
            reported["grid"]["gridGap"], expected,
            "case {case:?}: expected gridGap {expected}, got {reported}"
        );
        // The grid member whose documented default IS the model's `@default` does not
        // move with it.
        assert_eq!(
            reported["grid"]["omitStoppedVideo"],
            json!(false),
            "case {case:?}: expected omitStoppedVideo false, got {reported}"
        );
    }

    // The same member name on the picture-in-picture layout documents 0, and so does
    // `pipOffset`: a request that sizes neither reports both as the model declares them.
    let pip_cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "an empty picture-in-picture layout",
            json!({ "pip": {} }),
            json!(DEFAULT_PIP_GRID_GAP),
            json!(DEFAULT_PIP_OFFSET),
        ),
        (
            "a picture-in-picture layout sizing the gap and the offset",
            json!({ "pip": { "gridGap": 7, "pipOffset": 9 } }),
            json!(7),
            json!(9),
        ),
    ];
    for (case, layout, expected_gap, expected_offset) in pip_cases {
        let reported = started_layout(&service, case, &stage_arn, &layout).await;
        assert_eq!(
            reported["pip"]["gridGap"], expected_gap,
            "case {case:?}: expected the pip gridGap {expected_gap}, got {reported}"
        );
        assert_eq!(
            reported["pip"]["pipOffset"], expected_offset,
            "case {case:?}: expected pipOffset {expected_offset}, got {reported}"
        );
    }
}

/// A gap one past the published bound is still refused: the documented default applies to
/// a member the request left out, never to a value the member's `@range` excludes.
#[tokio::test]
async fn a_gap_below_the_published_bound_is_refused() {
    let service = RealTime::new();
    let stage = service.create_stage("refusal", &json!({ "name": "composited" })).await;
    let stage_arn = arn_of("refusal", &stage);

    // Rows: the same out-of-range value on each layout that carries the member.
    for (case, layout) in [
        ("a grid gap one below the minimum", json!({ "grid": { "gridGap": -1 } })),
        (
            "a picture-in-picture gap one below the minimum",
            json!({ "pip": { "gridGap": -1 } }),
        ),
    ] {
        let (status, response) = service
            .call(
                "StartComposition",
                &json!({
                    "stageArn": stage_arn.as_str(),
                    "destinations": [{ "channel": {
                        "channelArn":
                            format!("arn:aws:ivs:{REGION}:000000000000:channel/abcdABCD1234"),
                    }}],
                    "layout": layout,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}

/// Keeping a member's presence costs the shape none of the bounds the model declares for
/// its OTHER members: every one of them is still enforced at the dispatch boundary, so a
/// request outside a bound is the modelled `ValidationException` (HTTP 400) rather than a
/// stage or a composition.
///
/// Rows span the bounds the two shapes carry — the `@required` member, both `@length`
/// bounds, the `@pattern`, the list `@length`, and each closed value set — from
/// `smithy_query(query="shape",
/// shape="AutoParticipantRecordingConfiguration,GridConfiguration,
/// AutoParticipantRecordingStorageConfigurationArn,ParticipantRecordingMediaType,
/// VideoAspectRatio,VideoFillMode,AttributeKey")`.
#[tokio::test]
async fn the_bounds_the_model_declares_are_still_refused() {
    let service = RealTime::new();
    let storage = service.storage_configuration("bounds", "bounds").await;
    // `AutoParticipantRecordingStorageConfigurationArn` is `@length(0..128)`: one
    // character past the maximum, still in the member's `@pattern`.
    let too_long = format!(
        "arn:aws:ivs:{REGION}:000000000000:storage-configuration/{}",
        "a".repeat(129 - format!("arn:aws:ivs:{REGION}:000000000000:storage-configuration/").len())
    );

    // (case, the `autoParticipantRecordingConfiguration` a CreateStage carries)
    let recording: Vec<(&str, Value)> = vec![
        ("no storageConfigurationArn at all", json!({})),
        (
            "a storageConfigurationArn outside the member's pattern",
            json!({ "storageConfigurationArn": "storage-configuration/plain" }),
        ),
        (
            "a storageConfigurationArn one character past the length maximum",
            json!({ "storageConfigurationArn": too_long.as_str() }),
        ),
        (
            "two mediaTypes, one past the list's length maximum",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["AUDIO_VIDEO", "AUDIO_ONLY"],
            }),
        ),
        (
            "a mediaType outside the model's value set",
            json!({
                "storageConfigurationArn": storage.as_str(),
                "mediaTypes": ["VIDEO_ONLY"],
            }),
        ),
    ];
    for (case, configuration) in recording {
        let (status, response) = service
            .call(
                "CreateStage",
                &json!({ "autoParticipantRecordingConfiguration": configuration }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }

    let stage = service
        .create_stage("bounds", &json!({ "name": "composited" }))
        .await;
    let stage_arn = arn_of("bounds", &stage);
    // (case, the grid layout a StartComposition carries)
    let grids: Vec<(&str, Value)> = vec![
        (
            "a videoAspectRatio outside the model's value set",
            json!({ "grid": { "videoAspectRatio": "WIDESCREEN" } }),
        ),
        (
            "a videoFillMode outside the model's value set",
            json!({ "grid": { "videoFillMode": "STRETCH" } }),
        ),
        (
            "a featuredParticipantAttribute outside the attribute-key pattern",
            json!({ "grid": { "featuredParticipantAttribute": "the host" } }),
        ),
        (
            "a participantOrderAttribute one character past the length maximum",
            json!({ "grid": { "participantOrderAttribute": "a".repeat(129) } }),
        ),
    ];
    for (case, layout) in grids {
        let (status, response) = service
            .call(
                "StartComposition",
                &json!({
                    "stageArn": stage_arn.as_str(),
                    "destinations": [{ "channel": {
                        "channelArn":
                            format!("arn:aws:ivs:{REGION}:000000000000:channel/abcdABCD1234"),
                    }}],
                    "layout": layout,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}
