// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH series a metadata update's series-level attributes describe.
//!
//! RULE: the model declares the attributes of ONE series per image set, the series its
//! `DICOMSeriesInstanceUID` identifies, so an update writes a series-level attribute into
//! those modelled members only for THAT series — the one the image set holds, or the one
//! the same document identifies for an image set holding none — and keeps the attributes
//! of every other series at the path of the series they were written at, where the
//! metadata document reports them. An identifier that leaves takes the attributes of the
//! series it identified to that path with it. So every attribute an update accepted is
//! reported, each from the one place it is held, and the search view and the metadata
//! document never disagree about one.
//!
//! The rows differ along what the rule generalises over: whether the image set holds a
//! series identifier, how many series one document names, whether the document identifies
//! the series it describes (and whether that identifier agrees with the series level it
//! sits under), which series-level keyword is written (text, number, one the model does
//! not name), and writing versus removing the identifier. Every case drives real HTTP
//! through the production front door, and asserts both views: the aggregated document
//! `GetImageSetMetadata` serves and the summary `SearchImageSets` serves.
//!
//! As in `image_set_metadata_updates`, each case seeds its image set through the crate's
//! public `state` module, because DICOM content arrives through an import job this
//! emulator does not run yet, and then asserts only over the wire.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::IMAGE_SET_UPDATE_DELAY;

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_metadata_updates::{
    at, dicom_updates, read_document, seed_image_set, seeded_tags, update_metadata, SERIES_UID,
    STUDY_UID,
};
use crate::image_set_search::search;

/// A series UID no seeded image set holds, for a document that describes a series the
/// modelled members do not. Numeric and dotted, as `DICOMSeriesInstanceUID`'s own
/// `^[0-9.]+$` spells one.
const FIRST_SERIES: &str = "1.2.840.113619.2.1.2.11";

/// A SECOND such series UID, for a document naming two series at once.
const SECOND_SERIES: &str = "1.2.840.113619.2.1.2.12";

/// The attributes the image set of one row holds going in: the suite's seeded ones when
/// the row holds the seeded series, and none of the four series-level members at all when
/// the row holds no series — which is what an image set nothing has delivered DICOM
/// content to holds.
fn tags_holding(series_instance_uid: Option<&str>) -> DICOMTags {
    let mut tags = seeded_tags(STUDY_UID);
    tags.dicom_series_instance_uid = series_instance_uid.map(str::to_string);
    if series_instance_uid.is_none() {
        tags.dicom_series_modality = None;
        tags.dicom_series_body_part = None;
        tags.dicom_series_number = None;
    }
    tags
}

/// The `DICOMTags` of one image set as `SearchImageSets` reports them — the view the
/// metadata document has to agree with.
async fn searched_tags(
    emulator: &Emulator,
    datastore_id: &str,
    image_set_id: &str,
    case: &str,
) -> Value {
    let answer = search(emulator, datastore_id, None, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from SearchImageSets, got {answer:?}"
    );
    let summary = answer
        .field(&["imageSetsMetadataSummaries"])
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["imageSetId"].as_str() == Some(image_set_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a search summary for the image set, got {answer:?}")
        });
    summary["DICOMTags"].clone()
}

