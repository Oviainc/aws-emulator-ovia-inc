// SPDX-License-Identifier: CC0-1.0

//! `BatchDisassociateDataSegmentsFromDataset` end to end: the data segments a
//! curated dataset curates are disassociated over HTTP, through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a disassociation entry addresses ONE data segment of
//! the curated dataset the request names — by the source dataset the segment's data
//! came from, by its time series and by its own exact half-open span — so an entry
//! that addresses a held segment removes that segment and nothing else, leaving the
//! underlying time series samples and the source dataset's own segments where they
//! are, while an entry that addresses no held segment, or whose span covers no
//! time, fails on its own with its own `DataSegmentErrorCode` and removes nothing;
//! the dataset reaches its next version only when the call accepted at least one
//! disassociation.
//!
//! The table below spans the entries a request can carry: a segment's own span, a
//! span differing only in its nanosecond offset, a span with that offset, a span
//! covering two held segments, another series' segment, the same segment named
//! under another source dataset, a time series the dataset holds no segment of, a
//! repeat of an entry inside one batch, spans covering no time, and batches of one,
//! two and four entries. The whole-request refusals (an unknown dataset, a dataset
//! of another workspace, a dataset that is not a curated dataset), what a
//! disassociation leaves behind, and the version the dataset settles at are their
//! own suites.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`) is the one the
//! asset suite defines, and the dataset helpers (`create_dataset`, `ingest`,
//! `history_seconds`, `time_series_id`, `instant`, `wire_instant`, `relative`) are
//! the ones the deletion suite defines; both are reused rather than copied.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    empty_span_message, not_found_message, service_state, DataSegmentRecord, ACTIVATION_DELAY,
    MAX_DATA_SEGMENTS_PER_DATASET, RESOURCE_DATA_SEGMENT, RESOURCE_DATASET, RESOURCE_WORKSPACE,
};

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::dataset_data_segments::{
    create_dataset, history_seconds, ingest, instant, relative, time_series_id, wire_instant,
};

/// The workspace the datasets of these suites live in.
const WORKSPACE: &str = "kiln-line";

/// A second workspace, so a row can prove the addressed dataset is resolved inside
/// the workspace the request names.
const OTHER_WORKSPACE: &str = "dryer-line";

/// The alias of the first seeded data stream — the series most rows disassociate
/// slices of.
const FIRST_ALIAS: &str = "/plant/kiln/1/temperature";

/// The alias of the second seeded data stream, so a row can prove an entry reaches
/// ONE series' segment and not another's.
const SECOND_ALIAS: &str = "/plant/kiln/2/temperature";

/// A `TimeSeriesId` no stored stream carries, and no held segment covers: an
/// `ID`-shaped value inside the modelled `TimeSeriesId` length (36..73).
const ABSENT_TIME_SERIES: &str = "3f6a91d2-58c4-4b07-9e13-2c7d8a0b64e5";

/// A `datasetId` no stored dataset carries.
const ABSENT_DATASET: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The sample seconds, relative to the suite's anchor, the first stream is
/// ingested with — the data the held segments cover slices of.
const FIRST_SAMPLES: &[i64] = &[0, 5, 10, 15, 100, 150];

/// The same for the second stream.
const SECOND_SAMPLES: &[i64] = &[0, 5];

/// The `DataSegmentErrorCode` an entry addressing no segment the dataset holds
/// reports. Source: `smithy_query(query="shape", shape="DataSegmentErrorCode")` —
/// INTERNAL_FAILURE | VALIDATION_ERROR | RESOURCE_NOT_FOUND | LIMIT_EXCEEDED |
/// CONFLICTING_OPERATION.
const RESOURCE_NOT_FOUND: &str = "RESOURCE_NOT_FOUND";

/// The `DataSegmentErrorCode` an entry whose span covers no time reports; see
/// [`RESOURCE_NOT_FOUND`] for the value list.
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// The code a row expects for an entry the call APPLIED, which is reported in no
/// `failedDisassociations` element at all.
const APPLIED: &str = "";

