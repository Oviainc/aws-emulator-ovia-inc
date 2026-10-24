// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the account's BUILT-IN (`SYSTEM`) output presets.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule every case below is one row of: an account holds the service's published
//! output presets without creating them, so each reads back through `GetPreset` with
//! `Type` `SYSTEM` and the encoder settings its published name states, and none of them is
//! the caller's to re-create (`ConflictException`), change or remove
//! (`ForbiddenException`) — the same contract the built-in queue and the built-in job
//! templates carry.
//!
//! The catalogue is read out of the STORE, so no case here hard-codes it; what each case
//! does hard-code is the independent reading of a preset's NAME ([`NameSpec::of`]), which
//! is where the expectation for its settings comes from.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{TYPE_CUSTOM, TYPE_SYSTEM};
use service_com_amazonaws_mediaconvert_mediaconvert::{
    ComAmazonawsMediaconvertMediaconvertState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for unless it names another.
const REGION: &str = "us-west-2";

/// A SigV4 `Authorization` header in this service's scope, signed for `region`. It is what
/// the router reads to pick the service, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
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

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
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
        Self { engine, router }
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

    async fn get_preset(&self, name: &str) -> Reply {
        self.send("GET", &format!("/2017-08-29/presets/{name}"), None)
            .await
    }

    async fn create_preset(&self, body: Value) -> Reply {
        self.send("POST", "/2017-08-29/presets", Some(body)).await
    }

    async fn update_preset(&self, name: &str, body: Value) -> Reply {
        self.send("PUT", &format!("/2017-08-29/presets/{name}"), Some(body))
            .await
    }

    async fn delete_preset(&self, name: &str) -> Reply {
        self.send("DELETE", &format!("/2017-08-29/presets/{name}"), None)
            .await
    }

    /// The names of the stored presets of one `Type`, in store order — read from the store
    /// so no case hard-codes the built-in catalogue. (`ListPresets` is still a 501 stub, so
    /// there is no wire path that reports the whole preset set yet.)
    fn stored_names_of_type(&self, preset_type: &str) -> Vec<String> {
        self.engine
            .state::<ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .presets
            .values()
            .filter(|record| record.preset_type == preset_type)
            .map(|record| record.name.clone())
            .collect()
    }
}

/// `arn:aws:mediaconvert:<region>:<account>:<resource>`, built the way the emulator's own
/// ARN helper builds it, so the expectation is not a second hand-written format.
fn arn_in(region: &str, resource: &str) -> String {
    engine::emulator_arn("mediaconvert", region, resource)
}

/// Published names of the service's built-in presets that an account must serve.
///
/// Source: the AWS Elemental MediaConvert system-preset catalogue (the console's "System
/// presets" tab, the `System-*` names). The first row is the name the issue cited; the
/// others are entries it did not, spanning the catalogue's other output families — an HLS
/// (MPEG-2 TS) rendition, a DASH (fragmented MP4) rendition and a UHD HEVC output.
const PUBLISHED_NAMES: [&str; 4] = [
    "System-Generic_Hd_Mp4_Avc_Aac_16x9_1920x1080p_24Hz_6Mbps",
    "System-Ott_Hls_Ts_Avc_Aac_16x9_1280x720p_30Hz_3.5Mbps",
    "System-Ott_Dash_Mp4_Avc_Aac_16x9_1280x720p_30Hz_3.5Mbps",
    "System-Generic_Uhd_Mp4_Hevc_Aac_16x9_3840x2160p_24Hz_20Mbps",
];

/// What a built-in preset's published NAME states about the output it produces.
///
/// A system preset is named after its output — family, container, video codec, audio
/// codec, display aspect ratio, frame size, frame rate and video bitrate — so this suite
/// reads those parameters back OUT of the name and asserts the served `Settings` graph
/// agrees with them. The expectation is therefore independent of the table the emulator
/// seeds from: a settings graph that did not come from the name fails here.
#[derive(Debug)]
struct NameSpec {
    /// The `ContainerType` value the name's container token states.
    container: &'static str,
    /// The `VideoCodec` value the name's codec token states.
    video_codec: &'static str,
    /// The `(width, height)` of the name's `<W>x<H>p` token.
    frame_size: (i64, i64),
    /// The `(horizontal, vertical)` of the name's `<H>x<V>` display-aspect token.
    aspect: (i64, i64),
    /// Frames per second, from the name's `<N>Hz` token.
    framerate: i64,
    /// Bits per second, from the name's `<N>Mbps` token.
    video_bitrate: i64,
}

