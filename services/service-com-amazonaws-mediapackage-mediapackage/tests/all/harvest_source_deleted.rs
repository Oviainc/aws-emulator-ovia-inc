// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what removing an origin endpoint does to the harvest jobs that
//! were harvesting it.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//!
//! The rule under test: a harvest exports content its origin endpoint has published, so
//! work this service accepted and has not finished cannot complete once the resource it
//! reads from is removed — removing an origin endpoint settles every job still harvesting
//! it into the modelled failure status and leaves it owing no further transition — while
//! the submitted job RECORD survives the removal unchanged, with the ids it was submitted
//! under, because a submitted job is the history of a harvest and not a reference this
//! service resolves on a read. So the jobs that harvested an endpoint never refuse its
//! delete: an id they carry "cannot be changed after the HarvestJob is submitted" and this
//! service binds no operation that removes a stored job, so a job holding its endpoint
//! back would hold it back forever.
//!
//! The rows of the first suite span what the rule generalises over: how many jobs named
//! the endpoint (none, one, several), and what the clock had already made of each at the
//! moment of the delete (still harvesting, already finished, both at once). The second
//! suite spans the Region pairs the scope generalises over.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

/// The crate's shared test client, plus the router builder that also hands back the shared
/// engine: a case here has to hold the transition a submitted job is owed open, so the
/// window in which a job is still harvesting is deterministic rather than raced against
/// the wall clock.
use crate::channel_lifecycle::{
    call, call_in, create_channel, emulator_with_engine as fixture, error_code,
    CROSS_ACCOUNT_HARVEST_ROLE, REGION,
};
/// Writing the emulator's OWN stored due instant lives once, in the harvest-job suite.
use crate::harvest_jobs::{hold_transitions_open, make_transitions_due};

/// The channel whose endpoint every case below removes, and that endpoint.
const HARVEST_CHANNEL: &str = "harvest-channel";
const HARVEST_ENDPOINT: &str = "harvest-endpoint";

/// A second channel and endpoint, untouched by the delete: the control that shows the
/// removal reaches exactly the jobs of the endpoint removed and no others.
const OTHER_CHANNEL: &str = "other-channel";
const OTHER_ENDPOINT: &str = "other-endpoint";

/// The job harvesting [`OTHER_ENDPOINT`].
const CONTROL_JOB: &str = "control-job";

/// One `CreateHarvestJob` body: the window and the destination are fixed here, because
/// what they generalise over is the harvest-job suite's own subject
/// (`tests/all/harvest_jobs.rs`); what this suite varies is the jobs' number and status.
///
/// Shared with the sibling suite; see [`create_endpoint_in`].
pub(crate) fn job_body(id: &str, endpoint: &str) -> Value {
    json!({
        "id": id,
        "originEndpointId": endpoint,
        "startTime": "2026-01-01T00:00:00Z",
        "endTime": "2026-01-01T01:00:00Z",
        "s3Destination": {
            "bucketName": "harvest-bucket",
            "manifestKey": format!("{id}/index.m3u8"),
            "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
        }
    })
}

/// Create the channel `id` in `region`, failing the test if that does not succeed.
///
/// Shared with the sibling suite; see [`create_endpoint_in`].
pub(crate) async fn create_channel_in(router: &MultiServiceRouter, region: &str, id: &str) {
    let body = json!({ "id": id });
    let (status, body) = call_in(router, region, "POST", "/channels", Some(&body)).await;
    assert_eq!(
        status, 200,
        "creating the fixture channel {id:?} in {region:?} expected 200, got {status} {body:?}"
    );
}

/// Create the origin endpoint `id` of `channel` in `region`, failing the test if that does
/// not succeed.
///
/// Shared with the sibling suite for the OTHER write that takes a harvest's source away
/// (`tests/all/harvest_source_window_closed.rs`), so the two drive the same fixture.
pub(crate) async fn create_endpoint_in(router: &MultiServiceRouter, region: &str, id: &str, channel: &str) {
    // A harvest exports the content the endpoint's STARTOVER window retains, so a
    // harvestable endpoint keeps one: `tests/all/harvest_startover.rs` is the suite for
    // that precondition.
    let (status, body) = call_in(
        router,
        region,
        "POST",
        "/origin_endpoints",
        Some(&json!({
            "channelId": channel,
            "id": id,
            "hlsPackage": {},
            "startoverWindowSeconds": 86_400
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the fixture endpoint {id:?} in {region:?} expected 200, got {status} {body:?}"
    );
}

/// Submit the harvest job `id` on `endpoint` in `region`, asserting it was accepted and
/// started out harvesting, and answer the record the submit returned.
///
/// Shared with the sibling suite; see [`create_endpoint_in`].
pub(crate) async fn submit(router: &MultiServiceRouter, region: &str, id: &str, endpoint: &str) -> Value {
    let (status, created) = call_in(
        router,
        region,
        "POST",
        "/harvest_jobs",
        Some(&job_body(id, endpoint)),
    )
    .await;
    assert_eq!(
        status, 200,
        "submitting the fixture job {id:?} on {endpoint:?} in {region:?} expected 200, got \
         {status} {created:?}"
    );
    assert_eq!(
        created.get("status").and_then(Value::as_str),
        Some(state::HARVEST_IN_PROGRESS),
        "a submitted job starts {}, got {created:?}",
        state::HARVEST_IN_PROGRESS
    );
    created
}

/// The status a `DescribeHarvestJob` body reports.
pub(crate) fn status_of(body: &Value) -> Option<&str> {
    body.get("status").and_then(Value::as_str)
}

/// `body` without its `status`: everything about a job record that a status transition must
/// leave exactly as the submit stored it.
pub(crate) fn without_status(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("status");
    }
    copy
}

/// One row of the first suite: the jobs harvesting the endpoint when it is removed, and the
/// jobs that had already finished harvesting it by then.
struct Row {
    /// What a failing assertion calls this row.
    label: &'static str,
    unfinished: &'static [&'static str],
    finished: &'static [&'static str],
}