/// Which seeded time series an entry (or a held segment) names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Series {
    /// The stream [`FIRST_ALIAS`] names.
    First,
    /// The stream [`SECOND_ALIAS`] names.
    Second,
    /// [`ABSENT_TIME_SERIES`]: a `TimeSeriesId` no held segment covers.
    Absent,
}

/// Which dataset an entry names as the source the segment's data came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The session dataset the held segments of the first series were selected
    /// from.
    Session,
    /// A second session dataset of the same workspace, the source of the held
    /// segment of the second series.
    OtherSession,
    /// [`ABSENT_DATASET`]: a source no held segment names.
    Absent,
}

/// One data segment, as the fixture associates it and as an entry addresses it:
/// the source dataset it came from, the time series it covers and the half-open
/// span it covers of it, in seconds relative to the suite's anchor plus a
/// nanosecond offset.
#[derive(Debug, Clone, Copy)]
struct Segment {
    source: Source,
    series: Series,
    start: (i64, i32),
    end: (i64, i32),
}

/// The segments the curated dataset holds before every row runs: three spans of
/// the first series (two adjacent, the third with a nanosecond offset on its end)
/// selected from one session dataset, and one span of the second series selected
/// from another.
const HELD: &[Segment] = &[FIRST_EARLY, FIRST_MIDDLE, FIRST_OFFSET, SECOND_EARLY];

/// The first series' earliest held segment; see [`HELD`].
const FIRST_EARLY: Segment = Segment {
    source: Source::Session,
    series: Series::First,
    start: (0, 0),
    end: (10, 0),
};

/// The held segment immediately after [`FIRST_EARLY`], so a row can prove an
/// entry stops at its own span.
const FIRST_MIDDLE: Segment = Segment {
    source: Source::Session,
    series: Series::First,
    start: (10, 0),
    end: (20, 0),
};

/// The first series' last held segment, whose end carries a nanosecond offset.
const FIRST_OFFSET: Segment = Segment {
    source: Source::Session,
    series: Series::First,
    start: (100, 0),
    end: (160, 250_000_000),
};

/// The second series' held segment, selected from the other session dataset.
const SECOND_EARLY: Segment = Segment {
    source: Source::OtherSession,
    series: Series::Second,
    start: (0, 0),
    end: (10, 0),
};

/// One row of the disassociation table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The entries the request carries, in order.
    entries: &'static [Segment],
    /// What each entry answers, in the same order: [`APPLIED`], or the
    /// `DataSegmentErrorCode` it is reported in `failedDisassociations` with.
    expected: &'static [&'static str],
    /// The indices into [`HELD`] the curated dataset must still hold afterwards.
    remaining: &'static [usize],
    /// The `datasetVersion` the call reports: the fixture's association took the
    /// dataset to 2, and a call that removed at least one segment takes it to 3.
    version: &'static str,
}

/// Four entries, mixing every outcome one batch can carry.
const FOUR_ENTRIES: &[Segment] = &[
    FIRST_EARLY,
    Segment {
        source: Source::Session,
        series: Series::First,
        start: (10, 0),
        end: (10, 0),
    },
    FIRST_MIDDLE,
    Segment {
        source: Source::Absent,
        series: Series::Second,
        start: (0, 0),
        end: (10, 0),
    },
];

