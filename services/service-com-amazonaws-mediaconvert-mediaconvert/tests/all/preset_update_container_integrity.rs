// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the container a STORED preset must keep naming, on the
//! `UpdatePreset` door and across every preset the store holds.
//!
//! The rule every case below is one row of: a preset is the encoder settings of one output,
//! so the `PresetSettings` a request would leave a stored preset holding has to name that
//! output's container with a value that says something (`ContainerSettings.Container`) — a
//! request that would leave it unnamed is `BadRequestException` (400) that changes nothing
//! about the stored preset, and a request that names any container the model publishes is
//! stored and reads back. No preset the store holds, built-in or created, is therefore
//! without a container, which is what lets an output take its container from the preset it
//! names (`Output$Preset`: "You can specify either Preset or Container settings, but not
//! both").
//!
//! The members and their documented rules come from the model:
//! `smithy_query(query="shape",shape="PresetSettings,ContainerSettings,ContainerType",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! — `PresetSettings` is "Settings for preset", `ContainerSettings$Container` is "Container
//! for this output", and the other `PresetSettings` members configure the tracks written
//! into that container.
//!
//! This suite is the axes `tests/all/preset_settings_validation.rs` does not walk: a
//! container member that is PRESENT but says nothing, the whole published `ContainerType`
//! catalogue through the update door, the store-wide invariant read back through
//! `ListPresets`, and the downstream job that takes its container from a preset a refused
//! update tried to empty.
//!
//! Every case drives the real restJson1 wire path through the shared harness in
//! `job_lifecycle`, so what is asserted is the bytes a client would see, never an internal
//! call.

use serde_json::{json, Value};

use super::job_lifecycle::{Emulator, Reply, ROLE};

/// Every value of the model's `ContainerType` enum
/// (`smithy_query(query="shape",shape="ContainerType")`), which is where this catalogue
/// comes from: a preset may be stored for any of them, including the ones the issue did not
/// cite (F4V, GIF, ISMV, M2TS, CMFC, MOV, MPD, WEBM, RAW, Y4M).
const CONTAINERS: [&str; 14] = [
    "F4V", "GIF", "ISMV", "M2TS", "M3U8", "CMFC", "MOV", "MP4", "MPD", "MXF", "OGG", "WEBM",
    "RAW", "Y4M",
];

/// The settings member a refusal has to name, lower-cased so one expectation covers both
/// spellings the two refusal layers use: the handler names the modelled member
/// (`ContainerSettings.Container`), while a value the published enum does not carry is
/// refused at the boundary, which names the wire path (`settings.containerSettings.container`).
const CONTAINER_MEMBER: &str = "containersettings.container";

/// `POST /2017-08-29/presets`.
async fn create_preset(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("POST", "/2017-08-29/presets", Some(body))
        .await
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

/// The preset a row starts from: a graph that does name its output's container, plus the
/// free-text members an assertion can see a refused update did not touch.
fn fixture(name: &str) -> Value {
    json!({
        "name": name,
        "category": "web",
        "description": "the original",
        "settings": {
            "containerSettings": { "container": "MP4" },
            "videoDescription": { "width": 1280, "height": 720 }
        }
    })
}

/// Create the fixture preset and return the `preset` view the create reported.
async fn created_fixture(emulator: &Emulator, case: &str, name: &str) -> Value {
    let created = create_preset(emulator, fixture(name)).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );
    created.at("/preset").clone()
}

/// Every preset of the kind `list_by` selects, read through `ListPresets` and following the
/// `nextToken` the listing hands back, so the invariant case reads the whole store rather
/// than one page of it.
async fn all_presets(emulator: &Emulator, list_by: &str) -> Vec<Value> {
    let mut presets = Vec::new();
    let mut token = String::new();
    loop {
        let query = if token.is_empty() {
            format!("listBy={list_by}")
        } else {
            format!("listBy={list_by}&nextToken={token}")
        };
        let listed = emulator
            .send("GET", &format!("/2017-08-29/presets?{query}"), None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {query:?}: ListPresets must answer HTTP 200, got {} with body {}",
            listed.status, listed.body
        );
        presets.extend(
            listed
                .at("/presets")
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = listed.string("/nextToken");
        if token.is_empty() {
            break;
        }
    }
    presets
}

/// Assert a reply is the modelled refusal naming the settings member at fault.
///
/// Both layers that can refuse such a request answer the same status and code — the
/// boundary, when the container member carries a value the enum does not publish, and the
/// handler, when nothing names the container at all — so a row does not have to say which
/// one spoke, only that the request was refused and said which member was at fault.
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
        message.to_lowercase().contains(CONTAINER_MEMBER),
        "case {case:?}: the refusal must name the settings member at fault \
         ({CONTAINER_MEMBER}), got message {message:?}"
    );
}

