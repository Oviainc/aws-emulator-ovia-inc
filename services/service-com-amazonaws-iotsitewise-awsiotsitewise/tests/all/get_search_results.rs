// SPDX-License-Identifier: CC0-1.0

//! `GetSearchResults` end to end, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a result is a MATCHING data segment of a dataset of
//! the search's own workspace — so the page is computed, on every call, from what
//! the store holds (the search's natural-language statement and filters, and the
//! segments the workspace's datasets hold), ordered by descending relevance, and a
//! segment the statement matches nothing of is not a result at all. Relevance here
//! is lexical: the fraction of the statement's distinct terms the stored text about
//! a segment carries (its dataset's name, description, type and metadata, plus the
//! `TimeSeriesId` it covers), which is what this emulator can compute about data
//! whose content it does not hold.
//!
//! The rows of the table below differ along what that rule generalises over: how
//! many of the statement's terms match, which stored text they match (a name, a
//! description, a type, metadata, a time series id), the case and punctuation they
//! are written in, which filter the search carried (datasets, time series, time
//! intervals, two at once), and which workspace the search runs in. The sibling
//! suites pin the refusals (a search that has not succeeded, a pair no search is
//! stored under), the paging — including that a `nextToken` names a page only of
//! the search it was issued for — and that reading results writes nothing.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `is_modelled_id`,
//! `router_with_engine`) is the asset suite's and the `StartSearch` /
//! `CreateWorkspace` wrappers are the search suite's; this suite reuses both rather
//! than growing a third copy.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, DataSegmentRecord, RESOURCE_SEARCH, RESOURCE_WORKSPACE,
    SEARCH_QUEUED, SEARCH_QUEUED_FOR, SEARCH_RUNS_FOR, SEARCH_SUCCEEDED,
};

use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router_with_engine};
use crate::search_lifecycle::{create_workspace, start};

/// The workspace most searches below run in.
const WORKSPACE: &str = "search-plant";

/// A second workspace, so the table spans more than one search scope and can prove
/// a search reads only the datasets of its OWN workspace.
const OTHER_WORKSPACE: &str = "search-plant_2";

/// The second the seeded spans are measured from, so a span reads as a small
/// offset. Any instant the modelled `TimeInNanos` can express will do: a data
/// segment is seeded directly, not ingested, so no acceptance window applies.
const EPOCH: i64 = 1_700_000_000;

/// How long past the owed `QUEUED` -> `RUNNING` -> `SUCCEEDED` transitions a suite
/// waits before reading results, so a slow run cannot read a search that has not
/// reached the status results need.
const SETTLED: Duration = Duration::from_millis(400);

/// The `TimeSeriesId` of the first seeded series. Its terms (the hex runs between
/// the hyphens) are what a statement naming the series matches on, so the two
/// seeded ids share none.
const VIBRATION_SERIES: &str = "11111111-aaaa-4bbb-8ccc-111111111111";

/// The `TimeSeriesId` of the second seeded series; see [`VIBRATION_SERIES`].
const TEMPERATURE_SERIES: &str = "22222222-dddd-4eee-8fff-222222222222";

/// Which seeded dataset holds a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dataset {
    /// A dataset of [`WORKSPACE`] whose text is about a compressor, and the only
    /// seeded dataset carrying `metadata`.
    Press,
    /// A second dataset of [`WORKSPACE`], whose text is about a boiler instead.
    Boiler,
    /// A dataset of [`OTHER_WORKSPACE`] carrying the SAME text as [`Self::Press`],
    /// so a row separates workspace scoping from text matching.
    Neighbour,
    /// A dataset no workspace contains (an EXTERNAL one), carrying that same text
    /// again.
    Unscoped,
}

/// Which seeded time series a segment covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Series {
    Vibration,
    Temperature,
}

/// The `TimeSeriesId` `series` is seeded under.
fn series_id(series: Series) -> &'static str {
    match series {
        Series::Vibration => VIBRATION_SERIES,
        Series::Temperature => TEMPERATURE_SERIES,
    }
}

/// One seeded dataset: the text a caller gave it and the workspace it was created
/// in, if any.
#[derive(Debug, Clone, Copy)]
struct Catalogued {
    dataset: Dataset,
    name: &'static str,
    description: &'static str,
    /// The `DatasetTypeEnum` the create named. Source: `smithy_query(query="shape",
    /// shape="DatasetTypeEnum")` — SESSION | CURATED | EXTERNAL — of which only an
    /// EXTERNAL dataset may live outside a workspace ("omit this field for datasets
    /// that connect to an external datasource").
    dataset_type: &'static str,
    /// The `metadata` entries the create carried.
    metadata: &'static [(&'static str, &'static str)],
    workspace: Option<&'static str>,
}