/// One row of the table.
struct SeriesCase {
    case: &'static str,
    /// The Series Instance UID the image set holds going in, if any.
    held_series: Option<&'static str>,
    force: bool,
    updatable: Option<Value>,
    removable: Option<Value>,
    /// Document paths the served metadata document must hold these values at afterwards.
    expected: Vec<(Vec<&'static str>, Value)>,
    /// Document paths the served document must hold NOTHING at afterwards.
    absent: Vec<Vec<&'static str>>,
    /// What the search summary's `DICOMTags` must report, member by member — `null` for a
    /// member nothing is stored for.
    expected_tags: Vec<(&'static str, Value)>,
}

fn series_cases() -> Vec<SeriesCase> {
    vec![
        SeriesCase {
            case: "two series described on an image set holding no series identifier",
            held_series: None,
            force: false,
            updatable: Some(json!({
                "Study": {
                    "Series": {
                        FIRST_SERIES: { "DICOM": { "Modality": "CT" } },
                        SECOND_SERIES: { "DICOM": { "Modality": "MR" } },
                    },
                },
            })),
            removable: None,
            expected: vec![
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "Modality"],
                    json!("CT"),
                ),
                (
                    vec!["Study", "Series", SECOND_SERIES, "DICOM", "Modality"],
                    json!("MR"),
                ),
            ],
            absent: Vec::new(),
            // No identifier says which series the modelled members describe, so they
            // describe none: a search filters on nothing either series wrote.
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(null)),
                ("DICOMSeriesModality", json!(null)),
            ],
        },
        SeriesCase {
            case: "one series described on an image set holding no series identifier, text \
                   and a number",
            held_series: None,
            force: false,
            updatable: Some(json!({
                "Study": {
                    "Series": {
                        FIRST_SERIES: {
                            "DICOM": {
                                "Modality": "US",
                                "BodyPartExamined": "ABDOMEN",
                                "SeriesNumber": 4,
                            },
                        },
                    },
                },
            })),
            removable: None,
            expected: vec![
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "Modality"],
                    json!("US"),
                ),
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "BodyPartExamined"],
                    json!("ABDOMEN"),
                ),
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "SeriesNumber"],
                    json!(4),
                ),
            ],
            absent: Vec::new(),
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(null)),
                ("DICOMSeriesModality", json!(null)),
                ("DICOMSeriesBodyPart", json!(null)),
                ("DICOMSeriesNumber", json!(null)),
            ],
        },
        SeriesCase {
            case: "a document that identifies the series it describes on an image set \
                   holding none",
            held_series: None,
            // Changing a Series Instance UID is one of the four changes the `force` flag
            // exists for, so a document that identifies a series carries it.
            force: true,
            updatable: Some(json!({
                "Study": {
                    "Series": {
                        FIRST_SERIES: {
                            "DICOM": {
                                "SeriesInstanceUID": FIRST_SERIES,
                                "Modality": "NM",
                                "SeriesNumber": 7,
                            },
                        },
                    },
                },
            })),
            removable: None,
            expected: vec![
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "SeriesInstanceUID"],
                    json!(FIRST_SERIES),
                ),
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "Modality"],
                    json!("NM"),
                ),
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "SeriesNumber"],
                    json!(7),
                ),
            ],
            absent: Vec::new(),
            // The document said which series the image set holds, so the modelled members
            // describe it and a search filters on what it wrote.
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(FIRST_SERIES)),
                ("DICOMSeriesModality", json!("NM")),
                ("DICOMSeriesNumber", json!(7)),
            ],
        },
        SeriesCase {
            case: "an identifier that disagrees with the series level it sits under",
            held_series: None,
            force: true,
            updatable: Some(json!({
                "Study": {
                    "Series": {
                        FIRST_SERIES: {
                            "DICOM": { "SeriesInstanceUID": SECOND_SERIES, "Modality": "CR" },
                        },
                    },
                },
            })),
            removable: None,
            expected: vec![
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "SeriesInstanceUID"],
                    json!(SECOND_SERIES),
                ),
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "Modality"],
                    json!("CR"),
                ),
            ],
            absent: Vec::new(),
            // The series levels of the document are keyed BY the Series Instance UID, so
            // an identifier that is not the one it sits under identifies no series: the
            // modelled members describe none, and both attributes stay as written.
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(null)),
                ("DICOMSeriesModality", json!(null)),
            ],
        },
        SeriesCase {
            case: "the series the image set holds",
            held_series: Some(SERIES_UID),
            force: false,
            updatable: Some(json!({
                "Study": { "Series": { SERIES_UID: { "DICOM": { "Modality": "MR" } } } },
            })),
            removable: None,
            expected: vec![
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"],
                    json!("MR"),
                ),
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "SeriesInstanceUID"],
                    json!(SERIES_UID),
                ),
            ],
            absent: Vec::new(),
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(SERIES_UID)),
                ("DICOMSeriesModality", json!("MR")),
            ],
        },
        SeriesCase {
            case: "another series than the one the image set holds",
            held_series: Some(SERIES_UID),
            force: false,
            updatable: Some(json!({
                "Study": { "Series": { FIRST_SERIES: { "DICOM": { "Modality": "MR" } } } },
            })),
            removable: None,
            expected: vec![
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "Modality"],
                    json!("MR"),
                ),
                // The series the image set holds still reports what it held: the modelled
                // members describe it and nothing else.
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"],
                    json!("CT"),
                ),
            ],
            absent: Vec::new(),
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(SERIES_UID)),
                ("DICOMSeriesModality", json!("CT")),
            ],
        },
        SeriesCase {
            case: "the series the image set holds and another in one document",
            held_series: Some(SERIES_UID),
            force: false,
            updatable: Some(json!({
                "Study": {
                    "Series": {
                        SERIES_UID: { "DICOM": { "Modality": "MR" } },
                        FIRST_SERIES: { "DICOM": { "Modality": "US" } },
                    },
                },
            })),
            removable: None,
            expected: vec![
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"],
                    json!("MR"),
                ),
                (
                    vec!["Study", "Series", FIRST_SERIES, "DICOM", "Modality"],
                    json!("US"),
                ),
            ],
            absent: Vec::new(),
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(SERIES_UID)),
                ("DICOMSeriesModality", json!("MR")),
            ],
        },
        SeriesCase {
            case: "a series-level attribute the model does not name, on an image set \
                   holding no series identifier",
            held_series: None,
            force: false,
            updatable: Some(json!({
                "Study": {
                    "Series": { FIRST_SERIES: { "DICOM": { "SeriesDescription": "HEAD" } } },
                },
            })),
            removable: None,
            expected: vec![(
                vec!["Study", "Series", FIRST_SERIES, "DICOM", "SeriesDescription"],
                json!("HEAD"),
            )],
            absent: Vec::new(),
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(null)),
                ("DICOMSeriesModality", json!(null)),
            ],
        },
        SeriesCase {
            case: "the identifier of the series the image set holds, removed",
            held_series: Some(SERIES_UID),
            force: true,
            updatable: None,
            removable: Some(json!({
                "Study": {
                    "Series": { SERIES_UID: { "DICOM": { "SeriesInstanceUID": SERIES_UID } } },
                },
            })),
            // The attributes OF that series were not removed, so they are still reported —
            // at the path of the series they describe, now that no identifier says the
            // modelled members describe it.
            expected: vec![
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"],
                    json!("CT"),
                ),
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "BodyPartExamined"],
                    json!("CHEST"),
                ),
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "SeriesNumber"],
                    json!(3),
                ),
            ],
            absent: vec![vec![
                "Study",
                "Series",
                SERIES_UID,
                "DICOM",
                "SeriesInstanceUID",
            ]],
            // The modelled members describe the series their identifier names, so once it
            // is gone they describe none — and a search no longer reports attributes of a
            // series the document says the image set no longer identifies.
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(null)),
                ("DICOMSeriesModality", json!(null)),
                ("DICOMSeriesBodyPart", json!(null)),
                ("DICOMSeriesNumber", json!(null)),
            ],
        },
        SeriesCase {
            case: "an attribute of the series the image set holds, removed",
            held_series: Some(SERIES_UID),
            force: false,
            updatable: None,
            removable: Some(json!({
                "Study": { "Series": { SERIES_UID: { "DICOM": { "Modality": "CT" } } } },
            })),
            expected: vec![
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "SeriesInstanceUID"],
                    json!(SERIES_UID),
                ),
                (
                    vec!["Study", "Series", SERIES_UID, "DICOM", "BodyPartExamined"],
                    json!("CHEST"),
                ),
            ],
            absent: vec![vec!["Study", "Series", SERIES_UID, "DICOM", "Modality"]],
            expected_tags: vec![
                ("DICOMSeriesInstanceUID", json!(SERIES_UID)),
                ("DICOMSeriesModality", json!(null)),
                ("DICOMSeriesBodyPart", json!("CHEST")),
            ],
        },
    ]
}

