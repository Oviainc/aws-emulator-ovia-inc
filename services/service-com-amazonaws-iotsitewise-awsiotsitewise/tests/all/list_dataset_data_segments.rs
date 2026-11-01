// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDatasetDataSegments`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with.
//!
//! The rules these suites pin:
//!
//! * a listing reports every data segment the store holds for the dataset the
//!   request names, each summary read off the stored segment (the dataset its data
//!   came from, the time series it covers and the half-open span it covers of it)
//!   plus the alias and the data type of the time series it covers — never off the
//!   request;
//! * `maxResults`/`nextToken` page ONE stable enumeration: the pages joined are
//!   the whole listing once, in one order, whatever page size the caller asks for,
//!   a token this listing never issued is refused, and each published `maxResults`
//!   bound is served while one past it is refused;
//! * the dataset is resolved first and scoped by the `workspaceName` and the
//!   `datasetVersion` the request names, so one that resolves to nothing is the
//!   modelled 404 rather than an empty page, which would report absence as
//!   success;
//! * a listing REPORTS: it writes nothing, so it moves the dataset neither on in
//!   version nor out of the state the clock left it in, and the next listing
//!   follows whatever a deletion did to the store.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `is_modelled_id`,
//! `create_model`, `create_asset`, `property_id`, `property_catalog`) is the one
//! `asset_lifecycle` already defines, and the `timeSeriesId` oracle is
//! `time_series`'s; this suite reuses both rather than growing a second copy.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, DataSegmentRecord, ACTIVATION_DELAY, RESOURCE_DATASET,
    RESOURCE_WORKSPACE,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, is_modelled_id, property_catalog, property_id,
    router_with_engine,
};
use crate::support::anchor;
use crate::time_series::time_series_id_of;

/// The workspace the datasets of this suite live in.
const WORKSPACE: &str = "segment-floor";

/// A workspace name no dataset of this suite was written under.
const OTHER_WORKSPACE: &str = "segment-roof";

/// The alias of the DISASSOCIATED data stream alias-addressed ingestion creates,
/// whose data type is the `DOUBLE` of the values written to it.
const STREAM_ALIAS: &str = "/plant/press/1/temperature";

/// The alias bound to the `Cycles` (`INTEGER`) asset property, so a row can prove
/// an ASSOCIATED stream's summary carries the alias its property carries.
const CYCLES_ALIAS: &str = "/plant/press/1/cycles";

/// A `TimeSeriesId` no stored stream carries: an `ID`-shaped value this suite
/// seeds a segment under, so a row can prove a segment is still reported when the
/// store holds no stream to take an alias or a data type from.
const ABSENT_TIME_SERIES: &str = "9d4c2b61-7f08-4a35-8c19-5e6b0d3a7f21";

/// The sample seconds, relative to [`anchor`], the disassociated stream is
/// ingested with.
const STREAM_SAMPLES: &[i64] = &[0, 5, 10, 15, 100, 150];

/// Which seeded time series a segment covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The disassociated stream [`STREAM_ALIAS`] names: a `DOUBLE` series under
    /// an alias.
    Stream,
    /// The series the aliased `Cycles` asset property owns: an `INTEGER` series
    /// under an alias.
    Aliased,
    /// The series the `Status` asset property owns, which carries NO alias: a
    /// `STRING` series under none.
    Unaliased,
    /// [`ABSENT_TIME_SERIES`]: a `TimeSeriesId` the store holds no stream for.
    Absent,
}

/// One seeded data segment: the time series it covers, the half-open span it
/// covers of it (seconds relative to [`anchor`] plus a nanosecond offset), and
/// whether its data came from the holding dataset itself or from another one.
#[derive(Debug, Clone, Copy)]
struct Seed {
    target: Target,
    start: (i64, i32),
    end: (i64, i32),
    /// `true` when the segment's `sourceDatasetId` is the SOURCE dataset rather
    /// than the dataset holding it, the way a curated dataset's segments name the
    /// session dataset they were selected from.
    from_source: bool,
}