impl NameSpec {
    fn of(name: &str) -> Self {
        let container = if name.contains("_Hls_Ts_") {
            // An HLS rendition's segments are MPEG-2 transport streams, the `M3U8`
            // container (`M3u8Settings`: "the MPEG-2 transport stream (MPEG2-TS) container
            // for the MPEG2-TS segments in your HLS outputs").
            "M3U8"
        } else if name.contains("_Dash_Mp4_") {
            // A DASH rendition's segments are fragmented MP4, the `MPD` container.
            "MPD"
        } else if name.contains("_Mp4_") {
            "MP4"
        } else {
            panic!("case {name:?}: the name states no container this suite knows");
        };
        let video_codec = if name.contains("_Avc_") {
            "H_264"
        } else if name.contains("_Hevc_") {
            "H_265"
        } else {
            panic!("case {name:?}: the name states no video codec this suite knows");
        };

        let mut frame_size = None;
        let mut aspect = None;
        let mut framerate = None;
        let mut video_bitrate = None;
        for token in name.split('_') {
            if let Some(size) = token.strip_suffix('p').and_then(dimensions) {
                frame_size = Some(size);
            } else if let Some(ratio) = dimensions(token) {
                aspect = Some(ratio);
            } else if let Some(hz) = token.strip_suffix("Hz").and_then(|hz| hz.parse().ok()) {
                framerate = Some(hz);
            } else if let Some(mbps) = token
                .strip_suffix("Mbps")
                .and_then(|mbps| mbps.parse::<f64>().ok())
            {
                video_bitrate = Some((mbps * 1_000_000.0).round() as i64);
            }
        }

        Self {
            container,
            video_codec,
            frame_size: frame_size
                .unwrap_or_else(|| panic!("case {name:?}: the name states no frame size")),
            aspect: aspect
                .unwrap_or_else(|| panic!("case {name:?}: the name states no aspect ratio")),
            framerate: framerate
                .unwrap_or_else(|| panic!("case {name:?}: the name states no frame rate")),
            video_bitrate: video_bitrate
                .unwrap_or_else(|| panic!("case {name:?}: the name states no bitrate")),
        }
    }

    /// The member of `VideoCodecSettings` that carries this codec's own settings object —
    /// "for each codec enum that you choose, define the corresponding settings object".
    fn codec_settings_member(&self) -> &'static str {
        if self.video_codec == "H_265" {
            "h265Settings"
        } else {
            "h264Settings"
        }
    }
}

/// `<a>x<b>` as a pair, or `None` when the token is not of that form.
fn dimensions(token: &str) -> Option<(i64, i64)> {
    let (left, right) = token.split_once('x')?;
    Some((left.parse().ok()?, right.parse().ok()?))
}