/// The container value of a preset view, as a listing or a read reports it.
fn container_of(preset: &Value) -> &Value {
    preset
        .pointer("/settings/containerSettings/container")
        .unwrap_or(&Value::Null)
}

/// One row of the refusal table: a `PresetSettings` graph that leaves the output's container
/// unnamed because the member that names it says nothing.
struct Refused {
    case: &'static str,
    settings: Value,
}

/// The graphs whose container member is PRESENT but silent, plus one that drops the member
/// while carrying every other one.
///
/// The rows differ along what the rule generalises over: the FORM of the silent value (an
/// empty string, spaces, other whitespace), whether the container settings configure a
/// container beside the silent value, and whether the graph names the member at all.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "an empty container value",
            settings: json!({ "containerSettings": { "container": "" } }),
        },
        Refused {
            case: "a container value of spaces",
            settings: json!({ "containerSettings": { "container": "   " } }),
        },
        Refused {
            case: "a container value of other whitespace",
            settings: json!({ "containerSettings": { "container": "\t\n" } }),
        },
        Refused {
            case: "a silent container value beside settings that configure one",
            settings: json!({
                "containerSettings": {
                    "container": " ",
                    "mp4Settings": { "cttsVersion": 1, "mp4MajorBrand": "mp42" }
                },
                "videoDescription": { "width": 3840, "height": 2160 }
            }),
        },
        Refused {
            case: "every member but the container settings, with two audio tracks",
            settings: json!({
                "videoDescription": { "width": 640, "height": 360 },
                "audioDescriptions": [
                    { "audioSourceName": "Audio Selector 1", "languageCode": "ENG" },
                    { "audioSourceName": "Audio Selector 2", "languageCode": "DEU" }
                ],
                "captionDescriptions": [{ "languageCode": "FRA" }]
            }),
        },
    ]
}

/// An update whose settings would leave the output's container unnamed is refused, and the
/// stored preset keeps every member it had — its settings, its free text, and the
/// `LastUpdated` the issue reports moving.
#[tokio::test]
async fn an_update_that_leaves_the_container_unnamed_is_refused_and_moves_nothing() {
    let emulator = Emulator::new();

    for (index, row) in refused_rows().iter().enumerate() {
        let Refused { case, settings } = row;
        let name = format!("kept-{index}");
        let before = created_fixture(&emulator, case, &name).await;

        // The update carries members that are fine on their own beside the bad graph, so
        // the assertions below show the whole request was refused, not half applied.
        let updated = update_preset(
            &emulator,
            &name,
            json!({
                "category": "archive",
                "description": "the replacement",
                "settings": settings
            }),
        )
        .await;
        assert_refusal(case, &updated);

        let fetched = get_preset(&emulator, &name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the preset must still be readable, got {} with body {}",
            fetched.status, fetched.body
        );
        let after = fetched.at("/preset");
        assert_eq!(
            after, &before,
            "case {case:?}: a refused update must leave the stored preset as it was \
             ({before}), got {after}"
        );
        // Spelled out as well, because these are the three the report names: the graph, the
        // free text the request also carried, and the update timestamp.
        assert_eq!(
            container_of(after),
            &json!("MP4"),
            "case {case:?}: the stored graph must still name its container, got {after}"
        );
        assert_eq!(
            fetched.string("/preset/description"),
            "the original",
            "case {case:?}: a refused update must not apply the members beside Settings, \
             got {after}"
        );
        assert_eq!(
            after.pointer("/lastUpdated"),
            before.pointer("/lastUpdated"),
            "case {case:?}: a refused update must not move LastUpdated, got {after}"
        );
    }
}

