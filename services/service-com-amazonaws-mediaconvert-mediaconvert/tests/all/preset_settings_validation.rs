// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the settings graph an output PRESET may be left holding, on
//! both doors into a stored preset: `CreatePreset` and `UpdatePreset`.
//!
//! The rule every case below is one row of: the `PresetSettings` a request asks the service
//! to save must describe the output it is the encoder settings of, which means it must name
//! that output's container (`ContainerSettings.Container`) — or the request is refused with
//! `BadRequestException` (400), naming the settings member at fault, and the store is left
//! exactly as it was. The same test is taken on the job and job-template planes, where an
//! output states its container either in its own container settings or through the preset
//! that states it (`tests/all/job_template_settings_validation.rs`).
//!
//! The members and their documented rules come from the model:
//! `smithy_query(query="shape",shape="PresetSettings,ContainerSettings,ContainerType",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! — `PresetSettings` is "Settings for preset", `ContainerSettings$Container` is "Container
//! for this output", and the other members of `PresetSettings` configure the tracks written
//! into that container ("include one instance of VideoDescription per output").
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see, never an internal call. The rest of the
//! preset write path is `tests/all/preset_lifecycle.rs` and `tests/all/preset_update.rs`.

use serde_json::{json, Value};

use super::job_lifecycle::{Emulator, Reply};

/// `ContainerType` values from the model's enum
/// (`smithy_query(query="shape",shape="ContainerType")`: F4V, GIF, ISMV, M2TS, M3U8, CMFC,
/// MOV, MP4, MPD, MXF, OGG, WEBM, RAW, Y4M). `OGG` and `Y4M` are not values the issue cited.
const CONTAINERS: [&str; 5] = ["MP4", "M3U8", "MXF", "OGG", "Y4M"];

/// The settings member a refusal has to name, so the expectation lives in one place.
const CONTAINER_MEMBER: &str = "ContainerSettings.Container";

/// `POST /2017-08-29/presets`.
async fn create_preset(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("POST", "/2017-08-29/presets", Some(body)).await
}

/// `GET /2017-08-29/presets/{Name}`.
async fn get_preset(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("GET", &format!("/2017-08-29/presets/{name}"), None)
        .await
}

/// `PUT /2017-08-29/presets/{Name}`.
async fn update_preset(emulator: &Emulator, name: &str, body: Value) -> Reply {
    emulator
        .send("PUT", &format!("/2017-08-29/presets/{name}"), Some(body))
        .await
}

/// The preset every update row is created with first: a graph that does describe an output,
/// plus a description an assertion can see is unchanged.
fn fixture(name: &str) -> Value {
    json!({
        "name": name,
        "description": "the original",
        "settings": {
            "containerSettings": { "container": CONTAINERS[0] },
            "videoDescription": { "width": 1280, "height": 720 }
        }
    })
}

