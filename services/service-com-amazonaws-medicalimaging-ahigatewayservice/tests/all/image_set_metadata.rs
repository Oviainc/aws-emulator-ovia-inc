// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetImageSetMetadata`.
//!
//! RULE: `GetImageSetMetadata` answers HTTP 200 with the AGGREGATED DICOM metadata
//! document of the image set the request names — a JSON document written into the blob
//! as it stands, with `contentType` stating the format its own response member documents
//! this operation returns the study metadata in and
//! `contentEncoding` absent because nothing compressed it — and every value in that
//! document comes from persisted state: the patient, study and series attributes the
//! image set holds, under the DICOM keyword of each, and the image frames it holds,
//! placed under the SOP instance each one's pixel data came from. An attribute nothing
//! has stored is
//! absent from the document rather than invented, so an image set nothing has delivered
//! DICOM content to reports the hierarchy with no attributes and no series at all.
//! `version` names one version of the image set, whose own attributes the document then
//! reports (`image_set_metadata_versions` is that rule's suite), and a version it does not
//! hold is 404
//! `ResourceNotFoundException`, as are a data store or image set the service does not
//! hold and an image set whose content went with its deletion; an identifier outside its
//! modelled pattern is 400 `ValidationException`. Serving the metadata is an ACCESS of
//! the content the image set holds, so it moves `lastAccessedAt` — and a call that served
//! no document moves nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! attributes are stored (every one, a patient only, none at all), how many frames the
//! image set holds, how those frames are spread over SOP instances (one instance, two,
//! pixel data stored under none), whether the series carries a number and a modality,
//! what the study counts are, how many versions the image set holds and which one is
//! asked for, and which part of a request cannot be served. Every case drives real HTTP
//! through the production front door, so what is asserted is the wire behaviour a
//! `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image set through the crate's public `state` module with the
//! same `ImageSet::new` / `ImageFrame` the handlers use, and then asserts only over the
//! wire. The one path that writes DICOM content today is a copy, which carries its
//! source's; the last suite drives that over HTTP end to end.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageFrame, ImageSet, ImageSetState, ImageSetWorkflowStatus,
    DATASTORE_NOT_FOUND_MESSAGE, IMAGE_SET_COPY_DELAY, IMAGE_SET_NOT_FOUND_MESSAGE,
    IMAGE_SET_VERSION_NOT_FOUND_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_frames::pixel_data;
use crate::image_set_copy::{copy_image_set, get_image_set, information};

/// The `contentType` the document is served with: the format the response member's own
/// documentation names
/// (`smithy_query(query="doc",shape="GetImageSetMetadataResponse",member="contentType")` —
/// "Default is `text/plain`"). `crate::content_type_reporting` is that rule's suite.
const EXPECTED_CONTENT_TYPE: &str = "text/plain";

/// The `contentEncoding` the document is served with: the compression format AWS
/// HealthImaging returns the aggregated metadata blob in, which is why the response
/// declares one at all. `crate::content_type_reporting` is that rule's suite.
const EXPECTED_CONTENT_ENCODING: &str = "gzip";

/// The format version the aggregated metadata document carries, asserted as the literal
/// the document must state rather than through the handler's own constant.
const EXPECTED_SCHEMA_VERSION: &str = "1.1";

/// How far apart two events a case orders in time are driven, so the instant each one
/// records is distinguishable on the wire from the one before it.
const APART: Duration = Duration::from_millis(25);

/// The slack allowed when a reported instant is bounded by instants the test read off the
/// same clock.
const SLACK_SECS: f64 = 1e-3;

/// A well-formed `[0-9a-z]{32}` identifier no call ever mints (`mint_id` is SplitMix64
/// output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// `POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSetMetadata` —
/// `GetImageSetMetadata`, optionally for one version (`version` is an `@httpQuery`).
pub(crate) async fn get_image_set_metadata(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    version_id: Option<&str>,
) -> Answer {
    let mut uri = format!("/datastore/{datastore_id}/imageSet/{image_set_id}/getImageSetMetadata");
    if let Some(version_id) = version_id {
        uri.push_str(&format!("?version={version_id}"));
    }
    emulator.call("POST", &uri, None).await
}

