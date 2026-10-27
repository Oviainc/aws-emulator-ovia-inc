// SPDX-License-Identifier: CC0-1.0

//! `BatchDeleteDatasetDataSegments` end to end: the data segments a session
//! dataset holds are deleted over HTTP, through the same `MultiServiceRouter` the
//! emulator boots with.
//!
//! The rule these suites pin: a deletion entry addresses ONE data segment of the
//! named dataset by its time series and by its own exact span, so an entry that
//! addresses a stored segment removes that segment and the underlying time series
//! samples inside its half-open span — leaving every other segment and every
//! sample outside it — while an entry that addresses no stored segment, or whose
//! span covers no time, fails on its own with its own `DataSegmentErrorCode` and
//! removes nothing; the dataset reaches its next version only when the call
//! accepted at least one deletion.
//!
//! The table below spans the entries a request can carry: a segment's own span, a
//! span that differs only in its nanosecond offset, a span covering two segments,
//! another series' segment, a segment whose time series the store no longer holds,
//! a repeat of an entry inside one batch, spans covering no time, and batches of
//! one, two, five and the ten entries the model admits. The whole-request refusals
//! (an unknown dataset, a dataset of another workspace, a dataset that is not a
//! session dataset) are their own suite.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `is_modelled_id`)
//! is the one the asset suite already defines; this suite reuses it rather than
//! growing a second.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, DataSegmentRecord, ACTIVATION_DELAY,
    MAX_DATA_SEGMENTS_PER_DATASET, RESOURCE_DATA_SEGMENT,
};

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine};
use crate::support::anchor;

/// The workspace the datasets of this suite live in.
const WORKSPACE: &str = "press-floor";

/// The alias of the first seeded data stream — the one most rows delete segments
/// of.
const FIRST_ALIAS: &str = "/plant/press/1/temperature";

/// The alias of the second seeded data stream, so a row can prove a deletion
/// reaches ONE series' data and not another's.
const SECOND_ALIAS: &str = "/plant/press/2/temperature";

/// A `TimeSeriesId` no stored stream carries: an `ID`-shaped value this suite
/// seeds a segment under, so a row can prove the segment still goes when its
/// underlying data is no longer there to delete.
const ABSENT_TIME_SERIES: &str = "9d4c2b61-7f08-4a35-8c19-5e6b0d3a7f21";

/// The sample seconds, relative to [`anchor`], the first stream is ingested with.
const FIRST_SAMPLES: &[i64] = &[0, 5, 10, 15, 100, 150];

/// The same for the second stream.
const SECOND_SAMPLES: &[i64] = &[0, 5];

/// Which seeded time series an entry (or a seeded segment) names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The stream [`FIRST_ALIAS`] names.
    First,
    /// The stream [`SECOND_ALIAS`] names.
    Second,
    /// [`ABSENT_TIME_SERIES`]: a `TimeSeriesId` no stored stream carries.
    Absent,
}

/// One data segment, as a row seeds it and as an entry addresses it: the time
/// series it covers and the span it covers of it, in seconds relative to
/// [`anchor`] plus a nanosecond offset.
#[derive(Debug, Clone, Copy)]
struct Span {
    target: Target,
    start: (i64, i32),
    end: (i64, i32),
}

/// The segments every row of the table starts from: three adjacent and
/// non-adjacent spans of the first stream (the last with a nanosecond offset on
/// its end), one of the second stream at a span the first stream also has a
/// segment at, and one of a time series the store does not hold.
const SEEDED: &[Span] = &[FIRST_EARLY, FIRST_MIDDLE, FIRST_OFFSET, SECOND_EARLY, ABSENT_EARLY];

/// The first stream's earliest segment; see [`SEEDED`].
const FIRST_EARLY: Span = Span {
    target: Target::First,
    start: (0, 0),
    end: (10, 0),
};