/// Assert that one served preset body is the built-in preset its name states.
fn assert_matches_its_name(name: &str, reply: &Reply, region: &str) {
    let spec = NameSpec::of(name);
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    // `Preset$Type`: "A preset can be of two types: system or custom. System or built-in
    // preset can't be modified or deleted by the user."
    assert_eq!(
        reply.string("/preset/type"),
        TYPE_SYSTEM,
        "case {name:?}: expected type {TYPE_SYSTEM}, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("/preset/name"),
        name,
        "case {name:?}: expected the name read back verbatim, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("/preset/arn"),
        arn_in(region, &format!("presets/{name}")),
        "case {name:?}: expected the preset ARN in the reading region, got {}",
        reply.body
    );
    // `Category`/`Description` are "an optional category/description YOU create", so a
    // preset no caller created carries neither.
    for member in ["category", "description"] {
        assert_eq!(
            reply.at(&format!("/preset/{member}")),
            &Value::Null,
            "case {name:?}: a preset nobody created carries no {member}, got {}",
            reply.body
        );
    }
    // The timestamps are "the timestamp in epoch seconds", and nothing has updated a
    // built-in preset, so they agree.
    let created_at = reply.at("/preset/createdAt").as_i64();
    assert!(
        created_at.is_some_and(|seconds| seconds > 1_600_000_000),
        "case {name:?}: expected createdAt in epoch seconds, got {}",
        reply.body
    );
    assert_eq!(
        created_at,
        reply.at("/preset/lastUpdated").as_i64(),
        "case {name:?}: a preset nothing updated has lastUpdated == createdAt, got {}",
        reply.body
    );

    // The settings graph states exactly what the name does.
    assert_eq!(
        reply.string("/preset/settings/containerSettings/container"),
        spec.container,
        "case {name:?}: expected container {:?}, got {}",
        spec.container,
        reply.body
    );
    let (width, height) = spec.frame_size;
    assert_eq!(
        reply.at("/preset/settings/videoDescription/width").as_i64(),
        Some(width),
        "case {name:?}: expected width {width}, got {}",
        reply.body
    );
    assert_eq!(
        reply.at("/preset/settings/videoDescription/height").as_i64(),
        Some(height),
        "case {name:?}: expected height {height}, got {}",
        reply.body
    );
    // The frame size carries the display aspect ratio the name states.
    let (aspect_h, aspect_v) = spec.aspect;
    assert_eq!(
        width * aspect_v,
        height * aspect_h,
        "case {name:?}: expected a {aspect_h}x{aspect_v} frame, got {width}x{height}"
    );
    assert_eq!(
        reply.string("/preset/settings/videoDescription/codecSettings/codec"),
        spec.video_codec,
        "case {name:?}: expected video codec {:?}, got {}",
        spec.video_codec,
        reply.body
    );
    let codec = format!(
        "/preset/settings/videoDescription/codecSettings/{}",
        spec.codec_settings_member()
    );
    assert_eq!(
        reply.at(&format!("{codec}/bitrate")).as_i64(),
        Some(spec.video_bitrate),
        "case {name:?}: expected {} bits per second under {}, got {}",
        spec.video_bitrate,
        spec.codec_settings_member(),
        reply.body
    );
    // "specify the frame rate as a fraction": the fraction reduces to the stated Hz.
    let numerator = reply
        .at(&format!("{codec}/framerateNumerator"))
        .as_i64()
        .unwrap_or_default();
    let denominator = reply
        .at(&format!("{codec}/framerateDenominator"))
        .as_i64()
        .unwrap_or_default();
    assert!(
        denominator > 0 && numerator == spec.framerate * denominator,
        "case {name:?}: expected {} fps as a fraction, got {numerator}/{denominator}",
        spec.framerate
    );

    // The name states one AAC audio track, so there is exactly one audio description and
    // it carries AAC settings with a bitrate.
    let audio = reply
        .at("/preset/settings/audioDescriptions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        audio.len(),
        1,
        "case {name:?}: expected one audio description, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("/preset/settings/audioDescriptions/0/codecSettings/codec"),
        "AAC",
        "case {name:?}: expected audio codec AAC, got {}",
        reply.body
    );
    assert!(
        reply
            .at("/preset/settings/audioDescriptions/0/codecSettings/aacSettings/bitrate")
            .as_i64()
            // `AacSettings$Bitrate`'s published range is 6000..1024000.
            .is_some_and(|bitrate| (6_000..=1_024_000).contains(&bitrate)),
        "case {name:?}: expected an AAC bitrate inside the published range, got {}",
        reply.body
    );
}

