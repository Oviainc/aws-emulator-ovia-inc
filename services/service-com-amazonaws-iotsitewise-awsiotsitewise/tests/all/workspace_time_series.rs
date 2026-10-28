// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the WORKSPACE-scoped forms of `ListTimeSeries`,
//! `DescribeTimeSeries` and `DeleteTimeSeries`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: an operation scoped to a workspace is answered from
//! the time series stored UNDER that workspace — the streams ingestion recorded a
//! workspace for, which `GetCaptureData` reads back by the same identifiers a
//! summary of one reports — so a workspace that holds streams lists, describes and
//! deletes them, each series is addressable only inside its own workspace, and only
//! an identifier that names none of that workspace's series (or a workspace this
//! store does not hold) is the modelled refusal.
//!
//! The rows span what the rule generalises over: the shortest and the longest
//! `TimeSeriesId` the model admits, a stream with an alias and one with none, alias
//! trees that share a prefix and one that does not, one and several stored
//! segments, and the same alias held by two workspaces at once.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `REGION`) is the one
//! `asset_lifecycle` already defines; this suite reuses it rather than growing a
//! second.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, CaptureSegment};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, hex_token, property_catalog, property_id,
    router_with_engine, REGION,
};

/// The workspace most rows below live in.
const PLANT_ONE: &str = "plant-one";

/// A second workspace, so one identifier can be held by two at once.
const PLANT_TWO: &str = "plant-two";

/// The `VideoDataType` the seeded media is in. Source: `smithy_query(query="shape",
/// shape="VideoDataType")` — the enum holds exactly this one value.
const MEDIA_TYPE: &str = "VIDEO-MP4";

/// The `PropertyDataType` a video time series reports. Source:
/// `smithy_query(query="shape", shape="PropertyDataType")` — STRING | INTEGER |
/// DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON.
const VIDEO: &str = "VIDEO";

/// The shortest `TimeSeriesId` the model admits: 36 characters
/// (`smithy_query(query="shape", shape="TimeSeriesId")` — length 36..73).
const ID_SHORTEST: &str = "7f3a1c24-9b85-4d17-8e62-0a5c41d9b7e1";

/// The longest one: 73 characters, two 36-character ids joined by one separator.
const ID_LONGEST: &str =
    "7f3a1c24-9b85-4d17-8e62-0a5c41d9b7e2.1b2c3d4e-5f60-4a71-8b92-0c3d4e5f6a7b";

/// A 47-character id — between the two published bounds.
const ID_MIDDLING: &str = "7f3a1c24-9b85-4d17-8e62-0a5c41d9b7e3-camera-04";

/// The id of the stream no alias names.
const ID_UNALIASED: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

/// The id the second workspace holds under the SAME alias as [`ID_SHORTEST`].
const ID_OTHER_WORKSPACE: &str = "0f1e2d3c-4b5a-4968-8776-6554433221aa";

/// One seeded stream: which workspace holds it, how it is identified, and the spans
/// of video stored for it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    workspace: &'static str,
    time_series_id: &'static str,
    alias: Option<&'static str>,
    /// `(start seconds, end seconds)` per stored segment.
    segments: &'static [(i64, i64)],
}

/// The seeded streams. Deliberately varied: the two published id bounds and one
/// between them, an aliased and an unaliased stream, two aliases sharing a prefix
/// and one in another tree, one and two segments, and one alias held by both
/// workspaces.
const SEEDED: &[Row] = &[
    Row {
        case: "aliased, shortest id",
        workspace: PLANT_ONE,
        time_series_id: ID_SHORTEST,
        alias: Some("/plant/1/line/2/camera"),
        segments: &[(1_700_000_000, 1_700_000_010)],
    },
    Row {
        case: "aliased, longest id, two segments",
        workspace: PLANT_ONE,
        time_series_id: ID_LONGEST,
        alias: Some("/plant/1/line/3/camera"),
        segments: &[(1_700_000_000, 1_700_000_020), (1_700_000_030, 1_700_000_040)],
    },
    Row {
        case: "aliased, another tree",
        workspace: PLANT_ONE,
        time_series_id: ID_MIDDLING,
        alias: Some("/yard/gate/camera"),
        segments: &[(1_600_000_000, 1_600_000_030)],
    },
    Row {
        case: "no alias",
        workspace: PLANT_ONE,
        time_series_id: ID_UNALIASED,
        alias: None,
        segments: &[(1_500_000_000, 1_500_000_005)],
    },
    Row {
        case: "another workspace, same alias",
        workspace: PLANT_TWO,
        time_series_id: ID_OTHER_WORKSPACE,
        alias: Some("/plant/1/line/2/camera"),
        segments: &[(1_700_000_000, 1_700_000_010)],
    },
];