/// The first stream's segment immediately after [`FIRST_EARLY`], so a row can
/// prove a deletion stops at its own span.
const FIRST_MIDDLE: Span = Span {
    target: Target::First,
    start: (10, 0),
    end: (20, 0),
};

/// The first stream's last segment, whose end carries a nanosecond offset.
const FIRST_OFFSET: Span = Span {
    target: Target::First,
    start: (100, 0),
    end: (160, 250_000_000),
};

/// The second stream's segment, at the span [`FIRST_EARLY`] also covers.
const SECOND_EARLY: Span = Span {
    target: Target::Second,
    start: (0, 0),
    end: (10, 0),
};

/// A segment of a time series the store does not hold.
const ABSENT_EARLY: Span = Span {
    target: Target::Absent,
    start: (0, 0),
    end: (10, 0),
};

/// The `DataSegmentErrorCode` an entry addressing no stored segment reports.
/// Source: `smithy_query(query="shape", shape="DataSegmentErrorCode")` —
/// INTERNAL_FAILURE | VALIDATION_ERROR | RESOURCE_NOT_FOUND | LIMIT_EXCEEDED |
/// CONFLICTING_OPERATION.
const RESOURCE_NOT_FOUND: &str = "RESOURCE_NOT_FOUND";

/// The `DataSegmentErrorCode` an entry whose span covers no time reports; see
/// [`RESOURCE_NOT_FOUND`] for the value list.
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// The code a row expects for an entry the call APPLIED, which reports no error
/// element at all.
const APPLIED: &str = "";

/// One row of the deletion table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The entries the request carries, in order.
    entries: &'static [Span],
    /// What each entry answers, in the same order: [`APPLIED`], or the
    /// `DataSegmentErrorCode` it is reported in `errors` with.
    expected: &'static [&'static str],
    /// The indices into [`SEEDED`] the dataset must still hold afterwards.
    remaining: &'static [usize],
    /// The sample seconds the first stream must still report.
    first_samples: &'static [i64],
    /// The same for the second stream.
    second_samples: &'static [i64],
    /// The `datasetVersion` the call reports: the created dataset is at 1, and a
    /// call that accepted at least one deletion takes it to 2.
    version: &'static str,
}

/// Ten entries — the longest `deleteDataSegmentEntries` the model admits
/// (`@length` 1..10) — of which the first two address stored segments.
const TEN_ENTRIES: &[Span] = &[
    FIRST_EARLY,
    FIRST_MIDDLE,
    Span {
        target: Target::First,
        start: (200, 0),
        end: (210, 0),
    },
    Span {
        target: Target::First,
        start: (210, 0),
        end: (220, 0),
    },
    Span {
        target: Target::First,
        start: (220, 0),
        end: (230, 0),
    },
    Span {
        target: Target::Second,
        start: (200, 0),
        end: (210, 0),
    },
    Span {
        target: Target::Second,
        start: (210, 0),
        end: (220, 0),
    },
    Span {
        target: Target::Absent,
        start: (200, 0),
        end: (210, 0),
    },
    Span {
        target: Target::Absent,
        start: (210, 0),
        end: (220, 0),
    },
    Span {
        target: Target::Absent,
        start: (220, 0),
        end: (230, 0),
    },
];

