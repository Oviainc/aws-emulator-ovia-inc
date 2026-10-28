// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateImageSetMetadata`.
//!
//! RULE: an update writes the DICOM metadata of the image set the request names as a NEW
//! version of it, from the aggregated-metadata document the request carries: every
//! attribute the document names is written at the path it names it at — to the modelled
//! member when the model names one there, so a search and the metadata document never
//! disagree, and as it was written otherwise — and every attribute a removal document
//! names is gone afterwards. A revert puts back the attributes the version it names held.
//! The answer reports the image set at its new version, `LOCKED` / `UPDATING`, and the
//! clock settles it to `ACTIVE` / `UPDATED`; a stale `latestVersion` is 409
//! `ConflictException`, a data store, image set or version the service does not hold is
//! 404, and a document the service cannot read, a change the `force` flag exists for
//! without that flag, or a value of the wrong kind for a modelled attribute is 400
//! `ValidationException` that persists nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! level of the document an attribute sits at (patient, study, series, one SOP instance),
//! whether the model names that attribute, how many attributes one document carries, the
//! kind the value has (text, number), writing versus removing, which force-guarded
//! attribute is changed, which version a revert names, and whether the update reaches the
//! study. Every case drives real HTTP through the production front door, so what is
//! asserted is the wire behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image set through the crate's public `state` module with the
//! same `ImageSet::new` / `ImageFrame` the handlers use, and then asserts only over the
//! wire.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageFrame, ImageSet, ImageSetState, ImageSetWorkflowStatus,
    DATASTORE_NOT_FOUND_MESSAGE, IMAGE_SET_NOT_FOUND_MESSAGE, IMAGE_SET_NOT_UPDATABLE_MESSAGE,
    IMAGE_SET_UPDATE_DELAY, IMAGE_SET_VERSION_CONFLICT_MESSAGE,
    IMAGE_SET_VERSION_NOT_FOUND_MESSAGE, MAX_IMAGE_SET_EXTRA_ATTRIBUTES,
};

