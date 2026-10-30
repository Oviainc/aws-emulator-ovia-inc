// SPDX-License-Identifier: CC0-1.0

//! `DeleteTimeSeries` against a stream a dataset still holds a data segment of,
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a time series may not be deleted while a data
//! segment covering it is still held, so the delete is REFUSED — as the terminal
//! request error, naming the dataset that holds one — and removes nothing until
//! every such segment is gone, whichever dataset holds it, whichever dataset its
//! data came from, however the request identifies the stream and whichever scope
//! it addresses it in.
//!
//! The table spans what the rule generalises over: the dataset holding the
//! segment (a session dataset's own data, a curated dataset's association of it,
//! and both at once), one and three segments, the four forms a request identifies
//! a stream by (the alias of a stream no property owns, the alias of one bound to
//! an asset property, the `assetId`/`propertyId` pair, and a workspace-scoped
//! alias), and the control rows a segment must NOT stop: a stream nothing holds a
//! segment of, and one where the held segment covers another stream.
//!
//! The expected refusal message is written out BY HAND from the rule, as an
//! independent oracle: the suite never calls the handler's own wording.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`, the asset
//! helpers) and the dataset helpers (`create_dataset`, `ingest`, `instant`,
//! `time_series_id`, `wire_instant`, `at`) are the ones the asset and data-segment
//! suites already define; both are reused rather than copied.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, CaptureSegment, DataSegmentRecord,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router_with_engine,
};
use crate::dataset_data_segments::{
    at, create_dataset, ingest, instant, time_series_id, wire_instant,
};
use crate::hierarchies_etags_and_refusals::error_message;

/// The workspace this suite's datasets and workspace-scoped stream live in.
const WORKSPACE: &str = "kiln-floor";

/// The alias of the stream no asset property owns.
const LOOSE_ALIAS: &str = "/plant/kiln/1/temperature";

/// The alias BOUND to an asset property, so a row deletes an ASSOCIATED stream
/// through its alias.
const BOUND_ALIAS: &str = "/plant/kiln/2/temperature";

/// A stream no row ever deletes: the control rows hold segments of THIS series, so
/// a delete refused for someone else's segment cannot pass.
const SPARE_ALIAS: &str = "/plant/kiln/3/temperature";

/// The `TimeSeriesId` of the workspace's own stream — a capture source, which is
/// what a workspace-scoped delete addresses. 36 characters, the shortest the model
/// admits (`smithy_query(query="shape", shape="TimeSeriesId")` — length 36..73).
const WORKSPACE_SERIES: &str = "3c7d9e51-4a28-4b63-9f10-6d8e2b5a4c71";

/// The alias that workspace stream stands under.
const WORKSPACE_ALIAS: &str = "/plant/kiln/4/camera";

/// The `VideoDataType` the seeded capture is in. Source:
/// `smithy_query(query="shape", shape="VideoDataType")` — the enum holds exactly
/// this one value.
const MEDIA_TYPE: &str = "VIDEO-MP4";

/// The sample seconds, relative to the suite's anchor, each alias-addressed stream
/// is ingested with — enough to create the stream a row then deletes.
const SAMPLES: &[i64] = &[0, 5, 10, 15];

/// How a row's request identifies the stream it deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// `alias=`, naming the stream no asset property owns.
    LooseAlias,
    /// `alias=`, naming the stream bound to an asset property.
    BoundAlias,
    /// `assetId=&propertyId=`: the stream the asset property itself owns.
    Pair,
    /// `workspaceName=&alias=`: a stream one workspace holds.
    WorkspaceAlias,
}

/// Which dataset holds a seeded segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dataset {
    /// The SESSION dataset, holding its OWN data (source == holder) — what the
    /// model's "data segments ... in session datasets" names.
    Session,
    /// The CURATED dataset, holding an association selected FROM the session
    /// dataset (source != holder).
    Curated,
}

/// Which stream a seeded segment covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Covers {
    /// The stream the row's delete addresses.
    Deleted,
    /// [`SPARE_ALIAS`]' stream, which no row deletes.
    Spare,
}

/// One data segment a row seeds: who holds it, whose data it covers, and its
/// half-open span in table seconds.
#[derive(Debug, Clone, Copy)]
struct Seed {
    holder: Dataset,
    covers: Covers,
    start: i64,
    end: i64,
}