fn table() -> Vec<Row> {
    vec![
        Row {
            case: "the first segment of a series",
            entries: &[FIRST_EARLY],
            expected: &[APPLIED],
            remaining: &[1, 2, 3, 4],
            first_samples: &[10, 15, 100, 150],
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
        Row {
            case: "the segment between two others",
            entries: &[FIRST_MIDDLE],
            expected: &[APPLIED],
            remaining: &[0, 2, 3, 4],
            first_samples: &[0, 5, 100, 150],
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
        Row {
            case: "a span whose end carries a nanosecond offset",
            entries: &[FIRST_OFFSET],
            expected: &[APPLIED],
            remaining: &[0, 1, 3, 4],
            first_samples: &[0, 5, 10, 15],
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
        Row {
            case: "that same span to the whole second",
            entries: &[Span {
                target: Target::First,
                start: (100, 0),
                end: (160, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            remaining: &[0, 1, 2, 3, 4],
            first_samples: FIRST_SAMPLES,
            second_samples: SECOND_SAMPLES,
            version: "1",
        },
        Row {
            case: "a span covering two whole segments",
            entries: &[Span {
                target: Target::First,
                start: (0, 0),
                end: (20, 0),
            }],
            expected: &[RESOURCE_NOT_FOUND],
            remaining: &[0, 1, 2, 3, 4],
            first_samples: FIRST_SAMPLES,
            second_samples: SECOND_SAMPLES,
            version: "1",
        },
        Row {
            case: "another series' segment at the same span",
            entries: &[SECOND_EARLY],
            expected: &[APPLIED],
            remaining: &[0, 1, 2, 4],
            first_samples: FIRST_SAMPLES,
            second_samples: &[],
            version: "2",
        },
        Row {
            case: "a segment whose time series the store does not hold",
            entries: &[ABSENT_EARLY],
            expected: &[APPLIED],
            remaining: &[0, 1, 2, 3],
            first_samples: FIRST_SAMPLES,
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
        Row {
            case: "two entries, one stored segment and one absent",
            entries: &[
                FIRST_EARLY,
                Span {
                    target: Target::First,
                    start: (5, 0),
                    end: (15, 0),
                },
            ],
            expected: &[APPLIED, RESOURCE_NOT_FOUND],
            remaining: &[1, 2, 3, 4],
            first_samples: &[10, 15, 100, 150],
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
        Row {
            case: "the same segment twice in one batch",
            entries: &[FIRST_EARLY, FIRST_EARLY],
            expected: &[APPLIED, RESOURCE_NOT_FOUND],
            remaining: &[1, 2, 3, 4],
            first_samples: &[10, 15, 100, 150],
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
        Row {
            case: "a span covering no time",
            entries: &[Span {
                target: Target::First,
                start: (10, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            remaining: &[0, 1, 2, 3, 4],
            first_samples: FIRST_SAMPLES,
            second_samples: SECOND_SAMPLES,
            version: "1",
        },
        Row {
            case: "a span whose end precedes its start",
            entries: &[Span {
                target: Target::First,
                start: (20, 0),
                end: (10, 0),
            }],
            expected: &[VALIDATION_ERROR],
            remaining: &[0, 1, 2, 3, 4],
            first_samples: FIRST_SAMPLES,
            second_samples: SECOND_SAMPLES,
            version: "1",
        },
        Row {
            case: "a span covering no time beside a stored segment",
            entries: &[
                Span {
                    target: Target::First,
                    start: (10, 0),
                    end: (10, 0),
                },
                FIRST_EARLY,
            ],
            expected: &[VALIDATION_ERROR, APPLIED],
            remaining: &[1, 2, 3, 4],
            first_samples: &[10, 15, 100, 150],
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
        Row {
            case: "every segment the dataset holds",
            entries: SEEDED,
            expected: &[APPLIED, APPLIED, APPLIED, APPLIED, APPLIED],
            remaining: &[],
            first_samples: &[],
            second_samples: &[],
            version: "2",
        },
        Row {
            case: "ten entries, the longest batch the model admits",
            entries: TEN_ENTRIES,
            expected: &[
                APPLIED,
                APPLIED,
                RESOURCE_NOT_FOUND,
                RESOURCE_NOT_FOUND,
                RESOURCE_NOT_FOUND,
                RESOURCE_NOT_FOUND,
                RESOURCE_NOT_FOUND,
                RESOURCE_NOT_FOUND,
                RESOURCE_NOT_FOUND,
                RESOURCE_NOT_FOUND,
            ],
            remaining: &[2, 3, 4],
            first_samples: &[100, 150],
            second_samples: SECOND_SAMPLES,
            version: "2",
        },
    ]
}

/// A table second as the absolute `timeInSeconds` the wire carries, anchored
/// inside the window `BatchPutAssetPropertyValue` accepts; see `super::support`.
pub(crate) fn at(seconds: i64) -> i64 {
    anchor() + seconds
}

/// The inverse of [`at`].
pub(crate) fn relative(seconds: i64) -> i64 {
    seconds - anchor()
}

/// A modelled `TimeInNanos` at the table instant `(seconds, nanos)`.
pub(crate) fn instant(seconds: i64, nanos: i32) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: at(seconds),
        offset_in_nanos: Some(nanos),
    }
}

/// The same, on the wire.
pub(crate) fn wire_instant(seconds: i64, nanos: i32) -> Value {
    json!({ "timeInSeconds": at(seconds), "offsetInNanos": nanos })
}

/// The time series ids of the three targets, as the store minted them.
#[derive(Debug, Clone)]
struct Streams {
    first: String,
    second: String,
}

impl Streams {
    /// The `TimeSeriesId` `target` names.
    fn id(&self, target: Target) -> &str {
        match target {
            Target::First => &self.first,
            Target::Second => &self.second,
            Target::Absent => ABSENT_TIME_SERIES,
        }
    }
}

/// `POST /datasets`: create a dataset of `dataset_type` in `workspace` and answer
/// its minted id.
///
/// Shared with the association suite (`dataset_data_segment_association`), which
/// drives the producer side of the same data segments these suites delete, rather
/// than growing a second copy of this helper and the four below it.
pub(crate) async fn create_dataset(
    router: &MultiServiceRouter,
    name: &str,
    dataset_type: Option<&str>,
    workspace: Option<&str>,
) -> String {
    let mut body = json!({
        "datasetName": name,
        "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(dataset_type) = dataset_type {
        object.insert("datasetType".to_string(), json!(dataset_type));
    }
    if let Some(workspace) = workspace {
        object.insert("workspaceName".to_string(), json!(workspace));
    }
    let (status, created) = call(router, "POST", "/datasets", Some(body)).await;
    assert_eq!(status, 202, "create {name}: got {status} {created}");
    let id = created["datasetId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "create {name}: expected a modelled datasetId, got {created}"
    );
    id
}

/// Ingest a DOUBLE sample at each of `seconds` against `alias`, which CREATES the
/// data stream that alias names.
pub(crate) async fn ingest(router: &MultiServiceRouter, alias: &str, seconds: &[i64]) {
    let entries: Vec<Value> = seconds
        .iter()
        .map(|second| {
            json!({
                "entryId": format!("s{second}"),
                "propertyAlias": alias,
                "propertyValues": [{
                    "value": { "doubleValue": *second as f64 },
                    "timestamp": { "timeInSeconds": at(*second), "offsetInNanos": 0 },
                }],
            })
        })
        .collect();
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(status, 200, "ingest {alias}: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "ingest {alias}: every entry must be accepted, got {body}"
    );
}

/// The `timeSeriesId` `DescribeTimeSeries` reports for `alias`.
pub(crate) async fn time_series_id(router: &MultiServiceRouter, alias: &str) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/timeseries/describe?alias={alias}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe {alias}: got {status} {body}");
    body["timeSeriesId"].as_str().unwrap_or("").to_string()
}

/// The table seconds `GetAssetPropertyValueHistory` still reports for `alias`.
async fn history_seconds(router: &MultiServiceRouter, alias: &str) -> Vec<i64> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/properties/history?propertyAlias={alias}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "history {alias}: got {status} {body}");
    body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetPropertyValueHistory in {body}"))
        .iter()
        .map(|sample| {
            relative(
                sample["timestamp"]["timeInSeconds"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("a sample with no timeInSeconds in {body}")),
            )
        })
        .collect()
}

/// `POST /datasets/{datasetId}/data-segments/batch-delete` with `entries`.
async fn delete_segments(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
    entries: Vec<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/datasets/{dataset_id}/data-segments/batch-delete"),
        Some(json!({ "workspaceName": workspace, "deleteDataSegmentEntries": entries })),
    )
    .await
}

/// One `DeleteDataSegmentEntry` on the wire.
fn entry(streams: &Streams, span: &Span) -> Value {
    json!({
        "timeSeriesId": streams.id(span.target),
        "startTimestamp": wire_instant(span.start.0, span.start.1),
        "endTimestamp": wire_instant(span.end.0, span.end.1),
    })
}

/// `GET /datasets/{datasetId}?workspaceName=…`.
async fn describe_dataset(router: &MultiServiceRouter, dataset_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets/{dataset_id}?workspaceName={WORKSPACE}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe {dataset_id}: got {status} {body}");
    body
}

/// Seed `spans` as the data segments of `dataset_id`, through the producer entry
/// point the state exposes — the operations that associate or ingest a dataset's
/// segments are not implemented yet, so this is where a dataset's segments come
/// from today.
fn seed_segments(
    engine: &Arc<engine::InMemoryEngine>,
    dataset_id: &str,
    streams: &Streams,
    spans: &[Span],
) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for span in spans {
        let stored = store.put_dataset_data_segment(
            dataset_id,
            DataSegmentRecord {
                source_dataset_id: dataset_id.to_string(),
                time_series_id: streams.id(span.target).to_string(),
                start_timestamp: instant(span.start.0, span.start.1),
                end_timestamp: instant(span.end.0, span.end.1),
            },
        );
        assert!(stored, "seeding {span:?} must be accepted");
    }
}

/// One segment as these suites compare them: the `TimeSeriesId` it covers and
/// its span, in the table seconds and nanosecond offsets [`SEEDED`] is written
/// in.
type Held = (String, (i64, i32), (i64, i32));

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

/// The segments `remaining` names, sorted the way the store keeps them.
fn expected_segments(streams: &Streams, remaining: &[usize]) -> Vec<Held> {
    let mut expected: Vec<Held> = remaining
        .iter()
        .map(|at| {
            let span = SEEDED[*at];
            (streams.id(span.target).to_string(), span.start, span.end)
        })
        .collect();
    expected.sort();
    expected
}

/// A dataset of `WORKSPACE` with the two data streams ingested, the segments of
/// [`SEEDED`] seeded on it, and the ids those streams were minted with.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String, Streams) {
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Press session", Some("SESSION"), Some(WORKSPACE)).await;
    ingest(&router, FIRST_ALIAS, FIRST_SAMPLES).await;
    ingest(&router, SECOND_ALIAS, SECOND_SAMPLES).await;
    let streams = Streams {
        first: time_series_id(&router, FIRST_ALIAS).await,
        second: time_series_id(&router, SECOND_ALIAS).await,
    };
    assert!(
        is_modelled_id(&streams.first) && is_modelled_id(&streams.second),
        "a disassociated stream's timeSeriesId must be a modelled ID, got {streams:?}"
    );
    seed_segments(&engine, &dataset_id, &streams, SEEDED);
    (router, engine, dataset_id, streams)
}

#[tokio::test]
async fn a_deletion_entry_removes_the_segment_and_the_data_its_own_span_covers() {
    for row in table() {
        let Row {
            case,
            entries,
            expected,
            remaining,
            first_samples,
            second_samples,
            version,
        } = row;
        assert_eq!(
            entries.len(),
            expected.len(),
            "case {case:?}: the table must say what every entry answers"
        );

        let (router, engine, dataset_id, streams) = seeded().await;
        let wire: Vec<Value> = entries.iter().map(|span| entry(&streams, span)).collect();
        let (status, body) = delete_segments(&router, &dataset_id, WORKSPACE, wire).await;

        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["datasetId"].as_str(),
            Some(dataset_id.as_str()),
            "case {case:?}: the answer must name the dataset the request named, got {body}"
        );
        assert_eq!(
            body["datasetVersion"].as_str(),
            Some(version),
            "case {case:?}: expected datasetVersion {version}, got {body}"
        );

        // `errors` carries one element per entry the call could not apply, in the
        // order those entries were sent, each echoing the segment it addressed.
        let failures: Vec<(&Span, &&str)> = entries
            .iter()
            .zip(expected.iter())
            .filter(|(_, code)| **code != APPLIED)
            .collect();
        let reported = body["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no errors list in {body}"));
        assert_eq!(
            reported.len(),
            failures.len(),
            "case {case:?}: expected {} failed entries, got {body}",
            failures.len()
        );
        for ((span, code), element) in failures.iter().zip(reported.iter()) {
            let series = streams.id(span.target);
            assert_eq!(
                element["timeSeriesId"].as_str(),
                Some(series),
                "case {case:?}: a failure must echo its own timeSeriesId, got {element}"
            );
            assert_eq!(
                (
                    element["startTimestamp"]["timeInSeconds"].as_i64().map(relative),
                    element["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
                (Some(span.start.0), span.start.1),
                "case {case:?}: a failure must echo its own startTimestamp, got {element}"
            );
            assert_eq!(
                (
                    element["endTimestamp"]["timeInSeconds"].as_i64().map(relative),
                    element["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
                ),
                (Some(span.end.0), span.end.1),
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
                assert!(
                    message.contains("startTimestamp") && message.contains("endTimestamp"),
                    "case {case:?}: a validation failure must name the members it refused, \
                     got {element}"
                );
            }
        }

        // The segments the call did not apply an entry to are still held, and the
        // ones it did are gone.
        assert_eq!(
            held_segments(&engine, &dataset_id),
            expected_segments(&streams, remaining),
            "case {case:?}: unexpected segments left on the dataset"
        );

        // "Deleting a data segment deletes the underlying time series data for the
        // segment's time range": the samples inside an applied entry's half-open
        // span are gone from the stream it covered, and every other sample stays.
        assert_eq!(
            history_seconds(&router, FIRST_ALIAS).await,
            first_samples,
            "case {case:?}: unexpected samples left on {FIRST_ALIAS}"
        );
        assert_eq!(
            history_seconds(&router, SECOND_ALIAS).await,
            second_samples,
            "case {case:?}: unexpected samples left on {SECOND_ALIAS}"
        );

        // The version the call reported is the version the dataset reports.
        let described = describe_dataset(&router, &dataset_id).await;
        assert_eq!(
            described["datasetVersion"].as_str(),
            Some(version),
            "case {case:?}: the dataset must report the version the call answered, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn a_deleted_segment_is_absent_to_the_next_deletion_and_the_dataset_settles() {
    let (router, _engine, dataset_id, streams) = seeded().await;
    let wire = vec![entry(&streams, &FIRST_EARLY)];

    let (status, first) = delete_segments(&router, &dataset_id, WORKSPACE, wire.clone()).await;
    assert_eq!(status, 200, "the first deletion got {status} {first}");
    assert_eq!(
        first["errors"].as_array().map(Vec::len),
        Some(0),
        "the first deletion must apply its entry, got {first}"
    );

    // A write the dataset accepted leaves it UPDATING until the transition that
    // write owes falls due, exactly as an update does.
    let described = describe_dataset(&router, &dataset_id).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("UPDATING"),
        "straight after a deletion the dataset must still be settling, got {described}"
    );
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;
    let described = describe_dataset(&router, &dataset_id).await;
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("ACTIVE"),
        "once the owed transition falls due the dataset must be ACTIVE, got {described}"
    );
    assert_eq!(
        described["datasetVersion"].as_str(),
        Some("2"),
        "the accepted deletion must take the dataset to its next version, got {described}"
    );

    // The same entry again addresses a segment that is no longer there, so it
    // fails on its own and the dataset stays where the first call left it.
    let (status, again) = delete_segments(&router, &dataset_id, WORKSPACE, wire).await;
    assert_eq!(status, 200, "the second deletion got {status} {again}");
    assert_eq!(
        again["errors"][0]["errorCode"].as_str(),
        Some(RESOURCE_NOT_FOUND),
        "a repeat of an applied entry must report the absence, got {again}"
    );
    assert_eq!(
        again["datasetVersion"].as_str(),
        Some("2"),
        "a call that accepted no deletion must not take the dataset to a new version, got \
         {again}"
    );
    let described = describe_dataset(&router, &dataset_id).await;
    assert_eq!(
        described["datasetVersion"].as_str(),
        Some("2"),
        "the dataset must stay at the version the applied deletion left it, got {described}"
    );
    assert_eq!(
        described["datasetStatus"]["state"].as_str(),
        Some("ACTIVE"),
        "a call that wrote nothing must not restart the dataset's settling, got {described}"
    );
}

#[tokio::test]
async fn a_request_the_dataset_does_not_answer_is_refused_and_writes_nothing() {
    // (case, the datasetId the request names, the workspaceName it carries, the
    // status and error code expected) — an id that names nothing, the stored id
    // scoped to a workspace that does not hold it, and the two dataset types whose
    // segments this operation does not delete.
    let (router, engine, dataset_id, streams) = seeded().await;
    let curated =
        create_dataset(&router, "Press curated", Some("CURATED"), Some(WORKSPACE)).await;
    let external = create_dataset(&router, "Press external", Some("EXTERNAL"), None).await;
    seed_segments(&engine, &curated, &streams, &SEEDED[..1]);

    let rows: Vec<(&str, String, &str, u16, &str)> = vec![
        (
            "an id that names no dataset",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
            WORKSPACE,
            404,
            "ResourceNotFoundException",
        ),
        (
            "the dataset scoped to another workspace",
            dataset_id.clone(),
            "other-floor",
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
            "a curated dataset",
            curated.clone(),
            WORKSPACE,
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, target, workspace, expected_status, expected_code) in rows {
        let wire = vec![entry(&streams, &FIRST_EARLY)];
        let (status, body) = delete_segments(&router, &target, workspace, wire).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code in {body}"
        );

        // A refused request deletes nothing: neither the session dataset's
        // segments and samples, nor the curated dataset's segment.
        assert_eq!(
            held_segments(&engine, &dataset_id),
            expected_segments(&streams, &[0, 1, 2, 3, 4]),
            "case {case:?}: a refused request must leave every segment in place"
        );
        assert_eq!(
            held_segments(&engine, &curated),
            expected_segments(&streams, &[0]),
            "case {case:?}: a refused request must leave the curated dataset alone"
        );
        assert_eq!(
            history_seconds(&router, FIRST_ALIAS).await,
            FIRST_SAMPLES,
            "case {case:?}: a refused request must leave every sample in place"
        );
    }

    // The dataset's OWN workspace resolves, so the scoping above refuses only a
    // workspace that does not hold it.
    let wire = vec![entry(&streams, &FIRST_EARLY)];
    let (status, body) = delete_segments(&router, &dataset_id, WORKSPACE, wire).await;
    assert_eq!(status, 200, "the dataset's own workspace got {status} {body}");
}

#[tokio::test]
async fn a_dataset_the_store_holds_no_type_for_answers_the_deletion() {
    // `CreateDataset` keeps `datasetType` absent rather than guessing one, so a
    // dataset that states no type states nothing about which side of the
    // session-only rule it falls on and the deletion is applied.
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Press untyped", None, Some(WORKSPACE)).await;
    ingest(&router, FIRST_ALIAS, FIRST_SAMPLES).await;
    let streams = Streams {
        first: time_series_id(&router, FIRST_ALIAS).await,
        second: ABSENT_TIME_SERIES.to_string(),
    };
    seed_segments(&engine, &dataset_id, &streams, &SEEDED[..1]);

    let wire = vec![entry(&streams, &FIRST_EARLY)];
    let (status, body) = delete_segments(&router, &dataset_id, WORKSPACE, wire).await;
    assert_eq!(status, 200, "an untyped dataset got {status} {body}");
    assert_eq!(
        body["errors"].as_array().map(Vec::len),
        Some(0),
        "an untyped dataset must apply the entry, got {body}"
    );
    assert_eq!(
        held_segments(&engine, &dataset_id),
        Vec::new(),
        "the applied entry must leave the dataset holding no segment"
    );
    assert_eq!(
        history_seconds(&router, FIRST_ALIAS).await,
        [10, 15, 100, 150],
        "the applied entry must delete the data its span covered"
    );
}

#[tokio::test]
async fn the_producer_entry_point_refuses_what_the_store_cannot_hold() {
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Press bounds", Some("SESSION"), Some(WORKSPACE)).await;
    let state = service_state(&*engine).expect("the IoT SiteWise store");

    let segment = |start: (i64, i32), end: (i64, i32)| DataSegmentRecord {
        source_dataset_id: dataset_id.clone(),
        time_series_id: ABSENT_TIME_SERIES.to_string(),
        start_timestamp: instant(start.0, start.1),
        end_timestamp: instant(end.0, end.1),
    };

    // (case, the span offered, whether the store takes it) — a span covering no
    // time, one whose end precedes its start, and one whose offset is outside the
    // modelled `TimeInNanos` (`offsetInNanos` is range 0..999999999).
    let rows: Vec<(&str, DataSegmentRecord, bool)> = vec![
        ("a span covering no time", segment((0, 0), (0, 0)), false),
        ("an inverted span", segment((10, 0), (0, 0)), false),
        (
            "an offset outside the modelled range",
            segment((0, 0), (10, 1_000_000_000)),
            false,
        ),
        ("a span covering time", segment((0, 0), (10, 0)), true),
    ];
    for (case, offered, accepted) in rows {
        let mut store = state.lock();
        assert_eq!(
            store.put_dataset_data_segment(&dataset_id, offered),
            accepted,
            "case {case:?}: expected the producer to answer {accepted}"
        );
    }
    assert_eq!(
        held_segments(&engine, &dataset_id).len(),
        1,
        "only the acceptable span may be held"
    );

    // Storing the same series' same span again REPLACES the entry held for it, so
    // the dataset still holds one segment.
    {
        let mut store = state.lock();
        assert!(
            store.put_dataset_data_segment(&dataset_id, segment((0, 0), (10, 0))),
            "re-storing a held span must be accepted"
        );
    }
    assert_eq!(
        held_segments(&engine, &dataset_id).len(),
        1,
        "re-storing a held span must not hold it twice"
    );

    // Past the per-dataset bound the write is refused rather than evicting a
    // segment a deletion would then report as absent.
    {
        let mut store = state.lock();
        for span in 1..MAX_DATA_SEGMENTS_PER_DATASET as i64 {
            assert!(
                store.put_dataset_data_segment(
                    &dataset_id,
                    segment((span * 10, 0), (span * 10 + 10, 0))
                ),
                "span {span} must be accepted below the bound"
            );
        }
        assert_eq!(
            store.dataset_data_segments[&dataset_id].len(),
            MAX_DATA_SEGMENTS_PER_DATASET,
            "the dataset must hold exactly the bound"
        );
        let over = MAX_DATA_SEGMENTS_PER_DATASET as i64;
        assert!(
            !store.put_dataset_data_segment(
                &dataset_id,
                segment((over * 10, 0), (over * 10 + 10, 0))
            ),
            "past the bound the producer must refuse"
        );
        assert_eq!(
            store.dataset_data_segments[&dataset_id].len(),
            MAX_DATA_SEGMENTS_PER_DATASET,
            "a refused write must not grow the dataset"
        );
    }
}