/// The rows differ along how many jobs named the removed endpoint and what the clock had
/// already made of each: none at all, one still harvesting, several still harvesting, one
/// that had already finished, and a mix of both.
const ROWS: &[Row] = &[
    Row {
        label: "no job named the endpoint",
        unfinished: &[],
        finished: &[],
    },
    Row {
        label: "one job still harvesting it",
        unfinished: &["harvest-1"],
        finished: &[],
    },
    Row {
        label: "three jobs still harvesting it",
        unfinished: &["harvest-1", "harvest_2", "Harvest3"],
        finished: &[],
    },
    Row {
        label: "one job that had already finished",
        unfinished: &[],
        finished: &["done-1"],
    },
    Row {
        label: "finished and unfinished jobs",
        unfinished: &["harvest-1", "harvest-4"],
        finished: &["done-1", "done_2"],
    },
];

#[tokio::test]
async fn removing_an_endpoint_fails_the_harvests_still_running_against_it() {
    for row in ROWS {
        let case = row.label;
        let (router, engine) = fixture();
        create_channel(&router, HARVEST_CHANNEL).await;
        create_channel(&router, OTHER_CHANNEL).await;
        create_endpoint_in(&router, REGION, HARVEST_ENDPOINT, HARVEST_CHANNEL).await;
        create_endpoint_in(&router, REGION, OTHER_ENDPOINT, OTHER_CHANNEL).await;

        // The jobs that finish BEFORE the delete: submitted, then made due and read, so it
        // is the emulator's own clock-owed transition that moved them.
        let mut finished = Vec::new();
        for id in row.finished {
            submit(&router, REGION, id, HARVEST_ENDPOINT).await;
            make_transitions_due(&engine);
            let (status, body) = call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading job {id:?} expected 200, got {status} {body:?}"
            );
            assert_eq!(
                status_of(&body),
                Some(state::HARVEST_FAILED),
                "case {case:?}: job {id:?} must have settled on its own to {}, got {body:?}",
                state::HARVEST_FAILED
            );
            finished.push((*id, body));
        }

        // The jobs still harvesting at the delete, and the control job on the endpoint that
        // survives it. Each submitted job's owed transition is held open, so what the
        // delete finds is a job that has not finished.
        let mut unfinished = Vec::new();
        for id in row.unfinished {
            let created = submit(&router, REGION, id, HARVEST_ENDPOINT).await;
            hold_transitions_open(&engine);
            unfinished.push((*id, created));
        }
        let control = submit(&router, REGION, CONTROL_JOB, OTHER_ENDPOINT).await;
        hold_transitions_open(&engine);

        // The jobs that the jobs-still-harvesting rows are about really are still
        // harvesting when the delete arrives.
        for (id, _) in &unfinished {
            let (status, body) = call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading job {id:?} expected 200, got {status} {body:?}"
            );
            assert_eq!(
                status_of(&body),
                Some(state::HARVEST_IN_PROGRESS),
                "case {case:?}: job {id:?} must still be {} before the delete, got {body:?}",
                state::HARVEST_IN_PROGRESS
            );
        }

        // The delete itself: the jobs that harvested the endpoint never refuse it, however
        // many there are and whatever status the clock left them in.
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/origin_endpoints/{HARVEST_ENDPOINT}"),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: the jobs that harvested the endpoint must not refuse its delete, \
             expected 202, got {status} {body:?}"
        );
        let (status, error) = call(
            &router,
            "GET",
            &format!("/origin_endpoints/{HARVEST_ENDPOINT}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: the removed endpoint must answer 404, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {error:?}"
        );

        // A job that had not finished harvesting the removed endpoint can never finish it:
        // it carries the modelled failure status, owes nothing further, and keeps every
        // other member the submit stored — the ids among them.
        for (id, created) in &unfinished {
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: job {id:?} must still be readable once its endpoint is gone, \
                 got {status} {described:?}"
            );
            assert_eq!(
                status_of(&described),
                Some(state::HARVEST_FAILED),
                "case {case:?}: job {id:?} was still harvesting the removed endpoint, so it \
                 must be {}, got {described:?}",
                state::HARVEST_FAILED
            );
            assert_eq!(
                without_status(&described),
                without_status(created),
                "case {case:?}: job {id:?} must keep the record it was submitted with, got \
                 {described:?}"
            );
            let (_, again) = call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                again, described,
                "case {case:?}: job {id:?} reached a terminal status, so it must not move again"
            );
        }

        // A job that had already finished harvesting the endpoint did so while the endpoint
        // was there: the delete leaves its record exactly as it stood.
        for (id, before) in &finished {
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: job {id:?} must still be readable once its endpoint is gone, \
                 got {status} {described:?}"
            );
            assert_eq!(
                &described, before,
                "case {case:?}: job {id:?} had already finished, so the delete must change \
                 nothing about it, got {described:?}"
            );
        }

        // The control job harvests an endpoint this delete did not touch, so it is still
        // harvesting: the removal reaches the jobs of the removed endpoint and no others.
        let (status, described) =
            call(&router, "GET", &format!("/harvest_jobs/{CONTROL_JOB}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the control job expected 200, got {status} {described:?}"
        );
        assert_eq!(
            status_of(&described),
            Some(state::HARVEST_IN_PROGRESS),
            "case {case:?}: the job of the surviving endpoint must still be {}, got \
             {described:?}",
            state::HARVEST_IN_PROGRESS
        );
        assert_eq!(
            without_status(&described),
            without_status(&control),
            "case {case:?}: the control job's record must be untouched, got {described:?}"
        );

        // With its endpoints gone the channel is deletable, and the submitted jobs keep
        // reporting the ids they were submitted under: the record is the history of a
        // harvest, which no later delete rewrites.
        let (status, body) =
            call(&router, "DELETE", &format!("/channels/{HARVEST_CHANNEL}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: with no endpoints left the channel delete expected 202, got \
             {status} {body:?}"
        );
        for (id, _) in finished.iter().chain(unfinished.iter()) {
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: job {id:?} must still be readable, got {status} {described:?}"
            );
            assert_eq!(
                described.get("channelId").and_then(Value::as_str),
                Some(HARVEST_CHANNEL),
                "case {case:?}: job {id:?} must keep the channel id it was submitted with, got \
                 {described:?}"
            );
            assert_eq!(
                described.get("originEndpointId").and_then(Value::as_str),
                Some(HARVEST_ENDPOINT),
                "case {case:?}: job {id:?} must keep the endpoint id it was submitted with, got \
                 {described:?}"
            );
        }
    }
}

