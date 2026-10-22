// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeletePreset`.
//!
//! `DeletePreset` is documented as "Permanently delete a preset you have created", so the
//! rule under test is: the removal is IMMEDIATE and the store is what every later call
//! reads — the deleted name answers `NotFoundException` (404) from `GetPreset` and from a
//! second `DeletePreset`, the name is free to create again, and no other preset moves.
//! A name the store never held is the same 404.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconvert_mediaconvert::state::not_found_message;

use super::job_lifecycle::{Emulator, Reply};

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

/// `DELETE /2017-08-29/presets/{Name}` — the binding
/// `smithy_query(query="operation",operation="DeletePreset")` reports, whose success code
/// is 202 with an empty output.
async fn delete_preset(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("DELETE", &format!("/2017-08-29/presets/{name}"), None)
        .await
}

/// One preset to create, delete, and fail to read back again.
struct Case {
    label: &'static str,
    name: &'static str,
    request: Value,
    /// Whether the case updates the preset between the create and the delete, so the
    /// delete is exercised on a record that was mutated as well as on a freshly created
    /// one.
    update: Option<Value>,
}

/// The rows span what a caller can vary about the preset being deleted: which optional
/// members it carries, the shape and depth of its `Settings` graph (every member named
/// here is one the model declares on `PresetSettings` and its closure —
/// `smithy_query(query="shape",shape="PresetSettings")`), whether it was updated after
/// being created, whether it is tagged, and the shape of the name itself.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the required members",
            name: "minimal",
            request: json!({
                "name": "minimal",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
            update: None,
        },
        Case {
            label: "described, categorised and updated before the delete",
            name: "described",
            request: json!({
                "name": "described",
                "category": "probe-category",
                "description": "probe preset",
                "settings": {
                    "containerSettings": { "container": "MP4" },
                    "videoDescription": {
                        "width": 1280,
                        "height": 720,
                        "codecSettings": {
                            "codec": "H_264",
                            "h264Settings": { "rateControlMode": "QVBR", "maxBitrate": 5_000_000 }
                        }
                    }
                }
            }),
            update: Some(json!({ "description": "updated preset description" })),
        },
        Case {
            label: "two audio descriptions and a caption description",
            name: "multi-track",
            request: json!({
                "name": "multi-track",
                "settings": {
                    "containerSettings": { "container": "M2TS" },
                    "audioDescriptions": [
                        { "audioSourceName": "Audio Selector 1", "languageCode": "ENG" },
                        { "audioSourceName": "Audio Selector 2", "languageCode": "SPA" }
                    ],
                    "captionDescriptions": [{ "languageCode": "FRA" }]
                }
            }),
            update: None,
        },
        Case {
            label: "tagged, with an empty settings object",
            name: "tagged",
            request: json!({
                "name": "tagged",
                "settings": {},
                "tags": { "team": "media" }
            }),
            update: Some(json!({ "category": "archive" })),
        },
        Case {
            label: "a long mixed-case name",
            name: "A-Preset-Name_That.Is-Rather-Longer-0123456789",
            request: json!({
                "name": "A-Preset-Name_That.Is-Rather-Longer-0123456789",
                "settings": { "containerSettings": { "container": "MXF" } }
            }),
            update: None,
        },
    ]
}

