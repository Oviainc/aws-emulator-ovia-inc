// SPDX-License-Identifier: CC0-1.0

//! `BatchAssociateDataSegmentsToDataset` end to end: the data segments a curated
//! dataset curates are associated over HTTP, through the same `MultiServiceRouter`
//! the emulator boots with.
//!
//! The rule these suites pin: an association entry selects ONE slice of one time
//! series — the half-open span it names — out of the source session dataset it
//! names, and the curated dataset the request addresses then holds that slice, so
//! an entry the store can place is persisted and counted into the dataset's next
//! version, while an entry whose span covers no time, whose source dataset or time
//! series the store does not hold, whose source is not a session dataset, or whose
//! span overlaps a data segment of the same time series already associated with any
//! dataset, fails on its own with its own `DataSegmentErrorCode` and stores
//! nothing.
//!
//! The table below spans the entries a request can carry: one slice, two adjacent
//! slices, a slice whose bounds carry nanosecond offsets, two series overlapping in
//! time, two sources in one batch, a slice overlapping one the same call stored, a
//! repeat of an entry inside one batch, spans covering no time, an absent time
//! series, an absent, foreign and non-session source, the addressed dataset as its
//! own source, and batches of one, two and five entries. The whole-request refusals
//! (an unknown dataset, a dataset of another workspace, a dataset that is not a
//! curated dataset), the overlap rule across datasets, the data a source dataset
//! holds of its own, and the per-dataset bound are their own suites.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`) is the one the
//! asset suite defines, and the dataset helpers (`create_dataset`, `ingest`,
//! `time_series_id`, `instant`, `wire_instant`, `relative`) are the ones the
//! deletion suite beside this one defines; both are reused rather than copied.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    empty_span_message, not_found_message, service_state, DataSegmentRecord, ACTIVATION_DELAY,
    MAX_DATA_SEGMENTS_PER_DATASET, RESOURCE_DATASET, RESOURCE_TIME_SERIES,
};

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::dataset_data_segments::{
    create_dataset, ingest, instant, relative, time_series_id, wire_instant,
};

/// The workspace the datasets of these suites live in.
const WORKSPACE: &str = "glass-line";

/// A second workspace, so a row can prove a source dataset is resolved inside the
/// workspace the request names.
const OTHER_WORKSPACE: &str = "paint-line";

/// The alias of the first seeded data stream — the series most rows associate
/// slices of.
const FIRST_ALIAS: &str = "/plant/glass/1/thickness";

/// The alias of the second seeded data stream, so a row can prove the overlap rule
/// is asked of ONE time series and not of time itself.
const SECOND_ALIAS: &str = "/plant/glass/2/thickness";

/// A `TimeSeriesId` no stored stream carries: an `ID`-shaped value inside the
/// modelled `TimeSeriesId` length (36..73).
const ABSENT_TIME_SERIES: &str = "7b1e40c3-6d52-42af-9c08-1f4e5a2b8d60";

/// A `datasetId` no stored dataset carries.
const ABSENT_DATASET: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The sample seconds, relative to the suite's anchor, each stream is ingested
/// with — enough to create the stream the entries then name.
const SAMPLES: &[i64] = &[0, 5, 10, 15];

/// The `DataSegmentErrorCode` an entry naming a resource the store does not hold
/// reports. Source: `smithy_query(query="shape", shape="DataSegmentErrorCode")` —
/// INTERNAL_FAILURE | VALIDATION_ERROR | RESOURCE_NOT_FOUND | LIMIT_EXCEEDED |
/// CONFLICTING_OPERATION.
const RESOURCE_NOT_FOUND: &str = "RESOURCE_NOT_FOUND";

/// The `DataSegmentErrorCode` an entry the request itself states wrongly reports;
/// see [`RESOURCE_NOT_FOUND`] for the value list.
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// The `DataSegmentErrorCode` an entry overlapping an association already held
/// reports; see [`RESOURCE_NOT_FOUND`] for the value list.
const CONFLICTING_OPERATION: &str = "CONFLICTING_OPERATION";

/// The `DataSegmentErrorCode` an entry the dataset has no room for reports; see
/// [`RESOURCE_NOT_FOUND`] for the value list.
const LIMIT_EXCEEDED: &str = "LIMIT_EXCEEDED";

/// The code a row expects for an entry the call APPLIED, which is reported in no
/// `failedAssociations` element at all.
const APPLIED: &str = "";

