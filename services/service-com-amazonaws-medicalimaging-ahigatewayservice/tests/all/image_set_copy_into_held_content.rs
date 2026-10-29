// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a `CopyImageSet` into a destination that ALREADY HOLDS DICOM
//! content leaves that destination holding.
//!
//! RULE: a copy DELIVERS content into its destination item by item — pixel data under the
//! image frame id each frame is stored by, an attribute at its own path in the aggregated
//! metadata document — so once the copy has landed the destination holds what the copy
//! carried TOGETHER WITH everything of its own the copy carried nothing for: an attribute
//! only the copy has is delivered, an attribute both hold reads back as the copy's, and an
//! attribute only the destination has stays. That holds over both halves of an image set's
//! attribute state — the attributes the model names and the attributes kept at the document
//! path they were written at — and at every level of the document, and it is the premise the
//! copy's own consistency check rests on: an element only one of the two sides holds is no
//! disagreement because neither side loses it.
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: which
//! attributes the destination holds that the copy carries nothing for (patient-level text,
//! a study COUNT, an optional series number, an attribute the model does not name, a
//! private attribute of one SOP instance), which attributes the two hold in common with
//! equal and with differing values, whether the request is forced, whether the copy carries
//! the whole source or a SUBSET of its SOP instances, and a destination holding nothing at
//! all. Every case drives real HTTP through the production front door, so what is asserted
//! is the wire behaviour a `medical-imaging` SDK client sees.
//!
//! Because the delivery ADDS, the copy is also a writer of the attributes outside the
//! modelled ones, so the last suite pins the bound every writer of them answers to
//! (`MAX_IMAGE_SET_EXTRA_ATTRIBUTES`): a copy whose delivery would leave the destination
//! holding more than that is refused with 402 `ServiceQuotaExceededException` and writes
//! nothing, and one landing exactly on the bound completes.
//!
//! ## Why a seeded source and destination
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds both image sets through the crate's public `state` module with the
//! same `ImageSet::new` / `ImageFrame` the handlers use — through the sibling suite's
//! `seed_image_set_holding` — and then asserts only over the wire.

use serde_json::{json, Map, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    extra_attributes_quota_message, IMAGE_SET_COPY_DELAY, IMAGE_SET_UPDATE_DELAY,
    MAX_IMAGE_SET_EXTRA_ATTRIBUTES,
};

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_frames::{get_image_frame, pixel_data};
use crate::image_set_copy::{copy_image_set, get_image_set, information};
use crate::image_set_copy_subsets::{
    seed_image_set_holding, selector, FrameSeed, INSTANCE_ATTRIBUTE, INSTANCE_ONE, INSTANCE_THREE,
    INSTANCE_TWO, SERIES_UID,
};
use crate::image_set_metadata::{document, get_image_set_metadata};
use crate::image_set_metadata_updates::update_metadata;

/// An attribute the model does not name that BOTH image sets hold, so a case can see which
/// of the two values the destination reports once the copy has landed. DICOM's Ethnic Group
/// keyword, outside the attributes `DICOMTags` names
/// (`smithy_query(query="shape",shape="DICOMTags")`).
const SHARED_ATTRIBUTE: &str = "EthnicGroup";

/// An attribute the model does not name that only the DESTINATION holds — DICOM's Patient
/// Comments keyword — so what a copy carries nothing for is observable.
const DESTINATION_ATTRIBUTE: &str = "PatientComments";

/// A PRIVATE attribute of one SOP instance the destination holds: DICOM reserves every odd
/// group number for private attributes, and the aggregated metadata document spells such an
/// attribute with its tag — the four hexadecimal digits of the group followed by the four of
/// the element.
const DESTINATION_INSTANCE_TAG: &str = "00091001";

/// The value each side holds for [`SHARED_ATTRIBUTE`].
const SOURCE_SHARED_VALUE: &str = "SOURCE-GROUP";
const DESTINATION_SHARED_VALUE: &str = "HELD-GROUP";

/// What the destination holds for [`DESTINATION_ATTRIBUTE`] and for its own instance's
/// private attribute.
const DESTINATION_COMMENT: &str = "HELD-COMMENT";
const DESTINATION_INSTANCE_VALUE: &str = "private-held";