/// The bytes seeded for one segment of one stream — distinct per stream and span, so
/// an answer that served another stream's video cannot pass.
fn media(time_series_id: &str, start: i64) -> Vec<u8> {
    format!("mp4:{time_series_id}@{start}").into_bytes()
}

/// A modelled `TimeInNanos` at a whole second.
fn at(seconds: i64) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: seconds,
        offset_in_nanos: Some(0),
    }
}

/// The same, on the wire.
fn instant(seconds: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": 0 })
}

/// Create a workspace with the service's default encryption, over the API.
async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the workspace create to be accepted, got {status} {body}"
    );
}

/// A router over a fresh engine carrying the two workspaces, every [`SEEDED`]
/// stream, and one asset whose properties are the ACCOUNT's own time series — so a
/// scoped answer that leaked the account's series, or the other way round, fails.
///
/// The streams are seeded through the producer entry point on this service's state,
/// the one writer of a workspace's video, which is what `GetCaptureData` reads back.
async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String, Value) {
    let (router, engine) = router_with_engine();
    create_workspace(&router, PLANT_ONE).await;
    create_workspace(&router, PLANT_TWO).await;
    {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        for row in SEEDED {
            for (start, end) in row.segments {
                let stored = store.put_capture_segment(
                    row.workspace,
                    row.time_series_id,
                    row.alias,
                    MEDIA_TYPE,
                    CaptureSegment {
                        start_time: at(*start),
                        end_time: at(*end),
                        data: media(row.time_series_id, *start),
                    },
                    std::time::SystemTime::now(),
                );
                assert_eq!(
                    stored,
                    Ok(()),
                    "case {:?}: seeding the segment at {start} was refused",
                    row.case
                );
            }
        }
    }
    let model_id = create_model(&router, "WorkspaceBoiler", property_catalog()).await;
    let asset_id = create_asset(&router, "WorkspaceBoiler1", &model_id).await;
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    (router, engine, asset_id, asset)
}

/// Every seeded row of `workspace`.
fn rows_of(workspace: &str) -> Vec<Row> {
    SEEDED
        .iter()
        .copied()
        .filter(|row| row.workspace == workspace)
        .collect()
}

/// `GET /timeseries?<query>`.
async fn list_time_series(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/timeseries?{query}"), None).await
}

/// `GET /timeseries/describe?<query>`.
async fn describe_time_series(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/timeseries/describe?{query}"), None).await
}

/// `POST /timeseries/delete?<query>`.
async fn delete_time_series(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "POST", &format!("/timeseries/delete?{query}"), None).await
}

/// `POST /workspaces/{workspaceName}/get-capture-data` over `[start, end)`.
async fn get_capture_data(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/get-capture-data"),
        Some(body),
    )
    .await
}

/// The `timeSeriesId`s a listing body reports, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    body["TimeSeriesSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a summary list in {body}"))
        .iter()
        .map(|summary| {
            summary["timeSeriesId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary with no timeSeriesId in {body}"))
                .to_string()
        })
        .collect()
}

/// One listing body's summary for `time_series_id`.
fn summary_of(body: &Value, time_series_id: &str) -> Value {
    body["TimeSeriesSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a summary list in {body}"))
        .iter()
        .find(|summary| summary["timeSeriesId"].as_str() == Some(time_series_id))
        .unwrap_or_else(|| panic!("no summary for {time_series_id} in {body}"))
        .clone()
}