use crate::datastore_lifecycle::{unheld_id, Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::get_image_set;

/// The Series Instance UID every seeded image set holds — the one series the model
/// declares attributes for (`DICOMTags.dicomSeriesInstanceUID`).
pub(crate) const SERIES_UID: &str = "1.2.840.113619.2.1.2.3";

/// The SOP Instance UID the seeded pixel data came from, so the served document has an
/// instance level for a case that writes an attribute of one SOP instance.
const SOP_UID: &str = "1.2.840.113619.2.1.1.4";

/// The Study Instance UID a seeded image set holds unless a case names another.
///
/// `pub(crate)` because `crate::image_set_capacity` puts a retired image set in the same
/// study, and `crate::image_set_series_attributes` seeds the same study while varying only
/// the series — each of which is that suite's rule rather than this one's.
pub(crate) const STUDY_UID: &str = "1.2.840.113619.2.1.9.1";

/// The `contentType` `GetImageSetMetadata` serves the document with.
const METADATA_CONTENT_TYPE: &str = "application/json";

/// The DICOM attributes a seeded image set starts from: every modelled member filled, so
/// a case can write one, remove one, and still tell what it did not touch.
pub(crate) fn seeded_tags(study_instance_uid: &str) -> DICOMTags {
    DICOMTags {
        dicom_patient_id: Some("PID-1".to_string()),
        dicom_patient_name: Some("DOE^JANE".to_string()),
        dicom_patient_birth_date: Some("19700101".to_string()),
        dicom_patient_sex: Some("F".to_string()),
        dicom_study_instance_uid: Some(study_instance_uid.to_string()),
        dicom_study_id: Some("STUDY-1".to_string()),
        dicom_study_description: Some("CHEST".to_string()),
        dicom_number_of_study_related_series: 1,
        dicom_number_of_study_related_instances: 2,
        dicom_accession_number: Some("ACC-1".to_string()),
        dicom_series_instance_uid: Some(SERIES_UID.to_string()),
        dicom_series_modality: Some("CT".to_string()),
        dicom_series_body_part: Some("CHEST".to_string()),
        dicom_series_number: Some(3),
        dicom_study_date: Some("20240101".to_string()),
        dicom_study_time: Some("120000".to_string()),
    }
}

/// Seed one `ACTIVE` image set holding `tags` and one image frame of [`SOP_UID`], and
/// answer its identifier.
///
/// Built with the crate's own `ImageSet::new`, so a seeded image set is shaped exactly
/// like one a handler mints.
pub(crate) fn seed_image_set(emulator: &Emulator, datastore_id: &str, tags: DICOMTags) -> String {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let image_frame_id = store.mint_id();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let mut image_set = ImageSet::new(
        image_set_id.clone(),
        image_set_arn(&datastore.datastore_arn, &image_set_id),
        ImageSetState::Active,
        ImageSetWorkflowStatus::Imported,
        true,
        false,
        SystemTime::now(),
    );
    image_set.dicom_tags = tags;
    image_set.image_frames.insert(
        image_frame_id.clone(),
        ImageFrame {
            image_frame_id,
            sop_instance_uid: Some(SOP_UID.to_string()),
            transfer_syntax_uid: None,
            pixel_data: vec![0x11, 0x22, 0x33, 0x44],
        },
    );
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    image_set_id
}

/// The `updateImageSetMetadataUpdates` payload for a `DICOMUpdates` update, with each
/// document carried RAW — so a case can send bytes that are not a document at all. A blob
/// member is base64 inside a JSON body, which is how an SDK client sends one.
fn dicom_updates_raw(updatable: Option<&str>, removable: Option<&str>) -> Value {
    let mut updates = json!({});
    if let Some(document) = updatable {
        updates["updatableAttributes"] = json!(aws_smithy_types::base64::encode(document));
    }
    if let Some(document) = removable {
        updates["removableAttributes"] = json!(aws_smithy_types::base64::encode(document));
    }
    json!({ "DICOMUpdates": updates })
}

/// [`dicom_updates_raw`] for documents a case spells as JSON.
pub(crate) fn dicom_updates(updatable: Option<&Value>, removable: Option<&Value>) -> Value {
    dicom_updates_raw(
        updatable.map(Value::to_string).as_deref(),
        removable.map(Value::to_string).as_deref(),
    )
}

/// The `updateImageSetMetadataUpdates` payload for a revert.
fn revert_to(version_id: &str) -> Value {
    json!({ "revertToVersionId": version_id })
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/updateImageSetMetadata` —
/// `UpdateImageSetMetadata`. `latestVersion`, `force` and `includeStudyImageSets` are
/// `@httpQuery`; the body IS the `updateImageSetMetadataUpdates` payload.
pub(crate) async fn update_metadata(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    latest_version: &str,
    updates: Value,
    flags: &[(&str, bool)],
) -> Answer {
    let mut uri = format!(
        "/datastore/{datastore_id}/imageSet/{image_set_id}/updateImageSetMetadata\
         ?latestVersion={latest_version}"
    );
    for (name, value) in flags {
        uri.push_str(&format!("&{name}={value}"));
    }
    emulator.call("POST", &uri, Some(updates)).await
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSetMetadata` — the
/// aggregated document, which is what an update has to be visible in.
async fn get_metadata(emulator: &Emulator, datastore_id: &str, image_set_id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSetMetadata"),
            None,
        )
        .await
}

/// The aggregated document one `GetImageSetMetadata` answer carries.
fn document(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetImageSetMetadata, got {answer:?}"
    );
    assert_eq!(
        answer.content_type, METADATA_CONTENT_TYPE,
        "case {case:?}: expected contentType {METADATA_CONTENT_TYPE:?}, got {:?}",
        answer.content_type
    );
    serde_json::from_slice(&answer.body_bytes).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a JSON document in the blob, got {error} from {answer:?}")
    })
}

/// What `document` holds at `path`, or `Value::Null` when it holds nothing there.
pub(crate) fn at(document: &Value, path: &[&str]) -> Value {
    let mut cursor = document;
    for name in path {
        cursor = &cursor[*name];
    }
    cursor.clone()
}

/// Read the whole document of `image_set_id` back.
pub(crate) async fn read_document(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    case: &str,
) -> Value {
    let answer = get_metadata(emulator, datastore_id, image_set_id).await;
    document(&answer, case)
}

// ---------------------------------------------------------------------------
// 1. What an update writes
// ---------------------------------------------------------------------------