/// The segments the fixture seeds, spanning what a summary must report: the three
/// kinds of stream the store can resolve and one it cannot, adjacent and
/// non-adjacent spans, a span whose bounds carry a nanosecond offset, two series
/// holding a segment at the very same span, and a segment whose data came from
/// another dataset.
const SEEDED: &[Seed] = &[
    Seed {
        target: Target::Stream,
        start: (0, 0),
        end: (10, 0),
        from_source: false,
    },
    Seed {
        target: Target::Stream,
        start: (10, 0),
        end: (20, 0),
        from_source: false,
    },
    Seed {
        target: Target::Stream,
        start: (100, 0),
        end: (160, 250_000_000),
        from_source: true,
    },
    Seed {
        target: Target::Aliased,
        start: (0, 0),
        end: (10, 0),
        from_source: false,
    },
    Seed {
        target: Target::Aliased,
        start: (40, 125_000_000),
        end: (80, 750_000_000),
        from_source: true,
    },
    Seed {
        target: Target::Unaliased,
        start: (0, 0),
        end: (10, 0),
        from_source: false,
    },
    Seed {
        target: Target::Absent,
        start: (0, 0),
        end: (10, 0),
        from_source: false,
    },
];

/// The time series ids the fixture seeded segments under, and the dataset a
/// segment's data can have come from.
#[derive(Debug, Clone)]
struct Fixture {
    dataset_id: String,
    source_dataset_id: String,
    stream: String,
    aliased: String,
    unaliased: String,
}

impl Fixture {
    /// The `TimeSeriesId` `target` names.
    fn id(&self, target: Target) -> &str {
        match target {
            Target::Stream => &self.stream,
            Target::Aliased => &self.aliased,
            Target::Unaliased => &self.unaliased,
            Target::Absent => ABSENT_TIME_SERIES,
        }
    }

    /// The `sourceDatasetId` a seed's segment must be reported with.
    fn source_of(&self, seed: &Seed) -> &str {
        if seed.from_source {
            &self.source_dataset_id
        } else {
            &self.dataset_id
        }
    }

