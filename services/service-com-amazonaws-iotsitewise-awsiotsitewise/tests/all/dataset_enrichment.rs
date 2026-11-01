// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the enrichment a dataset and its data segments report,
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots
//! with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a dataset read derives the enrichment it reports
//! from the enrichment jobs the store holds for that dataset. A data segment
//! reports `ENRICHED`, at the instant the analysing job completed, exactly when
//! a COMPLETED job of its dataset analyses the segment's time series and its
//! trim window contains the segment's span whole ("Trim settings retrieve fully
//! contained data segments within the specified time range"), and
//! `NOT_ENRICHED` otherwise; the dataset's own `video` entry aggregates those
//! statuses — `FULLY_ENRICHED` when every segment it holds is enriched,
//! `PARTIALLY_ENRICHED` when only some are, `NOT_ENRICHED` when none is — and
//! carries the latest of their instants. A dataset no enrichment job names
//! reports no enrichment at all, on the dataset or on any segment of it.
//!
//! The rows differ along what that rule generalises over: how many segments a
//! dataset holds and how many of them a window contains, which identifier a
//! configuration names the analysed time series by (a `timeSeriesId`, a
//! `propertyAlias`, one no stream answers to, or neither member), whether the
//! window meets the segment's bounds exactly, misses them by a nanosecond or
//! only overlaps the segment, whether the analysing job was cancelled, and how
//! many jobs analyse one segment.
//!
//! The HTTP harness (`call`, `router_with_engine`) is the one `asset_lifecycle`
//! defines and the enrichment-job helpers (`create`, `jobs_uri`, `instant`,
//! `settled`) are `enrichment_jobs`'; this suite reuses both rather than growing
//! a second copy.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, DataSegmentRecord,
};

use crate::asset_lifecycle::{call, router_with_engine};
use crate::enrichment_jobs::{create as create_job, instant, jobs_uri, settled};
use crate::support::{anchor, ensure_workspace};

/// The workspace every dataset and job of this suite lives in.
const WORKSPACE: &str = "enrichment-view-floor";

/// The alias of the first video stream the datasets below hold segments of.
const CAMERA_ALIAS: &str = "/plant/line-1/camera";

/// The alias of a second stream, so a job naming ONE series cannot pass by
/// enriching every segment of the dataset.
const DOOR_ALIAS: &str = "/plant/line-1/door-camera";

/// An alias no stream of this store answers to, for the row about a
/// configuration naming a series this store cannot resolve.
const UNKNOWN_ALIAS: &str = "/plant/line-9/no-such-camera";

/// A `TimeSeriesId` no stored stream carries: an `ID`-shaped value a segment is
/// seeded under, so a row can prove a job that names no series analyses even the
/// segments of a stream this store holds nothing for.
const ABSENT_SERIES: &str = "9d4c2b61-7f08-4a35-8c19-5e6b0d3a7f21";

/// Every `DatasetEnrichmentStatus` the model declares — what
/// `DatasetEnrichmentEntry$status` targets. Source: `smithy_query(query="shape",
/// shape="DatasetEnrichmentStatus")`.
const DATASET_ENRICHMENT_STATUSES: [&str; 3] =
    ["FULLY_ENRICHED", "PARTIALLY_ENRICHED", "NOT_ENRICHED"];

/// Every `EnrichmentStatus` the model declares — what
/// `DataSegmentEnrichment$status` targets. Source: `smithy_query(query="shape",
/// shape="EnrichmentStatus")`.
const SEGMENT_ENRICHMENT_STATUSES: [&str; 2] = ["ENRICHED", "NOT_ENRICHED"];

/// The sample seconds, relative to [`anchor`], each stream is ingested with —
/// which is what CREATES the stream its alias names.
const SAMPLE_SECONDS: &[i64] = &[0, 10, 20, 30];

/// An instant of a table row: seconds relative to [`anchor`], plus a nanosecond
/// offset.
type Stamp = (i64, i32);

/// Which seeded time series a segment covers, or a job names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Series {
    /// The stream [`CAMERA_ALIAS`] names.
    Camera,
    /// The stream [`DOOR_ALIAS`] names.
    Door,
    /// [`ABSENT_SERIES`]: a `TimeSeriesId` this store holds no stream for.
    Absent,
}

