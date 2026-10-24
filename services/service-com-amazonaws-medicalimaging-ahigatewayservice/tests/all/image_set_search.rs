// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `SearchImageSets`.
//!
//! RULE: `SearchImageSets` answers HTTP 200 with the image sets the named data store
//! HOLDS whose PERSISTED attributes satisfy every filter the criteria carry — `EQUAL`
//! holding when the stored attribute equals the one value it names, `BETWEEN` when the
//! stored date or time lies in the inclusive range its lower and upper bound state —
//! ordered by the sort field and direction the request asks for (`updatedAt` descending
//! when it asks for none, which the response echoes), served one `maxResults`-sized page
//! at a time with an opaque token that resumes where the previous page stopped. Every
//! member of each summary comes from the record, and because the operation REPORTS,
//! nothing it reads changes: the `lastAccessedAt` it reports is the stored one — a search
//! is not an access of the content it describes — and two searches answer identically. A
//! criteria this service cannot evaluate is 400 `ValidationException`, and
//! a data store it does not hold is 404 `ResourceNotFoundException`.
//!
//! Every case drives real HTTP through the production front door, so what is asserted is
//! the wire behaviour a `medical-imaging` SDK client sees. The rows differ along what the
//! rule generalises over: which attribute a filter names, which operator it uses, how
//! many filters are ANDed, which sort field and direction order the result, what page
//! size walks it, and which part of a request cannot be evaluated.
//!
//! ## Why a seeded image set
//!
//! In AWS an image set — and the DICOM metadata a search matches on — comes into
//! existence through a DICOM import, which this emulator's import job does not do yet
//! (`issues/things-that-do-not-work/20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md`),
//! so each case seeds its image sets through the crate's public `state` module with
//! `ImageSet::new` — the same constructor the handlers mint image sets with — and then
//! asserts only over the wire. The one path that writes DICOM attributes today is a copy,
//! which carries its source's; the last suite drives that over HTTP end to end.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, service_state, ImageSet, ImageSetState, ImageSetWorkflowStatus,
    DEFAULT_STORAGE_TIER, IMAGE_SET_COPY_DELAY,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::{copy_image_set, information};

/// The instant the seeded timestamps are measured from — whole seconds, so each expected
/// timestamp compares exactly against the number on the wire.
const BASE_EPOCH_SECS: u64 = 1_767_225_600;

/// A seeded instant `offset_secs` past [`BASE_EPOCH_SECS`].
fn at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_EPOCH_SECS + offset_secs)
}

/// The number the wire carries for a seeded offset.
fn wire_instant(offset_secs: u64) -> f64 {
    (BASE_EPOCH_SECS + offset_secs) as f64
}

/// A well-formed `[0-9a-z]{32}` identifier no call ever mints (`mint_id` is SplitMix64
/// output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// One image set to seed, and the DICOM attributes it holds.
#[derive(Debug, Clone, Copy)]
struct ImageSetSeed {
    label: &'static str,
    patient_id: Option<&'static str>,
    accession_number: Option<&'static str>,
    study_id: Option<&'static str>,
    /// `DICOMStudyInstanceUID` / `DICOMSeriesInstanceUID` are `^[0-9.]+$`.
    study_instance_uid: Option<&'static str>,
    series_instance_uid: Option<&'static str>,
    /// `DICOMStudyDate`, and `DICOMStudyTime` in the `HHmmss.FFFFFF` form the model
    /// documents.
    study_date: Option<&'static str>,
    study_time: Option<&'static str>,
    is_primary: bool,
    created_offset_secs: u64,
    updated_offset_secs: u64,
    /// A tombstone: the data store keeps its properties for `GetImageSet`, but it holds
    /// no content, so no search reports it.
    deleted: bool,
}