fn table() -> Vec<Row> {
    vec![
        Row {
            case: "a held segment's own span",
            entries: &[FIRST_EARLY],
            expected: &[APPLIED],
            remaining: &[1, 2, 3],
            version: "3",
        },
        Row {
            case: "a held segment whose end carries a nanosecond offset",
            entries: &[FIRST_OFFSET],
            expected: &[APPLIED],
            remaining: &[0, 1, 3],
            version: "3",
        },
        Row {
            case: "a span differing from a held one only in its nanosecond offset",
            entries: &[Segment {
                source: Source::Session,
                series: Series::First,
                start: (100, 0),
                end: (160, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            remaining: &[0, 1, 2, 3],
            version: "2",
        },
        Row {
            case: "another series' held segment",
            entries: &[SECOND_EARLY],
            expected: &[APPLIED],
            remaining: &[0, 1, 2],
            version: "3",
        },
        Row {
            case: "a held segment named under another source dataset",
            entries: &[Segment {
                source: Source::OtherSession,
                series: Series::First,
                start: (0, 0),
                end: (10, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            remaining: &[0, 1, 2, 3],
            version: "2",
        },
        Row {
            case: "a span covering two held segments",
            entries: &[Segment {
                source: Source::Session,
                series: Series::First,
                start: (0, 0),
                end: (20, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            remaining: &[0, 1, 2, 3],
            version: "2",
        },
        Row {
            case: "a time series the dataset holds no segment of",
            entries: &[Segment {
                source: Source::Session,
                series: Series::Absent,
                start: (0, 0),
                end: (10, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            remaining: &[0, 1, 2, 3],
            version: "2",
        },
        Row {
            case: "two held segments in one batch",
            entries: &[FIRST_EARLY, SECOND_EARLY],
            expected: &[APPLIED, APPLIED],
            remaining: &[1, 2],
            version: "3",
        },
        Row {
            case: "two adjacent held segments of one series",
            entries: &[FIRST_EARLY, FIRST_MIDDLE],
            expected: &[APPLIED, APPLIED],
            remaining: &[2, 3],
            version: "3",
        },
        Row {
            case: "the same entry twice in one batch",
            entries: &[FIRST_EARLY, FIRST_EARLY],
            expected: &[APPLIED, RESOURCE_NOT_FOUND],
            remaining: &[1, 2, 3],
            version: "3",
        },
        Row {
            case: "a span covering no time",
            entries: &[Segment {
                source: Source::Session,
                series: Series::First,
                start: (10, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            remaining: &[0, 1, 2, 3],
            version: "2",
        },
        Row {
            case: "a span whose end precedes its start",
            entries: &[Segment {
                source: Source::Session,
                series: Series::First,
                start: (20, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            remaining: &[0, 1, 2, 3],
            version: "2",
        },
        Row {
            case: "a span covering no time beside a held segment",
            entries: &[
                Segment {
                    source: Source::Session,
                    series: Series::First,
                    start: (10, 0),
                    end: (10, 0),
                },
                FIRST_EARLY,
            ],
            expected: &[VALIDATION_ERROR, APPLIED],
            remaining: &[1, 2, 3],
            version: "3",
        },
        Row {
            case: "four entries, every outcome in one batch",
            entries: FOUR_ENTRIES,
            expected: &[APPLIED, VALIDATION_ERROR, APPLIED, RESOURCE_NOT_FOUND],
            remaining: &[2, 3],
            version: "3",
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
    first_series: String,
    second_series: String,
}

impl Fixture {
    /// The `datasetId` `source` names.
    fn dataset(&self, source: Source) -> &str {
        match source {
            Source::Session => &self.session,
            Source::OtherSession => &self.other_session,
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

/// `POST /datasets/{datasetId}/data-segments/disassociate` with `entries`.
async fn disassociate(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
    entries: Vec<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/datasets/{dataset_id}/data-segments/disassociate"),
        Some(json!({ "workspaceName": workspace, "disassociateDataSegmentEntries": entries })),
    )
    .await
}

/// `POST /datasets/{datasetId}/data-segments/associate` with `entries`: the
/// producer side, which is how the fixture's held segments get there.
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

/// One entry on the wire — the same four members an association and a
/// disassociation address a segment by.
fn wire_entry(fixture: &Fixture, segment: &Segment) -> Value {
    json!({
        "sourceDatasetId": fixture.dataset(segment.source),
        "timeSeriesId": fixture.series(segment.series),
        "startTimestamp": wire_instant(segment.start.0, segment.start.1),
        "endTimestamp": wire_instant(segment.end.0, segment.end.1),
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
/// implemented reader of what a dataset holds — in the order it reports them.
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

/// The held segments `remaining` names, sorted the way the store keeps them.
fn expected_segments(fixture: &Fixture, remaining: &[usize]) -> Vec<Held> {
    let mut expected: Vec<Held> = remaining
        .iter()
        .map(|at| {
            let segment = HELD[*at];
            (
                fixture.dataset(segment.source).to_string(),
                fixture.series(segment.series).to_string(),
                segment.start,
                segment.end,
            )
        })
        .collect();
    // The store keys a segment by its series and span, which is the order a read
    // of the log reports; the source is carried along with it.
    expected.sort_by(|left, right| (&left.1, left.2, left.3).cmp(&(&right.1, right.2, right.3)));
    expected
}

/// Two session datasets and the curated dataset of [`WORKSPACE`] holding every
/// segment of [`HELD`], associated over HTTP the way a caller puts them there.
async fn fixture() -> (MultiServiceRouter, Fixture) {
    let (router, engine) = router_with_engine();
    let target = create_dataset(&router, "Kiln curated", Some("CURATED"), Some(WORKSPACE)).await;
    let session = create_dataset(&router, "Kiln session", Some("SESSION"), Some(WORKSPACE)).await;
    let other_session =
        create_dataset(&router, "Kiln session 2", Some("SESSION"), Some(WORKSPACE)).await;
    ingest(&router, FIRST_ALIAS, FIRST_SAMPLES).await;
    ingest(&router, SECOND_ALIAS, SECOND_SAMPLES).await;
    let fixture = Fixture {
        engine,
        target,
        session,
        other_session,
        first_series: time_series_id(&router, FIRST_ALIAS).await,
        second_series: time_series_id(&router, SECOND_ALIAS).await,
    };

    let wire: Vec<Value> = HELD
        .iter()
        .map(|segment| wire_entry(&fixture, segment))
        .collect();
    let (status, body) = associate(&router, &fixture.target, WORKSPACE, wire).await;
    assert_eq!(status, 200, "seeding the segments got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "seeding must associate every segment, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.target).len(),
        HELD.len(),
        "the fixture must hold every seeded segment"
    );
    (router, fixture)
}

#[tokio::test]
async fn a_disassociation_entry_removes_the_segment_its_own_members_address() {
    for row in table() {
        let Row {
            case,
            entries,
            expected,
            remaining,
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
            .map(|segment| wire_entry(&fixture, segment))
            .collect();
        let (status, body) = disassociate(&router, &fixture.target, WORKSPACE, wire).await;

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

        // `failedDisassociations` carries one element per entry the call could not
        // apply, in the order those entries were sent, each echoing the segment it
        // addressed.
        let failures: Vec<(&Segment, &&str)> = entries
            .iter()
            .zip(expected.iter())
            .filter(|(_, code)| **code != APPLIED)
            .collect();
        let reported = body["failedDisassociations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no failedDisassociations list in {body}"));
        assert_eq!(
            reported.len(),
            failures.len(),
            "case {case:?}: expected {} failed entries, got {body}",
            failures.len()
        );
        for ((segment, code), element) in failures.iter().zip(reported.iter()) {
            let series = fixture.series(segment.series);
            assert_eq!(
                element["sourceDatasetId"].as_str(),
                Some(fixture.dataset(segment.source)),
                "case {case:?}: a failure must echo its own sourceDatasetId, got {element}"
            );
            assert_eq!(
                element["timeSeriesId"].as_str(),
                Some(series),
                "case {case:?}: a failure must echo its own timeSeriesId, got {element}"
            );
            assert_eq!(
                (
                    element["startTimestamp"]["timeInSeconds"]
                        .as_i64()
                        .map(relative),
                    element["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
                (Some(segment.start.0), segment.start.1),
                "case {case:?}: a failure must echo its own startTimestamp, got {element}"
            );
            assert_eq!(
                (
                    element["endTimestamp"]["timeInSeconds"]
                        .as_i64()
                        .map(relative),
                    element["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
                (Some(segment.end.0), segment.end.1),
                "case {case:?}: a failure must echo its own endTimestamp, got {element}"
            );
            assert_eq!(
                element["errorCode"].as_str(),
                Some(**code),
                "case {case:?}: expected errorCode {code}, got {element}"
            );
            let message = element["errorMessage"].as_str().unwrap_or("");
            if **code == RESOURCE_NOT_FOUND {
                assert_eq!(
                    message,
                    not_found_message(RESOURCE_DATA_SEGMENT, series),
                    "case {case:?}: an absent segment must carry the service's not-found \
                     wording, got {element}"
                );
            } else {
                assert_eq!(
                    message,
                    empty_span_message(),
                    "case {case:?}: a span covering no time must carry the service's span \
                     wording, got {element}"
                );
            }
        }

        // The segments the call applied an entry to are gone, and every other one
        // the dataset held is still there — in the store and over the wire alike.
        let expected_held = expected_segments(&fixture, remaining);
        assert_eq!(
            held_segments(&fixture.engine, &fixture.target),
            expected_held,
            "case {case:?}: unexpected segments on the curated dataset"
        );
        assert_eq!(
            listed_segments(&router, &fixture.target, WORKSPACE).await,
            expected_held,
            "case {case:?}: what the dataset still holds must read back through \
             ListDatasetDataSegments"
        );

        // "Disassociating a data segment doesn't delete the underlying data": every
        // sample both streams were ingested with is still there, whatever the call
        // removed.
        assert_eq!(
            history_seconds(&router, FIRST_ALIAS).await,
            FIRST_SAMPLES.to_vec(),
            "case {case:?}: the first series must keep every sample"
        );
        assert_eq!(
            history_seconds(&router, SECOND_ALIAS).await,
            SECOND_SAMPLES.to_vec(),
            "case {case:?}: the second series must keep every sample"
        );

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
async fn a_disassociation_leaves_the_source_dataset_and_lets_the_span_be_associated_again() {
    let (router, fixture) = fixture().await;

    // The source session dataset's OWN data — a segment whose source is the
    // dataset that holds it — is what the curated dataset selected its slices
    // from, and a disassociation must not reach it.
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

    let entry = vec![wire_entry(&fixture, &FIRST_EARLY)];
    let (status, body) = disassociate(&router, &fixture.target, WORKSPACE, entry.clone()).await;
    assert_eq!(status, 200, "the disassociation got {status} {body}");
    assert_eq!(
        body["failedDisassociations"].as_array().map(Vec::len),
        Some(0),
        "the held segment must be disassociated, got {body}"
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

    // A write the dataset accepted leaves it UPDATING until the transition that
    // write owes falls due, exactly as an update does.
    let described = describe_dataset(&router, &fixture.target, WORKSPACE).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("UPDATING"),
        "straight after a disassociation the dataset must still be settling, got {described}"
    );
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;
    let described = describe_dataset(&router, &fixture.target, WORKSPACE).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("ACTIVE"),
        "once the owed transition falls due the dataset must be ACTIVE, got {described}"
    );

    // The same entry again addresses a segment the dataset no longer holds, so it
    // fails on its own and the dataset stays where the first call left it.
    let (status, again) = disassociate(&router, &fixture.target, WORKSPACE, entry.clone()).await;
    assert_eq!(status, 200, "the repeat got {status} {again}");
    assert_eq!(
        again["failedDisassociations"][0]["errorCode"].as_str(),
        Some(RESOURCE_NOT_FOUND),
        "a repeat of an applied entry must report the absence, got {again}"
    );
    assert_eq!(
        again["datasetVersion"].as_str(),
        Some("3"),
        "a call that removed nothing must not take the dataset to a new version, got {again}"
    );
    let described = describe_dataset(&router, &fixture.target, WORKSPACE).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("ACTIVE"),
        "a call that wrote nothing must not restart the dataset's settling, got {described}"
    );

    // The span the disassociation freed carries no association of that series any
    // more, so it can be associated again — which is what proves the removal
    // reached the store the overlap rule is asked of.
    let (status, body) = associate(&router, &fixture.target, WORKSPACE, entry).await;
    assert_eq!(status, 200, "the re-association got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "a freed span must be associable again, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.target),
        expected_segments(&fixture, &[0, 1, 2, 3]),
        "the re-associated span must leave the dataset holding what it started with"
    );
}

#[tokio::test]
async fn a_request_the_dataset_does_not_answer_is_refused_and_removes_nothing() {
    let (router, fixture) = fixture().await;
    let external = create_dataset(&router, "Kiln external", Some("EXTERNAL"), None).await;
    // The row scoping the curated dataset to ANOTHER workspace is about the
    // dataset's scoping, so that other workspace is opened: a name this service
    // does not hold is its own row below, and reports the workspace.
    crate::support::ensure_workspace(&router, OTHER_WORKSPACE).await;

    // (case, the datasetId the request addresses, the workspaceName it carries,
    // the status and error code expected, the absence a 404 states) — an id that
    // names nothing, the stored id scoped to a workspace that does not hold it, a
    // dataset of no workspace, the dataset type whose segments are its own data
    // rather than a selection, and a workspace this service does not hold at all,
    // which is reported as the workspace being absent because the request's
    // workspace is resolved before the dataset scoped to it.
    let rows: Vec<(&str, String, &str, u16, &str, String)> = vec![
        (
            "an id that names no dataset",
            ABSENT_DATASET.to_string(),
            WORKSPACE,
            404,
            "ResourceNotFoundException",
            not_found_message(RESOURCE_DATASET, ABSENT_DATASET),
        ),
        (
            "the dataset scoped to another workspace",
            fixture.target.clone(),
            OTHER_WORKSPACE,
            404,
            "ResourceNotFoundException",
            not_found_message(RESOURCE_DATASET, &fixture.target),
        ),
        (
            "a dataset that belongs to no workspace",
            external.clone(),
            WORKSPACE,
            404,
            "ResourceNotFoundException",
            not_found_message(RESOURCE_DATASET, &external),
        ),
        (
            "a session dataset",
            fixture.session.clone(),
            WORKSPACE,
            400,
            "InvalidRequestException",
            String::new(),
        ),
        (
            "a workspace this service does not hold",
            fixture.target.clone(),
            "dryer-ghost",
            404,
            "ResourceNotFoundException",
            not_found_message(RESOURCE_WORKSPACE, "dryer-ghost"),
        ),
    ];

    for (case, target, workspace, expected_status, expected_code, expected_message) in rows {
        let wire = vec![wire_entry(&fixture, &FIRST_EARLY)];
        let (status, body) = disassociate(&router, &target, workspace, wire).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
        if expected_status == 404 {
            let message = body["message"].as_str().unwrap_or("");
            assert_eq!(
                message, expected_message,
                "case {case:?}: an absence must carry the service's not-found wording for the \
                 resource that is absent, got {body}"
            );
        }

        // A refused request removes nothing: the curated dataset still holds every
        // segment the fixture associated.
        assert_eq!(
            held_segments(&fixture.engine, &fixture.target),
            expected_segments(&fixture, &[0, 1, 2, 3]),
            "case {case:?}: a refused request must remove no segment"
        );
    }
}

#[tokio::test]
async fn a_batch_naming_more_entries_than_a_dataset_can_hold_is_refused() {
    let (router, fixture) = fixture().await;

    // A batch AT the bound is work the store could do, so it is answered: the one
    // entry that addresses a held segment is applied and every other entry — each
    // naming a span the dataset holds nothing at — is reported on its own.
    let at_the_bound: Vec<Value> = std::iter::once(wire_entry(&fixture, &FIRST_EARLY))
        .chain((1..MAX_DATA_SEGMENTS_PER_DATASET as i64).map(|span| {
            wire_entry(
                &fixture,
                &Segment {
                    source: Source::Session,
                    series: Series::First,
                    start: (1_000 + span * 10, 0),
                    end: (1_000 + span * 10 + 10, 0),
                },
            )
        }))
        .collect();
    assert_eq!(
        at_the_bound.len(),
        MAX_DATA_SEGMENTS_PER_DATASET,
        "the batch at the bound must carry exactly the bound's entries"
    );
    let (status, body) = disassociate(&router, &fixture.target, WORKSPACE, at_the_bound).await;
    assert_eq!(status, 200, "the batch at the bound got {status} {body}");
    assert_eq!(
        body["failedDisassociations"].as_array().map(Vec::len),
        Some(MAX_DATA_SEGMENTS_PER_DATASET - 1),
        "every entry but the one addressing a held segment must be reported, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.target),
        expected_segments(&fixture, &[1, 2, 3]),
        "the batch at the bound must remove exactly the segment it addressed"
    );

    // One entry past it could not address a distinct segment with every entry, so
    // the request itself is refused and nothing more is removed.
    let over = MAX_DATA_SEGMENTS_PER_DATASET as i64;
    let oversized: Vec<Value> = (0..=over)
        .map(|span| {
            wire_entry(
                &fixture,
                &Segment {
                    source: Source::Session,
                    series: Series::First,
                    start: (span * 10, 0),
                    end: (span * 10 + 10, 0),
                },
            )
        })
        .collect();
    let (status, body) = disassociate(&router, &fixture.target, WORKSPACE, oversized).await;
    assert_eq!(status, 400, "the oversized batch got {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "unexpected error code in {body}"
    );
    let message = body["message"].as_str().unwrap_or("");
    assert!(
        message.contains(&MAX_DATA_SEGMENTS_PER_DATASET.to_string())
            && message.contains(&(MAX_DATA_SEGMENTS_PER_DATASET + 1).to_string()),
        "the refusal must name the bound and the count the request carried, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &fixture.target),
        expected_segments(&fixture, &[1, 2, 3]),
        "a refused request must remove no further segment"
    );
}

#[tokio::test]
async fn a_dataset_the_store_holds_no_type_for_answers_the_disassociation() {
    // `CreateDataset` keeps `datasetType` absent rather than guessing one, so a
    // dataset that states no type states nothing about which side of the
    // curated-only rule it falls on and the disassociation is applied.
    let (router, fixture) = fixture().await;
    let untyped = create_dataset(&router, "Kiln untyped", None, Some(WORKSPACE)).await;

    let segment = Segment {
        source: Source::Session,
        series: Series::Second,
        start: (100, 0),
        end: (110, 0),
    };
    let wire = vec![wire_entry(&fixture, &segment)];
    let (status, body) = associate(&router, &untyped, WORKSPACE, wire.clone()).await;
    assert_eq!(status, 200, "the untyped dataset's association got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "an untyped dataset must take the association, got {body}"
    );

    let (status, body) = disassociate(&router, &untyped, WORKSPACE, wire).await;
    assert_eq!(status, 200, "an untyped dataset got {status} {body}");
    assert_eq!(
        body["failedDisassociations"].as_array().map(Vec::len),
        Some(0),
        "an untyped dataset must apply the entry, got {body}"
    );
    assert_eq!(
        held_segments(&fixture.engine, &untyped),
        Vec::new(),
        "the untyped dataset must hold nothing once its only segment went"
    );
    assert_eq!(
        body["datasetVersion"].as_str(),
        Some("3"),
        "the removal must take the untyped dataset to its next version, got {body}"
    );
}
