// SPDX-License-Identifier: CC0-1.0

//! A harvest is accepted only from an origin endpoint that RETAINS a startover window.
//!
//! RULE under test: a `CreateHarvestJob` exports content the origin endpoint has already
//! published, which the endpoint keeps only for as long as its startover (time-shift)
//! window retains it — so a submission naming an endpoint whose `StartoverWindowSeconds`
//! retains nothing (the model: "If not specified, startover playback will be disabled for
//! the OriginEndpoint", i.e. a window of zero seconds) is refused with the modelled
//! `UnprocessableEntityException` (HTTP 422, `smithy_query(query="operation",
//! operation="CreateHarvestJob")`) keyed by the member that names the endpoint, and
//! persists NOTHING: neither `DescribeHarvestJob` nor `ListHarvestJobs` reports a job for
//! that id afterwards. An endpoint retaining any window at all is harvestable.
//!
//! The window the stored endpoint carries is what decides, not the window its create
//! request carried: the second suite drives the same endpoint through
//! `UpdateOriginEndpoint` and submits after each write.
//!
//! `tests/all/harvest_job_rejections.rs` covers the other two preconditions (the window's
//! direction, the destination role); `tests/all/harvest_jobs.rs` covers what an accepted
//! job IS. Every case here drives real HTTP through the `MultiServiceRouter` — the same
//! dispatch the served emulator uses — and the harness (`call`, `emulator`, …) is the one
//! `tests/all/channel_lifecycle.rs` owns.

use serde_json::{json, Value};
// The bound the service applies, taken from its own constants rather than restated here:
// the member's documented default (startover disabled) and the smallest value that has it
// enabled.
use service_com_amazonaws_mediapackage_mediapackage::state::{
    DEFAULT_STARTOVER_WINDOW_SECONDS, MIN_ENABLED_STARTOVER_WINDOW_SECONDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{
    call, create_channel, emulator, error_code, CROSS_ACCOUNT_HARVEST_ROLE,
};

/// The channel every endpoint below packages.
const CHANNEL_ID: &str = "startover-channel";

/// One endpoint row: the startover window its create requests (or none at all), the
/// packaging configuration it is created with, and whether a harvest of it is accepted.
struct Row {
    case: &'static str,
    /// `StartoverWindowSeconds` as the create sends it; `None` leaves the member out, so
    /// the service fills its own default.
    startover: Option<i32>,
    /// The packaging configuration member the create carries — exactly one is required —
    /// so the rule is asserted across the packaging shapes rather than on one of them.
    package: &'static str,
    harvestable: bool,
}

/// Rows spanning what the rule generalises over: whether the member is sent at all, a
/// window that retains nothing, the smallest window that retains anything, and windows of
/// ten minutes, an hour, a day, the fortnight the console offers and the largest the
/// member's `__integer` can carry — each on a different packaging configuration. Every
/// window below is one the service accepts (`tests/all/startover_window_bound.rs` owns the
/// values it refuses), so what each row tests is the harvest rule and not the bound.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the member left out, so the service default applies",
            startover: None,
            package: "hlsPackage",
            harvestable: false,
        },
        Row {
            case: "a window of zero seconds, which retains nothing",
            startover: Some(DEFAULT_STARTOVER_WINDOW_SECONDS),
            package: "dashPackage",
            harvestable: false,
        },
        Row {
            case: "the member left out on a different packaging configuration",
            startover: None,
            package: "mssPackage",
            harvestable: false,
        },
        Row {
            case: "the smallest window with startover enabled",
            startover: Some(MIN_ENABLED_STARTOVER_WINDOW_SECONDS),
            package: "hlsPackage",
            harvestable: true,
        },
        Row {
            case: "a ten-minute window",
            startover: Some(600),
            package: "mssPackage",
            harvestable: true,
        },
        Row {
            case: "an hour-long window",
            startover: Some(3_600),
            package: "dashPackage",
            harvestable: true,
        },
        Row {
            case: "a day-long window",
            startover: Some(86_400),
            package: "hlsPackage",
            harvestable: true,
        },
        Row {
            case: "a fortnight-long window",
            startover: Some(1_209_600),
            package: "dashPackage",
            harvestable: true,
        },
        Row {
            case: "the largest window the member can carry",
            startover: Some(i32::MAX),
            package: "hlsPackage",
            harvestable: true,
        },
    ]
}

