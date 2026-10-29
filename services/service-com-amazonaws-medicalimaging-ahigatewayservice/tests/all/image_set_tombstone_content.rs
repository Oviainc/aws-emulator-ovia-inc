// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an image set gives up when its deletion LANDS.
//!
//! RULE: the landing of an image set's deletion releases the DICOM content the image set
//! held — its pixel data, the DICOM attributes describing that content (the ones the model
//! names and the ones held at the path they were written at alike) and the snapshot every
//! superseded version kept of those attributes — and leaves a tombstone that still reports
//! the image set's PROPERTIES: its identifiers, its versions, its states, its timestamps
//! and the tags kept on the record. Nothing of the content survives for a reader to report,
//! and the content goes exactly when the retirement lands, not when the call is made.
//!
//! This suite is a TABLE whose rows differ along what the record holds going in: modelled
//! attributes only, attributes the model does not name written at the patient, study,
//! series and SOP-instance levels, several versions each holding a snapshot of their own,
//! a removal before the delete, and an image set holding no DICOM attributes at all. Every
//! case drives the content in, and the delete, over real HTTP through the production front
//! door.
//!
//! ## Why this suite reads the record
//!
//! Every reader of an image set's content answers 404 on a retired one
//! (`crate::content_reads_shared_rules`), so what a tombstone still HOLDS is observable
//! nowhere on the wire: the record itself is where the rule has to be asserted. So each
//! case drives its content in and its delete over HTTP, asserts the wire behaviour of the
//! tombstone there, and then reads the surviving record through the crate's public `state`
//! module — the same seeding seam the sibling suites write through.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set and its pixel data enter a data store through a DICOM import, and
//! this emulator's import job does not create image sets yet — that is
//! `issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`.
//! So each case seeds one image set through the crate's public `state` module, with the
//! same `ImageSet::new` / `ImageFrame` the handlers hold, and drives everything else over
//! HTTP.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, IMAGE_SET_DELETION_DELAY, IMAGE_SET_NOT_FOUND_MESSAGE, IMAGE_SET_UPDATE_DELAY,
};

use crate::datastore_lifecycle::{Emulator, ACCOUNT, SETTLE_MULTIPLE};
use crate::image_frames::get_image_frame;
use crate::image_set_copy::get_image_set;
use crate::image_set_deletion::delete_image_set;
use crate::image_set_metadata::{document, get_image_set_metadata};
use crate::image_set_metadata_updates::{
    at, dicom_updates, seed_image_set, seeded_tags, update_metadata, SERIES_UID, SOP_UID, STUDY_UID,
};
use crate::resource_tagging::{list_tags, tag};

/// The tag the record carries, so each case can tell the properties a tombstone keeps from
/// the content it gives up.
const RECORD_TAG: (&str, &str) = ("team", "radiology");

