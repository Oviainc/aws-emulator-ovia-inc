// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IVS recording-configuration lifecycle, over the wire.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with
//! a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `CreateRecordingConfiguration` mints and persists a recording configuration
//!    and answers what it stored; `GetRecordingConfiguration` answers the same
//!    resource afterwards, and a resource that was never created is a modelled
//!    `ResourceNotFoundException`.
//! 2. A nested configuration structure whose every member has a documented default
//!    is materialised on the resource, so each of its members is reported with the
//!    value the caller sent or that default — whether or not the caller sent the
//!    structure at all.
//! 3. A new recording configuration is reported `CREATING` and the clock — not a
//!    read — turns it `ACTIVE` once the activation span is due.
//! 4. A `renditionConfiguration` that selects `CUSTOM` must name at least one
//!    rendition; anything else is a 400 `ValidationException` and persists nothing.
//! 5. A value outside a modelled range is refused on each published bound's far
//!    side and accepted on the bound itself.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::{
    RECORDING_CONFIGURATION_ACTIVATION, RECORDING_CONFIGURATION_ACTIVE,
    RECORDING_CONFIGURATION_CREATING,
};
use service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service and the region
/// it is addressed to, so every call carries one. The signature itself is never
/// checked here: request authentication is off unless a router turns it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The ARN prefix every recording configuration this emulator mints must carry,
/// from the modelled `RecordingConfigurationArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:recording-configuration/[a-zA-Z0-9-]+$` plus the
/// canonical emulator account id.
const ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:recording-configuration/";

/// A well-formed ARN of a recording configuration that was never created, so the
/// modelled pattern is satisfied and the request reaches the handler.
const ABSENT_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:recording-configuration/ffffffffffff";

/// Every value the model's `RenditionConfigurationRendition` enumeration declares
/// (source: the `RenditionConfigurationRendition` enum — SD | HD | FULL_HD |
/// LOWEST_RESOLUTION).
const RENDITIONS: &[&str] = &["SD", "HD", "FULL_HD", "LOWEST_RESOLUTION"];

/// One IVS emulator over one shared engine, reached the way the router reaches it.
struct Ivs {
    router: MultiServiceRouter,
}

/// One answer from the emulator: the status, the wire error code (the restJson1
/// `__type` discriminator) and the parsed body.
struct Reply {
    status: u16,
    error_code: String,
    body: Value,
}

impl Ivs {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://ivs.us-east-1.amazonaws.com/{operation}"))
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            error_code,
            body: parsed,
        }
    }

    /// Call `operation` and require it to have succeeded, returning the body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let reply = self.call(operation, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {operation} {body}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// Create a recording configuration and return the resource it answered.
    async fn create(&self, request: Value) -> Value {
        self.ok("CreateRecordingConfiguration", request).await["recordingConfiguration"].clone()
    }

    /// Read a recording configuration back and return the resource it answered.
    async fn get(&self, arn: &str) -> Value {
        self.ok("GetRecordingConfiguration", json!({ "arn": arn })).await
            ["recordingConfiguration"]
            .clone()
    }
}