/// The body that creates an origin endpoint `id` with `package` and, when `startover` is
/// `Some`, that startover window.
fn endpoint_body(id: &str, package: &str, startover: Option<i32>) -> Value {
    let mut body = json!({ "channelId": CHANNEL_ID, "id": id });
    body[package] = json!({});
    if let Some(seconds) = startover {
        body["startoverWindowSeconds"] = json!(seconds);
    }
    body
}

/// The body that updates an origin endpoint to `package` and, when `startover` is `Some`,
/// that startover window.
///
/// `UpdateOriginEndpoint` carries the id as an `@httpLabel` and models no `ChannelId` at
/// all (`smithy_query(query="operation",operation="UpdateOriginEndpoint")`), so neither
/// travels in its body.
fn update_body(package: &str, startover: Option<i32>) -> Value {
    let mut body = json!({});
    body[package] = json!({});
    if let Some(seconds) = startover {
        body["startoverWindowSeconds"] = json!(seconds);
    }
    body
}

/// Create the endpoint of `row` and assert the window it reports back is the one the rule
/// will be judged on.
async fn create_endpoint(router: &MultiServiceRouter, row: &Row, id: &str) {
    let body = endpoint_body(id, row.package, row.startover);
    let (status, created) = call(router, "POST", "/origin_endpoints", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateOriginEndpoint expected 200, got {status} {created:?}",
        row.case
    );
    let expected = row.startover.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS);
    assert_eq!(
        created.get("startoverWindowSeconds"),
        Some(&json!(expected)),
        "case {:?}: the stored endpoint must report startoverWindowSeconds {expected}, \
         got {created:?}",
        row.case
    );
}

/// A `CreateHarvestJob` body for the job `id` harvesting `endpoint`.
fn submission(id: &str, endpoint: &str) -> Value {
    json!({
        "id": id,
        "originEndpointId": endpoint,
        "startTime": "2026-01-01T00:00:00Z",
        "endTime": "2026-01-01T01:00:00Z",
        "s3Destination": {
            "bucketName": "startover-harvests",
            "manifestKey": "harvests/index.m3u8",
            "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
        }
    })
}

/// Submit a harvest job.
async fn submit(router: &MultiServiceRouter, id: &str, endpoint: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        "/harvest_jobs",
        Some(&submission(id, endpoint)),
    )
    .await
}

/// Every id `ListHarvestJobs` currently reports.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "GET", "/harvest_jobs", None).await;
    assert_eq!(
        status, 200,
        "ListHarvestJobs expected 200, got {status} {body:?}"
    );
    match body.get("harvestJobs") {
        None | Some(Value::Null) => Vec::new(),
        Some(jobs) => jobs
            .as_array()
            .unwrap_or_else(|| panic!("harvestJobs must be a list, got {jobs:?}"))
            .iter()
            .map(|job| {
                job.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
    }
}

/// Assert `refusal` is the modelled 422 this rule answers with, keyed by the member that
/// names the endpoint.
fn assert_refused(case: &str, status: u16, refusal: &Value) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert!(
        refusal.pointer("/errors/originEndpointId").is_some(),
        "case {case:?}: the refusal must be keyed by originEndpointId, got {refusal:?}"
    );
    let message = refusal
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("originEndpointId"),
        "case {case:?}: the refusal's sentence must name the member the caller has to \
         fix, got {message:?}"
    );
}

/// Assert the service holds no harvest job under `id`: neither the read of that id nor the
/// listing reports one.
async fn assert_nothing_persisted(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, described) = call(router, "GET", &format!("/harvest_jobs/{id}"), None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: a refused submission must persist no job, got {status} {described:?}"
    );
    assert_eq!(
        error_code(&described),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {described:?}"
    );
    let listed = listed_ids(router).await;
    assert!(
        !listed.contains(&id.to_string()),
        "case {case:?}: a refused submission must leave no listing entry, got {listed:?}"
    );
}

