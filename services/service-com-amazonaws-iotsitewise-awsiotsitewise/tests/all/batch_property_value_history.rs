// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `BatchGetAssetPropertyValueHistory`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: one batch request answers EVERY entry it carries
//! from the one shared engine store, each entry INDEPENDENTLY — the samples stored
//! for the data stream that entry identifies, kept inside its own half-open window
//! `(startDate, endDate]`, filtered to the quality it names (every quality when it
//! names none), ordered as its own `timeOrdering` asks — with the entries that
//! identify no stored stream reported as error entries, the data points of the
//! answer cut into `maxResults`-sized pages joined by `nextToken`, the entries a
//! previous page finished reported as skipped, and nothing a later caller reads
//! changed by the read.
//!
//! The harness (`router`, `call`, `create_model`, …) lives in the `asset_lifecycle`
//! suite and the clock anchoring in `support`; this file reuses both rather than
//! keeping a copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use super::asset_lifecycle::{
    call, create_asset, create_model, error_code, hex_token, property_id, router,
    router_with_engine,
};

/// Where `BatchGetAssetPropertyValueHistory` is bound: `POST
/// /properties/batch/history`, from `smithy_query(query="operation",
/// operation="BatchGetAssetPropertyValueHistory")`.
const BATCH_HISTORY: &str = "/properties/batch/history";

/// Where `BatchPutAssetPropertyValue` is bound, used here to seed the samples the
/// batch read must report.
const BATCH_PUT: &str = "/properties";

/// Every `Quality` the model declares. Source:
/// `smithy_query(query="shape", shape="Quality")` — GOOD | BAD | UNCERTAIN.
const QUALITIES: [&str; 3] = ["GOOD", "BAD", "UNCERTAIN"];

/// The documented entry ceiling, from the model doc on
/// `BatchGetAssetPropertyValueHistoryRequest$entries`: "You can specify up to 16
/// entries per request."
const MAX_ENTRIES: usize = 16;

/// The documented ceiling on `maxResults`, from the model doc on
/// `BatchGetAssetPropertyValueHistoryRequest$maxResults`: "The maximum value of
/// maxResults is 20000."
const MAX_RESULTS: i64 = 20_000;

/// The epoch this suite's tables measure their seconds from.
///
/// `BatchPutAssetPropertyValue` ingests only TQVs inside
/// `[now - 7 days, now + 10 minutes]`, so the shared `super::support` anchoring
/// maps this epoch into the accepted window at the wire boundary and
/// [`relative`] maps a reported timestamp back for the assertions.
const ROW_EPOCH: i64 = 0;

/// A table second as the absolute `timeInSeconds` the wire carries.
///
/// Shared with the sibling suite that reads this operation over a composite
/// model's property, so the two cannot drift about where a table second lands.
pub(crate) fn anchored(seconds: i64) -> i64 {
    super::support::anchored(ROW_EPOCH, seconds)
}

/// The inverse of [`anchored`], shared for the reason it gives.
pub(crate) fn relative(seconds: i64) -> i64 {
    super::support::relative(ROW_EPOCH, seconds)
}

/// The value this suite stores at `seconds`, so every assertion can check that a
/// reported sample kept its own value and did not pick up a neighbour's.
fn value_at(seconds: i64) -> f64 {
    f64::from(i32::try_from(seconds).expect("a second that fits")) / 10.0
}

