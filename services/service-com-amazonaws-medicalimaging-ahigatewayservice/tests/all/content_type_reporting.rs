// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `contentType` each content read of an image set reports.
//!
//! RULE: a read that serves content reports in `contentType` the format its OWN response
//! member documents that content is returned in — nothing the emulator prefers decides it,
//! and in particular not the bytes the blob happens to carry.
//! `GetImageSetMetadata`'s member documents one format for the study metadata and
//! distinguishes no case ("The format in which the study metadata is returned to the
//! customer. Default is `text/plain`."), so every answer it serves reports that documented
//! default, whatever the aggregated document holds and whichever version it was built from;
//! `GetImageFrame`'s member documents a per-transfer-syntax catalog with a default of its
//! own, so its answers report the entry documented for the syntax the pixel data is stored
//! in. Neither read names a compression its blob is not in: the metadata blob is served
//! gzip-compressed, as AWS returns it, and names that format in `contentEncoding`, while
//! the pixel data is served as it was stored and names none.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: for the
//! metadata read, how much the document holds (nothing stored at all, every modelled
//! attribute, attributes the model does not name, a document hundreds of attributes
//! larger), how many frames the image set holds, how many versions it holds and which one
//! the read names; for the pixel-data read, the stored transfer syntax (documented entries,
//! a missing one, one the documentation lists nothing for). Every case drives real HTTP
//! through the production front door, so what is asserted is the wire behaviour a
//! `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image set through the crate's public `state` module with the same
//! `ImageSet::new` / `ImageFrame` the handlers use, and then asserts only over the wire.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Map, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageFrame, ImageSet, ImageSetState, ImageSetWorkflowStatus,
};

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_frames::{get_image_frame, pixel_data, seed_image_set_with_frames, SeedFrame};
use crate::image_set_metadata::get_image_set_metadata;
use crate::image_set_metadata_updates::{seeded_tags, STUDY_UID};

/// The `contentType` `GetImageSetMetadata` must report, taken from the response member's
/// own documentation —
/// `smithy_query(query="doc",shape="GetImageSetMetadataResponse",member="contentType")`:
/// "The format in which the study metadata is returned to the customer. Default is
/// `text/plain`." That documentation names no case, so this is the format of every answer.
///
/// Spelled as the literal the member documents rather than read off the handler's own
/// constant, so a case here fails if that constant moves away from the documentation.
const DOCUMENTED_METADATA_CONTENT_TYPE: &str = "text/plain";

/// The compression format AWS HealthImaging returns the aggregated metadata blob in, which
/// is what `contentEncoding` ("the compression format in which image set metadata
/// attributes are returned") names.
const DOCUMENTED_METADATA_CONTENT_ENCODING: &str = "gzip";

/// Stored transfer syntax -> the `contentType` `GetImageFrame` must report for it.
///
/// Transcribed from that member's own documentation —
/// `smithy_query(query="doc",shape="GetImageFrameResponse",member="contentType")`, whose
/// whole catalog `crate::image_frames` drives row by row. The entries here are the ones
/// that show the SAME rule decides the sibling read: two catalogued UIDs, the missing
/// transfer syntax the documentation gives `image/jph`, and a real DICOM transfer syntax
/// (`1.2.840.10008.1.2.5`, RLE Lossless) the catalog lists nothing for, which takes the
/// documented default.
const DOCUMENTED_FRAME_CONTENT_TYPES: [(Option<&str>, &str); 4] = [
    (Some("1.2.840.10008.1.2.4.203"), "image/jphc"),
    (Some("1.2.840.10008.1.2.4.112"), "image/jxl"),
    (None, "image/jph"),
    (Some("1.2.840.10008.1.2.5"), "application/octet-stream"),
];

/// One row of the metadata table: the content to seed, how many versions the image set
/// holds, and which version the read names.
#[derive(Debug, Clone)]
struct MetadataCase {
    case: &'static str,
    /// The modelled DICOM attributes the image set holds.
    tags: DICOMTags,
    /// The attributes the model does not name, as the document carries them.
    extra: Value,
    /// How many image frames the image set holds, each under its own SOP instance.
    frames: usize,
    /// How many versions the image set holds; each one holds the content seeded here.
    versions: u32,
    /// The `version` the read names, `None` to read the latest.
    read_version: Option<&'static str>,
}