/// The document one successful answer carries: the blob read as the JSON document
/// `contentType` says it is, after asserting what the answer says the blob holds.
pub(crate) fn document(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetImageSetMetadata, got {answer:?}"
    );
    assert_eq!(
        answer.content_type, EXPECTED_CONTENT_TYPE,
        "case {case:?}: expected contentType {EXPECTED_CONTENT_TYPE:?}, got {:?}",
        answer.content_type
    );
    // The blob is served COMPRESSED, as AWS returns it, and the answer names the format
    // it is in — a `contentEncoding` must name the compression that was really applied.
    assert_eq!(
        answer.content_encoding, EXPECTED_CONTENT_ENCODING,
        "case {case:?}: expected contentEncoding {EXPECTED_CONTENT_ENCODING:?}, got {:?}",
        answer.content_encoding
    );
    let decoded = String::from_utf8(answer.body_bytes.clone()).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected a UTF-8 document, got {error} over {} bytes",
            answer.body_bytes.len()
        )
    });
    let parsed: Value = serde_json::from_str(&decoded).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a JSON document, got {error} from {decoded:?}")
    });
    assert_eq!(
        parsed["SchemaVersion"].as_str(),
        Some(EXPECTED_SCHEMA_VERSION),
        "case {case:?}: expected the document to state its schema version, got {parsed:?}"
    );
    parsed
}

/// Assert that `object` carries EXACTLY the attributes the expectation names: each stored
/// one under its DICOM keyword with the stored value, each number with its number, and
/// nothing else at all — an attribute nothing stored must be absent, not null.
fn assert_attributes(
    case: &str,
    path: &str,
    object: &Value,
    text: &[(&str, Option<&str>)],
    numbers: &[(&str, i64)],
) {
    let map = object.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected {path} to be an object, got {object:?}")
    });
    for &(keyword, expected) in text {
        match expected {
            Some(value) => assert_eq!(
                map.get(keyword).and_then(Value::as_str),
                Some(value),
                "case {case:?}: expected {path}.{keyword} to be {value:?}, got {object:?}"
            ),
            None => assert!(
                !map.contains_key(keyword),
                "case {case:?}: nothing stored {path}.{keyword}, so it must be absent, got \
                 {object:?}"
            ),
        }
    }
    for &(keyword, expected) in numbers {
        assert_eq!(
            map.get(keyword).and_then(Value::as_i64),
            Some(expected),
            "case {case:?}: expected {path}.{keyword} to be {expected}, got {object:?}"
        );
    }
    let expected_keys: BTreeSet<&str> = text
        .iter()
        .filter_map(|&(keyword, value)| value.map(|_| keyword))
        .chain(numbers.iter().map(|&(keyword, _)| keyword))
        .collect();
    let actual_keys: BTreeSet<&str> = map.keys().map(String::as_str).collect();
    assert_eq!(
        actual_keys, expected_keys,
        "case {case:?}: {path} must carry exactly the stored attributes, got {object:?}"
    );
}

/// One frame to seed: the SOP instance its pixel data came from (`None` for pixel data
/// stored without one), the transfer syntax it is stored in, and how many bytes it holds.
#[derive(Debug, Clone)]
struct FrameSeed {
    sop_instance_uid: Option<&'static str>,
    transfer_syntax_uid: Option<&'static str>,
    pixel_len: usize,
}

/// One row of the document table: the DICOM attributes and frames to seed, and how many
/// versions the image set holds.
#[derive(Debug, Clone)]
struct DocumentCase {
    case: &'static str,
    tags: DICOMTags,
    frames: Vec<FrameSeed>,
    versions: u32,
}