/// Which seeded time series an entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Series {
    /// The stream [`FIRST_ALIAS`] names.
    First,
    /// The stream [`SECOND_ALIAS`] names.
    Second,
    /// [`ABSENT_TIME_SERIES`]: a `TimeSeriesId` no stored stream carries.
    Absent,
}

/// Which dataset an entry names as the source its slice came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The session dataset of [`WORKSPACE`]: what a curated dataset selects
    /// slices from.
    Session,
    /// A second session dataset of the same workspace, so one batch can select
    /// from two sources.
    OtherSession,
    /// Another CURATED dataset of the same workspace: not a dataset slices are
    /// selected from.
    Curated,
    /// The curated dataset the request itself addresses.
    Target,
    /// A session dataset of [`OTHER_WORKSPACE`].
    Foreign,
    /// [`ABSENT_DATASET`].
    Absent,
}

/// One `AssociateDataSegmentEntry`, as a row writes it: the source dataset and
/// time series it names and the half-open span it covers, in seconds relative to
/// the suite's anchor plus a nanosecond offset.
#[derive(Debug, Clone, Copy)]
struct Entry {
    source: Source,
    series: Series,
    start: (i64, i32),
    end: (i64, i32),
}

/// The entry most rows are written around: the first ten seconds of the first
/// series, selected from the session dataset.
const FIRST_EARLY: Entry = Entry {
    source: Source::Session,
    series: Series::First,
    start: (0, 0),
    end: (10, 0),
};

/// One row of the association table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The entries the request carries, in order.
    entries: &'static [Entry],
    /// What each entry answers, in the same order: [`APPLIED`], or the
    /// `DataSegmentErrorCode` it is reported in `failedAssociations` with.
    expected: &'static [&'static str],
    /// The indices into [`Row::entries`] the curated dataset must hold a segment
    /// for afterwards.
    stored: &'static [usize],
    /// The `datasetVersion` the call reports: the created dataset is at 1, and a
    /// call that stored at least one segment takes it to 2.
    version: &'static str,
}

/// Five entries, mixing every outcome one batch can carry.
const FIVE_ENTRIES: &[Entry] = &[
    FIRST_EARLY,
    Entry {
        source: Source::Session,
        series: Series::Absent,
        start: (0, 0),
        end: (10, 0),
    },
    Entry {
        source: Source::Session,
        series: Series::First,
        start: (5, 0),
        end: (15, 0),
    },
    Entry {
        source: Source::Absent,
        series: Series::First,
        start: (100, 0),
        end: (110, 0),
    },
    Entry {
        source: Source::OtherSession,
        series: Series::Second,
        start: (0, 0),
        end: (10, 0),
    },
];