/// How a job's `eventDetection` configuration identifies the video it analyses.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// By the `timeSeriesId` of that series.
    Series(Series),
    /// By the `propertyAlias` bound to it.
    Alias(Series),
    /// By a `propertyAlias` no stream of this store answers to.
    UnknownAlias,
    /// By neither member — the dataset and the trim window alone.
    DatasetOnly,
}

/// One enrichment job a row creates on its dataset.
#[derive(Debug, Clone, Copy)]
struct JobSeed {
    target: Target,
    /// The `trimSettings` window, as `[startTime, endTime]`.
    trim: (Stamp, Stamp),
    /// `true` when the row cancels the job straight after creating it, so the
    /// clock never takes it to `COMPLETED`.
    cancel: bool,
}

/// One data segment a row's dataset holds, and what the rule says of it.
#[derive(Debug, Clone, Copy)]
struct SegmentSeed {
    series: Series,
    start: Stamp,
    end: Stamp,
    /// The index, in the row's `jobs`, of the job whose completion is this
    /// segment's `lastEnrichedAt` — or `None` when no job of the row enriches
    /// it.
    enriched_by: Option<usize>,
}

/// One row: a dataset, the segments it holds, the jobs created on it, and the
/// `DatasetEnrichmentStatus` it must then report.
#[derive(Debug, Clone, Copy)]
struct Row {
    label: &'static str,
    segments: &'static [SegmentSeed],
    jobs: &'static [JobSeed],
    /// The `status` of the dataset's `video` entry, or `None` when the dataset
    /// reports no `enrichmentStatus` member at all.
    dataset_status: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        label: "no enrichment job names the dataset",
        segments: &[
            SegmentSeed {
                series: Series::Camera,
                start: (0, 0),
                end: (10, 0),
                enriched_by: None,
            },
            SegmentSeed {
                series: Series::Door,
                start: (0, 0),
                end: (10, 0),
                enriched_by: None,
            },
        ],
        jobs: &[],
        dataset_status: None,
    },
    Row {
        label: "one completed job whose window contains every segment",
        segments: &[
            SegmentSeed {
                series: Series::Camera,
                start: (0, 0),
                end: (10, 0),
                enriched_by: Some(0),
            },
            SegmentSeed {
                series: Series::Camera,
                start: (10, 0),
                end: (20, 0),
                enriched_by: Some(0),
            },
        ],
        jobs: &[JobSeed {
            target: Target::Series(Series::Camera),
            trim: ((0, 0), (20, 0)),
            cancel: false,
        }],
        dataset_status: Some("FULLY_ENRICHED"),
    },
    Row {
        label: "one completed job whose window contains only some segments",
        segments: &[
            SegmentSeed {
                series: Series::Camera,
                start: (0, 0),
                end: (10, 0),
                enriched_by: Some(0),
            },
            SegmentSeed {
                series: Series::Camera,
                start: (10, 0),
                end: (20, 0),
                enriched_by: Some(0),
            },
            SegmentSeed {
                series: Series::Camera,
                start: (20, 0),
                end: (30, 0),
                enriched_by: None,
            },
        ],
        jobs: &[JobSeed {
            target: Target::Series(Series::Camera),
            trim: ((0, 0), (20, 0)),
            cancel: false,
        }],
        dataset_status: Some("PARTIALLY_ENRICHED"),
    },
    Row {
        label: "a window that only overlaps the segment",
        segments: &[SegmentSeed {
            series: Series::Camera,
            start: (0, 0),
            end: (10, 0),
            enriched_by: None,
        }],
        jobs: &[JobSeed {
            target: Target::Series(Series::Camera),
            trim: ((5, 0), (20, 0)),
            cancel: false,
        }],
        dataset_status: Some("NOT_ENRICHED"),
    },
    Row {
        label: "a window meeting the segment's sub-second bounds exactly",
        segments: &[SegmentSeed {
            series: Series::Camera,
            start: (0, 500_000_000),
            end: (10, 250_000_000),
            enriched_by: Some(0),
        }],
        jobs: &[JobSeed {
            target: Target::Series(Series::Camera),
            trim: ((0, 500_000_000), (10, 250_000_000)),
            cancel: false,
        }],
        dataset_status: Some("FULLY_ENRICHED"),
    },
    Row {
        label: "a window starting one nanosecond after the segment does",
        segments: &[SegmentSeed {
            series: Series::Camera,
            start: (0, 500_000_000),
            end: (10, 250_000_000),
            enriched_by: None,
        }],
        jobs: &[JobSeed {
            target: Target::Series(Series::Camera),
            trim: ((0, 500_000_001), (10, 250_000_000)),
            cancel: false,
        }],
        dataset_status: Some("NOT_ENRICHED"),
    },
    Row {
        label: "a window ending one nanosecond before the segment does",
        segments: &[SegmentSeed {
            series: Series::Camera,
            start: (0, 0),
            end: (10, 250_000_000),
            enriched_by: None,
        }],
        jobs: &[JobSeed {
            target: Target::Series(Series::Camera),
            trim: ((0, 0), (10, 249_999_999)),
            cancel: false,
        }],
        dataset_status: Some("NOT_ENRICHED"),
    },
    Row {
        label: "a job naming one series of two by its propertyAlias",
        segments: &[
            SegmentSeed {
                series: Series::Camera,
                start: (0, 0),
                end: (10, 0),
                enriched_by: None,
            },
            SegmentSeed {
                series: Series::Door,
                start: (0, 0),
                end: (10, 0),
                enriched_by: Some(0),
            },
        ],
        jobs: &[JobSeed {
            target: Target::Alias(Series::Door),
            trim: ((0, 0), (10, 0)),
            cancel: false,
        }],
        dataset_status: Some("PARTIALLY_ENRICHED"),
    },
    Row {
        label: "a job naming neither identifier analyses every series in its window",
        segments: &[
            SegmentSeed {
                series: Series::Camera,
                start: (0, 0),
                end: (10, 0),
                enriched_by: Some(0),
            },
            SegmentSeed {
                series: Series::Door,
                start: (0, 0),
                end: (10, 0),
                enriched_by: Some(0),
            },
            SegmentSeed {
                series: Series::Absent,
                start: (0, 0),
                end: (10, 0),
                enriched_by: Some(0),
            },
        ],
        jobs: &[JobSeed {
            target: Target::DatasetOnly,
            trim: ((0, 0), (10, 0)),
            cancel: false,
        }],
        dataset_status: Some("FULLY_ENRICHED"),
    },
    Row {
        label: "a cancelled job enriches nothing",
        segments: &[SegmentSeed {
            series: Series::Camera,
            start: (0, 0),
            end: (10, 0),
            enriched_by: None,
        }],
        jobs: &[JobSeed {
            target: Target::Series(Series::Camera),
            trim: ((0, 0), (10, 0)),
            cancel: true,
        }],
        dataset_status: Some("NOT_ENRICHED"),
    },
    Row {
        label: "a job whose propertyAlias names no stream of this store",
        segments: &[SegmentSeed {
            series: Series::Camera,
            start: (0, 0),
            end: (10, 0),
            enriched_by: None,
        }],
        jobs: &[JobSeed {
            target: Target::UnknownAlias,
            trim: ((0, 0), (10, 0)),
            cancel: false,
        }],
        dataset_status: Some("NOT_ENRICHED"),
    },
    Row {
        label: "a completed job on a dataset holding no segment at all",
        segments: &[],
        jobs: &[JobSeed {
            target: Target::DatasetOnly,
            trim: ((0, 0), (10, 0)),
            cancel: false,
        }],
        dataset_status: Some("NOT_ENRICHED"),
    },
    Row {
        label: "two completed jobs on one series report the later completion",
        segments: &[SegmentSeed {
            series: Series::Camera,
            start: (0, 0),
            end: (10, 0),
            enriched_by: Some(1),
        }],
        jobs: &[
            JobSeed {
                target: Target::Series(Series::Camera),
                trim: ((0, 0), (10, 0)),
                cancel: false,
            },
            JobSeed {
                target: Target::Alias(Series::Camera),
                trim: ((0, 0), (10, 0)),
                cancel: false,
            },
        ],
        dataset_status: Some("FULLY_ENRICHED"),
    },
];

