// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a `CopyImageSet` CARRIES into its destination.
//!
//! RULE: a copy delivers the DICOM content of its source to the destination — the image
//! frames and the attributes the source holds — and where the request supplies
//! `sourceImageSet.DICOMCopies.copiableAttributes`, the copy is RESTRICTED to the SOP
//! instances that document names: an aggregated metadata document places one instance at
//! `Study` → `Series` → series UID → `Instances` → SOP Instance UID, and only the
//! instances named there reach the destination — their pixel data and their attributes,
//! and nothing of the instances the document leaves out. The attributes of the patient,
//! the study and the series travel with whatever is copied, because the copied instances
//! still belong to them. And `force` is what its documentation says it is: without it a
//! copy into an image set whose patient, study or series level metadata elements disagree
//! with the copied content's is refused with 409 `ConflictException`; with it the copy
//! completes.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how many
//! SOP instances the selector names and which, whether it locates them under the series
//! the source holds or another, whether it names an instance the source does not hold,
//! whether it names none at all, what JSON form it takes, and — for the force suite —
//! which metadata element disagrees and whether only one of the two image sets holds it.
//! Every case drives real HTTP through the production front door, so what is asserted is
//! the wire behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded source
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its source through the crate's public `state` module with the same
//! `ImageSet::new` / `ImageFrame` the handlers use, and then asserts only over the wire.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Map, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageFrame, ImageSet, ImageSetState, ImageSetWorkflowStatus,
    IMAGE_FRAME_NOT_FOUND_MESSAGE, IMAGE_SET_COPY_DELAY,
    IMAGE_SET_COPY_INCONSISTENT_METADATA_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_frames::{get_image_frame, pixel_data};
use crate::image_set_copy::{copy_image_set, information};
use crate::image_set_metadata::{document, get_image_set_metadata};

/// The Series Instance UID the seeded source holds — the series its instances sit in.
pub(crate) const SERIES_UID: &str = "1.2.840.113619.2.1.1";

/// A Series Instance UID the seeded source does NOT hold, used to prove a selector
/// locates an instance by the instance's own UID: a SOP Instance UID identifies the
/// instance, whichever series of the document it is written under.
const OTHER_SERIES_UID: &str = "1.2.840.113619.2.9.9";

/// The three SOP instances the seeded source holds pixel data for.
pub(crate) const INSTANCE_ONE: &str = "1.2.840.113619.2.1.1.1";
pub(crate) const INSTANCE_TWO: &str = "1.2.840.113619.2.1.1.2";
pub(crate) const INSTANCE_THREE: &str = "1.2.840.113619.2.1.1.3";

/// A SOP Instance UID no seeded image set holds.
const UNHELD_INSTANCE: &str = "1.2.840.113619.2.1.1.404";

/// The attribute keyword the seeded per-instance attribute is written under. DICOM's
/// Instance Number (0020,0013), which the service's own `DICOMTags` does not name, so it
/// is persisted at the document path it was written at — which is what makes an
/// instance's ATTRIBUTES, and not only its pixel data, observable at the destination.
pub(crate) const INSTANCE_ATTRIBUTE: &str = "InstanceNumber";

/// One frame to seed: the SOP instance its pixel data came from (`None` for pixel data
/// stored under no instance) and how many bytes it holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameSeed {
    pub(crate) sop_instance_uid: Option<&'static str>,
    pub(crate) pixel_len: usize,
}

/// The frames the source of every copy suite holds: two of one instance, one each of two
/// more, and one stored under no instance at all.
fn source_frames() -> Vec<FrameSeed> {
    vec![
        FrameSeed {
            sop_instance_uid: Some(INSTANCE_ONE),
            pixel_len: 16,
        },
        FrameSeed {
            sop_instance_uid: Some(INSTANCE_ONE),
            pixel_len: 8,
        },
        FrameSeed {
            sop_instance_uid: Some(INSTANCE_TWO),
            pixel_len: 32,
        },
        FrameSeed {
            sop_instance_uid: Some(INSTANCE_THREE),
            pixel_len: 1,
        },
        FrameSeed {
            sop_instance_uid: None,
            pixel_len: 4,
        },
    ]
}