/// The seeded collection. The rows differ along every attribute a filter can name, so no
/// two of the six sort orders below put them in the same sequence: two image sets share a
/// patient (one primary, one not), one carries a study date with no time, one carries no
/// DICOM content at all, and one is a deleted tombstone.
fn seeds() -> Vec<ImageSetSeed> {
    vec![
        ImageSetSeed {
            label: "alpha",
            patient_id: Some("PID-1"),
            accession_number: Some("ACC-1"),
            study_id: Some("STU-1"),
            study_instance_uid: Some("1.2.840.1"),
            series_instance_uid: Some("1.2.840.1.1"),
            study_date: Some("19850412"),
            study_time: Some("232050.520000"),
            is_primary: true,
            created_offset_secs: 0,
            updated_offset_secs: 100,
            deleted: false,
        },
        ImageSetSeed {
            label: "bravo",
            patient_id: Some("PID-1"),
            accession_number: Some("ACC-2"),
            study_id: Some("STU-2"),
            study_instance_uid: Some("1.2.840.2"),
            series_instance_uid: Some("1.2.840.2.1"),
            study_date: Some("19850412"),
            study_time: Some("090000.000000"),
            is_primary: false,
            created_offset_secs: 10,
            updated_offset_secs: 300,
            deleted: false,
        },
        ImageSetSeed {
            label: "charlie",
            patient_id: Some("PID-2"),
            accession_number: Some("ACC-3"),
            study_id: Some("STU-3"),
            study_instance_uid: Some("1.2.840.3"),
            series_instance_uid: Some("1.2.840.3.1"),
            study_date: Some("20240101"),
            study_time: None,
            is_primary: true,
            created_offset_secs: 20,
            updated_offset_secs: 200,
            deleted: false,
        },
        ImageSetSeed {
            label: "delta",
            patient_id: Some("PID-3"),
            accession_number: Some("ACC-4"),
            study_id: Some("STU-4"),
            study_instance_uid: Some("1.2.840.4"),
            series_instance_uid: Some("1.2.840.4.1"),
            study_date: Some("20231231"),
            study_time: Some("120000.000000"),
            is_primary: false,
            created_offset_secs: 30,
            updated_offset_secs: 50,
            deleted: false,
        },
        ImageSetSeed {
            label: "echo",
            patient_id: None,
            accession_number: None,
            study_id: None,
            study_instance_uid: None,
            series_instance_uid: None,
            study_date: None,
            study_time: None,
            is_primary: false,
            created_offset_secs: 40,
            updated_offset_secs: 40,
            deleted: false,
        },
        ImageSetSeed {
            label: "foxtrot",
            patient_id: Some("PID-1"),
            accession_number: Some("ACC-1"),
            study_id: Some("STU-1"),
            study_instance_uid: Some("1.2.840.1"),
            series_instance_uid: Some("1.2.840.1.1"),
            study_date: Some("19850412"),
            study_time: Some("232050.520000"),
            is_primary: true,
            created_offset_secs: 50,
            updated_offset_secs: 60,
            deleted: true,
        },
    ]
}

/// Seed one image set, shaped exactly like one a handler mints, and answer its id.
fn seed_one(emulator: &Emulator, datastore_id: &str, seed: &ImageSetSeed) -> String {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set_id = store.mint_id();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let (image_set_state, workflow_status) = if seed.deleted {
        (ImageSetState::Deleted, ImageSetWorkflowStatus::Deleted)
    } else {
        (ImageSetState::Active, ImageSetWorkflowStatus::Imported)
    };
    let mut image_set = ImageSet::new(
        image_set_id.clone(),
        image_set_arn(&datastore.datastore_arn, &image_set_id),
        image_set_state,
        workflow_status,
        seed.is_primary,
        false,
        at(seed.updated_offset_secs),
    );
    image_set.created_at = at(seed.created_offset_secs);
    // Nothing has accessed the content of a seeded image set, so its last access is the
    // instant it came into existence.
    image_set.last_accessed_at = at(seed.created_offset_secs);
    if seed.deleted {
        image_set.deleted_at = Some(at(seed.updated_offset_secs));
    }
    image_set.dicom_tags = DICOMTags {
        dicom_patient_id: seed.patient_id.map(str::to_string),
        dicom_accession_number: seed.accession_number.map(str::to_string),
        dicom_study_id: seed.study_id.map(str::to_string),
        dicom_study_instance_uid: seed.study_instance_uid.map(str::to_string),
        dicom_series_instance_uid: seed.series_instance_uid.map(str::to_string),
        dicom_study_date: seed.study_date.map(str::to_string),
        dicom_study_time: seed.study_time.map(str::to_string),
        ..DICOMTags::default()
    };
    datastore.image_sets.insert(image_set_id.clone(), image_set);
    image_set_id
}

/// The seeded data store, and every seeded image set's id by its label.
struct Seeded {
    datastore_id: String,
    ids: Vec<(&'static str, String)>,
}

impl Seeded {
    async fn new(emulator: &Emulator, name: &str, client_token: &str) -> Self {
        let datastore_id = emulator.active_datastore(name, client_token).await;
        let ids = seeds()
            .iter()
            .map(|seed| (seed.label, seed_one(emulator, &datastore_id, seed)))
            .collect();
        Self { datastore_id, ids }
    }

    /// The identifier the image set labelled `label` was minted with.
    fn id(&self, label: &str) -> String {
        self.ids
            .iter()
            .find(|(seeded, _)| *seeded == label)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("a seeded image set labelled {label}"))
    }

    /// The identifiers of the image sets labelled `labels`, in that order.
    fn ids_of(&self, labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| self.id(label)).collect()
    }
}

