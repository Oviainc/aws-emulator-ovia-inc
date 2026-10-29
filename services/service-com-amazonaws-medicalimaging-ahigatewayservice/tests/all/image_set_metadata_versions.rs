// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VERSION `GetImageSetMetadata` serves.
//!
//! RULE: a read that names a version of an image set reports the content THAT version
//! holds, never the content the image set holds now — so `GetImageSetMetadata` with
//! `version` serves the DICOM attributes that version was serving (the latest version's are
//! the image set's own, an earlier version's the ones it held when a later version
//! superseded it), and a read naming no version serves the latest. The same snapshot a
//! metadata update's `revertToVersionId` puts back is therefore the one a read of that
//! version serves: the two can never disagree about what a version held.
//!
//! The first suite is a TABLE whose rows differ along what the rule generalises over: which
//! half of the attribute state the versions differ in (a modelled DICOM attribute, an
//! attribute the model does not name), the level of the document the difference sits at
//! (patient, study, series, one SOP instance), the kind of change between two versions (a
//! value rewritten, an attribute first written, a count moved, an attribute removed), and
//! how many versions the image set holds (two, three, four) — each version read back by
//! every version identifier the image set holds. The second suite drives a revert and reads
//! both the version it mints and the versions it passed over.
//!
//! Every case drives real HTTP through the production front door, so what is asserted is
//! the wire behaviour a `medical-imaging` SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image set through the crate's public `state` module exactly as
//! every image-set suite in this crate does, and then drives the versions over the wire
//! with `UpdateImageSetMetadata`.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::IMAGE_SET_UPDATE_DELAY;

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_set_metadata::{document, get_image_set_metadata};
use crate::image_set_metadata_updates::{
    at, dicom_updates, seed_image_set, seeded_tags, update_metadata, SERIES_UID, SOP_UID, STUDY_UID,
};

/// One update to drive: the attributes to write and the attributes to remove, in the shape
/// `updateImageSetMetadataUpdates.DICOMUpdates` carries them.
struct Write {
    updatable: Option<Value>,
    removable: Option<Value>,
}

/// One row of the version table: the updates that build the versions past the first, the
/// document paths the row states values at, and what each version must report there.
struct VersionCase {
    case: &'static str,
    /// Oldest first; version `n + 1` is what update `n` mints.
    updates: Vec<Write>,
    /// The paths of the served document this row speaks about.
    paths: Vec<Vec<&'static str>>,
    /// One entry per version the image set ends up holding, version 1 first, each stating
    /// the value expected at every path of `paths` — `Value::Null` where that version held
    /// nothing at that path.
    expected: Vec<Vec<Value>>,
}

