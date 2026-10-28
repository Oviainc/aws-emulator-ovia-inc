// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDatasetDataSegmentRelationships`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rules these suites pin:
//!
//! * a relationship is reported for each data segment ANOTHER dataset of the
//!   workspace holds whose data came from the dataset the request names, each
//!   summary naming the dataset holding the reference, the dataset the data came
//!   from, and the time series and half-open span the referenced segment covers —
//!   all read off the stored segment, never off the request;
//! * a dataset's own segments name the dataset itself as their source and so are
//!   no relationship of it, a segment sourced from another dataset is that
//!   dataset's relationship and not this one's, and a holder of another workspace
//!   is out of the listing's scope;
//! * `maxResults`/`nextToken` page ONE stable enumeration: the pages joined are
//!   the whole listing once, in one order, whatever page size the caller asks
//!   for; a token this listing never issued is refused; and each published
//!   `maxResults` bound is served while one past it is refused;
//! * the dataset is resolved first and scoped by the `workspaceName` the request
//!   names, so one that resolves to nothing is the modelled 404 rather than an
//!   empty page, which would report absence as success;
//! * a listing REPORTS: it writes nothing, so it moves no dataset on in version
//!   or state, and the next listing follows whatever a disassociation did to the
//!   store.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `is_modelled_id`,
//! `router_with_engine`) is the one `asset_lifecycle` defines and the dataset
//! helpers (`create_dataset`, `ingest`, `instant`, `time_series_id`,
//! `wire_instant`) are the ones the data segment suites define; both are reused
//! rather than copied.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, DataSegmentRecord, RESOURCE_DATASET,
};

use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router_with_engine};
use crate::dataset_data_segments::{
    create_dataset, ingest, instant, time_series_id, wire_instant,
};

/// The workspace the datasets of these suites live in.
const WORKSPACE: &str = "relation-floor";

/// A second workspace, so a row can prove the listing is scoped by the workspace
/// the request names.
const OTHER_WORKSPACE: &str = "relation-roof";

/// The alias of the first seeded data stream.
const FIRST_ALIAS: &str = "/plant/mill/1/thickness";

/// The alias of the second, so a relationship of one series is told apart from a
/// relationship of another at the very same span.
const SECOND_ALIAS: &str = "/plant/mill/2/thickness";

/// The alias of the third, covered by the reference a dataset the store holds no
/// type for carries.
const THIRD_ALIAS: &str = "/plant/mill/3/thickness";

/// The sample seconds, relative to the suite's anchor, each stream is ingested
/// with — enough to create the stream the segments then cover.
const SAMPLES: &[i64] = &[0, 5, 10, 15, 20, 25, 30, 35, 40];

/// A `datasetId` no stored dataset carries.
const ABSENT_DATASET: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The published maximum of `maxResults` on this listing (`@range(min: 1, max:
/// 250)`; `smithy_query(query="operation",
/// operation="ListDatasetDataSegmentRelationships")`).
const MAX_RESULTS_MAX: usize = 250;

/// The page size this listing serves when the caller sends no `maxResults`, as
/// the model documents it ("Default: 50").
const DEFAULT_MAX_RESULTS: usize = 50;

/// Which seeded time series a fixture segment covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Series {
    /// The stream [`FIRST_ALIAS`] names.
    First,
    /// The stream [`SECOND_ALIAS`] names.
    Second,
    /// The stream [`THIRD_ALIAS`] names.
    Third,
}

/// Which dataset of the fixture a segment names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dataset {
    /// The `SESSION` dataset of [`WORKSPACE`] whose relationships these suites
    /// list.
    Session,
    /// A second `SESSION` dataset of the same workspace, so a relationship of
    /// one source is told apart from a relationship of another.
    OtherSession,
    /// A `CURATED` dataset of the same workspace, referencing two of the
    /// session dataset's series.
    CuratedA,
    /// A second `CURATED` dataset of the same workspace, so one source's
    /// relationships span more than one holder.
    CuratedB,
    /// A dataset of the same workspace the store holds NO type for, which an
    /// association is written to all the same, so a row can prove a holder's own
    /// type is not what states the relationship.
    Typeless,
    /// A `CURATED` dataset of [`OTHER_WORKSPACE`].
    Foreign,
    /// A `CURATED` dataset of [`WORKSPACE`] holding no segment at all.
    Empty,
}