fn table() -> Vec<Row> {
    vec![
        Row {
            case: "one slice of a series",
            entries: &[FIRST_EARLY],
            expected: &[APPLIED],
            stored: &[0],
            version: "2",
        },
        Row {
            case: "two adjacent slices of one series",
            entries: &[
                FIRST_EARLY,
                Entry {
                    source: Source::Session,
                    series: Series::First,
                    start: (10, 0),
                    end: (20, 0),
                },
            ],
            expected: &[APPLIED, APPLIED],
            stored: &[0, 1],
            version: "2",
        },
        Row {
            case: "a slice whose bounds carry nanosecond offsets",
            entries: &[Entry {
                source: Source::Session,
                series: Series::First,
                start: (100, 250_000_000),
                end: (160, 750_000_000),
            }],
            expected: &[APPLIED],
            stored: &[0],
            version: "2",
        },
        Row {
            case: "two series' slices covering the same time",
            entries: &[
                FIRST_EARLY,
                Entry {
                    source: Source::Session,
                    series: Series::Second,
                    start: (0, 0),
                    end: (10, 0),
                },
            ],
            expected: &[APPLIED, APPLIED],
            stored: &[0, 1],
            version: "2",
        },
        Row {
            case: "slices selected from two source session datasets",
            entries: &[
                FIRST_EARLY,
                Entry {
                    source: Source::OtherSession,
                    series: Series::Second,
                    start: (0, 0),
                    end: (10, 0),
                },
            ],
            expected: &[APPLIED, APPLIED],
            stored: &[0, 1],
            version: "2",
        },
        Row {
            case: "a slice overlapping one the same call stored",
            entries: &[
                FIRST_EARLY,
                Entry {
                    source: Source::Session,
                    series: Series::First,
                    start: (5, 0),
                    end: (15, 0),
                },
            ],
            expected: &[APPLIED, CONFLICTING_OPERATION],
            stored: &[0],
            version: "2",
        },
        Row {
            case: "a slice containing one the same call stored",
            entries: &[
                FIRST_EARLY,
                Entry {
                    source: Source::Session,
                    series: Series::First,
                    start: (0, 0),
                    end: (20, 0),
                },
            ],
            expected: &[APPLIED, CONFLICTING_OPERATION],
            stored: &[0],
            version: "2",
        },
        Row {
            case: "the same slice twice in one batch",
            entries: &[FIRST_EARLY, FIRST_EARLY],
            expected: &[APPLIED, CONFLICTING_OPERATION],
            stored: &[0],
            version: "2",
        },
        Row {
            case: "a span covering no time",
            entries: &[Entry {
                source: Source::Session,
                series: Series::First,
                start: (10, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            stored: &[],
            version: "1",
        },
        Row {
            case: "a span whose end precedes its start",
            entries: &[Entry {
                source: Source::Session,
                series: Series::First,
                start: (20, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            stored: &[],
            version: "1",
        },
        Row {
            case: "a span covering no time beside one covering time",
            entries: &[
                Entry {
                    source: Source::Session,
                    series: Series::First,
                    start: (10, 0),
                    end: (10, 0),
                },
                FIRST_EARLY,
            ],
            expected: &[VALIDATION_ERROR, APPLIED],
            stored: &[1],
            version: "2",
        },
        Row {
            case: "a time series the store does not hold",
            entries: &[Entry {
                source: Source::Session,
                series: Series::Absent,
                start: (0, 0),
                end: (10, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            stored: &[],
            version: "1",
        },
        Row {
            case: "a source dataset the store does not hold",
            entries: &[Entry {
                source: Source::Absent,
                series: Series::First,
                start: (0, 0),
                end: (10, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            stored: &[],
            version: "1",
        },
        Row {
            case: "a source dataset of another workspace",
            entries: &[Entry {
                source: Source::Foreign,
                series: Series::First,
                start: (0, 0),
                end: (10, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            stored: &[],
            version: "1",
        },
        Row {
            case: "a source dataset that is not a session dataset",
            entries: &[Entry {
                source: Source::Curated,
                series: Series::First,
                start: (0, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            stored: &[],
            version: "1",
        },
        Row {
            case: "the addressed dataset as its own source",
            entries: &[Entry {
                source: Source::Target,
                series: Series::First,
                start: (0, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            stored: &[],
            version: "1",
        },
        Row {
            case: "five entries, every outcome in one batch",
            entries: FIVE_ENTRIES,
            expected: &[
                APPLIED,
                RESOURCE_NOT_FOUND,
                CONFLICTING_OPERATION,
                RESOURCE_NOT_FOUND,
                APPLIED,
            ],
            stored: &[0, 4],
            version: "2",
        },
    ]
}

/// The datasets and data streams every suite below starts from.
#[derive(Debug)]
struct Fixture {
    engine: Arc<engine::InMemoryEngine>,
    /// The CURATED dataset of [`WORKSPACE`] the requests address.
    target: String,
    session: String,
    other_session: String,
    curated: String,
    foreign_session: String,
    first_series: String,
    second_series: String,
}

impl Fixture {
    /// The `datasetId` `source` names.
    fn dataset(&self, source: Source) -> &str {
        match source {
            Source::Session => &self.session,
            Source::OtherSession => &self.other_session,
            Source::Curated => &self.curated,
            Source::Target => &self.target,
            Source::Foreign => &self.foreign_session,
            Source::Absent => ABSENT_DATASET,
        }
    }

    /// The `timeSeriesId` `series` names.
    fn series(&self, series: Series) -> &str {
        match series {
            Series::First => &self.first_series,
            Series::Second => &self.second_series,
            Series::Absent => ABSENT_TIME_SERIES,
        }
    }
}

/// Two session datasets and two curated datasets in [`WORKSPACE`], a session
/// dataset in [`OTHER_WORKSPACE`], and the two data streams the entries name.
async fn fixture() -> (MultiServiceRouter, Fixture) {
    let (router, engine) = router_with_engine();
    let target = create_dataset(&router, "Glass curated", Some("CURATED"), Some(WORKSPACE)).await;
    let session = create_dataset(&router, "Glass session", Some("SESSION"), Some(WORKSPACE)).await;
    let other_session =
        create_dataset(&router, "Glass session 2", Some("SESSION"), Some(WORKSPACE)).await;
    let curated = create_dataset(&router, "Glass curated 2", Some("CURATED"), Some(WORKSPACE)).await;
    let foreign_session =
        create_dataset(&router, "Paint session", Some("SESSION"), Some(OTHER_WORKSPACE)).await;
    ingest(&router, FIRST_ALIAS, SAMPLES).await;
    ingest(&router, SECOND_ALIAS, SAMPLES).await;
    let fixture = Fixture {
        engine,
        target,
        session,
        other_session,
        curated,
        foreign_session,
        first_series: time_series_id(&router, FIRST_ALIAS).await,
        second_series: time_series_id(&router, SECOND_ALIAS).await,
    };
    (router, fixture)
}

/// `POST /datasets/{datasetId}/data-segments/associate` with `entries`.
async fn associate(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
    entries: Vec<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/datasets/{dataset_id}/data-segments/associate"),
        Some(json!({ "workspaceName": workspace, "associateDataSegmentEntries": entries })),
    )
    .await
}

/// One `AssociateDataSegmentEntry` on the wire.
fn wire_entry(fixture: &Fixture, entry: &Entry) -> Value {
    json!({
        "sourceDatasetId": fixture.dataset(entry.source),
        "timeSeriesId": fixture.series(entry.series),
        "startTimestamp": wire_instant(entry.start.0, entry.start.1),
        "endTimestamp": wire_instant(entry.end.0, entry.end.1),
    })
}

/// `GET /datasets/{datasetId}?workspaceName=…`.
async fn describe_dataset(router: &MultiServiceRouter, dataset_id: &str, workspace: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets/{dataset_id}?workspaceName={workspace}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe {dataset_id}: got {status} {body}");
    body
}

/// One segment as these suites compare them: the source dataset it came from, the
/// `TimeSeriesId` it covers, and its span in the table seconds and nanosecond
/// offsets the rows are written in.
type Held = (String, String, (i64, i32), (i64, i32));

/// The segments `dataset_id` holds, in the store's own order.
fn held_segments(engine: &Arc<engine::InMemoryEngine>, dataset_id: &str) -> Vec<Held> {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .dataset_data_segments
        .get(dataset_id)
        .map(|segments| {
            segments
                .iter()
                .map(|segment| {
                    (
                        segment.source_dataset_id.clone(),
                        segment.time_series_id.clone(),
                        (
                            relative(segment.start_timestamp.time_in_seconds),
                            segment.start_timestamp.offset_in_nanos.unwrap_or(0),
                        ),
                        (
                            relative(segment.end_timestamp.time_in_seconds),
                            segment.end_timestamp.offset_in_nanos.unwrap_or(0),
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The segments `dataset_id` reports through `ListDatasetDataSegments` — the
/// implemented reader of what an association stores — in the order it reports
/// them.
async fn listed_segments(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
) -> Vec<Held> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets/{dataset_id}/data-segments?workspaceName={workspace}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "list {dataset_id}: got {status} {body}");
    body["dataSegments"]
        .as_array()
        .unwrap_or_else(|| panic!("no dataSegments in {body}"))
        .iter()
        .map(|segment| {
            (
                segment["sourceDatasetId"].as_str().unwrap_or("").to_string(),
                segment["timeSeriesId"].as_str().unwrap_or("").to_string(),
                (
                    segment["startTimestamp"]["timeInSeconds"]
                        .as_i64()
                        .map(relative)
                        .unwrap_or_else(|| panic!("a segment with no startTimestamp in {body}")),
                    segment["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
                (
                    segment["endTimestamp"]["timeInSeconds"]
                        .as_i64()
                        .map(relative)
                        .unwrap_or_else(|| panic!("a segment with no endTimestamp in {body}")),
                    segment["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
            )
        })
        .collect()
}

/// The segments the entries `stored` names must leave behind, sorted the way the
/// store keeps them.
fn expected_segments(fixture: &Fixture, entries: &[Entry], stored: &[usize]) -> Vec<Held> {
    let mut expected: Vec<Held> = stored
        .iter()
        .map(|at| {
            let entry = entries[*at];
            (
                fixture.dataset(entry.source).to_string(),
                fixture.series(entry.series).to_string(),
                entry.start,
                entry.end,
            )
        })
        .collect();
    // The store keys a segment by its series and span, which is the order a read
    // of the log reports; the source is carried along with it.
    expected.sort_by(|left, right| (&left.1, left.2, left.3).cmp(&(&right.1, right.2, right.3)));
    expected
}

/// The `errorMessage` an entry refused with `code` must carry.
fn expected_message(fixture: &Fixture, entry: &Entry, code: &str) -> Option<String> {
    match code {
        RESOURCE_NOT_FOUND if entry.series == Series::Absent => Some(not_found_message(
            RESOURCE_TIME_SERIES,
            fixture.series(entry.series),
        )),
        RESOURCE_NOT_FOUND => Some(not_found_message(
            RESOURCE_DATASET,
            fixture.dataset(entry.source),
        )),
        VALIDATION_ERROR if entry.start >= entry.end => Some(empty_span_message()),
        // The two remaining validation failures and the overlap are checked by
        // what they must NAME rather than by a whole message, since each states a
        // resource the row resolved at run time.
        _ => None,
    }
}

#[tokio::test]
async fn an_association_entry_stores_the_slice_its_own_span_selects() {
    for row in table() {
        let Row {
            case,
            entries,
            expected,
            stored,
            version,
        } = row;
        assert_eq!(
            entries.len(),
            expected.len(),
            "case {case:?}: the table must say what every entry answers"
        );

        let (router, fixture) = fixture().await;
        let wire: Vec<Value> = entries
            .iter()
            .map(|entry| wire_entry(&fixture, entry))
            .collect();
        let (status, body) = associate(&router, &fixture.target, WORKSPACE, wire).await;

        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["datasetId"].as_str(),
            Some(fixture.target.as_str()),
            "case {case:?}: the answer must name the dataset the request named, got {body}"
        );
        assert_eq!(
            body["datasetVersion"].as_str(),
            Some(version),
            "case {case:?}: expected datasetVersion {version}, got {body}"
        );

        // `failedAssociations` carries one element per entry the call could not
        // apply, in the order those entries were sent, each echoing the segment it
        // addressed.
        let failures: Vec<(&Entry, &&str)> = entries
            .iter()
            .zip(expected.iter())
            .filter(|(_, code)| **code != APPLIED)
            .collect();
        let reported = body["failedAssociations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no failedAssociations list in {body}"));
        assert_eq!(
            reported.len(),
            failures.len(),
            "case {case:?}: expected {} failed entries, got {body}",
            failures.len()
        );
        for ((entry, code), element) in failures.iter().zip(reported.iter()) {
            assert_eq!(
                element["sourceDatasetId"].as_str(),
                Some(fixture.dataset(entry.source)),
                "case {case:?}: a failure must echo its own sourceDatasetId, got {element}"
            );
            assert_eq!(
                element["timeSeriesId"].as_str(),
                Some(fixture.series(entry.series)),
                "case {case:?}: a failure must echo its own timeSeriesId, got {element}"
            );
            assert_eq!(
                (
                    element["startTimestamp"]["timeInSeconds"]
                        .as_i64()
                        .map(relative),
                    element["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
                (Some(entry.start.0), entry.start.1),
                "case {case:?}: a failure must echo its own startTimestamp, got {element}"
            );
            assert_eq!(
                (
                    element["endTimestamp"]["timeInSeconds"]
                        .as_i64()
                        .map(relative),
                    element["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
                (Some(entry.end.0), entry.end.1),
                "case {case:?}: a failure must echo its own endTimestamp, got {element}"
            );
            assert_eq!(
                element["errorCode"].as_str(),
                Some(**code),
                "case {case:?}: expected errorCode {code}, got {element}"
            );
            let message = element["errorMessage"].as_str().unwrap_or("");
            match expected_message(&fixture, entry, code) {
                Some(wording) => assert_eq!(
                    message, wording,
                    "case {case:?}: unexpected errorMessage in {element}"
                ),
                None if **code == CONFLICTING_OPERATION => assert!(
                    message.contains(fixture.series(entry.series)) && message.contains("overlap"),
                    "case {case:?}: an overlap must name the series it overlaps on, got {element}"
                ),
                None if entry.source == Source::Target => assert!(
                    message.contains("sourceDatasetId"),
                    "case {case:?}: a refusal must name the member it refused, got {element}"
                ),
                None => assert!(
                    message.contains("SESSION") && message.contains(fixture.dataset(entry.source)),
                    "case {case:?}: a non-session source must be named, got {element}"
                ),
            }
        }

        // The slices the call applied are what the curated dataset now holds, each
        // naming the source dataset it was selected from; an entry the call refused
        // left nothing behind.
        assert_eq!(
            held_segments(&fixture.engine, &fixture.target),
            expected_segments(&fixture, entries, stored),
            "case {case:?}: unexpected segments on the curated dataset"
        );

        // And they read back over the wire, through the operation that lists what a
        // dataset holds.
        assert_eq!(
            listed_segments(&router, &fixture.target, WORKSPACE).await,
            expected_segments(&fixture, entries, stored),
            "case {case:?}: the stored slices must read back through ListDatasetDataSegments"
        );

        // An association stores the slice on the dataset it is associated WITH, so
        // no source dataset gains a segment of its own.
        for source in [Source::Session, Source::OtherSession, Source::Curated] {
            assert_eq!(
                held_segments(&fixture.engine, fixture.dataset(source)),
                Vec::new(),
                "case {case:?}: {source:?} must hold no segment of its own"
            );
        }

        // The version the call reported is the version the dataset reports.
        let described = describe_dataset(&router, &fixture.target, WORKSPACE).await;
        assert_eq!(
            described["datasetVersion"].as_str(),
            Some(version),
            "case {case:?}: the dataset must report the version the call answered, got {described}"
        );
    }
}

#[tokio::test]
async fn an_associated_slice_blocks_an_overlap_in_every_dataset_and_the_dataset_settles() {
    let (router, fixture) = fixture().await;
    let first = vec![wire_entry(&fixture, &FIRST_EARLY)];

    let (status, body) = associate(&router, &fixture.target, WORKSPACE, first.clone()).await;
    assert_eq!(status, 200, "the first association got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "the first association must apply its entry, got {body}"
    );

    // A write the dataset accepted leaves it UPDATING until the transition that
    // write owes falls due, exactly as an update does.
    let described = describe_dataset(&router, &fixture.target, WORKSPACE).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("UPDATING"),
        "straight after an association the dataset must still be settling, got {described}"
    );
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;
    let described = describe_dataset(&router, &fixture.target, WORKSPACE).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("ACTIVE"),
        "once the owed transition falls due the dataset must be ACTIVE, got {described}"
    );

    // "Data segments that belong to the same time series can't overlap in time,
    // regardless of which dataset they belong to": the same span offered to ANOTHER
    // curated dataset is refused on that rule, and that dataset stores nothing.
    let (status, other) = associate(&router, &fixture.curated, WORKSPACE, first.clone()).await;
    assert_eq!(status, 200, "the second dataset's call got {status} {other}");
    assert_eq!(
        other["failedAssociations"][0]["errorCode"].as_str(),
        Some(CONFLICTING_OPERATION),
        "a span another dataset already holds must be refused, got {other}"
    );
    assert!(
        other["failedAssociations"][0]["errorMessage"]
            .as_str()
            .unwrap_or("")
            .contains(&fixture.target),
        "the refusal must name the dataset that holds the overlapping segment, got {other}"
    );
    assert_eq!(
        other["datasetVersion"].as_str(),
        Some("1"),
        "a call that stored nothing must leave its dataset's version alone, got {other}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.curated),
        Vec::new(),
        "a refused entry must leave the second dataset holding nothing"
    );

    // The same entry again addresses a slice this dataset already holds, so it
    // fails on its own and the dataset stays where the first call left it.
    let (status, again) = associate(&router, &fixture.target, WORKSPACE, first).await;
    assert_eq!(status, 200, "the repeat got {status} {again}");
    assert_eq!(
        again["failedAssociations"][0]["errorCode"].as_str(),
        Some(CONFLICTING_OPERATION),
        "a repeat of an applied entry must report the overlap, got {again}"
    );
    assert_eq!(
        again["datasetVersion"].as_str(),
        Some("2"),
        "a call that stored nothing must not take the dataset to a new version, got {again}"
    );
    let described = describe_dataset(&router, &fixture.target, WORKSPACE).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("ACTIVE"),
        "a call that wrote nothing must not restart the dataset's settling, got {described}"
    );

    // Two half-open spans that merely MEET do not overlap, so the slice starting
    // where the held one ends is accepted — in the other dataset too.
    let adjacent = vec![wire_entry(
        &fixture,
        &Entry {
            source: Source::Session,
            series: Series::First,
            start: (10, 0),
            end: (20, 0),
        },
    )];
    let (status, body) = associate(&router, &fixture.curated, WORKSPACE, adjacent).await;
    assert_eq!(status, 200, "the adjacent slice got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "a slice starting where the held one ends must be accepted, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.curated),
        vec![(
            fixture.session.clone(),
            fixture.first_series.clone(),
            (10, 0),
            (20, 0)
        )],
        "the accepted slice must be what the second dataset holds"
    );
}

#[tokio::test]
async fn a_source_dataset_s_own_data_is_what_an_association_selects_from() {
    // A segment whose source is the dataset that holds it is the source's OWN
    // ingested data, which is what a curated dataset selects a slice of — so it is
    // not one of the segments the overlap rule compares against.
    let (router, fixture) = fixture().await;
    {
        let state = service_state(&*fixture.engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        assert!(
            store.put_dataset_data_segment(
                &fixture.session,
                DataSegmentRecord {
                    source_dataset_id: fixture.session.clone(),
                    time_series_id: fixture.first_series.clone(),
                    start_timestamp: instant(0, 0),
                    end_timestamp: instant(20, 0),
                },
            ),
            "seeding the session dataset's own segment must be accepted"
        );
    }

    let (status, body) = associate(
        &router,
        &fixture.target,
        WORKSPACE,
        vec![wire_entry(&fixture, &FIRST_EARLY)],
    )
    .await;
    assert_eq!(status, 200, "the association got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "a slice of the source's own data must be accepted, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.target),
        vec![(
            fixture.session.clone(),
            fixture.first_series.clone(),
            (0, 0),
            (10, 0)
        )],
        "the curated dataset must hold the slice it selected"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.session),
        vec![(
            fixture.session.clone(),
            fixture.first_series.clone(),
            (0, 0),
            (20, 0)
        )],
        "the source dataset must keep the data the slice was selected from"
    );
    assert_eq!(
        listed_segments(&router, &fixture.target, WORKSPACE).await,
        vec![(
            fixture.session.clone(),
            fixture.first_series.clone(),
            (0, 0),
            (10, 0)
        )],
        "the selected slice must read back through ListDatasetDataSegments, naming its source"
    );
}

#[tokio::test]
async fn a_request_the_dataset_does_not_answer_is_refused_and_writes_nothing() {
    let (router, fixture) = fixture().await;
    let external = create_dataset(&router, "Glass external", Some("EXTERNAL"), None).await;

    // (case, the datasetId the request addresses, the workspaceName it carries,
    // the status and error code expected) — an id that names nothing, the stored id
    // scoped to a workspace that does not hold it, a dataset of no workspace, and
    // the dataset type whose segments are its own data rather than a selection.
    let rows: Vec<(&str, String, &str, u16, &str)> = vec![
        (
            "an id that names no dataset",
            ABSENT_DATASET.to_string(),
            WORKSPACE,
            404,
            "ResourceNotFoundException",
        ),
        (
            "the dataset scoped to another workspace",
            fixture.target.clone(),
            OTHER_WORKSPACE,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a dataset that belongs to no workspace",
            external.clone(),
            WORKSPACE,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a session dataset",
            fixture.session.clone(),
            WORKSPACE,
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, target, workspace, expected_status, expected_code) in rows {
        let wire = vec![wire_entry(&fixture, &FIRST_EARLY)];
        let (status, body) = associate(&router, &target, workspace, wire).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code in {body}"
        );

        // A refused request stores nothing, on the dataset it addressed or on any
        // other.
        for dataset in [&fixture.target, &fixture.session, &external, &target] {
            assert_eq!(
                held_segments(&fixture.engine, dataset),
                Vec::new(),
                "case {case:?}: a refused request must store no segment"
            );
        }
    }

    // The dataset's OWN workspace resolves, so the scoping above refuses only a
    // workspace that does not hold it.
    let wire = vec![wire_entry(&fixture, &FIRST_EARLY)];
    let (status, body) = associate(&router, &fixture.target, WORKSPACE, wire).await;
    assert_eq!(status, 200, "the dataset's own workspace got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "the dataset's own workspace must apply the entry, got {body}"
    );
}

#[tokio::test]
async fn a_dataset_the_store_holds_no_type_for_answers_the_association() {
    // `CreateDataset` keeps `datasetType` absent rather than guessing one, so a
    // dataset that states no type states nothing about which side of the
    // curated-only rule it falls on and the association is applied — on the target
    // and on the source alike.
    let (router, fixture) = fixture().await;
    let untyped = create_dataset(&router, "Glass untyped", None, Some(WORKSPACE)).await;

    let entry = Entry {
        source: Source::Session,
        series: Series::First,
        start: (0, 0),
        end: (10, 0),
    };
    let (status, body) = associate(
        &router,
        &untyped,
        WORKSPACE,
        vec![wire_entry(&fixture, &entry)],
    )
    .await;
    assert_eq!(status, 200, "an untyped target got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "an untyped target must apply the entry, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &untyped),
        vec![(
            fixture.session.clone(),
            fixture.first_series.clone(),
            (0, 0),
            (10, 0)
        )],
        "the untyped dataset must hold the slice it was given"
    );

    // The same for a source the store holds no type for: an entry naming it is
    // applied rather than refused as a non-session source.
    let untyped_source = json!({
        "sourceDatasetId": untyped,
        "timeSeriesId": fixture.second_series,
        "startTimestamp": wire_instant(0, 0),
        "endTimestamp": wire_instant(10, 0),
    });
    let (status, body) = associate(&router, &fixture.target, WORKSPACE, vec![untyped_source]).await;
    assert_eq!(status, 200, "an untyped source got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "an untyped source must apply the entry, got {body}"
    );
}

#[tokio::test]
async fn past_the_per_dataset_bound_an_entry_reports_the_quota() {
    let (router, fixture) = fixture().await;

    // Fill the curated dataset to the bound with associations the store accepts,
    // each covering its own span of the first series so none of them overlaps.
    {
        let state = service_state(&*fixture.engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        for span in 0..MAX_DATA_SEGMENTS_PER_DATASET as i64 {
            assert!(
                store.put_dataset_data_segment(
                    &fixture.target,
                    DataSegmentRecord {
                        source_dataset_id: fixture.session.clone(),
                        time_series_id: fixture.first_series.clone(),
                        start_timestamp: instant(span * 10, 0),
                        end_timestamp: instant(span * 10 + 10, 0),
                    },
                ),
                "span {span} must be accepted below the bound"
            );
        }
    }

    // A span past every held one: it overlaps nothing, so what refuses it is the
    // dataset standing at the segments one dataset holds.
    let over = MAX_DATA_SEGMENTS_PER_DATASET as i64;
    let entry = Entry {
        source: Source::Session,
        series: Series::First,
        start: (over * 10, 0),
        end: (over * 10 + 10, 0),
    };
    let (status, body) = associate(
        &router,
        &fixture.target,
        WORKSPACE,
        vec![wire_entry(&fixture, &entry)],
    )
    .await;
    assert_eq!(status, 200, "the entry past the bound got {status} {body}");
    assert_eq!(
        body["failedAssociations"][0]["errorCode"].as_str(),
        Some(LIMIT_EXCEEDED),
        "past the bound the entry must report the quota, got {body}"
    );
    assert_eq!(
        body["datasetVersion"].as_str(),
        Some("1"),
        "a call that stored nothing must leave the version alone, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.target).len(),
        MAX_DATA_SEGMENTS_PER_DATASET,
        "a refused entry must not grow the dataset"
    );

    // A batch naming more entries than one dataset can hold could not store them
    // all whatever they said, so the request itself is the quota refusal.
    let oversized: Vec<Value> = (0..=MAX_DATA_SEGMENTS_PER_DATASET as i64)
        .map(|span| {
            wire_entry(
                &fixture,
                &Entry {
                    source: Source::Session,
                    series: Series::Second,
                    start: (span * 10, 0),
                    end: (span * 10 + 10, 0),
                },
            )
        })
        .collect();
    let (status, body) = associate(&router, &fixture.target, WORKSPACE, oversized).await;
    assert_eq!(status, 410, "an oversized batch got {status} {body}");
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "an oversized batch must be the modelled quota refusal, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.target).len(),
        MAX_DATA_SEGMENTS_PER_DATASET,
        "a refused request must not grow the dataset"
    );
}