/// The rows span both halves of the attribute state, every level of the document the model
/// names attributes at, each kind of change one version can make to the one before it, and
/// two, three and four versions.
fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            case: "a modelled patient attribute rewritten at each of four versions",
            updates: vec![
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Patient": { "DICOM": { "PatientName": "ONE^ONE" } },
                    })),
                    removable: None,
                },
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Patient": { "DICOM": { "PatientName": "TWO^TWO" } },
                    })),
                    removable: None,
                },
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Patient": { "DICOM": { "PatientName": "THREE^THREE" } },
                    })),
                    removable: None,
                },
            ],
            paths: vec![vec!["Patient", "DICOM", "PatientName"]],
            expected: vec![
                vec![json!("DOE^JANE")],
                vec![json!("ONE^ONE")],
                vec![json!("TWO^TWO")],
                vec![json!("THREE^THREE")],
            ],
        },
        VersionCase {
            case: "an attribute the model does not name, first written at version 2",
            updates: vec![
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Patient": { "DICOM": { "PatientAge": "031Y" } },
                    })),
                    removable: None,
                },
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Patient": { "DICOM": { "PatientAge": "032Y" } },
                    })),
                    removable: None,
                },
            ],
            paths: vec![vec!["Patient", "DICOM", "PatientAge"]],
            expected: vec![
                vec![Value::Null],
                vec![json!("031Y")],
                vec![json!("032Y")],
            ],
        },
        VersionCase {
            case: "a study attribute and a study count moved at each version",
            updates: vec![
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Study": {
                            "DICOM": {
                                "StudyDescription": "ABDOMEN",
                                "NumberOfStudyRelatedInstances": 17,
                            },
                        },
                    })),
                    removable: None,
                },
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Study": {
                            "DICOM": {
                                "StudyDescription": "HEAD",
                                "NumberOfStudyRelatedInstances": 41,
                            },
                        },
                    })),
                    removable: None,
                },
            ],
            paths: vec![
                vec!["Study", "DICOM", "StudyDescription"],
                vec!["Study", "DICOM", "NumberOfStudyRelatedInstances"],
            ],
            expected: vec![
                // The seeded attributes, which is what version 1 was serving.
                vec![json!("CHEST"), json!(2)],
                vec![json!("ABDOMEN"), json!(17)],
                vec![json!("HEAD"), json!(41)],
            ],
        },
        VersionCase {
            case: "attributes removed at version 2",
            updates: vec![Write {
                updatable: None,
                removable: Some(json!({
                    "SchemaVersion": "1.1",
                    "Patient": { "DICOM": { "PatientSex": "F" } },
                    "Study": { "DICOM": { "StudyDescription": "CHEST" } },
                })),
            }],
            paths: vec![
                vec!["Patient", "DICOM", "PatientSex"],
                vec!["Study", "DICOM", "StudyDescription"],
            ],
            expected: vec![
                vec![json!("F"), json!("CHEST")],
                vec![Value::Null, Value::Null],
            ],
        },
        VersionCase {
            case: "a series attribute at version 2 and one SOP instance's at version 3",
            updates: vec![
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
                        "Study": {
                            "Series": {
                                SERIES_UID: { "DICOM": { "Modality": "MR" } },
                            },
                        },
                    })),
                    removable: None,
                },
                Write {
                    updatable: Some(json!({
                        "SchemaVersion": "1.1",
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
                },
            ],
            paths: vec![
                vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"],
                vec![
                    "Study",
                    "Series",
                    SERIES_UID,
                    "Instances",
                    SOP_UID,
                    "DICOM",
                    "InstanceNumber",
                ],
            ],
            expected: vec![
                vec![json!("CT"), Value::Null],
                vec![json!("MR"), Value::Null],
                vec![json!("MR"), json!("7")],
            ],
        },
    ]
}

/// Drive one `UpdateImageSetMetadata` against `latest_version`, assert it minted the next
/// version, and let the clock settle it.
async fn mint_version(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    latest_version: u32,
    updates: Value,
    case: &str,
) {
    let answer = update_metadata(
        emulator,
        datastore_id,
        image_set_id,
        &latest_version.to_string(),
        updates,
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from UpdateImageSetMetadata over version \
         {latest_version}, got {answer:?}"
    );
    assert_eq!(
        answer.string(&["latestVersionId"]),
        (latest_version + 1).to_string(),
        "case {case:?}: expected the update over version {latest_version} to mint version {}, \
         got {answer:?}",
        latest_version + 1
    );
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
}

/// Read one version's document and assert it holds `expected` at `paths`.
async fn assert_version_document(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    version: Option<&str>,
    paths: &[Vec<&str>],
    expected: &[Value],
    case: &str,
) -> Answer {
    let answer = get_image_set_metadata(emulator, datastore_id, image_set_id, version).await;
    let served = document(&answer, case);
    assert_eq!(
        served["ImageSetID"].as_str(),
        Some(image_set_id),
        "case {case:?}: expected version {version:?}'s document to name its image set, got \
         {served:?}"
    );
    for (path, want) in paths.iter().zip(expected) {
        let actual = at(&served, path);
        assert_eq!(
            actual, *want,
            "case {case:?}: expected version {version:?} to report {want} at {path:?}, got \
             {actual}"
        );
    }
    answer
}