/// A deleted preset is gone from the store: `GetPreset` on that name is the modelled
/// `NotFoundException` (404) right away and stays so, a second delete is the same 404,
/// and the name is free for a new preset afterwards.
#[tokio::test]
async fn a_deleted_preset_is_not_readable_and_its_name_is_free_again() {
    for case in cases() {
        let Case {
            label,
            name,
            request,
            update,
        } = case;
        // One account per row, so the row's own delete is the only thing that removed
        // anything.
        let emulator = Emulator::new();

        // A second preset that the delete must not touch.
        let bystander = create_preset(
            &emulator,
            json!({
                "name": "bystander",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
        )
        .await;
        assert_eq!(
            bystander.status, 201,
            "case {label:?}: the bystander preset must be created, got {} with body {}",
            bystander.status, bystander.body
        );

        let created = create_preset(&emulator, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the fixture preset must be created, got {} with body {}",
            created.status, created.body
        );
        if let Some(update) = &update {
            let updated = update_preset(&emulator, name, update.clone()).await;
            assert_eq!(
                updated.status, 200,
                "case {label:?}: the update before the delete must apply, got {} with body {}",
                updated.status, updated.body
            );
        }
        let live = get_preset(&emulator, name).await;
        assert_eq!(
            live.status, 200,
            "case {label:?}: the preset must be readable before the delete, got {} with body {}",
            live.status, live.body
        );

        // `DeletePreset`'s success code is 202 with an empty output.
        let deleted = delete_preset(&emulator, name).await;
        assert_eq!(
            deleted.status, 202,
            "case {label:?}: expected HTTP 202 from DeletePreset, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null() || deleted.body == json!({}),
            "case {label:?}: DeletePreset models an empty output, got body {}",
            deleted.body
        );

        // The read-after-delete is a 404 immediately, and repeating it never flips back.
        for attempt in 1..=3 {
            let gone = get_preset(&emulator, name).await;
            assert_eq!(
                gone.status, 404,
                "case {label:?} read #{attempt}: expected HTTP 404 after the delete, got {} with \
                 body {}",
                gone.status, gone.body
            );
            assert_eq!(
                gone.error_type.as_deref(),
                Some("NotFoundException"),
                "case {label:?} read #{attempt}: expected NotFoundException, got {:?} with body {}",
                gone.error_type,
                gone.body
            );
            assert_eq!(
                gone.string("/message"),
                not_found_message("name"),
                "case {label:?} read #{attempt}: the refusal must name the member the lookup \
                 read, got {}",
                gone.body
            );
        }

        // A second delete has nothing to remove, so it is the same 404 rather than a
        // second success.
        let again = delete_preset(&emulator, name).await;
        assert_eq!(
            again.status, 404,
            "case {label:?}: expected HTTP 404 from a second DeletePreset, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException from a second delete, got {:?} with body \
             {}",
            again.error_type,
            again.body
        );

        // Nothing else moved.
        let other = get_preset(&emulator, "bystander").await;
        assert_eq!(
            other.at("/preset"),
            bystander.at("/preset"),
            "case {label:?}: the delete must leave every other preset untouched, got {}",
            other.body
        );

        // The name is the resource's identity, so once the record is gone the name is
        // free: a create under it succeeds instead of conflicting.
        let recreated = create_preset(&emulator, request).await;
        assert_eq!(
            recreated.status, 201,
            "case {label:?}: the deleted name must be free to create again, got {} with body {}",
            recreated.status, recreated.body
        );
        // And the new record is the created one, not the deleted one: the update the row
        // may have applied before the delete is not carried over.
        assert_eq!(
            recreated.at("/preset/description"),
            created.at("/preset/description"),
            "case {label:?}: the re-created preset must carry the create's own members, got {}",
            recreated.body
        );
    }
}

/// A preset nobody created cannot be deleted: `DeletePreset` answers the modelled
/// `NotFoundException` (404) and removes nothing.
///
/// The rows differ in how they miss: never created, a prefix of a stored name, a
/// different case, and a name that looks like a built-in resource.
#[tokio::test]
async fn deleting_a_preset_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();
    let created = create_preset(
        &emulator,
        json!({
            "name": "present",
            "settings": { "containerSettings": { "container": "MP4" } }
        }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created, got {} with body {}",
        created.status, created.body
    );

    for name in ["absent", "presen", "PRESENT", "System-Generic-Hd-Mp4-Avc-Aac-16x9"] {
        let reply = delete_preset(&emulator, name).await;
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
        assert_eq!(
            reply.string("/message"),
            not_found_message("name"),
            "case {name:?}: the refusal must name the member the lookup read, got {}",
            reply.body
        );

        // The preset that IS there is untouched by every miss.
        let still_there = get_preset(&emulator, "present").await;
        assert_eq!(
            still_there.at("/preset"),
            created.at("/preset"),
            "case {name:?}: a refused delete must remove nothing, got {}",
            still_there.body
        );
    }
}