/// How a fixture segment gets into the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Written {
    /// Through `BatchAssociateDataSegmentsToDataset`: the producer a curated
    /// dataset's references really come from.
    Associated,
    /// Through the producer entry point the state exposes, for the two segments
    /// the association operation refuses to write: a dataset's own segment
    /// (whose source is the dataset itself) and a holder in another workspace
    /// (an association resolves both datasets in ONE workspace).
    Seeded,
}

/// One data segment the fixture writes: the dataset that holds it, the dataset
/// its data came from, the series it covers and the half-open span it covers of
/// it (seconds relative to the suite's anchor plus a nanosecond offset).
#[derive(Debug, Clone, Copy)]
struct Segment {
    holder: Dataset,
    source: Dataset,
    series: Series,
    start: (i64, i32),
    end: (i64, i32),
    written: Written,
}

/// Every segment the fixture writes, spanning what a relationship listing has to
/// tell apart: two references to one source held by one dataset (of two series at
/// the same span), a reference held by a second dataset (its span carrying a
/// nanosecond offset), a reference held by a dataset of no stated type, a
/// reference whose data came from ANOTHER source, the source dataset's own
/// segment, and a reference held by a dataset of another workspace.
///
/// Segments of one series never overlap, whichever dataset holds them, which is
/// the rule the association producer enforces.
const SEGMENTS: &[Segment] = &[
    Segment {
        holder: Dataset::CuratedA,
        source: Dataset::Session,
        series: Series::First,
        start: (0, 0),
        end: (10, 0),
        written: Written::Associated,
    },
    Segment {
        holder: Dataset::CuratedA,
        source: Dataset::Session,
        series: Series::Second,
        start: (0, 0),
        end: (10, 0),
        written: Written::Associated,
    },
    Segment {
        holder: Dataset::CuratedB,
        source: Dataset::Session,
        series: Series::First,
        start: (10, 0),
        end: (20, 500),
        written: Written::Associated,
    },
    Segment {
        holder: Dataset::Typeless,
        source: Dataset::Session,
        series: Series::Third,
        start: (0, 0),
        end: (10, 0),
        written: Written::Associated,
    },
    Segment {
        holder: Dataset::CuratedA,
        source: Dataset::OtherSession,
        series: Series::First,
        start: (30, 0),
        end: (40, 0),
        written: Written::Associated,
    },
    Segment {
        holder: Dataset::Session,
        source: Dataset::Session,
        series: Series::Second,
        start: (20, 0),
        end: (30, 0),
        written: Written::Seeded,
    },
    Segment {
        holder: Dataset::Foreign,
        source: Dataset::Session,
        series: Series::Third,
        start: (20, 0),
        end: (30, 0),
        written: Written::Seeded,
    },
];

/// The ids the fixture's datasets and streams were minted with.
#[derive(Debug, Clone)]
struct Fixture {
    session: String,
    other_session: String,
    curated_a: String,
    curated_b: String,
    typeless: String,
    foreign: String,
    empty: String,
    first: String,
    second: String,
    third: String,
}

impl Fixture {
    /// The `datasetId` `which` was minted with.
    fn dataset(&self, which: Dataset) -> &str {
        match which {
            Dataset::Session => &self.session,
            Dataset::OtherSession => &self.other_session,
            Dataset::CuratedA => &self.curated_a,
            Dataset::CuratedB => &self.curated_b,
            Dataset::Typeless => &self.typeless,
            Dataset::Foreign => &self.foreign,
            Dataset::Empty => &self.empty,
        }
    }

    /// The workspace `which` was created in.
    fn workspace(&self, which: Dataset) -> &str {
        match which {
            Dataset::Foreign => OTHER_WORKSPACE,
            _ => WORKSPACE,
        }
    }

    /// The `TimeSeriesId` the store minted for `series`.
    fn series(&self, series: Series) -> &str {
        match series {
            Series::First => &self.first,
            Series::Second => &self.second,
            Series::Third => &self.third,
        }
    }