/// One row of the table: the content the image set holds going in, and what the record
/// must still hold going out.
struct TombstoneCase {
    case: &'static str,
    /// The modelled attributes the image set is seeded with.
    tags: DICOMTags,
    /// One `UpdateImageSetMetadata` request per entry, sent in order before the delete:
    /// the attributes to write and the attributes to remove. Each lands a new VERSION, so
    /// a row with updates also exercises the snapshots the superseded versions keep.
    updates: Vec<(Option<Value>, Option<Value>)>,
    /// Paths the served metadata document must carry, with their values, BEFORE the
    /// delete — so no row asserts the release of content it never held.
    held: Vec<(Vec<&'static str>, Value)>,
    /// How many versions the record holds by the time it is deleted, and must still report
    /// as a tombstone.
    versions: usize,
    /// Whether the record holds at least one modelled attribute before the delete.
    holds_modelled: bool,
    /// Whether it holds at least one attribute the model does not name before the delete.
    holds_unmodelled: bool,
}

/// The rows span: modelled attributes alone; attributes the model does not name at the
/// patient level, the study level, one series and one SOP instance; one, two and three
/// versions (so one, two and three snapshots); a removal sent before the delete; and an
/// image set holding no DICOM attributes at all, which has nothing to give up.
fn tombstone_cases() -> Vec<TombstoneCase> {
    vec![
        TombstoneCase {
            case: "modelled attributes only, one version",
            tags: seeded_tags(STUDY_UID),
            updates: Vec::new(),
            held: vec![
                (vec!["Patient", "DICOM", "PatientID"], json!("PID-1")),
                (vec!["Patient", "DICOM", "PatientName"], json!("DOE^JANE")),
                (vec!["Study", "DICOM", "AccessionNumber"], json!("ACC-1")),
            ],
            versions: 1,
            holds_modelled: true,
            holds_unmodelled: false,
        },
        TombstoneCase {
            case: "a patient attribute the model does not name, two versions",
            tags: seeded_tags(STUDY_UID),
            updates: vec![(
                Some(json!({
                    "SchemaVersion": "1.1",
                    "Patient": { "DICOM": { "PatientAge": "054Y", "PatientName": "ROE^RICHARD" } },
                })),
                None,
            )],
            held: vec![
                (vec!["Patient", "DICOM", "PatientAge"], json!("054Y")),
                (vec!["Patient", "DICOM", "PatientName"], json!("ROE^RICHARD")),
            ],
            versions: 2,
            holds_modelled: true,
            holds_unmodelled: true,
        },
        TombstoneCase {
            case: "attributes at the study, series and instance levels, three versions",
            tags: seeded_tags(STUDY_UID),
            updates: vec![
                (
                    Some(json!({
                        "SchemaVersion": "1.1",
                        "Study": { "DICOM": { "InstitutionName": "GENERAL HOSPITAL" } },
                    })),
                    None,
                ),
                (
                    Some(json!({
                        "SchemaVersion": "1.1",
                        "Study": {
                            "Series": {
                                SERIES_UID: {
                                    "DICOM": { "ProtocolName": "CHEST ROUTINE" },
                                    "Instances": {
                                        SOP_UID: { "DICOM": { "InstanceNumber": "7" } },
                                    },
                                },
                            },
                        },
                    })),
                    None,
                ),
            ],
            held: vec![
                (
                    vec!["Study", "DICOM", "InstitutionName"],
                    json!("GENERAL HOSPITAL"),
                ),
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "ProtocolName"],
                    json!("CHEST ROUTINE"),
                ),
                (
                    vec![
                        "Study",
                        "Series",
                        SERIES_UID,
                        "Instances",
                        SOP_UID,
                        "DICOM",
                        "InstanceNumber",
                    ],
                    json!("7"),
                ),
            ],
            versions: 3,
            holds_modelled: true,
            holds_unmodelled: true,
        },
        TombstoneCase {
            case: "a removal before the delete, two versions",
            tags: seeded_tags(STUDY_UID),
            updates: vec![(
                Some(json!({
                    "SchemaVersion": "1.1",
                    "Patient": { "DICOM": { "PatientAge": "031Y" } },
                })),
                Some(json!({
                    "SchemaVersion": "1.1",
                    "Patient": { "DICOM": { "PatientSex": "F" } },
                })),
            )],
            held: vec![
                (vec!["Patient", "DICOM", "PatientAge"], json!("031Y")),
                (vec!["Patient", "DICOM", "PatientID"], json!("PID-1")),
            ],
            versions: 2,
            holds_modelled: true,
            holds_unmodelled: true,
        },
        TombstoneCase {
            case: "no DICOM attributes at all, one version",
            tags: DICOMTags::default(),
            updates: Vec::new(),
            held: Vec::new(),
            versions: 1,
            // An image set nothing has delivered DICOM content to holds no attribute of
            // either kind, so the landing has nothing to release but the pixel data.
            holds_modelled: false,
            holds_unmodelled: false,
        },
    ]
}

/// What one image-set RECORD holds, read through the crate's public `state` module.
#[derive(Debug)]
struct HeldContent {
    /// How many image frames of pixel data the record holds.
    frames: usize,
    /// The modelled attributes the image set itself holds, rendered so a comparison
    /// covers every member of `DICOMTags` rather than the ones a case thought to name.
    tags: String,
    /// How many attributes outside the modelled ones the image set itself holds.
    unmodelled: usize,
    /// One entry per version the record holds, oldest first: the version identifier, the
    /// modelled attributes that version reports (rendered as above) and how many
    /// unmodelled attributes it reports — i.e. what a revert to that version would put
    /// back.
    versions: Vec<(String, String, usize)>,
}

/// What the modelled attributes of an image set holding none render as: the one value
/// "no modelled attribute is held" has.
fn no_modelled_attributes() -> String {
    format!("{:?}", DICOMTags::default())
}

