// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` of a time-ranged read belongs to the REQUEST that minted it.
//!
//! The rule these suites pin: a paginated read honours a `nextToken` only against a
//! request asking for the same enumeration — the same stream or source, the same
//! range, the same filters and the same ordering — and refuses a token minted by any
//! other request with the modelled `InvalidRequestException` (HTTP 400), serving no
//! page at all. A resume point on its own (an instant, a window start) names a
//! position in every enumeration that happens to hold it, and the readings of several
//! properties are written at the SAME timestamps, so without that binding a token
//! would silently serve a slice of another enumeration and drop everything before the
//! point it resumed at.
//!
//! What does NOT change the enumeration does not break a token: the page size, and
//! naming the same stream or source by its other identifying form.
//!
//! The rows span the readers of this service that page by the instant they stopped
//! at — `GetAssetPropertyValueHistory`, `GetAssetPropertyAggregates` and
//! `GetCaptureData` — and, for each, the request members the rule generalises over.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, …) is the one the
//! asset suite already has; this suite reuses it rather than growing a second.

use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, CaptureSegment};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router_with_engine,
};

/// The epoch this suite's history and aggregate tables measure their seconds from.
///
/// `BatchPutAssetPropertyValue` ingests only TQVs inside `[now - 7 days, now + 10
/// minutes]`, so the shared `super::support` anchoring maps these small seconds into
/// the accepted window at the wire boundary.
const ROW_EPOCH: i64 = 0;

/// A table second as the absolute `timeInSeconds` the wire carries.
fn anchored(seconds: i64) -> i64 {
    super::support::anchored(ROW_EPOCH, seconds)
}

/// The inverse of [`anchored`].
fn relative(seconds: i64) -> i64 {
    super::support::relative(ROW_EPOCH, seconds)
}