/// `POST /datastore/{datastoreId}/searchImageSets` — `SearchImageSets`. The body IS the
/// `searchCriteria` payload; `maxResults` and `nextToken` are `@httpQuery`.
///
/// `None` sends `{}`, the empty criteria — which filters and sorts by nothing, and is
/// what an SDK puts on the wire when the caller names no criteria. The payload itself is
/// never omitted: a `POST` with no body at all is not a JSON document, which the protocol
/// refuses before any handler runs.
pub(crate) async fn search(
    emulator: &Emulator,
    datastore_id: &str,
    criteria: Option<Value>,
    query: &str,
) -> Answer {
    let uri = if query.is_empty() {
        format!("/datastore/{datastore_id}/searchImageSets")
    } else {
        format!("/datastore/{datastore_id}/searchImageSets?{query}")
    };
    emulator
        .call("POST", &uri, Some(criteria.unwrap_or_else(|| json!({}))))
        .await
}

/// The image set identifiers one answer reports, in the order it reported them.
fn reported_ids(answer: &Answer) -> Vec<String> {
    answer
        .field(&["imageSetsMetadataSummaries"])
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["imageSetId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A criteria holding one filter.
fn one_filter(operator: &str, values: Value) -> Value {
    json!({ "filters": [{ "operator": operator, "values": values }] })
}

/// One row of the filter table.
struct FilterCase {
    case: &'static str,
    /// The criteria payload, `None` for the empty one.
    criteria: Option<Value>,
    /// The labels the search must report, in no particular order.
    expected: Vec<&'static str>,
}

/// The rows span every attribute `SearchByAttributeValue` declares
/// (`smithy_query(query="shape",shape="SearchByAttributeValue")`: DICOMPatientId,
/// DICOMAccessionNumber, DICOMStudyId, DICOMStudyInstanceUID, DICOMSeriesInstanceUID,
/// createdAt, updatedAt, DICOMStudyDateAndTime, isPrimary), both operators, a value
/// nothing holds, a study-date bound with and without a time, and two filters ANDed.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "criteria that filter by nothing",
            criteria: None,
            expected: vec!["alpha", "bravo", "charlie", "delta", "echo"],
        },
        FilterCase {
            case: "criteria carrying only a sort",
            criteria: Some(json!({ "sort": { "sortField": "createdAt", "sortOrder": "ASC" } })),
            expected: vec!["alpha", "bravo", "charlie", "delta", "echo"],
        },
        FilterCase {
            case: "EQUAL on a patient two image sets share",
            criteria: Some(one_filter("EQUAL", json!([{ "DICOMPatientId": "PID-1" }]))),
            expected: vec!["alpha", "bravo"],
        },
        FilterCase {
            case: "EQUAL on a patient nothing holds",
            criteria: Some(one_filter("EQUAL", json!([{ "DICOMPatientId": "PID-404" }]))),
            expected: vec![],
        },
        FilterCase {
            case: "EQUAL on an accession number",
            criteria: Some(one_filter(
                "EQUAL",
                json!([{ "DICOMAccessionNumber": "ACC-3" }]),
            )),
            expected: vec!["charlie"],
        },
        FilterCase {
            case: "EQUAL on a study id",
            criteria: Some(one_filter("EQUAL", json!([{ "DICOMStudyId": "STU-4" }]))),
            expected: vec!["delta"],
        },
        FilterCase {
            case: "EQUAL on a study instance UID",
            criteria: Some(one_filter(
                "EQUAL",
                json!([{ "DICOMStudyInstanceUID": "1.2.840.2" }]),
            )),
            expected: vec!["bravo"],
        },
        FilterCase {
            case: "EQUAL on a series instance UID",
            criteria: Some(one_filter(
                "EQUAL",
                json!([{ "DICOMSeriesInstanceUID": "1.2.840.3.1" }]),
            )),
            expected: vec!["charlie"],
        },
        FilterCase {
            case: "EQUAL on isPrimary true",
            criteria: Some(one_filter("EQUAL", json!([{ "isPrimary": true }]))),
            expected: vec!["alpha", "charlie"],
        },
        FilterCase {
            case: "EQUAL on isPrimary false",
            criteria: Some(one_filter("EQUAL", json!([{ "isPrimary": false }]))),
            expected: vec!["bravo", "delta", "echo"],
        },
        FilterCase {
            case: "EQUAL on one createdAt instant",
            criteria: Some(one_filter(
                "EQUAL",
                json!([{ "createdAt": wire_instant(20) }]),
            )),
            expected: vec!["charlie"],
        },
        FilterCase {
            case: "EQUAL on one updatedAt instant",
            criteria: Some(one_filter(
                "EQUAL",
                json!([{ "updatedAt": wire_instant(300) }]),
            )),
            expected: vec!["bravo"],
        },
        FilterCase {
            case: "EQUAL on a study date with no time, which bounds the whole date",
            criteria: Some(one_filter(
                "EQUAL",
                json!([{ "DICOMStudyDateAndTime": { "DICOMStudyDate": "19850412" } }]),
            )),
            expected: vec!["alpha", "bravo"],
        },
        FilterCase {
            case: "EQUAL on a study date and one time",
            criteria: Some(one_filter(
                "EQUAL",
                json!([{
                    "DICOMStudyDateAndTime": {
                        "DICOMStudyDate": "19850412",
                        "DICOMStudyTime": "090000.000000",
                    }
                }]),
            )),
            expected: vec!["bravo"],
        },
        FilterCase {
            case: "BETWEEN over createdAt",
            criteria: Some(one_filter(
                "BETWEEN",
                json!([
                    { "createdAt": wire_instant(10) },
                    { "createdAt": wire_instant(30) },
                ]),
            )),
            expected: vec!["bravo", "charlie", "delta"],
        },
        FilterCase {
            case: "BETWEEN over updatedAt",
            criteria: Some(one_filter(
                "BETWEEN",
                json!([
                    { "updatedAt": wire_instant(40) },
                    { "updatedAt": wire_instant(100) },
                ]),
            )),
            expected: vec!["alpha", "delta", "echo"],
        },
        FilterCase {
            case: "BETWEEN over study dates, each bound a whole date",
            criteria: Some(one_filter(
                "BETWEEN",
                json!([
                    { "DICOMStudyDateAndTime": { "DICOMStudyDate": "19850412" } },
                    { "DICOMStudyDateAndTime": { "DICOMStudyDate": "20231231" } },
                ]),
            )),
            expected: vec!["alpha", "bravo", "delta"],
        },
        FilterCase {
            case: "BETWEEN over one day's study times",
            criteria: Some(one_filter(
                "BETWEEN",
                json!([
                    {
                        "DICOMStudyDateAndTime": {
                            "DICOMStudyDate": "19850412",
                            "DICOMStudyTime": "100000.000000",
                        }
                    },
                    {
                        "DICOMStudyDateAndTime": {
                            "DICOMStudyDate": "19850412",
                            "DICOMStudyTime": "235959.999999",
                        }
                    },
                ]),
            )),
            expected: vec!["alpha"],
        },
        FilterCase {
            case: "two EQUAL filters, ANDed",
            criteria: Some(json!({
                "filters": [
                    { "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] },
                    { "operator": "EQUAL", "values": [{ "isPrimary": false }] },
                ]
            })),
            expected: vec!["bravo"],
        },
        FilterCase {
            case: "an EQUAL filter ANDed with a BETWEEN one",
            criteria: Some(json!({
                "filters": [
                    { "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] },
                    {
                        "operator": "BETWEEN",
                        "values": [
                            { "createdAt": wire_instant(0) },
                            { "createdAt": wire_instant(5) },
                        ]
                    },
                ]
            })),
            expected: vec!["alpha"],
        },
        FilterCase {
            case: "two filters that cannot both hold",
            criteria: Some(json!({
                "filters": [
                    { "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] },
                    { "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-2" }] },
                ]
            })),
            expected: vec![],
        },
    ]
}