    /// The `alias` of the time series `target` names, as the store holds it: a
    /// stream the store cannot resolve, and one whose asset property carries no
    /// alias, have none.
    fn alias_of(&self, target: Target) -> &'static str {
        match target {
            Target::Stream => STREAM_ALIAS,
            Target::Aliased => CYCLES_ALIAS,
            Target::Unaliased | Target::Absent => "",
        }
    }

    /// The `dataType` of the time series `target` names: the `PropertyDataType`
    /// its asset property declares, or the type the disassociated stream was
    /// created with. Source: the property catalog `asset_lifecycle` defines
    /// (`Cycles` -> INTEGER, `Status` -> STRING) and the `DOUBLE` values ingested
    /// against [`STREAM_ALIAS`].
    fn data_type_of(&self, target: Target) -> &'static str {
        match target {
            Target::Stream => "DOUBLE",
            Target::Aliased => "INTEGER",
            Target::Unaliased => "STRING",
            Target::Absent => "",
        }
    }

    /// The whole enumeration a listing of the fixture's dataset must report, in
    /// the one order it is paged in: a segment's time series and its own span are
    /// its identity, so that is what the order is on.
    ///
    /// An INDEPENDENT oracle: built from the seeds rather than from a listing.
    fn expected(&self) -> Vec<Value> {
        let mut rows: Vec<Value> = SEEDED
            .iter()
            .map(|seed| {
                json!({
                    "sourceDatasetId": self.source_of(seed),
                    "timeSeriesId": self.id(seed.target),
                    "startTimestamp": wire_instant(seed.start.0, seed.start.1),
                    "endTimestamp": wire_instant(seed.end.0, seed.end.1),
                    "alias": self.alias_of(seed.target),
                    "dataType": self.data_type_of(seed.target),
                })
            })
            .collect();
        rows.sort_by_key(|row| {
            (
                row["timeSeriesId"].as_str().unwrap_or_default().to_string(),
                row["startTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
                row["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
                row["endTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
                row["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
            )
        });
        rows
    }
}

/// A table second as the absolute `timeInSeconds` the wire carries, anchored
/// inside the window `BatchPutAssetPropertyValue` accepts; see `super::support`.
fn at(seconds: i64) -> i64 {
    anchor() + seconds
}

/// A modelled `TimeInNanos` at the table instant `(seconds, nanos)`.
fn instant(seconds: i64, nanos: i32) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: at(seconds),
        offset_in_nanos: Some(nanos),
    }
}

/// The same, on the wire.
fn wire_instant(seconds: i64, nanos: i32) -> Value {
    json!({ "timeInSeconds": at(seconds), "offsetInNanos": nanos })
}

/// A `nextToken` in the form this listing issues: the hex of the identity of the
/// segment the next page resumes from.
///
/// Hand-rolled on purpose, as an INDEPENDENT oracle: the handler encodes its token
/// with the `hex` crate, so re-using that crate here would let one encoding bug
/// agree with itself and pass.
fn token_of(summary: &Value) -> String {
    let key = format!(
        "{}|{}.{}|{}.{}",
        summary["timeSeriesId"].as_str().unwrap_or_default(),
        summary["startTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
        summary["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
        summary["endTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
        summary["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
    );
    key.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `POST /datasets`: create a dataset of `dataset_type` in `workspace` and answer
/// its minted id.
async fn create_dataset(
    router: &MultiServiceRouter,
    name: &str,
    dataset_type: &str,
    workspace: Option<&str>,
) -> String {
    // A dataset the request puts in a workspace is created INSIDE it, and the
    // create resolves the name through the workspace registry.
    if let Some(workspace) = workspace {
        crate::support::ensure_workspace(router, workspace).await;
    }
    let mut body = json!({
        "datasetName": name,
        "datasetType": dataset_type,
        "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
    });
    if let Some(workspace) = workspace {
        body.as_object_mut()
            .expect("an object body")
            .insert("workspaceName".to_string(), json!(workspace));
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

/// `GET /datasets/{datasetId}/data-segments` with the query `extra` adds.
async fn list_raw(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
    extra: &[String],
) -> (u16, Value) {
    let mut query = format!("workspaceName={workspace}");
    for one in extra {
        query.push('&');
        query.push_str(one);
    }
    call(
        router,
        "GET",
        &format!("/datasets/{dataset_id}/data-segments?{query}"),
        None,
    )
    .await
}

/// The same, asserting the listing was served, and answering its body.
async fn listed(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
    extra: &[String],
) -> Value {
    let (status, body) = list_raw(router, dataset_id, workspace, extra).await;
    assert_eq!(
        status, 200,
        "list {dataset_id} {extra:?}: got {status} {body}"
    );
    body
}

/// The `dataSegments` of a listing body, as the summaries it reported.
fn segments(body: &Value) -> Vec<Value> {
    body["dataSegments"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing carries dataSegments, got {body}"))
        .clone()
}

/// One reported summary reduced to the members the oracle states, so a comparison
/// names the difference rather than the whole body.
fn reported(summary: &Value) -> Value {
    json!({
        "sourceDatasetId": summary["sourceDatasetId"],
        "timeSeriesId": summary["timeSeriesId"],
        "startTimestamp": summary["startTimestamp"],
        "endTimestamp": summary["endTimestamp"],
        "alias": summary["alias"],
        "dataType": summary["dataType"],
    })
}

/// Ingest a DOUBLE sample at each of `seconds` against `alias`, which CREATES the
/// disassociated data stream that alias names.
async fn ingest(router: &MultiServiceRouter, alias: &str, seconds: &[i64]) {
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
    let (status, body) = call(router, "POST", "/properties", Some(json!({ "entries": entries }))).await;
    assert_eq!(status, 200, "ingest {alias}: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "ingest {alias}: every entry must be accepted, got {body}"
    );
}

/// Seed `spans` as the data segments of `dataset_id`, through the producer entry
/// point the state exposes — the operations that associate or ingest a dataset's
/// segments are not implemented yet, so this is where a dataset's segments come
/// from today.
fn seed_segments(
    engine: &Arc<engine::InMemoryEngine>,
    fixture: &Fixture,
    dataset_id: &str,
    seeds: &[Seed],
) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for seed in seeds {
        let stored = store.put_dataset_data_segment(
            dataset_id,
            DataSegmentRecord {
                source_dataset_id: fixture.source_of(seed).to_string(),
                time_series_id: fixture.id(seed.target).to_string(),
                start_timestamp: instant(seed.start.0, seed.start.1),
                end_timestamp: instant(seed.end.0, seed.end.1),
            },
        );
        assert!(stored, "the store must hold the seeded segment {seed:?}");
    }
}

/// The fixture: a session dataset of [`WORKSPACE`] holding [`SEEDED`], the three
/// resolvable data streams those segments cover, and a second dataset a segment's
/// data can have come from.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Fixture) {
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Press session", "SESSION", Some(WORKSPACE)).await;
    let source_dataset_id =
        create_dataset(&router, "Press source", "SESSION", Some(WORKSPACE)).await;

    // A DISASSOCIATED stream, created by alias-addressed ingestion.
    ingest(&router, STREAM_ALIAS, STREAM_SAMPLES).await;
    let (status, described) = call(
        &router,
        "GET",
        &format!("/timeseries/describe?alias={STREAM_ALIAS}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe {STREAM_ALIAS}: got {status} {described}");
    let stream = described["timeSeriesId"].as_str().unwrap_or("").to_string();

    // Two ASSOCIATED streams: the series of an asset property carrying an alias,
    // and the series of one carrying none.
    let model_id = create_model(&router, "Press", property_catalog()).await;
    let asset_id = create_asset(&router, "Press-1", &model_id).await;
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "describe {asset_id}: got {status} {asset}");
    let cycles = property_id(&asset["assetProperties"], "Cycles");
    let status_property = property_id(&asset["assetProperties"], "Status");
    let (code, bound) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}/properties/{cycles}"),
        Some(json!({ "propertyAlias": CYCLES_ALIAS })),
    )
    .await;
    assert_eq!(code, 200, "bind {CYCLES_ALIAS}: got {code} {bound}");

    let fixture = Fixture {
        dataset_id,
        source_dataset_id,
        stream,
        aliased: time_series_id_of(&asset_id, &cycles),
        unaliased: time_series_id_of(&asset_id, &status_property),
    };
    assert!(
        is_modelled_id(&fixture.stream),
        "a disassociated stream's timeSeriesId must be a modelled ID, got {fixture:?}"
    );
    seed_segments(&engine, &fixture, &fixture.dataset_id.clone(), SEEDED);
    (router, engine, fixture)
}

#[tokio::test]
async fn every_summary_is_read_off_the_stored_segment_and_the_series_it_covers() {
    let (router, _engine, fixture) = seeded().await;

    let body = listed(
        &router,
        &fixture.dataset_id,
        WORKSPACE,
        &["maxResults=250".to_string()],
    )
    .await;
    let got: Vec<Value> = segments(&body).iter().map(reported).collect();
    let expected = fixture.expected();
    assert_eq!(
        got.len(),
        expected.len(),
        "the listing must report every seeded segment once, got {body}"
    );
    for (summary, wanted) in got.iter().zip(expected.iter()) {
        assert_eq!(
            summary, wanted,
            "case {:?}: expected {wanted}, got {summary}",
            wanted["timeSeriesId"]
        );
    }
    assert_eq!(
        body["nextToken"].as_str(),
        None,
        "a listing that reported everything issues no token, got {body}"
    );

    // No enrichment job names this dataset, so no summary claims an
    // enrichment it was never given.
    for summary in segments(&body) {
        assert!(
            summary.get("enrichment").is_none() || summary["enrichment"].is_null(),
            "no segment is enriched, got {summary}"
        );
    }

    // A dataset the store holds no segment for is an empty page, not a refusal:
    // the dataset itself resolves.
    let empty = create_dataset(&router, "Press empty", "SESSION", Some(WORKSPACE)).await;
    let body = listed(&router, &empty, WORKSPACE, &[]).await;
    assert_eq!(
        segments(&body),
        Vec::<Value>::new(),
        "a dataset holding no segment lists none, got {body}"
    );
    assert_eq!(
        body["nextToken"].as_str(),
        None,
        "an empty page issues no token, got {body}"
    );
}

#[tokio::test]
async fn max_results_and_next_token_page_one_stable_enumeration() {
    let (router, _engine, fixture) = seeded().await;
    let baseline = fixture.expected();
    let total = baseline.len();

    // Rows: a page smaller than the listing, sizes that divide it and sizes that
    // do not, exactly the listing, more than it, the modelled maximum, and no
    // `maxResults` at all (served at the documented default of 50).
    let page_sizes: [Option<i64>; 8] = [
        Some(1),
        Some(2),
        Some(3),
        Some(total as i64 - 1),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(250),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<Value> = Vec::new();
        let mut pages = 0usize;
        let mut token: Option<String> = None;
        loop {
            let mut extra: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                extra.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                extra.push(format!("nextToken={token}"));
            }
            let body = listed(&router, &fixture.dataset_id, WORKSPACE, &extra).await;
            let page = segments(&body);
            pages += 1;
            if let Some(size) = page_size {
                assert!(
                    page.len() as i64 <= size,
                    "case {case:?}: a page must hold at most {size} segments, got {body}"
                );
            }
            walked.extend(page.iter().map(reported));
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total + 1,
                "case {case:?}: the walk must terminate, walked {pages} pages"
            );
        }
        assert_eq!(
            walked, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one \
             order, got {walked:?}"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            // An absent `maxResults` is served at the documented default of 50,
            // which is more than this fixture holds.
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the segment the NEXT page starts at, in the
    // form this listing issues: the hex of that segment's identity, built by an
    // oracle that does not share the handler's encoder.
    let first = listed(
        &router,
        &fixture.dataset_id,
        WORKSPACE,
        &["maxResults=1".to_string()],
    )
    .await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(token_of(&baseline[1]).as_str()),
        "the first page's token resumes at the second segment, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an
    // empty page: text that is not hex at all, and the hex of an identity no
    // segment of this dataset carries.
    let absent = json!({
        "timeSeriesId": ABSENT_TIME_SERIES,
        "startTimestamp": wire_instant(900, 0),
        "endTimestamp": wire_instant(901, 0),
    });
    for (case, token) in [
        ("not hex at all", "notatoken".to_string()),
        ("hex of an identity no segment carries", token_of(&absent)),
    ] {
        let (status, body) = list_raw(
            &router,
            &fixture.dataset_id,
            WORKSPACE,
            &[format!("nextToken={token}")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..250 (`smithy_query(query="operation",
    // operation="ListDatasetDataSegments")`): each published bound is served and
    // one past it on either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", 1_i64, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ] {
        let (status, body) = list_raw(
            &router,
            &fixture.dataset_id,
            WORKSPACE,
            &[format!("maxResults={max_results}")],
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be \
                 served, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must \
                 be refused, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_dataset_the_request_does_not_resolve_is_refused_rather_than_empty() {
    let (router, _engine, fixture) = seeded().await;
    let outside = create_dataset(&router, "Press outside", "EXTERNAL", None).await;
    // The row scoping the stored dataset to ANOTHER workspace is about the
    // dataset's scoping, so that other workspace is opened: a name this service
    // does not hold is its own row below, and reports the workspace.
    crate::support::ensure_workspace(&router, OTHER_WORKSPACE).await;

    // (case, the datasetId the request names, the workspaceName it carries, the
    // extra query, the absence the refusal states) — an id that names nothing, the
    // stored id scoped to a workspace that does not hold it, a dataset held by no
    // workspace at all, a `datasetVersion` other than the one this store holds,
    // and a workspace this service does not hold at all. Each is resolved BEFORE
    // the page is cut, so paging query is no excuse to answer success.
    let rows: Vec<(&str, String, &str, Vec<String>, String)> = vec![
        (
            "an id no dataset carries",
            ABSENT_TIME_SERIES.to_string(),
            WORKSPACE,
            Vec::new(),
            not_found_message(RESOURCE_DATASET, ABSENT_TIME_SERIES),
        ),
        (
            "an id no dataset carries, with a page size",
            ABSENT_TIME_SERIES.to_string(),
            WORKSPACE,
            vec!["maxResults=1".to_string()],
            not_found_message(RESOURCE_DATASET, ABSENT_TIME_SERIES),
        ),
        (
            "the stored dataset under another workspace",
            fixture.dataset_id.clone(),
            OTHER_WORKSPACE,
            Vec::new(),
            not_found_message(RESOURCE_DATASET, &fixture.dataset_id),
        ),
        (
            "a dataset held by no workspace",
            outside.clone(),
            WORKSPACE,
            Vec::new(),
            not_found_message(RESOURCE_DATASET, &outside),
        ),
        (
            "a datasetVersion the store does not hold",
            fixture.dataset_id.clone(),
            WORKSPACE,
            vec!["datasetVersion=2".to_string()],
            not_found_message(RESOURCE_DATASET, &fixture.dataset_id),
        ),
        (
            "the version a dataset reaches only after a write",
            fixture.dataset_id.clone(),
            WORKSPACE,
            vec!["datasetVersion=7".to_string()],
            not_found_message(RESOURCE_DATASET, &fixture.dataset_id),
        ),
        (
            "a workspace this service does not hold",
            fixture.dataset_id.clone(),
            "segment-ghost",
            Vec::new(),
            not_found_message(RESOURCE_WORKSPACE, "segment-ghost"),
        ),
    ];

    for (case, dataset_id, workspace, extra, expected) in rows {
        let (status, body) = list_raw(&router, &dataset_id, workspace, &extra).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: the refusal must name the resource that is absent, got {body}"
        );
    }

    // The version the dataset IS, and the dataset's own workspace, both resolve:
    // the scoping above refuses only what the store does not hold.
    let body = listed(
        &router,
        &fixture.dataset_id,
        WORKSPACE,
        &["datasetVersion=1".to_string()],
    )
    .await;
    assert_eq!(
        segments(&body).len(),
        SEEDED.len(),
        "the dataset's own version lists its segments, got {body}"
    );
}

#[tokio::test]
async fn a_listing_reports_and_the_next_one_follows_the_store() {
    let (router, _engine, fixture) = seeded().await;

    // A listing taken straight after the create is served while the dataset is
    // still settling, and leaves it settling: a read owes no transition of its own.
    let before = describe_dataset(&router, &fixture.dataset_id).await;
    assert_eq!(
        before["datasetStatus"]["state"].as_str(),
        Some("CREATING"),
        "a dataset read straight after its create is still settling, got {before}"
    );
    let first = listed(&router, &fixture.dataset_id, WORKSPACE, &[]).await;
    let again = listed(&router, &fixture.dataset_id, WORKSPACE, &[]).await;
    assert_eq!(
        segments(&again),
        segments(&first),
        "two listings of one unchanged store report the same segments in the same order"
    );
    let after = describe_dataset(&router, &fixture.dataset_id).await;
    assert_eq!(
        after["datasetVersion"].as_str(),
        before["datasetVersion"].as_str(),
        "a listing must not take the dataset to another version, got {after}"
    );
    assert_eq!(
        after["datasetStatus"]["state"].as_str(),
        Some("CREATING"),
        "a listing must not settle the dataset early, got {after}"
    );

    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;

    // What a deletion did to the store is what the next listing reports: the
    // segment it removed is gone and every other one stays, in the same order.
    // The entry a deletion addresses carries a `timeSeriesId` the model bounds at
    // 36 characters (`smithy_query(query="shape", shape="DeleteDataSegmentEntry")`),
    // so the segment deleted here is one of the stream whose id is a bare `ID` —
    // the series an asset property owns is addressed by a longer composite id that
    // a deletion entry cannot carry.
    let doomed = segments(&first)
        .into_iter()
        .find(|summary| summary["timeSeriesId"].as_str() == Some(fixture.stream.as_str()))
        .unwrap_or_else(|| panic!("the fixture holds a segment of the stream, got {first}"));
    let (status, deleted) = call(
        &router,
        "POST",
        &format!("/datasets/{}/data-segments/batch-delete", fixture.dataset_id),
        Some(json!({
            "workspaceName": WORKSPACE,
            "deleteDataSegmentEntries": [{
                "timeSeriesId": doomed["timeSeriesId"],
                "startTimestamp": doomed["startTimestamp"],
                "endTimestamp": doomed["endTimestamp"],
            }],
        })),
    )
    .await;
    assert_eq!(status, 200, "the deletion got {status} {deleted}");
    assert_eq!(
        deleted["errors"].as_array().map(Vec::len),
        Some(0),
        "the deletion must apply its entry, got {deleted}"
    );

    // The dataset is at its next version now, so the listing is asked for THAT
    // version; the segments it reports are the fixture's without the deleted one.
    let version = deleted["datasetVersion"].as_str().unwrap_or_default();
    let body = listed(
        &router,
        &fixture.dataset_id,
        WORKSPACE,
        &[format!("datasetVersion={version}")],
    )
    .await;
    let remaining: Vec<Value> = fixture
        .expected()
        .into_iter()
        .filter(|row| row != &reported(&doomed))
        .collect();
    assert_eq!(
        segments(&body).iter().map(reported).collect::<Vec<Value>>(),
        remaining,
        "the listing must follow the deletion, got {body}"
    );
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