/// Seed one image set holding `case`'s DICOM attributes and frames, and answer its
/// identifier beside the minted identifier of each frame, in the order given.
fn seed(emulator: &Emulator, datastore_id: &str, case: &DocumentCase) -> (String, Vec<String>) {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let frame_ids: Vec<String> = case.frames.iter().map(|_| store.mint_id()).collect();
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
    // The attributes before the versions: `push_version` snapshots what the image set
    // holds AT THAT INSTANT onto the version it supersedes, so seeding this way gives every
    // version this image set holds the content the case names — which is the content each
    // of them must then report.
    image_set.dicom_tags = case.tags.clone();
    for _ in 1..case.versions {
        image_set.push_version(
            ImageSetState::Active,
            ImageSetWorkflowStatus::Updated,
            false,
            SystemTime::now(),
        );
    }
    for (frame_id, frame) in frame_ids.iter().zip(&case.frames) {
        image_set.image_frames.insert(
            frame_id.clone(),
            ImageFrame {
                image_frame_id: frame_id.clone(),
                sop_instance_uid: frame.sop_instance_uid.map(str::to_string),
                transfer_syntax_uid: frame.transfer_syntax_uid.map(str::to_string),
                pixel_data: pixel_data(7, frame.pixel_len),
            },
        );
    }
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    (image_set_id, frame_ids)
}

/// The DICOM attributes of one patient and study, spelled with the values the model's
/// own constraints admit (`DICOMStudyInstanceUID` / `DICOMSeriesInstanceUID` are
/// `^[0-9.]+$`, `DICOMStudyDate` a date and `DICOMStudyTime` the `HHmmss.FFFFFF` form
/// the model documents).
fn full_tags() -> DICOMTags {
    DICOMTags {
        dicom_patient_id: Some("PID-7".to_string()),
        dicom_patient_name: Some("Doe^Jane".to_string()),
        dicom_patient_birth_date: Some("19260216".to_string()),
        dicom_patient_sex: Some("F".to_string()),
        dicom_study_instance_uid: Some("1.2.840.113619.2.1".to_string()),
        dicom_study_id: Some("STU-7".to_string()),
        dicom_study_description: Some("CHEST".to_string()),
        dicom_number_of_study_related_series: 2,
        dicom_number_of_study_related_instances: 5,
        dicom_accession_number: Some("ACC-7".to_string()),
        dicom_series_instance_uid: Some("1.2.840.113619.2.1.1".to_string()),
        dicom_series_modality: Some("CT".to_string()),
        dicom_series_body_part: Some("CHEST".to_string()),
        dicom_series_number: Some(3),
        dicom_study_date: Some("20240115".to_string()),
        dicom_study_time: Some("103000.000000".to_string()),
    }
}