/// The modelled DICOM attributes the seeded source holds, spelled with values the model's
/// own constraints admit (`DICOMStudyInstanceUID` / `DICOMSeriesInstanceUID` are
/// `^[0-9.]+$`, `DICOMStudyDate` a date and `DICOMStudyTime` the documented
/// `HHmmss.FFFFFF` form).
fn source_tags() -> DICOMTags {
    DICOMTags {
        dicom_patient_id: Some("PID-31".to_string()),
        dicom_patient_name: Some("Doe^Jane".to_string()),
        dicom_patient_birth_date: Some("19260216".to_string()),
        dicom_patient_sex: Some("F".to_string()),
        dicom_study_instance_uid: Some("1.2.840.113619.2.1".to_string()),
        dicom_study_id: Some("STU-31".to_string()),
        dicom_study_description: Some("CHEST".to_string()),
        dicom_number_of_study_related_series: 1,
        dicom_number_of_study_related_instances: 3,
        dicom_accession_number: Some("ACC-31".to_string()),
        dicom_series_instance_uid: Some(SERIES_UID.to_string()),
        dicom_series_modality: Some("CT".to_string()),
        dicom_series_body_part: Some("CHEST".to_string()),
        dicom_series_number: Some(3),
        dicom_study_date: Some("20240115".to_string()),
        dicom_study_time: Some("103000.000000".to_string()),
    }
}

/// The attributes the model does NOT name that the seeded source holds, nested exactly as
/// the aggregated metadata document nests them: one attribute per SOP instance, and one at
/// the patient level, so a case can tell apart what a subset copy keeps from what it drops.
fn source_extra_attributes(instances: &[&str]) -> Map<String, Value> {
    let mut held = Map::new();
    for (number, uid) in instances.iter().enumerate() {
        held.insert(
            (*uid).to_string(),
            json!({ "DICOM": { INSTANCE_ATTRIBUTE: (number + 1).to_string() } }),
        );
    }
    let mut document = Map::new();
    document.insert(
        "Patient".to_string(),
        json!({ "DICOM": { "EthnicGroup": "UNKNOWN" } }),
    );
    document.insert(
        "Study".to_string(),
        json!({ "Series": { SERIES_UID: { "Instances": Value::Object(held) } } }),
    );
    document
}

/// Seed one `ACTIVE` image set holding `tags`, `extra_attributes` and one frame per entry
/// of `frames`, and answer its identifier beside the minted identifier of each frame, in
/// the order given.
///
/// Built with the crate's own `ImageSet::new` and `ImageFrame`, and every identifier minted
/// by the same `mint_id` the handlers mint with, so a seeded image set is shaped exactly
/// like one a handler holds.
pub(crate) fn seed_image_set_holding(
    emulator: &Emulator,
    datastore_id: &str,
    tags: DICOMTags,
    frames: &[FrameSeed],
    extra_attributes: Map<String, Value>,
) -> (String, Vec<String>) {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let frame_ids: Vec<String> = frames.iter().map(|_| store.mint_id()).collect();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let mut image_set = ImageSet::new(
        image_set_id.clone(),
        image_set_arn(&datastore.datastore_arn, &image_set_id),
        ImageSetState::Active,
        ImageSetWorkflowStatus::Imported,
        false,
        false,
        SystemTime::now(),
    );
    image_set.dicom_tags = tags;
    image_set.extra_attributes = extra_attributes;
    for (index, (frame_id, frame)) in frame_ids.iter().zip(frames).enumerate() {
        image_set.image_frames.insert(
            frame_id.clone(),
            ImageFrame {
                image_frame_id: frame_id.clone(),
                sop_instance_uid: frame.sop_instance_uid.map(str::to_string),
                transfer_syntax_uid: Some("1.2.840.10008.1.2.1".to_string()),
                pixel_data: pixel_data(index as u8, frame.pixel_len),
            },
        );
    }
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    (image_set_id, frame_ids)
}