/// One row of the update table.
struct UpdateCase {
    case: &'static str,
    /// The attributes to write, as the document the member carries.
    updatable: Option<Value>,
    /// The attributes to remove, in the same shape.
    removable: Option<Value>,
    force: bool,
    /// Document paths the served document must hold these values at afterwards.
    expected: Vec<(Vec<&'static str>, Value)>,
    /// Document paths the served document must hold NOTHING at afterwards.
    absent: Vec<Vec<&'static str>>,
}

/// The rows span: a modelled attribute at each level the model names one (patient, study,
/// series); both numeric members and text members; several attributes in one document; an
/// attribute the model does NOT name, at the patient level and at one SOP instance's;
/// writing and removing in the same request; and a document that names no attribute at
/// all.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "one modelled patient attribute",
            updatable: Some(json!({
                "SchemaVersion": "1.1",
                "Patient": { "DICOM": { "PatientName": "ROE^RICHARD" } },
            })),
            removable: None,
            force: false,
            expected: vec![
                (
                    vec!["Patient", "DICOM", "PatientName"],
                    json!("ROE^RICHARD"),
                ),
                // Untouched attributes keep what the image set held.
                (vec!["Patient", "DICOM", "PatientID"], json!("PID-1")),
                (vec!["Study", "DICOM", "StudyDescription"], json!("CHEST")),
            ],
            absent: Vec::new(),
        },
        UpdateCase {
            case: "several modelled study attributes, text and both counts",
            updatable: Some(json!({
                "SchemaVersion": "1.1",
                "Study": {
                    "DICOM": {
                        "StudyDescription": "ABDOMEN",
                        "AccessionNumber": "ACC-9",
                        "NumberOfStudyRelatedSeries": 4,
                        "NumberOfStudyRelatedInstances": 17,
                    },
                },
            })),
            removable: None,
            force: false,
            expected: vec![
                (vec!["Study", "DICOM", "StudyDescription"], json!("ABDOMEN")),
                (vec!["Study", "DICOM", "AccessionNumber"], json!("ACC-9")),
                (
                    vec!["Study", "DICOM", "NumberOfStudyRelatedSeries"],
                    json!(4),
                ),
                (
                    vec!["Study", "DICOM", "NumberOfStudyRelatedInstances"],
                    json!(17),
                ),
            ],
            absent: Vec::new(),
        },
        UpdateCase {
            case: "modelled series attributes of the series the image set holds",
            updatable: Some(json!({
                "SchemaVersion": "1.1",
                "Study": {
                    "Series": {
                        SERIES_UID: {
                            "DICOM": { "Modality": "MR", "BodyPartExamined": "HEAD", "SeriesNumber": 9 },
                        },
                    },
                },
            })),
            removable: None,
            force: false,
            expected: vec![
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"],
                    json!("MR"),
                ),
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "BodyPartExamined"],
                    json!("HEAD"),
                ),
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "SeriesNumber"],
                    json!(9),
                ),
                // The series is still the one the image set holds, under its own UID.
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "SeriesInstanceUID"],
                    json!(SERIES_UID),
                ),
            ],
            absent: Vec::new(),
        },
        UpdateCase {
            case: "attributes the model does not name, at the patient level and at one SOP instance",
            updatable: Some(json!({
                "SchemaVersion": "1.1",
                "Patient": { "DICOM": { "PatientAge": "054Y" } },
                "Study": {
                    "Series": {
                        SERIES_UID: {
                            "Instances": {
                                SOP_UID: { "DICOM": { "InstanceNumber": "7" } },
                            },
                        },
                    },
                },
            })),
            removable: None,
            force: false,
            expected: vec![
                (vec!["Patient", "DICOM", "PatientAge"], json!("054Y")),
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
                // The instance the pixel data belongs to still reports its own UID, so an
                // attribute written into that level joined the level rather than replacing
                // it.
                (
                    vec![
                        "Study",
                        "Series",
                        SERIES_UID,
                        "Instances",
                        SOP_UID,
                        "DICOM",
                        "SOPInstanceUID",
                    ],
                    json!(SOP_UID),
                ),
                (vec!["Patient", "DICOM", "PatientName"], json!("DOE^JANE")),
            ],
            absent: Vec::new(),
        },
        UpdateCase {
            case: "a removal of modelled attributes, text and a count",
            updatable: None,
            removable: Some(json!({
                "SchemaVersion": "1.1",
                "Patient": { "DICOM": { "PatientSex": "F" } },
                "Study": { "DICOM": { "StudyDescription": "CHEST", "NumberOfStudyRelatedSeries": 1 } },
            })),
            force: false,
            expected: vec![
                // The model types the two study counts as plain numbers, so a removed one
                // reads back as the zero an image set with no count reports.
                (
                    vec!["Study", "DICOM", "NumberOfStudyRelatedSeries"],
                    json!(0),
                ),
                (vec!["Patient", "DICOM", "PatientID"], json!("PID-1")),
            ],
            absent: vec![
                vec!["Patient", "DICOM", "PatientSex"],
                vec!["Study", "DICOM", "StudyDescription"],
            ],
        },
        UpdateCase {
            case: "one request that writes and removes, modelled and unmodelled",
            updatable: Some(json!({
                "SchemaVersion": "1.1",
                "Patient": { "DICOM": { "PatientAge": "031Y", "PatientName": "LOE^LEE" } },
            })),
            removable: Some(json!({
                "SchemaVersion": "1.1",
                "Patient": { "DICOM": { "PatientBirthDate": "19700101" } },
            })),
            force: false,
            expected: vec![
                (vec!["Patient", "DICOM", "PatientAge"], json!("031Y")),
                (vec!["Patient", "DICOM", "PatientName"], json!("LOE^LEE")),
            ],
            absent: vec![vec!["Patient", "DICOM", "PatientBirthDate"]],
        },
        UpdateCase {
            case: "a force-guarded attribute, with the force flag",
            updatable: Some(json!({
                "SchemaVersion": "1.1",
                "Study": { "DICOM": { "StudyInstanceUID": "1.2.840.113619.2.1.9.77", "StudyID": "STUDY-77" } },
            })),
            removable: None,
            force: true,
            expected: vec![
                (
                    vec!["Study", "DICOM", "StudyInstanceUID"],
                    json!("1.2.840.113619.2.1.9.77"),
                ),
                (vec!["Study", "DICOM", "StudyID"], json!("STUDY-77")),
            ],
            absent: Vec::new(),
        },
        UpdateCase {
            case: "a document that declares its schema and names no attribute",
            updatable: Some(json!({ "SchemaVersion": "1.1" })),
            removable: None,
            force: false,
            expected: vec![
                // The schema declaration is not an attribute: the served document still
                // carries the version the service writes, not one a caller sent.
                (vec!["SchemaVersion"], json!("1.1")),
                (vec!["Patient", "DICOM", "PatientName"], json!("DOE^JANE")),
            ],
            absent: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn an_update_writes_the_attributes_it_names_and_the_document_reports_them() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.update", "token-metadata-update")
        .await;

    for case in update_cases() {
        let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        let before = read_document(&emulator, &datastore_id, &image_set_id, case.case).await;

        let flags: Vec<(&str, bool)> = if case.force {
            vec![("force", true)]
        } else {
            Vec::new()
        };
        let updated = update_metadata(
            &emulator,
            &datastore_id,
            &image_set_id,
            "1",
            dicom_updates(case.updatable.as_ref(), case.removable.as_ref()),
            &flags,
        )
        .await;

        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateImageSetMetadata, got {updated:?}",
            case.case
        );
        assert_eq!(
            updated.string(&["latestVersionId"]),
            "2",
            "case {:?}: expected the update to mint version 2, got {updated:?}",
            case.case
        );
        assert_eq!(
            updated.string(&["imageSetState"]),
            "LOCKED",
            "case {:?}: expected the image set LOCKED while the update is in flight, got \
             {updated:?}",
            case.case
        );
        assert_eq!(
            updated.string(&["imageSetWorkflowStatus"]),
            "UPDATING",
            "case {:?}: expected UPDATING while the update is in flight, got {updated:?}",
            case.case
        );
        assert_eq!(
            updated.string(&["datastoreId"]),
            datastore_id,
            "case {:?}: expected the answer to name the data store, got {updated:?}",
            case.case
        );
        assert_eq!(
            updated.string(&["imageSetId"]),
            image_set_id,
            "case {:?}: expected the answer to name the image set, got {updated:?}",
            case.case
        );
        assert!(
            updated.field(&["message"]).is_null(),
            "case {:?}: expected no failure message on an update that did not fail, got \
             {updated:?}",
            case.case
        );

        // The clock, not a reader, settles the update.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
        let settled = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            (
                settled.string(&["versionId"]),
                settled.string(&["imageSetState"]),
                settled.string(&["imageSetWorkflowStatus"]),
            ),
            (
                "2".to_string(),
                "ACTIVE".to_string(),
                "UPDATED".to_string()
            ),
            "case {:?}: expected version 2 settled to ACTIVE / UPDATED, got {settled:?}",
            case.case
        );
        let forced_reported = settled.field(&["overrides", "forced"]).as_bool();
        assert_eq!(
            forced_reported,
            case.force.then_some(true),
            "case {:?}: expected the forced override {:?}, got {settled:?}",
            case.case,
            case.force.then_some(true)
        );
        // The version the update superseded is still readable, and still reports what it
        // reported before.
        let superseded = get_image_set(&emulator, &datastore_id, &image_set_id, Some("1")).await;
        assert_eq!(
            superseded.string(&["imageSetWorkflowStatus"]),
            "IMPORTED",
            "case {:?}: expected the superseded version to report what it did before, got \
             {superseded:?}",
            case.case
        );

        let after = read_document(&emulator, &datastore_id, &image_set_id, case.case).await;
        for (path, expected) in &case.expected {
            assert_eq!(
                at(&after, path),
                *expected,
                "case {:?}: expected {path:?} to hold {expected}, got {}",
                case.case,
                at(&after, path)
            );
        }
        for path in &case.absent {
            assert!(
                at(&after, path).is_null(),
                "case {:?}: expected {path:?} to hold nothing, got {}",
                case.case,
                at(&after, path)
            );
        }
        // The identifiers the document names never move: an update writes attributes.
        for path in [vec!["DatastoreID"], vec!["ImageSetID"]] {
            assert_eq!(
                at(&after, &path),
                at(&before, &path),
                "case {:?}: expected {path:?} unchanged by an update, got {}",
                case.case,
                at(&after, &path)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. What an update refuses
// ---------------------------------------------------------------------------

/// What one refusal row sends.
enum Wrong {
    /// A `latestVersion` that is not the image set's current latest.
    StaleVersion(&'static str),
    /// An update aimed at a data store this emulator does not hold.
    UnheldDatastore,
    /// An update aimed at an image set the data store does not hold.
    UnheldImageSet,
    /// `updatableAttributes` carrying bytes that are not a document.
    RawUpdatable(&'static str),
    /// Neither attribute document.
    NoDocument,
    /// A document the service can read, sent without the `force` flag it needs.
    UnforcedChange(Value),
    /// A document holding a value of the wrong kind for a modelled attribute.
    MismatchedValue(Value),
    /// More attributes outside the modelled ones than one image set will hold.
    TooManyAttributes,
    /// A revert to a version the image set does not hold.
    RevertUnheld(&'static str),
    /// A revert to the image set's own latest version.
    RevertLatest,
    /// A revert that also asks for the study.
    RevertWithStudy,
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    wrong: Wrong,
    expected_status: u16,
    expected_code: &'static str,
    /// The message the refusal must carry, where the service states it as a constant.
    expected_message: Option<&'static str>,
}

/// The rows span: a stale version; each resource the request names being absent; a
/// document that is not one; a `DICOMUpdates` naming neither document; each of the four
/// attributes the `force` flag exists for, and a private attribute of one SOP instance,
/// changed without it; a text value for a numeric attribute and a number for a text one;
/// one attribute past the bound on what an image set holds; and each way a revert can name
/// no previous version.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "a latestVersion that is not the image set's latest",
            wrong: Wrong::StaleVersion("2"),
            expected_status: 409,
            expected_code: "ConflictException",
            expected_message: Some(IMAGE_SET_VERSION_CONFLICT_MESSAGE),
        },
        RefusalCase {
            case: "a data store this emulator does not hold",
            wrong: Wrong::UnheldDatastore,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(DATASTORE_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "an image set the data store does not hold",
            wrong: Wrong::UnheldImageSet,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "updatableAttributes that is not a JSON document",
            wrong: Wrong::RawUpdatable("PatientName=ROE^RICHARD"),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "updatableAttributes that is JSON but not an object of attributes",
            wrong: Wrong::RawUpdatable("[\"PatientName\"]"),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "DICOMUpdates naming neither document",
            wrong: Wrong::NoDocument,
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "StudyInstanceUID changed without the force flag",
            wrong: Wrong::UnforcedChange(json!({
                "Study": { "DICOM": { "StudyInstanceUID": "1.2.840.113619.2.1.9.77" } },
            })),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "StudyID changed without the force flag",
            wrong: Wrong::UnforcedChange(json!({
                "Study": { "DICOM": { "StudyID": "STUDY-77" } },
            })),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "SeriesInstanceUID changed without the force flag",
            wrong: Wrong::UnforcedChange(json!({
                "Study": {
                    "Series": { SERIES_UID: { "DICOM": { "SeriesInstanceUID": SERIES_UID } } },
                },
            })),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "SOPInstanceUID changed without the force flag",
            wrong: Wrong::UnforcedChange(json!({
                "Study": {
                    "Series": {
                        SERIES_UID: {
                            "Instances": { SOP_UID: { "DICOM": { "SOPInstanceUID": SOP_UID } } },
                        },
                    },
                },
            })),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            // A DICOM tag whose group number is ODD is a private attribute, which the
            // second case the `force` documentation lists covers.
            case: "a private attribute of one SOP instance, without the force flag",
            wrong: Wrong::UnforcedChange(json!({
                "Study": {
                    "Series": {
                        SERIES_UID: {
                            "Instances": { SOP_UID: { "DICOM": { "00291010": "SIEMENS" } } },
                        },
                    },
                },
            })),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "text where a modelled attribute holds a number",
            wrong: Wrong::MismatchedValue(json!({
                "Study": { "DICOM": { "NumberOfStudyRelatedSeries": "four" } },
            })),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "a number where a modelled attribute holds text",
            wrong: Wrong::MismatchedValue(json!({
                "Patient": { "DICOM": { "PatientName": 7 } },
            })),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "more attributes than one image set will hold outside the modelled ones",
            wrong: Wrong::TooManyAttributes,
            expected_status: 402,
            expected_code: "ServiceQuotaExceededException",
            expected_message: None,
        },
        RefusalCase {
            case: "a revert to a version the image set does not hold",
            wrong: Wrong::RevertUnheld("9"),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_VERSION_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "a revert to the image set's own latest version",
            wrong: Wrong::RevertLatest,
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "a revert that also asks for every image set of the study",
            wrong: Wrong::RevertWithStudy,
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
    ]
}

#[tokio::test]
async fn an_update_the_rule_does_not_admit_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.refusal", "token-metadata-refusal")
        .await;
    let patient_name = json!({
        "SchemaVersion": "1.1",
        "Patient": { "DICOM": { "PatientName": "ROE^RICHARD" } },
    });

    for case in refusal_cases() {
        let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        let before = read_document(&emulator, &datastore_id, &image_set_id, case.case).await;

        let (target_datastore, target_image_set, latest_version, updates, flags): (
            String,
            String,
            &str,
            Value,
            Vec<(&str, bool)>,
        ) = match &case.wrong {
            Wrong::StaleVersion(version) => (
                datastore_id.clone(),
                image_set_id.clone(),
                version,
                dicom_updates(Some(&patient_name), None),
                Vec::new(),
            ),
            Wrong::UnheldDatastore => (
                unheld_id(),
                image_set_id.clone(),
                "1",
                dicom_updates(Some(&patient_name), None),
                Vec::new(),
            ),
            Wrong::UnheldImageSet => (
                datastore_id.clone(),
                unheld_id(),
                "1",
                dicom_updates(Some(&patient_name), None),
                Vec::new(),
            ),
            Wrong::RawUpdatable(raw) => (
                datastore_id.clone(),
                image_set_id.clone(),
                "1",
                dicom_updates_raw(Some(raw), None),
                Vec::new(),
            ),
            Wrong::NoDocument => (
                datastore_id.clone(),
                image_set_id.clone(),
                "1",
                dicom_updates_raw(None, None),
                Vec::new(),
            ),
            Wrong::UnforcedChange(document) | Wrong::MismatchedValue(document) => (
                datastore_id.clone(),
                image_set_id.clone(),
                "1",
                dicom_updates(Some(document), None),
                Vec::new(),
            ),
            Wrong::TooManyAttributes => {
                // One attribute past the cap, every one of them an attribute the model
                // does not name, so the document is refused for its size alone.
                let mut attributes = serde_json::Map::new();
                for ordinal in 0..=MAX_IMAGE_SET_EXTRA_ATTRIBUTES {
                    attributes.insert(format!("PrivateAttribute{ordinal}"), json!("held"));
                }
                let document = json!({
                    "SchemaVersion": "1.1",
                    "Patient": { "DICOM": Value::Object(attributes) },
                });
                (
                    datastore_id.clone(),
                    image_set_id.clone(),
                    "1",
                    dicom_updates(Some(&document), None),
                    Vec::new(),
                )
            }
            Wrong::RevertUnheld(version) => (
                datastore_id.clone(),
                image_set_id.clone(),
                "1",
                revert_to(version),
                Vec::new(),
            ),
            Wrong::RevertLatest => (
                datastore_id.clone(),
                image_set_id.clone(),
                "1",
                revert_to("1"),
                Vec::new(),
            ),
            Wrong::RevertWithStudy => (
                datastore_id.clone(),
                image_set_id.clone(),
                "1",
                revert_to("1"),
                vec![("includeStudyImageSets", true)],
            ),
        };

        let refused = update_metadata(
            &emulator,
            &target_datastore,
            &target_image_set,
            latest_version,
            updates,
            &flags,
        )
        .await;

        assert_eq!(
            refused.status, case.expected_status,
            "case {:?}: expected HTTP {}, got {refused:?}",
            case.case, case.expected_status
        );
        assert_eq!(
            refused.error_code, case.expected_code,
            "case {:?}: expected error code {:?}, got {refused:?}",
            case.case, case.expected_code
        );
        if let Some(expected) = case.expected_message {
            assert_eq!(
                refused.string(&["message"]),
                expected,
                "case {:?}: expected the message {expected:?}, got {refused:?}",
                case.case
            );
        }
        // A refusal about a document of DICOM attributes never reflects a VALUE out of it
        // back: the model marks both members `@sensitive`. The attribute's own name is
        // not a value — a refusal states which attribute broke the rule, as every
        // constraint refusal of this service states which member did.
        let reported = refused.string(&["message"]);
        for withheld in ["ROE^RICHARD", "DOE^JANE", "SIEMENS", "four"] {
            assert!(
                !reported.contains(withheld),
                "case {:?}: expected the refusal to withhold the attribute value \
                 {withheld:?}, got {reported:?}",
                case.case
            );
        }

        // Nothing was written: the image set is still at the version and the document it
        // was at.
        let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            (
                read.string(&["versionId"]),
                read.string(&["imageSetState"]),
            ),
            ("1".to_string(), "ACTIVE".to_string()),
            "case {:?}: expected a refused update to leave version 1 ACTIVE, got {read:?}",
            case.case
        );
        let after = read_document(&emulator, &datastore_id, &image_set_id, case.case).await;
        assert_eq!(
            after, before,
            "case {:?}: expected a refused update to leave the document as it was, got {after}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_in_flight_locks_the_image_set_against_another() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.locked", "token-metadata-locked")
        .await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
    let document = json!({
        "SchemaVersion": "1.1",
        "Patient": { "DICOM": { "PatientName": "ROE^RICHARD" } },
    });

    let first = update_metadata(
        &emulator,
        &datastore_id,
        &image_set_id,
        "1",
        dicom_updates(Some(&document), None),
        &[],
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first update: expected HTTP 200, got {first:?}"
    );

    let second = update_metadata(
        &emulator,
        &datastore_id,
        &image_set_id,
        "2",
        dicom_updates(Some(&document), None),
        &[],
    )
    .await;
    assert_eq!(
        (second.status, second.error_code.as_str()),
        (409, "ConflictException"),
        "an update while one is in flight: expected 409 ConflictException, got {second:?}"
    );
    assert_eq!(
        second.string(&["message"]),
        IMAGE_SET_NOT_UPDATABLE_MESSAGE,
        "an update while one is in flight: expected the not-updatable sentence, got {second:?}"
    );

    // Once the clock has settled the first update, the image set takes another.
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
    let third = update_metadata(
        &emulator,
        &datastore_id,
        &image_set_id,
        "2",
        dicom_updates(Some(&document), None),
        &[],
    )
    .await;
    assert_eq!(
        third.status, 200,
        "an update after the first settled: expected HTTP 200, got {third:?}"
    );
    assert_eq!(
        third.string(&["latestVersionId"]),
        "3",
        "an update after the first settled: expected version 3, got {third:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. A revert
// ---------------------------------------------------------------------------

/// How far apart two updates a case orders in time are driven, so the version each one
/// minted is distinguishable.
const APART: Duration = Duration::from_millis(5);

#[tokio::test]
async fn a_revert_puts_back_the_attributes_the_version_it_names_held() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.revert", "token-metadata-revert")
        .await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));

    // Three versions, each holding attributes the one before it did not: a modelled
    // attribute and an attribute the model does not name, so a revert is asserted over
    // both halves of what an image set holds.
    let versions = [
        (
            "2",
            json!({
                "SchemaVersion": "1.1",
                "Patient": { "DICOM": { "PatientName": "ONE^ONE", "PatientAge": "031Y" } },
            }),
        ),
        (
            "3",
            json!({
                "SchemaVersion": "1.1",
                "Patient": { "DICOM": { "PatientName": "TWO^TWO", "PatientAge": "032Y" } },
            }),
        ),
    ];
    for (minted, document) in &versions {
        let answer = update_metadata(
            &emulator,
            &datastore_id,
            &image_set_id,
            &(minted.parse::<u32>().expect("a decimal version") - 1).to_string(),
            dicom_updates(Some(document), None),
            &[],
        )
        .await;
        assert_eq!(
            answer.string(&["latestVersionId"]),
            *minted,
            "building version {minted}: expected it minted, got {answer:?}"
        );
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY + APART).await;
    }

    // Each row reverts to one version and states the attributes that version held: the
    // seeded ones for version 1, the first update's for version 2.
    let rows = [
        (
            "1",
            "4",
            json!("DOE^JANE"),
            Value::Null,
        ),
        ("2", "5", json!("ONE^ONE"), json!("031Y")),
    ];
    for (revert_to_version, minted, expected_name, expected_age) in &rows {
        let latest = (minted.parse::<u32>().expect("a decimal version") - 1).to_string();
        let reverted = update_metadata(
            &emulator,
            &datastore_id,
            &image_set_id,
            &latest,
            revert_to(revert_to_version),
            &[],
        )
        .await;
        assert_eq!(
            reverted.status, 200,
            "revert to {revert_to_version}: expected HTTP 200, got {reverted:?}"
        );
        assert_eq!(
            reverted.string(&["latestVersionId"]),
            *minted,
            "revert to {revert_to_version}: expected version {minted}, got {reverted:?}"
        );
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY + APART).await;

        let case = format!("revert to {revert_to_version}");
        let after = read_document(&emulator, &datastore_id, &image_set_id, &case).await;
        assert_eq!(
            at(&after, &["Patient", "DICOM", "PatientName"]),
            *expected_name,
            "case {case:?}: expected the modelled attribute of version {revert_to_version}, \
             got {}",
            at(&after, &["Patient", "DICOM", "PatientName"])
        );
        assert_eq!(
            at(&after, &["Patient", "DICOM", "PatientAge"]),
            *expected_age,
            "case {case:?}: expected the unmodelled attribute of version \
             {revert_to_version}, got {}",
            at(&after, &["Patient", "DICOM", "PatientAge"])
        );
        // The versions the revert passed over are still there, so a later revert can name
        // one of them.
        for held in 1..=minted.parse::<u32>().expect("a decimal version") {
            let read =
                get_image_set(&emulator, &datastore_id, &image_set_id, Some(&held.to_string()))
                    .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected version {held} still readable, got {read:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. The study
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_study_flag_decides_which_image_sets_one_update_reaches() {
    // The rows differ along the flag: asking for the study, and not asking for it.
    for (case, include_study) in [("the study", true), ("one image set", false)] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore(
                &format!("metadata.study.{include_study}"),
                &format!("token-metadata-study-{include_study}"),
            )
            .await;
        let other_study_uid = "1.2.840.113619.2.1.9.2";
        let named = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        let sibling = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        let outsider = seed_image_set(&emulator, &datastore_id, seeded_tags(other_study_uid));

        let document = json!({
            "SchemaVersion": "1.1",
            "Patient": { "DICOM": { "PatientName": "ROE^RICHARD" } },
        });
        let updated = update_metadata(
            &emulator,
            &datastore_id,
            &named,
            "1",
            dicom_updates(Some(&document), None),
            &[("includeStudyImageSets", include_study)],
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200, got {updated:?}"
        );

        // The image set the request named is always the one the answer reports.
        assert_eq!(
            (
                updated.string(&["imageSetId"]),
                updated.string(&["latestVersionId"]),
                updated.string(&["imageSetWorkflowStatus"]),
            ),
            (named.clone(), "2".to_string(), "UPDATING".to_string()),
            "case {case:?}: expected the named image set at version 2 UPDATING, got {updated:?}"
        );

        let sibling_read = get_image_set(&emulator, &datastore_id, &sibling, None).await;
        if include_study {
            assert_eq!(
                (
                    sibling_read.string(&["versionId"]),
                    sibling_read.string(&["imageSetState"]),
                    sibling_read.string(&["imageSetWorkflowStatus"]),
                ),
                (
                    "2".to_string(),
                    "LOCKED".to_string(),
                    "UPDATING_FOR_STUDY_CONSISTENCY".to_string()
                ),
                "case {case:?}: expected the other image set of the study brought into line, \
                 got {sibling_read:?}"
            );
        } else {
            assert_eq!(
                sibling_read.string(&["versionId"]),
                "1",
                "case {case:?}: expected the other image set of the study untouched, got \
                 {sibling_read:?}"
            );
        }

        // An image set of ANOTHER study is never reached.
        let outsider_read = get_image_set(&emulator, &datastore_id, &outsider, None).await;
        assert_eq!(
            outsider_read.string(&["versionId"]),
            "1",
            "case {case:?}: expected an image set of another study untouched, got \
             {outsider_read:?}"
        );

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
        let sibling_document = read_document(&emulator, &datastore_id, &sibling, case).await;
        let expected_sibling_name = if include_study {
            json!("ROE^RICHARD")
        } else {
            json!("DOE^JANE")
        };
        assert_eq!(
            at(&sibling_document, &["Patient", "DICOM", "PatientName"]),
            expected_sibling_name,
            "case {case:?}: expected the other image set of the study to report \
             {expected_sibling_name}, got {}",
            at(&sibling_document, &["Patient", "DICOM", "PatientName"])
        );
        let outsider_document = read_document(&emulator, &datastore_id, &outsider, case).await;
        assert_eq!(
            at(&outsider_document, &["Patient", "DICOM", "PatientName"]),
            json!("DOE^JANE"),
            "case {case:?}: expected an image set of another study to report what it held, \
             got {}",
            at(&outsider_document, &["Patient", "DICOM", "PatientName"])
        );
        if include_study {
            let settled = get_image_set(&emulator, &datastore_id, &sibling, None).await;
            assert_eq!(
                (
                    settled.string(&["imageSetState"]),
                    settled.string(&["imageSetWorkflowStatus"]),
                ),
                ("ACTIVE".to_string(), "UPDATED".to_string()),
                "case {case:?}: expected the study's other image set settled to ACTIVE / \
                 UPDATED, got {settled:?}"
            );
        }
    }
}
