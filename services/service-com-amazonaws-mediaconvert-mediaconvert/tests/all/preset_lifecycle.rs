// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the output-preset control plane: `CreatePreset` and
//! `GetPreset`.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from
//! the request's SigV4 scope, the framework deserializes and validates, the handler
//! runs, and the response is serialized — so what is asserted is the bytes a client
//! would see, never an internal call.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header in this service's scope, signed for `region`. It is
/// what the router reads to pick the service, and what the handlers read the ARN region
/// from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
}

/// The region every case signs for unless it names another.
const REGION: &str = "us-west-2";

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
    router: MultiServiceRouter,
}

/// One HTTP reply, decoded far enough to assert on.
struct Reply {
    status: u16,
    /// `x-amzn-errortype`, the header restJson1 resolves an error shape from.
    error_type: Option<String>,
    /// The JSON body, or `Value::Null` for an empty one.
    body: Value,
}

impl Reply {
    /// A member of the body, or `Value::Null` when it is absent.
    fn at(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    /// A string member of the body.
    fn string(&self, pointer: &str) -> String {
        self.at(pointer).as_str().unwrap_or_default().to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the ARN region read
        // the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.send_in(REGION, method, uri, body).await
    }

    async fn send_in(&self, region: &str, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("mediaconvert.{region}.amazonaws.com"))
            .header("authorization", authorization(region))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create_preset(&self, body: Value) -> Reply {
        self.send("POST", "/2017-08-29/presets", Some(body)).await
    }

    async fn get_preset(&self, name: &str) -> Reply {
        self.send("GET", &format!("/2017-08-29/presets/{name}"), None)
            .await
    }
}

/// `arn:aws:mediaconvert:<region>:<account>:<resource>`, built the way the emulator's
/// own ARN helper builds it, so the expectation is not a second hand-written format.
fn arn_in(region: &str, resource: &str) -> String {
    engine::emulator_arn("mediaconvert", region, resource)
}

/// One `CreatePreset` request and what it must read back as.
struct Case {
    label: &'static str,
    request: Value,
    /// The `category` the stored preset must carry, or `None` for an absent member.
    category: Option<&'static str>,
    /// The `description` the stored preset must carry, or `None` for an absent member.
    description: Option<&'static str>,
}

/// The settings shapes the table below spans. Every member named here is one the model
/// declares on `PresetSettings` and its closure — `ContainerSettings.Container` (the
/// `ContainerType` enum), `VideoDescription`, `AudioDescription` and
/// `CaptionDescriptionPreset` — see
/// `smithy_query(query="shape",shape="PresetSettings")`.
fn preset_cases() -> Vec<Case> {
    vec![
        Case {
            label: "container only, no optional metadata",
            request: json!({
                "name": "container-only",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
            category: None,
            description: None,
        },
        Case {
            label: "a category and a description beside a video description",
            request: json!({
                "name": "video-1080p",
                "category": "web",
                "description": "1080p H.264 in MP4",
                "settings": {
                    "containerSettings": { "container": "MP4" },
                    "videoDescription": {
                        "width": 1920,
                        "height": 1080,
                        "codecSettings": {
                            "codec": "H_264",
                            "h264Settings": { "bitrate": 5_000_000 }
                        }
                    }
                }
            }),
            category: Some("web"),
            description: Some("1080p H.264 in MP4"),
        },
        Case {
            label: "two audio descriptions, order-significant",
            request: json!({
                "name": "dual-audio",
                "category": "broadcast",
                "settings": {
                    "containerSettings": { "container": "M2TS" },
                    "audioDescriptions": [
                        {
                            "audioSourceName": "Audio Selector 1",
                            "languageCode": "ENG",
                            "codecSettings": {
                                "codec": "AAC",
                                "aacSettings": { "bitrate": 96_000, "sampleRate": 48_000 }
                            }
                        },
                        {
                            "audioSourceName": "Audio Selector 2",
                            "languageCode": "SPA",
                            "codecSettings": {
                                "codec": "AAC",
                                "aacSettings": { "bitrate": 64_000, "sampleRate": 44_100 }
                            }
                        }
                    ]
                }
            }),
            category: Some("broadcast"),
            description: None,
        },
        Case {
            label: "a caption description and a nested container settings object",
            request: json!({
                "name": "captions-mov",
                "description": "captions passthrough",
                "settings": {
                    "containerSettings": {
                        "container": "MP4",
                        "mp4Settings": { "cttsVersion": 1, "mp4MajorBrand": "mp42" }
                    },
                    "captionDescriptions": [{
                        "languageCode": "FRA",
                        "customLanguageCode": "fr-CA",
                        "languageDescription": "French (Canada)"
                    }]
                }
            }),
            category: None,
            description: Some("captions passthrough"),
        },
        Case {
            label: "every PresetSettings member at once, with tags",
            request: json!({
                "name": "everything",
                "category": "archive",
                "description": "all four PresetSettings members",
                "tags": { "team": "media", "cost-centre": "42" },
                "settings": {
                    "containerSettings": { "container": "MXF" },
                    "videoDescription": { "width": 1280, "height": 720 },
                    "audioDescriptions": [{ "audioSourceName": "Audio Selector 1" }],
                    "captionDescriptions": [{ "languageCode": "DEU" }]
                }
            }),
            category: Some("archive"),
            description: Some("all four PresetSettings members"),
        },
        Case {
            label: "empty settings object: no member of PresetSettings given",
            request: json!({ "name": "empty-settings", "settings": {} }),
            category: None,
            description: None,
        },
    ]
}

/// `CreatePreset` persists every member it accepts, derives the rest, and `GetPreset`
/// reads back exactly what was stored.
///
/// The table spans the input class along what the rule generalises over: which optional
/// members are present, the container enum value, the nesting depth of `Settings`, the
/// number and order of the list members, and whether tags are given.
#[tokio::test]
async fn create_preset_persists_its_input_and_get_preset_reads_it_back() {
    let emulator = Emulator::new();

    for case in preset_cases() {
        let label = case.label;
        let name = case.request.pointer("/name").and_then(Value::as_str).unwrap_or_default().to_string();
        let sent_settings = case
            .request
            .pointer("/settings")
            .cloned()
            .unwrap_or(Value::Null);

        let created = emulator.create_preset(case.request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );

        // The create's own response, and then the independent read, must agree with the
        // request member for member.
        let fetched = emulator.get_preset(&name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetPreset, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/preset"),
            created.at("/preset"),
            "case {label:?}: GetPreset must return what CreatePreset returned, got {}",
            fetched.body
        );

        for (source, reply) in [("CreatePreset", &created), ("GetPreset", &fetched)] {
            assert_eq!(
                reply.string("/preset/name"),
                name,
                "case {label:?} via {source}: expected the name as sent, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/preset/settings"),
                &sent_settings,
                "case {label:?} via {source}: expected the settings as sent, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/preset/category"),
                &case.category.map_or(Value::Null, |value| json!(value)),
                "case {label:?} via {source}: expected category {:?}, got {}",
                case.category,
                reply.body
            );
            assert_eq!(
                reply.at("/preset/description"),
                &case
                    .description
                    .map_or(Value::Null, |value| json!(value)),
                "case {label:?} via {source}: expected description {:?}, got {}",
                case.description,
                reply.body
            );
            // `Type`: "A preset can be of two types: system or custom." A preset a caller
            // created is never a built-in one.
            assert_eq!(
                reply.string("/preset/type"),
                "CUSTOM",
                "case {label:?} via {source}: expected type CUSTOM, got {}",
                reply.body
            );
            assert_eq!(
                reply.string("/preset/arn"),
                arn_in(REGION, &format!("presets/{name}")),
                "case {label:?} via {source}: expected the preset ARN, got {}",
                reply.body
            );
            // `CreatedAt`/`LastUpdated` are "the timestamp in epoch seconds", so whole
            // integers; nothing has updated a freshly created preset, so they agree.
            let created_at = reply.at("/preset/createdAt").as_i64();
            let last_updated = reply.at("/preset/lastUpdated").as_i64();
            assert!(
                created_at.is_some_and(|seconds| seconds > 1_600_000_000),
                "case {label:?} via {source}: expected createdAt in epoch seconds, got {}",
                reply.body
            );
            assert_eq!(
                created_at, last_updated,
                "case {label:?} via {source}: a preset nothing updated has \
                 lastUpdated == createdAt, got {}",
                reply.body
            );
        }
    }
}

/// `Name` is the preset's identity: a second create under a name already in the store is
/// IDEMPOTENT — the same 201 with the preset that is already there — and it leaves the
/// stored preset alone whatever the repeat's body asks for.
#[tokio::test]
async fn re_creating_a_preset_returns_the_stored_preset() {
    let emulator = Emulator::new();
    let first = emulator
        .create_preset(json!({
            "name": "unique",
            "description": "the original",
            "settings": { "containerSettings": { "container": "MP4" } }
        }))
        .await;
    assert_eq!(
        first.status, 201,
        "the fixture preset must be created: got {} with body {}",
        first.status, first.body
    );

    // Each row differs in what the second create asks for, so what comes back cannot
    // depend on the body matching the first one: the stored preset is returned either way.
    let collisions = [
        (
            "identical body",
            json!({
                "name": "unique",
                "description": "the original",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
        ),
        (
            "different settings",
            json!({
                "name": "unique",
                "settings": { "containerSettings": { "container": "WEBM" } }
            }),
        ),
        (
            "a category the first create did not carry",
            json!({
                "name": "unique",
                "category": "web",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
        ),
    ];

    for (label, request) in collisions {
        let reply = emulator.create_preset(request).await;
        assert_eq!(
            reply.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.at("/preset"),
            first.at("/preset"),
            "case {label:?}: the repeat must return the STORED preset, got {}",
            reply.body
        );
    }

    // The name is the resource's identity and is never normalised, so a name that only
    // differs by surrounding whitespace is a DIFFERENT preset, not the same one, and it
    // is echoed back exactly as sent.
    for padded in ["unique ", " unique", "  unique  "] {
        let reply = emulator
            .create_preset(json!({
                "name": padded,
                "settings": { "containerSettings": { "container": "MP4" } }
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {padded:?}: expected HTTP 201 for a distinct name, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("/preset/name"),
            padded,
            "case {padded:?}: expected the name echoed verbatim, got {}",
            reply.body
        );
    }

    let fetched = emulator.get_preset("unique").await;
    assert_eq!(
        fetched.at("/preset"),
        first.at("/preset"),
        "the repeated creates must leave the stored preset untouched, got {}",
        fetched.body
    );
}

/// A preset nobody created does not exist: `GetPreset` answers the modelled
/// `NotFoundException`, and a preset under a neighbouring name does not satisfy a read.
#[tokio::test]
async fn getting_a_preset_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_preset(json!({
            "name": "present",
            "settings": { "containerSettings": { "container": "MP4" } }
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created: got {} with body {}",
        created.status, created.body
    );

    // The rows differ in how they miss: never created, a prefix of a stored name, a
    // different case, and a name no create ever used.
    for name in ["absent", "presen", "PRESENT", "System-Ultra-HD"] {
        let reply = emulator.get_preset(name).await;
        assert_eq!(
            reply.status, 404,
            "case {name:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {name:?}: expected NotFoundException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
    }
}

/// A request the model's own rules refuse, and a name carrying no characters, are
/// answered with a code this service's model declares — `BadRequestException`.
///
/// The bounded rows use `VideoDescription`'s published `Width`/`Height` range, 32 to
/// 8192 (`smithy_query(query="shape",shape="VideoDescription")`): each bound is accepted
/// and one step past each is refused.
#[tokio::test]
async fn a_request_the_model_refuses_is_a_bad_request() {
    let emulator = Emulator::new();
    let refusals: Vec<(&str, Value)> = vec![
        (
            "Name absent",
            json!({ "settings": { "containerSettings": { "container": "MP4" } } }),
        ),
        ("Settings absent", json!({ "name": "no-settings" })),
        ("both absent", json!({})),
        (
            "Name is only whitespace",
            json!({
                "name": "   ",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
        ),
        (
            "Name is empty",
            json!({
                "name": "",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
        ),
        (
            "Width one below the 32 minimum",
            json!({
                "name": "too-narrow",
                "settings": { "videoDescription": { "width": 31 } }
            }),
        ),
        (
            "Height one above the 8192 maximum",
            json!({
                "name": "too-tall",
                "settings": { "videoDescription": { "height": 8193 } }
            }),
        ),
        (
            "Container outside the enumerated set",
            json!({
                "name": "no-such-container",
                "settings": { "containerSettings": { "container": "MKV" } }
            }),
        ),
    ];

    for (label, request) in &refusals {
        let reply = emulator.create_preset(request.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        // A refused create persists nothing, so the name it asked for is still free.
        // Only names that can sit in a URI path segment are checked this way.
        let name = request.pointer("/name").and_then(Value::as_str).unwrap_or_default();
        if !name.is_empty() && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
            let fetched = emulator.get_preset(name).await;
            assert_eq!(
                fetched.status, 404,
                "case {label:?}: a refused create must persist nothing, got {} with body {}",
                fetched.status, fetched.body
            );
        }
    }

    // And each published bound of the range is accepted and stored.
    for edge in [32, 8192] {
        let name = format!("edge-{edge}");
        let reply = emulator
            .create_preset(json!({
                "name": name,
                "settings": { "videoDescription": { "width": edge, "height": edge } }
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case width/height {edge}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        let fetched = emulator.get_preset(&name).await;
        assert_eq!(
            fetched.at("/preset/settings/videoDescription/width"),
            &json!(edge),
            "case width/height {edge}: expected the bound stored, got {}",
            fetched.body
        );
    }
}

/// Every preset ARN names the region of the request that read it — not the region of
/// whichever caller happened to create it.
#[tokio::test]
async fn a_preset_arn_names_the_region_of_the_request_that_reads_it() {
    let emulator = Emulator::new();
    let created = emulator
        .send_in(
            "eu-central-1",
            "POST",
            "/2017-08-29/presets",
            Some(json!({
                "name": "regional",
                "settings": { "containerSettings": { "container": "MP4" } }
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created: got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        created.string("/preset/arn"),
        arn_in("eu-central-1", "presets/regional"),
        "the create's own ARN follows its own region, got {}",
        created.body
    );

    for region in ["eu-central-1", "us-west-2", "ap-southeast-2"] {
        let reply = emulator
            .send_in(region, "GET", "/2017-08-29/presets/regional", None)
            .await;
        assert_eq!(
            reply.string("/preset/arn"),
            arn_in(region, "presets/regional"),
            "case {region:?}: expected the preset ARN in the reading region, got {}",
            reply.body
        );
    }
}

/// `GetPreset` reports; it changes nothing a later caller sees. Reading a preset
/// repeatedly returns byte-identical bodies, and reading one does not bring another into
/// existence.
#[tokio::test]
async fn reading_a_preset_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    for name in ["first", "second"] {
        let created = emulator
            .create_preset(json!({
                "name": name,
                "category": "stable",
                "settings": { "containerSettings": { "container": "MP4" } }
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: the fixture preset must be created: got {} with body {}",
            created.status, created.body
        );
    }

    let baseline = emulator.get_preset("first").await;
    for round in 1..=4 {
        // A miss between the hits: a 404 must not create anything either.
        let missing = emulator.get_preset("never-created").await;
        assert_eq!(
            missing.status, 404,
            "case round {round}: expected HTTP 404 for an absent preset, got {} with body {}",
            missing.status, missing.body
        );
        let again = emulator.get_preset("first").await;
        assert_eq!(
            again.body, baseline.body,
            "case round {round}: a read must not change what a later read sees, got {}",
            again.body
        );
        let other = emulator.get_preset("second").await;
        assert_eq!(
            other.status, 200,
            "case round {round}: the other preset must still read back, got {} with body {}",
            other.status, other.body
        );
    }
}