/// How many of the account's OWN presets the store holds, read back through `ListPresets` —
/// the only way a case checks the store, never by reaching into the state struct.
async fn stored_presets(emulator: &Emulator) -> usize {
    let listed = emulator.send("GET", "/2017-08-29/presets", None).await;
    assert_eq!(
        listed.status, 200,
        "ListPresets must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed
        .at("/presets")
        .as_array()
        .map_or(usize::MAX, Vec::len)
}

/// Assert a reply is the modelled refusal naming the settings member at fault.
fn assert_refusal(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    let message = reply.string("/message");
    assert!(
        message.contains(CONTAINER_MEMBER),
        "case {case:?}: the refusal must name the settings member at fault \
         ({CONTAINER_MEMBER}), got message {message:?}"
    );
}

/// One row of the refusal table: a `PresetSettings` graph that names no container.
struct Refused {
    case: &'static str,
    settings: Value,
}

/// The graphs that describe no output, which both write paths are driven through.
///
/// The table spans what the rule generalises over: whether `ContainerSettings` is absent or
/// present-but-silent about the container, WHICH other members stand beside it (none, the
/// video description, the audio list, the captions, all three), how deep the graph is, and
/// how many entries its lists carry.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "settings with no member at all",
            settings: json!({}),
        },
        Refused {
            case: "container settings that configure a container without naming one",
            settings: json!({ "containerSettings": { "mp4Settings": { "cslgAtom": "INCLUDE" } } }),
        },
        Refused {
            case: "an empty container settings object",
            settings: json!({ "containerSettings": {} }),
        },
        Refused {
            case: "a video description and no container settings",
            settings: json!({
                "videoDescription": {
                    "width": 1920,
                    "height": 1080,
                    "codecSettings": { "codec": "H_264", "h264Settings": { "bitrate": 5_000_000 } }
                }
            }),
        },
        Refused {
            case: "two audio descriptions and no container settings",
            settings: json!({
                "audioDescriptions": [
                    { "audioSourceName": "Audio Selector 1", "languageCode": "ENG" },
                    { "audioSourceName": "Audio Selector 2", "languageCode": "SPA" }
                ]
            }),
        },
        Refused {
            case: "a caption description and no container settings",
            settings: json!({ "captionDescriptions": [{ "languageCode": "FRA" }] }),
        },
        Refused {
            case: "every other PresetSettings member at once, but no container",
            settings: json!({
                "videoDescription": { "width": 1280, "height": 720 },
                "audioDescriptions": [{ "audioSourceName": "Audio Selector 1" }],
                "captionDescriptions": [{ "languageCode": "DEU" }]
            }),
        },
        Refused {
            case: "a deep container settings object for several containers, none named",
            settings: json!({
                "containerSettings": {
                    "mp4Settings": { "cttsVersion": 1, "mp4MajorBrand": "mp42" },
                    "m3u8Settings": { "audioFramesPerPes": 4 },
                    "mpdSettings": { "captionContainerType": "RAW" }
                },
                "videoDescription": { "width": 640, "height": 360 }
            }),
        },
    ]
}

/// `CreatePreset` refuses a settings graph that describes no output, and stores nothing: the
/// name it named is still unreadable afterwards, and no preset appears in the listing.
#[tokio::test]
async fn creating_a_preset_from_a_graph_that_describes_no_output_is_a_bad_request() {
    let emulator = Emulator::new();
    let before = stored_presets(&emulator).await;
    let rows = refused_rows();

    for (index, row) in rows.iter().enumerate() {
        let Refused { case, settings } = row;
        let name = format!("rejected-{index}");

        let created = create_preset(&emulator, json!({ "name": &name, "settings": settings })).await;
        assert_refusal(case, &created);

        // Nothing was stored: the name the refused create addressed is still free, so a
        // read of it is the modelled 404.
        let fetched = get_preset(&emulator, &name).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a refused create must store nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }

    let after = stored_presets(&emulator).await;
    assert_eq!(
        after, before,
        "after {} refused creates the store must hold the presets it started with \
         ({before}), got {after}",
        rows.len()
    );
}

/// `UpdatePreset` refuses the same graphs, and leaves the stored preset exactly as it was —
/// every member, not only `Settings` — so a preset created usable cannot be emptied.
#[tokio::test]
async fn updating_a_preset_to_a_graph_that_describes_no_output_is_a_bad_request() {
    let emulator = Emulator::new();

    for (index, row) in refused_rows().iter().enumerate() {
        let Refused { case, settings } = row;
        let name = format!("kept-{index}");

        let created = create_preset(&emulator, fixture(&name)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the fixture preset must be created, got {} with body {}",
            created.status, created.body
        );

        // The update carries the bad graph AND a member that is fine on its own, so the
        // assertion below shows the whole request was refused rather than half applied.
        let updated = update_preset(
            &emulator,
            &name,
            json!({ "description": "the replacement", "settings": settings }),
        )
        .await;
        assert_refusal(case, &updated);

        let fetched = get_preset(&emulator, &name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the preset must still be readable, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/preset"),
            created.at("/preset"),
            "case {case:?}: a refused update must change nothing, got {} after {}",
            fetched.at("/preset"),
            created.at("/preset")
        );
    }
}

/// One row of the accepted table: a graph that DOES name its output's container.
struct Accepted {
    case: String,
    settings: Value,
}