/// One row of the table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    address: Address,
    /// The segments held when the delete is called.
    seeds: &'static [Seed],
    /// Whether the delete is refused for them.
    refused: bool,
}

fn table() -> Vec<Row> {
    vec![
        Row {
            case: "a loose alias, the session dataset's own segment",
            address: Address::LooseAlias,
            seeds: &[Seed {
                holder: Dataset::Session,
                covers: Covers::Deleted,
                start: 0,
                end: 10,
            }],
            refused: true,
        },
        Row {
            case: "a loose alias, a curated dataset's association",
            address: Address::LooseAlias,
            seeds: &[Seed {
                holder: Dataset::Curated,
                covers: Covers::Deleted,
                start: 10,
                end: 20,
            }],
            refused: true,
        },
        Row {
            case: "a bound alias, three of the session dataset's segments",
            address: Address::BoundAlias,
            seeds: &[
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Deleted,
                    start: 0,
                    end: 5,
                },
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Deleted,
                    start: 5,
                    end: 10,
                },
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Deleted,
                    start: 100,
                    end: 160,
                },
            ],
            refused: true,
        },
        Row {
            case: "the asset-property pair, a curated dataset's association",
            address: Address::Pair,
            seeds: &[Seed {
                holder: Dataset::Curated,
                covers: Covers::Deleted,
                start: 0,
                end: 10,
            }],
            refused: true,
        },
        Row {
            case: "the asset-property pair, a segment in each dataset",
            address: Address::Pair,
            seeds: &[
                Seed {
                    holder: Dataset::Session,
                    covers: Covers::Deleted,
                    start: 0,
                    end: 10,
                },
                Seed {
                    holder: Dataset::Curated,
                    covers: Covers::Deleted,
                    start: 10,
                    end: 20,
                },
            ],
            refused: true,
        },
        Row {
            case: "a workspace-scoped stream, the session dataset's own segment",
            address: Address::WorkspaceAlias,
            seeds: &[Seed {
                holder: Dataset::Session,
                covers: Covers::Deleted,
                start: 0,
                end: 10,
            }],
            refused: true,
        },
        Row {
            case: "a loose alias, no segment anywhere",
            address: Address::LooseAlias,
            seeds: &[],
            refused: false,
        },
        Row {
            case: "a bound alias, a session segment of another stream",
            address: Address::BoundAlias,
            seeds: &[Seed {
                holder: Dataset::Session,
                covers: Covers::Spare,
                start: 0,
                end: 10,
            }],
            refused: false,
        },
        Row {
            case: "the asset-property pair, a curated association of another stream",
            address: Address::Pair,
            seeds: &[Seed {
                holder: Dataset::Curated,
                covers: Covers::Spare,
                start: 0,
                end: 10,
            }],
            refused: false,
        },
        Row {
            case: "a workspace-scoped stream, a session segment of another stream",
            address: Address::WorkspaceAlias,
            seeds: &[Seed {
                holder: Dataset::Session,
                covers: Covers::Spare,
                start: 10,
                end: 20,
            }],
            refused: false,
        },
    ]
}

/// The datasets, asset and data streams every row starts from.
#[derive(Debug)]
struct Fixture {
    engine: Arc<engine::InMemoryEngine>,
    /// The SESSION dataset: the source a curated association selects from, and
    /// the holder of its own ingested segments.
    session: String,
    /// The CURATED dataset: the holder of associations.
    curated: String,
    asset_id: String,
    /// The property whose OWN stream the pair form addresses.
    pair_property: String,
}

impl Fixture {
    /// The `datasetId` `holder` names.
    fn dataset(&self, holder: Dataset) -> &str {
        match holder {
            Dataset::Session => &self.session,
            Dataset::Curated => &self.curated,
        }
    }

    /// The query a `DeleteTimeSeries` or `DescribeTimeSeries` request carries to
    /// identify the stream `address` names.
    fn query(&self, address: Address) -> String {
        match address {
            Address::LooseAlias => format!("alias={LOOSE_ALIAS}"),
            Address::BoundAlias => format!("alias={BOUND_ALIAS}"),
            Address::Pair => format!(
                "assetId={}&propertyId={}",
                self.asset_id, self.pair_property
            ),
            Address::WorkspaceAlias => {
                format!("workspaceName={WORKSPACE}&alias={WORKSPACE_ALIAS}")
            }
        }
    }
}