/// The document paths the cases read, each named by the attribute that sits there. The
/// series levels are keyed BY the Series Instance UID, and a SOP instance sits under
/// `Study` → `Series` → series UID → `Instances` → SOP Instance UID.
const PATIENT_ID: &[&str] = &["Patient", "DICOM", "PatientID"];
const PATIENT_NAME: &[&str] = &["Patient", "DICOM", "PatientName"];
const PATIENT_BIRTH_DATE: &[&str] = &["Patient", "DICOM", "PatientBirthDate"];
const PATIENT_SHARED: &[&str] = &["Patient", "DICOM", SHARED_ATTRIBUTE];
const PATIENT_HELD_ONLY: &[&str] = &["Patient", "DICOM", DESTINATION_ATTRIBUTE];
const STUDY_INSTANCE_UID: &[&str] = &["Study", "DICOM", "StudyInstanceUID"];
const STUDY_DESCRIPTION: &[&str] = &["Study", "DICOM", "StudyDescription"];
const STUDY_RELATED_SERIES: &[&str] = &["Study", "DICOM", "NumberOfStudyRelatedSeries"];
const STUDY_RELATED_INSTANCES: &[&str] = &["Study", "DICOM", "NumberOfStudyRelatedInstances"];
const SERIES_MODALITY: &[&str] = &["Study", "Series", SERIES_UID, "DICOM", "Modality"];
const SERIES_NUMBER: &[&str] = &["Study", "Series", SERIES_UID, "DICOM", "SeriesNumber"];
const INSTANCE_ONE_ATTRIBUTE: &[&str] = &[
    "Study",
    "Series",
    SERIES_UID,
    "Instances",
    INSTANCE_ONE,
    "DICOM",
    INSTANCE_ATTRIBUTE,
];
const INSTANCE_TWO_ATTRIBUTE: &[&str] = &[
    "Study",
    "Series",
    SERIES_UID,
    "Instances",
    INSTANCE_TWO,
    "DICOM",
    INSTANCE_ATTRIBUTE,
];
const INSTANCE_THREE_ATTRIBUTE: &[&str] = &[
    "Study",
    "Series",
    SERIES_UID,
    "Instances",
    INSTANCE_THREE,
    "DICOM",
    DESTINATION_INSTANCE_TAG,
];

/// The modelled attributes the SOURCE of every case holds: some of the ones the model names
/// and not others, so a case can tell what the copy delivers from what it carries nothing
/// for. The values satisfy the model's own constraints (`DICOMStudyInstanceUID` /
/// `DICOMSeriesInstanceUID` are `^[0-9.]+$`).
fn source_tags() -> DICOMTags {
    DICOMTags {
        dicom_patient_id: Some("PID-7".to_string()),
        dicom_patient_name: Some("Doe^Source".to_string()),
        dicom_study_instance_uid: Some("1.2.840.113619.2.5".to_string()),
        dicom_number_of_study_related_instances: 2,
        dicom_series_instance_uid: Some(SERIES_UID.to_string()),
        dicom_series_modality: Some("CT".to_string()),
        ..DICOMTags::default()
    }
}

/// The modelled attributes the DESTINATION of the content-holding rows holds going in: two
/// the source holds the same values for, and four the source holds nothing for at all — one
/// text attribute at the patient level, one at the study level, a study COUNT and the
/// optional series number.
fn destination_tags() -> DICOMTags {
    DICOMTags {
        dicom_patient_id: Some("PID-7".to_string()),
        dicom_series_instance_uid: Some(SERIES_UID.to_string()),
        dicom_patient_birth_date: Some("19260216".to_string()),
        dicom_study_description: Some("HELD-CHEST".to_string()),
        dicom_number_of_study_related_series: 4,
        dicom_series_number: Some(9),
        ..DICOMTags::default()
    }
}

/// The frames the source holds: one each of two SOP instances.
fn source_frames() -> Vec<FrameSeed> {
    vec![
        FrameSeed {
            sop_instance_uid: Some(INSTANCE_ONE),
            pixel_len: 16,
        },
        FrameSeed {
            sop_instance_uid: Some(INSTANCE_TWO),
            pixel_len: 32,
        },
    ]
}

