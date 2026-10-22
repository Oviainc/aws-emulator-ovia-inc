// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetAssetPropertyValueHistory`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: the operation reports the samples the shared store
//! holds for the ONE property the request identifies, keeping those inside the
//! half-open window `(startDate, endDate]`, keeping only the qualities the request
//! names, ordering them by timestamp as `timeOrdering` asks (ASCENDING by default)
//! and cutting the ordered run into `maxResults`-sized pages joined by `nextToken`.
//! The recorded ordering case is ONE row of each table below.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use super::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router, router_with_engine,
};

/// Every `Quality` the model declares. Source:
/// `smithy_query(query="shape", shape="Quality")` — GOOD | BAD | UNCERTAIN.
const QUALITIES: [&str; 3] = ["GOOD", "BAD", "UNCERTAIN"];

/// The documented ceiling on `maxResults`, from the model doc on
/// `GetAssetPropertyValueHistoryRequest$maxResults`: "The maximum value of
/// `maxResults` is 20000."
const MAX_RESULTS: i64 = 20_000;

/// The epoch this suite's tables measure their seconds from.
///
/// They name small seconds (100..9000) rather than calendar instants, and
/// `BatchPutAssetPropertyValue` ingests only TQVs inside
/// `[now - 7 days, now + 10 minutes]`, so the shared `super::support` anchoring
/// maps this epoch into the accepted window at the wire boundary.
const ROW_EPOCH: i64 = 0;

/// A table second as the absolute `timeInSeconds` the wire carries.
fn anchored(seconds: i64) -> i64 {
    super::support::anchored(ROW_EPOCH, seconds)
}

/// The inverse of [`anchored`].
fn relative(seconds: i64) -> i64 {
    super::support::relative(ROW_EPOCH, seconds)
}

