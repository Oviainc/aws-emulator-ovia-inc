// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a `SearchImageSets` `BETWEEN` bound over
//! `DICOMStudyDateAndTime` means when it names no `DICOMStudyTime`.
//!
//! RULE: a study date and time bound that names no `DICOMStudyTime` bounds the WHOLE of
//! its study date, so a time-less lower bound starts at the first instant of its date and
//! a time-less upper bound runs to the last instant of its own; the pre-flight check that
//! the values are `(lowerBound, upperBound)` places the bounds exactly as the range's
//! evaluation does, so a range is refused with `ValidationException` (HTTP 400) only when
//! no study date and time could lie in it — bounds of one date therefore state a range
//! whenever either of them names no time, and are ordered by time only when both do.
//! `DICOMStudyTime` is optional on `DICOMStudyDateAndTime` and carries no minimum length
//! (`smithy_query(query="shape",shape="DICOMStudyDateAndTime")`), so a bound naming no
//! time and a bound naming the EMPTY time are different bounds: the first bounds the whole
//! date, the second is the position every stored time sorts at or above.
//!
//! The rows differ along what the rule generalises over: which of the two bounds names a
//! time (both, the lower only, the upper only, neither), whether the bounds share a date
//! or span several, where a named time sits against the stored times (below every one,
//! between them, above every one), the empty time on either bound, and the direction the
//! two bounds are written in. A range that is well-ordered but holds nothing, and an image
//! set carrying no study date at all, are both rows: the first is answered with an empty
//! page rather than refused, the second lies in no study-date range.
//!
//! Because the filters a token was minted under are part of the collection it is a
//! position in, the last suite drives the same distinction through a continuation token:
//! criteria naming no time and criteria naming the empty time select different image sets,
//! so neither one's token is a position in the other's walk.
//!
//! Every case drives real HTTP through the production front door
//! (`crate::datastore_lifecycle`'s harness); nothing is asserted through the state struct.
//!
//! ## Why a seeded image set
//!
//! The DICOM attributes a search matches on come into existence through a DICOM import,
//! which this emulator's import job does not do yet, so each image set is seeded through
//! the crate's public `state` module — exactly as `crate::image_set_search` seeds one —
//! and everything the cases then assert goes over the wire. The collection is this suite's
//! own because the rule needs one the filter suite's does not hold: several studies of ONE
//! date at different times, one study of that same date carrying no time at all, and dates
//! either side of it. The criteria and the answers are read with that suite's own
//! `search` and `reported_ids`, so only the seeded attributes are spelled here.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    service_state, INVALID_NEXT_TOKEN_MESSAGE,
};

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_copy::seed_image_set_in_hierarchy;
use crate::image_set_search::{reported_ids, search};

/// The earliest study date the suite seeds, which carries three studies with a time and
/// one with none.
const EARLY: &str = "19850412";

/// A study date between the other two, carrying one timed study.
const MIDDLE: &str = "20231231";

/// The latest study date the suite seeds, carrying one study with no time.
const LATE: &str = "20240101";

/// One image set to seed: the study date it holds, and the study time when it holds one.
///
/// `DICOMStudyDate` is documented as `yyMMdd` and `DICOMStudyTime` as `HHmmss.FFFFFF`
/// (`smithy_query(query="shape",shape="DICOMStudyDateAndTime")`).
#[derive(Debug, Clone, Copy)]
struct Seed {
    label: &'static str,
    study_date: Option<&'static str>,
    study_time: Option<&'static str>,
}

/// The seeded collection: one date holding studies at three times and one with no time of
/// its own, two further dates either side of it, and one image set carrying no study date
/// at all.
const SEEDS: [Seed; 7] = [
    Seed {
        label: "early-morning",
        study_date: Some(EARLY),
        study_time: Some("090000.000000"),
    },
    Seed {
        label: "early-noon",
        study_date: Some(EARLY),
        study_time: Some("120000.000000"),
    },
    Seed {
        label: "early-night",
        study_date: Some(EARLY),
        study_time: Some("232050.520000"),
    },
    Seed {
        label: "early-untimed",
        study_date: Some(EARLY),
        study_time: None,
    },
    Seed {
        label: "middle-noon",
        study_date: Some(MIDDLE),
        study_time: Some("120000.000000"),
    },
    Seed {
        label: "late-untimed",
        study_date: Some(LATE),
        study_time: None,
    },
    Seed {
        label: "no-study-date",
        study_date: None,
        study_time: None,
    },
];

