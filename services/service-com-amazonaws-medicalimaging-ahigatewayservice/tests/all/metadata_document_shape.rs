// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SHAPE every member this service documents as carrying a
//! document of DICOM attributes admits.
//!
//! RULE: such a member admits a JSON OBJECT and nothing else. Text that parses as some
//! other JSON value — a scalar, `null`, an array — carries no level of the aggregated
//! metadata document, so it names neither an attribute nor a SOP instance and is refused
//! with 400 `ValidationException` naming the member
//! (`Member must be a JSON object of DICOM attributes`), exactly as text that does not
//! parse as JSON at all is (`Member must be a JSON document`). Neither refusal echoes the
//! value, which the model marks `@sensitive`, and neither writes anything: no version is
//! pushed and no image set is minted.
//!
//! The members the rule covers are the three this service documents that way:
//! `CopyImageSet`'s `copyImageSetInformation.sourceImageSet.DICOMCopies.copiableAttributes`
//! ("the JSON string used to specify a subset of SOP Instances to copy", named in the shape
//! of the aggregated metadata document) and `UpdateImageSetMetadata`'s
//! `updateImageSetMetadataUpdates.DICOMUpdates.updatableAttributes` and
//! `.removableAttributes` ("the DICOM tags" to update or to remove) —
//! `smithy_query(query="shape",shape="MetadataCopies,MetadataUpdates")` constrains only
//! their length, so the shape is the service's own check and has to read the same on all
//! three.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! member carries the value, and what the text parses to — an object (empty, carrying only
//! the keys a document states about itself, carrying one attribute, nested down to the
//! instance level), another JSON value (empty and non-empty array, string, number,
//! boolean, `null`) and text that is no JSON at all (truncated, unterminated, blank,
//! trailing content, not JSON in any form). Every case drives real HTTP through the
//! production front door, so what is asserted is the wire behaviour a `medical-imaging`
//! SDK client sees.
//!
//! ## Why a seeded image set
//!
//! In AWS the DICOM content of an image set arrives through a DICOM import, which this
//! emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image set through the crate's public `state` module with the same
//! `ImageSet::new` the handlers use, and then asserts only over the wire.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    METADATA_DOCUMENT_REQUIREMENT, METADATA_OBJECT_REQUIREMENT,
};

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_copy::{copy_image_set, get_image_set, information};
use crate::image_set_metadata_updates::{
    dicom_updates_raw, seed_image_set, seeded_tags, update_metadata, SERIES_UID, STUDY_UID,
};
use crate::image_set_search::search;

/// One member documented as carrying a document of DICOM attributes.
#[derive(Debug, Clone, Copy)]
enum Member {
    /// `CopyImageSet`'s subset selector, `copiableAttributes`.
    Copiable,
    /// `UpdateImageSetMetadata`'s attributes to write, `updatableAttributes`.
    Updatable,
    /// `UpdateImageSetMetadata`'s attributes to remove, `removableAttributes`.
    Removable,
}

impl Member {
    /// The member's own name, which its refusal has to name.
    fn name(self) -> &'static str {
        match self {
            Member::Copiable => "copiableAttributes",
            Member::Updatable => "updatableAttributes",
            Member::Removable => "removableAttributes",
        }
    }

    /// Send `text` as this member's value, against an image set at version 1.
    async fn send(
        self,
        emulator: &Emulator,
        datastore_id: &str,
        image_set_id: &str,
        text: &str,
    ) -> Answer {
        match self {
            Member::Copiable => {
                copy_image_set(
                    emulator,
                    datastore_id,
                    image_set_id,
                    information("1", Some(text), None),
                    &[],
                )
                .await
            }
            Member::Updatable => {
                update_metadata(
                    emulator,
                    datastore_id,
                    image_set_id,
                    "1",
                    dicom_updates_raw(Some(text), None),
                    &[],
                )
                .await
            }
            Member::Removable => {
                update_metadata(
                    emulator,
                    datastore_id,
                    image_set_id,
                    "1",
                    dicom_updates_raw(None, Some(text)),
                    &[],
                )
                .await
            }
        }
    }
}

/// Every member the rule covers.
const MEMBERS: [Member; 3] = [Member::Copiable, Member::Updatable, Member::Removable];

