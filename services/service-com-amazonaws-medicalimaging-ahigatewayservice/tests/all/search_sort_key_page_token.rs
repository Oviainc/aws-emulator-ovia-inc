// SPDX-License-Identifier: CC0-1.0

//! The SORT KEY a `SearchImageSets` continuation token carries is honoured only when it is
//! spelled the way this service writes a key for the field the token was minted under.
//!
//! RULE: a `nextToken` is this service's own opaque handle on a position in one walk, so
//! every part of its decoded payload is read back only when it is spelled as the service
//! writes that part — for a search, the key of the ORDER the token was minted under: the
//! instant fields' key is a plain number of seconds and a plain sub-second remainder, and
//! the study field's key is the stored study date and the stored study time joined by the
//! service's own separator. A key spelled any other way is a position this service never
//! handed out, so it is refused with `ValidationException` (HTTP 400) carrying the
//! service's fixed invalid-token sentence rather than resumed from: resuming at a key no
//! record can hold walks past the image sets the search matched, and the caller's walk
//! then ends having been shown none of them.
//!
//! What the rule is NOT about is whether a record still sits at the key: a key spelled as
//! the service writes one is honoured even when no image set holds it, which is what keeps
//! a walk correct when the record a token was minted at has been deleted since. So each
//! sort below is driven with a well-spelled key beyond every stored one as a positive
//! control, and that walk must still report every matching image set.
//!
//! The suite is a TABLE over the two axes the rule generalises over: the ORDER a token is
//! minted under — each of the three `SortField` values the model declares, in each of the
//! two `SortOrder` directions (`smithy_query(query="shape",shape="SortField,SortOrder")`)
//! — and how a key can be written other than as the service writes it (sign, space
//! padding, a leading zero, a missing separator, a sub-second part past the values one
//! second's remainder holds, text that is no key at all, nothing at all). Every row edits
//! the key of a token the service really ISSUED, with the collection marker and the
//! tie-breaking identifier read back off that token rather than spelled again here.
//!
//! Every case drives real HTTP through the production front door
//! (`crate::datastore_lifecycle`'s harness); nothing is asserted through the state struct.
//!
//! ## Why a seeded image set
//!
//! The DICOM attributes a study sort ranks by come into existence through a DICOM import,
//! which this emulator's import job does not do yet, so each image set is seeded through
//! the crate's public `state` module — exactly as `crate::image_set_search` seeds one —
//! and everything the cases then assert goes over the wire.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, INVALID_NEXT_TOKEN_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_copy::seed_image_set_in_hierarchy;
use crate::image_set_search::{reported_ids, search};
use crate::page_token_payload_forgery::{split_issued_token, token_carrying};

/// The instant the seeded timestamps are measured from — whole seconds, so a key's
/// sub-second half is the one-character spelling the service writes for zero.
const BASE_EPOCH_SECS: u64 = 1_767_225_600;

/// A seeded instant `offset_secs` past [`BASE_EPOCH_SECS`].
fn at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_EPOCH_SECS + offset_secs)
}

/// One image set this suite seeds: the study date and time it holds, and the instants it
/// was created and last updated at — so each of the three sort fields ranks the seeded
/// image sets in an order of its own.
#[derive(Debug, Clone, Copy)]
struct Seed {
    /// `DICOMStudyDate`, and `DICOMStudyTime` in the `HHmmss.FFFFFF` form the model
    /// documents (`smithy_query(query="shape",shape="DICOMStudyDateAndTime")`).
    study_date: &'static str,
    study_time: Option<&'static str>,
    created_offset_secs: u64,
    updated_offset_secs: u64,
}

/// The seeded collection: two image sets sharing a study date and differing in their study
/// time, one carrying a study date with no time at all, and no two of them in the same
/// order under the three sort fields.
const SEEDS: [Seed; 3] = [
    Seed {
        study_date: "19850412",
        study_time: Some("232050.520000"),
        created_offset_secs: 0,
        updated_offset_secs: 300,
    },
    Seed {
        study_date: "19850412",
        study_time: Some("090000.000000"),
        created_offset_secs: 10,
        updated_offset_secs: 100,
    },
    Seed {
        study_date: "20240101",
        study_time: None,
        created_offset_secs: 20,
        updated_offset_secs: 200,
    },
];