/// A table second as the RFC 3339 form an `@httpQuery` timestamp travels as:
/// `startDate` and `endDate` carry no `@timestampFormat`, so they use Smithy's
/// default `date-time`.
fn query_date(seconds: i64) -> String {
    DateTime::from_secs(anchored(seconds))
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

/// The modelled `Quality` values a filter may name. Source:
/// `smithy_query(query="shape", shape="Quality")` — GOOD | BAD | UNCERTAIN.
const QUALITIES: [&str; 3] = ["GOOD", "BAD", "UNCERTAIN"];

/// An asset carrying two DOUBLE measurement properties; answers
/// `(assetId, firstPropertyId, secondPropertyId)`.
async fn asset_with_two_properties(
    router: &MultiServiceRouter,
    name: &str,
    first: &str,
    second: &str,
) -> (String, String, String) {
    let model_id = create_model(
        router,
        &format!("{name}Model"),
        json!([
            {"name": first, "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": second, "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(router, name, &model_id).await;
    let (status, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset({name}): got {status} {asset}");
    let first_id = property_id(&asset["assetProperties"], first);
    let second_id = property_id(&asset["assetProperties"], second);
    (asset_id, first_id, second_id)
}

/// Write `samples` — `(table second, quality)`, a `None` quality meaning the write
/// carries none — to every `(assetId, propertyId)` in `streams`, so the SAME instants
/// land in each of them, which is what a single `BatchPutAssetPropertyValue` call
/// does.
async fn write_to_each(
    router: &MultiServiceRouter,
    streams: &[(&str, &str)],
    samples: &[(i64, Option<&str>)],
) {
    let entries: Vec<Value> = streams
        .iter()
        .enumerate()
        .map(|(at, (asset_id, property))| {
            let values: Vec<Value> = samples
                .iter()
                .map(|(seconds, quality)| {
                    let mut sample = json!({
                        "value": {"doubleValue": *seconds as f64 / 10.0},
                        "timestamp": {"timeInSeconds": anchored(*seconds), "offsetInNanos": 0},
                    });
                    if let Some(quality) = quality {
                        sample["quality"] = json!(quality);
                    }
                    sample
                })
                .collect();
            json!({
                "entryId": format!("e{at}"),
                "assetId": asset_id,
                "propertyId": property,
                "propertyValues": values,
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
    assert_eq!(status, 200, "writing samples: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "writing samples: every entry must be accepted, got {body}"
    );
}

/// `GET /properties/history?<query>`.
async fn history(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/properties/history?{query}"), None).await
}

/// The table seconds of the samples a history answer reports, in the order reported.
fn reported_seconds(case: &str, body: &Value) -> Vec<i64> {
    body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {body}"))
        .iter()
        .map(|sample| {
            relative(
                sample["timestamp"]["timeInSeconds"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("case {case:?}: a sample with no timestamp: {body}")),
            )
        })
        .collect()
}

/// Mint a first-page `nextToken` from the history read `query` names, asserting the
/// page it answers is the one sample the query asked for.
async fn mint_history_token(router: &MultiServiceRouter, case: &str, query: &str) -> String {
    let (status, body) = history(router, &format!("{query}&maxResults=1")).await;
    assert_eq!(status, 200, "minting {case:?}: got {status} {body}");
    assert_eq!(
        reported_seconds(case, &body).len(),
        1,
        "minting {case:?}: expected a one-sample page, got {body}"
    );
    body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting {case:?}: expected a nextToken, got {body}"))
        .to_string()
}

/// Assert `body`/`status` is the modelled refusal of a token this request never
/// issued, carrying no page at all.
fn assert_token_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for a token minted by another request, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {body}"
    );
    assert!(
        body.get("assetPropertyValueHistory").is_none()
            && body.get("aggregatedValues").is_none()
            && body.get("data").is_none(),
        "case {case:?}: a refused token must serve no page, got {body}"
    );
}

#[tokio::test]
async fn a_history_token_is_honoured_only_by_the_request_that_minted_it() {
    let (router, engine) = router_with_engine();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Turbine1", "Speed", "Torque").await;
    let (other_asset, other_speed, _) =
        asset_with_two_properties(&router, "Turbine2", "Speed", "Torque").await;

    // One batch, the same instants in every stream — the shape the issue's own
    // reproduction takes, and the ordinary shape of industrial ingest.
    let written: [(i64, Option<&str>); 5] = [
        (100, None),
        (200, None),
        (250, Some("BAD")),
        (300, None),
        (400, None),
    ];
    write_to_each(
        &router,
        &[
            (&asset_id, &speed),
            (&asset_id, &torque),
            (&other_asset, &other_speed),
        ],
        &written,
    )
    .await;

    // The alias form names the same stream as the ids do. The operations that bind an
    // alias are still 501 scaffolds, so the index is seeded in the one shared store
    // the handlers read.
    let alias = "/company/windfarm/3/turbine/1/speed";
    service_state(&*engine)
        .expect("the service store")
        .lock()
        .property_alias_index
        .insert(alias.to_string(), (asset_id.clone(), speed.clone()));

    let minted = format!("assetId={asset_id}&propertyId={speed}");
    let token = mint_history_token(&router, "the plain read", &minted).await;

    // Rows that ask for the SAME enumeration: the token resumes at the sample after
    // the first page, whatever page size or identifying form the request uses.
    let honoured: Vec<(&str, String)> = vec![
        ("the request that minted it", format!("{minted}&maxResults=1")),
        ("a wider page", format!("{minted}&maxResults=4")),
        ("no page size at all", minted.clone()),
        (
            "the ordering named explicitly, as the default",
            format!("{minted}&timeOrdering=ASCENDING&maxResults=1"),
        ),
        ("the same stream by its alias", format!("propertyAlias={alias}")),
    ];
    for (case, query) in honoured {
        let (status, body) = history(&router, &format!("{query}&nextToken={token}")).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let reported = reported_seconds(case, &body);
        assert_eq!(
            reported.first().copied(),
            Some(200),
            "case {case:?}: expected the page to resume at 200, got {reported:?}"
        );
    }

    // Rows that ask for ANOTHER enumeration. Each one holds a sample at the resumed
    // instant, so each would be served from that offset — dropping every earlier
    // sample of it — were the token not bound to the request that minted it.
    let refused: Vec<(&str, String)> = vec![
        (
            "another property of the same asset",
            format!("assetId={asset_id}&propertyId={torque}"),
        ),
        (
            "the same property name on another asset",
            format!("assetId={other_asset}&propertyId={other_speed}"),
        ),
        (
            "the ordering flipped",
            format!("{minted}&timeOrdering=DESCENDING"),
        ),
        (
            "a quality filter the minting read did not carry",
            format!("{minted}&qualities=GOOD"),
        ),
        (
            "a start bound the minting read did not carry",
            format!("{minted}&startDate={}", query_date(150)),
        ),
        (
            "an end bound the minting read did not carry",
            format!("{minted}&endDate={}", query_date(300)),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = history(&router, &format!("{query}&nextToken={token}")).await;
        assert_token_refused(case, status, &body);
    }

    // The same binding the other way round: a token minted by a request that names a
    // filter, an ordering or a window is no token of the plain read either. Rows span
    // the whole `Quality` catalog, including the values the issue never cites.
    let mut variants: Vec<(String, String)> = vec![
        (
            "minted DESCENDING".to_string(),
            format!("{minted}&timeOrdering=DESCENDING"),
        ),
        (
            "minted over a narrower window".to_string(),
            format!(
                "{minted}&startDate={}&endDate={}",
                query_date(99),
                query_date(300)
            ),
        ),
    ];
    for quality in QUALITIES {
        // Every quality has at least two samples to page through: BAD has the one
        // written as BAD plus — for GOOD and UNCERTAIN — none, so only the qualities
        // the writes cover can mint a token.
        variants.push((
            format!("minted qualities={quality}"),
            format!("{minted}&qualities={quality}"),
        ));
    }
    for (case, query) in variants {
        let (status, body) = history(&router, &format!("{query}&maxResults=1")).await;
        assert_eq!(status, 200, "minting {case:?}: got {status} {body}");
        let Some(variant_token) = body["nextToken"].as_str() else {
            // A variant whose enumeration holds one sample or none issues no token,
            // so there is nothing to replay for it.
            continue;
        };
        let (status, replayed) =
            history(&router, &format!("{minted}&nextToken={variant_token}")).await;
        assert_token_refused(&case, status, &replayed);
        // ... and it still resumes its OWN enumeration.
        let (status, resumed) =
            history(&router, &format!("{query}&nextToken={variant_token}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: its own request must still resume it, got {status} {resumed}"
        );
        assert!(
            !reported_seconds(&case, &resumed).is_empty(),
            "case {case:?}: its own request must be answered a page, got {resumed}"
        );
    }
}

/// `GET /properties/aggregates?...` for one property over `(start, end]` at
/// `resolution`.
fn aggregates_query(
    asset_id: &str,
    property: &str,
    resolution: &str,
    start: i64,
    end: i64,
) -> String {
    format!(
        "assetId={asset_id}&propertyId={property}&resolution={resolution}\
         &aggregateTypes=COUNT&startDate={}&endDate={}",
        query_date(start),
        query_date(end)
    )
}

/// The epoch seconds of the windows an aggregate answer reports, in order.
///
/// Absolute, not table seconds: the window grid is aligned to the Unix epoch, so
/// where a window starts is decided by that grid and not by this suite's own epoch.
fn reported_windows(case: &str, body: &Value) -> Vec<i64> {
    body["aggregatedValues"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {body}"))
        .iter()
        .map(|aggregate| {
            aggregate["timestamp"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: an aggregate with no timestamp"))
                as i64
        })
        .collect()
}

#[tokio::test]
async fn an_aggregates_token_is_honoured_only_by_the_request_that_minted_it() {
    let router = crate::asset_lifecycle::router();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Mixer1", "Speed", "Torque").await;

    // Readings in three distinct `1m` windows, written to both properties at the same
    // instants: the window grid is aligned to the epoch, so the two enumerations carry
    // exactly the same window starts.
    let written: [(i64, Option<&str>); 3] = [(0, None), (120, None), (240, None)];
    write_to_each(
        &router,
        &[(&asset_id, &speed), (&asset_id, &torque)],
        &written,
    )
    .await;

    let minted = aggregates_query(&asset_id, &speed, "1m", -60, 300);

    // What the whole enumeration is, so the resumed page can be checked against the
    // window that really follows the first one.
    let (status, whole) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the unpaginated read: got {status} {whole}");
    let windows = reported_windows("the unpaginated read", &whole);
    assert!(
        windows.len() >= 3,
        "the unpaginated read: expected at least three windows, got {windows:?}"
    );

    let (status, first) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}&maxResults=1"),
        None,
    )
    .await;
    assert_eq!(status, 200, "minting an aggregates token: got {status} {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting an aggregates token: expected one, got {first}"))
        .to_string();

    // The request that minted it resumes at the next window; a request naming another
    // enumeration is refused, even though the window start it carries exists there.
    let (status, resumed) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}&nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the minting request: got {status} {resumed}");
    assert_eq!(
        reported_windows("the minting request", &resumed)
            .first()
            .copied(),
        windows.get(1).copied(),
        "the minting request: expected the page to resume at the second window, got {resumed}"
    );

    let refused: Vec<(&str, String)> = vec![
        (
            "another property of the same asset",
            aggregates_query(&asset_id, &torque, "1m", -60, 300),
        ),
        (
            "the ordering flipped",
            format!("{minted}&timeOrdering=DESCENDING"),
        ),
        (
            "a quality filter the minting read did not carry",
            format!("{minted}&qualities=BAD"),
        ),
        (
            "a wider range",
            aggregates_query(&asset_id, &speed, "1m", -120, 300),
        ),
        (
            "a coarser resolution",
            aggregates_query(&asset_id, &speed, "1h", -60, 300),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/properties/aggregates?{query}&nextToken={token}"),
            None,
        )
        .await;
        assert_token_refused(case, status, &body);
    }
}

/// The workspace this suite's capture sources live in.
const WORKSPACE: &str = "token-binding-floor";

/// The `VideoDataType` of the seeded media. Source:
/// `smithy_query(query="shape", shape="VideoDataType")` — the enum holds exactly this
/// one value.
const CAPTURE_DATA_TYPE: &str = "VIDEO-MP4";

/// A modelled `TimeInNanos`.
fn time_in_nanos(seconds: i64) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: seconds,
        offset_in_nanos: Some(0),
    }
}

#[tokio::test]
async fn a_capture_data_token_is_honoured_only_by_the_request_that_minted_it() {
    let (router, engine) = router_with_engine();
    super::support::ensure_workspace(&router, WORKSPACE).await;

    // Two capture sources of ONE workspace whose segments start at the same instants —
    // two cameras of one line, started together.
    let first_source = "4c1d6f28-7a93-4b55-9e01-2d8f6c3a5b74";
    let second_source = "9b2e7a31-5c48-4f60-8d13-7e9a4c2b6f05";
    let first_alias = "/company/line/1/camera/a";
    let starts = [1_700_000_100_i64, 1_700_000_200, 1_700_000_300];
    {
        let state = service_state(&*engine).expect("the service store");
        let mut store = state.lock();
        for (source, alias) in [(first_source, Some(first_alias)), (second_source, None)] {
            for (at, start) in starts.iter().enumerate() {
                let stored = store.put_capture_segment(
                    WORKSPACE,
                    source,
                    alias,
                    CAPTURE_DATA_TYPE,
                    CaptureSegment {
                        start_time: time_in_nanos(*start),
                        end_time: time_in_nanos(start + 10),
                        data: vec![at as u8 + 1; 4],
                    },
                    SystemTime::now(),
                );
                assert_eq!(
                    stored,
                    Ok(()),
                    "seeding {source} at {start}: the producer entry point refused it"
                );
            }
        }
    }

    let uri = format!("/workspaces/{WORKSPACE}/get-capture-data");
    let window = |start: i64, end: i64| {
        json!({
            "startTime": {"timeInSeconds": start, "offsetInNanos": 0},
            "endTime": {"timeInSeconds": end, "offsetInNanos": 0},
        })
    };
    let request = |source: Option<&str>, alias: Option<&str>, start: i64, end: i64| {
        let mut body = window(start, end);
        if let Some(source) = source {
            body["timeSeriesId"] = json!(source);
        }
        if let Some(alias) = alias {
            body["propertyAlias"] = json!(alias);
        }
        body
    };

    let minting = request(Some(first_source), None, 1_700_000_000, 1_700_000_400);
    let (status, first) = call(&router, "POST", &uri, Some(minting.clone())).await;
    assert_eq!(status, 200, "minting a capture token: got {status} {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting a capture token: expected one, got {first}"))
        .to_string();

    // The requests that ask for the SAME enumeration — including the alias form, which
    // names the same source — resume at the second segment.
    for (case, mut body) in [
        ("the request that minted it", minting.clone()),
        (
            "the same source by its alias",
            request(None, Some(first_alias), 1_700_000_000, 1_700_000_400),
        ),
    ] {
        body["nextToken"] = json!(token);
        let (status, resumed) = call(&router, "POST", &uri, Some(body)).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {resumed}");
        assert_eq!(
            resumed["startTime"]["timeInSeconds"].as_i64(),
            Some(starts[1]),
            "case {case:?}: expected the chunk starting at {}, got {resumed}",
            starts[1]
        );
    }

    // Another source of the same workspace holds a segment starting at the resumed
    // instant, and a narrower window selects a different run of this source's
    // segments: neither is the enumeration the token was minted in.
    for (case, mut body) in [
        (
            "another source of the same workspace",
            request(Some(second_source), None, 1_700_000_000, 1_700_000_400),
        ),
        (
            "a narrower window",
            request(Some(first_source), None, 1_700_000_150, 1_700_000_400),
        ),
    ] {
        body["nextToken"] = json!(token);
        let (status, refused) = call(&router, "POST", &uri, Some(body)).await;
        assert_token_refused(case, status, &refused);
    }
}
