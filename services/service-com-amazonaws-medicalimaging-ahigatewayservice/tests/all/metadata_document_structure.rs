// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the STRUCTURE of the aggregated DICOM metadata document.
//!
//! RULE: the aggregated metadata document is built from the records it is served out of —
//! which shape of document it is, which data store and image set it describes, and the
//! patient → study → series → instance levels holding the attributes and image frames the
//! image set holds. That structure is computed, not stored: the attributes written into an
//! image set are merged INTO it, so an attribute fills a place the document has nothing at
//! and never takes the place of a value the service computed there or collapses a level it
//! built. An update stores nothing at all for a key that says what the document IS.
//!
//! This suite is a TABLE whose rows differ along what the rule generalises over: which
//! computed key an attribute is written under (the schema declaration, the data store
//! identifier, the image set identifier), which level it is written over (the patient
//! level, the study level, a `DICOM` level, one series, one SOP instance, the image frames
//! of one instance), and the KIND of value written there (text, a number, an array, an
//! object) — plus the rows that must still be served, so the merge is shown to do its job:
//! an attribute the model does not name inside a level the document builds, one under a
//! SOP instance it builds, and a whole level the document has no member for.
//!
//! Every case drives real HTTP through the production front door, so what is asserted is
//! the wire behaviour a `medical-imaging` SDK client sees. The one read of state is the
//! last assertion of each row, which is about what was PERSISTED rather than about an
//! answer.
//!
//! The image set each row starts from is seeded the way the sibling update suite seeds
//! one, for the reason given there: in AWS an image set's DICOM content arrives through a
//! DICOM import, which this emulator does not do yet.

use serde_json::{json, Map, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, IMAGE_SET_UPDATE_DELAY, METADATA_COMPUTED_KEYS,
};

use crate::datastore_lifecycle::{unheld_id, Emulator, SETTLE_MULTIPLE};
use crate::image_set_metadata_updates::{
    at, dicom_updates, read_document, seed_image_set, seeded_tags, update_metadata, SERIES_UID,
    SOP_UID, STUDY_UID,
};

/// The schema version every served document declares, asserted as the literal a client
/// reads rather than through the handler's own constant.
const SERVED_SCHEMA_VERSION: &str = "1.1";

/// How long an identifier this service mints is, and which characters it is spelled with
/// (`DatastoreId` / `ImageSetId`, both `^[0-9a-z]{32}$`).
const ID_LENGTH: usize = 32;

/// One row of the table.
struct StructureCase {
    case: &'static str,
    /// The `updatableAttributes` document the row stores into the image set.
    stored: Value,
    /// The paths that document must be SERVED at afterwards, with the value each holds —
    /// empty for a row whose attributes the computed structure stands against.
    served: Vec<(Vec<&'static str>, Value)>,
}

/// The path of the image frames reported for the SOP instance the seeded pixel data came
/// from.
fn image_frames_path() -> Vec<&'static str> {
    vec![
        "Study",
        "Series",
        SERIES_UID,
        "Instances",
        SOP_UID,
        "ImageFrames",
    ]
}

