// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the MediaPackage HarvestJob operations.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//!
//! The rules under test, one per suite below:
//!
//! 1. `CreateHarvestJob` persists the job and returns the STORED record: the ARN it
//!    minted, the `CreatedAt` it minted, and the `ChannelId` of the origin endpoint being
//!    harvested (the request carries none, so it is computed from state); every later
//!    read of it — `DescribeHarvestJob`, each `ListHarvestJobs` entry — renders that same
//!    record.
//! 2. A submitted job is `IN_PROGRESS` and reaches its terminal status only when the
//!    CLOCK makes its harvest due, at a stored instant 250-750 ms after the submit; a
//!    read reports what the clock made due and never advances a job itself. WHICH
//!    terminal status it reaches is decided by whether the harvest's output reached its
//!    destination, which `tests/all/harvest_export.rs` is the suite for — the submits
//!    here name buckets no object store holds, so their harvests produce nothing and the
//!    jobs fail. (Nothing holds an object store HERE, so nothing answers about the
//!    destination either and no submit is refused for it; the destination pre-flight is
//!    `tests/all/harvest_destination.rs`, and the roles below are of another account for
//!    the reason `channel_lifecycle::CROSS_ACCOUNT_HARVEST_ROLE` states.)
//! 3. `ListHarvestJobs` filters by `IncludeChannelId` or by `IncludeStatus` — never by
//!    both, which it refuses (`tests/all/mutually_exclusive_list_filters.rs`) — BEFORE
//!    paging, so the filtered pages carry exactly the matching jobs, in id order, with a
//!    `NextToken` only while more remain, and a status filter follows the status the
//!    clock left each job in.
//! 4. A submit this service cannot carry out changes nothing: an `OriginEndpointId`
//!    naming no stored endpoint and an `Id` already in use are refused with
//!    `UnprocessableEntityException` (422), and a read of an id the service holds no job
//!    for is `NotFoundException` (404).

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::ops::types::shared_types as types;
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

/// The crate's one router builder, in the form that also hands back the shared engine:
/// the handle is what lets a case read the emulator's OWN stored due instant for a
/// transition the clock owes, and hold it open or bring it forward, so the transient
/// window is deterministic instead of raced against the wall clock.
use crate::channel_lifecycle::{
    call, create_channel, emulator_with_engine as fixture, error_code, ACCOUNT,
    CROSS_ACCOUNT_HARVEST_ROLE, OTHER_ACCOUNT, REGION,
};

/// The channels this suite's endpoints package, and the endpoint each channel holds.
///
/// Two channels, so the `IncludeChannelId` filter has something to exclude, and an
/// endpoint per channel plus a second one on the first channel, so a job's `ChannelId`
/// cannot be guessed from its `OriginEndpointId`.
const ENDPOINTS: &[(&str, &str)] = &[
    ("news-hls", "news-channel"),
    ("news-dash", "news-channel"),
    ("sports-hls", "sports-channel"),
];