/// A string member, read as the empty string when absent — which is what a client
/// sees for a member the wire omits.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `arn` satisfies the modelled `RecordingConfigurationArn` pattern: the
/// fixed prefix followed by a non-empty `[a-zA-Z0-9-]+` id, within the modelled
/// 128-character bound.
fn arn_is_well_formed(arn: &str) -> bool {
    let Some(id) = arn.strip_prefix(ARN_PREFIX) else {
        return false;
    };
    arn.len() <= 128
        && !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// An S3 destination naming `bucket`, the only destination type the model declares.
fn destination(bucket: &str) -> Value {
    json!({ "s3": { "bucketName": bucket } })
}

/// Rules 1 and 2 together, over a table spanning the create surface: what
/// `CreateRecordingConfiguration` returns is what `GetRecordingConfiguration`
/// answers afterwards, and every omitted member of a nested configuration the row
/// sent carries the default the model documents for it.
#[tokio::test]
async fn a_created_recording_configuration_is_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: which members are sent at
    // all, the bucket name, the reconnect window, the size of `tags`, and the shape
    // of each nested configuration.
    let cases: Vec<(&str, Value)> = vec![
        (
            "only the required destination",
            json!({ "destinationConfiguration": destination("ivs-recordings") }),
        ),
        (
            "named, with tags",
            json!({
                "name": "demo_recording-1",
                "destinationConfiguration": destination("a-b.c"),
                "tags": { "team": "video", "env": "test" },
            }),
        ),
        (
            "reconnect window at the top of the modelled range",
            json!({
                "name": "reconnect",
                "destinationConfiguration": destination("ivs-recordings"),
                "recordingReconnectWindowSeconds": 300,
            }),
        ),
        (
            "thumbnails every second, stored both ways, at a chosen resolution",
            json!({
                "destinationConfiguration": destination("ivs-recordings"),
                "thumbnailConfiguration": {
                    "recordingMode": "INTERVAL",
                    "targetIntervalSeconds": 1,
                    "resolution": "FULL_HD",
                    "storage": ["SEQUENTIAL", "LATEST"],
                },
            }),
        ),
        (
            "thumbnails disabled",
            json!({
                "destinationConfiguration": destination("ivs-recordings"),
                "thumbnailConfiguration": { "recordingMode": "DISABLED" },
            }),
        ),
        (
            "thumbnail configuration sent empty, so every member defaults",
            json!({
                "destinationConfiguration": destination("ivs-recordings"),
                "thumbnailConfiguration": {},
            }),
        ),
        (
            "all renditions recorded",
            json!({
                "destinationConfiguration": destination("ivs-recordings"),
                "renditionConfiguration": { "renditionSelection": "ALL" },
            }),
        ),
        (
            "no renditions recorded",
            json!({
                "destinationConfiguration": destination("ivs-recordings"),
                "renditionConfiguration": { "renditionSelection": "NONE" },
            }),
        ),
        (
            "a custom set of renditions, and thumbnails beside it",
            json!({
                "name": "custom",
                "destinationConfiguration": destination("ivs-recordings"),
                "renditionConfiguration": {
                    "renditionSelection": "CUSTOM",
                    "renditions": ["LOWEST_RESOLUTION", "HD"],
                },
                "thumbnailConfiguration": {
                    "recordingMode": "INTERVAL",
                    "targetIntervalSeconds": 60,
                },
                "recordingReconnectWindowSeconds": 42,
            }),
        ),
        (
            "rendition configuration sent empty, so the selection defaults",
            json!({
                "destinationConfiguration": destination("ivs-recordings"),
                "renditionConfiguration": {},
            }),
        ),
    ];

    let ivs = Ivs::new();
    for (case, request) in cases {
        let created = ivs.create(request.clone()).await;

        let arn = text(&created, "arn");
        assert!(
            arn_is_well_formed(&arn),
            "case {case:?}: arn must satisfy the modelled RecordingConfigurationArn \
             pattern, got {arn:?}"
        );
        assert_eq!(
            text(&created, "state"),
            RECORDING_CONFIGURATION_CREATING,
            "case {case:?}: a new recording configuration is reported CREATING, got {created}"
        );

        // Echoed-and-stored members, and the model's documented defaults for the
        // members this row left out.
        assert_eq!(
            text(&created, "name"),
            request.get("name").and_then(Value::as_str).unwrap_or(""),
            "case {case:?}: name"
        );
        assert_eq!(
            created["destinationConfiguration"], request["destinationConfiguration"],
            "case {case:?}: destinationConfiguration"
        );
        assert_eq!(
            created
                .get("recordingReconnectWindowSeconds")
                .and_then(Value::as_i64),
            Some(
                request
                    .get("recordingReconnectWindowSeconds")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            ),
            "case {case:?}: recordingReconnectWindowSeconds (model default 0), got {created}"
        );
        assert_eq!(
            created.get("tags").cloned().unwrap_or_else(|| json!({})),
            request.get("tags").cloned().unwrap_or_else(|| json!({})),
            "case {case:?}: tags"
        );
        assert_eq!(
            created.get("thumbnailConfiguration").cloned(),
            expected_thumbnail(&request),
            "case {case:?}: thumbnailConfiguration, got {created}"
        );
        assert_eq!(
            created.get("renditionConfiguration").cloned(),
            expected_rendition(&request),
            "case {case:?}: renditionConfiguration, got {created}"
        );

        // Rule 1: the read answers from state, identically. Only `state` may have
        // moved on, and only because the clock owed the move (rule 3 below).
        let fetched = ivs.get(&arn).await;
        let settled = text(&fetched, "state");
        assert!(
            settled == RECORDING_CONFIGURATION_CREATING || settled == RECORDING_CONFIGURATION_ACTIVE,
            "case {case:?}: a read reports CREATING or ACTIVE, got {settled:?}"
        );
        let mut without_state = (created.clone(), fetched.clone());
        for side in [&mut without_state.0, &mut without_state.1] {
            side.as_object_mut()
                .expect("the resource is a JSON object")
                .remove("state");
        }
        assert_eq!(
            without_state.1, without_state.0,
            "case {case:?}: GetRecordingConfiguration must answer the resource \
             CreateRecordingConfiguration persisted"
        );
    }
}