/// Seed the source every subset case copies from.
fn seed_source(emulator: &Emulator, datastore_id: &str) -> (String, Vec<String>) {
    seed_image_set_holding(
        emulator,
        datastore_id,
        source_tags(),
        &source_frames(),
        source_extra_attributes(&[INSTANCE_ONE, INSTANCE_TWO, INSTANCE_THREE]),
    )
}

/// The SOP instances `document` reports, read out of the series it reports them under.
fn reported_instances(document: &Value, series_uid: &str) -> BTreeSet<String> {
    document["Study"]["Series"][series_uid]["Instances"]
        .as_object()
        .map(|instances| instances.keys().cloned().collect())
        .unwrap_or_default()
}

/// One row of the subset table.
struct SubsetCase {
    case: &'static str,
    /// The `copiableAttributes` document the request carries, or `None` for a copy of the
    /// whole image set.
    copiable_attributes: Option<String>,
    /// The SOP instances the destination must end up holding.
    expected_instances: Vec<&'static str>,
}

/// A `copiableAttributes` document naming `instances` under `series_uid`, written in the
/// shape of the aggregated metadata document.
pub(crate) fn selector(series_uid: &str, instances: &[&str]) -> String {
    let mut named = Map::new();
    for uid in instances {
        named.insert((*uid).to_string(), json!({}));
    }
    json!({
        "SchemaVersion": "1.1",
        "Study": { "Series": { series_uid: { "Instances": Value::Object(named) } } },
    })
    .to_string()
}