/// One shape of accepted graph: a label, and how it builds the graph around `container`.
type Shape = (&'static str, fn(&str) -> Value);

/// The positive control of the rule, on both doors: a graph that names a container is stored
/// and reads back verbatim.
///
/// The rows span the container value (five of the model's enum) against the shapes the
/// refused rows vary: the container alone, the container beside each other `PresetSettings`
/// member, and the container inside a container settings object that configures it too.
fn accepted_rows() -> Vec<Accepted> {
    let shapes: [Shape; 4] = [
        ("the container alone", |container| {
            json!({ "containerSettings": { "container": container } })
        }),
        ("the container and a video description", |container| {
            json!({
                "containerSettings": { "container": container },
                "videoDescription": { "width": 1920, "height": 1080 }
            })
        }),
        (
            "the container, two audio descriptions and captions",
            |container| {
                json!({
                    "containerSettings": { "container": container },
                    "audioDescriptions": [
                        { "audioSourceName": "Audio Selector 1", "languageCode": "ENG" },
                        { "audioSourceName": "Audio Selector 2", "languageCode": "SPA" }
                    ],
                    "captionDescriptions": [{ "languageCode": "FRA" }]
                })
            },
        ),
        (
            "a container settings object that names and configures the container",
            |container| {
                json!({
                    "containerSettings": {
                        "container": container,
                        "mp4Settings": { "cttsVersion": 1, "mp4MajorBrand": "mp42" }
                    },
                    "videoDescription": { "width": 640, "height": 360 }
                })
            },
        ),
    ];
    let mut rows = Vec::new();
    for container in CONTAINERS {
        for (shape, build) in &shapes {
            rows.push(Accepted {
                case: format!("{container}: {shape}"),
                settings: build(container),
            });
        }
    }
    rows
}

/// A graph that names its output's container is accepted by `CreatePreset`, accepted by
/// `UpdatePreset`, and read back by `GetPreset` exactly as it was sent.
#[tokio::test]
async fn a_graph_that_names_its_container_is_stored_and_reads_back() {
    let emulator = Emulator::new();

    for (index, row) in accepted_rows().iter().enumerate() {
        let Accepted { case, settings } = row;
        let name = format!("accepted-{index}");

        let created = create_preset(&emulator, json!({ "name": &name, "settings": settings })).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from the create, got {} with body {}",
            created.status, created.body
        );

        // The same graph through the update door, onto a preset created with another one,
        // so the update is a real replacement.
        let target = format!("replaced-{index}");
        let fixture_reply = create_preset(&emulator, fixture(&target)).await;
        assert_eq!(
            fixture_reply.status, 201,
            "case {case:?}: the fixture preset must be created, got {} with body {}",
            fixture_reply.status, fixture_reply.body
        );
        let updated = update_preset(&emulator, &target, json!({ "settings": settings })).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} with body {}",
            updated.status, updated.body
        );

        for name in [&name, &target] {
            let fetched = get_preset(&emulator, name).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: expected {name} to read back, got {} with body {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.at("/preset/settings"),
                settings,
                "case {case:?}: expected the settings as sent under {name}, got {}",
                fetched.at("/preset/settings")
            );
        }
    }
}

/// An update that leaves `Settings` out does not have to restate the container: the stored
/// graph already names one, and the members the request does carry are applied.
#[tokio::test]
async fn an_update_that_sends_no_settings_keeps_the_stored_graph() {
    let emulator = Emulator::new();
    let created = create_preset(&emulator, fixture("untouched")).await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );

    // The rows differ in which members the update carries, none of them `Settings`.
    let rows = [
        ("a category alone", json!({ "category": "web" })),
        ("a description alone", json!({ "description": "revised" })),
        (
            "both free-text members",
            json!({ "category": "archive", "description": "revised again" }),
        ),
        ("no member at all", json!({})),
    ];
    for (case, request) in rows {
        let updated = update_preset(&emulator, "untouched", request).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.at("/preset/settings"),
            created.at("/preset/settings"),
            "case {case:?}: the stored graph must be kept, got {}",
            updated.at("/preset/settings")
        );
    }
}