/// The `thumbnailConfiguration` the service must report for `request`: what the row
/// sent, with each omitted member filled from the model's documented default
/// (`recordingMode` INTERVAL; `targetIntervalSeconds` 60, which is documented only
/// for INTERVAL mode; `storage` [SEQUENTIAL]; `resolution` null, so it stays out).
/// A row that sent no `thumbnailConfiguration` at all chose nothing for a structure
/// every other member of which has a documented default, so it is reported
/// defaulted — the same answer an empty structure gets.
fn expected_thumbnail(request: &Value) -> Option<Value> {
    let sent = request
        .get("thumbnailConfiguration")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut expected = sent.as_object().expect("an object was sent").clone();
    let mode = expected
        .get("recordingMode")
        .and_then(Value::as_str)
        .unwrap_or("INTERVAL")
        .to_string();
    expected.insert("recordingMode".to_string(), json!(mode));
    if mode == "INTERVAL" && !expected.contains_key("targetIntervalSeconds") {
        expected.insert("targetIntervalSeconds".to_string(), json!(60));
    }
    if !expected.contains_key("storage") {
        expected.insert("storage".to_string(), json!(["SEQUENTIAL"]));
    }
    Some(Value::Object(expected))
}

/// The `renditionConfiguration` the service must report for `request`: what the row
/// sent, with `renditionSelection` filled from its documented default (ALL) when the
/// row left it out. As above, an omitted structure is reported defaulted.
fn expected_rendition(request: &Value) -> Option<Value> {
    let sent = request
        .get("renditionConfiguration")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut expected = sent.as_object().expect("an object was sent").clone();
    if !expected.contains_key("renditionSelection") {
        expected.insert("renditionSelection".to_string(), json!("ALL"));
    }
    Some(Value::Object(expected))
}

/// Rule 3: the `CREATING` → `ACTIVE` move is owed by the clock, so a caller acting
/// straight after the create still observes `CREATING`, and once the span is due
/// every read reports `ACTIVE` — repeatedly, because a read changes nothing.
#[tokio::test]
async fn the_clock_turns_a_new_recording_configuration_active() {
    assert!(
        (Duration::from_millis(250)..=Duration::from_millis(750))
            .contains(&RECORDING_CONFIGURATION_ACTIVATION),
        "the owed transition must be due 250-750 ms out, got {RECORDING_CONFIGURATION_ACTIVATION:?}"
    );

    let ivs = Ivs::new();
    // Two resources created one after the other: each settles on its own due
    // instant, so neither read drags the other along.
    let mut arns = Vec::new();
    for bucket in ["first-bucket", "second-bucket"] {
        let created = ivs
            .create(json!({ "destinationConfiguration": destination(bucket) }))
            .await;
        let arn = text(&created, "arn");
        assert_eq!(
            text(&ivs.get(&arn).await, "state"),
            RECORDING_CONFIGURATION_CREATING,
            "case {bucket}: a read before the activation span is due reports CREATING"
        );
        arns.push((bucket, arn));
    }

    tokio::time::sleep(RECORDING_CONFIGURATION_ACTIVATION + Duration::from_millis(250)).await;

    for (bucket, arn) in &arns {
        for read in 1..=2 {
            assert_eq!(
                text(&ivs.get(arn).await, "state"),
                RECORDING_CONFIGURATION_ACTIVE,
                "case {bucket} read #{read}: once the activation span is due every read \
                 reports ACTIVE"
            );
        }
    }
}

/// Rule 4, over the model's `RenditionConfigurationRenditionSelection` values
/// (source: that enum — ALL | NONE | CUSTOM) crossed with an absent, empty,
/// single-entry and multi-entry `renditions` list.
#[tokio::test]
async fn custom_rendition_selection_must_name_at_least_one_rendition() {
    let mut cases: Vec<(String, Value, bool)> = Vec::new();
    for selection in ["ALL", "NONE", "CUSTOM"] {
        let lists: Vec<(&str, Option<Value>)> = vec![
            ("renditions omitted", None),
            ("renditions empty", Some(json!([]))),
            // An entry the issue did not cite, from the modelled enum.
            ("one rendition", Some(json!([RENDITIONS[0]]))),
            ("every rendition", Some(json!(RENDITIONS))),
        ];
        for (label, renditions) in lists {
            let mut rendition_configuration = json!({ "renditionSelection": selection });
            let empty = renditions
                .as_ref()
                .is_none_or(|list| list.as_array().is_some_and(Vec::is_empty));
            if let Some(list) = renditions {
                rendition_configuration["renditions"] = list;
            }
            cases.push((
                format!("{selection} with {label}"),
                json!({
                    "destinationConfiguration": destination("ivs-recordings"),
                    "renditionConfiguration": rendition_configuration,
                }),
                // Only CUSTOM needs the list: "If CUSTOM is specified, a set of
                // renditions must be specified in the renditions field."
                !(selection == "CUSTOM" && empty),
            ));
        }
    }

    let ivs = Ivs::new();
    for (case, request, accepted) in cases {
        let reply = ivs.call("CreateRecordingConfiguration", &request).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {}",
                reply.status, reply.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code.ends_with("ValidationException"),
            "case {case:?}: expected error code ValidationException, got {:?}",
            reply.error_code
        );
    }

    // Nothing a refusal touched was persisted: only the accepted rows exist, and
    // every one of them is readable.
    let listed = ivs
        .ok("CreateRecordingConfiguration", json!({
            "destinationConfiguration": destination("ivs-recordings"),
            "renditionConfiguration": { "renditionSelection": "CUSTOM", "renditions": ["SD"] },
        }))
        .await;
    let arn = text(&listed["recordingConfiguration"], "arn");
    assert_eq!(
        text(&ivs.get(&arn).await, "arn"),
        arn,
        "a resource created after the refusals is still readable"
    );
}