/// Assert `summary` reports `row` as the time series it is.
fn assert_reports_row(summary: &Value, row: &Row, label: &str) {
    let case = row.case;
    assert_eq!(
        summary["timeSeriesId"].as_str(),
        Some(row.time_series_id),
        "case {case:?}/{label}: unexpected timeSeriesId in {summary}"
    );
    assert_eq!(
        summary.get("alias").and_then(Value::as_str),
        row.alias,
        "case {case:?}/{label}: expected alias {:?}, got {summary}",
        row.alias
    );
    // A workspace's video stream is owned by no asset property, which is what makes
    // it DISASSOCIATED, and its `PropertyDataType` is VIDEO; `dataTypeSpec` is
    // "required for time series that have the STRUCT data type" and so is absent.
    assert_eq!(
        summary["dataType"].as_str(),
        Some(VIDEO),
        "case {case:?}/{label}: expected dataType {VIDEO}, got {summary}"
    );
    assert!(
        summary.get("dataTypeSpec").and_then(Value::as_str).is_none(),
        "case {case:?}/{label}: a video series carries no dataTypeSpec, got {summary}"
    );
    assert!(
        summary.get("assetId").and_then(Value::as_str).is_none()
            && summary.get("propertyId").and_then(Value::as_str).is_none(),
        "case {case:?}/{label}: no asset property owns this stream, got {summary}"
    );
    assert_eq!(
        summary["timeSeriesArn"].as_str(),
        Some(
            format!(
                "arn:aws:iotsitewise:{REGION}:{}:time-series/{}",
                engine::EMULATOR_ACCOUNT_ID, row.time_series_id
            )
            .as_str()
        ),
        "case {case:?}/{label}: unexpected timeSeriesArn in {summary}"
    );
    let created = summary["timeSeriesCreationDate"].as_f64();
    let updated = summary["timeSeriesLastUpdateDate"].as_f64();
    assert!(
        created.is_some_and(|seconds| seconds > 0.0),
        "case {case:?}/{label}: expected a creation date, got {summary}"
    );
    assert!(
        updated.is_some_and(|seconds| created.is_some_and(|created| seconds >= created)),
        "case {case:?}/{label}: the last-update date cannot precede the creation date, \
         got {summary}"
    );
}