/// Seed one image set holding `seed`'s attributes and instants, and answer its identifier.
///
/// The record itself is built by the crate's own seeder
/// ([`seed_image_set_in_hierarchy`]), so it is shaped exactly like one a handler mints;
/// only the attributes and instants a sort ranks by are written here.
fn seed_one(emulator: &Emulator, datastore_id: &str, seed: &Seed) -> String {
    let image_set_id = seed_image_set_in_hierarchy(emulator, datastore_id, 1, true);
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let image_set = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store")
        .image_sets
        .get_mut(&image_set_id)
        .expect("the seeded image set");
    image_set.created_at = at(seed.created_offset_secs);
    // Nothing has accessed a seeded image set's content, so its last access is the instant
    // it came into existence.
    image_set.last_accessed_at = at(seed.created_offset_secs);
    image_set.latest_version.updated_at = at(seed.updated_offset_secs);
    image_set.dicom_tags.dicom_study_date = Some(seed.study_date.to_string());
    image_set.dicom_tags.dicom_study_time = seed.study_time.map(str::to_string);
    image_set_id
}

/// A data store holding every seeded image set.
async fn seeded(emulator: &Emulator) -> String {
    let datastore_id = emulator
        .active_datastore("search.sort.key", "token-search-sort-key")
        .await;
    for seed in &SEEDS {
        seed_one(emulator, &datastore_id, seed);
    }
    datastore_id
}

/// The `searchCriteria` that names one sort and filters nothing.
fn sorted_by(field: &str, order: &str) -> Value {
    json!({ "sort": { "sortField": field, "sortOrder": order } })
}

/// How a sort field's key is SHAPED — which decides both how a row forges one and what a
/// well-spelled key beyond every stored one looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyShape {
    /// `updatedAt` and `createdAt` rank by an instant, whose key is a number of seconds
    /// and a sub-second remainder.
    Instant,
    /// `DICOMStudyDateAndTime` ranks by the stored study date and time, whose key joins
    /// the two with the service's own separator.
    Study,
}

/// The one order a case pages under.
#[derive(Debug, Clone, Copy)]
struct SortCase {
    /// A `SortField` value the model declares.
    field: &'static str,
    /// A `SortOrder` value the model declares.
    order: &'static str,
    shape: KeyShape,
}

/// Every order a token can be minted under: each modelled sort field in each modelled
/// direction.
const SORT_CASES: [SortCase; 6] = [
    SortCase {
        field: "updatedAt",
        order: "DESC",
        shape: KeyShape::Instant,
    },
    SortCase {
        field: "updatedAt",
        order: "ASC",
        shape: KeyShape::Instant,
    },
    SortCase {
        field: "createdAt",
        order: "DESC",
        shape: KeyShape::Instant,
    },
    SortCase {
        field: "createdAt",
        order: "ASC",
        shape: KeyShape::Instant,
    },
    SortCase {
        field: "DICOMStudyDateAndTime",
        order: "DESC",
        shape: KeyShape::Study,
    },
    SortCase {
        field: "DICOMStudyDateAndTime",
        order: "ASC",
        shape: KeyShape::Study,
    },
];

/// The character the service joins a study date and its time with, taken off the key it
/// ISSUED rather than spelled here: in the records this suite seeds it is the only control
/// character the key holds, and every row that edits the join works off that.
fn is_join(character: char) -> bool {
    character.is_control()
}

