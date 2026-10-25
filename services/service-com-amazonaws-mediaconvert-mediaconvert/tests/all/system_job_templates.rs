// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the account's BUILT-IN (`SYSTEM`) job templates.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The rule every case below is one row of: a job template is "a pre-made set of encoding
//! instructions that you can use to quickly create a job", so a template the account holds
//! without creating it reports the encoding configuration its published name states — one
//! output group of the package the name names, with an output stating the container it is
//! written in and the codecs it is encoded with — through `ListJobTemplates(listBy=SYSTEM)`
//! and `GetJobTemplate` alike, and a job created from it inherits that group.
//!
//! The catalogue is read out of the STORE, so no case here hard-codes it; what each case
//! does hard-code is the independent reading of a template's NAME ([`NameSpec::of`]), which
//! is where the expectation for its settings comes from.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    job_template_settings_problem, MAX_JOB_TEMPLATE_INPUTS, TYPE_CUSTOM, TYPE_SYSTEM,
};
use service_com_amazonaws_mediaconvert_mediaconvert::{
    ComAmazonawsMediaconvertMediaconvertState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for unless it names another.
const REGION: &str = "us-west-2";

/// The IAM role every job this suite submits is created with (`Job$Role`).
const ROLE: &str = "arn:aws:iam::123456789012:role/MediaConvertRole";

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
    /// The JSON body, or `Value::Null` for an empty one.
    body: Value,
}

impl Reply {
    /// A member of the body, or `Value::Null` when it is absent.
    fn at(&self, pointer: &str) -> &Value {
        at(&self.body, pointer)
    }

    /// A string member of the body.
    fn string(&self, pointer: &str) -> String {
        string(&self.body, pointer)
    }
}

/// A member of a JSON value, or `Value::Null` when it is absent.
fn at<'a>(view: &'a Value, pointer: &str) -> &'a Value {
    view.pointer(pointer).unwrap_or(&Value::Null)
}

/// A string member of a JSON value, or the empty string when it is absent.
fn string(view: &Value, pointer: &str) -> String {
    at(view, pointer).as_str().unwrap_or_default().to_string()
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
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    async fn get_job_template(&self, name: &str) -> Reply {
        self.send("GET", &format!("/2017-08-29/jobTemplates/{name}"), None)
            .await
    }

    async fn list_job_templates(&self, query: &str) -> Reply {
        self.send("GET", &format!("/2017-08-29/jobTemplates?{query}"), None)
            .await
    }

    /// The names of the stored templates of one `Type`, in store order — read from the
    /// store so no case hard-codes the built-in catalogue.
    fn stored_names_of_type(&self, template_type: &str) -> Vec<String> {
        self.engine
            .state::<ComAmazonawsMediaconvertMediaconvertState>()
            .lock()
            .job_templates
            .values()
            .filter(|record| record.template_type == template_type)
            .map(|record| record.name.clone())
            .collect()
    }
}

/// `arn:aws:mediaconvert:<region>:<account>:<resource>`, built the way the emulator's own
/// ARN helper builds it, so the expectation is not a second hand-written format.
fn arn_in(region: &str, resource: &str) -> String {
    engine::emulator_arn("mediaconvert", region, resource)
}

/// Published names of the service's built-in job templates that an account must serve.
///
/// Source: the AWS Elemental MediaConvert system job-template catalogue (the console's
/// "System templates" list, the `System-*` names). The first row is a name the issue cited;
/// the others are entries it did not, spanning the catalogue's other output packages — an
/// HLS (MPEG-2 TS) package, a DASH (fragmented MP4) package, a CMAF package and a
/// single-file HEVC output.
const PUBLISHED_NAMES: [&str; 5] = [
    "System-Avc_16x9_1080p_29_97fps_8500kbps",
    "System-Ott_Hls_Ts_Avc_Aac",
    "System-Ott_Dash_Mp4_Avc_Aac",
    "System-Ott_Cmaf_Cmfc_Avc_Aac",
    "System-Hevc_16x9_2160p_29_97fps_20000kbps",
];

/// The frame size and encode rate a built-in template's name states, when it states them.
#[derive(Debug, Clone, Copy)]
struct Rendition {
    /// The `<N>p` line count, and the width the stated display aspect ratio puts with it.
    frame_size: (i64, i64),
    /// Frames per second, from the name's `<A>_<B>fps` token pair.
    framerate: f64,
    /// Bits per second, from the name's `<N>kbps` token.
    video_bitrate: i64,
}