/// The datasets every suite below starts from.
const CATALOGUE: &[Catalogued] = &[
    Catalogued {
        dataset: Dataset::Press,
        name: "Compressor vibration session",
        description: "Vibration readings from the main compressor",
        dataset_type: "SESSION",
        metadata: &[("line", "press"), ("unit", "mm per second")],
        workspace: Some(WORKSPACE),
    },
    Catalogued {
        dataset: Dataset::Boiler,
        name: "Boiler temperature session",
        description: "Temperature of the boiler loop",
        dataset_type: "SESSION",
        metadata: &[],
        workspace: Some(WORKSPACE),
    },
    Catalogued {
        dataset: Dataset::Neighbour,
        name: "Compressor vibration session",
        description: "Vibration readings from the main compressor",
        dataset_type: "SESSION",
        metadata: &[],
        workspace: Some(OTHER_WORKSPACE),
    },
    Catalogued {
        dataset: Dataset::Unscoped,
        name: "Compressor vibration session",
        description: "Vibration readings from the main compressor",
        // A dataset outside a workspace can only be an EXTERNAL one; see
        // [`Catalogued::dataset_type`].
        dataset_type: "EXTERNAL",
        metadata: &[],
        workspace: None,
    },
];

/// One seeded data segment: the dataset that holds it, the series it covers and the
/// span it covers of it, in seconds relative to [`EPOCH`] plus a nanosecond offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment {
    dataset: Dataset,
    series: Series,
    start: (i64, i32),
    end: (i64, i32),
}

/// The first series' earlier segment of the compressor dataset.
const PRESS_VIBRATION_EARLY: Segment = Segment {
    dataset: Dataset::Press,
    series: Series::Vibration,
    start: (1_000, 0),
    end: (2_000, 0),
};

/// The same series' later segment, whose end carries a nanosecond offset — so the
/// reported `topTimestamp` has to be computed at nanosecond precision.
const PRESS_VIBRATION_LATE: Segment = Segment {
    dataset: Dataset::Press,
    series: Series::Vibration,
    start: (3_000, 0),
    end: (4_000, 500_000_000),
};

/// The second series' segment of that same dataset, at the span
/// [`PRESS_VIBRATION_EARLY`] also covers.
const PRESS_TEMPERATURE: Segment = Segment {
    dataset: Dataset::Press,
    series: Series::Temperature,
    start: (1_000, 0),
    end: (2_000, 0),
};

/// The other workspace-scoped dataset's only segment, at a later span.
const BOILER_TEMPERATURE: Segment = Segment {
    dataset: Dataset::Boiler,
    series: Series::Temperature,
    start: (5_000, 0),
    end: (6_000, 0),
};

/// A segment of the dataset in the OTHER workspace.
const NEIGHBOUR_VIBRATION: Segment = Segment {
    dataset: Dataset::Neighbour,
    series: Series::Vibration,
    start: (1_000, 0),
    end: (2_000, 0),
};

/// A segment of the dataset no workspace contains.
const UNSCOPED_VIBRATION: Segment = Segment {
    dataset: Dataset::Unscoped,
    series: Series::Vibration,
    start: (1_000, 0),
    end: (2_000, 0),
};

/// Every segment the suites seed.
const SEEDED: &[Segment] = &[
    PRESS_VIBRATION_EARLY,
    PRESS_VIBRATION_LATE,
    PRESS_TEMPERATURE,
    BOILER_TEMPERATURE,
    NEIGHBOUR_VIBRATION,
    UNSCOPED_VIBRATION,
];

/// The minted `datasetId` of every seeded dataset.
type Catalog = Vec<(Dataset, String)>;

/// The id `dataset` was created under.
fn id(catalog: &Catalog, dataset: Dataset) -> &str {
    catalog
        .iter()
        .find(|(seeded, _)| *seeded == dataset)
        .map(|(_, id)| id.as_str())
        .unwrap_or_else(|| panic!("no seeded dataset for {dataset:?}"))
}

