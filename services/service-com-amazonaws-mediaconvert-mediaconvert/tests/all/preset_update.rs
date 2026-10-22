// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdatePreset`.
//!
//! The rule under test: a member the request carries replaces the stored one, a member it
//! leaves out keeps the value an earlier call persisted, `Settings` is replaced as a
//! whole graph, `LastUpdated` moves while `CreatedAt` does not, and every later read of
//! the preset sees exactly what the update stored.
//!
//! Every case drives the real restJson1 wire path through the shared harness in
//! `job_lifecycle` — the router picks the service from the request's SigV4 scope, the
//! framework deserializes and validates, the handler runs, and the response is
//! serialized — so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::{arn_in, Emulator, Reply, REGION};

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

/// The settings every row starts from: a two-member `PresetSettings` graph, so a row that
/// replaces it can be told apart from one that merged into it.
fn original_settings() -> Value {
    json!({
        "containerSettings": { "container": "MP4" },
        "videoDescription": {
            "width": 1920,
            "height": 1080,
            "codecSettings": {
                "codec": "H_264",
                "h264Settings": { "bitrate": 5_000_000 }
            }
        }
    })
}

/// The preset every row is created with before it is updated.
fn original_preset(name: &str) -> Value {
    json!({
        "name": name,
        "category": "web",
        "description": "the original",
        "settings": original_settings(),
    })
}

/// One `UpdatePreset` request and the stored preset it must leave behind.
struct Case {
    label: &'static str,
    /// The request body, i.e. which optional members the caller sends.
    request: Value,
    /// The `category` the stored preset must carry afterwards, `None` for an absent one.
    category: Option<&'static str>,
    /// The `description` the stored preset must carry afterwards.
    description: Option<&'static str>,
    /// The whole `settings` graph the stored preset must carry afterwards.
    settings: Value,
}

/// The table spans the input class along what the rule generalises over: WHICH members
/// the request carries (each one alone, all of them, none of them), the SHAPE of the
/// replacement `Settings` (a smaller graph, a deeper one, a list of two), and whether a
/// sent free-text member carries a value or nothing at all.
///
/// Every member named here is one the model declares for `UpdatePreset` or on
/// `PresetSettings` and its closure —
/// `smithy_query(query="operation",operation="UpdatePreset")` and
/// `smithy_query(query="shape",shape="PresetSettings")`.
fn update_cases() -> Vec<Case> {
    vec![
        Case {
            label: "category alone: the other members keep their stored values",
            request: json!({ "category": "archive" }),
            category: Some("archive"),
            description: Some("the original"),
            settings: original_settings(),
        },
        Case {
            label: "description alone",
            request: json!({ "description": "revised wording" }),
            category: Some("web"),
            description: Some("revised wording"),
            settings: original_settings(),
        },
        Case {
            label: "settings alone, a smaller graph replacing the stored one",
            request: json!({ "settings": { "containerSettings": { "container": "WEBM" } } }),
            category: Some("web"),
            description: Some("the original"),
            settings: json!({ "containerSettings": { "container": "WEBM" } }),
        },
        Case {
            label: "settings alone, a deeper graph with two ordered audio descriptions",
            request: json!({
                "settings": {
                    "containerSettings": {
                        "container": "M2TS",
                        "mp4Settings": { "cttsVersion": 1, "mp4MajorBrand": "mp42" }
                    },
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
                    ],
                    "captionDescriptions": [{ "languageCode": "DEU" }]
                }
            }),
            category: Some("web"),
            description: Some("the original"),
            settings: json!({
                "containerSettings": {
                    "container": "M2TS",
                    "mp4Settings": { "cttsVersion": 1, "mp4MajorBrand": "mp42" }
                },
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
                ],
                "captionDescriptions": [{ "languageCode": "DEU" }]
            }),
        },
        Case {
            label: "every updatable member at once",
            request: json!({
                "category": "broadcast",
                "description": "all three members",
                "settings": {
                    "containerSettings": { "container": "MXF" },
                    "videoDescription": { "width": 1280, "height": 720 }
                }
            }),
            category: Some("broadcast"),
            description: Some("all three members"),
            settings: json!({
                "containerSettings": { "container": "MXF" },
                "videoDescription": { "width": 1280, "height": 720 }
            }),
        },
        Case {
            label: "no updatable member at all: nothing but the timestamp changes",
            request: json!({}),
            category: Some("web"),
            description: Some("the original"),
            settings: original_settings(),
        },
        Case {
            label: "an empty category clears it, as an empty one does on create",
            request: json!({ "category": "" }),
            category: None,
            description: Some("the original"),
            settings: original_settings(),
        },
        Case {
            label: "a blank description clears it and leaves the category alone",
            request: json!({ "description": "   " }),
            category: Some("web"),
            description: None,
            settings: original_settings(),
        },
        Case {
            label: "an empty settings object: no member of PresetSettings given",
            request: json!({ "settings": {} }),
            category: Some("web"),
            description: Some("the original"),
            settings: json!({}),
        },
    ]
}