/// The frames a content-holding destination holds: one of a THIRD SOP instance, which no
/// copy in this suite carries.
fn destination_frames() -> Vec<FrameSeed> {
    vec![FrameSeed {
        sop_instance_uid: Some(INSTANCE_THREE),
        pixel_len: 8,
    }]
}

/// The attributes the model does not name that the source holds, nested exactly as the
/// aggregated metadata document nests them: one at the patient level and one per SOP
/// instance.
fn source_extra_attributes() -> Map<String, Value> {
    let mut held = Map::new();
    held.insert(
        "Patient".to_string(),
        json!({ "DICOM": { SHARED_ATTRIBUTE: SOURCE_SHARED_VALUE } }),
    );
    held.insert(
        "Study".to_string(),
        json!({ "Series": { SERIES_UID: { "Instances": {
            INSTANCE_ONE: { "DICOM": { INSTANCE_ATTRIBUTE: "1" } },
            INSTANCE_TWO: { "DICOM": { INSTANCE_ATTRIBUTE: "2" } },
        } } } }),
    );
    held
}

/// The same for the destination: the attribute both sides hold, one only it holds, and a
/// private attribute of its own SOP instance — the per-instance attributes a metadata
/// update writes into an image set.
fn destination_extra_attributes() -> Map<String, Value> {
    let mut held = Map::new();
    held.insert(
        "Patient".to_string(),
        json!({ "DICOM": {
            SHARED_ATTRIBUTE: DESTINATION_SHARED_VALUE,
            DESTINATION_ATTRIBUTE: DESTINATION_COMMENT,
        } }),
    );
    held.insert(
        "Study".to_string(),
        json!({ "Series": { SERIES_UID: { "Instances": {
            INSTANCE_THREE: { "DICOM": { DESTINATION_INSTANCE_TAG: DESTINATION_INSTANCE_VALUE } },
        } } } }),
    );
    held
}

/// One attribute a case reads back: the path the aggregated metadata document holds it at,
/// and the value the destination must report there once the copy has landed — `None` where
/// the document must report nothing at all.
struct Expected {
    path: &'static [&'static str],
    value: Option<Value>,
}

/// An attribute the destination must report `value` for.
fn reports(path: &'static [&'static str], value: Value) -> Expected {
    Expected {
        path,
        value: Some(value),
    }
}

/// An attribute neither side holds, so the document reports nothing there rather than a
/// place with nothing in it.
fn reports_nothing(path: &'static [&'static str]) -> Expected {
    Expected { path, value: None }
}

/// What `document` reports at `path`, or `None` when it reports nothing there.
fn reported_at(document: &Value, path: &[&str]) -> Option<Value> {
    let mut at = document;
    for name in path {
        at = at.get(name)?;
    }
    Some(at.clone())
}

/// One row of the delivery table.
struct MergeCase {
    case: &'static str,
    /// The modelled attributes the destination holds going in.
    destination_tags: DICOMTags,
    /// The attributes kept as written the destination holds going in.
    destination_extra: Map<String, Value>,
    /// The frames the destination holds going in.
    destination_frames: Vec<FrameSeed>,
    /// `sourceImageSet.DICOMCopies.copiableAttributes`, for a copy restricted to a subset
    /// of the source's SOP instances.
    copiable_attributes: Option<String>,
    /// Whether the request carries `force`.
    force: bool,
    /// The source's SOP instances this copy delivers — whose attributes and pixel data
    /// reach the destination.
    copied_instances: Vec<&'static str>,
    /// What the destination must report once the copy has landed.
    expected: Vec<Expected>,
}