/// A seeded instant as the modelled `TimeInNanos`.
fn instant(at: (i64, i32)) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: EPOCH + at.0,
        offset_in_nanos: Some(at.1),
    }
}

/// The same, on the wire.
fn wire_instant(at: (i64, i32)) -> Value {
    json!({ "timeInSeconds": EPOCH + at.0, "offsetInNanos": at.1 })
}

/// The instant an answered result must report as its `topTimestamp`: the middle of
/// the span the segment covers, in the seconds-and-offset form the table is written
/// in.
///
/// Hand-rolled as an INDEPENDENT oracle rather than read back off the handler, so a
/// midpoint computed wrongly cannot agree with itself and pass.
fn middle(segment: &Segment) -> (i64, i32) {
    let nanos = |at: (i64, i32)| i128::from(at.0) * 1_000_000_000 + i128::from(at.1);
    let start = nanos(segment.start);
    let middle = start + (nanos(segment.end) - start) / 2;
    (
        (middle / 1_000_000_000) as i64,
        (middle % 1_000_000_000) as i32,
    )
}

/// One row of the matching table: the search to start and the results, in order,
/// it must answer.
#[derive(Debug)]
struct Row {
    case: &'static str,
    workspace: &'static str,
    /// The `queryStatement` the search is started with.
    query: &'static str,
    /// The `searchFilters` the start carries, when the row is about one.
    filters: Option<Value>,
    /// The results the search must answer, in the order it must answer them, each
    /// with the relevance score it must carry.
    expected: Vec<(Segment, f64)>,
}

/// Two thirds, as a statement of three terms two of which match scores.
const TWO_OF_THREE: f64 = 2.0 / 3.0;

/// One third; see [`TWO_OF_THREE`].
const ONE_OF_THREE: f64 = 1.0 / 3.0;

/// The matching table. `catalog` is needed because the rows about the `datasetIds`
/// filter name a dataset by the id the create minted for it.
fn table(catalog: &Catalog) -> Vec<Row> {
    vec![
        Row {
            case: "every term of the statement in the dataset's own text",
            workspace: WORKSPACE,
            query: "compressor vibration",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, 1.0),
                (PRESS_VIBRATION_LATE, 1.0),
                (PRESS_TEMPERATURE, 1.0),
            ],
        },
        Row {
            case: "a term only the other dataset's text carries",
            workspace: WORKSPACE,
            query: "boiler",
            filters: None,
            expected: vec![(BOILER_TEMPERATURE, 1.0)],
        },
        Row {
            case: "a statement half of which matches each dataset",
            workspace: WORKSPACE,
            query: "compressor boiler",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, 0.5),
                (PRESS_VIBRATION_LATE, 0.5),
                (PRESS_TEMPERATURE, 0.5),
                (BOILER_TEMPERATURE, 0.5),
            ],
        },
        Row {
            case: "a stronger match ranks above a weaker one",
            workspace: WORKSPACE,
            query: "compressor vibration boiler",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, TWO_OF_THREE),
                (PRESS_VIBRATION_LATE, TWO_OF_THREE),
                (PRESS_TEMPERATURE, TWO_OF_THREE),
                (BOILER_TEMPERATURE, ONE_OF_THREE),
            ],
        },
        Row {
            case: "terms carried by the dataset's metadata",
            workspace: WORKSPACE,
            query: "press mm",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, 1.0),
                (PRESS_VIBRATION_LATE, 1.0),
                (PRESS_TEMPERATURE, 1.0),
            ],
        },
        Row {
            case: "a term carried by the dataset's type alone",
            workspace: WORKSPACE,
            query: "session",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, 1.0),
                (PRESS_VIBRATION_LATE, 1.0),
                (PRESS_TEMPERATURE, 1.0),
                (BOILER_TEMPERATURE, 1.0),
            ],
        },
        Row {
            case: "the statement's case and punctuation do not matter",
            workspace: WORKSPACE,
            query: "COMPRESSOR, vibration!",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, 1.0),
                (PRESS_VIBRATION_LATE, 1.0),
                (PRESS_TEMPERATURE, 1.0),
            ],
        },
        Row {
            case: "a statement naming one series' own id",
            workspace: WORKSPACE,
            query: VIBRATION_SERIES,
            filters: None,
            expected: vec![(PRESS_VIBRATION_EARLY, 1.0), (PRESS_VIBRATION_LATE, 1.0)],
        },
        Row {
            case: "a statement no stored text carries",
            workspace: WORKSPACE,
            query: "turbine",
            filters: None,
            expected: vec![],
        },
        Row {
            case: "a statement with no terms at all",
            workspace: WORKSPACE,
            query: "!!! ???",
            filters: None,
            expected: vec![],
        },
        Row {
            case: "the datasetIds filter restricts the search to one dataset",
            workspace: WORKSPACE,
            query: "session",
            filters: Some(json!({ "datasetIds": [id(catalog, Dataset::Boiler)] })),
            expected: vec![(BOILER_TEMPERATURE, 1.0)],
        },
        Row {
            case: "the timeSeriesIds filter restricts the search to one series",
            workspace: WORKSPACE,
            query: "session",
            filters: Some(json!({ "timeSeriesIds": [TEMPERATURE_SERIES] })),
            expected: vec![(PRESS_TEMPERATURE, 1.0), (BOILER_TEMPERATURE, 1.0)],
        },
        Row {
            case: "the timeIntervals filter keeps the segments it overlaps",
            workspace: WORKSPACE,
            query: "session",
            filters: Some(json!({
                "timeIntervals": [{
                    "startTime": wire_instant((1_900, 0)),
                    "endTime": wire_instant((2_500, 0)),
                }],
            })),
            expected: vec![(PRESS_VIBRATION_EARLY, 1.0), (PRESS_TEMPERATURE, 1.0)],
        },
        Row {
            case: "an interval between two segments overlaps neither",
            workspace: WORKSPACE,
            query: "session",
            filters: Some(json!({
                "timeIntervals": [{
                    "startTime": wire_instant((2_000, 0)),
                    "endTime": wire_instant((3_000, 0)),
                }],
            })),
            expected: vec![],
        },
        Row {
            case: "two filters at once",
            workspace: WORKSPACE,
            query: "session",
            filters: Some(json!({
                "datasetIds": [id(catalog, Dataset::Press)],
                "timeSeriesIds": [VIBRATION_SERIES],
            })),
            expected: vec![(PRESS_VIBRATION_EARLY, 1.0), (PRESS_VIBRATION_LATE, 1.0)],
        },
        Row {
            case: "a search reads only the datasets of its own workspace",
            workspace: OTHER_WORKSPACE,
            query: "compressor vibration",
            filters: None,
            expected: vec![(NEIGHBOUR_VIBRATION, 1.0)],
        },
    ]
}