/// The rows differ along: no selector at all; a selector naming one, two and all three of
/// the source's instances; a selector naming an instance under a series the source does
/// not hold; one naming an instance nothing holds; one naming no instance at all; and one
/// that is a JSON array rather than a metadata document.
fn subset_cases() -> Vec<SubsetCase> {
    vec![
        SubsetCase {
            case: "no DICOMCopies: the whole image set is copied",
            copiable_attributes: None,
            expected_instances: vec![INSTANCE_ONE, INSTANCE_TWO, INSTANCE_THREE],
        },
        SubsetCase {
            case: "one instance named: only that instance is copied",
            copiable_attributes: Some(selector(SERIES_UID, &[INSTANCE_ONE])),
            expected_instances: vec![INSTANCE_ONE],
        },
        SubsetCase {
            case: "two instances named: both are copied and the third is not",
            copiable_attributes: Some(selector(SERIES_UID, &[INSTANCE_TWO, INSTANCE_THREE])),
            expected_instances: vec![INSTANCE_TWO, INSTANCE_THREE],
        },
        SubsetCase {
            case: "every instance named: the same instances a whole copy carries",
            copiable_attributes: Some(selector(
                SERIES_UID,
                &[INSTANCE_ONE, INSTANCE_TWO, INSTANCE_THREE],
            )),
            expected_instances: vec![INSTANCE_ONE, INSTANCE_TWO, INSTANCE_THREE],
        },
        SubsetCase {
            case: "an instance named under another series: located by its own UID",
            copiable_attributes: Some(selector(OTHER_SERIES_UID, &[INSTANCE_TWO])),
            expected_instances: vec![INSTANCE_TWO],
        },
        SubsetCase {
            case: "an instance nothing holds: nothing is copied",
            copiable_attributes: Some(selector(SERIES_UID, &[UNHELD_INSTANCE])),
            expected_instances: Vec::new(),
        },
        SubsetCase {
            case: "a document naming no instance: nothing is copied",
            copiable_attributes: Some(
                json!({ "SchemaVersion": "1.1", "Study": { "Series": {} } }).to_string(),
            ),
            expected_instances: Vec::new(),
        },
        SubsetCase {
            case: "a JSON array: a document that names no instance",
            copiable_attributes: Some(json!([{ "SOPInstanceUID": INSTANCE_ONE }]).to_string()),
            expected_instances: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_copy_delivers_exactly_the_sop_instances_the_subset_names() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.subset", "token-copy-subset")
        .await;

    for case in subset_cases() {
        let (source_id, frame_ids) = seed_source(&emulator, &datastore_id);
        let frames = source_frames();

        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information("1", case.copiable_attributes.as_deref(), None),
            &[],
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {:?}: expected HTTP 200 from CopyImageSet, got {copied:?}",
            case.case
        );
        let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

        let landed =
            get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await;
        let landed_document = document(&landed, case.case);

        // The instances the destination reports are exactly the ones the copy carried.
        let expected: BTreeSet<String> = case
            .expected_instances
            .iter()
            .map(|uid| (*uid).to_string())
            .collect();
        assert_eq!(
            reported_instances(&landed_document, SERIES_UID),
            expected,
            "case {:?}: expected the destination to hold instances {expected:?}, got \
             {landed_document:?}",
            case.case
        );

        // Each copied instance brings the attributes written under it; an instance left
        // behind brings none, so its level is absent rather than empty.
        for (number, uid) in [INSTANCE_ONE, INSTANCE_TWO, INSTANCE_THREE]
            .iter()
            .enumerate()
        {
            let reported = landed_document["Study"]["Series"][SERIES_UID]["Instances"][*uid]
                ["DICOM"][INSTANCE_ATTRIBUTE]
                .as_str();
            let wanted = case
                .expected_instances
                .contains(uid)
                .then(|| (number + 1).to_string());
            assert_eq!(
                reported.map(str::to_string),
                wanted,
                "case {:?}: expected instance {uid} to report {wanted:?} for \
                 {INSTANCE_ATTRIBUTE}, got {landed_document:?}",
                case.case
            );
        }

        // The patient, study and series attributes travel with whatever was copied: the
        // copied instances still belong to that patient, study and series.
        assert_eq!(
            landed_document["Patient"]["DICOM"]["PatientID"].as_str(),
            source_tags().dicom_patient_id.as_deref(),
            "case {:?}: expected the patient attributes to travel with the copy, got \
             {landed_document:?}",
            case.case
        );
        assert_eq!(
            landed_document["Patient"]["DICOM"]["EthnicGroup"].as_str(),
            Some("UNKNOWN"),
            "case {:?}: expected a patient attribute the model does not name to travel too, \
             got {landed_document:?}",
            case.case
        );
        assert_eq!(
            landed_document["Study"]["DICOM"]["StudyInstanceUID"].as_str(),
            source_tags().dicom_study_instance_uid.as_deref(),
            "case {:?}: expected the study attributes to travel with the copy, got \
             {landed_document:?}",
            case.case
        );
        assert_eq!(
            landed_document["Study"]["Series"][SERIES_UID]["DICOM"]["SeriesInstanceUID"].as_str(),
            Some(SERIES_UID),
            "case {:?}: expected the series the content belongs to, got {landed_document:?}",
            case.case
        );

        // The PIXEL DATA follows the same rule, frame by frame: a frame of a copied
        // instance is served from the destination byte for byte, and one of an instance
        // the copy left behind is not there at all. A whole copy carries every frame,
        // including pixel data stored under no instance; a subset copy names instances, so
        // pixel data belonging to none of them is not named either.
        for (index, (frame_id, frame)) in frame_ids.iter().zip(&frames).enumerate() {
            let carried = match (&case.copiable_attributes, frame.sop_instance_uid) {
                (None, _) => true,
                (Some(_), Some(uid)) => case.expected_instances.contains(&uid),
                (Some(_), None) => false,
            };
            let served = get_image_frame(&emulator, &datastore_id, &destination_id, frame_id).await;
            if carried {
                assert_eq!(
                    served.status, 200,
                    "case {:?}: expected frame {index} of {:?} at the destination, got \
                     {served:?}",
                    case.case, frame.sop_instance_uid
                );
                assert_eq!(
                    served.body_bytes,
                    pixel_data(index as u8, frame.pixel_len),
                    "case {:?}: expected frame {index} byte for byte, got {} bytes",
                    case.case,
                    served.body_bytes.len()
                );
            } else {
                assert_eq!(
                    served.status, 404,
                    "case {:?}: frame {index} of {:?} was not copied, so the destination \
                     holds no pixel data under it, got {served:?}",
                    case.case, frame.sop_instance_uid
                );
                assert_eq!(
                    served.string(&["message"]),
                    IMAGE_FRAME_NOT_FOUND_MESSAGE,
                    "case {:?}: expected the image-frame-not-found text, got {served:?}",
                    case.case
                );
            }
        }
    }
}

/// A subset copy and a whole copy of the SAME source are distinguishable at the
/// destination: the subset selection is what the copy carried, so the two destinations
/// report different content.
#[tokio::test]
async fn a_subset_copy_and_a_whole_copy_of_one_source_differ() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.subset.differs", "token-copy-subset-differs")
        .await;
    let (source_id, _) = seed_source(&emulator, &datastore_id);

    let mut documents: Vec<(&str, Value)> = Vec::new();
    for (case, attributes) in [
        ("the whole image set", None),
        (
            "a subset of one instance",
            Some(selector(SERIES_UID, &[INSTANCE_ONE])),
        ),
    ] {
        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information("1", attributes.as_deref(), None),
            &[],
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: expected HTTP 200 from CopyImageSet, got {copied:?}"
        );
        let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);
        // The source is LOCKED read-only while a copy runs, so the next copy waits for the
        // clock to release it — which is also when this copy's content lands.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
        let landed =
            get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await;
        documents.push((case, document(&landed, case)));
    }

    let whole = reported_instances(&documents[0].1, SERIES_UID);
    let subset = reported_instances(&documents[1].1, SERIES_UID);
    assert_eq!(
        whole,
        BTreeSet::from([
            INSTANCE_ONE.to_string(),
            INSTANCE_TWO.to_string(),
            INSTANCE_THREE.to_string(),
        ]),
        "case {:?}: a whole copy carries every instance, got {:?}",
        documents[0].0,
        documents[0].1
    );
    assert_eq!(
        subset,
        BTreeSet::from([INSTANCE_ONE.to_string()]),
        "case {:?}: a subset copy carries the instance it named, got {:?}",
        documents[1].0,
        documents[1].1
    );
    assert_ne!(
        whole, subset,
        "case \"two copies of one source\": a subset copy and a whole copy must be \
         distinguishable, got {whole:?} against {subset:?}"
    );
}