#[tokio::test]
async fn search_reports_the_image_sets_every_filter_holds_of() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.filters", "filters").await;

    for row in filter_cases() {
        let answer = search(
            &emulator,
            &seeded.datastore_id,
            row.criteria.clone(),
            "maxResults=50",
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {answer:?}",
            row.case
        );
        let reported: BTreeSet<String> = reported_ids(&answer).into_iter().collect();
        let expected: BTreeSet<String> = seeded.ids_of(&row.expected).into_iter().collect();
        assert_eq!(
            reported, expected,
            "case {:?}: expected the image sets labelled {:?}, got {answer:?}",
            row.case, row.expected
        );
        assert!(
            answer.field(&["nextToken"]).is_null(),
            "case {:?}: a page that exhausts the result carries no nextToken, got {answer:?}",
            row.case
        );
    }

    // A deleted tombstone is reachable through `GetImageSet` and reported by no search:
    // every row above excludes it, including the one with no filters at all.
    let whole = search(&emulator, &seeded.datastore_id, None, "maxResults=50").await;
    assert!(
        !reported_ids(&whole).contains(&seeded.id("foxtrot")),
        "case \"a deleted tombstone\": a retired image set holds no content to report, got {whole:?}"
    );
}

#[tokio::test]
async fn every_reported_member_comes_from_the_persisted_image_set() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.summary", "summary").await;

    let answer = search(&emulator, &seeded.datastore_id, None, "maxResults=50").await;
    assert_eq!(
        answer.status, 200,
        "case \"the whole data store\": expected HTTP 200, got {answer:?}"
    );
    let summaries = answer
        .field(&["imageSetsMetadataSummaries"])
        .as_array()
        .cloned()
        .unwrap_or_default();

    for seed in seeds().iter().filter(|seed| !seed.deleted) {
        let image_set_id = seeded.id(seed.label);
        let summary = summaries
            .iter()
            .find(|summary| summary["imageSetId"].as_str() == Some(image_set_id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a summary for the seeded image set, got {answer:?}",
                    seed.label
                )
            });
        assert_eq!(
            summary["version"].as_i64(),
            Some(1),
            "case {:?}: expected the image set's latest version, got {summary:?}",
            seed.label
        );
        assert_eq!(
            summary["createdAt"].as_f64(),
            Some(wire_instant(seed.created_offset_secs)),
            "case {:?}: expected the image set's own createdAt, got {summary:?}",
            seed.label
        );
        assert_eq!(
            summary["updatedAt"].as_f64(),
            Some(wire_instant(seed.updated_offset_secs)),
            "case {:?}: expected the latest version's updatedAt, got {summary:?}",
            seed.label
        );
        assert_eq!(
            summary["isPrimary"],
            Value::Bool(seed.is_primary),
            "case {:?}: expected isPrimary {}, got {summary:?}",
            seed.label,
            seed.is_primary
        );
        assert_eq!(
            summary["storageTier"].as_str(),
            Some(DEFAULT_STORAGE_TIER),
            "case {:?}: expected the stored storage tier, got {summary:?}",
            seed.label
        );
        assert_eq!(
            summary["lastAccessedAt"].as_f64(),
            Some(wire_instant(seed.created_offset_secs)),
            "case {:?}: expected the stored last-access instant, which a search does not \
             move, got {summary:?}",
            seed.label
        );
        for (member, stored) in [
            ("DICOMPatientId", seed.patient_id),
            ("DICOMAccessionNumber", seed.accession_number),
            ("DICOMStudyId", seed.study_id),
            ("DICOMStudyInstanceUID", seed.study_instance_uid),
            ("DICOMSeriesInstanceUID", seed.series_instance_uid),
            ("DICOMStudyDate", seed.study_date),
            ("DICOMStudyTime", seed.study_time),
        ] {
            assert_eq!(
                summary["DICOMTags"][member].as_str(),
                stored,
                "case {:?}: expected DICOMTags.{member} to be the stored {stored:?}, got {summary:?}",
                seed.label
            );
        }
    }
}