/// `POST /datasets`: create `catalogued` and answer its minted id.
async fn create_dataset(router: &MultiServiceRouter, catalogued: &Catalogued) -> String {
    let mut body = json!({
        "datasetName": catalogued.name,
        "datasetDescription": catalogued.description,
        "datasetType": catalogued.dataset_type,
        "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
    });
    let object = body.as_object_mut().expect("an object body");
    if !catalogued.metadata.is_empty() {
        let metadata: serde_json::Map<String, Value> = catalogued
            .metadata
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect();
        object.insert("metadata".to_string(), Value::Object(metadata));
    }
    if let Some(workspace) = catalogued.workspace {
        object.insert("workspaceName".to_string(), json!(workspace));
    }
    let (status, created) = call(router, "POST", "/datasets", Some(body)).await;
    assert_eq!(
        status, 202,
        "create {:?}: got {status} {created}",
        catalogued.dataset
    );
    let minted = created["datasetId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&minted),
        "create {:?}: expected a modelled datasetId, got {created}",
        catalogued.dataset
    );
    minted
}

/// `GET /workspaces/{workspaceName}/searches/{searchId}/results`, with the paging
/// members the caller named.
async fn results(
    router: &MultiServiceRouter,
    workspace: &str,
    search_id: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    call(
        router,
        "GET",
        &format!("/workspaces/{workspace}/searches/{search_id}/results{suffix}"),
        None,
    )
    .await
}

/// The two workspaces, the datasets of [`CATALOGUE`] and the segments of
/// [`SEEDED`], on a fresh engine.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Catalog) {
    let (router, engine) = router_with_engine();
    for workspace in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(&router, workspace).await;
    }
    let mut catalog = Catalog::new();
    for catalogued in CATALOGUE {
        catalog.push((catalogued.dataset, create_dataset(&router, catalogued).await));
    }

    // The operations that associate or ingest a dataset's segments are not
    // implemented yet, so the state's own producer entry point is where a dataset's
    // segments come from today — the way the sibling segment suite seeds them.
    let state = service_state(&*engine).expect("the iotsitewise store");
    let mut store = state.lock();
    for segment in SEEDED {
        let stored = store.put_dataset_data_segment(
            id(&catalog, segment.dataset),
            DataSegmentRecord {
                source_dataset_id: id(&catalog, segment.dataset).to_string(),
                time_series_id: series_id(segment.series).to_string(),
                start_timestamp: instant(segment.start),
                end_timestamp: instant(segment.end),
            },
        );
        assert!(stored, "seeding {segment:?} must be accepted");
    }
    drop(store);

    (router, engine, catalog)
}