/// The rows differ along which attributes are stored, how many frames the image set
/// holds, how those frames are spread over SOP instances, whether the series carries a
/// number, modality and body part, what the two study counts are, and how many versions
/// the image set holds.
fn document_cases() -> Vec<DocumentCase> {
    vec![
        DocumentCase {
            case: "every attribute stored, three frames of one instance",
            tags: full_tags(),
            frames: vec![
                FrameSeed {
                    sop_instance_uid: Some("1.2.840.113619.2.1.1.1"),
                    transfer_syntax_uid: Some("1.2.840.10008.1.2.4.50"),
                    pixel_len: 16,
                },
                FrameSeed {
                    sop_instance_uid: Some("1.2.840.113619.2.1.1.1"),
                    transfer_syntax_uid: Some("1.2.840.10008.1.2.4.50"),
                    pixel_len: 32,
                },
                FrameSeed {
                    sop_instance_uid: Some("1.2.840.113619.2.1.1.1"),
                    transfer_syntax_uid: None,
                    pixel_len: 1,
                },
            ],
            versions: 1,
        },
        DocumentCase {
            case: "four frames split over two instances, one stored under none",
            tags: full_tags(),
            frames: vec![
                FrameSeed {
                    sop_instance_uid: Some("1.2.840.113619.2.1.1.2"),
                    transfer_syntax_uid: Some("1.2.840.10008.1.2.4.202"),
                    pixel_len: 8,
                },
                FrameSeed {
                    sop_instance_uid: Some("1.2.840.113619.2.1.1.1"),
                    transfer_syntax_uid: Some("1.2.840.10008.1.2.4.202"),
                    pixel_len: 24,
                },
                FrameSeed {
                    sop_instance_uid: Some("1.2.840.113619.2.1.1.2"),
                    transfer_syntax_uid: Some("1.2.840.10008.1.2.1"),
                    pixel_len: 48,
                },
                FrameSeed {
                    sop_instance_uid: None,
                    transfer_syntax_uid: None,
                    pixel_len: 4,
                },
            ],
            versions: 3,
        },
        DocumentCase {
            case: "a series with no number, modality or body part, holding no frames",
            tags: DICOMTags {
                dicom_series_number: None,
                dicom_series_modality: None,
                dicom_series_body_part: None,
                ..full_tags()
            },
            frames: Vec::new(),
            versions: 2,
        },
        DocumentCase {
            case: "patient attributes only: no study, no series, no frames",
            tags: DICOMTags {
                dicom_patient_id: Some("PID-8".to_string()),
                dicom_patient_name: Some("Roe^Richard".to_string()),
                ..DICOMTags::default()
            },
            frames: Vec::new(),
            versions: 1,
        },
        DocumentCase {
            case: "nothing delivered: no attribute stored at all, one orphan frame",
            tags: DICOMTags::default(),
            frames: vec![FrameSeed {
                sop_instance_uid: Some("1.2.840.113619.2.1.1.9"),
                transfer_syntax_uid: None,
                pixel_len: 12,
            }],
            versions: 1,
        },
        DocumentCase {
            case: "study counts past one series, a study with no description",
            tags: DICOMTags {
                dicom_study_description: None,
                dicom_number_of_study_related_series: 7,
                dicom_number_of_study_related_instances: 113,
                dicom_series_number: Some(0),
                ..full_tags()
            },
            frames: vec![FrameSeed {
                sop_instance_uid: Some("1.2.840.113619.2.1.1.3"),
                transfer_syntax_uid: Some("1.2.840.10008.1.2.4.91"),
                pixel_len: 64,
            }],
            versions: 4,
        },
    ]
}