/// Any container the model publishes can be stored through the update door, and reads back
/// as sent — the positive control of the same rule, over the whole `ContainerType`
/// catalogue rather than the one value the report cited.
#[tokio::test]
async fn every_published_container_is_stored_through_the_update_door() {
    let emulator = Emulator::new();

    for (index, container) in CONTAINERS.iter().enumerate() {
        let case = format!("container {container}");
        let name = format!("replaced-{index}");
        let before = created_fixture(&emulator, &case, &name).await;

        let settings = json!({
            "containerSettings": { "container": container },
            "audioDescriptions": [{ "audioSourceName": "Audio Selector 1" }]
        });
        let updated = update_preset(&emulator, &name, json!({ "settings": settings })).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} with body {}",
            updated.status, updated.body
        );

        let fetched = get_preset(&emulator, &name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetPreset HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/preset/settings"),
            &settings,
            "case {case:?}: GetPreset must report the graph the update stored, got {}",
            fetched.at("/preset/settings")
        );
        assert_eq!(
            fetched.at("/preset/arn"),
            before.pointer("/arn").unwrap_or(&Value::Null),
            "case {case:?}: an accepted update must address the same preset, got {}",
            fetched.body
        );
    }
}

/// No preset this account can read describes no output: the built-in ones it is seeded with
/// and the ones a caller creates all name their output's container, and an update cannot
/// take one away.
#[tokio::test]
async fn no_preset_the_store_holds_leaves_its_container_unnamed() {
    let emulator = Emulator::new();

    // A couple of created presets beside the built-in ones, so the listing spans both
    // `Type` values, and one refused attempt to empty each of them.
    for (index, container) in ["MP4", "WEBM"].iter().enumerate() {
        let name = format!("own-{index}");
        let created = create_preset(
            &emulator,
            json!({
                "name": &name,
                "settings": { "containerSettings": { "container": container } }
            }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: the preset must be created, got {} with body {}",
            created.status, created.body
        );
        let emptied = update_preset(&emulator, &name, json!({ "settings": {} })).await;
        assert_refusal(&name, &emptied);
    }

    for list_by in ["SYSTEM", "NAME"] {
        let presets = all_presets(&emulator, list_by).await;
        assert!(
            !presets.is_empty(),
            "case {list_by:?}: the listing must report at least one preset, got none"
        );
        for preset in &presets {
            let name = preset
                .pointer("/name")
                .and_then(Value::as_str)
                .unwrap_or("<unnamed>")
                .to_string();
            let container = container_of(preset);
            let named = container
                .as_str()
                .is_some_and(|value| !value.trim().is_empty());
            assert!(
                named,
                "case {list_by:?}/{name:?}: every stored preset must name its output's \
                 container, got {container} in {preset}"
            );
        }
    }
}

/// A job whose output takes its container from a preset still runs after an update tried to
/// empty that preset: the preset the output names is the one the create stored.
#[tokio::test]
async fn an_output_that_names_a_preset_still_has_a_container_after_a_refused_update() {
    let emulator = Emulator::new();
    let name = "container-from-here";
    let created = create_preset(
        &emulator,
        json!({
            "name": name,
            "settings": { "containerSettings": { "container": "M3U8" } }
        }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );

    // The rows are the two forms the report's step 3 takes: an absent graph member set and
    // one that carries only tracks.
    for (case, settings) in [
        ("an empty graph", json!({})),
        (
            "a graph of tracks alone",
            json!({ "videoDescription": { "width": 1920, "height": 1080 } }),
        ),
    ] {
        let emptied = update_preset(&emulator, name, json!({ "settings": settings })).await;
        assert_refusal(case, &emptied);
    }

    let fetched = get_preset(&emulator, name).await;
    assert_eq!(
        container_of(fetched.at("/preset")),
        &json!("M3U8"),
        "the preset must still name the container it was created with, got {}",
        fetched.body
    );

    let job = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": {
                "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }],
                "outputGroups": [{
                    "name": "File Group",
                    "outputs": [{ "nameModifier": "_1", "preset": name }],
                    "outputGroupSettings": {
                        "type": "FILE_GROUP_SETTINGS",
                        "fileGroupSettings": { "destination": "s3://example-bucket/out/" }
                    }
                }]
            }
        }))
        .await;
    assert_eq!(
        job.status, 201,
        "a job whose output takes its container from the preset must be accepted, got {} \
         with body {}",
        job.status, job.body
    );
    assert_eq!(
        job.string("/job/settings/outputGroups/0/outputs/0/preset"),
        name,
        "the stored job must keep the preset its output names, got {}",
        job.body
    );
}