/// What a built-in job template's published NAME states about the output it produces.
///
/// A system job template is named after that output — the output package, the video and
/// audio codecs and, where the name carries them, the display aspect ratio, line count,
/// frame rate and video bitrate — so this suite reads those parameters back OUT of the name
/// and asserts the served `Settings` graph agrees with them. The expectation is therefore
/// independent of the table the emulator seeds from: a settings graph that did not come from
/// the name fails here.
#[derive(Debug)]
struct NameSpec {
    /// The `OutputGroupType` value the name's package token states.
    group_type: &'static str,
    /// The `OutputGroupSettings` member that type's own settings object lives under.
    group_settings_member: &'static str,
    /// The `ContainerType` value the package's outputs are written in.
    container: &'static str,
    /// The `VideoCodec` value the name's codec token states.
    video_codec: &'static str,
    /// The rendition the name states, or `None` for a name that states none.
    rendition: Option<Rendition>,
}

impl NameSpec {
    fn of(name: &str) -> Self {
        // The package token: an HLS package's segments are MPEG-2 transport streams (the
        // `M3U8` container), a DASH package's are fragmented MP4 (`MPD`), a CMAF package's
        // are the fragmented MP4 of `CMFC`, a Smooth Streaming package's are `ISMV`, and a
        // name that states no streaming package at all is a single standalone file in
        // `MP4` — the File output group, which "generate[s] a single standalone file,
        // rather than a streaming package".
        let (group_type, group_settings_member, container) = if name.contains("_Hls_Ts_") {
            ("HLS_GROUP_SETTINGS", "hlsGroupSettings", "M3U8")
        } else if name.contains("_Dash_Mp4_") {
            ("DASH_ISO_GROUP_SETTINGS", "dashIsoGroupSettings", "MPD")
        } else if name.contains("_Cmaf_Cmfc_") {
            ("CMAF_GROUP_SETTINGS", "cmafGroupSettings", "CMFC")
        } else if name.contains("_Smooth_Ismv_") {
            ("MS_SMOOTH_GROUP_SETTINGS", "msSmoothGroupSettings", "ISMV")
        } else {
            ("FILE_GROUP_SETTINGS", "fileGroupSettings", "MP4")
        };
        let video_codec = if name.contains("Hevc") {
            "H_265"
        } else if name.contains("Avc") {
            "H_264"
        } else {
            panic!("case {name:?}: the name states no video codec this suite knows");
        };

        let tokens: Vec<&str> = name.split('_').collect();
        let mut aspect = None;
        let mut lines = None;
        let mut framerate = None;
        let mut video_bitrate = None;
        for (index, token) in tokens.iter().enumerate() {
            if let Some(ratio) = dimensions(token) {
                aspect = Some(ratio);
            } else if let Some(count) = token
                .strip_suffix('p')
                .and_then(|count| count.parse::<i64>().ok())
            {
                lines = Some(count);
            } else if let Some(fraction) = token.strip_suffix("fps") {
                // `29_97fps` is 29.97 frames per second: the token before the one ending
                // `fps` carries the whole part, this one the fractional part.
                let whole = index
                    .checked_sub(1)
                    .and_then(|before| tokens.get(before))
                    .and_then(|before| before.parse::<i64>().ok());
                framerate = match whole {
                    Some(whole) => format!("{whole}.{fraction}").parse::<f64>().ok(),
                    None => fraction.parse::<f64>().ok(),
                };
            } else if let Some(kbps) = token
                .strip_suffix("kbps")
                .and_then(|kbps| kbps.parse::<i64>().ok())
            {
                video_bitrate = Some(kbps * 1_000);
            }
        }

        // A name states a rendition only when it states ALL of it; the streaming packages'
        // names state none of it, and leave the output's resolution and frame rate to the
        // input.
        let rendition = match (aspect, lines, framerate, video_bitrate) {
            (Some((aspect_h, aspect_v)), Some(lines), Some(framerate), Some(video_bitrate)) => {
                assert!(
                    aspect_v > 0 && lines * aspect_h % aspect_v == 0,
                    "case {name:?}: {lines} lines is not a whole {aspect_h}x{aspect_v} frame"
                );
                Some(Rendition {
                    frame_size: (lines * aspect_h / aspect_v, lines),
                    framerate,
                    video_bitrate,
                })
            }
            _ => None,
        };

        Self {
            group_type,
            group_settings_member,
            container,
            video_codec,
            rendition,
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

/// Assert that one served `JobTemplate` is the built-in template its name states.
fn assert_matches_its_name(case: &str, name: &str, view: &Value, region: &str) {
    let spec = NameSpec::of(name);
    // `JobTemplate$Type`: "A job template can be of two types: system or custom. System or
    // built-in job templates can't be modified or deleted by the user."
    assert_eq!(
        string(view, "/type"),
        TYPE_SYSTEM,
        "case {case:?}: expected type {TYPE_SYSTEM}, got {view}"
    );
    assert_eq!(
        string(view, "/name"),
        name,
        "case {case:?}: expected the name read back verbatim, got {view}"
    );
    assert_eq!(
        string(view, "/arn"),
        arn_in(region, &format!("jobTemplates/{name}")),
        "case {case:?}: expected the job template ARN in the reading region, got {view}"
    );
    // `Category`/`Description` are the optional ones a caller creates for their own
    // templates, so a template nobody created carries neither.
    for member in ["category", "description"] {
        assert_eq!(
            at(view, &format!("/{member}")),
            &Value::Null,
            "case {case:?}: a template nobody created carries no {member}, got {view}"
        );
    }
    let created_at = at(view, "/createdAt").as_i64();
    assert!(
        created_at.is_some_and(|seconds| seconds > 1_600_000_000),
        "case {case:?}: expected createdAt in epoch seconds, got {view}"
    );
    assert_eq!(
        created_at,
        at(view, "/lastUpdated").as_i64(),
        "case {case:?}: a template nothing updated has lastUpdated == createdAt, got {view}"
    );

    // "There can only be one input in a job template": the one input a job created from it
    // inherits, carrying no source location of its own — `InputTemplate` models no
    // `FileInput` — so the job states where its content is read from.
    let inputs = at(view, "/settings/inputs")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        inputs.len(),
        MAX_JOB_TEMPLATE_INPUTS,
        "case {case:?}: expected {MAX_JOB_TEMPLATE_INPUTS} input in the template, got {view}"
    );
    assert_eq!(
        at(view, "/settings/inputs/0/fileInput"),
        &Value::Null,
        "case {case:?}: a template states no input location, got {view}"
    );

    // The encoding configuration: one output group of the package the name states.
    let groups = at(view, "/settings/outputGroups")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: expected one output group, got {view}"
    );
    assert_eq!(
        string(view, "/settings/outputGroups/0/outputGroupSettings/type"),
        spec.group_type,
        "case {case:?}: expected output group type {:?}, got {view}",
        spec.group_type
    );
    assert!(
        at(
            view,
            &format!(
                "/settings/outputGroups/0/outputGroupSettings/{}",
                spec.group_settings_member
            )
        )
        .is_object(),
        "case {case:?}: expected the {} object the type pairs with, got {view}",
        spec.group_settings_member
    );
    assert!(
        !string(view, "/settings/outputGroups/0/name").is_empty(),
        "case {case:?}: expected the output group to be named, got {view}"
    );

    let outputs = at(view, "/settings/outputGroups/0/outputs")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        outputs.len(),
        1,
        "case {case:?}: expected one output in the group, got {view}"
    );
    assert_eq!(
        string(
            view,
            "/settings/outputGroups/0/outputs/0/containerSettings/container"
        ),
        spec.container,
        "case {case:?}: expected container {:?}, got {view}",
        spec.container
    );