/// The rows differ along: a destination holding attributes of its own that the copy carries
/// nothing for; a destination disagreeing with the copy about one attribute, which takes
/// `force`; a destination holding nothing at all; and a SUBSET copy into a destination that
/// holds content.
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            case: "a destination holding attributes the copy carries nothing for",
            destination_tags: destination_tags(),
            destination_extra: destination_extra_attributes(),
            destination_frames: destination_frames(),
            copiable_attributes: None,
            force: false,
            copied_instances: vec![INSTANCE_ONE, INSTANCE_TWO],
            expected: vec![
                // Both sides hold it, with the same value.
                reports(PATIENT_ID, json!("PID-7")),
                // Only the copy holds it: delivered.
                reports(PATIENT_NAME, json!("Doe^Source")),
                reports(STUDY_INSTANCE_UID, json!("1.2.840.113619.2.5")),
                reports(STUDY_RELATED_INSTANCES, json!(2)),
                reports(SERIES_MODALITY, json!("CT")),
                reports(INSTANCE_ONE_ATTRIBUTE, json!("1")),
                reports(INSTANCE_TWO_ATTRIBUTE, json!("2")),
                // Only the destination holds it: kept.
                reports(PATIENT_BIRTH_DATE, json!("19260216")),
                reports(STUDY_DESCRIPTION, json!("HELD-CHEST")),
                reports(STUDY_RELATED_SERIES, json!(4)),
                reports(SERIES_NUMBER, json!(9)),
                reports(PATIENT_HELD_ONLY, json!(DESTINATION_COMMENT)),
                reports(INSTANCE_THREE_ATTRIBUTE, json!(DESTINATION_INSTANCE_VALUE)),
                // Both hold it at the same path, with different values: the copy delivers
                // its own over it.
                reports(PATIENT_SHARED, json!(SOURCE_SHARED_VALUE)),
            ],
        },
        MergeCase {
            case: "a destination disagreeing about two elements, forced",
            destination_tags: DICOMTags {
                dicom_patient_name: Some("Doe^Held".to_string()),
                dicom_series_modality: Some("MR".to_string()),
                ..destination_tags()
            },
            destination_extra: destination_extra_attributes(),
            destination_frames: destination_frames(),
            copiable_attributes: None,
            force: true,
            copied_instances: vec![INSTANCE_ONE, INSTANCE_TWO],
            expected: vec![
                // The copy's value lands on the elements the two disagreed about.
                reports(PATIENT_NAME, json!("Doe^Source")),
                reports(SERIES_MODALITY, json!("CT")),
                // Forcing the copy loses nothing else the destination held.
                reports(PATIENT_BIRTH_DATE, json!("19260216")),
                reports(STUDY_DESCRIPTION, json!("HELD-CHEST")),
                reports(STUDY_RELATED_SERIES, json!(4)),
                reports(SERIES_NUMBER, json!(9)),
                reports(PATIENT_HELD_ONLY, json!(DESTINATION_COMMENT)),
                reports(INSTANCE_THREE_ATTRIBUTE, json!(DESTINATION_INSTANCE_VALUE)),
                reports(INSTANCE_ONE_ATTRIBUTE, json!("1")),
            ],
        },
        MergeCase {
            case: "a destination nothing has delivered content to: it reports what was copied and nothing besides",
            destination_tags: DICOMTags::default(),
            destination_extra: Map::new(),
            destination_frames: Vec::new(),
            copiable_attributes: None,
            force: false,
            copied_instances: vec![INSTANCE_ONE, INSTANCE_TWO],
            expected: vec![
                reports(PATIENT_ID, json!("PID-7")),
                reports(PATIENT_NAME, json!("Doe^Source")),
                reports(PATIENT_SHARED, json!(SOURCE_SHARED_VALUE)),
                reports(STUDY_RELATED_INSTANCES, json!(2)),
                reports(INSTANCE_ONE_ATTRIBUTE, json!("1")),
                reports(INSTANCE_TWO_ATTRIBUTE, json!("2")),
                // Nothing stored either side's attribute here, so nothing is reported —
                // and the two study counts are modelled as plain numbers, so a count
                // nothing stored reads back as zero.
                reports_nothing(PATIENT_BIRTH_DATE),
                reports_nothing(PATIENT_HELD_ONLY),
                reports_nothing(SERIES_NUMBER),
                reports_nothing(INSTANCE_THREE_ATTRIBUTE),
                reports(STUDY_RELATED_SERIES, json!(0)),
            ],
        },
        MergeCase {
            case: "a subset copy into a destination holding content",
            destination_tags: destination_tags(),
            destination_extra: destination_extra_attributes(),
            destination_frames: destination_frames(),
            copiable_attributes: Some(selector(SERIES_UID, &[INSTANCE_ONE])),
            force: false,
            copied_instances: vec![INSTANCE_ONE],
            expected: vec![
                // The instance the selector named is delivered; the one it left out is
                // not carried, so nothing reports it.
                reports(INSTANCE_ONE_ATTRIBUTE, json!("1")),
                reports_nothing(INSTANCE_TWO_ATTRIBUTE),
                // The destination's own instance is untouched by either.
                reports(INSTANCE_THREE_ATTRIBUTE, json!(DESTINATION_INSTANCE_VALUE)),
                // The patient, study and series attributes travel with whatever was
                // copied, and the destination's own stay.
                reports(PATIENT_NAME, json!("Doe^Source")),
                reports(PATIENT_SHARED, json!(SOURCE_SHARED_VALUE)),
                reports(PATIENT_BIRTH_DATE, json!("19260216")),
                reports(STUDY_DESCRIPTION, json!("HELD-CHEST")),
                reports(STUDY_RELATED_SERIES, json!(4)),
                reports(SERIES_NUMBER, json!(9)),
            ],
        },
    ]
}