/// Every forged key one sort case replays, built from the key of a token the service
/// really issued.
///
/// The instant rows span how a number can be written other than as the service writes one
/// — signed, space-padded, zero-padded, no separator, a sub-second part at a whole second,
/// and the negative spelling the issue walked — and the study rows span the join the
/// service writes between the two halves: dropped, replaced, and one half on its own.
/// Both shapes also take text that is no key at all and no key at all.
fn forged_keys(case: &str, key: &str, shape: KeyShape) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = match shape {
        KeyShape::Instant => {
            let (secs, nanos) = key.split_once('.').unwrap_or_else(|| {
                panic!("case {case:?}: expected an instant-shaped sort key, got {key:?}")
            });
            vec![
                (
                    "a negative number of seconds, which no key this service writes holds"
                        .to_string(),
                    "-1.0".to_string(),
                ),
                (
                    "a signed number of seconds".to_string(),
                    format!("+{secs}.{nanos}"),
                ),
                (
                    "seconds padded with a space".to_string(),
                    format!(" {secs}.{nanos}"),
                ),
                (
                    "seconds padded with a leading zero".to_string(),
                    format!("0{secs}.{nanos}"),
                ),
                (
                    "a sub-second part padded with a leading zero".to_string(),
                    format!("{secs}.0{nanos}"),
                ),
                (
                    "a whole second of sub-second remainder".to_string(),
                    format!("{secs}.1000000000"),
                ),
                ("no fraction separator at all".to_string(), secs.to_string()),
            ]
        }
        KeyShape::Study => {
            let joined: String = key.chars().filter(|character| !is_join(*character)).collect();
            let rejoined: String = key
                .chars()
                .map(|character| if is_join(character) { '-' } else { character })
                .collect();
            let first_half: String = key.chars().take_while(|c| !is_join(*c)).collect();
            vec![
                (
                    "the two halves with the join dropped".to_string(),
                    joined,
                ),
                (
                    "the two halves joined by another character".to_string(),
                    rejoined,
                ),
                (
                    "the study date on its own, with no join at all".to_string(),
                    first_half,
                ),
            ]
        }
    };
    // Both shapes: text that is no key at all — the spelling the issue forged, which sorts
    // above every key this service writes — and nothing at all.
    rows.push((
        "text that is no key this service writes at all".to_string(),
        "zzzzzz".to_string(),
    ));
    rows.push(("an empty key".to_string(), String::new()));
    rows
}

/// A key spelled exactly as this service writes one for `shape`, at a position BEYOND
/// every seeded image set in `order` — so a walk resumed from it has every matching image
/// set still ahead of it.
///
/// The instant beyond every record is the epoch in `ASC` and the largest instant a key can
/// hold in `DESC`; the study rank beyond every record is the empty date and time in `ASC`
/// (an absent value is the lowest) and both halves raised above every stored digit in
/// `DESC`, each built off the issued key so the join stays the service's own.
fn well_spelled_key_before_everything(key: &str, shape: KeyShape, order: &str) -> String {
    match (shape, order) {
        (KeyShape::Instant, "ASC") => "0.0".to_string(),
        (KeyShape::Instant, _) => format!("{}.{}", i64::MAX, 999_999_999_u32),
        (KeyShape::Study, "ASC") => key.chars().filter(|c| is_join(*c)).collect(),
        (KeyShape::Study, _) => key
            .chars()
            .map(|character| if is_join(character) { character } else { 'z' })
            .collect(),
    }
}

/// Assert that `answer` is this service's refusal of a token it did not issue: HTTP 400,
/// `ValidationException`, the fixed sentence, and no page beside it.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for a sort key this service never wrote, got \
         {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.string(&["message"]),
        INVALID_NEXT_TOKEN_MESSAGE,
        "case {case:?}: expected the service's fixed invalid-token sentence, got {answer:?}"
    );
    assert!(
        answer.field(&["imageSetsMetadataSummaries"]).is_null(),
        "case {case:?}: expected no page beside the refusal, got {answer:?}"
    );
}