/// The seeded streams, by the `TimeSeriesId` each one carries.
#[derive(Debug, Clone)]
struct Streams {
    camera: String,
    door: String,
}

impl Streams {
    /// The `TimeSeriesId` of `series`.
    fn id(&self, series: Series) -> &str {
        match series {
            Series::Camera => &self.camera,
            Series::Door => &self.door,
            Series::Absent => ABSENT_SERIES,
        }
    }

    /// The `propertyAlias` bound to `series`; the absent series carries none, so
    /// no row names it that way.
    fn alias(&self, series: Series) -> &'static str {
        match series {
            Series::Camera => CAMERA_ALIAS,
            Series::Door => DOOR_ALIAS,
            Series::Absent => UNKNOWN_ALIAS,
        }
    }
}

/// What one row left in the store: the dataset it created and, per job it
/// created, the `completedAt` `DescribeEnrichmentJob` reports for that job once
/// the clock has settled it.
#[derive(Debug, Clone)]
struct RowState {
    dataset_id: String,
    completed_at: Vec<Option<f64>>,
}

/// A table second as the absolute `timeInSeconds` the wire carries, anchored
/// inside the window `BatchPutAssetPropertyValue` accepts; see `super::support`.
fn at(seconds: i64) -> i64 {
    anchor() + seconds
}