/// Assert `answer`'s document is the one `case` seeded, member for member: the two
/// identifiers, the patient and study attributes under their DICOM keywords, and the
/// series with the instances the stored pixel data belongs to.
fn assert_document(
    answer: &Answer,
    case: &DocumentCase,
    datastore_id: &str,
    image_set_id: &str,
    frame_ids: &[String],
) {
    let parsed = document(answer, case.case);
    let tags = &case.tags;
    assert_eq!(
        parsed["DatastoreID"].as_str(),
        Some(datastore_id),
        "case {:?}: expected the document to name its data store, got {parsed:?}",
        case.case
    );
    assert_eq!(
        parsed["ImageSetID"].as_str(),
        Some(image_set_id),
        "case {:?}: expected the document to name its image set, got {parsed:?}",
        case.case
    );

    // The patient attributes, under the DICOM keyword of each stored member.
    assert_attributes(
        case.case,
        "Patient.DICOM",
        &parsed["Patient"]["DICOM"],
        &[
            ("PatientID", tags.dicom_patient_id.as_deref()),
            ("PatientName", tags.dicom_patient_name.as_deref()),
            ("PatientBirthDate", tags.dicom_patient_birth_date.as_deref()),
            ("PatientSex", tags.dicom_patient_sex.as_deref()),
        ],
        &[],
    );

    // The study attributes, with the two counts always reported because they are always
    // stored.
    assert_attributes(
        case.case,
        "Study.DICOM",
        &parsed["Study"]["DICOM"],
        &[
            ("StudyInstanceUID", tags.dicom_study_instance_uid.as_deref()),
            ("StudyID", tags.dicom_study_id.as_deref()),
            ("StudyDescription", tags.dicom_study_description.as_deref()),
            ("StudyDate", tags.dicom_study_date.as_deref()),
            ("StudyTime", tags.dicom_study_time.as_deref()),
            ("AccessionNumber", tags.dicom_accession_number.as_deref()),
        ],
        &[
            (
                "NumberOfStudyRelatedSeries",
                i64::from(tags.dicom_number_of_study_related_series),
            ),
            (
                "NumberOfStudyRelatedInstances",
                i64::from(tags.dicom_number_of_study_related_instances),
            ),
        ],
    );

    let series = &parsed["Study"]["Series"];
    let series_map = series.as_object().unwrap_or_else(|| {
        panic!(
            "case {:?}: expected Study.Series to be an object, got {series:?}",
            case.case
        )
    });
    let Some(series_instance_uid) = tags.dicom_series_instance_uid.as_deref() else {
        assert!(
            series_map.is_empty(),
            "case {:?}: nothing stored a series, so none must be reported, got {series:?}",
            case.case
        );
        return;
    };
    assert_eq!(
        series_map.keys().map(String::as_str).collect::<Vec<&str>>(),
        vec![series_instance_uid],
        "case {:?}: expected the stored series keyed by its instance UID, got {series:?}",
        case.case
    );
    let held = &series[series_instance_uid];
    let series_numbers: Vec<(&str, i64)> = tags
        .dicom_series_number
        .map(|number| vec![("SeriesNumber", i64::from(number))])
        .unwrap_or_default();
    assert_attributes(
        case.case,
        "Study.Series.DICOM",
        &held["DICOM"],
        &[
            ("SeriesInstanceUID", Some(series_instance_uid)),
            ("Modality", tags.dicom_series_modality.as_deref()),
            ("BodyPartExamined", tags.dicom_series_body_part.as_deref()),
        ],
        &series_numbers,
    );

    // The instances are exactly the ones the stored pixel data belongs to, each holding
    // the frame identifiers `GetImageFrame` takes, both ordered.
    let mut expected: Vec<(&str, Vec<&str>)> = Vec::new();
    for (frame, frame_id) in case.frames.iter().zip(frame_ids) {
        let Some(uid) = frame.sop_instance_uid else {
            continue;
        };
        match expected.iter_mut().find(|(held, _)| *held == uid) {
            Some((_, ids)) => ids.push(frame_id),
            None => expected.push((uid, vec![frame_id])),
        }
    }
    expected.sort_by(|left, right| left.0.cmp(right.0));
    for (_, ids) in &mut expected {
        ids.sort_unstable();
    }
    let instances = &held["Instances"];
    let instance_map = instances.as_object().unwrap_or_else(|| {
        panic!(
            "case {:?}: expected Instances to be an object, got {instances:?}",
            case.case
        )
    });
    assert_eq!(
        instance_map.keys().map(String::as_str).collect::<Vec<&str>>(),
        expected.iter().map(|(uid, _)| *uid).collect::<Vec<&str>>(),
        "case {:?}: expected exactly the instances the stored pixel data belongs to, got \
         {instances:?}",
        case.case
    );
    for (uid, ids) in &expected {
        let instance = &instances[*uid];
        assert_eq!(
            instance["DICOM"]["SOPInstanceUID"].as_str(),
            Some(*uid),
            "case {:?}: expected the instance to name itself, got {instance:?}",
            case.case
        );
        let served: Vec<&str> = instance["ImageFrames"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected ImageFrames to be a list, got {instance:?}",
                    case.case
                )
            })
            .iter()
            .map(|frame| frame["ID"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            served, *ids,
            "case {:?}: expected instance {uid} to report the frames the image set holds \
             under it, got {instance:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_document_aggregates_exactly_what_the_image_set_holds() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.read", "token-metadata-read")
        .await;

    for case in document_cases() {
        let (image_set_id, frame_ids) = seed(&emulator, &datastore_id, &case);

        let read = get_image_set_metadata(&emulator, &datastore_id, &image_set_id, None).await;
        assert_document(&read, &case, &datastore_id, &image_set_id, &frame_ids);

        // Every version this image set holds held the content the case seeded, so each one
        // reports it; one past its latest is absent.
        for version in 1..=case.versions {
            let for_version = get_image_set_metadata(
                &emulator,
                &datastore_id,
                &image_set_id,
                Some(&version.to_string()),
            )
            .await;
            assert_document(&for_version, &case, &datastore_id, &image_set_id, &frame_ids);
        }
        let past_latest = get_image_set_metadata(
            &emulator,
            &datastore_id,
            &image_set_id,
            Some(&(case.versions + 1).to_string()),
        )
        .await;
        assert_eq!(
            past_latest.status, 404,
            "case {:?}: one past the latest version is absent, got {past_latest:?}",
            case.case
        );

        // The same bytes to the next caller: the document is read from state and the
        // read writes nothing into it.
        let again = get_image_set_metadata(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            again.body_bytes, read.body_bytes,
            "case {:?}: a second read must serve the same {} bytes, got {}",
            case.case,
            read.body_bytes.len(),
            again.body_bytes.len()
        );
    }
}