/// A member the request carries replaces the stored one, a member it omits is left
/// exactly as it was, and `GetPreset` reads back precisely what the update stored.
#[tokio::test]
async fn update_preset_replaces_the_members_it_is_sent_and_keeps_the_rest() {
    let emulator = Emulator::new();

    for (index, case) in update_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("preset-{index}");
        let created = create_preset(&emulator, original_preset(&name)).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the fixture preset must be created, got {} with body {}",
            created.status, created.body
        );

        let updated = update_preset(&emulator, &name, case.request.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdatePreset, got {} with body {}",
            updated.status, updated.body
        );

        // The update's own response and the independent read must agree member for
        // member: the response is the stored record, not the request echoed.
        let fetched = get_preset(&emulator, &name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetPreset, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/preset"),
            updated.at("/preset"),
            "case {label:?}: GetPreset must return what UpdatePreset returned, got {}",
            fetched.body
        );

        for (source, reply) in [("UpdatePreset", &updated), ("GetPreset", &fetched)] {
            assert_eq!(
                reply.string("/preset/name"),
                name,
                "case {label:?} via {source}: expected the name as addressed, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/preset/settings"),
                &case.settings,
                "case {label:?} via {source}: expected settings {}, got {}",
                case.settings,
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
                &case.description.map_or(Value::Null, |value| json!(value)),
                "case {label:?} via {source}: expected description {:?}, got {}",
                case.description,
                reply.body
            );
            // An update never re-creates the resource: its `Type`, ARN and `CreatedAt`
            // are the ones the create persisted.
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
            assert_eq!(
                reply.at("/preset/createdAt"),
                created.at("/preset/createdAt"),
                "case {label:?} via {source}: CreatedAt must not move on an update, got {}",
                reply.body
            );
            // "The timestamp in epoch seconds when the preset was last updated" — whole
            // seconds, so a fast case can land in the same second as the create; it can
            // never land before it.
            let created_at = reply.at("/preset/createdAt").as_i64();
            let last_updated = reply.at("/preset/lastUpdated").as_i64();
            assert!(
                last_updated.is_some_and(|updated_at| created_at
                    .is_some_and(|created_at| updated_at >= created_at)),
                "case {label:?} via {source}: expected lastUpdated at or after createdAt, got {}",
                reply.body
            );
        }
    }
}

/// Updates compose: each one applies to what the previous one stored, so the preset a
/// reader sees is the accumulation of every update, never the last request alone.
#[tokio::test]
async fn successive_updates_each_apply_to_what_the_previous_one_stored() {
    let emulator = Emulator::new();
    let created = create_preset(&emulator, original_preset("accumulating")).await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );

    // Each step sends ONE member and names what the whole preset must then be.
    let steps: Vec<(&str, Value, Value, Value, Value)> = vec![
        (
            "the category alone",
            json!({ "category": "step-one" }),
            json!("step-one"),
            json!("the original"),
            original_settings(),
        ),
        (
            "then the settings alone",
            json!({ "settings": { "containerSettings": { "container": "WEBM" } } }),
            json!("step-one"),
            json!("the original"),
            json!({ "containerSettings": { "container": "WEBM" } }),
        ),
        (
            "then the description alone",
            json!({ "description": "step-three" }),
            json!("step-one"),
            json!("step-three"),
            json!({ "containerSettings": { "container": "WEBM" } }),
        ),
        (
            "then clearing the category",
            json!({ "category": "" }),
            Value::Null,
            json!("step-three"),
            json!({ "containerSettings": { "container": "WEBM" } }),
        ),
    ];

    for (label, request, category, description, settings) in steps {
        let reply = update_preset(&emulator, "accumulating", request).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        let fetched = get_preset(&emulator, "accumulating").await;
        for (source, reply) in [("UpdatePreset", &reply), ("GetPreset", &fetched)] {
            assert_eq!(
                reply.at("/preset/category"),
                &category,
                "case {label:?} via {source}: expected category {category}, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/preset/description"),
                &description,
                "case {label:?} via {source}: expected description {description}, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/preset/settings"),
                &settings,
                "case {label:?} via {source}: expected settings {settings}, got {}",
                reply.body
            );
        }
    }

    // Re-sending the same update is idempotent in everything but the timestamp.
    let once = update_preset(&emulator, "accumulating", json!({ "description": "settled" })).await;
    let twice = update_preset(&emulator, "accumulating", json!({ "description": "settled" })).await;
    for pointer in ["/preset/name", "/preset/arn", "/preset/type", "/preset/description"] {
        assert_eq!(
            once.at(pointer),
            twice.at(pointer),
            "case {pointer:?}: a repeated update must store the same value, got {} then {}",
            once.body,
            twice.body
        );
    }
}