/// A router over a fresh engine carrying: a session and a curated dataset in
/// [`WORKSPACE`], three account-scoped data streams (one loose, one bound to an
/// asset property, one spare), an asset property owning a stream of its own, and
/// one workspace-scoped stream.
async fn fixture() -> (MultiServiceRouter, Fixture) {
    let (router, engine) = router_with_engine();
    let session = create_dataset(&router, "Kiln session", Some("SESSION"), Some(WORKSPACE)).await;
    let curated = create_dataset(&router, "Kiln curated", Some("CURATED"), Some(WORKSPACE)).await;

    ingest(&router, LOOSE_ALIAS, SAMPLES).await;
    ingest(&router, BOUND_ALIAS, SAMPLES).await;
    ingest(&router, SPARE_ALIAS, SAMPLES).await;

    let model_id = create_model(
        &router,
        "Kiln",
        json!([
            {"name": "Force", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Pressure", "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(&router, "Kiln1", &model_id).await;
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    let bound_property = property_id(&asset["assetProperties"], "Force");
    let pair_property = property_id(&asset["assetProperties"], "Pressure");

    // The bound stream: the alias-addressed stream becomes the property's.
    let (status, body) = call(
        &router,
        "POST",
        &format!(
            "/timeseries/associate?alias={BOUND_ALIAS}&assetId={asset_id}\
             &propertyId={bound_property}"
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "associate {BOUND_ALIAS}: got {status} {body}");

    // The pair-addressed stream: the samples the property itself owns.
    ingest_pair(&router, &asset_id, &pair_property, SAMPLES).await;

    // The workspace's own stream, seeded through the state's producer entry point
    // — the one writer of a workspace's capture data, which no implemented
    // operation of this service feeds yet.
    {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        let stored = store.put_capture_segment(
            WORKSPACE,
            WORKSPACE_SERIES,
            Some(WORKSPACE_ALIAS),
            MEDIA_TYPE,
            CaptureSegment {
                start_time: instant(0, 0),
                end_time: instant(20, 0),
                data: b"mp4:kiln".to_vec(),
            },
            std::time::SystemTime::now(),
        );
        assert_eq!(
            stored,
            Ok(()),
            "seeding the workspace stream must be accepted"
        );
    }

    (
        router,
        Fixture {
            engine,
            session,
            curated,
            asset_id,
            pair_property,
        },
    )
}

/// Ingest a DOUBLE sample at each of `seconds` against the `(assetId, propertyId)`
/// pair, so the property owns a stream of its own.
///
/// Shared with the asset-delete suite (`asset_delete_data_segments`), which holds
/// segments of the same pair-derived streams, rather than growing a second copy.
pub(crate) async fn ingest_pair(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    seconds: &[i64],
) {
    let entries: Vec<Value> = seconds
        .iter()
        .map(|second| {
            json!({
                "entryId": format!("p{second}"),
                "assetId": asset_id,
                "propertyId": property,
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
    assert_eq!(status, 200, "ingest the pair: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "ingest the pair: every entry must be accepted, got {body}"
    );
}

/// `GET /timeseries/describe?<query>`.
async fn describe(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/timeseries/describe?{query}"), None).await
}

/// `POST /timeseries/delete?<query>`.
async fn delete_time_series(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/timeseries/delete?{query}"),
        Some(json!({})),
    )
    .await
}

/// The `timeSeriesId` the stream `address` names carries.
async fn series_of(router: &MultiServiceRouter, fixture: &Fixture, address: Address) -> String {
    let query = fixture.query(address);
    let (status, body) = describe(router, &query).await;
    assert_eq!(status, 200, "describe {query}: got {status} {body}");
    body["timeSeriesId"]
        .as_str()
        .unwrap_or_else(|| panic!("describe {query}: no timeSeriesId in {body}"))
        .to_string()
}

/// `POST /datasets/{datasetId}/data-segments/associate` with one entry, asserting
/// the association was applied.
async fn associate(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    series: &str,
    start: i64,
    end: i64,
) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/datasets/{}/data-segments/associate", fixture.curated),
        Some(json!({
            "workspaceName": WORKSPACE,
            "associateDataSegmentEntries": [{
                "sourceDatasetId": fixture.session,
                "timeSeriesId": series,
                "startTimestamp": wire_instant(start, 0),
                "endTimestamp": wire_instant(end, 0),
            }],
        })),
    )
    .await;
    assert_eq!(status, 200, "associate {series}: got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "associate {series}: the entry must be applied, got {body}"
    );
}

/// Seed one segment of `series` as the session dataset's OWN data, through the
/// state's producer entry point — a session dataset's own segments are its
/// ingested data, which no implemented operation of this service writes yet.
fn seed_own_segment(fixture: &Fixture, series: &str, start: i64, end: i64) {
    let state = service_state(&*fixture.engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    let stored = store.put_dataset_data_segment(
        &fixture.session,
        DataSegmentRecord {
            source_dataset_id: fixture.session.clone(),
            time_series_id: series.to_string(),
            start_timestamp: instant(start, 0),
            end_timestamp: instant(end, 0),
        },
    );
    assert!(
        stored,
        "seeding the segment [{start}, {end}) of {series} must be accepted"
    );
}

/// How many data segments this store holds of `series`, across every dataset.
///
/// Shared with the asset-delete suite, for the reason [`ingest_pair`] is.
pub(crate) fn segments_of(engine: &Arc<engine::InMemoryEngine>, series: &str) -> usize {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .dataset_data_segments
        .values()
        .flatten()
        .filter(|segment| segment.time_series_id == series)
        .count()
}

/// The refusal the rule produces, written out by hand: the stream that cannot go
/// and the dataset that still holds a segment of it.
///
/// One hand-written oracle for every call the rule refuses, shared with the
/// asset-delete suite: the suites never call the handlers' own wording.
pub(crate) fn expected_message(series: &str, holder: &str) -> String {
    format!(
        "The time series {series} has data segments in the dataset {holder}. \
         Delete them before deleting it."
    )
}

/// Assert the stream `address` named is gone.
///
/// A stream an ALIAS named resolves to nothing afterwards. The `(assetId,
/// propertyId)` pair is different and the operation says why: "the asset property
/// still exists, but the time series will no longer be associated with this asset
/// property", so the pair still describes the property's own derived stream and
/// what went is the stream's data.
async fn assert_stream_gone(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    address: Address,
    case: &str,
) {
    let query = fixture.query(address);
    if address != Address::Pair {
        let (status, gone) = describe(router, &query).await;
        assert_eq!(
            (status, error_code(&gone)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the deleted stream must resolve to nothing, got {status} {gone}"
        );
        return;
    }
    let (status, history) = call(
        router,
        "GET",
        &format!("/properties/history?{query}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the surviving property must still read, got {status} {history}"
    );
    assert_eq!(
        history["assetPropertyValueHistory"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: the deleted stream's samples must be gone, got {history}"
    );
}

#[tokio::test]
async fn a_delete_is_refused_while_any_dataset_holds_a_data_segment_of_the_stream() {
    for row in table() {
        let case = row.case;
        let (router, fixture) = fixture().await;
        let series = series_of(&router, &fixture, row.address).await;
        let spare = time_series_id(&router, SPARE_ALIAS).await;

        // The row's segments, each covering the stream it names and held by the
        // dataset it names.
        for seed in row.seeds {
            let covered = match seed.covers {
                Covers::Deleted => series.as_str(),
                Covers::Spare => spare.as_str(),
            };
            match seed.holder {
                Dataset::Session => seed_own_segment(&fixture, covered, seed.start, seed.end),
                Dataset::Curated => {
                    associate(&router, &fixture, covered, seed.start, seed.end).await
                }
            }
        }
        let held_before = segments_of(&fixture.engine, &series);

        let query = fixture.query(row.address);
        let (status, body) = delete_time_series(&router, &query).await;

        if !row.refused {
            assert_eq!(
                status, 200,
                "case {case:?}: nothing holds a segment of this stream, so the delete stands; \
                 got {status} {body}"
            );
            assert_stream_gone(&router, &fixture, row.address, case).await;
            // The segment that covered ANOTHER stream is untouched by this delete.
            assert_eq!(
                segments_of(&fixture.engine, &spare),
                row.seeds.len(),
                "case {case:?}: a delete must leave another stream's segments alone"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        // Any dataset still holding a segment of the stream is a reason to refuse,
        // so the message names one of them.
        let holders: Vec<String> = row
            .seeds
            .iter()
            .filter(|seed| seed.covers == Covers::Deleted)
            .map(|seed| fixture.dataset(seed.holder).to_string())
            .collect();
        let candidates: Vec<String> = holders
            .iter()
            .map(|holder| expected_message(&series, holder))
            .collect();
        assert!(
            candidates.iter().any(|expected| expected == error_message(&body)),
            "case {case:?}: expected one of {candidates:?}, got {:?}",
            error_message(&body)
        );

        // A refused delete removed nothing: the stream still resolves to itself,
        // and every segment of it is still held.
        let (status, described) = describe(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the stream in place, got {status} \
             {described}"
        );
        assert_eq!(
            described["timeSeriesId"].as_str(),
            Some(series.as_str()),
            "case {case:?}: the stream must still be the one addressed, got {described}"
        );
        assert_eq!(
            segments_of(&fixture.engine, &series),
            held_before,
            "case {case:?}: a refused delete must leave every data segment held"
        );
    }
}

#[tokio::test]
async fn the_delete_goes_through_once_the_last_data_segment_is_gone() {
    let (router, fixture) = fixture().await;
    let series = series_of(&router, &fixture, Address::LooseAlias).await;
    let query = fixture.query(Address::LooseAlias);

    // Two segments of one stream: the session dataset's own early span, and the
    // curated dataset's association of the next one.
    seed_own_segment(&fixture, &series, 0, 10);
    associate(&router, &fixture, &series, 10, 20).await;

    let (status, body) = delete_time_series(&router, &query).await;
    assert_eq!(status, 400, "two segments held: got {status} {body}");
    let candidates = [
        expected_message(&series, &fixture.session),
        expected_message(&series, &fixture.curated),
    ];
    assert!(
        candidates.iter().any(|expected| expected == error_message(&body)),
        "two segments held: expected one of {candidates:?}, got {:?}",
        error_message(&body)
    );

    // Dropping the association is not enough: the session dataset's own segment
    // still covers the stream, and the refusal now names that dataset.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/datasets/{}/data-segments/disassociate", fixture.curated),
        Some(json!({
            "workspaceName": WORKSPACE,
            "disassociateDataSegmentEntries": [{
                "sourceDatasetId": fixture.session,
                "timeSeriesId": series,
                "startTimestamp": wire_instant(10, 0),
                "endTimestamp": wire_instant(20, 0),
            }],
        })),
    )
    .await;
    assert_eq!(status, 200, "disassociate: got {status} {body}");
    assert_eq!(
        body["failedDisassociations"].as_array().map(Vec::len),
        Some(0),
        "disassociate: the entry must be applied, got {body}"
    );

    let (status, body) = delete_time_series(&router, &query).await;
    assert_eq!(status, 400, "one segment left: got {status} {body}");
    assert_eq!(
        error_message(&body),
        expected_message(&series, &fixture.session),
        "one segment left: the refusal must name the dataset holding it, got {body}"
    );

    // With the last segment deleted the stream has nothing depending on it, and the
    // delete stands.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/datasets/{}/data-segments/batch-delete", fixture.session),
        Some(json!({
            "workspaceName": WORKSPACE,
            "deleteDataSegmentEntries": [{
                "timeSeriesId": series,
                "startTimestamp": wire_instant(0, 0),
                "endTimestamp": wire_instant(10, 0),
            }],
        })),
    )
    .await;
    assert_eq!(status, 200, "batch-delete: got {status} {body}");
    assert_eq!(
        body["errors"].as_array().map(Vec::len),
        Some(0),
        "batch-delete: the entry must be applied, got {body}"
    );
    assert_eq!(
        segments_of(&fixture.engine, &series),
        0,
        "no segment of the stream is left to hold the delete up"
    );

    let (status, body) = delete_time_series(&router, &query).await;
    assert_eq!(status, 200, "the last segment gone: got {status} {body}");
    let (status, gone) = describe(&router, &query).await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "the deleted stream must resolve to nothing, got {status} {gone}"
    );

    // And no dataset is left listing a segment that names a stream the store no
    // longer holds.
    for dataset in [&fixture.session, &fixture.curated] {
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/datasets/{dataset}/data-segments?workspaceName={WORKSPACE}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "list {dataset}: got {status} {listed}");
        let dangling = listed["dataSegments"]
            .as_array()
            .unwrap_or_else(|| panic!("a listing carries dataSegments, got {listed}"))
            .iter()
            .filter(|summary| summary["timeSeriesId"].as_str() == Some(series.as_str()))
            .count();
        assert_eq!(
            dangling, 0,
            "list {dataset}: no segment may name the deleted stream, got {listed}"
        );
    }
}