/// A table instant as the stored shape.
fn time_in_nanos(stamp: Stamp) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: at(stamp.0),
        offset_in_nanos: Some(stamp.1),
    }
}

/// A table instant on the wire.
fn wire(stamp: Stamp) -> Value {
    instant(at(stamp.0), stamp.1)
}

/// Ingest a DOUBLE sample at each of `seconds` against `alias`, which CREATES
/// the data stream that alias names, and answer its `TimeSeriesId`.
async fn seed_stream(router: &MultiServiceRouter, alias: &str) -> String {
    let entries: Vec<Value> = SAMPLE_SECONDS
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
    let (status, described) = call(
        router,
        "GET",
        &format!("/timeseries/describe?alias={alias}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "describe the stream {alias}: got {status} {described}"
    );
    described["timeSeriesId"]
        .as_str()
        .unwrap_or_else(|| panic!("the stream {alias} carries a timeSeriesId, got {described}"))
        .to_string()
}

/// Create a SESSION dataset of [`WORKSPACE`] and answer its minted id.
async fn create_dataset(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": name,
            "datasetType": "SESSION",
            "workspaceName": WORKSPACE,
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "create the dataset {name}: got {status} {created}"
    );
    created["datasetId"]
        .as_str()
        .unwrap_or_else(|| panic!("a created dataset carries a datasetId, got {created}"))
        .to_string()
}

/// Seed `segments` on `dataset_id` through the producer entry point the state
/// exposes — the operations that associate or ingest a dataset's segments are
/// covered by their own suites, so this is where a dataset's segments come from
/// here.
fn seed_segments(
    engine: &Arc<engine::InMemoryEngine>,
    streams: &Streams,
    dataset_id: &str,
    segments: &[SegmentSeed],
) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for segment in segments {
        let stored = store.put_dataset_data_segment(
            dataset_id,
            DataSegmentRecord {
                source_dataset_id: dataset_id.to_string(),
                time_series_id: streams.id(segment.series).to_string(),
                start_timestamp: time_in_nanos(segment.start),
                end_timestamp: time_in_nanos(segment.end),
            },
        );
        assert!(stored, "the store must hold the seeded segment {segment:?}");
    }
}

/// The `eventDetection` configuration `seed` sends for `dataset_id`.
fn configuration(streams: &Streams, dataset_id: &str, seed: &JobSeed) -> Value {
    let mut detection = json!({
        "datasetId": dataset_id,
        "trimSettings": { "startTime": wire(seed.trim.0), "endTime": wire(seed.trim.1) },
    });
    let object = detection.as_object_mut().expect("an object configuration");
    match seed.target {
        Target::Series(series) => {
            object.insert("timeSeriesId".to_string(), json!(streams.id(series)));
        }
        Target::Alias(series) => {
            object.insert("propertyAlias".to_string(), json!(streams.alias(series)));
        }
        Target::UnknownAlias => {
            object.insert("propertyAlias".to_string(), json!(UNKNOWN_ALIAS));
        }
        Target::DatasetOnly => {}
    }
    json!({ "eventDetection": detection })
}