/// One row of the order table: a sort to ask for, and the labels in the order it must
/// serve them.
struct SortCase {
    case: &'static str,
    /// The `sort` the criteria carry, or `None` to ask for no sort at all.
    sort: Option<(&'static str, &'static str)>,
    expected: Vec<&'static str>,
}

/// The rows span every `SortField` and every `SortOrder` the model declares
/// (`smithy_query(query="shape",shape="SortField,SortOrder")`), plus the documented
/// default. The seeded collection orders differently under each field, so no row could
/// pass by accident.
fn sort_cases() -> Vec<SortCase> {
    vec![
        SortCase {
            case: "no sort at all, which is updatedAt DESC",
            sort: None,
            expected: vec!["bravo", "charlie", "alpha", "delta", "echo"],
        },
        SortCase {
            case: "updatedAt DESC",
            sort: Some(("updatedAt", "DESC")),
            expected: vec!["bravo", "charlie", "alpha", "delta", "echo"],
        },
        SortCase {
            case: "updatedAt ASC",
            sort: Some(("updatedAt", "ASC")),
            expected: vec!["echo", "delta", "alpha", "charlie", "bravo"],
        },
        SortCase {
            case: "createdAt ASC",
            sort: Some(("createdAt", "ASC")),
            expected: vec!["alpha", "bravo", "charlie", "delta", "echo"],
        },
        SortCase {
            case: "createdAt DESC",
            sort: Some(("createdAt", "DESC")),
            expected: vec!["echo", "delta", "charlie", "bravo", "alpha"],
        },
        SortCase {
            // An image set carrying no study date ranks below every one that carries a
            // date: an absent value is the lowest.
            case: "DICOMStudyDateAndTime ASC",
            sort: Some(("DICOMStudyDateAndTime", "ASC")),
            expected: vec!["echo", "bravo", "alpha", "delta", "charlie"],
        },
        SortCase {
            case: "DICOMStudyDateAndTime DESC",
            sort: Some(("DICOMStudyDateAndTime", "DESC")),
            expected: vec!["charlie", "delta", "alpha", "bravo", "echo"],
        },
    ]
}