    /// The whole enumeration a listing of `subject` in `workspace` must report,
    /// in the one order it is paged in.
    ///
    /// An INDEPENDENT oracle, built from [`SEGMENTS`] rather than from a
    /// listing: a relationship is a segment another dataset of the workspace
    /// holds whose data came from the subject, and the order is on the holder
    /// and the referenced segment's identity inside it.
    fn expected(&self, subject: Dataset, workspace: &str) -> Vec<Value> {
        let mut rows: Vec<Value> = SEGMENTS
            .iter()
            .filter(|segment| segment.source == subject)
            .filter(|segment| segment.holder != subject)
            .filter(|segment| self.workspace(segment.holder) == workspace)
            .map(|segment| {
                json!({
                    "targetDatasetId": self.dataset(segment.holder),
                    "sourceDatasetId": self.dataset(segment.source),
                    "timeSeriesId": self.series(segment.series),
                    "startTimestamp": wire_instant(segment.start.0, segment.start.1),
                    "endTimestamp": wire_instant(segment.end.0, segment.end.1),
                })
            })
            .collect();
        rows.sort_by_key(sort_key);
        rows
    }
}

/// The order a relationship listing reports in: the dataset holding the
/// reference, then the referenced segment's time series and its own span.
fn sort_key(row: &Value) -> (String, String, i64, i64, i64, i64) {
    (
        row["targetDatasetId"].as_str().unwrap_or_default().to_string(),
        row["timeSeriesId"].as_str().unwrap_or_default().to_string(),
        row["startTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
        row["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
        row["endTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
        row["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
    )
}

/// A `nextToken` in the form this listing issues: the hex of the identity of the
/// relationship the next page resumes from, through the hand-rolled hex oracle
/// `asset_lifecycle` already defines for the tokens of this service's listings.
///
/// Hand-rolled on purpose: the handler encodes its token with the `hex` crate, so
/// re-using that crate here would let one encoding bug agree with itself and pass.
fn token_of(summary: &Value) -> String {
    let key = format!(
        "{}|{}|{}.{}|{}.{}",
        summary["targetDatasetId"].as_str().unwrap_or_default(),
        summary["timeSeriesId"].as_str().unwrap_or_default(),
        summary["startTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
        summary["startTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
        summary["endTimestamp"]["timeInSeconds"].as_i64().unwrap_or_default(),
        summary["endTimestamp"]["offsetInNanos"].as_i64().unwrap_or_default(),
    );
    hex_token(&key)
}

/// `GET /datasets/{datasetId}/data-segment-relationships` with the query `extra`
/// adds.
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
        &format!("/datasets/{dataset_id}/data-segment-relationships?{query}"),
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

/// The `dataSegmentRelationshipSummaries` of a listing body.
fn summaries(body: &Value) -> Vec<Value> {
    body["dataSegmentRelationshipSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("a listing carries dataSegmentRelationshipSummaries, got {body}")
        })
        .clone()
}

/// One reported summary reduced to the members the oracle states, so a comparison
/// names the difference rather than the whole body.
fn reported(summary: &Value) -> Value {
    json!({
        "targetDatasetId": summary["targetDatasetId"],
        "sourceDatasetId": summary["sourceDatasetId"],
        "timeSeriesId": summary["timeSeriesId"],
        "startTimestamp": summary["startTimestamp"],
        "endTimestamp": summary["endTimestamp"],
    })
}

/// The whole enumeration of `subject`'s relationships, reported in one page of
/// the published maximum.
async fn whole_listing(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: &str,
) -> Vec<Value> {
    let body = listed(
        router,
        dataset_id,
        workspace,
        &[format!("maxResults={MAX_RESULTS_MAX}")],
    )
    .await;
    assert_eq!(
        body["nextToken"].as_str(),
        None,
        "a listing that reported everything issues no token, got {body}"
    );
    summaries(&body).iter().map(reported).collect()
}

/// `POST /datasets/{datasetId}/data-segments/associate` with one entry, asserting
/// the association was applied.
async fn associate(router: &MultiServiceRouter, fixture: &Fixture, segment: &Segment) {
    let (status, body) = call(
        router,
        "POST",
        &format!(
            "/datasets/{}/data-segments/associate",
            fixture.dataset(segment.holder)
        ),
        Some(json!({
            "workspaceName": fixture.workspace(segment.holder),
            "associateDataSegmentEntries": [wire_entry(fixture, segment)],
        })),
    )
    .await;
    assert_eq!(status, 200, "associate {segment:?}: got {status} {body}");
    assert_eq!(
        body["failedAssociations"].as_array().map(Vec::len),
        Some(0),
        "associate {segment:?}: the entry must be applied, got {body}"
    );
}

/// `POST /datasets/{datasetId}/data-segments/disassociate` with one entry.
async fn disassociate(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    segment: &Segment,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!(
            "/datasets/{}/data-segments/disassociate",
            fixture.dataset(segment.holder)
        ),
        Some(json!({
            "workspaceName": fixture.workspace(segment.holder),
            "disassociateDataSegmentEntries": [wire_entry(fixture, segment)],
        })),
    )
    .await
}

/// One entry on the wire — the four members an association and a disassociation
/// address a segment by.
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

/// Write the [`Written::Seeded`] segments straight through the producer entry
/// point the state exposes: the association operation refuses both, so this is
/// the only way the store comes to hold them.
fn seed_segments(engine: &Arc<engine::InMemoryEngine>, fixture: &Fixture) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for segment in SEGMENTS
        .iter()
        .filter(|segment| segment.written == Written::Seeded)
    {
        let stored = store.put_dataset_data_segment(
            fixture.dataset(segment.holder),
            DataSegmentRecord {
                source_dataset_id: fixture.dataset(segment.source).to_string(),
                time_series_id: fixture.series(segment.series).to_string(),
                start_timestamp: instant(segment.start.0, segment.start.1),
                end_timestamp: instant(segment.end.0, segment.end.1),
            },
        );
        assert!(stored, "seeding {segment:?} must be accepted");
    }
}

/// The fixture: the seven datasets, the three data streams their segments cover,
/// and every segment of [`SEGMENTS`] written.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Fixture) {
    let (router, engine) = router_with_engine();

    let fixture = Fixture {
        session: create_dataset(&router, "Mill session", Some("SESSION"), Some(WORKSPACE)).await,
        other_session: create_dataset(&router, "Mill source", Some("SESSION"), Some(WORKSPACE))
            .await,
        curated_a: create_dataset(&router, "Mill curated A", Some("CURATED"), Some(WORKSPACE))
            .await,
        curated_b: create_dataset(&router, "Mill curated B", Some("CURATED"), Some(WORKSPACE))
            .await,
        typeless: create_dataset(&router, "Mill untyped", None, Some(WORKSPACE)).await,
        foreign: create_dataset(
            &router,
            "Mill curated abroad",
            Some("CURATED"),
            Some(OTHER_WORKSPACE),
        )
        .await,
        empty: create_dataset(&router, "Mill curated empty", Some("CURATED"), Some(WORKSPACE))
            .await,
        first: String::new(),
        second: String::new(),
        third: String::new(),
    };

    for alias in [FIRST_ALIAS, SECOND_ALIAS, THIRD_ALIAS] {
        ingest(&router, alias, SAMPLES).await;
    }
    let fixture = Fixture {
        first: time_series_id(&router, FIRST_ALIAS).await,
        second: time_series_id(&router, SECOND_ALIAS).await,
        third: time_series_id(&router, THIRD_ALIAS).await,
        ..fixture
    };
    assert!(
        is_modelled_id(&fixture.first)
            && is_modelled_id(&fixture.second)
            && is_modelled_id(&fixture.third),
        "a disassociated stream's timeSeriesId must be a modelled ID, got {fixture:?}"
    );

    for segment in SEGMENTS
        .iter()
        .filter(|segment| segment.written == Written::Associated)
    {
        associate(&router, &fixture, segment).await;
    }
    seed_segments(&engine, &fixture);

    (router, engine, fixture)
}

#[tokio::test]
async fn every_relationship_is_read_off_the_segment_the_referencing_dataset_holds() {
    let (router, _engine, fixture) = seeded().await;

    // Rows differ in which dataset the request names: the session dataset two
    // curated datasets and an untyped one reference, the second session dataset
    // one of them references, a curated dataset holding references but referenced
    // by nobody, and a dataset holding no segment at all.
    let subjects = [
        Dataset::Session,
        Dataset::OtherSession,
        Dataset::CuratedA,
        Dataset::Typeless,
        Dataset::Empty,
    ];
    for subject in subjects {
        let dataset_id = fixture.dataset(subject);
        let got = whole_listing(&router, dataset_id, WORKSPACE).await;
        let expected = fixture.expected(subject, WORKSPACE);
        assert_eq!(
            got.len(),
            expected.len(),
            "case {subject:?}: expected {} relationships, got {got:?}",
            expected.len()
        );
        for (summary, wanted) in got.iter().zip(expected.iter()) {
            assert_eq!(
                summary, wanted,
                "case {subject:?}: expected {wanted}, got {summary}"
            );
        }
        // Every reported relationship is one OF the dataset named, and names a
        // holder other than it.
        for summary in &got {
            assert_eq!(
                summary["sourceDatasetId"].as_str(),
                Some(dataset_id),
                "case {subject:?}: every relationship is sourced from the dataset named, \
                 got {summary}"
            );
            assert_ne!(
                summary["targetDatasetId"].as_str(),
                Some(dataset_id),
                "case {subject:?}: a dataset's own segments reference nobody, got {summary}"
            );
        }
    }
}

#[tokio::test]
async fn the_listing_pages_one_stable_enumeration_whatever_the_page_size() {
    let (router, _engine, fixture) = seeded().await;

    let reference = whole_listing(&router, &fixture.session, WORKSPACE).await;
    let entries = reference.len();
    assert!(
        entries >= 4,
        "the fixture must hold enough relationships to page, got {reference:?}"
    );

    // Rows differ in page size: the published minimum, sizes that divide the
    // enumeration and sizes that do not, exactly it, one past it, the published
    // maximum, and no `maxResults` at all (the documented default of 50).
    let page_sizes = [
        Some(1_usize),
        Some(2),
        Some(3),
        Some(entries),
        Some(entries + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("page size {page_size:?}");
        let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS);
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut extra: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                extra.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                extra.push(format!("nextToken={token}"));
            }
            let body = listed(&router, &fixture.session, WORKSPACE, &extra).await;
            let page: Vec<Value> = summaries(&body).iter().map(reported).collect();
            pages += 1;
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} entries, got {} in {body}",
                page.len()
            );
            seen.extend(page.iter().cloned());
            token = body["nextToken"].as_str().map(str::to_string);
            match &token {
                // A token is issued only while an entry follows the page, and a
                // page another entry follows is a FULL page resuming at the entry
                // after it.
                Some(issued) => {
                    assert_eq!(
                        page.len(),
                        limit,
                        "case {case:?}: a page followed by a token must be full, got {body}"
                    );
                    let next = seen.len();
                    assert_eq!(
                        *issued,
                        token_of(&reference[next]),
                        "case {case:?}: the token must resume at entry {next}, got {body}"
                    );
                }
                None => break,
            }
            assert!(
                pages <= entries + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, reference,
            "case {case:?}: the pages joined must be the whole enumeration once, in one order"
        );
        let expected_pages = entries.div_ceil(limit);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages over {entries} relationships, \
             walked {pages}"
        );
    }
}