/// The Region pairs the scope is driven over: the Region the write that takes the harvest
/// source away is served in ("home") and a second Region holding the same ids ("foreign").
/// The rows span the emulator's own default Region as home and as foreign, two Regions on
/// different continents, and two Regions sharing a prefix — so a scope comparing Region
/// prefixes would fail a row.
///
/// Shared with the sibling suite; see [`create_endpoint_in`].
pub(crate) const REGION_PAIRS: &[(&str, &str)] = &[
    ("eu-west-1", "us-east-1"),
    ("us-east-1", "eu-central-1"),
    ("us-east-1", "us-east-2"),
];

#[tokio::test]
async fn a_removed_endpoint_fails_only_the_harvests_of_its_own_region() {
    for (home, foreign) in REGION_PAIRS {
        let case = format!("{home} -> {foreign}");
        let (router, engine) = fixture();
        // The same ids in both Regions: each names a separate resource, and the job of each
        // Region harvests that Region's endpoint.
        for region in [home, foreign] {
            create_channel_in(&router, region, HARVEST_CHANNEL).await;
            create_endpoint_in(&router, region, HARVEST_ENDPOINT, HARVEST_CHANNEL).await;
            submit(&router, region, "harvest-1", HARVEST_ENDPOINT).await;
            hold_transitions_open(&engine);
        }

        let (status, body) = call_in(
            &router,
            home,
            "DELETE",
            &format!("/origin_endpoints/{HARVEST_ENDPOINT}"),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {body:?}"
        );

        let (status, described) =
            call_in(&router, home, "GET", "/harvest_jobs/harvest-1", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the home job expected 200, got {status} {described:?}"
        );
        assert_eq!(
            status_of(&described),
            Some(state::HARVEST_FAILED),
            "case {case:?}: the job harvesting the removed endpoint must be {}, got \
             {described:?}",
            state::HARVEST_FAILED
        );

        // The foreign Region's endpoint is a separate resource, so its job is still
        // harvesting it.
        let (status, described) =
            call_in(&router, foreign, "GET", "/harvest_jobs/harvest-1", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the foreign job expected 200, got {status} {described:?}"
        );
        assert_eq!(
            status_of(&described),
            Some(state::HARVEST_IN_PROGRESS),
            "case {case:?}: the job of the other Region's endpoint must still be {}, got \
             {described:?}",
            state::HARVEST_IN_PROGRESS
        );
        let (status, body) = call_in(
            &router,
            foreign,
            "GET",
            &format!("/origin_endpoints/{HARVEST_ENDPOINT}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the other Region's endpoint must still be readable, got {status} \
             {body:?}"
        );
    }
}