    let video = "/settings/outputGroups/0/outputs/0/videoDescription";
    assert_eq!(
        string(view, &format!("{video}/codecSettings/codec")),
        spec.video_codec,
        "case {case:?}: expected video codec {:?}, got {view}",
        spec.video_codec
    );
    let codec = format!("{video}/codecSettings/{}", spec.codec_settings_member());
    assert!(
        at(view, &codec).is_object(),
        "case {case:?}: expected the {} object the codec pairs with, got {view}",
        spec.codec_settings_member()
    );

    match spec.rendition {
        Some(rendition) => {
            let (width, height) = rendition.frame_size;
            assert_eq!(
                at(view, &format!("{video}/width")).as_i64(),
                Some(width),
                "case {case:?}: expected width {width}, got {view}"
            );
            assert_eq!(
                at(view, &format!("{video}/height")).as_i64(),
                Some(height),
                "case {case:?}: expected height {height}, got {view}"
            );
            assert_eq!(
                at(view, &format!("{codec}/bitrate")).as_i64(),
                Some(rendition.video_bitrate),
                "case {case:?}: expected {} bits per second under {}, got {view}",
                rendition.video_bitrate,
                spec.codec_settings_member()
            );
            // "specify the frame rate as a fraction": the fraction reduces to the stated
            // rate, which for 29.97 fps is exactly 30000/1001 rather than a rounded 30.
            let numerator = at(view, &format!("{codec}/framerateNumerator"))
                .as_i64()
                .unwrap_or_default();
            let denominator = at(view, &format!("{codec}/framerateDenominator"))
                .as_i64()
                .unwrap_or_default();
            assert!(
                denominator > 0
                    && (numerator as f64 / denominator as f64 - rendition.framerate).abs() < 0.01,
                "case {case:?}: expected {} fps as a fraction, got {numerator}/{denominator}",
                rendition.framerate
            );
        }
        None => {
            // A name that states no frame size states none in the graph either: the output
            // takes its resolution and frame rate from the input.
            for member in ["width", "height"] {
                assert_eq!(
                    at(view, &format!("{video}/{member}")),
                    &Value::Null,
                    "case {case:?}: a name stating no frame size states no {member}, got {view}"
                );
            }
        }
    }