/// What one refusal row gets wrong about the request.
#[derive(Debug)]
enum Wrong {
    /// A data store the service does not hold.
    Datastore,
    /// A well-formed image set id the data store holds nothing under.
    ImageSet,
    /// An image set whose content went with its deletion.
    DeletedImageSet,
    /// A well-formed version identifier the image set holds no such version of.
    Version(&'static str),
    /// A `datastoreId` outside `DatastoreId`'s own constraint (`^[0-9a-z]{32}$`).
    MalformedDatastoreId(String),
    /// An `imageSetId` outside `ImageSetId`'s own constraint (`^[0-9a-z]{32}$`).
    MalformedImageSetId(String),
    /// A `version` outside `ImageSetExternalVersionId`'s constraint (`^\d+$`).
    MalformedVersion(&'static str),
}

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    wrong: Wrong,
    expected_status: u16,
    expected_code: &'static str,
    /// The text the refusal must state, or `None` where the row asserts only the status
    /// and the code (a constraint envelope the framework words).
    expected_message: Option<&'static str>,
}

/// The rows span: each identifier naming nothing held, an image set retired by a delete,
/// a version below and past the ones the image set holds, and each identifier outside its
/// modelled pattern — wrong length, wrong character class, and a `version` that is not a
/// decimal number.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "a data store nothing holds",
            wrong: Wrong::Datastore,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(DATASTORE_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "an image set nothing holds",
            wrong: Wrong::ImageSet,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "an image set whose content went with its deletion",
            wrong: Wrong::DeletedImageSet,
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "version 0, below the first version identifier",
            wrong: Wrong::Version("0"),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_VERSION_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "a version one past the latest",
            wrong: Wrong::Version("3"),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_VERSION_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "a version far past the latest",
            wrong: Wrong::Version("4096"),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            expected_message: Some(IMAGE_SET_VERSION_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "a datastoreId one character short of the pattern",
            wrong: Wrong::MalformedDatastoreId("a".repeat(31)),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "a datastoreId carrying an upper-case character",
            wrong: Wrong::MalformedDatastoreId(format!("A{}", "0".repeat(31))),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "an imageSetId one character past the pattern",
            wrong: Wrong::MalformedImageSetId("b".repeat(33)),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
        RefusalCase {
            case: "a version that is not a decimal number",
            wrong: Wrong::MalformedVersion("latest"),
            expected_status: 400,
            expected_code: "ValidationException",
            expected_message: None,
        },
    ]
}

#[tokio::test]
async fn a_document_the_service_cannot_aggregate_is_refused() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.refusal", "token-metadata-refusal")
        .await;
    // Two versions, so "3" is one past the latest.
    let held = DocumentCase {
        case: "held",
        tags: full_tags(),
        frames: vec![FrameSeed {
            sop_instance_uid: Some("1.2.840.113619.2.1.1.4"),
            transfer_syntax_uid: None,
            pixel_len: 16,
        }],
        versions: 2,
    };
    let (image_set_id, _) = seed(&emulator, &datastore_id, &held);
    let (deleted_id, _) = seed(&emulator, &datastore_id, &held);
    {
        let state = service_state(emulator.engine()).expect("the medical-imaging state store");
        let mut store = state.lock();
        let image_set = store
            .datastores
            .get_mut(&datastore_id)
            .and_then(|datastore| datastore.image_sets.get_mut(&deleted_id))
            .expect("the seeded image set");
        image_set.set_state_on_every_version(ImageSetState::Deleted);
        image_set.latest_version.workflow_status = ImageSetWorkflowStatus::Deleted;
        image_set.deleted_at = Some(SystemTime::now());
        image_set.image_frames.clear();
    }