#[tokio::test]
async fn series_attributes_are_reported_for_the_series_they_describe() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.series", "token-metadata-series")
        .await;

    for case in series_cases() {
        let image_set_id = seed_image_set(&emulator, &datastore_id, tags_holding(case.held_series));

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

        // The clock, not a reader, settles the update.
        tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;

        let document = read_document(&emulator, &datastore_id, &image_set_id, case.case).await;
        for (path, expected) in &case.expected {
            assert_eq!(
                at(&document, path),
                *expected,
                "case {:?}: expected the document to hold {expected} at {path:?}, got {}",
                case.case,
                at(&document, path)
            );
        }
        for path in &case.absent {
            assert!(
                at(&document, path).is_null(),
                "case {:?}: expected the document to hold nothing at {path:?}, got {}",
                case.case,
                at(&document, path)
            );
        }

        let tags = searched_tags(&emulator, &datastore_id, &image_set_id, case.case).await;
        for (member, expected) in &case.expected_tags {
            assert_eq!(
                tags[*member],
                *expected,
                "case {:?}: expected the search view to report {expected} for {member}, got \
                 {tags}",
                case.case
            );
        }
    }
}

/// The promotion half of the rule: once a later document says WHICH series the image set
/// holds, the attributes of that series already kept at its path are the modelled ones, so
/// the search view and the metadata document report the same value for each from then on.
#[tokio::test]
async fn a_series_identified_later_takes_over_the_attributes_already_kept_for_it() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("metadata.series.later", "token-metadata-series-later")
        .await;
    let case = "a series identified by a later update";
    let image_set_id = seed_image_set(&emulator, &datastore_id, tags_holding(None));

    // First update: nothing says which series the image set holds, so what it describes is
    // kept at the path of the series it was written at.
    let written = update_metadata(
        &emulator,
        &datastore_id,
        &image_set_id,
        "1",
        dicom_updates(
            Some(&json!({
                "Study": {
                    "Series": {
                        FIRST_SERIES: { "DICOM": { "Modality": "PT", "SeriesNumber": 12 } },
                    },
                },
            })),
            None,
        ),
        &[],
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case {case:?}: expected HTTP 200 from the first update, got {written:?}"
    );
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
    let tags = searched_tags(&emulator, &datastore_id, &image_set_id, case).await;
    assert_eq!(
        tags["DICOMSeriesModality"],
        json!(null),
        "case {case:?}: expected the search view to report no series modality while no \
         identifier names the series, got {tags}"
    );

    // Second update: the document identifies that very series, so its attributes are the
    // modelled ones from then on — reported by the document at the same path, and now by a
    // search too.
    let identified = update_metadata(
        &emulator,
        &datastore_id,
        &image_set_id,
        "2",
        dicom_updates(
            Some(&json!({
                "Study": {
                    "Series": { FIRST_SERIES: { "DICOM": { "SeriesInstanceUID": FIRST_SERIES } } },
                },
            })),
            None,
        ),
        &[("force", true)],
    )
    .await;
    assert_eq!(
        identified.status, 200,
        "case {case:?}: expected HTTP 200 from the second update, got {identified:?}"
    );
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;

    let document = read_document(&emulator, &datastore_id, &image_set_id, case).await;
    for (keyword, expected) in [("Modality", json!("PT")), ("SeriesNumber", json!(12))] {
        let path = vec!["Study", "Series", FIRST_SERIES, "DICOM", keyword];
        assert_eq!(
            at(&document, &path),
            expected,
            "case {case:?}: expected the document to hold {expected} at {path:?}, got {}",
            at(&document, &path)
        );
    }
    let tags = searched_tags(&emulator, &datastore_id, &image_set_id, case).await;
    for (member, expected) in [
        ("DICOMSeriesInstanceUID", json!(FIRST_SERIES)),
        ("DICOMSeriesModality", json!("PT")),
        ("DICOMSeriesNumber", json!(12)),
    ] {
        assert_eq!(
            tags[member], expected,
            "case {case:?}: expected the search view to report {expected} for {member} once \
             the series is identified, got {tags}"
        );
    }
}