#[tokio::test]
async fn a_harvest_is_accepted_only_from_an_endpoint_retaining_a_startover_window() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    let mut accepted: Vec<String> = Vec::new();
    for (index, row) in rows().iter().enumerate() {
        let endpoint = format!("startover-endpoint-{index}");
        let id = format!("startover-harvest-{index}");
        create_endpoint(&router, row, &endpoint).await;

        let (status, answered) = submit(&router, &id, &endpoint).await;
        if row.harvestable {
            assert_eq!(
                status, 200,
                "case {:?}: CreateHarvestJob expected 200, got {status} {answered:?}",
                row.case
            );
            // Accepted: the job is persisted, and the read renders the stored record.
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {:?}: DescribeHarvestJob expected 200, got {status} {described:?}",
                row.case
            );
            assert_eq!(
                described.get("originEndpointId"),
                Some(&json!(endpoint)),
                "case {:?}: the stored job must harvest {endpoint:?}, got {described:?}",
                row.case
            );
            accepted.push(id.clone());
        } else {
            assert_refused(row.case, status, &answered);
            assert_nothing_persisted(&router, row.case, &id).await;
        }

        // Whatever this row did, the listing reports exactly the jobs that were accepted:
        // a refusal neither adds an entry nor removes one.
        let mut listed = listed_ids(&router).await;
        listed.sort();
        let mut expected = accepted.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {:?}: ListHarvestJobs must report exactly the accepted jobs, got \
             {listed:?}",
            row.case
        );
    }
}

/// One step of the update sequence: the window `UpdateOriginEndpoint` writes, and whether a
/// harvest submitted straight afterwards is accepted.
struct Step {
    case: &'static str,
    startover: Option<i32>,
    harvestable: bool,
}

/// The sequence: a harvestable endpoint turned off, back on, off again by omission (the
/// update fills the member's default the way a create does), and on with a different span.
const STEPS: &[Step] = &[
    Step {
        case: "created with a day-long window",
        startover: Some(86_400),
        harvestable: true,
    },
    Step {
        case: "updated to a window of zero seconds",
        startover: Some(0),
        harvestable: false,
    },
    Step {
        case: "updated back to an hour-long window",
        startover: Some(3_600),
        harvestable: true,
    },
    Step {
        case: "updated with the member left out",
        startover: None,
        harvestable: false,
    },
    Step {
        case: "updated to the smallest enabled window",
        startover: Some(MIN_ENABLED_STARTOVER_WINDOW_SECONDS),
        harvestable: true,
    },
];

#[tokio::test]
async fn the_window_the_stored_endpoint_carries_now_is_what_decides() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;
    let endpoint = "startover-updated-endpoint";

    for (index, step) in STEPS.iter().enumerate() {
        let (method, uri, body) = if index == 0 {
            (
                "POST",
                "/origin_endpoints".to_string(),
                endpoint_body(endpoint, "hlsPackage", step.startover),
            )
        } else {
            (
                "PUT",
                format!("/origin_endpoints/{endpoint}"),
                update_body("hlsPackage", step.startover),
            )
        };
        let (status, written) = call(&router, method, &uri, Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {:?}: writing the endpoint expected 200, got {status} {written:?}",
            step.case
        );
        let expected = step.startover.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS);
        assert_eq!(
            written.get("startoverWindowSeconds"),
            Some(&json!(expected)),
            "case {:?}: the written endpoint must report startoverWindowSeconds \
             {expected}, got {written:?}",
            step.case
        );

        let id = format!("updated-harvest-{index}");
        let (status, answered) = submit(&router, &id, endpoint).await;
        if step.harvestable {
            assert_eq!(
                status, 200,
                "case {:?}: CreateHarvestJob expected 200, got {status} {answered:?}",
                step.case
            );
        } else {
            assert_refused(step.case, status, &answered);
            assert_nothing_persisted(&router, step.case, &id).await;
        }
    }
}