/// The criteria for one sort row.
fn sort_criteria(sort: Option<(&str, &str)>) -> Option<Value> {
    sort.map(|(field, order)| json!({ "sort": { "sortField": field, "sortOrder": order } }))
}

#[tokio::test]
async fn search_serves_the_order_the_request_asks_for_and_reports_it() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.sort", "sort").await;

    for row in sort_cases() {
        let answer = search(
            &emulator,
            &seeded.datastore_id,
            sort_criteria(row.sort),
            "maxResults=50",
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {answer:?}",
            row.case
        );
        assert_eq!(
            reported_ids(&answer),
            seeded.ids_of(&row.expected),
            "case {:?}: expected the image sets labelled {:?} in that order, got {answer:?}",
            row.case,
            row.expected
        );
        let (expected_field, expected_order) = row.sort.unwrap_or(("updatedAt", "DESC"));
        assert_eq!(
            answer.string(&["sort", "sortField"]),
            expected_field,
            "case {:?}: expected the served sortField, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer.string(&["sort", "sortOrder"]),
            expected_order,
            "case {:?}: expected the served sortOrder, got {answer:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn paging_walks_each_order_exactly_once() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.paging", "paging").await;

    // 1 and 50 are `maxResults`' published bounds; 2 and 4 straddle the five image sets
    // the search reports.
    for page_size in [1_usize, 2, 4, 50] {
        for row in sort_cases() {
            let expected = seeded.ids_of(&row.expected);
            let mut walked: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            // One page more than the collection can fill, so a token that never
            // terminates fails instead of looping.
            for page_number in 0..=expected.len() + 1 {
                let mut query = format!("maxResults={page_size}");
                if let Some(token) = &next_token {
                    query.push_str(&format!("&nextToken={token}"));
                }
                let page = search(
                    &emulator,
                    &seeded.datastore_id,
                    sort_criteria(row.sort),
                    &query,
                )
                .await;
                assert_eq!(
                    page.status, 200,
                    "case {:?} maxResults={page_size} page={page_number}: expected HTTP 200, got {page:?}",
                    row.case
                );
                let ids = reported_ids(&page);
                assert!(
                    ids.len() <= page_size,
                    "case {:?} maxResults={page_size} page={page_number}: expected at most {page_size} summaries, got {}",
                    row.case,
                    ids.len()
                );
                walked.extend(ids.clone());
                match page.field(&["nextToken"]).as_str() {
                    Some(token) => {
                        assert_eq!(
                            ids.len(),
                            page_size,
                            "case {:?} maxResults={page_size} page={page_number}: a page with a token must be full, got {page:?}",
                            row.case
                        );
                        next_token = Some(token.to_string());
                    }
                    None => {
                        next_token = None;
                        break;
                    }
                }
            }
            assert!(
                next_token.is_none(),
                "case {:?} maxResults={page_size}: paging did not terminate, got {walked:?}",
                row.case
            );
            assert_eq!(
                walked, expected,
                "case {:?} maxResults={page_size}: expected every image set once in the asked-for order, got {walked:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn searching_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.reads", "reads").await;

    let criteria = one_filter("EQUAL", json!([{ "DICOMPatientId": "PID-1" }]));
    let first = search(
        &emulator,
        &seeded.datastore_id,
        Some(criteria.clone()),
        "maxResults=50",
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case \"a filtered search\": expected HTTP 200, got {first:?}"
    );
    let again = search(
        &emulator,
        &seeded.datastore_id,
        Some(criteria.clone()),
        "maxResults=50",
    )
    .await;
    assert_eq!(
        again.body, first.body,
        "case \"a repeated search\": expected an identical answer, got {again:?} after {first:?}"
    );

    // And again past the longest span any image-set transition is configured with: a
    // search materialises only what the CLOCK already owed, and nothing is owed on a
    // seeded image set.
    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
    let settled = search(
        &emulator,
        &seeded.datastore_id,
        Some(criteria),
        "maxResults=50",
    )
    .await;
    assert_eq!(
        settled.body, first.body,
        "case \"a search past every configured span\": expected an identical answer, got {settled:?} after {first:?}"
    );

    // The whole collection is still exactly the collection it was: the searches above
    // retired nothing and minted nothing.
    let whole = search(&emulator, &seeded.datastore_id, None, "maxResults=50").await;
    assert_eq!(
        reported_ids(&whole).len(),
        seeds().iter().filter(|seed| !seed.deleted).count(),
        "case \"after the searches\": expected the seeded collection untouched, got {whole:?}"
    );
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// The data store the request names, or `None` for the seeded one.
    datastore_id: Option<String>,
    criteria: Option<Value>,
    query: &'static str,
    expected_status: u16,
    expected_code: &'static str,
}

/// The rows span: each operator given the wrong number of bounds; a range over an
/// attribute that is not a date or a time; bounds naming two different attributes; a
/// reversed range on each ordered attribute; more filters than the model admits; a sort
/// field outside the enum; each published `maxResults` bound overstepped; a data store
/// nothing holds; and an identifier outside its modelled pattern.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "EQUAL given two values",
            datastore_id: None,
            criteria: Some(one_filter(
                "EQUAL",
                json!([{ "DICOMPatientId": "PID-1" }, { "DICOMPatientId": "PID-2" }]),
            )),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "BETWEEN given one bound",
            datastore_id: None,
            criteria: Some(one_filter(
                "BETWEEN",
                json!([{ "createdAt": wire_instant(0) }]),
            )),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "BETWEEN over an attribute that is not a date or a time",
            datastore_id: None,
            criteria: Some(one_filter(
                "BETWEEN",
                json!([{ "DICOMPatientId": "PID-1" }, { "DICOMPatientId": "PID-2" }]),
            )),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "BETWEEN bounds naming two different attributes",
            datastore_id: None,
            criteria: Some(one_filter(
                "BETWEEN",
                json!([
                    { "createdAt": wire_instant(0) },
                    { "updatedAt": wire_instant(300) },
                ]),
            )),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "BETWEEN over createdAt with the bounds the wrong way round",
            datastore_id: None,
            criteria: Some(one_filter(
                "BETWEEN",
                json!([
                    { "createdAt": wire_instant(30) },
                    { "createdAt": wire_instant(10) },
                ]),
            )),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "BETWEEN over study dates with the bounds the wrong way round",
            datastore_id: None,
            criteria: Some(one_filter(
                "BETWEEN",
                json!([
                    { "DICOMStudyDateAndTime": { "DICOMStudyDate": "20240101" } },
                    { "DICOMStudyDateAndTime": { "DICOMStudyDate": "19850412" } },
                ]),
            )),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "more filters than the criteria admit",
            datastore_id: None,
            criteria: Some(json!({
                "filters": [
                    { "operator": "EQUAL", "values": [{ "DICOMPatientId": "PID-1" }] },
                    { "operator": "EQUAL", "values": [{ "DICOMStudyId": "STU-1" }] },
                    { "operator": "EQUAL", "values": [{ "isPrimary": true }] },
                ]
            })),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a filters list with nothing in it",
            datastore_id: None,
            criteria: Some(json!({ "filters": [] })),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a sortField outside the enum",
            datastore_id: None,
            criteria: Some(json!({
                "sort": { "sortField": "DICOMPatientId", "sortOrder": "ASC" }
            })),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a sortOrder outside the enum",
            datastore_id: None,
            criteria: Some(json!({
                "sort": { "sortField": "updatedAt", "sortOrder": "DESCENDING" }
            })),
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "maxResults below the published minimum",
            datastore_id: None,
            criteria: None,
            query: "maxResults=0",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "maxResults above the published maximum",
            datastore_id: None,
            criteria: None,
            query: "maxResults=51",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a nextToken that is not hex",
            datastore_id: None,
            criteria: None,
            query: "nextToken=not-a-token",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a nextToken that is hex this service never minted",
            datastore_id: None,
            criteria: None,
            query: "nextToken=abcdef",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a data store nothing holds",
            datastore_id: Some(unheld_id()),
            criteria: None,
            query: "",
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a datastoreId one character short of the pattern",
            datastore_id: Some("a".repeat(31)),
            criteria: None,
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            case: "a datastoreId carrying an upper-case character",
            datastore_id: Some(format!("A{}", "0".repeat(31))),
            criteria: None,
            query: "",
            expected_status: 400,
            expected_code: "ValidationException",
        },
    ]
}