/// A fresh account serves every built-in preset: the published names resolve, each one is
/// `SYSTEM`, and each one's settings are the ones its name states.
///
/// The table is the whole seeded catalogue, read from the store — its rows differ in
/// container, video codec, frame size, aspect ratio, frame rate and bitrate — plus the
/// published names a caller starts from, asserted by name.
#[tokio::test]
async fn a_fresh_account_serves_every_built_in_preset() {
    let emulator = Emulator::new();

    // Nothing was created, so every preset in the store is a built-in one.
    let built_ins = emulator.stored_names_of_type(TYPE_SYSTEM);
    assert!(
        built_ins.len() >= PUBLISHED_NAMES.len(),
        "a fresh account must hold the service's built-in presets, got {built_ins:?}"
    );
    assert_eq!(
        emulator.stored_names_of_type(TYPE_CUSTOM),
        Vec::<String>::new(),
        "a fresh account has created no preset of its own, got {:?}",
        emulator.stored_names_of_type(TYPE_CUSTOM)
    );

    for name in &built_ins {
        let reply = emulator.get_preset(name).await;
        assert_matches_its_name(name, &reply, REGION);
    }

    for name in PUBLISHED_NAMES {
        assert!(
            built_ins.iter().any(|stored| stored == name),
            "case {name:?}: a published built-in preset must be in the store, got {built_ins:?}"
        );
        let reply = emulator.get_preset(name).await;
        assert_matches_its_name(name, &reply, REGION);
    }

    // A built-in preset's ARN names the region of the request that reads it, as every
    // other resource's does.
    let name = PUBLISHED_NAMES[0];
    for region in ["eu-central-1", "ap-southeast-2"] {
        let reply = emulator
            .send_in(region, "GET", &format!("/2017-08-29/presets/{name}"), None)
            .await;
        assert_matches_its_name(name, &reply, region);
    }
}