#[tokio::test]
async fn a_token_this_listing_never_issued_and_a_page_size_outside_the_range_are_refused() {
    let (router, _engine, fixture) = seeded().await;
    let reference = whole_listing(&router, &fixture.session, WORKSPACE).await;

    // Rows differ in HOW the token is wrong: not hex at all, an odd-length hex
    // run, the hex of a relationship identity no entry of THIS enumeration
    // carries, and the hex of text that is no identity at all.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        ("an odd-length hex run", "abc".to_string()),
        (
            "the hex of an identity this enumeration does not carry",
            hex_token(&format!(
                "{}|{}|0.0|1.0",
                fixture.curated_a, fixture.first
            )),
        ),
        (
            "the hex of text that is no identity",
            hex_token("no-such-entry"),
        ),
    ];
    for (how, token) in &forged {
        let (status, body) =
            list_raw(&router, &fixture.session, WORKSPACE, &[format!("nextToken={token}")]).await;
        assert_eq!(
            status, 400,
            "case {how:?}: a forged nextToken must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {how:?}: unexpected error code in {body}"
        );
    }

    // Each published `maxResults` bound is served, and one past either is refused
    // by the modelled range before the handler runs.
    for size in [1_usize, MAX_RESULTS_MAX] {
        let body = listed(
            &router,
            &fixture.session,
            WORKSPACE,
            &[format!("maxResults={size}")],
        )
        .await;
        assert!(
            !summaries(&body).is_empty(),
            "case maxResults={size}: a published bound is served, got {body}"
        );
    }
    for size in [0_usize, MAX_RESULTS_MAX + 1] {
        let (status, body) = list_raw(
            &router,
            &fixture.session,
            WORKSPACE,
            &[format!("maxResults={size}")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={size}: a page size outside the modelled range is refused, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case maxResults={size}: unexpected error code in {body}"
        );
    }

    // The refusals changed nothing a later caller sees.
    let again = whole_listing(&router, &fixture.session, WORKSPACE).await;
    assert_eq!(
        again, reference,
        "the refusals must leave the enumeration as it was, got {again:?}"
    );
}