#[tokio::test]
async fn a_copy_delivers_its_content_into_the_destination_without_erasing_the_destination_s_own() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.merge", "token-copy-merge")
        .await;

    for case in merge_cases() {
        // A fresh source and destination per row: a copy locks both image sets, so no row
        // inherits another's locks.
        let (source_id, source_frame_ids) = seed_image_set_holding(
            &emulator,
            &datastore_id,
            source_tags(),
            &source_frames(),
            source_extra_attributes(),
        );
        let (destination_id, destination_frame_ids) = seed_image_set_holding(
            &emulator,
            &datastore_id,
            case.destination_tags.clone(),
            &case.destination_frames,
            case.destination_extra.clone(),
        );

        let query: Vec<(&str, bool)> = if case.force {
            vec![("force", true)]
        } else {
            Vec::new()
        };
        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information(
                "1",
                case.copiable_attributes.as_deref(),
                Some((destination_id.as_str(), "1")),
            ),
            &query,
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {:?}: expected HTTP 200 from CopyImageSet, got {copied:?}",
            case.case
        );
        assert_eq!(
            copied.string(&["destinationImageSetProperties", "imageSetId"]),
            destination_id,
            "case {:?}: the copy lands on the destination the request named, got {copied:?}",
            case.case
        );

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

        let landed = get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await;
        let landed_document = document(&landed, case.case);
        for Expected { path, value } in &case.expected {
            let reported = reported_at(&landed_document, path);
            assert_eq!(
                reported.as_ref(),
                value.as_ref(),
                "case {:?}: expected {path:?} to report {value:?}, got {reported:?} in \
                 {landed_document:?}",
                case.case
            );
        }

        // The PIXEL DATA follows the same rule: the destination's own frames are still
        // served from it, each under the image frame id it was stored by, and the copied
        // frames are served beside them.
        for (index, (frame_id, frame)) in destination_frame_ids
            .iter()
            .zip(&case.destination_frames)
            .enumerate()
        {
            let served = get_image_frame(&emulator, &datastore_id, &destination_id, frame_id).await;
            assert_eq!(
                served.status, 200,
                "case {:?}: the destination's own frame {index} must still be served, got \
                 {served:?}",
                case.case
            );
            assert_eq!(
                served.body_bytes,
                pixel_data(index as u8, frame.pixel_len),
                "case {:?}: expected the destination's own frame {index} byte for byte, got {} \
                 bytes",
                case.case,
                served.body_bytes.len()
            );
        }
        for (index, (frame_id, frame)) in source_frame_ids.iter().zip(&source_frames()).enumerate()
        {
            let carried = frame
                .sop_instance_uid
                .is_some_and(|uid| case.copied_instances.contains(&uid));
            let served = get_image_frame(&emulator, &datastore_id, &destination_id, frame_id).await;
            let expected_status = if carried { 200 } else { 404 };
            assert_eq!(
                served.status, expected_status,
                "case {:?}: the copied frame {index} of {:?} must answer {expected_status}, got \
                 {served:?}",
                case.case, frame.sop_instance_uid
            );
            if carried {
                assert_eq!(
                    served.body_bytes,
                    pixel_data(index as u8, frame.pixel_len),
                    "case {:?}: expected the copied frame {index} byte for byte, got {} bytes",
                    case.case,
                    served.body_bytes.len()
                );
            }
        }
    }
}