/// `count` attributes the model does not name, at the document's top level: a row that
/// makes the served document hundreds of attributes larger than the others' without
/// changing anything about the format it is served in.
fn many_attributes(count: usize) -> Value {
    let mut attributes = Map::new();
    for index in 0..count {
        attributes.insert(
            format!("PrivateAttribute{index:04}"),
            json!(format!("value-{index}")),
        );
    }
    Value::Object(attributes)
}

/// Seed one `ACTIVE` image set holding `case`'s content and answer its identifier.
///
/// The attributes are set BEFORE the versions are pushed, because `push_version` snapshots
/// what the image set holds at that instant onto the version it supersedes — so every
/// version this image set holds carries the content the row names, and a row reading an
/// earlier version reads a document of its own.
fn seed(emulator: &Emulator, datastore_id: &str, case: &MetadataCase) -> String {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let frame_ids: Vec<String> = (0..case.frames).map(|_| store.mint_id()).collect();
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
    image_set.dicom_tags = case.tags.clone();
    image_set.extra_attributes = case
        .extra
        .as_object()
        .cloned()
        .expect("a row's extra attributes spelled as a JSON object");
    for _ in 1..case.versions {
        image_set.push_version(
            ImageSetState::Active,
            ImageSetWorkflowStatus::Updated,
            false,
            SystemTime::now(),
        );
    }
    for (index, frame_id) in frame_ids.iter().enumerate() {
        image_set.image_frames.insert(
            frame_id.clone(),
            ImageFrame {
                image_frame_id: frame_id.clone(),
                sop_instance_uid: Some(format!("1.2.840.113619.2.1.1.{index}")),
                transfer_syntax_uid: Some("1.2.840.10008.1.2.4.50".to_string()),
                pixel_data: pixel_data(index as u8 + 1, 8 * (index + 1)),
            },
        );
    }
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    image_set_id
}

/// The rows differ along how much the aggregated document holds, how many frames the image
/// set holds, how many versions it holds and which one the read names.
fn metadata_cases() -> Vec<MetadataCase> {
    vec![
        MetadataCase {
            case: "nothing stored at all: no attribute, no frame, one version",
            tags: DICOMTags::default(),
            extra: json!({}),
            frames: 0,
            versions: 1,
            read_version: None,
        },
        MetadataCase {
            case: "every modelled attribute, two frames, the latest of one version",
            tags: seeded_tags(STUDY_UID),
            extra: json!({}),
            frames: 2,
            versions: 1,
            read_version: None,
        },
        MetadataCase {
            case: "attributes the model does not name, version 1 of 3 named by the read",
            tags: seeded_tags(STUDY_UID),
            extra: json!({
                "Patient": { "DICOM": { "PatientWeight": "70" } },
                "PrivateRoot": { "Note": "an attribute the model never names" },
            }),
            frames: 1,
            versions: 3,
            read_version: Some("1"),
        },
        MetadataCase {
            case: "a document hundreds of attributes larger, the latest of two versions",
            tags: seeded_tags(STUDY_UID),
            extra: many_attributes(200),
            frames: 3,
            versions: 2,
            read_version: None,
        },
        MetadataCase {
            case: "patient attributes only, version 2 of 2 named by the read",
            tags: DICOMTags {
                dicom_patient_id: Some("PID-21".to_string()),
                dicom_patient_name: Some("Roe^Ray".to_string()),
                ..DICOMTags::default()
            },
            extra: json!({}),
            frames: 0,
            versions: 2,
            read_version: Some("2"),
        },
    ]
}