/// The RFC 3339 form a query-bound timestamp takes on the wire: `startDate` and
/// `endDate` carry no `@timestampFormat`, so an `@httpQuery` timestamp uses
/// Smithy's default `date-time` format.
fn query_date(seconds: i64) -> String {
    DateTime::from_secs(anchored(seconds))
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

/// An asset with one DOUBLE measurement property; answers `(assetId, propertyId)`.
async fn speed_property(router: &MultiServiceRouter) -> (String, String) {
    let model_id = create_model(
        router,
        "Turbine",
        json!([{"name": "Speed", "dataType": "DOUBLE", "unit": "rpm", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(router, "Turbine1", &model_id).await;
    let (_, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    let speed = property_id(&asset["assetProperties"], "Speed");
    (asset_id, speed)
}

/// The value this suite stores at `seconds`, so every assertion can check that a
/// reported sample kept its own value and did not pick up a neighbour's.
fn value_at(seconds: i64) -> f64 {
    f64::from(i32::try_from(seconds).expect("a second that fits")) / 10.0
}

/// Write `samples` — `(timeInSeconds, quality)`, a `None` quality meaning the write
/// carries none — through `BatchPutAssetPropertyValue`, which takes at most ten
/// entries of ten values each.
async fn write_samples(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    samples: &[(i64, Option<&str>)],
) {
    for (chunk, group) in samples.chunks(100).enumerate() {
        let entries: Vec<Value> = group
            .chunks(10)
            .enumerate()
            .map(|(at, values)| {
                let property_values: Vec<Value> = values
                    .iter()
                    .map(|(seconds, quality)| {
                        let mut sample = json!({
                            "value": {"doubleValue": value_at(*seconds)},
                            "timestamp": {"timeInSeconds": anchored(*seconds), "offsetInNanos": 0},
                        });
                        if let Some(quality) = quality {
                            sample["quality"] = json!(quality);
                        }
                        sample
                    })
                    .collect();
                json!({
                    "entryId": format!("c{chunk}e{at}"),
                    "assetId": asset_id,
                    "propertyId": property,
                    "propertyValues": property_values,
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
}

/// `GET /properties/history?<query>`.
async fn history(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/properties/history?{query}"), None).await
}

/// The `timeInSeconds` of every reported sample, as this suite's relative seconds,
/// in the order reported.
fn reported_seconds(body: &Value) -> Vec<i64> {
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

/// Assert every reported sample still carries the value written at its timestamp.
fn assert_values_match(case: &str, body: &Value) {
    for sample in body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {body}"))
    {
        let seconds = relative(
            sample["timestamp"]["timeInSeconds"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: a sample with no timeInSeconds in {body}")),
        );
        assert_eq!(
            sample["value"]["doubleValue"].as_f64(),
            Some(value_at(seconds)),
            "case {case:?}: the sample at {seconds} must keep its own value, got {sample}"
        );
    }
}

#[tokio::test]
async fn the_history_is_ordered_by_timestamp_as_time_ordering_asks() {
    let router = router();
    let (asset_id, speed) = speed_property(&router).await;
    // Written out of order on purpose: the reported order must come from the
    // timestamps, never from the write order.
    let written = [1_030_i64, 1_000, 1_050, 1_010, 1_040, 1_020];
    let samples: Vec<(i64, Option<&str>)> =
        written.iter().map(|seconds| (*seconds, None)).collect();
    write_samples(&router, &asset_id, &speed, &samples).await;

    let ascending: Vec<i64> = vec![1_000, 1_010, 1_020, 1_030, 1_040, 1_050];
    let descending: Vec<i64> = ascending.iter().rev().copied().collect();
    // Rows span every `TimeOrdering` value plus the documented default (the member
    // left off the request entirely).
    let cases: Vec<(&str, String, Vec<i64>)> = vec![
        ("default", String::new(), ascending.clone()),
        (
            "ASCENDING",
            "&timeOrdering=ASCENDING".to_string(),
            ascending.clone(),
        ),
        (
            "DESCENDING",
            "&timeOrdering=DESCENDING".to_string(),
            descending,
        ),
    ];

    for (case, ordering, expected) in cases {
        let (status, body) = history(
            &router,
            &format!("assetId={asset_id}&propertyId={speed}{ordering}"),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            reported_seconds(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert_values_match(case, &body);
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: one page holds every sample, so there is no nextToken: {body}"
        );
    }
}

/// One row of the window table: the case name, the `startDate` and `endDate` to
/// send (each absent when `None`) and the `timeInSeconds` the answer must carry.
type WindowCase = (&'static str, Option<i64>, Option<i64>, Vec<i64>);

#[tokio::test]
async fn the_queried_window_excludes_its_start_and_includes_its_end() {
    let router = router();
    let (asset_id, speed) = speed_property(&router).await;
    let written = [(100_i64, None), (200, None), (300, None), (400, None)];
    write_samples(&router, &asset_id, &speed, &written).await;

    // Rows span the ways the two documented bounds — "the exclusive start" and "the
    // inclusive end" — can cut the stored run, including each bound landing exactly
    // on a stored sample and a window that holds none.
    let cases: Vec<WindowCase> = vec![
        ("no bounds", None, None, vec![100, 200, 300, 400]),
        ("start on a sample", Some(100), None, vec![200, 300, 400]),
        ("start between samples", Some(150), None, vec![200, 300, 400]),
        ("end on a sample", None, Some(300), vec![100, 200, 300]),
        ("end between samples", None, Some(350), vec![100, 200, 300]),
        ("both bounds", Some(100), Some(300), vec![200, 300]),
        ("one sample wide", Some(199), Some(200), vec![200]),
        ("empty window", Some(200), Some(200), vec![]),
        ("after everything", Some(400), None, vec![]),
        ("before everything", None, Some(99), vec![]),
    ];

    for (case, start, end, expected) in cases {
        let mut query = format!("assetId={asset_id}&propertyId={speed}");
        if let Some(start) = start {
            query.push_str(&format!("&startDate={}", query_date(start)));
        }
        if let Some(end) = end {
            query.push_str(&format!("&endDate={}", query_date(end)));
        }
        let (status, body) = history(&router, &query).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            reported_seconds(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert_values_match(case, &body);
    }

    // A window whose end precedes its start names no range at all.
    let (status, body) = history(
        &router,
        &format!(
            "assetId={asset_id}&propertyId={speed}&startDate={}&endDate={}",
            query_date(300),
            query_date(200)
        ),
    )
    .await;
    assert_eq!(status, 400, "endDate before startDate: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "endDate before startDate: unexpected error code in {body}"
    );
}

#[tokio::test]
async fn qualities_keeps_only_the_quality_the_request_names() {
    let router = router();
    let (asset_id, speed) = speed_property(&router).await;
    // One sample per modelled `Quality`, plus one written without a quality at all.
    let written: Vec<(i64, Option<&str>)> = QUALITIES
        .iter()
        .enumerate()
        .map(|(at, quality)| (1_000 + at as i64 * 1_000, Some(*quality)))
        .chain(std::iter::once((9_000_i64, None)))
        .collect();
    write_samples(&router, &asset_id, &speed, &written).await;

    // A request naming no quality asks for every sample; one naming a quality keeps
    // only the samples reporting it, and a write that carried no quality counts as
    // GOOD. Rows span the whole `Quality` catalog, including UNCERTAIN, which the
    // reported case never mentions.
    let mut cases: Vec<(String, Option<&str>, Vec<i64>)> =
        vec![("no filter".to_string(), None, vec![1_000, 2_000, 3_000, 9_000])];
    for (at, quality) in QUALITIES.iter().enumerate() {
        let mut expected = vec![1_000 + at as i64 * 1_000];
        if *quality == "GOOD" {
            expected.push(9_000);
        }
        cases.push((format!("qualities={quality}"), Some(quality), expected));
    }

    for (case, quality, expected) in cases {
        let mut query = format!("assetId={asset_id}&propertyId={speed}");
        if let Some(quality) = quality {
            query.push_str(&format!("&qualities={quality}"));
        }
        let (status, body) = history(&router, &query).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            reported_seconds(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert_values_match(&case, &body);
    }
}

#[tokio::test]
async fn next_token_walks_every_sample_of_the_ordered_run_exactly_once() {
    let router = router();
    let (asset_id, speed) = speed_property(&router).await;
    let written: Vec<(i64, Option<&str>)> =
        (0..25).map(|at| (5_000 + at * 7, None)).collect();
    write_samples(&router, &asset_id, &speed, &written).await;

    let ascending: Vec<i64> = written.iter().map(|(seconds, _)| *seconds).collect();
    let descending: Vec<i64> = ascending.iter().rev().copied().collect();

    // Rows span page sizes that divide the run, do not divide it, hold it whole and
    // exceed it — in both orderings, since a page's resume point must follow the
    // order the request asked for.
    for (ordering, expected) in [("ASCENDING", &ascending), ("DESCENDING", &descending)] {
        for page_size in [1_i64, 2, 5, 7, 25, 26, MAX_RESULTS] {
            let case = format!("{ordering} maxResults={page_size}");
            let mut seen: Vec<i64> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut query = format!(
                    "assetId={asset_id}&propertyId={speed}\
                     &timeOrdering={ordering}&maxResults={page_size}"
                );
                if let Some(token) = token.as_deref() {
                    query.push_str(&format!("&nextToken={token}"));
                }
                let (status, body) = history(&router, &query).await;
                assert_eq!(status, 200, "case {case:?}: got {status} {body}");
                let page = reported_seconds(&body);
                assert!(
                    !page.is_empty() && page.len() as i64 <= page_size,
                    "case {case:?}: a page holds 1..={page_size} samples, got {page:?}"
                );
                assert_values_match(&case, &body);
                seen.extend(page);
                token = body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                pages += 1;
                assert!(
                    pages <= written.len() + 1,
                    "case {case:?}: paging must terminate, but ran {pages} pages"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                &seen, expected,
                "case {case:?}: paging must report the whole ordered run once, got {seen:?}"
            );
        }
    }

    // `maxResults` carries the documented maximum 20000 and the modelled minimum 1;
    // each published bound is accepted and one past it is refused.
    let bounds = [(1_i64, 200), (MAX_RESULTS, 200), (MAX_RESULTS + 1, 400), (0, 400)];
    for (requested, expected_status) in bounds {
        let (status, body) = history(
            &router,
            &format!("assetId={asset_id}&propertyId={speed}&maxResults={requested}"),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case maxResults={requested}: expected {expected_status}, got {status} {body}"
        );
    }

    // A token this operation never issued is refused rather than answered with an
    // empty page.
    for (case, token) in [
        ("not hex", "zzzz"),
        ("hex of nothing timestamp-shaped", "6162"),
        ("a timestamp no sample carries", "392e30"),
    ] {
        let (status, body) = history(
            &router,
            &format!("assetId={asset_id}&propertyId={speed}&nextToken={token}"),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn history_reads_the_one_property_the_request_identifies() {
    let (router, engine) = router_with_engine();
    let (asset_id, speed) = speed_property(&router).await;
    let other_asset = {
        let model_id = create_model(
            &router,
            "Pump",
            json!([{"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
        )
        .await;
        create_asset(&router, "Pump1", &model_id).await
    };
    let (_, pump) = call(&router, "GET", &format!("/assets/{other_asset}"), None).await;
    let flow = property_id(&pump["assetProperties"], "Flow");

    write_samples(&router, &asset_id, &speed, &[(700, None), (800, None)]).await;
    write_samples(&router, &other_asset, &flow, &[(900, None)]).await;

    // Each property reports only its own samples.
    for (case, asset, property, expected) in [
        ("turbine speed", &asset_id, &speed, vec![700_i64, 800]),
        ("pump flow", &other_asset, &flow, vec![900]),
    ] {
        let (status, body) =
            history(&router, &format!("assetId={asset}&propertyId={property}")).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            reported_seconds(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The alias form names the same property as its ids do. The alias index is
    // written by operations that are still 501 scaffolds
    // (`UpdateAssetProperty`, `AssociateTimeSeriesToAssetProperty`), so the test
    // seeds it in the one shared store the handlers read.
    let alias = "/company/windfarm/3/turbine/7/speed";
    service_state(&*engine)
        .expect("the service store")
        .lock()
        .property_alias_index
        .insert(alias.to_string(), (asset_id.clone(), speed.clone()));
    let (status, body) = history(&router, &format!("propertyAlias={alias}")).await;
    assert_eq!(status, 200, "alias form: got {status} {body}");
    assert_eq!(
        reported_seconds(&body),
        vec![700, 800],
        "alias form: expected the same samples as the ids report, got {body}"
    );

    // A property nothing was written to has an empty history, and that is not an
    // error: the member is required, so the list is present and empty.
    let unwritten = {
        let asset = create_asset(
            &router,
            "Turbine2",
            call(&router, "GET", &format!("/assets/{asset_id}"), None).await.1["assetModelId"]
                .as_str()
                .expect("the asset's model id"),
        )
        .await;
        let (_, described) = call(&router, "GET", &format!("/assets/{asset}"), None).await;
        (asset, property_id(&described["assetProperties"], "Speed"))
    };
    let (status, body) = history(
        &router,
        &format!("assetId={}&propertyId={}", unwritten.0, unwritten.1),
    )
    .await;
    assert_eq!(status, 200, "unwritten property: got {status} {body}");
    assert_eq!(
        body["assetPropertyValueHistory"].as_array().map(Vec::len),
        Some(0),
        "unwritten property: expected an empty history, got {body}"
    );
}

#[tokio::test]
async fn a_property_that_resolves_to_nothing_is_a_modelled_refusal() {
    let router = router();
    let (asset_id, speed) = speed_property(&router).await;
    let absent = "11111111-2222-3333-4444-555555555555";

    // Rows span every way the request can fail to identify one stored property.
    let refusals: Vec<(&str, String, u16, &str)> = vec![
        (
            "unknown asset",
            format!("assetId={absent}&propertyId={speed}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown property",
            format!("assetId={asset_id}&propertyId={absent}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown alias",
            "propertyAlias=/no/such/stream".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "no identifier at all",
            "timeOrdering=ASCENDING".to_string(),
            400,
            "InvalidRequestException",
        ),
        (
            "asset without property",
            format!("assetId={asset_id}"),
            400,
            "InvalidRequestException",
        ),
        (
            "both forms",
            format!("assetId={asset_id}&propertyId={speed}&propertyAlias=/a/b"),
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, query, expected_status, expected_code) in refusals {
        let (status, body) = history(&router, &query).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
    }
}