#[tokio::test]
async fn a_workspace_scoped_listing_reports_the_series_the_workspace_holds() {
    let (router, _engine, _asset_id, asset) = seeded().await;

    for workspace in [PLANT_ONE, PLANT_TWO] {
        let (status, body) = list_time_series(&router, &format!("workspaceName={workspace}")).await;
        assert_eq!(
            status, 200,
            "case {workspace:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["workspaceName"].as_str(),
            Some(workspace),
            "case {workspace:?}: a scoped listing reports the workspace it was scoped to, \
             got {body}"
        );

        // Exactly the streams this workspace holds — no more (another workspace's,
        // or the account's own) and no fewer.
        let rows = rows_of(workspace);
        let mut listed = listed_ids(&body);
        listed.sort();
        let mut expected: Vec<String> = rows
            .iter()
            .map(|row| row.time_series_id.to_string())
            .collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {workspace:?}: expected the workspace's own streams, got {body}"
        );

        for row in &rows {
            assert_reports_row(&summary_of(&body, row.time_series_id), row, "listed");
        }

        // The order a `nextToken` resumes through: oldest first, ties broken by id.
        let order: Vec<(f64, String)> = body["TimeSeriesSummaries"]
            .as_array()
            .expect("a summary list")
            .iter()
            .map(|summary| {
                (
                    summary["timeSeriesCreationDate"].as_f64().unwrap_or(0.0),
                    summary["timeSeriesId"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        for pair in order.windows(2) {
            assert!(
                pair[0].0 < pair[1].0 || (pair[0].0 == pair[1].0 && pair[0].1 < pair[1].1),
                "case {workspace:?}: the listing must be ordered by creation date then id, \
                 got {order:?}"
            );
        }

        // A listing is a read: the second answer is the first.
        let (status, again) = list_time_series(&router, &format!("workspaceName={workspace}")).await;
        assert_eq!(status, 200, "case {workspace:?}: re-read got {status} {again}");
        assert_eq!(
            again, body,
            "case {workspace:?}: a second listing differs from the first: {again}"
        );
    }

    // Each listed stream is the one `GetCaptureData` reads back under that
    // identifier — the listing and the data plane name one stream.
    for row in SEEDED {
        let (status, body) = get_capture_data(
            &router,
            row.workspace,
            json!({
                "timeSeriesId": row.time_series_id,
                "startTime": instant(row.segments[0].0),
                "endTime": instant(row.segments[0].1),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the listed stream must be readable, got {status} {body}",
            row.case
        );
        assert_eq!(
            body["data"].as_str(),
            Some(
                ::aws_smithy_types::base64::encode(media(row.time_series_id, row.segments[0].0))
                    .as_str()
            ),
            "case {:?}: unexpected video in {body}",
            row.case
        );
    }

    // An UNSCOPED listing enumerates the account's own series instead: every stored
    // asset property, and none of the workspaces' streams.
    let (status, account) = list_time_series(&router, "maxResults=250").await;
    assert_eq!(status, 200, "the account listing: got {status} {account}");
    assert!(
        account.get("workspaceName").and_then(Value::as_str).is_none(),
        "the account listing names no workspace, got {account}"
    );
    let account_ids = listed_ids(&account);
    for row in SEEDED {
        assert!(
            !account_ids.contains(&row.time_series_id.to_string()),
            "case {:?}: a workspace's stream is not one of the account's series, got {account}",
            row.case
        );
    }
    assert!(
        account_ids.contains(&format!(
            "{}.{}",
            asset["assetId"].as_str().unwrap_or(""),
            property_id(&asset["assetProperties"], "Temperature")
        )),
        "the account listing must still report an asset property's series, got {account}"
    );
}

#[tokio::test]
async fn the_filters_and_the_page_walk_hold_for_a_workspace_scoped_listing() {
    let (router, _engine, asset_id, _asset) = seeded().await;

    // `aliasPrefix` keeps the streams whose alias starts with it; a stream no alias
    // names is kept by no prefix. (case, prefix, the ids expected)
    let prefixes: [(&str, &str, Vec<&str>); 5] = [
        ("one tree", "/plant/1/line/2", vec![ID_SHORTEST]),
        (
            "a shared prefix",
            "/plant/1/line",
            vec![ID_SHORTEST, ID_LONGEST],
        ),
        ("another tree", "/yard", vec![ID_MIDDLING]),
        ("a whole alias", "/yard/gate/camera", vec![ID_MIDDLING]),
        ("a prefix nothing carries", "/no/such", vec![]),
    ];
    for (case, prefix, expected) in prefixes {
        let (status, body) = list_time_series(
            &router,
            &format!("workspaceName={PLANT_ONE}&aliasPrefix={prefix}"),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        let mut listed = listed_ids(&body);
        listed.sort();
        let mut expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: unexpected streams under {prefix}, got {body}"
        );
    }

    // `timeSeriesType`: a workspace's streams are owned by no asset property, so
    // they are DISASSOCIATED and an ASSOCIATED listing of one is empty. The
    // `assetId` filter keeps only an asset's own series, which none of them is.
    let held: Vec<String> = rows_of(PLANT_ONE)
        .iter()
        .map(|row| row.time_series_id.to_string())
        .collect();
    let types: [(&str, String, usize); 3] = [
        (
            "disassociated",
            "timeSeriesType=DISASSOCIATED".to_string(),
            held.len(),
        ),
        ("associated", "timeSeriesType=ASSOCIATED".to_string(), 0),
        ("an asset's own", format!("assetId={asset_id}"), 0),
    ];
    for (case, filter, expected) in types {
        let (status, body) =
            list_time_series(&router, &format!("workspaceName={PLANT_ONE}&{filter}")).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            listed_ids(&body).len(),
            expected,
            "case {case:?}: expected {expected} streams, got {body}"
        );
    }

    // The page walk: one entry per page reaches every stream exactly once, in the
    // order the unpaged listing reported, and the last page issues no token.
    let (_, whole) = list_time_series(&router, &format!("workspaceName={PLANT_ONE}")).await;
    let expected_order = listed_ids(&whole);
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=expected_order.len() {
        let query = match &token {
            Some(token) => {
                format!("workspaceName={PLANT_ONE}&maxResults=1&nextToken={token}")
            }
            None => format!("workspaceName={PLANT_ONE}&maxResults=1"),
        };
        let (status, body) = list_time_series(&router, &query).await;
        assert_eq!(
            status, 200,
            "case \"page {step}\": expected 200, got {status} {body}"
        );
        let page = listed_ids(&body);
        assert!(
            page.len() <= 1,
            "case \"page {step}\": maxResults=1 must cut the page at one, got {body}"
        );
        walked.extend(page);
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, expected_order,
        "the page walk must reach every stream of the workspace exactly once, in order"
    );
    assert!(
        token.is_none(),
        "the last page of a scoped listing issues no token, got {token:?}"
    );

    // A token this operation never issued is the modelled refusal, not an empty
    // page: text that is not a resume key, and a key no stream of this scope carries.
    for (case, bad) in [
        ("not a token", "zzzz"),
        // The hex of an id the OTHER workspace holds — a resume point outside this
        // enumeration. `hex_token` is the harness's independent encoder of a resume
        // key, not the handler's.
        (
            "another workspace's resume point",
            &hex_token(ID_OTHER_WORKSPACE),
        ),
    ] {
        let (status, body) = list_time_series(
            &router,
            &format!("workspaceName={PLANT_ONE}&nextToken={bad}"),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A workspace this store does not hold is the absent resource it is.
    let (status, body) = list_time_series(&router, "workspaceName=never-opened").await;
    assert_eq!(status, 404, "an absent workspace: got {status} {body}");
    assert_eq!(
        body["message"].as_str(),
        Some("Workspace with ID never-opened not found"),
        "an absent workspace: unexpected message in {body}"
    );
}

#[tokio::test]
async fn a_workspace_scoped_describe_reports_the_series_that_workspace_holds() {
    let (router, _engine, asset_id, asset) = seeded().await;

    // Every aliased stream, named by its alias inside its own workspace: the
    // describe reports what the listing reported for it, plus the workspace.
    for row in SEEDED.iter().filter(|row| row.alias.is_some()) {
        let alias = row.alias.expect("an aliased row");
        let (status, body) = describe_time_series(
            &router,
            &format!("workspaceName={}&alias={alias}", row.workspace),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {body}",
            row.case
        );
        assert_reports_row(&body, row, "described");
        assert_eq!(
            body["workspaceName"].as_str(),
            Some(row.workspace),
            "case {:?}: a scoped describe reports its workspace, got {body}",
            row.case
        );

        let (status, listing) =
            list_time_series(&router, &format!("workspaceName={}", row.workspace)).await;
        assert_eq!(status, 200, "case {:?}: listing got {status}", row.case);
        let summary = summary_of(&listing, row.time_series_id);
        for member in [
            "timeSeriesId",
            "alias",
            "dataType",
            "timeSeriesArn",
            "timeSeriesCreationDate",
            "timeSeriesLastUpdateDate",
        ] {
            assert_eq!(
                body.get(member),
                summary.get(member),
                "case {:?}: the describe and the listing must agree on {member}: \
                 {body} vs {summary}",
                row.case
            );
        }
    }

    // One alias, two workspaces, two streams: each workspace answers with its own.
    let shared = "/plant/1/line/2/camera";
    let (_, one) = describe_time_series(
        &router,
        &format!("workspaceName={PLANT_ONE}&alias={shared}"),
    )
    .await;
    let (_, two) = describe_time_series(
        &router,
        &format!("workspaceName={PLANT_TWO}&alias={shared}"),
    )
    .await;
    assert_ne!(
        one["timeSeriesId"], two["timeSeriesId"],
        "one alias in two workspaces names two streams, got {one} and {two}"
    );

    // What resolves to none of a workspace's streams. (case, query, the message)
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let absent_asset = "11111111-2222-3333-4444-555555555555";
    let refusals: [(&str, String, String); 5] = [
        (
            "an alias only the other workspace holds",
            format!("workspaceName={PLANT_ONE}&alias=/yard/gate/camera-2"),
            "Time Series with ID /yard/gate/camera-2 not found".to_string(),
        ),
        (
            "an alias this workspace does not hold",
            format!("workspaceName={PLANT_TWO}&alias=/yard/gate/camera"),
            "Time Series with ID /yard/gate/camera not found".to_string(),
        ),
        (
            "a workspace nothing created",
            format!("workspaceName=never-opened&alias={shared}"),
            "Workspace with ID never-opened not found".to_string(),
        ),
        (
            "an asset property, which no workspace stream is",
            format!("workspaceName={PLANT_ONE}&assetId={asset_id}&propertyId={temperature}"),
            format!("Time Series with ID {asset_id}.{temperature} not found"),
        ),
        (
            "an asset nothing created",
            format!("workspaceName={PLANT_ONE}&assetId={absent_asset}&propertyId={temperature}"),
            format!("Asset with ID {absent_asset} not found"),
        ),
    ];
    for (case, query, message) in refusals {
        let (status, body) = describe_time_series(&router, &query).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(message.as_str()),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // A request that identifies no one series is invalid whatever scope it names.
    for (case, query) in [
        ("no identifier", format!("workspaceName={PLANT_ONE}")),
        (
            "half a pair",
            format!("workspaceName={PLANT_ONE}&assetId={asset_id}"),
        ),
        (
            "an alias and a pair",
            format!(
                "workspaceName={PLANT_ONE}&alias={shared}&assetId={asset_id}\
                 &propertyId={temperature}"
            ),
        ),
    ] {
        let (status, body) = describe_time_series(&router, &query).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_workspace_scoped_delete_removes_that_workspace_s_series() {
    let (router, _engine, _asset_id, _asset) = seeded().await;
    let doomed = SEEDED[0];
    let alias = doomed.alias.expect("the row is aliased");

    // A workspace this store does not hold, and an alias this workspace does not
    // hold, remove nothing.
    for (case, query, message) in [
        (
            "a workspace nothing created",
            format!("workspaceName=never-opened&alias={alias}"),
            "Workspace with ID never-opened not found".to_string(),
        ),
        (
            "an alias this workspace does not hold",
            format!("workspaceName={PLANT_ONE}&alias=/no/such/camera"),
            "Time Series with ID /no/such/camera not found".to_string(),
        ),
    ] {
        let (status, body) = delete_time_series(&router, &query).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            body["message"].as_str(),
            Some(message.as_str()),
            "case {case:?}: unexpected message in {body}"
        );
    }
    let (status, body) = list_time_series(&router, &format!("workspaceName={PLANT_ONE}")).await;
    assert_eq!(status, 200, "after the refusals: got {status} {body}");
    assert_eq!(
        listed_ids(&body).len(),
        rows_of(PLANT_ONE).len(),
        "a refused delete removes nothing, got {body}"
    );

    // The accepted delete: the stream goes, video and all.
    let (status, body) = delete_time_series(
        &router,
        &format!("workspaceName={PLANT_ONE}&alias={alias}"),
    )
    .await;
    assert_eq!(status, 200, "the delete: got {status} {body}");

    let (status, body) = list_time_series(&router, &format!("workspaceName={PLANT_ONE}")).await;
    assert_eq!(status, 200, "after the delete: got {status} {body}");
    let mut left = listed_ids(&body);
    left.sort();
    let mut expected: Vec<String> = rows_of(PLANT_ONE)
        .iter()
        .filter(|row| row.time_series_id != doomed.time_series_id)
        .map(|row| row.time_series_id.to_string())
        .collect();
    expected.sort();
    assert_eq!(
        left, expected,
        "the deleted stream is gone and every other one stands, got {body}"
    );

    let (status, body) = describe_time_series(
        &router,
        &format!("workspaceName={PLANT_ONE}&alias={alias}"),
    )
    .await;
    assert_eq!(status, 404, "the deleted alias: got {status} {body}");
    assert_eq!(
        body["message"].as_str(),
        Some(format!("Time Series with ID {alias} not found").as_str()),
        "the deleted alias: unexpected message in {body}"
    );

    // Its video goes with it: the data plane no longer resolves that stream.
    let (status, body) = get_capture_data(
        &router,
        PLANT_ONE,
        json!({
            "timeSeriesId": doomed.time_series_id,
            "startTime": instant(doomed.segments[0].0),
            "endTime": instant(doomed.segments[0].1),
        }),
    )
    .await;
    assert_eq!(status, 404, "the deleted stream's video: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "the deleted stream's video: unexpected error code in {body}"
    );

    // Deleting it again is the absence it now is.
    let (status, body) = delete_time_series(
        &router,
        &format!("workspaceName={PLANT_ONE}&alias={alias}"),
    )
    .await;
    assert_eq!(status, 404, "the second delete: got {status} {body}");

    // The OTHER workspace held the same alias, and keeps its own stream and video.
    let (status, body) = describe_time_series(
        &router,
        &format!("workspaceName={PLANT_TWO}&alias={alias}"),
    )
    .await;
    assert_eq!(status, 200, "the neighbour's stream: got {status} {body}");
    assert_eq!(
        body["timeSeriesId"].as_str(),
        Some(ID_OTHER_WORKSPACE),
        "the neighbour's stream: unexpected timeSeriesId in {body}"
    );
    let (status, body) = get_capture_data(
        &router,
        PLANT_TWO,
        json!({
            "timeSeriesId": ID_OTHER_WORKSPACE,
            "startTime": instant(1_700_000_000),
            "endTime": instant(1_700_000_010),
        }),
    )
    .await;
    assert_eq!(status, 200, "the neighbour's video: got {status} {body}");
    assert_eq!(
        body["data"].as_str(),
        Some(
            ::aws_smithy_types::base64::encode(media(ID_OTHER_WORKSPACE, 1_700_000_000)).as_str()
        ),
        "the neighbour's video: unexpected bytes in {body}"
    );
}