/// The path of one attribute keyword under the `DICOM` level of that SOP instance.
fn instance_attribute_path(keyword: &'static str) -> Vec<&'static str> {
    vec![
        "Study",
        "Series",
        SERIES_UID,
        "Instances",
        SOP_UID,
        "DICOM",
        keyword,
    ]
}

/// An attribute document writing `value` at the path the image frames of the seeded SOP
/// instance are reported at.
fn over_image_frames(value: Value) -> Value {
    json!({
        "Study": { "Series": { SERIES_UID: { "Instances": { SOP_UID: { "ImageFrames": value } } } } },
    })
}

fn structure_cases() -> Vec<StructureCase> {
    vec![
        StructureCase {
            case: "an attribute written under the data store identifier",
            stored: json!({ "DatastoreID": unheld_id() }),
            served: Vec::new(),
        },
        StructureCase {
            case: "an attribute written under the image set identifier",
            stored: json!({ "ImageSetID": unheld_id() }),
            served: Vec::new(),
        },
        StructureCase {
            case: "another schema version written under the schema declaration",
            stored: json!({ "SchemaVersion": "9.9" }),
            served: Vec::new(),
        },
        StructureCase {
            case: "every computed key at once, as a number, an object and an array",
            stored: json!({
                "SchemaVersion": 9,
                "DatastoreID": { "DICOM": { "PatientID": "PID-SPOOF" } },
                "ImageSetID": ["not-an-image-set"],
            }),
            served: Vec::new(),
        },
        StructureCase {
            case: "text written over the whole patient level",
            stored: json!({ "Patient": "x" }),
            served: Vec::new(),
        },
        StructureCase {
            case: "a number written over the whole study level",
            stored: json!({ "Study": 7 }),
            served: Vec::new(),
        },
        StructureCase {
            case: "text written over the patient DICOM level",
            stored: json!({ "Patient": { "DICOM": "x" } }),
            served: Vec::new(),
        },
        StructureCase {
            case: "text written over the one series the image set holds",
            stored: json!({ "Study": { "Series": { SERIES_UID: "x" } } }),
            served: Vec::new(),
        },
        StructureCase {
            case: "text written over the SOP instance the pixel data came from",
            stored: json!({
                "Study": { "Series": { SERIES_UID: { "Instances": { SOP_UID: "x" } } } },
            }),
            served: Vec::new(),
        },
        StructureCase {
            case: "text written over the image frames of that instance",
            stored: over_image_frames(json!("none")),
            served: Vec::new(),
        },
        StructureCase {
            case: "an object written over the image frames of that instance",
            stored: over_image_frames(json!({ "ID": "not-an-image-frame" })),
            served: Vec::new(),
        },
        StructureCase {
            case: "an array written over the image frames of that instance",
            stored: over_image_frames(json!([{ "ID": "not-an-image-frame" }])),
            served: Vec::new(),
        },
        StructureCase {
            // The merge still does its job: an attribute the model does not name, inside a
            // level the document builds, is served from where it was stored.
            case: "an unmodelled attribute inside a level the document builds",
            stored: json!({ "Patient": { "DICOM": { "EthnicGroup": "UNKNOWN" } } }),
            served: vec![(
                vec!["Patient", "DICOM", "EthnicGroup"],
                json!("UNKNOWN"),
            )],
        },
        StructureCase {
            case: "an unmodelled attribute under the SOP instance the document builds",
            stored: json!({
                "Study": {
                    "Series": {
                        SERIES_UID: {
                            "Instances": { SOP_UID: { "DICOM": { "InstanceNumber": "7" } } },
                        },
                    },
                },
            }),
            served: vec![(instance_attribute_path("InstanceNumber"), json!("7"))],
        },
        StructureCase {
            case: "a whole level the document has no member for",
            stored: json!({ "Equipment": { "DICOM": { "Manufacturer": "ACME" } } }),
            served: vec![(
                vec!["Equipment", "DICOM", "Manufacturer"],
                json!("ACME"),
            )],
        },
    ]
}

/// Every leaf the stored document names: the path it sits at, and the value the row wrote
/// there. A level is an object, so anything else is one value a row tried to put into the
/// served document.
fn stored_leaves(document: &Value) -> Vec<(Vec<String>, Value)> {
    let mut found = Vec::new();
    collect_leaves(document, &mut Vec::new(), &mut found);
    found
}

/// The recursive half of [`stored_leaves`], carrying the path walked so far.
fn collect_leaves(value: &Value, path: &mut Vec<String>, found: &mut Vec<(Vec<String>, Value)>) {
    match value {
        Value::Object(level) => {
            for (name, nested) in level {
                path.push(name.clone());
                collect_leaves(nested, path, found);
                path.pop();
            }
        }
        _ => found.push((path.clone(), value.clone())),
    }
}

/// The attributes `image_set_id` HOLDS outside the modelled ones — the one assertion of
/// this suite that is about what was persisted rather than about an answer.
fn stored_attributes(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
) -> Map<String, Value> {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let store = state.lock();
    store
        .datastores
        .get(datastore_id)
        .and_then(|datastore| datastore.image_sets.get(image_set_id))
        .map(|image_set| image_set.extra_attributes.clone())
        .expect("the seeded image set")
}

#[tokio::test]
async fn a_stored_attribute_never_displaces_the_structure_the_document_computes() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.structure", "token-metadata-structure")
        .await;

    for case in structure_cases() {
        let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        let stored = update_metadata(
            &emulator,
            &datastore_id,
            &image_set_id,
            "1",
            dicom_updates(Some(&case.stored), None),
            &[],
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {:?}: expected HTTP 200 from UpdateImageSetMetadata, got {stored:?}",
            case.case
        );

        // The clock, not a reader, settles the update.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
        let document = read_document(&emulator, &datastore_id, &image_set_id, case.case).await;

        // 1. The document still says what it IS: the shape the service writes, and the
        //    records it was built from.
        for (path, expected) in [
            (vec!["SchemaVersion"], SERVED_SCHEMA_VERSION.to_string()),
            (vec!["DatastoreID"], datastore_id.clone()),
            (vec!["ImageSetID"], image_set_id.clone()),
        ] {
            assert_eq!(
                at(&document, &path),
                Value::String(expected.clone()),
                "case {:?}: expected {path:?} to hold {expected:?}, got {}",
                case.case,
                at(&document, &path)
            );
        }

        // 2. Every level the handler builds is still a level, carrying the attributes the
        //    image set holds (the ones `SearchImageSets` filters on).
        for (path, expected) in [
            (vec!["Patient", "DICOM", "PatientID"], json!("PID-1")),
            (vec!["Patient", "DICOM", "PatientName"], json!("DOE^JANE")),
            (vec!["Study", "DICOM", "StudyInstanceUID"], json!(STUDY_UID)),
            (
                vec!["Study", "DICOM", "NumberOfStudyRelatedInstances"],
                json!(2),
            ),
            (
                vec!["Study", "Series", SERIES_UID, "DICOM", "SeriesInstanceUID"],
                json!(SERIES_UID),
            ),
            (
                vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"],
                json!("CT"),
            ),
            (instance_attribute_path("SOPInstanceUID"), json!(SOP_UID)),
        ] {
            assert_eq!(
                at(&document, &path),
                expected,
                "case {:?}: expected {path:?} to hold {expected}, got {}",
                case.case,
                at(&document, &path)
            );
        }

        // 3. The image frames reported for that instance are still the ones the image set
        //    holds: the single seeded frame, under the identifier `GetImageFrame` takes.
        let frames = at(&document, &image_frames_path());
        let reported: Vec<String> = frames
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|frame| frame["ID"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected the one image frame the image set holds to be reported, \
             got {frames}",
            case.case
        );
        assert!(
            reported.iter().all(|id| id.len() == ID_LENGTH
                && id
                    .chars()
                    .all(|digit| digit.is_ascii_digit() || digit.is_ascii_lowercase())),
            "case {:?}: expected every reported frame to carry an identifier this service \
             minted, got {reported:?}",
            case.case
        );

        // 4. What the row stored at a place the document had nothing at IS served there.
        for (path, expected) in &case.served {
            assert_eq!(
                at(&document, path),
                *expected,
                "case {:?}: expected {path:?} to hold {expected}, got {}",
                case.case,
                at(&document, path)
            );
        }

        // 5. A row the computed structure stands against reports NONE of the values it
        //    wrote, at the very paths it wrote them at.
        if case.served.is_empty() {
            for (path, written) in stored_leaves(&case.stored) {
                let names: Vec<&str> = path.iter().map(String::as_str).collect();
                assert_ne!(
                    at(&document, &names),
                    written,
                    "case {:?}: expected {names:?} not to report {written}, the value the \
                     update wrote there",
                    case.case
                );
            }
        }

        // 6. Nothing is PERSISTED under a key that says what the document is: such a key
        //    names no attribute of an image set, so there is nothing in it to store.
        let held = stored_attributes(&emulator, &datastore_id, &image_set_id);
        for key in METADATA_COMPUTED_KEYS {
            assert!(
                !held.contains_key(key),
                "case {:?}: expected nothing stored under {key:?}, got {held:?}",
                case.case
            );
        }
    }
}