/// Seed one image set holding `seed`'s study attributes and answer its identifier.
///
/// The record itself is built by the crate's own seeder
/// ([`seed_image_set_in_hierarchy`]), so it is shaped exactly like one a handler mints;
/// only the attributes a study-date range reads are written here.
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
    image_set.dicom_tags.dicom_study_date = seed.study_date.map(str::to_string);
    image_set.dicom_tags.dicom_study_time = seed.study_time.map(str::to_string);
    image_set_id
}

/// The seeded data store, and every seeded image set's identifier by its label.
struct Seeded {
    datastore_id: String,
    ids: Vec<(&'static str, String)>,
}

impl Seeded {
    async fn new(emulator: &Emulator, name: &str, client_token: &str) -> Self {
        let datastore_id = emulator.active_datastore(name, client_token).await;
        let ids = SEEDS
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

    /// The identifiers of the image sets labelled `labels`.
    fn ids_of(&self, labels: &[&str]) -> BTreeSet<String> {
        labels.iter().map(|label| self.id(label)).collect()
    }
}

/// One bound of a study-date range: a study date, and a study time only when the bound
/// names one.
#[derive(Debug, Clone, Copy)]
struct Bound {
    date: &'static str,
    time: Option<&'static str>,
}

/// A bound that names no time, which bounds the whole of `date`.
const fn whole_date(date: &'static str) -> Bound {
    Bound { date, time: None }
}

/// A bound that names `date` and `time`.
const fn at_time(date: &'static str, time: &'static str) -> Bound {
    Bound {
        date,
        time: Some(time),
    }
}

/// One bound as the `SearchByAttributeValue` the wire carries. A bound naming no time
/// carries no `DICOMStudyTime` member at all, which is not the same document as one
/// carrying it empty.
fn bound_value(bound: Bound) -> Value {
    let mut value = json!({ "DICOMStudyDate": bound.date });
    if let Some(time) = bound.time {
        value["DICOMStudyTime"] = json!(time);
    }
    json!({ "DICOMStudyDateAndTime": value })
}

/// The criteria holding one `BETWEEN` filter over the range `lower`..`upper` states.
fn between(lower: Bound, upper: Bound) -> Value {
    json!({
        "filters": [{
            "operator": "BETWEEN",
            "values": [bound_value(lower), bound_value(upper)],
        }]
    })
}

/// The criteria holding one `EQUAL` filter on `bound`.
fn equal(bound: Bound) -> Value {
    json!({ "filters": [{ "operator": "EQUAL", "values": [bound_value(bound)] }] })
}

/// One row of the accepted-range table: the bounds, and the image sets the range holds.
struct RangeCase {
    case: &'static str,
    lower: Bound,
    upper: Bound,
    expected: &'static [&'static str],
}

/// Every range the rule says states a usable range, with the image sets it holds.
fn range_cases() -> Vec<RangeCase> {
    vec![
        RangeCase {
            case: "a timed lower bound and a date-only upper bound of the same date",
            lower: at_time(EARLY, "120000.000000"),
            upper: whole_date(EARLY),
            expected: &["early-noon", "early-night"],
        },
        RangeCase {
            case: "a date-only lower bound and a timed upper bound of the same date",
            lower: whole_date(EARLY),
            upper: at_time(EARLY, "120000.000000"),
            expected: &["early-untimed", "early-morning", "early-noon"],
        },
        RangeCase {
            case: "both bounds the same whole date",
            lower: whole_date(EARLY),
            upper: whole_date(EARLY),
            expected: &[
                "early-untimed",
                "early-morning",
                "early-noon",
                "early-night",
            ],
        },
        RangeCase {
            case: "both bounds timed within one date",
            lower: at_time(EARLY, "100000.000000"),
            upper: at_time(EARLY, "235959.999999"),
            expected: &["early-noon", "early-night"],
        },
        RangeCase {
            case: "a timed lower bound and a date-only upper bound of a later date",
            lower: at_time(EARLY, "120000.000000"),
            upper: whole_date(LATE),
            expected: &[
                "early-noon",
                "early-night",
                "middle-noon",
                "late-untimed",
            ],
        },
        RangeCase {
            case: "a date-only lower bound and a timed upper bound of a later date",
            lower: whole_date(EARLY),
            upper: at_time(MIDDLE, "110000.000000"),
            expected: &[
                "early-untimed",
                "early-morning",
                "early-noon",
                "early-night",
            ],
        },
        RangeCase {
            case: "a date-only lower bound and a date-only upper bound spanning every date",
            lower: whole_date(EARLY),
            upper: whole_date(LATE),
            expected: &[
                "early-untimed",
                "early-morning",
                "early-noon",
                "early-night",
                "middle-noon",
                "late-untimed",
            ],
        },
        RangeCase {
            case: "a timed lower bound above every stored time of its date",
            lower: at_time(EARLY, "235959.999999"),
            upper: whole_date(EARLY),
            expected: &[],
        },
        RangeCase {
            case: "the empty time as the lower bound, which no stored time sorts below",
            lower: at_time(EARLY, ""),
            upper: whole_date(EARLY),
            expected: &[
                "early-untimed",
                "early-morning",
                "early-noon",
                "early-night",
            ],
        },
        RangeCase {
            case: "the empty time as the upper bound, which only a study with no time is at",
            lower: whole_date(EARLY),
            upper: at_time(EARLY, ""),
            expected: &["early-untimed"],
        },
    ]
}

#[tokio::test]
async fn a_study_date_bound_naming_no_time_bounds_the_whole_date() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.study.range", "search-study-range").await;