#[tokio::test]
async fn search_refuses_what_it_cannot_evaluate() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.refusals", "refusals").await;

    for row in refusal_cases() {
        let datastore_id = row
            .datastore_id
            .clone()
            .unwrap_or_else(|| seeded.datastore_id.clone());
        let answer = search(&emulator, &datastore_id, row.criteria.clone(), row.query).await;
        assert_eq!(
            answer.status, row.expected_status,
            "case {:?}: expected HTTP {}, got {answer:?}",
            row.case, row.expected_status
        );
        assert_eq!(
            answer.error_code, row.expected_code,
            "case {:?}: expected {}, got {answer:?}",
            row.case, row.expected_code
        );
    }

    // A token this service DID mint is a position in the order it was minted under and in
    // no other: honouring it under another sort would skip or repeat image sets, so it is
    // refused exactly like a token the service never issued.
    let first_page = search(
        &emulator,
        &seeded.datastore_id,
        sort_criteria(Some(("createdAt", "ASC"))),
        "maxResults=1",
    )
    .await;
    let token = first_page.string(&["nextToken"]);
    assert!(
        !token.is_empty(),
        "case \"a token under another sort\": expected a first page with a token, got {first_page:?}"
    );
    for (case, sort) in [
        ("another field", Some(("updatedAt", "ASC"))),
        ("another direction", Some(("createdAt", "DESC"))),
        ("no sort at all", None),
    ] {
        let replayed = search(
            &emulator,
            &seeded.datastore_id,
            sort_criteria(sort),
            &format!("maxResults=1&nextToken={token}"),
        )
        .await;
        assert_eq!(
            replayed.status, 400,
            "case {case:?}: a token minted under another order must be refused, got {replayed:?}"
        );
        assert_eq!(
            replayed.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {replayed:?}"
        );
    }
    // Under the order it was minted under it resumes, so the refusals above are about the
    // order and not about the token.
    let resumed = search(
        &emulator,
        &seeded.datastore_id,
        sort_criteria(Some(("createdAt", "ASC"))),
        &format!("maxResults=1&nextToken={token}"),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case \"its own order\": expected the token to resume, got {resumed:?}"
    );
    assert_eq!(
        reported_ids(&resumed),
        seeded.ids_of(&["bravo"]),
        "case \"its own order\": expected the second image set of createdAt ASC, got {resumed:?}"
    );
}