    for row in refusal_cases() {
        let (requested_datastore, requested_image_set, version) = match &row.wrong {
            Wrong::Datastore => (unheld_id(), image_set_id.clone(), None),
            Wrong::ImageSet => (datastore_id.clone(), unheld_id(), None),
            Wrong::DeletedImageSet => (datastore_id.clone(), deleted_id.clone(), None),
            Wrong::Version(version) => (
                datastore_id.clone(),
                image_set_id.clone(),
                Some(*version),
            ),
            Wrong::MalformedDatastoreId(wrong) => (wrong.clone(), image_set_id.clone(), None),
            Wrong::MalformedImageSetId(wrong) => (datastore_id.clone(), wrong.clone(), None),
            Wrong::MalformedVersion(version) => (
                datastore_id.clone(),
                image_set_id.clone(),
                Some(*version),
            ),
        };
        let refused = get_image_set_metadata(
            &emulator,
            &requested_datastore,
            &requested_image_set,
            version,
        )
        .await;
        assert_eq!(
            refused.status, row.expected_status,
            "case {:?}: expected HTTP {}, got {refused:?}",
            row.case, row.expected_status
        );
        assert_eq!(
            refused.error_code, row.expected_code,
            "case {:?}: expected {}, got {refused:?}",
            row.case, row.expected_code
        );
        if let Some(expected) = row.expected_message {
            assert_eq!(
                refused.string(&["message"]),
                expected,
                "case {:?}: expected the message {expected:?}, got {refused:?}",
                row.case
            );
        }
    }

    // The image set the rows did not name is still served, so each row refused what it
    // named rather than everything.
    let served = get_image_set_metadata(&emulator, &datastore_id, &image_set_id, None).await;
    assert_eq!(
        served.status, 200,
        "case \"after the refusals\": the held image set must still be served, got {served:?}"
    );
}

/// The `lastAccessedAt` one `GetImageSet` answer reports.
fn last_access(answer: &Answer, case: &str) -> f64 {
    answer
        .field(&["lastAccessedAt"])
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a lastAccessedAt instant, got {answer:?}"))
}

/// The epoch-seconds number the wire carries for `instant`.
fn wire_instant(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant past the Unix epoch")
        .as_secs_f64()
}

