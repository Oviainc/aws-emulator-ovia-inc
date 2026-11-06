// SPDX-License-Identifier: CC0-1.0

//! The constraints a value written into a modelled `DICOMTags` member must satisfy.
//!
//! RULE: an attribute an update document names for a member the model declares is written
//! only when its value satisfies every constraint that member's shape declares — the kind
//! it holds, its `@length`, its `@pattern`, its `@range` — and a value failing one is
//! refused with `ValidationException` (HTTP 400) that persists nothing. The document
//! arrives in a blob, which the boundary validator does not walk, so the handler is where
//! those constraints reach it; without them a search summary and the served metadata
//! document would report values the model forbids.
//!
//! The rows differ along what the rule generalises over: which constraint class the value
//! fails (kind, maximum length, pattern, lower bound, upper bound), which member carries
//! the attribute and at which level of the document it sits (patient, study, one series),
//! whether the member holds text or a number, and which side of a bound the value sits on
//! — each member's extreme LEGAL value is written and read back, and the value one step
//! past it is refused.
//!
//! Every bound and pattern below is the model's own, read off
//! `smithy_query(query="shape",shape="DICOMTags")`.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::IMAGE_SET_UPDATE_DELAY;

use crate::datastore_lifecycle::{Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::get_image_set;
use crate::image_set_metadata_updates::{
    at, dicom_updates, read_document, seed_image_set, seeded_tags, update_metadata, SERIES_UID,
    STUDY_UID,
};
use crate::image_set_search::search;

/// The validation path every refusal about the attributes to WRITE names — the member the
/// document was carried in, which is the member the boundary validator would have named.
const UPDATABLE_PATH: &str = "updateImageSetMetadataUpdates.DICOMUpdates.updatableAttributes";

/// The `@pattern` both UID members declare, as a refusal quotes it: the expression without
/// the `^` / `$` anchors the model spells it with, which is how this service advertises
/// every pattern.
const UNANCHORED_UID_PATTERN: &str = "[0-9.]+";

/// The `@range` maximum of both study counts (`@range(min: 0, max: 1000000)`).
const MAX_STUDY_RELATED_COUNT: i64 = 1_000_000;

/// The message one refusal of a modelled attribute's value carries: the envelope and the
/// clause this service words every constraint refusal through, naming the blob member the
/// document was carried in and, in the requirement, the attribute that broke the rule.
fn expected_message(requirement: &str, keyword: &str) -> String {
    format!(
        "1 validation error detected: {UPDATABLE_PATH} failed to satisfy constraint: \
         {requirement} for {keyword}"
    )
}

/// The requirement a value longer than a member's `@length` maximum fails.
fn over_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a number above a member's `@range` maximum fails.
fn over_value(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The requirement a number below a member's `@range` minimum fails.
fn under_value(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement a value outside a member's `@pattern` fails.
fn outside_pattern() -> String {
    format!("Member must satisfy regular expression pattern: {UNANCHORED_UID_PATTERN}")
}

/// The requirement a value of the wrong KIND for a member fails.
fn wrong_kind(kind: &str) -> String {
    format!("Member must hold {kind}")
}

/// One value a member cannot carry, and the requirement it fails.
struct Illegal {
    case: &'static str,
    value: Value,
    requirement: String,
}

/// One modelled attribute: where the document carries it, which member holds it, and what
/// that member's constraints admit and refuse.
struct MemberCase {
    case: &'static str,
    /// The path the aggregated metadata document holds the attribute at, keyword last.
    path: Vec<&'static str>,
    /// The `DICOMTags` member a search summary reports it under.
    member: &'static str,
    /// Whether changing the attribute needs the `force` flag (an identifying keyword).
    force: bool,
    /// Values the member legally carries, at the bounds of what the model declares.
    legal: Vec<Value>,
    /// The ONE legal value the combined write uses for this member, so one document can
    /// carry every member at once.
    combined: Value,
    illegal: Vec<Illegal>,
}

/// The path of an attribute of the patient level.
fn patient(keyword: &'static str) -> Vec<&'static str> {
    vec!["Patient", "DICOM", keyword]
}

/// The path of an attribute of the study level.
fn study(keyword: &'static str) -> Vec<&'static str> {
    vec!["Study", "DICOM", keyword]
}

/// The path of an attribute of the one series a seeded image set holds — the series the
/// modelled members describe.
fn series(keyword: &'static str) -> Vec<&'static str> {
    vec!["Study", "Series", SERIES_UID, "DICOM", keyword]
}

/// A string of exactly `length` characters, for a case that writes a member's `@length`
/// maximum or one character past it.
fn filled(length: usize, filler: &str) -> String {
    filler.repeat(length)
}

/// One TEXT member's row: its `@length` maximum is written, one character past it is
/// refused, and a number is refused for its kind.
fn text_member(
    case: &'static str,
    path: Vec<&'static str>,
    member: &'static str,
    force: bool,
    max_length: usize,
) -> MemberCase {
    MemberCase {
        case,
        path,
        member,
        force,
        legal: vec![json!(filled(max_length, "A"))],
        combined: json!(filled(max_length, "A")),
        illegal: vec![
            Illegal {
                case: "one character past the length maximum",
                value: json!(filled(max_length + 1, "A")),
                requirement: over_length(max_length),
            },
            Illegal {
                case: "a number where the member holds text",
                value: json!(7),
                requirement: wrong_kind("a string"),
            },
        ],
    }
}

/// One INTEGER member's row: both `@range` bounds are written, one step past each is
/// refused, and text is refused for its kind.
fn integer_member(
    case: &'static str,
    path: Vec<&'static str>,
    member: &'static str,
    min: i64,
    max: i64,
) -> MemberCase {
    MemberCase {
        case,
        path,
        member,
        force: false,
        legal: vec![json!(min), json!(max)],
        combined: json!(max),
        illegal: vec![
            Illegal {
                case: "one below the range minimum",
                value: json!(min - 1),
                requirement: under_value(min),
            },
            Illegal {
                case: "one above the range maximum",
                value: json!(max + 1),
                requirement: over_value(max),
            },
            Illegal {
                case: "text where the member holds a number",
                value: json!("four"),
                requirement: wrong_kind("an integer"),
            },
        ],
    }
}

/// Every member of `DICOMTags` an update document can name, with the constraints the model
/// declares on it.
fn member_cases() -> Vec<MemberCase> {
    let mut cases = vec![
        text_member(
            "the patient identifier (len 0..512)",
            patient("PatientID"),
            "DICOMPatientId",
            false,
            512,
        ),
        text_member(
            "the patient name (len 0..512)",
            patient("PatientName"),
            "DICOMPatientName",
            false,
            512,
        ),
        text_member(
            "the patient birth date (len 0..36)",
            patient("PatientBirthDate"),
            "DICOMPatientBirthDate",
            false,
            36,
        ),
        text_member(
            "the patient sex (len 0..16)",
            patient("PatientSex"),
            "DICOMPatientSex",
            false,
            16,
        ),
        text_member(
            "the study identifier (len 0..512)",
            study("StudyID"),
            "DICOMStudyId",
            true,
            512,
        ),
        text_member(
            "the study description (len 0..512)",
            study("StudyDescription"),
            "DICOMStudyDescription",
            false,
            512,
        ),
        text_member(
            "the accession number (len 0..512)",
            study("AccessionNumber"),
            "DICOMAccessionNumber",
            false,
            512,
        ),
        text_member(
            "the study date (len 0..36)",
            study("StudyDate"),
            "DICOMStudyDate",
            false,
            36,
        ),
        text_member(
            "the study time (len 0..56)",
            study("StudyTime"),
            "DICOMStudyTime",
            false,
            56,
        ),
        text_member(
            "the series modality (len 0..32)",
            series("Modality"),
            "DICOMSeriesModality",
            false,
            32,
        ),
        text_member(
            "the series body part (len 0..128)",
            series("BodyPartExamined"),
            "DICOMSeriesBodyPart",
            false,
            128,
        ),
        integer_member(
            "the number of study related series (range 0..1000000)",
            study("NumberOfStudyRelatedSeries"),
            "DICOMNumberOfStudyRelatedSeries",
            0,
            MAX_STUDY_RELATED_COUNT,
        ),
        integer_member(
            "the number of study related instances (range 0..1000000)",
            study("NumberOfStudyRelatedInstances"),
            "DICOMNumberOfStudyRelatedInstances",
            0,
            MAX_STUDY_RELATED_COUNT,
        ),
        integer_member(
            "the series number (range -2147483648..2147483647)",
            series("SeriesNumber"),
            "DICOMSeriesNumber",
            i64::from(i32::MIN),
            i64::from(i32::MAX),
        ),
    ];
    // The two UID members carry a `@pattern` on top of their length, and both are
    // force-guarded: the keyword identifies where the content sits in the DICOM hierarchy.
    cases.push(MemberCase {
        case: "the study instance UID (len 0..512, pattern ^[0-9.]+$)",
        path: study("StudyInstanceUID"),
        member: "DICOMStudyInstanceUID",
        force: true,
        legal: vec![json!(filled(512, "9")), json!("1.2.840.10008.1.2.1")],
        combined: json!(filled(512, "9")),
        illegal: vec![
            Illegal {
                case: "one character past the length maximum",
                value: json!(filled(513, "9")),
                requirement: over_length(512),
            },
            Illegal {
                case: "text that is not a UID",
                value: json!("NOT-A-UID"),
                requirement: outside_pattern(),
            },
            Illegal {
                case: "a UID with no digit or dot in it at all",
                value: json!(""),
                requirement: outside_pattern(),
            },
            Illegal {
                case: "a number where the member holds text",
                value: json!(7),
                requirement: wrong_kind("a string"),
            },
        ],
    });
    cases.push(MemberCase {
        case: "the series instance UID (len 0..512, pattern ^[0-9.]+$)",
        // The legal value is the UID the image set already holds: a DIFFERENT legal UID
        // re-homes the series, which `crate::image_set_series_attributes` is the suite for.
        // What this row is about is which values the member admits at all.
        path: series("SeriesInstanceUID"),
        member: "DICOMSeriesInstanceUID",
        force: true,
        legal: vec![json!(SERIES_UID)],
        combined: json!(SERIES_UID),
        illegal: vec![
            Illegal {
                case: "one character past the length maximum",
                value: json!(filled(513, "9")),
                requirement: over_length(512),
            },
            Illegal {
                case: "text that is not a UID",
                value: json!("SERIES-ONE"),
                requirement: outside_pattern(),
            },
            Illegal {
                case: "a number where the member holds text",
                value: json!(7),
                requirement: wrong_kind("a string"),
            },
        ],
    });
    cases
}

/// The update document that writes `value` at `path`, carrying the schema declaration
/// every aggregated metadata document does.
fn document_writing(path: &[&str], value: Value) -> Value {
    let mut built = value;
    for name in path.iter().rev() {
        built = json!({ *name: built });
    }
    built["SchemaVersion"] = json!("1.1");
    built
}

/// The flags one write needs: the `force` flag for an attribute that identifies where the
/// content sits, nothing for any other.
fn flags(force: bool) -> Vec<(&'static str, bool)> {
    if force {
        vec![("force", true)]
    } else {
        Vec::new()
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_members_constraints_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("attribute.constraints", "token-attribute-constraints")
        .await;

    for member in member_cases() {
        let keyword = *member.path.last().expect("the keyword the path ends at");
        for illegal in &member.illegal {
            let case = format!("{}: {}", member.case, illegal.case);
            let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
            let before = read_document(&emulator, &datastore_id, &image_set_id, &case).await;

            let document = document_writing(&member.path, illegal.value.clone());
            let refused = update_metadata(
                &emulator,
                &datastore_id,
                &image_set_id,
                "1",
                dicom_updates(Some(&document), None),
                &flags(member.force),
            )
            .await;

            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {refused:?}"
            );
            assert_eq!(
                refused.string(&["message"]),
                expected_message(&illegal.requirement, keyword),
                "case {case:?}: expected the refusal to state the constraint the value \
                 failed, got {refused:?}"
            );
            // The refusal names the attribute, never the value: both documents are
            // `@sensitive`, so a value a caller composed is never reflected back.
            let reported = refused.string(&["message"]);
            let rendered = match &illegal.value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            assert!(
                rendered.is_empty() || !reported.contains(&rendered),
                "case {case:?}: expected the refusal to withhold the value it refused, got \
                 {reported:?}"
            );

            // Nothing was written: no new version, and the document is as it was.
            let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
            assert_eq!(
                (
                    read.string(&["versionId"]),
                    read.string(&["imageSetState"]),
                ),
                ("1".to_string(), "ACTIVE".to_string()),
                "case {case:?}: expected a refused update to leave version 1 ACTIVE, got \
                 {read:?}"
            );
            let after = read_document(&emulator, &datastore_id, &image_set_id, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected a refused update to leave the document as it was, \
                 got {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_a_modelled_member_admits_is_written_and_read_back() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("attribute.admitted", "token-attribute-admitted")
        .await;

    for member in member_cases() {
        for value in &member.legal {
            let case = format!("{}: {value}", member.case);
            let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
            let document = document_writing(&member.path, value.clone());

            let written = update_metadata(
                &emulator,
                &datastore_id,
                &image_set_id,
                "1",
                dicom_updates(Some(&document), None),
                &flags(member.force),
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected HTTP 200 from UpdateImageSetMetadata, got {written:?}"
            );
            assert_eq!(
                written.string(&["latestVersionId"]),
                "2",
                "case {case:?}: expected the admitted write to mint version 2, got {written:?}"
            );

            let served = read_document(&emulator, &datastore_id, &image_set_id, &case).await;
            assert_eq!(
                &at(&served, &member.path),
                value,
                "case {case:?}: expected the served document to report the written value at \
                 {:?}, got {served}",
                member.path
            );
        }
    }
}

#[tokio::test]
async fn a_search_reports_every_modelled_member_at_the_bound_of_its_constraints() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("attribute.searched", "token-attribute-searched")
        .await;
    let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
    let case = "every modelled member at the bound of its constraints";

    // One document carrying every member's extreme legal value, so what a search reports
    // for each member is what the one admitted write put in it.
    let members = member_cases();
    let mut document = json!({ "SchemaVersion": "1.1" });
    for member in &members {
        let mut cursor = &mut document;
        for name in &member.path[..member.path.len() - 1] {
            cursor = cursor
                .as_object_mut()
                .expect("a level of the document")
                .entry((*name).to_string())
                .or_insert_with(|| json!({}));
        }
        cursor[*member.path.last().expect("the keyword")] = member.combined.clone();
    }

    let written = update_metadata(
        &emulator,
        &datastore_id,
        &image_set_id,
        "1",
        dicom_updates(Some(&document), None),
        &[("force", true)],
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case {case:?}: expected HTTP 200 from UpdateImageSetMetadata, got {written:?}"
    );

    // The clock, not a reader, settles the update before the image set is searched.
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_UPDATE_DELAY).await;
    let answer = search(&emulator, &datastore_id, None, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from SearchImageSets, got {answer:?}"
    );
    let summary = answer
        .field(&["imageSetsMetadataSummaries"])
        .as_array()
        .into_iter()
        .flatten()
        .find(|summary| summary["imageSetId"].as_str() == Some(image_set_id.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary for the image set, got {answer:?}"));

    for member in &members {
        assert_eq!(
            summary["DICOMTags"][member.member],
            member.combined,
            "case {case:?}: expected DICOMTags.{} to report the written {:?}, got {summary}",
            member.member,
            member.combined
        );
    }
}