/// Build every row's dataset, segments and jobs, wait out the whole span the
/// clock owes those jobs, and answer what each row left behind.
async fn prepared() -> (MultiServiceRouter, Streams, Vec<RowState>) {
    let (router, engine) = router_with_engine();
    ensure_workspace(&router, WORKSPACE).await;
    let streams = Streams {
        camera: seed_stream(&router, CAMERA_ALIAS).await,
        door: seed_stream(&router, DOOR_ALIAS).await,
    };

    let mut jobs: Vec<Vec<String>> = Vec::new();
    let mut datasets: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let dataset_id = create_dataset(&router, &format!("Enrichment row {index}")).await;
        seed_segments(&engine, &streams, &dataset_id, row.segments);
        let mut row_jobs: Vec<String> = Vec::new();
        for seed in row.jobs {
            let (status, created) = create_job(
                &router,
                WORKSPACE,
                json!({ "jobConfiguration": configuration(&streams, &dataset_id, seed) }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: creating the job {seed:?} got {status} {created}",
                row.label
            );
            let job_id = created["jobId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: a created job carries a jobId, got {created}",
                        row.label
                    )
                })
                .to_string();
            if seed.cancel {
                // Cancelled straight away, while the job is still one a cancel
                // may be recorded on, so the clock never takes it to COMPLETED.
                let (status, cancelled) = call(
                    &router,
                    "POST",
                    &format!("{}/{job_id}/cancel", jobs_uri(WORKSPACE)),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: cancelling {job_id} got {status} {cancelled}",
                    row.label
                );
            }
            row_jobs.push(job_id);
        }
        datasets.push(dataset_id);
        jobs.push(row_jobs);
    }

    // Past the whole span the clock owes every job created above, so each one
    // stands in the terminal status its row expects.
    tokio::time::sleep(settled()).await;

    let mut states: Vec<RowState> = Vec::new();
    for ((row, dataset_id), row_jobs) in ROWS.iter().zip(datasets).zip(jobs) {
        let mut completed_at: Vec<Option<f64>> = Vec::new();
        for job_id in &row_jobs {
            let (status, described) = call(
                &router,
                "GET",
                &format!("{}/{job_id}", jobs_uri(WORKSPACE)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: describing {job_id} got {status} {described}",
                row.label
            );
            completed_at.push(described.get("completedAt").and_then(Value::as_f64));
        }
        states.push(RowState {
            dataset_id,
            completed_at,
        });
    }
    (router, streams, states)
}

/// `DescribeDataset`'s body for `dataset_id`.
async fn describe(router: &MultiServiceRouter, dataset_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/datasets/{dataset_id}"), None).await;
    assert_eq!(
        status, 200,
        "describe the dataset {dataset_id}: got {status} {body}"
    );
    body
}

/// The `enrichmentStatus` `ListDatasets` reports for `dataset_id`, and `None`
/// when the summary carries none.
async fn listed_enrichment(router: &MultiServiceRouter, dataset_id: &str) -> Option<Value> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets?sourceType=SITEWISE&workspaceName={WORKSPACE}&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(status, 200, "list the datasets: got {status} {body}");
    let summaries = body["datasetSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing carries datasetSummaries, got {body}"));
    let summary = summaries
        .iter()
        .find(|summary| summary["id"].as_str() == Some(dataset_id))
        .unwrap_or_else(|| panic!("the listing must report {dataset_id}, got {body}"));
    summary.get("enrichmentStatus").cloned()
}

/// The data segments `ListDatasetDataSegments` reports for `dataset_id`.
async fn listed_segments(router: &MultiServiceRouter, dataset_id: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/datasets/{dataset_id}/data-segments?workspaceName={WORKSPACE}&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list the segments of {dataset_id}: got {status} {body}"
    );
    body["dataSegments"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing carries dataSegments, got {body}"))
        .clone()
}

/// The instant the row expects on `segment`, as the `completedAt` of the job
/// that enriched it.
fn expected_instant(state: &RowState, segment: &SegmentSeed) -> Option<f64> {
    let job = segment.enriched_by?;
    Some(
        state.completed_at[job]
            .unwrap_or_else(|| panic!("the job {job} that enriched a segment must have completed")),
    )
}