/// One row of the force table: how the destination's metadata differs from the source's.
struct ForceCase {
    case: &'static str,
    /// The destination's modelled attributes going in.
    destination_tags: DICOMTags,
    /// Whether the copy is admitted without `force`.
    consistent: bool,
}

/// The rows span: a destination holding no DICOM content at all; one holding exactly the
/// source's elements; a disagreement at the patient, the study and the series level; a
/// disagreement in a study COUNT; and an element only one of the two image sets holds,
/// which is no disagreement because the copy supplies it.
fn force_cases() -> Vec<ForceCase> {
    vec![
        ForceCase {
            case: "a destination nothing has delivered content to",
            destination_tags: DICOMTags::default(),
            consistent: true,
        },
        ForceCase {
            case: "a destination holding exactly the source's elements",
            destination_tags: source_tags(),
            consistent: true,
        },
        ForceCase {
            case: "a destination holding only elements the source does not",
            destination_tags: DICOMTags {
                dicom_patient_id: None,
                dicom_patient_name: None,
                dicom_patient_birth_date: None,
                dicom_patient_sex: None,
                dicom_study_instance_uid: None,
                dicom_study_id: None,
                dicom_study_description: None,
                dicom_accession_number: None,
                dicom_series_instance_uid: None,
                dicom_series_modality: None,
                dicom_series_body_part: None,
                dicom_series_number: None,
                dicom_study_date: None,
                dicom_study_time: None,
                dicom_number_of_study_related_series: 0,
                dicom_number_of_study_related_instances: 0,
            },
            consistent: true,
        },
        ForceCase {
            case: "a patient level element disagreeing",
            destination_tags: DICOMTags {
                dicom_patient_id: Some("PID-99".to_string()),
                ..source_tags()
            },
            consistent: false,
        },
        ForceCase {
            case: "a study level element disagreeing",
            destination_tags: DICOMTags {
                dicom_study_instance_uid: Some("1.2.840.113619.2.99".to_string()),
                ..source_tags()
            },
            consistent: false,
        },
        ForceCase {
            case: "a series level element disagreeing",
            destination_tags: DICOMTags {
                dicom_series_modality: Some("MR".to_string()),
                ..source_tags()
            },
            consistent: false,
        },
        ForceCase {
            case: "a study count disagreeing",
            destination_tags: DICOMTags {
                dicom_number_of_study_related_instances: 9,
                ..source_tags()
            },
            consistent: false,
        },
        ForceCase {
            case: "a series number disagreeing",
            destination_tags: DICOMTags {
                dicom_series_number: Some(11),
                ..source_tags()
            },
            consistent: false,
        },
    ]
}