/// Read what the record of `image_set_id` holds. Nothing is WRITTEN through this handle:
/// the content it reports was driven in over HTTP.
fn held_content(emulator: &Emulator, datastore_id: &str, image_set_id: &str) -> HeldContent {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let store = state.lock();
    let image_set = store
        .datastores
        .get(datastore_id)
        .and_then(|datastore| datastore.image_sets.get(image_set_id))
        .expect("the image set record under test");
    let versions = image_set
        .versions()
        .map(|version| {
            let version_id = version.wire_version_id();
            let attributes = image_set
                .attributes_of_version(&version_id)
                .expect("a version the record holds");
            (
                version_id,
                format!("{:?}", attributes.dicom_tags),
                attributes.extra_attributes.len(),
            )
        })
        .collect();
    HeldContent {
        frames: image_set.image_frames.len(),
        tags: format!("{:?}", image_set.dicom_tags),
        unmodelled: image_set.extra_attributes.len(),
        versions,
    }
}

/// The identifiers of the image frames the record of `image_set_id` holds, so a case can
/// read each one back over HTTP before and after the retirement.
fn frame_ids(emulator: &Emulator, datastore_id: &str, image_set_id: &str) -> Vec<String> {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let store = state.lock();
    let image_set = store
        .datastores
        .get(datastore_id)
        .and_then(|datastore| datastore.image_sets.get(image_set_id))
        .expect("the image set record under test");
    let mut ids: Vec<String> = image_set.image_frames.keys().cloned().collect();
    ids.sort();
    ids
}