    for row in range_cases() {
        let answer = search(
            &emulator,
            &seeded.datastore_id,
            Some(between(row.lower, row.upper)),
            "maxResults=50",
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200 for the range {:?}..{:?}, got {answer:?}",
            row.case, row.lower, row.upper
        );
        let reported: BTreeSet<String> = reported_ids(&answer).into_iter().collect();
        assert_eq!(
            reported,
            seeded.ids_of(row.expected),
            "case {:?}: expected the image sets labelled {:?}, got {answer:?}",
            row.case,
            row.expected
        );
        assert!(
            answer.field(&["nextToken"]).is_null(),
            "case {:?}: a page that exhausts the result carries no nextToken, got {answer:?}",
            row.case
        );
    }
}

/// The sentence this service refuses an out-of-order range with: its one validation
/// envelope over the element path of the filter at fault and the ordering requirement.
const UNORDERED_BOUNDS_MESSAGE: &str = "1 validation error detected: \
     searchCriteria.filters.1.member.values failed to satisfy constraint: Member must hold \
     the lower bound before the upper bound";

/// One row of the refusal table: bounds no study date and time can lie between.
struct UnorderedCase {
    case: &'static str,
    lower: Bound,
    upper: Bound,
}

/// Every pair of bounds the rule says states no range at all: the lower bound's earliest
/// point is past the upper bound's latest one.
fn unordered_cases() -> Vec<UnorderedCase> {
    vec![
        UnorderedCase {
            case: "two whole dates the wrong way round",
            lower: whole_date(LATE),
            upper: whole_date(EARLY),
        },
        UnorderedCase {
            case: "two times of one date the wrong way round",
            lower: at_time(EARLY, "150000.000000"),
            upper: at_time(EARLY, "120000.000000"),
        },
        UnorderedCase {
            case: "a timed lower bound on a later date than a date-only upper bound",
            lower: at_time(LATE, "000000.000000"),
            upper: whole_date(EARLY),
        },
        UnorderedCase {
            case: "a date-only lower bound on a later date than a timed upper bound",
            lower: whole_date(LATE),
            upper: at_time(EARLY, "235959.999999"),
        },
        UnorderedCase {
            case: "the empty time as the upper bound of a timed lower bound",
            lower: at_time(EARLY, "120000.000000"),
            upper: at_time(EARLY, ""),
        },
    ]
}