/// RULE: a search's continuation token is honoured only when its sort key is spelled as
/// this service writes a key for the field the token was minted under, so a token carrying
/// any other key is refused with `ValidationException` (HTTP 400) and the fixed
/// invalid-token sentence instead of being resumed from — which is what stops a forged key
/// from ending a walk that has reported none of the matching image sets.
#[tokio::test]
async fn a_forged_sort_key_is_refused_rather_than_resumed_from() {
    let emulator = Emulator::new();
    let datastore_id = seeded(&emulator).await;

    for sort in &SORT_CASES {
        let label = format!("{} {}", sort.field, sort.order);
        let criteria = sorted_by(sort.field, sort.order);

        // The whole result in one page, which every walk below is compared against.
        let whole = search(
            &emulator,
            &datastore_id,
            Some(criteria.clone()),
            "maxResults=50",
        )
        .await;
        assert_eq!(
            whole.status, 200,
            "case {label:?}: expected HTTP 200 from an unpaged search, got {whole:?}"
        );
        let records = reported_ids(&whole);
        assert_eq!(
            records.len(),
            SEEDS.len(),
            "case {label:?}: expected every seeded image set to match, got {records:?}"
        );

        // A token this service really issued, at the boundary after the first image set.
        let first_page = search(
            &emulator,
            &datastore_id,
            Some(criteria.clone()),
            "maxResults=1",
        )
        .await;
        assert_eq!(
            first_page.status, 200,
            "case {label:?}: expected HTTP 200 from the first page, got {first_page:?}"
        );
        let issued = first_page
            .field(&["nextToken"])
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: expected a nextToken on a one-per-page search, got \
                     {first_page:?}"
                )
            })
            .to_string();
        let (marker, key, identifier) = split_issued_token(&label, &issued);
        assert_eq!(
            identifier, records[1],
            "case {label:?}: expected the issued token to name the next image set, got \
             {identifier:?}"
        );

        for (row, forged_key) in forged_keys(&label, &key, sort.shape) {
            let case = format!("{label}: {row}");
            let forged = token_carrying(&marker, &format!("{forged_key}:{identifier}"));
            assert_ne!(
                forged, issued,
                "case {case:?}: the forged token must differ from the issued one"
            );
            let answer = search(
                &emulator,
                &datastore_id,
                Some(criteria.clone()),
                &format!("maxResults=1&nextToken={forged}"),
            )
            .await;
            assert_refused(&case, &answer);
        }

        // The first control: a key spelled as this service writes one is honoured even
        // though no seeded image set sits at it, and the walk it resumes still reports
        // every matching image set — the guard above is about the SPELLING of a key, not
        // about a record still holding it.
        let before_everything = token_carrying(
            &marker,
            &format!(
                "{}:{identifier}",
                well_spelled_key_before_everything(&key, sort.shape, sort.order)
            ),
        );
        let resumed_before = search(
            &emulator,
            &datastore_id,
            Some(criteria.clone()),
            &format!("maxResults=50&nextToken={before_everything}"),
        )
        .await;
        assert_eq!(
            resumed_before.status, 200,
            "case {label:?}: expected a well-spelled key to be honoured, got {resumed_before:?}"
        );
        assert_eq!(
            reported_ids(&resumed_before),
            records,
            "case {label:?}: expected a resume from a key before every image set to report \
             every one of them, got {resumed_before:?}"
        );

        // The second control: the unedited token still resumes exactly where the first page
        // stopped, so the refusals above are about the forged keys and not about paging.
        let resumed = search(
            &emulator,
            &datastore_id,
            Some(criteria.clone()),
            &format!("maxResults=1&nextToken={issued}"),
        )
        .await;
        assert_eq!(
            resumed.status, 200,
            "case {label:?}: expected the issued token to be honoured, got {resumed:?}"
        );
        assert_eq!(
            reported_ids(&resumed),
            vec![records[1].clone()],
            "case {label:?}: expected the resume to serve the image set after the boundary, \
             got {resumed:?}"
        );

        // The third control: the image set a forged key would have skipped is still served
        // by a walk that pages with the tokens this service mints, exactly once and in the
        // order the unpaged search reported.
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut query = "maxResults=1".to_string();
            if let Some(token) = &next_token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let page = search(&emulator, &datastore_id, Some(criteria.clone()), &query).await;
            assert_eq!(
                page.status, 200,
                "case {label:?} page {pages}: expected HTTP 200, got {page:?}"
            );
            walked.extend(reported_ids(&page));
            pages += 1;
            assert!(
                pages <= records.len(),
                "case {label:?}: paged further than the result can hold, got {walked:?}"
            );
            match page.field(&["nextToken"]).as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, records,
            "case {label:?}: expected the paged walk to visit every image set exactly once in \
             order, got {walked:?}"
        );
    }
}