#[tokio::test]
async fn each_version_reports_the_attributes_that_version_held() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.versions", "token-metadata-versions")
        .await;

    for case in version_cases() {
        let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
        for (index, write) in case.updates.iter().enumerate() {
            let latest_version = u32::try_from(index).expect("a small version count") + 1;
            mint_version(
                &emulator,
                &datastore_id,
                &image_set_id,
                latest_version,
                dicom_updates(write.updatable.as_ref(), write.removable.as_ref()),
                case.case,
            )
            .await;
        }

        // Every version the image set holds, oldest first, reports the content IT held —
        // which for every version but the latest is content the image set no longer holds.
        for (index, expected) in case.expected.iter().enumerate() {
            let version = (index + 1).to_string();
            assert_version_document(
                &emulator,
                &datastore_id,
                &image_set_id,
                Some(&version),
                &case.paths,
                expected,
                case.case,
            )
            .await;
        }

        // A read naming no version reads the latest, so the two serve the same bytes — and
        // reading an earlier version left that answer where it was, because a read of the
        // document writes none of it.
        let latest = case
            .expected
            .last()
            .expect("every row states at least one version");
        let newest = assert_version_document(
            &emulator,
            &datastore_id,
            &image_set_id,
            None,
            &case.paths,
            latest,
            case.case,
        )
        .await;
        let by_identifier = assert_version_document(
            &emulator,
            &datastore_id,
            &image_set_id,
            Some(&case.expected.len().to_string()),
            &case.paths,
            latest,
            case.case,
        )
        .await;
        assert_eq!(
            newest.body_bytes, by_identifier.body_bytes,
            "case {:?}: naming the latest version must serve the same {} bytes as naming none, \
             got {}",
            case.case,
            newest.body_bytes.len(),
            by_identifier.body_bytes.len()
        );
    }
}

#[tokio::test]
async fn a_revert_and_a_version_read_agree_on_what_a_version_held() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.versions.revert", "token-metadata-versions-revert")
        .await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));

    // Three versions, each holding a modelled attribute and an unmodelled one the version
    // before it did not.
    for (latest_version, name, age) in [(1, "ONE^ONE", "031Y"), (2, "TWO^TWO", "032Y")] {
        mint_version(
            &emulator,
            &datastore_id,
            &image_set_id,
            latest_version,
            dicom_updates(
                Some(&json!({
                    "SchemaVersion": "1.1",
                    "Patient": { "DICOM": { "PatientName": name, "PatientAge": age } },
                })),
                None,
            ),
            "a revert agrees with a version read",
        )
        .await;
    }

    let paths = [
        vec!["Patient", "DICOM", "PatientName"],
        vec!["Patient", "DICOM", "PatientAge"],
    ];
    // What each of the three versions held: version 1 the seeded attributes, with no age
    // stored at all.
    let held = [
        vec![json!("DOE^JANE"), Value::Null],
        vec![json!("ONE^ONE"), json!("031Y")],
        vec![json!("TWO^TWO"), json!("032Y")],
    ];

    // Revert to version 1. The revert mints version 4 holding what version 1 held, so a
    // read of version 4 and a read of version 1 report the same attributes — and the
    // versions the revert passed over still report their own.
    let reverted = update_metadata(
        &emulator,
        &datastore_id,
        &image_set_id,
        "3",
        json!({ "revertToVersionId": "1" }),
        &[],
    )
    .await;
    assert_eq!(
        reverted.status, 200,
        "case \"revert to 1\": expected HTTP 200 from UpdateImageSetMetadata, got {reverted:?}"
    );
    assert_eq!(
        reverted.string(&["latestVersionId"]),
        "4",
        "case \"revert to 1\": expected the revert to mint version 4, got {reverted:?}"
    );
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;

    for (index, expected) in held.iter().enumerate() {
        let version = (index + 1).to_string();
        assert_version_document(
            &emulator,
            &datastore_id,
            &image_set_id,
            Some(&version),
            &paths,
            expected,
            "after a revert to 1",
        )
        .await;
    }
    assert_version_document(
        &emulator,
        &datastore_id,
        &image_set_id,
        Some("4"),
        &paths,
        &held[0],
        "the version a revert to 1 minted",
    )
    .await;
}