    // The name states one AAC audio track, so there is exactly one audio description and it
    // carries AAC settings with a bitrate inside the member's published range.
    let audio = at(view, "/settings/outputGroups/0/outputs/0/audioDescriptions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        audio.len(),
        1,
        "case {case:?}: expected one audio description, got {view}"
    );
    let audio_codec = "/settings/outputGroups/0/outputs/0/audioDescriptions/0/codecSettings";
    assert_eq!(
        string(view, &format!("{audio_codec}/codec")),
        "AAC",
        "case {case:?}: expected audio codec AAC, got {view}"
    );
    assert!(
        at(view, &format!("{audio_codec}/aacSettings/bitrate"))
            .as_i64()
            // `AacSettings$Bitrate`'s published range is 6000..1024000.
            .is_some_and(|bitrate| (6_000..=1_024_000).contains(&bitrate)),
        "case {case:?}: expected an AAC bitrate inside the published range, got {view}"
    );
}

/// A fresh account serves every built-in job template, through both readers: the published
/// names resolve, each one is `SYSTEM`, and each one's settings are the configuration its
/// name states.
///
/// The table is the whole seeded catalogue, read from the store — its rows differ in output
/// package, video codec, and whether the name states a frame size, frame rate and bitrate at
/// all — plus the published names a caller starts from, asserted by name.
#[tokio::test]
async fn a_fresh_account_serves_every_built_in_job_template() {
    let emulator = Emulator::new();

    // Nothing was created, so every template in the store is a built-in one.
    let built_ins = emulator.stored_names_of_type(TYPE_SYSTEM);
    assert!(
        built_ins.len() >= PUBLISHED_NAMES.len(),
        "a fresh account must hold the service's built-in job templates, got {built_ins:?}"
    );
    assert_eq!(
        emulator.stored_names_of_type(TYPE_CUSTOM),
        Vec::<String>::new(),
        "a fresh account has created no template of its own, got {:?}",
        emulator.stored_names_of_type(TYPE_CUSTOM)
    );

    for name in &built_ins {
        let reply = emulator.get_job_template(name).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected HTTP 200 from GetJobTemplate, got {} with body {}",
            reply.status, reply.body
        );
        assert_matches_its_name(name, name, reply.at("/jobTemplate"), REGION);
    }

    for name in PUBLISHED_NAMES {
        assert!(
            built_ins.iter().any(|stored| stored == name),
            "case {name:?}: a published built-in template must be in the store, got {built_ins:?}"
        );
        let reply = emulator.get_job_template(name).await;
        assert_matches_its_name(name, name, reply.at("/jobTemplate"), REGION);
    }

    // The listing a client discovers the catalogue through reports the same graph, for every
    // entry, on one page.
    let listed = emulator.list_job_templates("listBy=SYSTEM").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListJobTemplates, got {} with body {}",
        listed.status, listed.body
    );
    let items = listed
        .at("/jobTemplates")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        built_ins.len(),
        "the SYSTEM listing must report every built-in template, got {}",
        listed.body
    );
    for item in &items {
        let name = string(item, "/name");
        assert_matches_its_name(&format!("listed {name}"), &name, item, REGION);
    }

    // A built-in template's ARN names the region of the request that reads it, as every
    // other resource's does; the settings do not vary with the region.
    let name = PUBLISHED_NAMES[0];
    for region in ["eu-central-1", "ap-southeast-2"] {
        let reply = emulator
            .send_in(
                region,
                "GET",
                &format!("/2017-08-29/jobTemplates/{name}"),
                None,
            )
            .await;
        assert_matches_its_name(
            &format!("{name} in {region}"),
            name,
            reply.at("/jobTemplate"),
            region,
        );
    }
}