#[tokio::test]
async fn a_copy_makes_its_destination_searchable_by_the_source_attributes() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.copy", "copy").await;
    let source_id = seeded.id("charlie");

    // A patient only the source holds, so the search answers about the copy alone.
    let criteria = one_filter("EQUAL", json!([{ "DICOMPatientId": "PID-2" }]));
    let copied = copy_image_set(
        &emulator,
        &seeded.datastore_id,
        &source_id,
        information("1", None, None),
        &[],
    )
    .await;
    assert_eq!(
        copied.status, 200,
        "case \"a copy into a minted image set\": expected HTTP 200, got {copied:?}"
    );
    let destination_id = copied.string(&["destinationImageSetProperties", "imageSetId"]);

    // While the copy is in flight the destination holds no DICOM content yet: the
    // attributes land with the content they describe.
    let in_flight = search(
        &emulator,
        &seeded.datastore_id,
        Some(criteria.clone()),
        "maxResults=50",
    )
    .await;
    assert_eq!(
        reported_ids(&in_flight),
        vec![source_id.clone()],
        "case \"while the copy is in flight\": expected only the source, got {in_flight:?}"
    );

    tokio::time::sleep(SETTLE_MULTIPLE * IMAGE_SET_COPY_DELAY).await;
    let landed = search(
        &emulator,
        &seeded.datastore_id,
        Some(criteria),
        "maxResults=50",
    )
    .await;
    let reported: BTreeSet<String> = reported_ids(&landed).into_iter().collect();
    let expected: BTreeSet<String> = [source_id, destination_id.clone()].into_iter().collect();
    assert_eq!(
        reported, expected,
        "case \"once the copy has landed\": expected the source and its copy, got {landed:?}"
    );

    // And the copy reports the source's attributes, member for member.
    let summaries = landed
        .field(&["imageSetsMetadataSummaries"])
        .as_array()
        .cloned()
        .unwrap_or_default();
    let destination = summaries
        .iter()
        .find(|summary| summary["imageSetId"].as_str() == Some(destination_id.as_str()))
        .unwrap_or_else(|| panic!("case \"once the copy has landed\": expected the copy's summary, got {landed:?}"));
    let source_seed = seeds()
        .into_iter()
        .find(|seed| seed.label == "charlie")
        .expect("the seeded source");
    for (member, stored) in [
        ("DICOMPatientId", source_seed.patient_id),
        ("DICOMAccessionNumber", source_seed.accession_number),
        ("DICOMStudyId", source_seed.study_id),
        ("DICOMStudyInstanceUID", source_seed.study_instance_uid),
        ("DICOMSeriesInstanceUID", source_seed.series_instance_uid),
        ("DICOMStudyDate", source_seed.study_date),
        ("DICOMStudyTime", source_seed.study_time),
    ] {
        assert_eq!(
            destination["DICOMTags"][member].as_str(),
            stored,
            "case \"once the copy has landed\": expected DICOMTags.{member} to be the source's {stored:?}, got {destination:?}"
        );
    }
}