/// An asset with one DOUBLE measurement property; answers `(assetId, propertyId)`.
async fn double_property(
    router: &MultiServiceRouter,
    model_name: &str,
    asset_name: &str,
    property: &str,
) -> (String, String) {
    let model_id = create_model(
        router,
        model_name,
        json!([{"name": property, "dataType": "DOUBLE", "unit": "rpm", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(router, asset_name, &model_id).await;
    let (status, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {asset_name}: got {status} {asset}");
    let property_id = property_id(&asset["assetProperties"], property);
    (asset_id, property_id)
}

/// One sample to seed: `(timeInSeconds, offsetInNanos, quality)`, a `None`
/// quality meaning the write carries none.
type Sample<'a> = (i64, i64, Option<&'a str>);

/// Write `samples` through `BatchPutAssetPropertyValue`, which takes at most ten
/// entries of ten values each.
async fn write_samples(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    samples: &[Sample<'_>],
) {
    for (chunk, group) in samples.chunks(100).enumerate() {
        let entries: Vec<Value> = group
            .chunks(10)
            .enumerate()
            .map(|(at, values)| {
                let property_values: Vec<Value> = values
                    .iter()
                    .map(|(seconds, offset, quality)| {
                        let mut sample = json!({
                            "value": {"doubleValue": value_at(*seconds)},
                            "timestamp": {
                                "timeInSeconds": anchored(*seconds),
                                "offsetInNanos": offset,
                            },
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
        let (status, body) = call(router, "POST", BATCH_PUT, Some(json!({"entries": entries}))).await;
        assert_eq!(status, 200, "writing samples: got {status} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "writing samples: every entry must be accepted, got {body}"
        );
    }
}

/// How many elements a response list carries (0 when it is absent).
fn count(list: &Value) -> usize {
    list.as_array().map_or(0, Vec::len)
}

/// The entry of `list` whose `entryId` is `entry_id`, if any.
fn find_entry<'a>(list: &'a Value, entry_id: &str) -> Option<&'a Value> {
    list.as_array()?
        .iter()
        .find(|entry| entry["entryId"] == entry_id)
}

/// The `timeInSeconds` of every sample a success entry reports, as this suite's
/// relative seconds, in the order reported.
///
/// Shared with the sibling suite for the reason [`anchored`] gives: one reading
/// of this operation's success-entry shape.
pub(crate) fn reported_seconds(case: &str, entry: &Value) -> Vec<i64> {
    entry["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {entry}"))
        .iter()
        .map(|sample| {
            relative(
                sample["timestamp"]["timeInSeconds"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("case {case:?}: a sample with no timeInSeconds in {entry}")),
            )
        })
        .collect()
}

/// Assert every sample a success entry reports still carries the value written at
/// its timestamp, so a reported sample cannot have picked up a neighbour's.
fn assert_samples_intact(case: &str, entry: &Value) {
    for sample in entry["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {entry}"))
    {
        let seconds = relative(
            sample["timestamp"]["timeInSeconds"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: a sample with no timeInSeconds in {sample}")),
        );
        assert_eq!(
            sample["value"]["doubleValue"].as_f64(),
            Some(value_at(seconds)),
            "case {case:?}: the sample at {seconds} must keep its own value, got {sample}"
        );
    }
}

#[tokio::test]
async fn every_entry_is_answered_with_its_own_window_qualities_and_ordering() {
    let (router, engine) = router_with_engine();
    let (turbine, speed) = double_property(&router, "Turbine", "Turbine1", "Speed").await;
    let (pump, flow) = double_property(&router, "Pump", "Pump1", "Flow").await;

    // One sample per modelled `Quality`, plus one written without a quality at all
    // — which IoT SiteWise counts as GOOD. Written out of order on purpose: the
    // reported order must come from the timestamps, never from the write order.
    write_samples(
        &router,
        &turbine,
        &speed,
        &[
            (300, 0, Some(QUALITIES[2])),
            (100, 0, Some(QUALITIES[0])),
            (400, 0, None),
            (200, 0, Some(QUALITIES[1])),
        ],
    )
    .await;
    write_samples(&router, &pump, &flow, &[(900, 0, None)]).await;

    // `UpdateAssetProperty` — the operation that gives a property its alias — is
    // still a scaffold stub, so the alias index is seeded through the shared store
    // the handlers read, exactly as that operation will write it.
    let alias = "/company/windfarm/3/turbine/7/speed";
    service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .property_alias_index
        .insert(alias.to_string(), (turbine.clone(), speed.clone()));

    let all = vec![100_i64, 200, 300, 400];
    // Rows differ along every dimension the rule generalises over: which bounds the
    // entry carries and where they land (on a stored sample, between two, naming an
    // empty range), which `Quality` it filters on, which `TimeOrdering` it asks for,
    // which identifying FORM it uses, and which stream it names. Every row rides in
    // ONE request, so a handler that applies one entry's members to another fails.
    let rows: Vec<(&str, Value, Vec<i64>)> = vec![
        (
            "no bounds, no filter",
            json!({"entryId": "plain", "assetId": turbine, "propertyId": speed}),
            all.clone(),
        ),
        (
            "startDate is exclusive, landing on a sample",
            json!({"entryId": "startonsample", "assetId": turbine, "propertyId": speed,
                   "startDate": anchored(100)}),
            vec![200, 300, 400],
        ),
        (
            "startDate between two samples",
            json!({"entryId": "startbetween", "assetId": turbine, "propertyId": speed,
                   "startDate": anchored(150)}),
            vec![200, 300, 400],
        ),
        (
            "endDate is inclusive, landing on a sample",
            json!({"entryId": "endonsample", "assetId": turbine, "propertyId": speed,
                   "endDate": anchored(300)}),
            vec![100, 200, 300],
        ),
        (
            "both bounds",
            json!({"entryId": "bothbounds", "assetId": turbine, "propertyId": speed,
                   "startDate": anchored(100), "endDate": anchored(300)}),
            vec![200, 300],
        ),
        (
            "one sample wide",
            json!({"entryId": "onewide", "assetId": turbine, "propertyId": speed,
                   "startDate": anchored(199), "endDate": anchored(200)}),
            vec![200],
        ),
        (
            "a window holding no sample",
            json!({"entryId": "emptywindow", "assetId": turbine, "propertyId": speed,
                   "startDate": anchored(200), "endDate": anchored(200)}),
            vec![],
        ),
        (
            "DESCENDING",
            json!({"entryId": "descending", "assetId": turbine, "propertyId": speed,
                   "timeOrdering": "DESCENDING"}),
            all.iter().rev().copied().collect(),
        ),
        (
            "ASCENDING, stated",
            json!({"entryId": "ascending", "assetId": turbine, "propertyId": speed,
                   "timeOrdering": "ASCENDING"}),
            all.clone(),
        ),
        (
            "qualities=GOOD, which an unqualified write counts as",
            json!({"entryId": "goodonly", "assetId": turbine, "propertyId": speed,
                   "qualities": [QUALITIES[0]]}),
            vec![100, 400],
        ),
        (
            "qualities=BAD",
            json!({"entryId": "badonly", "assetId": turbine, "propertyId": speed,
                   "qualities": [QUALITIES[1]]}),
            vec![200],
        ),
        (
            "qualities=UNCERTAIN",
            json!({"entryId": "uncertainonly", "assetId": turbine, "propertyId": speed,
                   "qualities": [QUALITIES[2]]}),
            vec![300],
        ),
        (
            "a quality filter and a window at once, DESCENDING",
            json!({"entryId": "filteredwindow", "assetId": turbine, "propertyId": speed,
                   "qualities": [QUALITIES[0]], "startDate": anchored(50),
                   "endDate": anchored(400), "timeOrdering": "DESCENDING"}),
            vec![400, 100],
        ),
        (
            "the alias form names the same stream as the ids do",
            json!({"entryId": "byalias", "propertyAlias": alias}),
            all.clone(),
        ),
        (
            "another asset's property reports only its own samples",
            json!({"entryId": "otherstream", "assetId": pump, "propertyId": flow}),
            vec![900],
        ),
    ];
    assert!(
        rows.len() <= MAX_ENTRIES,
        "the table must fit one request: {} rows",
        rows.len()
    );

    let entries: Vec<Value> = rows.iter().map(|(_, entry, _)| entry.clone()).collect();
    let (status, body) = call(
        &router,
        "POST",
        BATCH_HISTORY,
        Some(json!({"entries": entries})),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetAssetPropertyValueHistory: got {status} {body}"
    );
    assert_eq!(
        count(&body["errorEntries"]),
        0,
        "every entry names a stored stream, so errorEntries must be empty: {body}"
    );
    assert_eq!(
        count(&body["skippedEntries"]),
        0,
        "no previous page processed anything, so skippedEntries must be empty: {body}"
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "the whole result set fits one page, so there must be no nextToken: {body}"
    );
    assert_eq!(
        count(&body["successEntries"]),
        rows.len(),
        "expected one success entry per requested entry, got {body}"
    );

    for (case, entry, expected) in &rows {
        let entry_id = entry["entryId"].as_str().expect("an entryId");
        let answered = find_entry(&body["successEntries"], entry_id)
            .unwrap_or_else(|| panic!("case {case:?}: no success entry {entry_id} in {body}"));
        assert_eq!(
            &reported_seconds(case, answered),
            expected,
            "case {case:?}: expected {expected:?}, got {answered}"
        );
        assert_samples_intact(case, answered);
    }

    // Reporting changes nothing a later caller sees: the same request answers the
    // same way.
    let (again_status, again) = call(
        &router,
        "POST",
        BATCH_HISTORY,
        Some(json!({"entries": entries})),
    )
    .await;
    assert_eq!(again_status, 200, "second read: got {again_status} {again}");
    assert_eq!(
        again, body,
        "a history read must not change what the next caller reads: {again}"
    );
}

#[tokio::test]
async fn the_window_bounds_compare_at_the_nanosecond_precision_a_sample_carries() {
    let router = router();
    let (asset_id, speed) = double_property(&router, "Press", "Press1", "Force").await;
    // Two samples at the SAME second, one on it and one a few nanoseconds after:
    // `TimeInNanos` carries `offsetInNanos` (range 0..=999999999), so the two are
    // different instants and a bound landing on the whole second separates them.
    let second = 4_000_i64;
    write_samples(
        &router,
        &asset_id,
        &speed,
        &[(second, 0, None), (second, 250, None)],
    )
    .await;

    // Rows: the exclusive start and the inclusive end each landing exactly on the
    // whole second, and a bound inside the subsecond gap.
    let cases: Vec<(&str, Value, Vec<i64>)> = vec![
        (
            "no bounds reports both samples of the second",
            json!({"entryId": "both", "assetId": asset_id, "propertyId": speed}),
            vec![0, 250],
        ),
        (
            "an exclusive start on the whole second drops the sample on it",
            json!({"entryId": "exclusivestart", "assetId": asset_id, "propertyId": speed,
                   "startDate": anchored(second)}),
            vec![250],
        ),
        (
            "an inclusive end on the whole second keeps only the sample on it",
            json!({"entryId": "inclusiveend", "assetId": asset_id, "propertyId": speed,
                   "endDate": anchored(second)}),
            vec![0],
        ),
    ];

    let entries: Vec<Value> = cases.iter().map(|(_, entry, _)| entry.clone()).collect();
    let (status, body) = call(
        &router,
        "POST",
        BATCH_HISTORY,
        Some(json!({"entries": entries})),
    )
    .await;
    assert_eq!(status, 200, "nanosecond bounds: got {status} {body}");
    for (case, entry, expected_offsets) in &cases {
        let entry_id = entry["entryId"].as_str().expect("an entryId");
        let answered = find_entry(&body["successEntries"], entry_id)
            .unwrap_or_else(|| panic!("case {case:?}: no success entry {entry_id} in {body}"));
        let offsets: Vec<i64> = answered["assetPropertyValueHistory"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {answered}"))
            .iter()
            .map(|sample| {
                sample["timestamp"]["offsetInNanos"]
                    .as_i64()
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            &offsets, expected_offsets,
            "case {case:?}: expected the samples at offsets {expected_offsets:?}, got {answered}"
        );
        assert_eq!(
            reported_seconds(case, answered),
            vec![second; expected_offsets.len()],
            "case {case:?}: every reported sample is at second {second}, got {answered}"
        );
    }
}

#[tokio::test]
async fn an_entry_that_identifies_no_stored_stream_is_an_error_entry() {
    let router = router();
    let (asset_id, speed) = double_property(&router, "Blower", "Blower1", "Draft").await;
    write_samples(&router, &asset_id, &speed, &[(500, 0, None)]).await;
    let absent = "11111111-2222-3333-4444-555555555555";

    // Rows span the ways one entry can fail to name one answerable stream. The
    // codes are the `BatchGetAssetPropertyValueHistoryErrorCode` values the model
    // lists: ResourceNotFoundException | InvalidRequestException |
    // AccessDeniedException (the last needs an authorization decision this emulator
    // does not make).
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "resolves",
            json!({"entryId": "ok", "assetId": asset_id, "propertyId": speed}),
            None,
        ),
        (
            "unknown asset",
            json!({"entryId": "noasset", "assetId": absent, "propertyId": speed}),
            Some("ResourceNotFoundException"),
        ),
        (
            "unknown property",
            json!({"entryId": "noproperty", "assetId": asset_id, "propertyId": absent}),
            Some("ResourceNotFoundException"),
        ),
        (
            "unknown alias",
            json!({"entryId": "noalias", "propertyAlias": "/no/such/stream"}),
            Some("ResourceNotFoundException"),
        ),
        (
            "no identifier at all",
            json!({"entryId": "noid"}),
            Some("InvalidRequestException"),
        ),
        (
            "both identifying forms at once",
            json!({"entryId": "bothforms", "assetId": asset_id, "propertyId": speed,
                   "propertyAlias": "/company/blower/1/draft"}),
            Some("InvalidRequestException"),
        ),
        (
            "a property id without an asset id",
            json!({"entryId": "lonelyproperty", "propertyId": speed}),
            Some("InvalidRequestException"),
        ),
        (
            "endDate before startDate names no range",
            json!({"entryId": "invertedwindow", "assetId": asset_id, "propertyId": speed,
                   "startDate": anchored(600), "endDate": anchored(500)}),
            Some("InvalidRequestException"),
        ),
    ];

    let entries: Vec<Value> = cases.iter().map(|(_, entry, _)| entry.clone()).collect();
    let (status, body) = call(
        &router,
        "POST",
        BATCH_HISTORY,
        Some(json!({"entries": entries})),
    )
    .await;
    assert_eq!(
        status, 200,
        "a per-entry failure is reported in the body, not as a request failure: got {status} {body}"
    );

    for (case, entry, expected) in &cases {
        let entry_id = entry["entryId"].as_str().expect("an entryId");
        match expected {
            None => {
                let success = find_entry(&body["successEntries"], entry_id).unwrap_or_else(|| {
                    panic!("case {case:?}: expected a success entry {entry_id}, got {body}")
                });
                assert_eq!(
                    reported_seconds(case, success),
                    vec![500],
                    "case {case:?}: expected the stored sample, got {success}"
                );
                assert!(
                    find_entry(&body["errorEntries"], entry_id).is_none(),
                    "case {case:?}: {entry_id} must not also be an error entry: {body}"
                );
            }
            Some(code) => {
                let failed = find_entry(&body["errorEntries"], entry_id).unwrap_or_else(|| {
                    panic!("case {case:?}: expected an error entry {entry_id}, got {body}")
                });
                assert_eq!(
                    failed["errorCode"].as_str(),
                    Some(*code),
                    "case {case:?}: expected errorCode {code}, got {failed}"
                );
                assert!(
                    failed["errorMessage"]
                        .as_str()
                        .is_some_and(|message| !message.is_empty()),
                    "case {case:?}: an error entry must carry an errorMessage, got {failed}"
                );
                assert!(
                    find_entry(&body["successEntries"], entry_id).is_none(),
                    "case {case:?}: {entry_id} must not also be a success entry: {body}"
                );
            }
        }
    }

    assert_eq!(
        count(&body["successEntries"]) + count(&body["errorEntries"]),
        cases.len(),
        "every entry is answered exactly once: {body}"
    );
}

#[tokio::test]
async fn max_results_cuts_the_data_points_of_every_entry_into_pages() {
    let router = router();
    let (asset_id, speed) = double_property(&router, "Mill", "Mill1", "Torque").await;
    let written: Vec<Sample<'_>> = (0..10).map(|at| (1_000 + at * 10, 0, None)).collect();
    write_samples(&router, &asset_id, &speed, &written).await;
    let ascending: Vec<i64> = written.iter().map(|(seconds, ..)| *seconds).collect();
    let descending: Vec<i64> = ascending.iter().rev().copied().collect();

    // Three entries over the same stream, the middle one reading the other way
    // round and the last one windowed, so a page boundary can fall inside any of
    // them and each must resume in ITS own order.
    let entries = json!([
        {"entryId": "first", "assetId": asset_id, "propertyId": speed},
        {"entryId": "middle", "assetId": asset_id, "propertyId": speed,
         "timeOrdering": "DESCENDING"},
        {"entryId": "last", "assetId": asset_id, "propertyId": speed,
         "startDate": anchored(1_050), "endDate": anchored(1_080)},
    ]);
    let expected: Vec<(&str, Vec<i64>)> = vec![
        ("first", ascending.clone()),
        ("middle", descending),
        ("last", vec![1_060, 1_070, 1_080]),
    ];
    let total: usize = expected.iter().map(|(_, seconds)| seconds.len()).sum();

    // Page sizes that divide the answer, that do not, that hold it whole, that
    // exceed it, and the documented maximum.
    for page_size in [1_i64, 2, 3, 7, 23, total as i64, total as i64 + 1, MAX_RESULTS] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<(String, Vec<i64>)> = expected
            .iter()
            .map(|(entry_id, _)| ((*entry_id).to_string(), Vec::new()))
            .collect();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"entries": entries, "maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "POST", BATCH_HISTORY, Some(request)).await;
            assert_eq!(status, 200, "case {case:?}: got {status} {body}");
            assert_eq!(
                count(&body["errorEntries"]),
                0,
                "case {case:?}: every entry resolves, so errorEntries must be empty: {body}"
            );

            // Everything a previous page finished is reported as skipped, in the
            // order the request carries, with the status it was answered with.
            let skipped: Vec<String> = body["skippedEntries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no skippedEntries in {body}"))
                .iter()
                .map(|entry| entry["entryId"].as_str().unwrap_or_default().to_string())
                .collect();
            let finished: Vec<String> = expected
                .iter()
                .zip(&seen)
                .take_while(|((_, want), (_, got))| want == got)
                .map(|((entry_id, _), _)| (*entry_id).to_string())
                .collect();
            assert_eq!(
                skipped,
                finished[..skipped.len().min(finished.len())].to_vec(),
                "case {case:?}: skipped entries are the leading entries earlier pages \
                 finished, got {body}"
            );
            for entry in body["skippedEntries"].as_array().into_iter().flatten() {
                assert_eq!(
                    entry["completionStatus"].as_str(),
                    Some("SUCCESS"),
                    "case {case:?}: a skipped entry that resolves reports SUCCESS, got {entry}"
                );
                assert!(
                    entry.get("errorInfo").is_none_or(Value::is_null),
                    "case {case:?}: a SUCCESS skipped entry carries no errorInfo, got {entry}"
                );
            }

            let mut on_this_page = 0;
            for answered in body["successEntries"].as_array().into_iter().flatten() {
                let entry_id = answered["entryId"].as_str().unwrap_or_default().to_string();
                let page = reported_seconds(&case, answered);
                on_this_page += page.len();
                let slot = seen
                    .iter_mut()
                    .find(|(seen_id, _)| *seen_id == entry_id)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {entry_id} is not an entry of this request: {body}")
                    });
                slot.1.extend(page);
                assert_samples_intact(&case, answered);
            }
            assert!(
                on_this_page <= page_size as usize,
                "case {case:?}: a page carries at most {page_size} data points, got {on_this_page}"
            );

            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: paging must terminate, but ran {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        on_this_page > 0,
                        "case {case:?}: a page that promises more must report something: {body}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        for ((entry_id, want), (_, got)) in expected.iter().zip(&seen) {
            assert_eq!(
                got, want,
                "case {case:?}: entry {entry_id} must report its whole ordered run exactly \
                 once, got {got:?}"
            );
        }
    }

    // A token this operation never issued for these entries is refused rather than
    // answered with an empty page. Every row is inside `NextToken`'s modelled
    // `^[A-Za-z0-9+/=]+$` pattern, so the refusal comes from the handler.
    let bad_tokens = [
        ("not hex", "zzzz".to_string()),
        ("hex of nothing cursor-shaped", hex_token("ab")),
        ("an entry past the end of the list", hex_token("9:0")),
        ("a data point past the end of an entry", hex_token("0:99")),
        ("a cursor with no numbers", hex_token("a:b")),
    ];
    for (case, token) in bad_tokens {
        let (status, body) = call(
            &router,
            "POST",
            BATCH_HISTORY,
            Some(json!({"entries": entries, "nextToken": token})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_request_is_bounded_at_the_documented_entry_and_result_counts() {
    let router = router();
    let (asset_id, speed) = double_property(&router, "Kiln", "Kiln1", "Heat").await;
    write_samples(&router, &asset_id, &speed, &[(2_000, 0, None)]).await;
    let one_entry = |at: usize| {
        json!({"entryId": format!("e{at}"), "assetId": asset_id, "propertyId": speed})
    };

    // Each published bound, and one past it: "You can specify up to 16 entries per
    // request", and a request carrying none asks for nothing.
    for entry_count in [0_usize, 1, 15, MAX_ENTRIES, MAX_ENTRIES + 1] {
        let entries: Vec<Value> = (0..entry_count).map(one_entry).collect();
        let (status, body) = call(
            &router,
            "POST",
            BATCH_HISTORY,
            Some(json!({"entries": entries})),
        )
        .await;
        if (1..=MAX_ENTRIES).contains(&entry_count) {
            assert_eq!(
                status, 200,
                "case {entry_count} entries: expected 200, got {status} {body}"
            );
            assert_eq!(
                count(&body["successEntries"]),
                entry_count,
                "case {entry_count} entries: every entry must be answered, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {entry_count} entries: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {entry_count} entries: unexpected error code in {body}"
            );
        }
    }

    // `maxResults` carries the documented maximum 20000 and the modelled minimum 1;
    // each published bound is accepted and one past it is refused.
    for (requested, expected_status) in [
        (1_i64, 200),
        (MAX_RESULTS, 200),
        (MAX_RESULTS + 1, 400),
        (0, 400),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            BATCH_HISTORY,
            Some(json!({"entries": [one_entry(0)], "maxResults": requested})),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case maxResults={requested}: expected {expected_status}, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_the_modelled_catalog_is_refused_never_answered() {
    let router = router();
    let (asset_id, speed) = double_property(&router, "Fan", "Fan1", "Pitch").await;
    write_samples(&router, &asset_id, &speed, &[(3_000, 0, None)]).await;

    // Rows take each enumerated member of an entry one step outside the catalogue
    // the model publishes: `Quality` and `TimeOrdering`. Whether the refusal lands
    // as a request-level constraint failure or as this entry's own
    // `InvalidRequestException`, what must never happen is an answer: the emulator
    // cannot filter or order on a value it has no definition for.
    let cases: [(&str, &str, Value); 2] = [
        ("qualities", "qualities", json!(["PERFECT"])),
        ("timeOrdering", "timeOrdering", json!("SIDEWAYS")),
    ];

    for (case, member, value) in cases {
        let mut entry =
            json!({"entryId": "c", "assetId": asset_id, "propertyId": speed});
        entry[member] = value.clone();
        let (status, body) = call(
            &router,
            "POST",
            BATCH_HISTORY,
            Some(json!({"entries": [entry]})),
        )
        .await;
        if status == 200 {
            let refused = find_entry(&body["errorEntries"], "c").unwrap_or_else(|| {
                panic!("case {case:?}: {member}={value} was answered rather than refused: {body}")
            });
            assert_eq!(
                refused["errorCode"].as_str(),
                Some("InvalidRequestException"),
                "case {case:?}: expected errorCode InvalidRequestException, got {refused}"
            );
            assert_eq!(
                count(&body["successEntries"]),
                0,
                "case {case:?}: a refused entry must not also succeed, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a 400 refusal, got {status} {body}"
            );
        }
    }
}