/// Assert one metadata answer carries the documented format, names the compression its
/// blob is really in, and still holds the JSON document in that blob; answer how many bytes
/// that document held.
fn assert_metadata_answer(answer: &Answer, case: &str) -> usize {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetImageSetMetadata, got {answer:?}"
    );
    assert_eq!(
        answer.content_type, DOCUMENTED_METADATA_CONTENT_TYPE,
        "case {case:?}: expected contentType {DOCUMENTED_METADATA_CONTENT_TYPE:?}, got {:?}",
        answer.content_type
    );
    assert_eq!(
        answer.content_encoding, DOCUMENTED_METADATA_CONTENT_ENCODING,
        "case {case:?}: expected contentEncoding {DOCUMENTED_METADATA_CONTENT_ENCODING:?}, got \
         {:?}",
        answer.content_encoding
    );
    // The header states the format; the blob is untouched by it, so the aggregated
    // document is still the JSON object the read builds from state.
    let parsed: Value = serde_json::from_slice(&answer.body_bytes).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected the aggregated document in the blob, got {error} over {} \
             bytes",
            answer.body_bytes.len()
        )
    });
    assert!(
        parsed.is_object(),
        "case {case:?}: expected the document to be an object of attributes, got {parsed:?}"
    );
    answer.body_bytes.len()
}

/// RULE: `GetImageSetMetadata` reports the format its own response member documents for the
/// study metadata, whatever the document it aggregates holds and whichever version of the
/// image set it was built from.
#[tokio::test]
async fn the_metadata_read_reports_the_content_type_its_member_documents() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("content.type.metadata", "token-content-type-metadata")
        .await;

    let mut served_sizes: BTreeSet<usize> = BTreeSet::new();
    for case in metadata_cases() {
        let image_set_id = seed(&emulator, &datastore_id, &case);
        let answer =
            get_image_set_metadata(&emulator, &datastore_id, &image_set_id, case.read_version)
                .await;
        served_sizes.insert(assert_metadata_answer(&answer, case.case));
    }

    // The rows really did serve documents of different sizes, so the one documented format
    // above was reported over a class of documents rather than over one of them five times.
    assert!(
        served_sizes.len() > 1,
        "the table must serve documents of differing sizes, got {served_sizes:?}"
    );
}

/// RULE: each of the two content reads answers with the format ITS member documents, so the
/// same image set serves its pixel data under the content type documented for the stored
/// transfer syntax and its metadata under the one documented for the study metadata.
#[tokio::test]
async fn each_content_read_reports_its_own_members_documented_content_type() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("content.type.both", "token-content-type-both")
        .await;

    let seeds: Vec<SeedFrame> = DOCUMENTED_FRAME_CONTENT_TYPES
        .iter()
        .enumerate()
        .map(|(index, (transfer_syntax_uid, _))| SeedFrame {
            transfer_syntax_uid: *transfer_syntax_uid,
            pixel_data: pixel_data(index as u8 + 3, 16 * (index + 1)),
        })
        .collect();
    let (image_set_id, frame_ids) =
        seed_image_set_with_frames(&emulator, &datastore_id, ImageSetState::Active, &seeds);

    for ((transfer_syntax_uid, documented), frame_id) in
        DOCUMENTED_FRAME_CONTENT_TYPES.iter().zip(&frame_ids)
    {
        let case = format!("pixel data stored in {transfer_syntax_uid:?}");
        let read = get_image_frame(&emulator, &datastore_id, &image_set_id, frame_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetImageFrame, got {read:?}"
        );
        assert_eq!(
            read.content_type, *documented,
            "case {case:?}: expected contentType {documented:?}, got {:?}",
            read.content_type
        );
    }

    // The metadata of the SAME image set: the study metadata's own documented format, not
    // any of the formats its pixel data is served under.
    let case = "the metadata of the image set holding every frame above";
    let answer = get_image_set_metadata(&emulator, &datastore_id, &image_set_id, None).await;
    assert_metadata_answer(&answer, case);
    for (_, documented) in DOCUMENTED_FRAME_CONTENT_TYPES {
        assert_ne!(
            answer.content_type, documented,
            "case {case:?}: the metadata format must not be a pixel-data format, got {:?}",
            answer.content_type
        );
    }
}