/// `StartSearch` with `query` and `filters`, asserted accepted, answering the
/// minted id.
async fn start_search(
    router: &MultiServiceRouter,
    workspace: &str,
    query: &str,
    filters: Option<&Value>,
) -> String {
    let mut body = json!({ "queryStatement": query });
    if let Some(filters) = filters {
        body.as_object_mut()
            .expect("an object body")
            .insert("searchFilters".to_string(), filters.clone());
    }
    let (status, started) = start(router, workspace, body).await;
    assert_eq!(
        status, 202,
        "StartSearch {query:?}: expected 202, got {status} {started}"
    );
    started["searchId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartSearch {query:?}: no searchId in {started}"))
        .to_string()
}

/// Who answered a page being asserted: the search, the workspace it ran in, and
/// the ids the seeded datasets were created under.
#[derive(Debug, Clone, Copy)]
struct Answered<'a> {
    workspace: &'a str,
    search_id: &'a str,
    catalog: &'a Catalog,
}

/// Assert `element` is the result `segment` with the score `score`, as the search
/// `answered` must report it.
fn assert_result(
    case: &str,
    at: usize,
    element: &Value,
    segment: &Segment,
    score: f64,
    answered: Answered<'_>,
) {
    assert_eq!(
        element["searchId"].as_str(),
        Some(answered.search_id),
        "case {case:?}: result {at} must name the search it answers, got {element}"
    );
    assert_eq!(
        element["workspaceName"].as_str(),
        Some(answered.workspace),
        "case {case:?}: result {at} must name the workspace searched, got {element}"
    );
    assert_eq!(
        element["datasetId"].as_str(),
        Some(id(answered.catalog, segment.dataset)),
        "case {case:?}: result {at} must name the dataset holding the data, got {element}"
    );
    assert_eq!(
        element["timeSeriesId"].as_str(),
        Some(series_id(segment.series)),
        "case {case:?}: result {at} must name the series it covers, got {element}"
    );
    for (member, expected) in [
        ("startTimestamp", segment.start),
        ("endTimestamp", segment.end),
        ("topTimestamp", middle(segment)),
    ] {
        assert_eq!(
            (
                element[member]["timeInSeconds"].as_i64(),
                element[member]["offsetInNanos"].as_i64().unwrap_or(0) as i32,
            ),
            (Some(EPOCH + expected.0), expected.1),
            "case {case:?}: result {at} must report {member} {expected:?}, got {element}"
        );
    }
    let reported = element["score"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: result {at} carries no score in {element}"));
    assert!(
        (reported - score).abs() < 1e-6,
        "case {case:?}: result {at} must score {score}, got {reported}"
    );
}

/// The `nextToken` a page must issue to resume at `segment` of the enumeration
/// `answered`: the hex of that result's identity — the search that produced it (the
/// workspace it ran in and its id) and the segment it reports (the series covered,
/// the span covered of it and the dataset holding it).
///
/// Hand-rolled as an INDEPENDENT oracle rather than read back off the handler, so a
/// token bound to the wrong thing cannot agree with itself and pass.
fn resume_token(segment: &Segment, answered: Answered<'_>) -> String {
    hex_token(&format!(
        "{}|{}|{}|{}.{}|{}.{}|{}",
        answered.workspace,
        answered.search_id,
        series_id(segment.series),
        EPOCH + segment.start.0,
        segment.start.1,
        EPOCH + segment.end.0,
        segment.end.1,
        id(answered.catalog, segment.dataset),
    ))
}

/// Every result of a 200 answer, asserted against `expected` in order.
fn assert_page(case: &str, body: &Value, expected: &[(Segment, f64)], answered: Answered<'_>) {
    let reported = body["searchResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no searchResults in {body}"));
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected {} results, got {body}",
        expected.len()
    );
    for (at, (element, (segment, score))) in reported.iter().zip(expected.iter()).enumerate() {
        assert_result(case, at, element, segment, *score, answered);
    }
}