/// What a member's text parses to — which decides how the service has to answer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parsed {
    /// A JSON object: a document of DICOM attributes, which is admitted.
    Object,
    /// JSON, but some other value: refused for its shape.
    OtherJson,
    /// Not JSON at all: refused for not being a document.
    NotJson,
}

impl Parsed {
    /// The requirement the member states for this text, or `None` where it states none
    /// because the text is admitted.
    fn requirement(self) -> Option<&'static str> {
        match self {
            Parsed::Object => None,
            Parsed::OtherJson => Some(METADATA_OBJECT_REQUIREMENT),
            Parsed::NotJson => Some(METADATA_DOCUMENT_REQUIREMENT),
        }
    }
}

/// One row of the shape table: the text a member carries and what it parses to.
struct FormCase {
    case: &'static str,
    text: String,
    parsed: Parsed,
}

/// The rows span the JSON forms a caller can send: four objects (empty, carrying only the
/// keys a document states about itself, carrying one attribute, nested to the instance
/// level of the series the seeded image set holds), every other JSON value an RFC 8259
/// document can be (array — empty and not —, string, integer, fractional number, both
/// booleans, `null`), and text that is no JSON at all (truncated object and array,
/// unterminated string, a blank value, trailing content after a complete document, and a
/// `keyword=value` form that is not JSON in any shape).
fn form_cases() -> Vec<FormCase> {
    vec![
        FormCase {
            case: "an empty object: a document naming nothing",
            text: json!({}).to_string(),
            parsed: Parsed::Object,
        },
        FormCase {
            case: "an object of only the keys a document states about itself",
            text: json!({ "SchemaVersion": "1.1" }).to_string(),
            parsed: Parsed::Object,
        },
        FormCase {
            case: "an object naming one patient-level attribute",
            text: json!({ "Patient": { "DICOM": { "PatientName": "DOE^JOHN" } } }).to_string(),
            parsed: Parsed::Object,
        },
        FormCase {
            case: "an object nested down to the instance level",
            text: json!({
                "Study": { "Series": { SERIES_UID: { "Instances": {} } } },
            })
            .to_string(),
            parsed: Parsed::Object,
        },
        FormCase {
            case: "an empty JSON array",
            text: json!([]).to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "a JSON array of objects",
            text: json!([{ "SOPInstanceUID": "1.2.840.113619.2.1.1.7" }]).to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "a JSON string",
            text: json!("PatientName").to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "a JSON integer",
            text: json!(123).to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "a fractional JSON number",
            text: json!(-45.5).to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "JSON true",
            text: json!(true).to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "JSON false",
            text: json!(false).to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "JSON null",
            text: json!(null).to_string(),
            parsed: Parsed::OtherJson,
        },
        FormCase {
            case: "a truncated object",
            text: "{".to_string(),
            parsed: Parsed::NotJson,
        },
        FormCase {
            case: "a truncated array",
            text: "[456,789".to_string(),
            parsed: Parsed::NotJson,
        },
        FormCase {
            case: "an object with a member and no value",
            text: r#"{"Study":}"#.to_string(),
            parsed: Parsed::NotJson,
        },
        FormCase {
            case: "an unterminated string value",
            text: r#"{"Study": "unterminated}"#.to_string(),
            parsed: Parsed::NotJson,
        },
        FormCase {
            case: "a blank value",
            text: " ".to_string(),
            parsed: Parsed::NotJson,
        },
        FormCase {
            case: "trailing content after a complete document",
            text: r#"{"Patient":{}} {"Patient":{}}"#.to_string(),
            parsed: Parsed::NotJson,
        },
        FormCase {
            case: "a keyword=value form that is no JSON",
            text: "PatientName=ROE^RICHARD".to_string(),
            parsed: Parsed::NotJson,
        },
    ]
}