#[tokio::test]
async fn a_dataset_reports_the_enrichment_its_jobs_derive() {
    let (router, _streams, states) = prepared().await;

    for (row, state) in ROWS.iter().zip(&states) {
        let described = describe(&router, &state.dataset_id).await;
        let reported = described.get("enrichmentStatus").cloned();

        match row.dataset_status {
            None => assert!(
                reported.is_none() || reported == Some(Value::Null),
                "case {:?}: a dataset no enrichment job names reports no enrichmentStatus, \
                 got {described}",
                row.label
            ),
            Some(wanted) => {
                assert!(
                    DATASET_ENRICHMENT_STATUSES.contains(&wanted),
                    "case {:?}: the row's own expectation {wanted} must be a modelled \
                     DatasetEnrichmentStatus",
                    row.label
                );
                let entry = reported
                    .as_ref()
                    .map(|status| status["video"].clone())
                    .unwrap_or(Value::Null);
                assert_eq!(
                    entry["status"].as_str(),
                    Some(wanted),
                    "case {:?}: expected the video entry {wanted}, got {described}",
                    row.label
                );
                // The latest instant of the segments the row says are enriched,
                // which is "the date the data was last enriched".
                let wanted_instant = row
                    .segments
                    .iter()
                    .filter_map(|segment| expected_instant(state, segment))
                    .fold(None::<f64>, |latest, instant| {
                        Some(latest.map_or(instant, |held: f64| held.max(instant)))
                    });
                assert_eq!(
                    entry.get("lastEnrichedAt").and_then(Value::as_f64),
                    wanted_instant,
                    "case {:?}: expected lastEnrichedAt {wanted_instant:?}, got {described}",
                    row.label
                );
            }
        }

        // The listing projects the same derivation, so a listed dataset and a
        // described one cannot disagree about its enrichment.
        let listed = listed_enrichment(&router, &state.dataset_id).await;
        assert_eq!(
            listed, reported,
            "case {:?}: ListDatasets reported {listed:?} where DescribeDataset reported \
             {reported:?}",
            row.label
        );

        // A read REPORTS: the second answer is the first.
        let again = describe(&router, &state.dataset_id).await;
        assert_eq!(
            again, described,
            "case {:?}: the second DescribeDataset answered differently: {again}",
            row.label
        );
    }

    assert_eq!(
        states.len(),
        ROWS.len(),
        "every row must have built a dataset, got {states:?}"
    );
}

#[tokio::test]
async fn every_data_segment_reports_whether_a_completed_job_analysed_it() {
    let (router, streams, states) = prepared().await;

    for (row, state) in ROWS.iter().zip(&states) {
        let listed = listed_segments(&router, &state.dataset_id).await;
        assert_eq!(
            listed.len(),
            row.segments.len(),
            "case {:?}: the listing must report every seeded segment once, got {listed:?}",
            row.label
        );

        for segment in row.segments {
            let wanted_series = streams.id(segment.series);
            let wanted_start = at(segment.start.0);
            let reported = listed
                .iter()
                .find(|summary| {
                    summary["timeSeriesId"].as_str() == Some(wanted_series)
                        && summary["startTimestamp"]["timeInSeconds"].as_i64()
                            == Some(wanted_start)
                        && summary["startTimestamp"]["offsetInNanos"].as_i64()
                            == Some(i64::from(segment.start.1))
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the listing must report the segment {segment:?}, \
                         got {listed:?}"
                        ,
                        row.label
                    )
                });
            let enrichment = reported.get("enrichment").cloned();

            if row.jobs.is_empty() {
                assert!(
                    enrichment.is_none() || enrichment == Some(Value::Null),
                    "case {:?}: a segment of a dataset no job names reports no enrichment, \
                     got {reported}",
                    row.label
                );
                continue;
            }

            let wanted_instant = expected_instant(state, segment);
            let wanted_status = match wanted_instant {
                Some(_) => "ENRICHED",
                None => "NOT_ENRICHED",
            };
            assert!(
                SEGMENT_ENRICHMENT_STATUSES.contains(&wanted_status),
                "case {:?}: {wanted_status} must be a modelled EnrichmentStatus",
                row.label
            );
            let enrichment = enrichment.unwrap_or_else(|| {
                panic!(
                    "case {:?}: a segment of a dataset a job names reports an enrichment, \
                     got {reported}",
                    row.label
                )
            });
            assert_eq!(
                enrichment["status"].as_str(),
                Some(wanted_status),
                "case {:?}: segment {segment:?} expected {wanted_status}, got {reported}",
                row.label
            );
            assert_eq!(
                enrichment.get("lastEnrichedAt").and_then(Value::as_f64),
                wanted_instant,
                "case {:?}: segment {segment:?} expected lastEnrichedAt {wanted_instant:?}, \
                 got {reported}",
                row.label
            );
        }
    }
}