/// A built-in preset is not the caller's to re-create, change or remove: `CreatePreset`
/// under its name conflicts, `UpdatePreset` and `DeletePreset` are refused, and the preset
/// is left exactly as it was — while the account's OWN preset accepts all three.
#[tokio::test]
async fn a_built_in_preset_cannot_be_re_created_updated_or_deleted() {
    let emulator = Emulator::new();
    let built_ins = emulator.stored_names_of_type(TYPE_SYSTEM);
    assert!(
        !built_ins.is_empty(),
        "a fresh account must hold the service's built-in presets"
    );

    for name in &built_ins {
        let before = emulator.get_preset(name).await;
        assert_eq!(
            before.status, 200,
            "case {name:?}: the built-in preset must read back first, got {} with body {}",
            before.status, before.body
        );

        // Twice each, so a refusal that quietly mutated would show up on the second pass.
        for attempt in 1..=2 {
            let recreated = emulator
                .create_preset(json!({
                    "name": name,
                    "settings": { "containerSettings": { "container": "WEBM" } }
                }))
                .await;
            assert_eq!(
                recreated.status, 409,
                "case {name:?} attempt {attempt}: expected HTTP 409 from CreatePreset, got {} \
                 with body {}",
                recreated.status, recreated.body
            );
            assert_eq!(
                recreated.error_type.as_deref(),
                Some("ConflictException"),
                "case {name:?} attempt {attempt}: expected ConflictException, got {:?}",
                recreated.error_type
            );

            let updated = emulator
                .update_preset(name, json!({ "description": "mine now" }))
                .await;
            assert_eq!(
                updated.status, 403,
                "case {name:?} attempt {attempt}: expected HTTP 403 from UpdatePreset, got {} \
                 with body {}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.error_type.as_deref(),
                Some("ForbiddenException"),
                "case {name:?} attempt {attempt}: expected ForbiddenException, got {:?}",
                updated.error_type
            );

            let deleted = emulator.delete_preset(name).await;
            assert_eq!(
                deleted.status, 403,
                "case {name:?} attempt {attempt}: expected HTTP 403 from DeletePreset, got {} \
                 with body {}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.error_type.as_deref(),
                Some("ForbiddenException"),
                "case {name:?} attempt {attempt}: expected ForbiddenException, got {:?}",
                deleted.error_type
            );

            let again = emulator.get_preset(name).await;
            assert_eq!(
                again.body, before.body,
                "case {name:?} attempt {attempt}: a refused call must leave the built-in \
                 preset exactly as it was, got {}",
                again.body
            );
        }
    }

    // The contrast row: the account's own preset is created, updated and deleted, so the
    // refusals above come from the preset's TYPE and not from the operations being closed.
    let created = emulator
        .create_preset(json!({
            "name": "my-own-preset",
            "settings": { "containerSettings": { "container": "MP4" } }
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "case \"my-own-preset\": expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        created.string("/preset/type"),
        TYPE_CUSTOM,
        "case \"my-own-preset\": expected type {TYPE_CUSTOM}, got {}",
        created.body
    );
    let updated = emulator
        .update_preset("my-own-preset", json!({ "description": "mine" }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"my-own-preset\": expected HTTP 200 from UpdatePreset, got {} with body {}",
        updated.status, updated.body
    );
    let deleted = emulator.delete_preset("my-own-preset").await;
    assert_eq!(
        deleted.status, 202,
        "case \"my-own-preset\": expected HTTP 202 from DeletePreset, got {} with body {}",
        deleted.status, deleted.body
    );
}

/// A built-in preset is recognised by what the store HOLDS, never by the shape of its
/// name: a `System-`-looking name the catalogue does not hold is still the modelled
/// `NotFoundException`, and so is a near miss of one it does.
#[tokio::test]
async fn a_system_looking_name_the_catalogue_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let stored = PUBLISHED_NAMES[0];

    // The rows differ in how they miss: an invented `System-` name, a different case, a
    // truncation of a stored name, a stored name with one component changed, and a name
    // with no `System-` prefix at all.
    let misses = [
        "System-Invented_Hd_Mp4_Avc_Aac_16x9_1920x1080p_24Hz_6Mbps".to_string(),
        stored.to_lowercase(),
        stored[..stored.len() - 1].to_string(),
        stored.replace("1920x1080p", "1900x1000p"),
        "Generic_Hd_Mp4_Avc_Aac_16x9_1920x1080p_24Hz_6Mbps".to_string(),
    ];

    for name in misses {
        let reply = emulator.get_preset(&name).await;
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
        // A name the catalogue does not hold is free: a caller may create it, and what
        // they get is their OWN preset.
        let created = emulator
            .create_preset(json!({
                "name": name,
                "settings": { "containerSettings": { "container": "MP4" } }
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: a free name must be creatable, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("/preset/type"),
            TYPE_CUSTOM,
            "case {name:?}: a preset the caller created is {TYPE_CUSTOM}, got {}",
            created.body
        );
    }

    // And the stored catalogue is untouched by all of that.
    let reply = emulator.get_preset(stored).await;
    assert_matches_its_name(stored, &reply, REGION);
}

/// Reading a built-in preset reports: repeated reads answer byte-identical bodies, and a
/// job that names one in an output resolves and reads the name back.
#[tokio::test]
async fn reading_a_built_in_preset_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let built_ins = emulator.stored_names_of_type(TYPE_SYSTEM);
    assert!(
        !built_ins.is_empty(),
        "a fresh account must hold the service's built-in presets"
    );
    let name = built_ins
        .first()
        .cloned()
        .unwrap_or_else(|| PUBLISHED_NAMES[0].to_string());

    let baseline = emulator.get_preset(&name).await;
    for round in 1..=4 {
        let again = emulator.get_preset(&name).await;
        assert_eq!(
            again.body, baseline.body,
            "case round {round}: a read must not change what a later read sees, got {}",
            again.body
        );
    }
    assert_eq!(
        emulator.stored_names_of_type(TYPE_SYSTEM),
        built_ins,
        "the built-in catalogue must be untouched by reads, got {:?}",
        emulator.stored_names_of_type(TYPE_SYSTEM)
    );

    // A job built from a built-in preset — the normal starting point for a MediaConvert
    // client — resolves, and the output reads the preset name back.
    let job = emulator
        .send(
            "POST",
            "/2017-08-29/jobs",
            Some(json!({
                "role": "arn:aws:iam::123456789012:role/MediaConvertRole",
                "settings": {
                    "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }],
                    "outputGroups": [{
                        "name": "File Group",
                        "outputs": [{ "preset": name, "nameModifier": "_a" }],
                        "outputGroupSettings": {
                            "type": "FILE_GROUP_SETTINGS",
                            "fileGroupSettings": { "destination": "s3://example-bucket/out/" }
                        }
                    }]
                }
            })),
        )
        .await;
    assert_eq!(
        job.status, 201,
        "case job from {name:?}: expected HTTP 201, got {} with body {}",
        job.status, job.body
    );
    let id = job.string("/job/id");
    let fetched = emulator
        .send("GET", &format!("/2017-08-29/jobs/{id}"), None)
        .await;
    assert_eq!(
        fetched.string("/job/settings/outputGroups/0/outputs/0/preset"),
        name,
        "case job from {name:?}: expected the built-in preset named in the output, got {}",
        fetched.body
    );
}