#[tokio::test]
async fn serving_the_document_records_the_access_and_a_refusal_records_none() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.access", "token-metadata-access")
        .await;

    // The rows differ along how much content the image set holds, which is what the
    // served document aggregates: everything, attributes with no pixel data, and nothing
    // at all.
    let cases = [
        document_cases()[0].clone(),
        document_cases()[2].clone(),
        document_cases()[4].clone(),
    ];

    for case in &cases {
        let (image_set_id, _) = seed(&emulator, &datastore_id, case);
        let before_any = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        let mut previous = last_access(&before_any, case.case);

        for step in 0..2 {
            tokio::time::sleep(APART).await;
            let before = wire_instant(SystemTime::now());
            let served = get_image_set_metadata(&emulator, &datastore_id, &image_set_id, None).await;
            let after = wire_instant(SystemTime::now());
            assert_eq!(
                served.status, 200,
                "case {:?} step {step}: expected HTTP 200, got {served:?}",
                case.case
            );

            let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
            let reported = last_access(&read, case.case);
            assert!(
                reported >= before - SLACK_SECS && reported <= after + SLACK_SECS,
                "case {:?} step {step}: expected the last-access instant inside \
                 [{before}, {after}], got {reported} from {read:?}",
                case.case
            );
            assert!(
                reported > previous,
                "case {:?} step {step}: serving the document must move the last-access instant \
                 past {previous}, got {reported} from {read:?}",
                case.case
            );
            previous = reported;
        }

        // A call that served no document is no access: the instant stays where the last
        // served document left it.
        tokio::time::sleep(APART).await;
        let refused = get_image_set_metadata(
            &emulator,
            &datastore_id,
            &image_set_id,
            Some(&(case.versions + 1).to_string()),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {:?}: expected the unheld version to be refused, got {refused:?}",
            case.case
        );
        let after_refusal = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
        assert_eq!(
            last_access(&after_refusal, case.case),
            previous,
            "case {:?}: a refused request must leave the last-access instant at {previous}, got \
             {after_refusal:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_copy_delivers_the_document_its_source_aggregates() {
    // The rows differ along how the copied content is shaped: frames of one instance,
    // frames of two instances, and an image set with attributes but no pixel data.
    let cases = [
        document_cases()[0].clone(),
        document_cases()[1].clone(),
        document_cases()[2].clone(),
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore(
                &format!("metadata.copy.{index}"),
                &format!("token-metadata-copy-{index}"),
            )
            .await;
        let (source_id, _) = seed(&emulator, &datastore_id, case);

        let source_document =
            document(&get_image_set_metadata(&emulator, &datastore_id, &source_id, None).await,
                case.case);

        let copied = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information(&case.versions.to_string(), None, None),
            &[],
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {:?}: expected HTTP 200 from CopyImageSet, got {copied:?}",
            case.case
        );
        let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);

        // Nothing has been delivered into the destination yet, so it aggregates no
        // content: the document reports the hierarchy with no attribute and no series.
        let in_flight =
            get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await;
        let in_flight_document = document(&in_flight, case.case);
        assert_eq!(
            in_flight_document["Patient"]["DICOM"],
            Value::Object(serde_json::Map::new()),
            "case {:?}: a copy in flight has delivered no patient attribute, got \
             {in_flight_document:?}",
            case.case
        );
        assert_eq!(
            in_flight_document["Study"]["Series"],
            Value::Object(serde_json::Map::new()),
            "case {:?}: a copy in flight has delivered no series, got {in_flight_document:?}",
            case.case
        );

        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;

        // The content landed, so the destination aggregates exactly what its source
        // does — under its OWN image set identifier.
        let landed = get_image_set_metadata(&emulator, &datastore_id, &destination_id, None).await;
        let mut landed_document = document(&landed, case.case);
        assert_eq!(
            landed_document["ImageSetID"].as_str(),
            Some(destination_id.as_str()),
            "case {:?}: the document must name the destination image set, got {landed_document:?}",
            case.case
        );
        landed_document["ImageSetID"] = source_document["ImageSetID"].clone();
        assert_eq!(
            landed_document, source_document,
            "case {:?}: the copy must deliver the content its source aggregates, got \
             {landed_document:?} against {source_document:?}",
            case.case
        );
    }
}