/// Every built-in template's settings graph is one a job can be created from — the SAME test
/// a template a caller stores has to pass, so the catalogue cannot hold a graph the write
/// path would refuse.
#[tokio::test]
async fn every_built_in_template_holds_a_graph_a_job_can_be_created_from() {
    let emulator = Emulator::new();
    let records = emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_templates
        .values()
        .filter(|record| record.template_type == TYPE_SYSTEM)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        !records.is_empty(),
        "a fresh account must hold the service's built-in job templates"
    );

    for record in &records {
        assert_eq!(
            job_template_settings_problem(&record.settings),
            None,
            "case {:?}: a built-in template's graph must pass the test a caller's template \
             takes, got {:?}",
            record.name,
            job_template_settings_problem(&record.settings)
        );
    }
}

/// A job created from a built-in template inherits that template's output group: the caller
/// states only where the content comes from, and the stored job reports the encoding
/// configuration the catalogue entry carries.
///
/// The table is the whole built-in catalogue, so it spans every output package and both
/// video codecs.
#[tokio::test]
async fn a_job_created_from_a_built_in_template_inherits_its_output_group() {
    let emulator = Emulator::new();
    let built_ins = emulator.stored_names_of_type(TYPE_SYSTEM);
    assert!(
        !built_ins.is_empty(),
        "a fresh account must hold the service's built-in job templates"
    );

    for (index, name) in built_ins.iter().enumerate() {
        let template = emulator.get_job_template(name).await;
        let expected_groups = template.at("/jobTemplate/settings/outputGroups").clone();

        let input = format!("s3://example-bucket/in/{index}.mp4");
        let created = emulator
            .send(
                "POST",
                "/2017-08-29/jobs",
                Some(json!({
                    "role": ROLE,
                    "jobTemplate": name,
                    "settings": { "inputs": [{ "fileInput": &input }] }
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected HTTP 201 from CreateJob, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("/job/jobTemplate"),
            name.as_str(),
            "case {name:?}: expected the job to name the template it was created from, got {}",
            created.body
        );

        let id = created.string("/job/id");
        let fetched = emulator
            .send("GET", &format!("/2017-08-29/jobs/{id}"), None)
            .await;
        assert_eq!(
            fetched.at("/job/settings/outputGroups"),
            &expected_groups,
            "case {name:?}: expected the template's output groups on the job, got {}",
            fetched.body
        );
        // What the request DID state is the job's own: the input it names.
        assert_eq!(
            fetched.string("/job/settings/inputs/0/fileInput"),
            input,
            "case {name:?}: expected the job's own input location, got {}",
            fetched.body
        );
    }
}

/// Reading a built-in template reports: repeated reads of both readers answer byte-identical
/// bodies, and the catalogue is unchanged by them.
#[tokio::test]
async fn reading_a_built_in_template_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let built_ins = emulator.stored_names_of_type(TYPE_SYSTEM);
    let name = built_ins
        .first()
        .cloned()
        .unwrap_or_else(|| PUBLISHED_NAMES[0].to_string());

    let baseline = emulator.get_job_template(&name).await;
    let baseline_listing = emulator.list_job_templates("listBy=SYSTEM").await;
    for round in 1..=4 {
        let again = emulator.get_job_template(&name).await;
        assert_eq!(
            again.body, baseline.body,
            "case round {round}: a read must not change what a later read sees, got {}",
            again.body
        );
        let listed = emulator.list_job_templates("listBy=SYSTEM").await;
        assert_eq!(
            listed.body, baseline_listing.body,
            "case round {round}: a listing must not change what a later listing sees, got {}",
            listed.body
        );
    }
    assert_eq!(
        emulator.stored_names_of_type(TYPE_SYSTEM),
        built_ins,
        "the built-in catalogue must be untouched by reads, got {:?}",
        emulator.stored_names_of_type(TYPE_SYSTEM)
    );
}