#[tokio::test]
async fn bounds_no_study_date_and_time_can_lie_between_are_refused() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.study.unordered", "search-study-unordered").await;

    for row in unordered_cases() {
        let answer = search(
            &emulator,
            &seeded.datastore_id,
            Some(between(row.lower, row.upper)),
            "maxResults=50",
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400 for the bounds {:?}..{:?}, got {answer:?}",
            row.case, row.lower, row.upper
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {:?}: expected ValidationException, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer.string(&["message"]),
            UNORDERED_BOUNDS_MESSAGE,
            "case {:?}: expected the service's ordering refusal, got {answer:?}",
            row.case
        );
        assert!(
            answer.field(&["imageSetsMetadataSummaries"]).is_null(),
            "case {:?}: expected no page beside the refusal, got {answer:?}",
            row.case
        );
    }
}

/// One row of the token table: criteria naming no study time, and criteria naming the
/// empty one, which select different image sets.
struct TokenCase {
    case: &'static str,
    minted_under: Value,
    replayed_under: Value,
}

/// The same distinction through each operator a study date and time can be compared with
/// (`smithy_query(query="shape",shape="Operator")`: EQUAL, BETWEEN).
fn token_cases() -> Vec<TokenCase> {
    vec![
        TokenCase {
            case: "EQUAL on a whole date, replayed on the empty time of that date",
            minted_under: equal(whole_date(EARLY)),
            replayed_under: equal(at_time(EARLY, "")),
        },
        TokenCase {
            case: "BETWEEN over a whole date, replayed on a range ending at the empty time",
            minted_under: between(whole_date(EARLY), whole_date(EARLY)),
            replayed_under: between(whole_date(EARLY), at_time(EARLY, "")),
        },
    ]
}

/// Every image set one criteria's walk reports in a single page, in the order it walks
/// them.
async fn walk(emulator: &Emulator, datastore_id: &str, case: &str, criteria: &Value) -> Vec<String> {
    let answer = search(
        emulator,
        datastore_id,
        Some(criteria.clone()),
        "maxResults=50",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from an unpaged search, got {answer:?}"
    );
    reported_ids(&answer)
}

#[tokio::test]
async fn a_token_minted_under_a_time_less_bound_is_no_position_under_the_empty_time() {
    let emulator = Emulator::new();
    let seeded = Seeded::new(&emulator, "search.study.tokens", "search-study-tokens").await;

    for row in token_cases() {
        let minted_walk = walk(
            &emulator,
            &seeded.datastore_id,
            row.case,
            &row.minted_under,
        )
        .await;
        let replayed_walk = walk(
            &emulator,
            &seeded.datastore_id,
            row.case,
            &row.replayed_under,
        )
        .await;
        assert!(
            minted_walk.len() >= 2,
            "case {:?}: expected the minting criteria to select at least two image sets, got \
             {minted_walk:?}",
            row.case
        );
        assert_ne!(
            minted_walk.iter().collect::<BTreeSet<_>>(),
            replayed_walk.iter().collect::<BTreeSet<_>>(),
            "case {:?}: expected the two criteria to select different image sets, got \
             {replayed_walk:?}",
            row.case
        );

        let first_page = search(
            &emulator,
            &seeded.datastore_id,
            Some(row.minted_under.clone()),
            "maxResults=1",
        )
        .await;
        let token = first_page
            .field(&["nextToken"])
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a nextToken on a one-per-page search, got {first_page:?}",
                    row.case
                )
            })
            .to_string();

        let replayed = search(
            &emulator,
            &seeded.datastore_id,
            Some(row.replayed_under.clone()),
            &format!("maxResults=1&nextToken={token}"),
        )
        .await;
        assert_refused(row.case, &replayed);

        // The control: the same token is honoured under the criteria that minted it, and
        // resumes exactly where the first page stopped.
        let resumed = search(
            &emulator,
            &seeded.datastore_id,
            Some(row.minted_under.clone()),
            &format!("maxResults=1&nextToken={token}"),
        )
        .await;
        assert_eq!(
            resumed.status, 200,
            "case {:?}: expected the token to be honoured under the criteria that minted it, got \
             {resumed:?}",
            row.case
        );
        assert_eq!(
            reported_ids(&resumed),
            vec![minted_walk[1].clone()],
            "case {:?}: expected the resume to serve the image set after the boundary, got \
             {resumed:?}",
            row.case
        );
    }
}

/// Assert that `answer` is this service's refusal of a token that is not a position in the
/// walk the request asked for: HTTP 400, `ValidationException`, the fixed sentence, and no
/// page beside it.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for a token minted under other criteria, got {answer:?}"
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