#[tokio::test]
async fn a_result_is_a_matching_segment_of_the_searched_workspaces_datasets() {
    let (router, _engine, catalog) = seeded().await;
    let rows = table(&catalog);

    // Every search is started FIRST and the owed transitions waited out once: a
    // search answers results only once the clock has taken it to SUCCEEDED.
    let mut started = Vec::new();
    for row in &rows {
        started.push(start_search(&router, row.workspace, row.query, row.filters.as_ref()).await);
    }
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR + SETTLED).await;

    for (row, search_id) in rows.iter().zip(started.iter()) {
        let (status, body) = results(&router, row.workspace, search_id, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {body}",
            row.case
        );
        assert_page(
            row.case,
            &body,
            &row.expected,
            Answered {
                workspace: row.workspace,
                search_id,
                catalog: &catalog,
            },
        );
        // Everything this search matched fits one default page, so there is nothing
        // left to resume: "absent when there are no more results".
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {:?}: a single page must carry no nextToken, got {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_page_is_bounded_by_max_results_and_resumed_by_its_next_token() {
    let (router, _engine, catalog) = seeded().await;
    // The statement every seeded dataset's type carries, so the ranked enumeration
    // is the four segments of the searched workspace.
    let ranked = [
        PRESS_VIBRATION_EARLY,
        PRESS_VIBRATION_LATE,
        PRESS_TEMPERATURE,
        BOILER_TEMPERATURE,
    ];
    let search_id = start_search(&router, WORKSPACE, "session", None).await;
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR + SETTLED).await;
    let answered = Answered {
        workspace: WORKSPACE,
        search_id: &search_id,
        catalog: &catalog,
    };

    // The rows differ along the page size: one result per page, an uneven split, a
    // page exactly as long as the enumeration, and a page longer than it.
    for size in [1usize, 3, 4, 10] {
        let case = format!("maxResults={size}");
        let mut seen = 0usize;
        let mut token: Option<String> = None;
        while seen < ranked.len() {
            let (status, body) = results(
                &router,
                WORKSPACE,
                &search_id,
                Some(size as i32),
                token.as_deref(),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
            let end = (seen + size).min(ranked.len());
            let expected: Vec<(Segment, f64)> = ranked[seen..end]
                .iter()
                .map(|segment| (*segment, 1.0))
                .collect();
            assert_page(&case, &body, &expected, answered);
            let reported = body.get("nextToken").and_then(Value::as_str);
            if end < ranked.len() {
                assert_eq!(
                    reported,
                    Some(resume_token(&ranked[end], answered).as_str()),
                    "case {case:?}: the token must resume at result {end} of this \
                     search's own enumeration, got {body}"
                );
            } else {
                assert!(
                    reported.is_none(),
                    "case {case:?}: the last page carries no nextToken, got {body}"
                );
            }
            token = reported.map(str::to_string);
            seen = end;
        }
        assert!(
            token.is_none(),
            "case {case:?}: paging must end with no token left, got {token:?}"
        );
    }

    // A token this operation never issued is a bad request, not an empty page: text
    // that is not hex at all, hex of a bare ordinal (never a token this operation
    // writes, in any spelling), hex of a resume point this enumeration holds no
    // result at, and the uppercase spelling of a token it DID issue.
    let issued = resume_token(&ranked[1], answered);
    for (case, token) in [
        ("not-a-token", "Zz".to_string()),
        ("a-bare-ordinal", hex_token("1")),
        ("a-padded-ordinal", hex_token("001")),
        ("a-signed-ordinal", hex_token("+1")),
        ("past-the-end", hex_token("4")),
        ("far-past-the-end", hex_token("99")),
        (
            "a-segment-of-no-result-of-this-search",
            resume_token(&UNSCOPED_VIBRATION, answered),
        ),
        ("another-hex-spelling-of-an-issued-token", issued.to_uppercase()),
    ] {
        let (status, body) = results(&router, WORKSPACE, &search_id, None, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unissued token must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: an unissued token is a modelled bad request, got {body}"
        );
    }
}

#[tokio::test]
async fn results_are_available_only_once_the_search_has_succeeded() {
    let (router, _engine, catalog) = seeded().await;
    let search_id = start_search(&router, WORKSPACE, "compressor vibration", None).await;

    // The transition the start owes is not instant, so a read taken straight after
    // it finds the search queued — and is refused rather than answered with an
    // empty page, which would report "not ready yet" as "nothing matched".
    let (status, body) = results(&router, WORKSPACE, &search_id, None, None).await;
    assert_eq!(
        status, 400,
        "a search that has not succeeded must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a search that has not succeeded is a modelled bad request, got {body}"
    );
    let message = body["message"].as_str().unwrap_or("");
    assert!(
        message.contains(SEARCH_QUEUED) && message.contains(SEARCH_SUCCEEDED),
        "the refusal must name the status reached and the one results need, got {body}"
    );

    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR + SETTLED).await;
    let (status, body) = results(&router, WORKSPACE, &search_id, None, None).await;
    assert_eq!(
        status, 200,
        "a succeeded search must answer its results, got {status} {body}"
    );
    assert_page(
        "succeeded",
        &body,
        &[
            (PRESS_VIBRATION_EARLY, 1.0),
            (PRESS_VIBRATION_LATE, 1.0),
            (PRESS_TEMPERATURE, 1.0),
        ],
        Answered {
            workspace: WORKSPACE,
            search_id: &search_id,
            catalog: &catalog,
        },
    );
}

#[tokio::test]
async fn a_pair_no_search_is_stored_under_is_a_modelled_not_found() {
    let (router, _engine, _catalog) = seeded().await;
    let search_id = start_search(&router, WORKSPACE, "compressor vibration", None).await;
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR + SETTLED).await;

    // A well-formed `SearchId` (len 23..36) no start ever minted.
    let unknown_search = "00000000-1111-2222-3333-444444444444";
    for (case, workspace, search, expected) in [
        ("its-own-workspace", WORKSPACE, search_id.as_str(), 200),
        ("another-workspace", OTHER_WORKSPACE, search_id.as_str(), 404),
        ("a-workspace-no-call-made", "search-plant_3", search_id.as_str(), 404),
        ("an-id-no-start-minted", WORKSPACE, unknown_search, 404),
    ] {
        let (status, body) = results(&router, workspace, search, None, None).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
        if expected == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: an unresolvable read is a modelled not-found, got {body}"
            );
            // The refusal names the resource that is ABSENT: the workspace when the
            // store holds no such workspace, otherwise the search.
            let expected_message = if workspace == "search-plant_3" {
                not_found_message(RESOURCE_WORKSPACE, workspace)
            } else {
                not_found_message(RESOURCE_SEARCH, search)
            };
            assert_eq!(
                body["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?}: expected the service's not-found wording, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn reading_results_writes_nothing() {
    let (router, engine, catalog) = seeded().await;
    let search_id = start_search(&router, WORKSPACE, "session", None).await;
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR + SETTLED).await;

    // The whole searched slice of the store, rendered through `Debug` so a record
    // that grows a member is covered without this suite being updated.
    let snapshot = |shared: &Arc<engine::InMemoryEngine>| {
        let state = service_state(&**shared).expect("the iotsitewise store");
        let guard = state.lock();
        let mut rendered: Vec<String> = guard
            .searches
            .iter()
            .map(|(key, record)| format!("search {key:?} => {record:?}"))
            .chain(
                guard
                    .dataset_data_segments
                    .iter()
                    .map(|(key, held)| format!("segments {key:?} => {held:?}")),
            )
            .collect();
        drop(guard);
        rendered.sort();
        rendered
    };

    let before = snapshot(&engine);
    let mut answers = Vec::new();
    for read in 1..=4 {
        let (status, body) = results(&router, WORKSPACE, &search_id, Some(2), None).await;
        assert_eq!(status, 200, "read {read}: expected 200, got {status} {body}");
        answers.push(body);
    }

    for (read, body) in answers.iter().enumerate() {
        assert_eq!(
            body, &answers[0],
            "read {read}: every read of one store must answer the same page, expected {:?}",
            answers[0]
        );
    }
    assert_page(
        "repeated-read",
        &answers[0],
        &[(PRESS_VIBRATION_EARLY, 1.0), (PRESS_VIBRATION_LATE, 1.0)],
        Answered {
            workspace: WORKSPACE,
            search_id: &search_id,
            catalog: &catalog,
        },
    );
    assert_eq!(
        snapshot(&engine),
        before,
        "no number of reads may change the stored searches or segments, expected {before:?}"
    );
}

/// The searches the cross-enumeration suite pages, differing along what makes two
/// enumerations different: how many results they hold, which datasets and which
/// series those come from, whether a filter narrowed them, and which workspace the
/// search ran in. Every statement below matches each of its results in full, so
/// each one scores `1.0`.
fn paged_searches() -> Vec<Row> {
    let all = 1.0;
    vec![
        Row {
            case: "four results, the whole searched workspace",
            workspace: WORKSPACE,
            query: "session",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, all),
                (PRESS_VIBRATION_LATE, all),
                (PRESS_TEMPERATURE, all),
                (BOILER_TEMPERATURE, all),
            ],
        },
        Row {
            case: "three results, one dataset's segments",
            workspace: WORKSPACE,
            query: "compressor vibration",
            filters: None,
            expected: vec![
                (PRESS_VIBRATION_EARLY, all),
                (PRESS_VIBRATION_LATE, all),
                (PRESS_TEMPERATURE, all),
            ],
        },
        Row {
            case: "two results, narrowed to one series by a filter",
            workspace: WORKSPACE,
            query: "session",
            filters: Some(json!({ "timeSeriesIds": [TEMPERATURE_SERIES] })),
            expected: vec![(PRESS_TEMPERATURE, all), (BOILER_TEMPERATURE, all)],
        },
        Row {
            case: "one result, so no page is left to resume",
            workspace: WORKSPACE,
            query: "boiler",
            filters: None,
            expected: vec![(BOILER_TEMPERATURE, all)],
        },
        Row {
            case: "a search of the other workspace",
            workspace: OTHER_WORKSPACE,
            query: "compressor vibration",
            filters: None,
            expected: vec![(NEIGHBOUR_VIBRATION, all)],
        },
    ]
}