/// RULE (above): the landing of a deletion releases every piece of DICOM content the image
/// set held, at every version of it, and keeps the properties the tombstone reports.
#[tokio::test]
async fn the_landing_of_a_deletion_releases_the_content_the_image_set_held() {
    for (row, case) in tombstone_cases().into_iter().enumerate() {
        let name = case.case;
        // One emulator per row, so no row inherits another's resources.
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("tombstone.store", &format!("tombstone-{row}"))
            .await;
        let image_set_id = seed_image_set(&emulator, &datastore_id, case.tags);
        let arn = format!(
            "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}/imageset/{image_set_id}"
        );
        let tagged = tag(&emulator, &arn, json!({ RECORD_TAG.0: RECORD_TAG.1 })).await;
        assert_eq!(
            tagged.status, 200,
            "case {name:?}: expected HTTP 200 from TagResource, got {tagged:?}"
        );

        // The content goes in over the wire, one version per request.
        for (index, (updatable, removable)) in case.updates.iter().enumerate() {
            let latest_version = (index + 1).to_string();
            let updated = update_metadata(
                &emulator,
                &datastore_id,
                &image_set_id,
                &latest_version,
                dicom_updates(updatable.as_ref(), removable.as_ref()),
                &[],
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {name:?} update {index}: expected HTTP 200 from UpdateImageSetMetadata, \
                 got {updated:?}"
            );
            // The update is asynchronous, and a delete is refused while other work is in
            // flight, so the row waits the write out before asking for the deletion.
            tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
        }

        // What the image set holds going in, over the wire and in the record.
        let served = get_image_set_metadata(&emulator, &datastore_id, &image_set_id, None).await;
        let served = document(&served, name);
        for (path, expected) in &case.held {
            assert_eq!(
                at(&served, path),
                *expected,
                "case {name:?}: expected {path:?} to carry {expected} before the delete, \
                 got {served}"
            );
        }
        let frames = frame_ids(&emulator, &datastore_id, &image_set_id);
        assert!(
            !frames.is_empty(),
            "case {name:?}: the row must hold pixel data going in, got {frames:?}"
        );
        for frame_id in &frames {
            let read = get_image_frame(&emulator, &datastore_id, &image_set_id, frame_id).await;
            assert_eq!(
                read.status, 200,
                "case {name:?} frame {frame_id}: pixel data is served before the delete, \
                 got {read:?}"
            );
        }
        let before = held_content(&emulator, &datastore_id, &image_set_id);
        assert_eq!(
            before.versions.len(),
            case.versions,
            "case {name:?}: expected {} versions going in, got {before:?}",
            case.versions
        );
        assert_eq!(
            before.tags != no_modelled_attributes(),
            case.holds_modelled,
            "case {name:?}: expected holds_modelled={}, got {before:?}",
            case.holds_modelled
        );
        assert_eq!(
            before.unmodelled > 0,
            case.holds_unmodelled,
            "case {name:?}: expected holds_unmodelled={}, got {before:?}",
            case.holds_unmodelled
        );
        // A superseded version's snapshot holds the content that version was serving, so
        // a row that wrote content through several versions holds it several times over.
        let snapshots_holding_content = before
            .versions
            .iter()
            .filter(|(_, tags, unmodelled)| tags != &no_modelled_attributes() || *unmodelled > 0)
            .count();
        assert_eq!(
            snapshots_holding_content,
            if case.holds_modelled || case.holds_unmodelled {
                case.versions
            } else {
                0
            },
            "case {name:?}: expected every version going in to hold the content it served, \
             got {before:?}"
        );

        // The delete, and the clock that lands it — nothing else.
        let deleted = delete_image_set(&emulator, &datastore_id, &image_set_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected HTTP 200 from DeleteImageSet, got {deleted:?}"
        );
        // Mid-flight the content is still held: the deletion releases it when it LANDS.
        let in_flight = held_content(&emulator, &datastore_id, &image_set_id);
        assert_eq!(
            in_flight.frames, before.frames,
            "case {name:?}: a retirement in flight releases no pixel data, got {in_flight:?}"
        );
        assert_eq!(
            in_flight.tags, before.tags,
            "case {name:?}: a retirement in flight releases no DICOM attribute, got {in_flight:?}"
        );
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_DELETION_DELAY).await;

        let landed = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            landed.string(&["imageSetState"]),
            "DELETED",
            "case {name:?}: expected the clock to land DELETED, got {landed:?}"
        );
        assert!(
            landed
                .field(&["deletedAt"])
                .as_f64()
                .is_some_and(|instant| instant > 0.0),
            "case {name:?}: expected a deletedAt once the retirement landed, got {landed:?}"
        );

        // What the tombstone KEEPS: the properties of the image set, including the tags
        // held on the record.
        assert_eq!(
            landed.string(&["imageSetId"]),
            image_set_id,
            "case {name:?}: a tombstone reports its own identifier, got {landed:?}"
        );
        let tags_after = list_tags(&emulator, &arn).await;
        assert_eq!(
            tags_after.string(&["tags", RECORD_TAG.0]),
            RECORD_TAG.1,
            "case {name:?}: a tombstone keeps the tags held on the record, got {tags_after:?}"
        );

        // What it GIVES UP: every content read answers for an absent image set.
        let metadata = get_image_set_metadata(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            metadata.status, 404,
            "case {name:?}: a tombstone serves no metadata document, got {metadata:?}"
        );
        assert_eq!(
            metadata.string(&["message"]),
            IMAGE_SET_NOT_FOUND_MESSAGE,
            "case {name:?}: expected the image-set-not-found text, got {metadata:?}"
        );
        for frame_id in &frames {
            let gone = get_image_frame(&emulator, &datastore_id, &image_set_id, frame_id).await;
            assert_eq!(
                gone.status, 404,
                "case {name:?} frame {frame_id}: a tombstone serves no pixel data, got {gone:?}"
            );
        }

        // And the record holds none of it, at any version: nothing is left for a reader to
        // report or for a revert to put back.
        let after = held_content(&emulator, &datastore_id, &image_set_id);
        assert_eq!(
            after.frames, 0,
            "case {name:?}: expected a tombstone to hold no pixel data, got {after:?}"
        );
        assert_eq!(
            after.tags,
            no_modelled_attributes(),
            "case {name:?}: expected a tombstone to hold no modelled attribute, got {after:?}"
        );
        assert_eq!(
            after.unmodelled, 0,
            "case {name:?}: expected a tombstone to hold no unmodelled attribute, got {after:?}"
        );
        assert_eq!(
            after.versions.len(),
            case.versions,
            "case {name:?}: a tombstone keeps the versions it reported, got {after:?}"
        );
        for (version_id, tags, unmodelled) in &after.versions {
            assert_eq!(
                tags,
                &no_modelled_attributes(),
                "case {name:?} version {version_id}: expected no modelled attribute to survive \
                 the retirement, got {after:?}"
            );
            assert_eq!(
                *unmodelled, 0,
                "case {name:?} version {version_id}: expected no unmodelled attribute to survive \
                 the retirement, got {after:?}"
            );
        }
    }
}