/// `UpdatePreset` modifies "one of your existing presets": a name the store does not hold
/// is the modelled `NotFoundException`, and the refused call creates nothing.
#[tokio::test]
async fn updating_a_preset_that_does_not_exist_is_not_found() {
    let emulator = Emulator::new();
    let created = create_preset(&emulator, original_preset("present")).await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );

    // The rows differ in how they miss — never created, a prefix of a stored name, a
    // different case — and in what the update asks for.
    let misses: Vec<(&str, Value)> = vec![
        ("never created", json!({ "category": "web" })),
        ("a prefix of a stored name", json!({ "description": "nothing to change" })),
        (
            "the same name in another case",
            json!({ "settings": { "containerSettings": { "container": "MP4" } } }),
        ),
        ("an empty body", json!({})),
    ];
    for (index, (label, request)) in misses.iter().enumerate() {
        let name = ["absent", "presen", "PRESENT", "also-absent"]
            .get(index)
            .copied()
            .unwrap_or("absent");
        let reply = update_preset(&emulator, name, request.clone()).await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        let fetched = get_preset(&emulator, name).await;
        assert_eq!(
            fetched.status, 404,
            "case {label:?}: a refused update must create nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }

    // And the preset that does exist is untouched by every one of those refusals.
    let fetched = get_preset(&emulator, "present").await;
    assert_eq!(
        fetched.at("/preset"),
        created.at("/preset"),
        "the refused updates must leave the stored preset alone, got {}",
        fetched.body
    );
}

/// A request the model's own rules refuse is a `BadRequestException`, and it persists
/// nothing: the stored preset still reads back exactly as it was.
///
/// The bounded rows use `VideoDescription`'s published `Width`/`Height` range, 32 to 8192
/// (`smithy_query(query="shape",shape="VideoDescription")`): one step past each bound is
/// refused, and each bound itself is accepted and stored.
#[tokio::test]
async fn an_update_the_model_refuses_changes_nothing() {
    let emulator = Emulator::new();
    let created = create_preset(&emulator, original_preset("guarded")).await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );

    let refusals: Vec<(&str, Value)> = vec![
        (
            "Width one below the 32 minimum",
            json!({ "settings": { "videoDescription": { "width": 31 } } }),
        ),
        (
            "Height one above the 8192 maximum",
            json!({ "settings": { "videoDescription": { "height": 8193 } } }),
        ),
        (
            "Container outside the enumerated set",
            json!({ "settings": { "containerSettings": { "container": "MKV" } } }),
        ),
    ];
    for (label, request) in refusals {
        let reply = update_preset(&emulator, "guarded", request).await;
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
        let fetched = get_preset(&emulator, "guarded").await;
        assert_eq!(
            fetched.at("/preset"),
            created.at("/preset"),
            "case {label:?}: a refused update must change nothing, got {}",
            fetched.body
        );
    }

    for edge in [32, 8192] {
        let reply = update_preset(
            &emulator,
            "guarded",
            json!({ "settings": { "videoDescription": { "width": edge, "height": edge } } }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case width/height {edge}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        let fetched = get_preset(&emulator, "guarded").await;
        assert_eq!(
            fetched.at("/preset/settings/videoDescription/width"),
            &json!(edge),
            "case width/height {edge}: expected the bound stored and read back, got {}",
            fetched.body
        );
    }
}

/// The ARN an update reports names the region of the request that made it, exactly as a
/// read's does: the ARN is derived per request, never stored.
#[tokio::test]
async fn an_updated_preset_reads_back_in_every_region() {
    let emulator = Emulator::new();
    let created = emulator
        .send_in(
            "eu-central-1",
            "POST",
            "/2017-08-29/presets",
            Some(original_preset("regional")),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );

    for region in ["eu-central-1", "us-west-2", "ap-southeast-2"] {
        let updated = emulator
            .send_in(
                region,
                "PUT",
                "/2017-08-29/presets/regional",
                Some(json!({ "category": region })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {region:?}: expected HTTP 200, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("/preset/arn"),
            arn_in(region, "presets/regional"),
            "case {region:?}: expected the preset ARN in the updating region, got {}",
            updated.body
        );
        // The update is one resource in one store: a read from ANOTHER region sees the
        // value it wrote, under that reader's own ARN region.
        let fetched = emulator
            .send_in("us-east-1", "GET", "/2017-08-29/presets/regional", None)
            .await;
        assert_eq!(
            fetched.string("/preset/category"),
            region,
            "case {region:?}: expected the category the update stored, got {}",
            fetched.body
        );
        assert_eq!(
            fetched.string("/preset/arn"),
            arn_in("us-east-1", "presets/regional"),
            "case {region:?}: expected the preset ARN in the reading region, got {}",
            fetched.body
        );
    }
}