#[tokio::test]
async fn a_token_names_a_page_only_of_the_search_it_was_issued_for() {
    let (router, _engine, catalog) = seeded().await;
    let searches = paged_searches();

    // Every search is started first and the owed transitions waited out once.
    let mut started = Vec::new();
    for row in &searches {
        started.push(start_search(&router, row.workspace, row.query, row.filters.as_ref()).await);
    }
    tokio::time::sleep(SEARCH_QUEUED_FOR + SEARCH_RUNS_FOR + SETTLED).await;

    let answered = |at: usize| Answered {
        workspace: searches[at].workspace,
        search_id: started[at].as_str(),
        catalog: &catalog,
    };

    // The first page of each search, one result long, and the token it issued: the
    // identity of the result the NEXT page of THAT search resumes at, absent when
    // the search has nothing left to resume.
    let mut tokens: Vec<Option<String>> = Vec::new();
    for (at, row) in searches.iter().enumerate() {
        let (status, body) = results(&router, row.workspace, &started[at], Some(1), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {body}",
            row.case
        );
        assert_page(row.case, &body, &row.expected[..1], answered(at));
        let reported = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        let expected = row
            .expected
            .get(1)
            .map(|(segment, _)| resume_token(segment, answered(at)));
        assert_eq!(
            reported, expected,
            "case {:?}: expected the token {expected:?} for this search's second \
             result, got {reported:?} in {body}",
            row.case
        );
        tokens.push(reported);
    }

    // The rule, over every pair: a token is honoured against the search that minted
    // it, and against no other — not even one whose results include the very
    // segment the token resumes at.
    for (minted_by, token) in tokens.iter().enumerate() {
        let Some(token) = token.as_deref() else {
            continue;
        };
        for (against, row) in searches.iter().enumerate() {
            let case = format!(
                "token of {:?} replayed against {:?}",
                searches[minted_by].case, row.case
            );
            let (status, body) =
                results(&router, row.workspace, &started[against], Some(1), Some(token)).await;
            if against == minted_by {
                assert_eq!(
                    status, 200,
                    "case {case:?}: its own search must honour it, got {status} {body}"
                );
                assert_page(&case, &body, &row.expected[1..2], answered(against));
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: a token of another search names no page here, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: a token this search never issued is a modelled bad \
                 request, got {body}"
            );
            // And nothing of that search was consumed or hidden by the replay: its
            // own first page still leads with its most relevant result.
            let (status, fresh) =
                results(&router, row.workspace, &started[against], Some(1), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the search itself still answers, got {status} {fresh}"
            );
            assert_page(&case, &fresh, &row.expected[..1], answered(against));
        }
    }
}