/// Every member documented as carrying a document of DICOM attributes answers one rule:
/// an object is admitted, every other JSON value and every text that is not JSON is
/// refused — each with its own requirement, naming the member, echoing nothing and
/// writing nothing.
#[tokio::test]
async fn every_metadata_document_member_admits_an_object_and_refuses_every_other_value() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("document.shape", "token-document-shape")
        .await;

    for form in form_cases() {
        for member in MEMBERS {
            let case = format!("{} / {}", form.case, member.name());
            // A fresh image set per case: an admitted call locks the one it names, so no
            // case can be answered out of what another case left behind.
            let image_set_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
            let answer = member
                .send(&emulator, &datastore_id, &image_set_id, &form.text)
                .await;

            let Some(requirement) = form.parsed.requirement() else {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a JSON object of DICOM attributes must be admitted, got \
                     {answer:?}"
                );
                assert_eq!(
                    answer.error_code, "",
                    "case {case:?}: an admitted call reports no error code, got {answer:?}"
                );
                continue;
            };

            assert_eq!(
                answer.status, 400,
                "case {case:?}: a value that is not a JSON object of DICOM attributes must be \
                 refused, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: wrong error code, got {answer:?}"
            );
            let message = answer.string(&["message"]);
            assert!(
                message.contains(member.name()),
                "case {case:?}: the refusal must name the member, got {message:?}"
            );
            assert!(
                message.ends_with(requirement),
                "case {case:?}: expected the refusal to state {requirement:?}, got {message:?}"
            );
            let trimmed = form.text.trim();
            assert!(
                trimmed.is_empty() || !message.contains(trimmed),
                "case {case:?}: a `@sensitive` value must not be echoed, got {message:?}"
            );

            // And the refusal wrote nothing: the image set is still ACTIVE at version 1,
            // neither locked for a copy nor carrying a new version.
            let read = get_image_set(&emulator, &datastore_id, &image_set_id, None).await;
            assert_eq!(
                read.string(&["imageSetState"]),
                "ACTIVE",
                "case {case:?}: a refusal must leave the image set untouched, got {read:?}"
            );
            assert_eq!(
                read.string(&["versionId"]),
                "1",
                "case {case:?}: a refusal must push no version, got {read:?}"
            );
        }
    }
}

/// The image set identifiers the data store reports, sorted — the records a copy either
/// added to or did not.
async fn held_image_sets(emulator: &Emulator, datastore_id: &str, case: &str) -> Vec<String> {
    let answer = search(emulator, datastore_id, None, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from SearchImageSets, got {answer:?}"
    );
    let mut ids: Vec<String> = answer
        .field(&["imageSetsMetadataSummaries"])
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["imageSetId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// A selector the service cannot read is not honoured as "select nothing": the copy is
/// refused outright, so no destination image set comes into existence — where a selector
/// that IS a document, naming no instance, mints one.
#[tokio::test]
async fn a_selector_that_is_not_a_document_mints_no_destination() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("selector.shape", "token-selector-shape")
        .await;
    let source_id = seed_image_set(&emulator, &datastore_id, seeded_tags(STUDY_UID));
    let before = held_image_sets(&emulator, &datastore_id, "seeded").await;
    assert_eq!(
        before,
        vec![source_id.clone()],
        "case \"seeded\": expected the data store to hold only the source, got {before:?}"
    );

    // Rows: the JSON forms a caller most easily sends in place of a document, each of
    // which the handler used to honour as an empty selection.
    for text in ["123", "[]", r#""abc""#, "null", "true"] {
        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &source_id,
            information("1", Some(text), None),
            &[],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {text:?}: a selector that is not a document must be refused, got {refused:?}"
        );
        let held = held_image_sets(&emulator, &datastore_id, text).await;
        assert_eq!(
            held,
            vec![source_id.clone()],
            "case {text:?}: a refused copy must mint no destination, got {held:?}"
        );
    }

    // The contrast: a document that names no instance IS a selector, so the copy is
    // admitted and a destination is minted.
    let selector: Value = json!({ "Study": { "Series": {} } });
    let admitted = copy_image_set(
        &emulator,
        &datastore_id,
        &source_id,
        information("1", Some(&selector.to_string()), None),
        &[],
    )
    .await;
    assert_eq!(
        admitted.status, 200,
        "case \"a document naming no instance\": expected HTTP 200, got {admitted:?}"
    );
    let destination_id = admitted.string(&["destinationImageSetProperties", "imageSetId"]);
    let held = held_image_sets(&emulator, &datastore_id, "a document naming no instance").await;
    let mut expected = vec![source_id.clone(), destination_id.clone()];
    expected.sort();
    assert_eq!(
        held, expected,
        "case \"a document naming no instance\": expected the minted destination \
         {destination_id:?} beside the source, got {held:?}"
    );
}