/// Copy `source_id` into `destination_id`, with `force` only when asked for.
async fn copy_into(
    emulator: &Emulator,
    datastore_id: &str,
    source_id: &str,
    destination_id: &str,
    forced: bool,
) -> Answer {
    let query: Vec<(&str, bool)> = if forced {
        vec![("force", true)]
    } else {
        Vec::new()
    };
    copy_image_set(
        emulator,
        datastore_id,
        source_id,
        information("1", None, Some((destination_id, "1"))),
        &query,
    )
    .await
}

#[tokio::test]
async fn inconsistent_metadata_is_refused_unless_the_copy_is_forced() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("copy.force", "token-copy-force")
        .await;

    for case in force_cases() {
        // A fresh source and destination per row: a copy locks both image sets, so no row
        // inherits another's locks.
        let (source_id, _) = seed_source(&emulator, &datastore_id);
        let (destination_id, _) = seed_image_set_holding(
            &emulator,
            &datastore_id,
            case.destination_tags.clone(),
            &[],
            Map::new(),
        );

        let unforced = copy_into(
            &emulator,
            &datastore_id,
            &source_id,
            &destination_id,
            false,
        )
        .await;
        if case.consistent {
            assert_eq!(
                unforced.status, 200,
                "case {:?}: nothing disagrees, so the copy needs no force, got {unforced:?}",
                case.case
            );
            tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
            continue;
        }

        assert_eq!(
            unforced.status, 409,
            "case {:?}: expected the unforced copy to be refused, got {unforced:?}",
            case.case
        );
        assert_eq!(
            unforced.error_code, "ConflictException",
            "case {:?}: expected ConflictException, got {unforced:?}",
            case.case
        );
        assert_eq!(
            unforced.string(&["message"]),
            IMAGE_SET_COPY_INCONSISTENT_METADATA_MESSAGE,
            "case {:?}: expected the inconsistent-metadata text, got {unforced:?}",
            case.case
        );

        // The refusal wrote nothing: the destination still holds the version it had going
        // in, so the forced copy that follows is the FIRST one to reach it.
        let forced = copy_into(&emulator, &datastore_id, &source_id, &destination_id, true).await;
        assert_eq!(
            forced.status, 200,
            "case {:?}: force must complete the copy anyway, got {forced:?}",
            case.case
        );
        assert_eq!(
            forced.string(&["destinationImageSetProperties", "latestVersionId"]),
            "2",
            "case {:?}: the forced copy lands as the destination's second version, got \
             {forced:?}",
            case.case
        );
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
    }
}