/// Create every channel and origin endpoint in [`ENDPOINTS`], failing the test if that
/// does not succeed.
async fn create_endpoints(router: &MultiServiceRouter) {
    let channels: BTreeSet<&str> = ENDPOINTS.iter().map(|(_, channel)| *channel).collect();
    for channel in channels {
        create_channel(router, channel).await;
    }
    // Each fixture endpoint retains a STARTOVER window, because that is the content a
    // harvest exports: a submission naming an endpoint with startover playback disabled is
    // refused, which `tests/all/harvest_startover.rs` is the suite for. The windows differ
    // per endpoint so no case here depends on one particular span.
    for (index, (endpoint, channel)) in ENDPOINTS.iter().enumerate() {
        let startover_window_seconds = 3_600 * (index as i64 + 1);
        let (status, body) = call(
            router,
            "POST",
            "/origin_endpoints",
            Some(&json!({
                "channelId": channel,
                "id": endpoint,
                "hlsPackage": {},
                "startoverWindowSeconds": startover_window_seconds
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "creating the fixture endpoint {endpoint:?} expected 200, got {status} {body:?}"
        );
    }
}

/// The channel the fixture endpoint `endpoint` packages, as [`ENDPOINTS`] records it.
fn channel_of(endpoint: &str) -> &'static str {
    ENDPOINTS
        .iter()
        .find(|(id, _)| *id == endpoint)
        .map(|(_, channel)| *channel)
        .unwrap_or_else(|| panic!("{endpoint:?} is not one of this suite's fixture endpoints"))
}

/// One `CreateHarvestJob` request body, with the endpoint it harvests.
struct Submit {
    /// What a failing assertion calls this row.
    label: &'static str,
    id: &'static str,
    endpoint: &'static str,
    body: Value,
}

/// The table of submits the suites below drive.
///
/// The rows span the class the rules generalise over: WHICH endpoint (and so which
/// channel) is harvested, the FORM of the harvest window (a `Z` instant, a numeric UTC
/// offset, a fractional second, a window of a different length), the SHAPE of the S3
/// destination (a bare key, a nested key, a key with a dot) and the shape of the id
/// (hyphens, underscores, digits, mixed case).
fn submits() -> Vec<Submit> {
    vec![
        Submit {
            label: "Z instants, bare manifest key",
            id: "harvest-1",
            endpoint: "news-hls",
            body: json!({
                "startTime": "2026-01-01T00:00:00Z",
                "endTime": "2026-01-01T01:00:00Z",
                "s3Destination": {
                    "bucketName": "harvest-bucket",
                    "manifestKey": "index.m3u8",
                    "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
                }
            }),
        },
        Submit {
            label: "numeric UTC offset, nested manifest key",
            id: "harvest_2",
            endpoint: "news-dash",
            body: json!({
                "startTime": "2026-02-03T10:15:00+00:00",
                "endTime": "2026-02-03T10:45:30+00:00",
                "s3Destination": {
                    "bucketName": "another-harvest-bucket",
                    "manifestKey": "harvests/2026/02/03/index.mpd",
                    "roleArn": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/service-role/Harvester")
                }
            }),
        },
        Submit {
            label: "fractional seconds, long window",
            id: "Harvest3",
            endpoint: "sports-hls",
            body: json!({
                "startTime": "2026-03-04T23:59:59.250Z",
                "endTime": "2026-03-05T05:00:00.000Z",
                "s3Destination": {
                    "bucketName": "sports.harvests",
                    "manifestKey": "live-to-vod/game-7/index.m3u8",
                    "roleArn": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/Harvest-Role_2")
                }
            }),
        },
        Submit {
            label: "same endpoint as another job, one-minute window",
            id: "harvest-4",
            endpoint: "news-hls",
            body: json!({
                "startTime": "2026-04-05T06:07:00Z",
                "endTime": "2026-04-05T06:08:00Z",
                "s3Destination": {
                    "bucketName": "harvest-bucket",
                    "manifestKey": "minute/index.m3u8",
                    "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
                }
            }),
        },
    ]
}

/// The request body for `row`: the row's members plus the two the request keys the job by.
fn request_body(row: &Submit) -> Value {
    let mut body = row.body.clone();
    body["id"] = json!(row.id);
    body["originEndpointId"] = json!(row.endpoint);
    body
}

/// Submit every row of [`submits`], holding each submitted job's owed transition open so
/// the suite's reads see the transient `IN_PROGRESS` window deterministically. Answers the
/// create response of each row, keyed by id.
async fn submit_all(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> Vec<(&'static str, Value)> {
    let mut created = Vec::new();
    for row in submits() {
        let (status, body) = call(router, "POST", "/harvest_jobs", Some(&request_body(&row))).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateHarvestJob expected 200, got {status} {body:?}",
            row.label
        );
        hold_transitions_open(engine);
        created.push((row.id, body));
    }
    created
}

/// The span between `from` and the instant the emulator stored as the due date of `id`'s
/// owed status transition. `None` once the job has settled.
///
/// This reads the emulator's OWN record of when the transition falls due, so the band
/// asserted below is a property of the stored due instant rather than of how fast the
/// test ran.
fn pending_span(engine: &engine::InMemoryEngine, id: &str, from: SystemTime) -> Option<Duration> {
    let store = state::service_state(engine).expect("the mediapackage state store");
    let store = store.lock();
    store
        .harvest_job(REGION, id)?
        .next_status_due
        .and_then(|due| due.duration_since(from).ok())
}

/// Push every outstanding harvest-job transition an hour out, so the transient status
/// stays observable for as long as a case needs it.
///
/// This writes only the due instant the emulator itself stores; the transition is still
/// the emulator's own, applied by the emulator, when the clock reaches that instant.
///
/// Shared with the sibling harvest suites, so no suite writes a second one.
pub(crate) fn hold_transitions_open(engine: &engine::InMemoryEngine) {
    set_due(engine, SystemTime::now() + Duration::from_secs(3_600));
}

/// Make every outstanding harvest-job transition due, so the next call materialises it.
/// See [`hold_transitions_open`] for why writing the due instant is not the transition.
pub(crate) fn make_transitions_due(engine: &engine::InMemoryEngine) {
    set_due(engine, SystemTime::now() - Duration::from_secs(1));
}

/// Point every outstanding harvest-job transition at `due`.
fn set_due(engine: &engine::InMemoryEngine, due: SystemTime) {
    let store = state::service_state(engine).expect("the mediapackage state store");
    let mut store = store.lock();
    for job in store.harvest_jobs.values_mut() {
        if let Some(pending) = job.next_status_due.as_mut() {
            *pending = due;
        }
    }
}

/// The `harvestJobs` array of a `ListHarvestJobs` body.
fn listed(body: &Value) -> Vec<Value> {
    body.get("harvestJobs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("a ListHarvestJobs body carries harvestJobs, got {body:?}"))
}

/// The `id` of each entry of a `ListHarvestJobs` body, in the order served.
fn listed_ids(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|job| {
            job.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Walk every page of `path` at `max_results`, answering the ids served in order.
async fn walk_pages(router: &MultiServiceRouter, path: &str, max_results: usize) -> Vec<String> {
    let separator = if path.contains('?') { '&' } else { '?' };
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut uri = format!("{path}{separator}maxResults={max_results}");
        if let Some(token) = &token {
            uri.push_str(&format!("&nextToken={token}"));
        }
        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: ListHarvestJobs expected 200, got {status} {body:?}"
        );
        let page = listed_ids(&body);
        assert!(
            page.len() <= max_results,
            "case {uri:?}: a page must carry at most maxResults={max_results} entries, got {page:?}"
        );
        ids.extend(page);
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// Rule 1: a submit persists the job, and every read renders that record
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_submitted_harvest_job_is_read_back_from_state_unchanged() {
    let (router, engine) = fixture();
    create_endpoints(&router).await;

    for row in submits() {
        let case = row.label;
        let before = SystemTime::now();
        let (status, created) =
            call(&router, "POST", "/harvest_jobs", Some(&request_body(&row))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );

        // The transition the clock owes this job is due a NON-ZERO span out, inside the
        // project's 250-750 ms band, so the submitted job is observably IN_PROGRESS.
        let span = pending_span(&engine, row.id, before).unwrap_or_else(|| {
            panic!("case {case:?}: the submitted job must carry a pending transition")
        });
        assert!(
            (250..=750).contains(&span.as_millis()),
            "case {case:?}: the owed transition must be due 250-750 ms out, got {span:?}"
        );
        hold_transitions_open(&engine);

        // What the service itself minted: the ARN for this region and account, an
        // ISO-8601 CreatedAt, and the submitted job's transient status.
        let expected_arn = format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:harvest_jobs/{}", row.id);
        assert_eq!(
            created.get("arn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected arn {expected_arn:?}, got {created:?}"
        );
        let created_at = created
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            created_at.len() >= 20 && created_at.contains('T') && created_at.ends_with('Z'),
            "case {case:?}: createdAt must be an ISO-8601 instant, got {created_at:?}"
        );
        assert_eq!(
            created.get("status").and_then(Value::as_str),
            Some(state::HARVEST_IN_PROGRESS),
            "case {case:?}: a submitted job must be {}, got {created:?}",
            state::HARVEST_IN_PROGRESS
        );

        // The ChannelId is the harvested ENDPOINT's channel, which the request does not
        // carry: it is read off the stored endpoint, never echoed.
        let expected_channel = channel_of(row.endpoint);
        assert_eq!(
            created.get("channelId").and_then(Value::as_str),
            Some(expected_channel),
            "case {case:?}: channelId must be the harvested endpoint's channel \
             {expected_channel:?}, got {created:?}"
        );
        assert!(
            request_body(&row).get("channelId").is_none(),
            "case {case:?}: the request must not carry a channelId for the response to echo"
        );

        // Everything the request did supply is stored as sent.
        let request = request_body(&row);
        for member in [
            "id",
            "originEndpointId",
            "startTime",
            "endTime",
            "s3Destination",
        ] {
            assert_eq!(
                created.get(member),
                request.get(member),
                "case {case:?}: {member} must be stored as sent, got {created:?}"
            );
        }

        // DescribeHarvestJob renders the same stored record, byte for byte, and a
        // repeated read does not change it.
        let uri = format!("/harvest_jobs/{}", row.id);
        let (status, described) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeHarvestJob expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: DescribeHarvestJob must return exactly what was stored"
        );
        let (_, described_again) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            described_again, created,
            "case {case:?}: a repeated DescribeHarvestJob must not change the record"
        );

        // And the listing renders that same record.
        let (status, body) = call(&router, "GET", "/harvest_jobs", None).await;
        assert_eq!(status, 200, "case {case:?}: ListHarvestJobs expected 200");
        let entry = listed(&body)
            .into_iter()
            .find(|job| job.get("id").and_then(Value::as_str) == Some(row.id))
            .unwrap_or_else(|| panic!("case {case:?}: the submitted job must be listed, got {body:?}"));
        assert_eq!(
            entry, created,
            "case {case:?}: the listed job must be the stored record"
        );
    }

    // Every submitted job is listed, once.
    let (_, body) = call(&router, "GET", "/harvest_jobs", None).await;
    let mut ids = listed_ids(&body);
    ids.sort();
    let mut expected: Vec<String> = submits().iter().map(|row| row.id.to_string()).collect();
    expected.sort();
    assert_eq!(
        ids, expected,
        "every submitted job must be listed exactly once, got {ids:?}"
    );
    assert_eq!(
        body.get("nextToken"),
        None,
        "ListHarvestJobs must omit nextToken when no more remain, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the clock, not the reader, moves a harvest job's status
// ---------------------------------------------------------------------------

/// The statuses this suite names are the ones the model declares, read from the generated
/// `Status` value list rather than written out here.
#[test]
fn the_statuses_this_service_reports_are_the_modelled_ones() {
    for status in [
        state::HARVEST_IN_PROGRESS,
        state::HARVEST_SUCCEEDED,
        state::HARVEST_FAILED,
    ] {
        assert!(
            types::STATUS_VALUES.contains(&status),
            "case {status:?}: every status this service reports must be one the model \
             declares, got {:?}",
            types::STATUS_VALUES
        );
    }
    for status in [state::HARVEST_SUCCEEDED, state::HARVEST_FAILED] {
        assert!(
            state::HARVEST_TERMINAL_STATUSES.contains(&status),
            "case {status:?}: a status a harvest job settles into must be terminal, got {:?}",
            state::HARVEST_TERMINAL_STATUSES
        );
    }
    assert!(
        !state::HARVEST_TERMINAL_STATUSES.contains(&state::HARVEST_IN_PROGRESS),
        "a submitted job's status must be the non-terminal one, got {:?}",
        state::HARVEST_TERMINAL_STATUSES
    );
}

#[tokio::test]
async fn a_harvest_job_leaves_in_progress_only_when_the_clock_makes_the_transition_due() {
    let (router, engine) = fixture();
    create_endpoints(&router).await;
    let created = submit_all(&router, &engine).await;

    // While the transition is not yet due, every read — however many times it is made —
    // reports the transient status: a read does not advance a job because it was polled.
    for (id, body) in &created {
        for round in 1..=3 {
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {id:?} read {round}: expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described, *body,
                "case {id:?} read {round}: a read before the transition is due must report \
                 the submitted record unchanged"
            );
        }
    }

    // Once the clock has made it due, the job carries the terminal status its harvest
    // earned — these destinations are buckets no object store holds, so nothing could be
    // written and the jobs failed — and nothing else about the record moved.
    make_transitions_due(&engine);
    for (id, body) in &created {
        let (status, described) = call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(state::HARVEST_FAILED),
            "case {id:?}: once the harvest is due and its output could not be written the \
             job must be {}, got {described:?}",
            state::HARVEST_FAILED
        );
        for member in [
            "arn",
            "channelId",
            "createdAt",
            "endTime",
            "id",
            "originEndpointId",
            "s3Destination",
            "startTime",
        ] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {id:?}: the transition must not change {member}, got {described:?}"
            );
        }

        // And it stays there: a terminal status owes nothing further.
        let (_, again) = call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
        assert_eq!(
            again, described,
            "case {id:?}: a job in a terminal status must not move again"
        );
    }
}

#[tokio::test]
async fn the_owed_transition_settles_on_its_own_wall_clock() {
    let (router, _engine) = fixture();
    create_endpoints(&router).await;
    let row = submits().remove(0);
    let (status, created) = call(&router, "POST", "/harvest_jobs", Some(&request_body(&row))).await;
    assert_eq!(status, 200, "the fixture submit expected 200, got {created:?}");

    // Nothing in the emulator sleeps for the transition and nothing here writes the due
    // instant: waiting past the span the submit stored is enough for the next read to
    // report what the clock made due.
    std::thread::sleep(state::HARVEST_SETTLE_AFTER + Duration::from_millis(250));
    let (status, described) = call(
        &router,
        "GET",
        &format!("/harvest_jobs/{}", row.id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"read after the span elapsed\": expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some(state::HARVEST_FAILED),
        "case \"read after the span elapsed\": expected {}, got {described:?}",
        state::HARVEST_FAILED
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the list filters, and paging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_filters_select_by_channel_and_by_the_status_the_clock_left() {
    let (router, engine) = fixture();
    create_endpoints(&router).await;
    submit_all(&router, &engine).await;

    /// The ids of the submits harvesting `channel`, in id order.
    fn ids_for_channel(channel: Option<&str>) -> Vec<String> {
        let mut ids: Vec<String> = submits()
            .iter()
            .filter(|row| channel.is_none_or(|channel| channel_of(row.endpoint) == channel))
            .map(|row| row.id.to_string())
            .collect();
        ids.sort();
        ids
    }

    // Before the transition is due every job is IN_PROGRESS, so that is the status the
    // filter selects on and no terminal status matches anything.
    for (filter, expected) in [
        (String::new(), ids_for_channel(None)),
        (
            format!("?includeStatus={}", state::HARVEST_IN_PROGRESS),
            ids_for_channel(None),
        ),
        (
            format!("?includeStatus={}", state::HARVEST_SUCCEEDED),
            Vec::new(),
        ),
        (
            format!("?includeStatus={}", state::HARVEST_FAILED),
            Vec::new(),
        ),
        (
            "?includeChannelId=news-channel".to_string(),
            ids_for_channel(Some("news-channel")),
        ),
        (
            "?includeChannelId=sports-channel".to_string(),
            ids_for_channel(Some("sports-channel")),
        ),
        (
            "?includeChannelId=holds-nothing".to_string(),
            Vec::new(),
        ),
        ("?includeStatus=NOT_A_STATUS".to_string(), Vec::new()),
    ] {
        let uri = format!("/harvest_jobs{filter}");
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            listed_ids(&body),
            expected,
            "case {uri:?}: expected {expected:?}, got {body:?}"
        );
        assert_eq!(
            body.get("nextToken"),
            None,
            "case {uri:?}: no more remain, so no nextToken, got {body:?}"
        );
        for job in listed(&body) {
            assert_eq!(
                job.get("status").and_then(Value::as_str),
                Some(state::HARVEST_IN_PROGRESS),
                "case {uri:?}: every entry must carry the status the clock left, got {job:?}"
            );
        }
    }

    // Neither filter may be combined with the other: the service refuses the pair rather
    // than intersecting them, so no row above carries both.
    // `tests/all/mutually_exclusive_list_filters.rs` is the suite for that rule.
    for status in [state::HARVEST_IN_PROGRESS, state::HARVEST_FAILED] {
        let uri = format!("/harvest_jobs?includeChannelId=news-channel&includeStatus={status}");
        let (code, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(code, 422, "case {uri:?}: expected 422, got {code} {body:?}");
        assert_eq!(
            error_code(&body),
            "UnprocessableEntityException",
            "case {uri:?}: expected UnprocessableEntityException, got {body:?}"
        );
    }

    // Once the clock has made the harvests due the SAME filters follow the new status:
    // the transient one now selects nothing and the terminal one these harvests earned
    // selects everything.
    make_transitions_due(&engine);
    for (filter, expected) in [
        (
            format!("?includeStatus={}", state::HARVEST_IN_PROGRESS),
            Vec::new(),
        ),
        (
            format!("?includeStatus={}", state::HARVEST_FAILED),
            ids_for_channel(None),
        ),
        (
            "?includeChannelId=sports-channel".to_string(),
            ids_for_channel(Some("sports-channel")),
        ),
    ] {
        let uri = format!("/harvest_jobs{filter}");
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            listed_ids(&body),
            expected,
            "case {uri:?}: expected {expected:?} once the transition is due, got {body:?}"
        );
    }
}

#[tokio::test]
async fn paging_serves_every_harvest_job_once_in_id_order() {
    let (router, engine) = fixture();
    create_endpoints(&router).await;
    submit_all(&router, &engine).await;

    let mut all: Vec<String> = submits().iter().map(|row| row.id.to_string()).collect();
    all.sort();
    let mut news: Vec<String> = submits()
        .iter()
        .filter(|row| channel_of(row.endpoint) == "news-channel")
        .map(|row| row.id.to_string())
        .collect();
    news.sort();

    // Rows differ along the page size: the service's minimum, sizes that split the set
    // into several pages, one that holds it exactly, and the service's maximum.
    for max_results in [
        state::MIN_PAGE_SIZE,
        2,
        all.len(),
        state::MAX_PAGE_SIZE,
    ] {
        let walked = walk_pages(&router, "/harvest_jobs", max_results).await;
        assert_eq!(
            walked, all,
            "case maxResults={max_results}: paging must serve every job exactly once in id \
             order, got {walked:?}"
        );
        // The filter applies BEFORE paging, so a filtered walk serves exactly the
        // matching jobs however small the page.
        let walked = walk_pages(&router, "/harvest_jobs?includeChannelId=news-channel", max_results)
            .await;
        assert_eq!(
            walked, news,
            "case maxResults={max_results}, includeChannelId=news-channel: expected {news:?}, \
             got {walked:?}"
        );
    }

    // A nextToken this service did not issue is refused rather than served as page one.
    for token in ["not-a-token", "abc", "zz", "%%%"] {
        let uri = format!("/harvest_jobs?nextToken={token}");
        let (status, error) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 422,
            "case {uri:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {uri:?}: expected UnprocessableEntityException, got {error:?}"
        );
    }

    // The page-size bound this service enforces: each published bound is accepted and one
    // past it is refused.
    for max_results in [state::MIN_PAGE_SIZE, state::MAX_PAGE_SIZE] {
        let uri = format!("/harvest_jobs?maxResults={max_results}");
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: a page size at the bound must be accepted, got {status} {body:?}"
        );
    }
    for max_results in [state::MIN_PAGE_SIZE - 1, state::MAX_PAGE_SIZE + 1] {
        let uri = format!("/harvest_jobs?maxResults={max_results}");
        let (status, error) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 422,
            "case {uri:?}: a page size past the bound must be refused, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {uri:?}: expected UnprocessableEntityException, got {error:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a submit this service cannot carry out changes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_harvest_job_this_service_cannot_submit_is_refused() {
    let (router, engine) = fixture();
    create_endpoints(&router).await;
    let created = submit_all(&router, &engine).await;
    let template = submits().remove(0);

    // An OriginEndpointId naming no stored endpoint: nothing to harvest. It is a value in
    // the REQUEST the caller has to fix, so 422 — not the 404 a read of an absent job
    // gets. Rows differ along the shape of the absent endpoint id.
    for endpoint in ["no-such-endpoint", "news-hl", "NEWS-HLS", "news-hls-2"] {
        let case = format!("absent endpoint {endpoint}");
        let mut body = request_body(&template);
        body["id"] = json!("rejected");
        body["originEndpointId"] = json!(endpoint);
        let (status, error) = call(&router, "POST", "/harvest_jobs", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (status, _) = call(&router, "GET", "/harvest_jobs/rejected", None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused submit must not have stored anything, got {status}"
        );
    }

    // An Id already in use is refused, never an overwrite: the stored job is untouched.
    for (id, stored) in &created {
        let case = format!("duplicate {id}");
        let mut body = request_body(&template);
        body["id"] = json!(id);
        body["startTime"] = json!("2027-12-31T00:00:00Z");
        let (status, error) = call(&router, "POST", "/harvest_jobs", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (_, described) = call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
        assert_eq!(
            described, *stored,
            "case {case:?}: the refused duplicate must not have changed the stored job"
        );
    }

    // A required member the request omits is refused the same way, by the service's own
    // input gate. Rows differ along WHICH required member is missing, including one
    // nested inside the required S3Destination.
    let required_rows: Vec<(&str, Value)> = vec![
        ("no id", json!({ "originEndpointId": "news-hls", "startTime": "2026-01-01T00:00:00Z", "endTime": "2026-01-01T01:00:00Z", "s3Destination": { "bucketName": "b", "manifestKey": "k", "roleArn": "r" } })),
        ("no originEndpointId", json!({ "id": "rejected", "startTime": "2026-01-01T00:00:00Z", "endTime": "2026-01-01T01:00:00Z", "s3Destination": { "bucketName": "b", "manifestKey": "k", "roleArn": "r" } })),
        ("no startTime", json!({ "id": "rejected", "originEndpointId": "news-hls", "endTime": "2026-01-01T01:00:00Z", "s3Destination": { "bucketName": "b", "manifestKey": "k", "roleArn": "r" } })),
        ("no endTime", json!({ "id": "rejected", "originEndpointId": "news-hls", "startTime": "2026-01-01T00:00:00Z", "s3Destination": { "bucketName": "b", "manifestKey": "k", "roleArn": "r" } })),
        ("no s3Destination", json!({ "id": "rejected", "originEndpointId": "news-hls", "startTime": "2026-01-01T00:00:00Z", "endTime": "2026-01-01T01:00:00Z" })),
        ("no roleArn", json!({ "id": "rejected", "originEndpointId": "news-hls", "startTime": "2026-01-01T00:00:00Z", "endTime": "2026-01-01T01:00:00Z", "s3Destination": { "bucketName": "b", "manifestKey": "k" } })),
    ];
    for (case, body) in required_rows {
        let (status, error) = call(&router, "POST", "/harvest_jobs", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (status, _) = call(&router, "GET", "/harvest_jobs/rejected", None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused submit must not have stored anything, got {status}"
        );
    }

    // A caller-supplied string past the bound this service stores is refused, and the
    // same string AT the bound is accepted: the bound refuses only what is past it.
    let past_bound = "x".repeat(state::MAX_STRING_CHARS + 1);
    let at_bound = "x".repeat(state::MAX_STRING_CHARS);
    for member in ["startTime", "endTime", "originEndpointId"] {
        let case = format!("{member} past the bound");
        let mut body = request_body(&template);
        body["id"] = json!("rejected");
        body[member] = json!(past_bound);
        let (status, error) = call(&router, "POST", "/harvest_jobs", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
    }
    for member in ["bucketName", "manifestKey", "roleArn"] {
        let case = format!("s3Destination.{member} past the bound");
        let mut body = request_body(&template);
        body["id"] = json!("rejected");
        body["s3Destination"][member] = json!(past_bound);
        let (status, error) = call(&router, "POST", "/harvest_jobs", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
    }
    let mut accepted = request_body(&template);
    accepted["id"] = json!("at-bound");
    accepted["s3Destination"]["manifestKey"] = json!(at_bound);
    let (status, body) = call(&router, "POST", "/harvest_jobs", Some(&accepted)).await;
    assert_eq!(
        status, 200,
        "case \"manifestKey at the bound\": a value at the bound must be accepted, got \
         {status} {body:?}"
    );

    // A read of an id this service holds no job for is 404. Rows differ along the shape
    // of the absent id.
    for id in ["absent", "harvest-", "HARVEST-1", "harvest-1-2", "at_bound"] {
        let uri = format!("/harvest_jobs/{id}");
        let (status, error) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {uri:?}: expected 404, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "NotFoundException",
            "case {uri:?}: expected NotFoundException, got {error:?}"
        );
    }
}