/// A copy lands as a NEW version of its destination, and the version it superseded keeps
/// the attributes the destination held going in — so a REVERT to that version puts exactly
/// those back, without any attribute the copy delivered.
#[tokio::test]
async fn a_revert_puts_back_what_the_destination_held_before_the_copy() {
    const CASE: &str = "a revert to the version the copy superseded";

    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.merge.versions", "token-copy-merge-versions")
        .await;
    let (source_id, _) = seed_image_set_holding(
        &emulator,
        &datastore_id,
        source_tags(),
        &source_frames(),
        source_extra_attributes(),
    );
    let (destination_id, _) = seed_image_set_holding(
        &emulator,
        &datastore_id,
        destination_tags(),
        &destination_frames(),
        destination_extra_attributes(),
    );

    let copied = copy_image_set(
        &emulator,
        &datastore_id,
        &source_id,
        information("1", None, Some((destination_id.as_str(), "1"))),
        &[],
    )
    .await;
    assert_eq!(
        copied.status, 200,
        "case {CASE:?}: expected HTTP 200 from CopyImageSet, got {copied:?}"
    );
    assert_eq!(
        copied.string(&["destinationImageSetProperties", "latestVersionId"]),
        "2",
        "case {CASE:?}: the copy lands as the destination's second version, got {copied:?}"
    );

    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

    let landed = document(
        &get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await,
        CASE,
    );
    for (path, expected) in [
        (PATIENT_BIRTH_DATE, json!("19260216")),
        (PATIENT_SHARED, json!(SOURCE_SHARED_VALUE)),
        (PATIENT_NAME, json!("Doe^Source")),
        (STUDY_INSTANCE_UID, json!("1.2.840.113619.2.5")),
    ] {
        let reported = reported_at(&landed, path);
        assert_eq!(
            reported.as_ref(),
            Some(&expected),
            "case {CASE:?}: the version the copy landed on must report {path:?} as {expected:?}, \
             got {reported:?} in {landed:?}"
        );
    }

    let reverted = update_metadata(
        &emulator,
        &datastore_id,
        &destination_id,
        "2",
        json!({ "revertToVersionId": "1" }),
        &[],
    )
    .await;
    assert_eq!(
        reverted.status, 200,
        "case {CASE:?}: expected HTTP 200 from the revert, got {reverted:?}"
    );

    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;

    let before_the_copy = document(
        &get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await,
        CASE,
    );
    for (path, expected) in [
        // What the destination held going in is back, both halves of it.
        (PATIENT_BIRTH_DATE, Some(json!("19260216"))),
        (PATIENT_SHARED, Some(json!(DESTINATION_SHARED_VALUE))),
        (PATIENT_HELD_ONLY, Some(json!(DESTINATION_COMMENT))),
        (
            INSTANCE_THREE_ATTRIBUTE,
            Some(json!(DESTINATION_INSTANCE_VALUE)),
        ),
        // And nothing the copy delivered is: that version held none of it.
        (PATIENT_NAME, None),
        (STUDY_INSTANCE_UID, None),
        (INSTANCE_ONE_ATTRIBUTE, None),
    ] {
        let reported = reported_at(&before_the_copy, path);
        assert_eq!(
            reported.as_ref(),
            expected.as_ref(),
            "case {CASE:?}: the revert must report {path:?} as {expected:?}, got {reported:?} in \
             {before_the_copy:?}"
        );
    }
}

/// `count` attributes the model does not name, written at the patient level under distinct
/// PRIVATE tags of the group `group`: DICOM reserves every odd group number for private
/// attributes, and the aggregated metadata document spells such an attribute with its tag,
/// so two image sets given different groups hold attributes at wholly distinct paths and the
/// count a delivery leaves behind is the sum of the two.
fn private_patient_attributes(group: u16, count: usize) -> Map<String, Value> {
    let mut dicom = Map::new();
    for element in 0..count {
        dicom.insert(format!("{group:04x}{element:04x}"), json!("private"));
    }
    let mut held = Map::new();
    if !dicom.is_empty() {
        held.insert(
            "Patient".to_string(),
            json!({ "DICOM": Value::Object(dicom) }),
        );
    }
    held
}

/// How many attributes a document reports at the patient level — the leaves of that level,
/// which is what the bound counts.
fn reported_patient_attributes(document: &Value) -> usize {
    document["Patient"]["DICOM"]
        .as_object()
        .map(serde_json::Map::len)
        .unwrap_or_default()
}