/// Rule 5, on each modelled numeric range this operation carries: the published
/// bounds are accepted and one step past either is refused with a 400
/// `ValidationException` (source: `RecordingReconnectWindowSeconds` @range 0..300
/// and `ThumbnailConfiguration$targetIntervalSeconds` @range 1..60).
#[tokio::test]
async fn a_value_outside_a_modelled_range_is_refused() {
    let reconnect = |seconds: i64| {
        json!({
            "destinationConfiguration": destination("ivs-recordings"),
            "recordingReconnectWindowSeconds": seconds,
        })
    };
    let interval = |seconds: i64| {
        json!({
            "destinationConfiguration": destination("ivs-recordings"),
            "thumbnailConfiguration": {
                "recordingMode": "INTERVAL",
                "targetIntervalSeconds": seconds,
            },
        })
    };
    let cases: Vec<(String, Value, bool)> = vec![
        ("reconnect window 0 (lower bound)".to_string(), reconnect(0), true),
        ("reconnect window 300 (upper bound)".to_string(), reconnect(300), true),
        ("reconnect window -1 (below the lower bound)".to_string(), reconnect(-1), false),
        ("reconnect window 301 (above the upper bound)".to_string(), reconnect(301), false),
        ("thumbnail interval 1 (lower bound)".to_string(), interval(1), true),
        ("thumbnail interval 60 (upper bound)".to_string(), interval(60), true),
        ("thumbnail interval 0 (below the lower bound)".to_string(), interval(0), false),
        ("thumbnail interval 61 (above the upper bound)".to_string(), interval(61), false),
    ];

    let ivs = Ivs::new();
    for (case, request, accepted) in cases {
        let reply = ivs.call("CreateRecordingConfiguration", &request).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_code.ends_with("ValidationException"),
                "case {case:?}: expected error code ValidationException, got {:?}",
                reply.error_code
            );
        }
    }
}

/// Rule 1's negative half: a well-formed ARN the store never minted is the modelled
/// `ResourceNotFoundException` (404), both before anything exists and beside a
/// resource that does.
#[tokio::test]
async fn reading_a_recording_configuration_that_was_never_created_is_not_found() {
    let ivs = Ivs::new();
    for stage in ["empty store", "beside an existing resource"] {
        let reply = ivs
            .call("GetRecordingConfiguration", &json!({ "arn": ABSENT_ARN }))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {stage:?}: expected HTTP 404, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code.ends_with("ResourceNotFoundException"),
            "case {stage:?}: expected error code ResourceNotFoundException, got {:?}",
            reply.error_code
        );
        ivs.create(json!({ "destinationConfiguration": destination("ivs-recordings") }))
            .await;
    }
}

/// "Recording-configuration name. The value does not need to be unique" (model doc
/// on `RecordingConfigurationName`): each create mints a distinct resource, and the
/// two are readable independently.
#[tokio::test]
async fn names_need_not_be_unique_and_every_create_mints_its_own_resource() {
    let ivs = Ivs::new();
    let request = json!({
        "name": "shared-name",
        "destinationConfiguration": destination("ivs-recordings"),
    });
    let first = text(&ivs.create(request.clone()).await, "arn");
    let second = text(&ivs.create(request.clone()).await, "arn");
    assert_ne!(
        first, second,
        "case {request}: two creates must mint two distinct ARNs"
    );
    for arn in [&first, &second] {
        assert_eq!(
            text(&ivs.get(arn).await, "arn"),
            *arn,
            "case {request}: each minted resource is readable by its own ARN"
        );
        assert_eq!(
            text(&ivs.get(arn).await, "name"),
            "shared-name",
            "case {request}: each minted resource keeps the name it was created with"
        );
    }
}