#[tokio::test]
async fn a_dataset_that_resolves_to_nothing_is_the_modelled_not_found() {
    let (router, _engine, fixture) = seeded().await;
    let no_workspace = create_dataset(&router, "Mill loose", Some("EXTERNAL"), None).await;

    // Rows differ in WHY the dataset the request names resolves to nothing: no
    // dataset carries the id, the dataset is held by another workspace, and the
    // dataset is held by no workspace at all.
    let cases = [
        ("an id no dataset carries", ABSENT_DATASET, WORKSPACE),
        (
            "a dataset of another workspace",
            fixture.session.as_str(),
            OTHER_WORKSPACE,
        ),
        (
            "a dataset of no workspace",
            no_workspace.as_str(),
            WORKSPACE,
        ),
    ];
    for (case, dataset_id, workspace) in cases {
        let (status, body) = list_raw(&router, dataset_id, workspace, &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or_default(),
            not_found_message(RESOURCE_DATASET, dataset_id),
            "case {case:?}: the message names the dataset the request asked for, got {body}"
        );
    }
}

#[tokio::test]
async fn the_listing_reports_and_follows_the_store() {
    let (router, _engine, fixture) = seeded().await;

    let before = describe_dataset(&router, &fixture.session, WORKSPACE).await;
    let reference = whole_listing(&router, &fixture.session, WORKSPACE).await;

    // A read writes nothing: the dataset stands where the creates and the
    // associations left it, whatever the listing was asked for.
    for extra in [vec![], vec!["maxResults=1".to_string()]] {
        listed(&router, &fixture.session, WORKSPACE, &extra).await;
    }
    let after = describe_dataset(&router, &fixture.session, WORKSPACE).await;
    for member in ["datasetVersion", "datasetStatus", "datasetName"] {
        assert_eq!(
            after[member], before[member],
            "a listing must not move the dataset's {member}, got {after}"
        );
    }
    let again = whole_listing(&router, &fixture.session, WORKSPACE).await;
    assert_eq!(
        again, reference,
        "two listings of one store must agree, got {again:?}"
    );

    // And the next listing follows whatever the producer side did: the reference
    // a disassociation removed is gone, and only that one.
    let removed = SEGMENTS
        .iter()
        .find(|segment| {
            segment.written == Written::Associated
                && segment.source == Dataset::Session
                && segment.holder == Dataset::CuratedA
        })
        .expect("the fixture associates a reference held by the first curated dataset");
    let (status, body) = disassociate(&router, &fixture, removed).await;
    assert_eq!(status, 200, "disassociate {removed:?}: got {status} {body}");
    assert_eq!(
        body["failedDisassociations"].as_array().map(Vec::len),
        Some(0),
        "disassociate {removed:?}: the entry must be applied, got {body}"
    );

    let gone = json!({
        "targetDatasetId": fixture.dataset(removed.holder),
        "sourceDatasetId": fixture.dataset(removed.source),
        "timeSeriesId": fixture.series(removed.series),
        "startTimestamp": wire_instant(removed.start.0, removed.start.1),
        "endTimestamp": wire_instant(removed.end.0, removed.end.1),
    });
    let listed_now = whole_listing(&router, &fixture.session, WORKSPACE).await;
    let expected: Vec<Value> = reference
        .iter()
        .filter(|summary| **summary != gone)
        .cloned()
        .collect();
    assert_eq!(
        expected.len(),
        reference.len() - 1,
        "the disassociated reference must be one the listing reported, got {reference:?}"
    );
    assert_eq!(
        listed_now, expected,
        "the listing must follow the store the disassociation left, got {listed_now:?}"
    );
}