/// One row of the attribute-bound table: how many attributes each side holds going in, and
/// whether the copy is admitted.
struct BoundCase {
    case: &'static str,
    source_attributes: usize,
    destination_attributes: usize,
    admitted: bool,
}

/// The rows stand on each side of the published bound: a delivery filling it on its own, two
/// sides meeting exactly on it, one attribute past it, and both sides full.
fn bound_cases() -> Vec<BoundCase> {
    let half = MAX_IMAGE_SET_EXTRA_ATTRIBUTES / 2;
    vec![
        BoundCase {
            case: "a delivery filling the bound into a destination holding none",
            source_attributes: MAX_IMAGE_SET_EXTRA_ATTRIBUTES,
            destination_attributes: 0,
            admitted: true,
        },
        BoundCase {
            case: "two sides meeting exactly on the bound",
            source_attributes: half,
            destination_attributes: MAX_IMAGE_SET_EXTRA_ATTRIBUTES - half,
            admitted: true,
        },
        BoundCase {
            case: "one attribute past the bound",
            source_attributes: half,
            destination_attributes: MAX_IMAGE_SET_EXTRA_ATTRIBUTES - half + 1,
            admitted: false,
        },
        BoundCase {
            case: "both sides holding the whole bound of their own",
            source_attributes: MAX_IMAGE_SET_EXTRA_ATTRIBUTES,
            destination_attributes: MAX_IMAGE_SET_EXTRA_ATTRIBUTES,
            admitted: false,
        },
    ]
}

/// Delivering ADDS, so the copy is a writer of the attributes outside the modelled ones and
/// answers to the same bound the metadata update does: a copy that would leave its
/// destination holding more than the bound is refused before anything is written.
#[tokio::test]
async fn a_delivery_past_the_attribute_bound_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.merge.bound", "token-copy-merge-bound")
        .await;

    for case in bound_cases() {
        let (source_id, _) = seed_image_set_holding(
            &emulator,
            &datastore_id,
            DICOMTags::default(),
            &[],
            private_patient_attributes(0x0009, case.source_attributes),
        );
        let (destination_id, _) = seed_image_set_holding(
            &emulator,
            &datastore_id,
            DICOMTags::default(),
            &[],
            private_patient_attributes(0x0011, case.destination_attributes),
        );

        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information("1", None, Some((destination_id.as_str(), "1"))),
            &[],
        )
        .await;

        if !case.admitted {
            assert_eq!(
                copied.status, 402,
                "case {:?}: a delivery past the bound must be refused, got {copied:?}",
                case.case
            );
            assert_eq!(
                copied.error_code, "ServiceQuotaExceededException",
                "case {:?}: expected ServiceQuotaExceededException, got {copied:?}",
                case.case
            );
            assert_eq!(
                copied.string(&["message"]),
                extra_attributes_quota_message(),
                "case {:?}: expected the attribute-bound text, got {copied:?}",
                case.case
            );
            // The refusal wrote nothing: the destination still holds the one version it
            // had going in, and the attributes it held.
            let untouched = get_image_set(&emulator, &datastore_id, &destination_id, None).await;
            assert_eq!(
                untouched.string(&["versionId"]),
                "1",
                "case {:?}: the refused copy must leave the destination at its own version, \
                 got {untouched:?}",
                case.case
            );
            let held = document(
                &get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await,
                case.case,
            );
            assert_eq!(
                reported_patient_attributes(&held),
                case.destination_attributes,
                "case {:?}: the refused copy must leave the destination's attributes alone, \
                 got {} of them",
                case.case,
                reported_patient_attributes(&held)
            );
            continue;
        }

        assert_eq!(
            copied.status, 200,
            "case {:?}: a delivery within the bound must be admitted, got {copied:?}",
            case.case
        );
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
        let landed = document(
            &get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await,
            case.case,
        );
        let expected = case.source_attributes + case.destination_attributes;
        assert_eq!(
            reported_patient_attributes(&landed),
            expected,
            "case {:?}: expected the destination to report {expected} attributes once the copy \
             had landed, got {}",
            case.case,
            reported_patient_attributes(&landed)
        );
    }
}
