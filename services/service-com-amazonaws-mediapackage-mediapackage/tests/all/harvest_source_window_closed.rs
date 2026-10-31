// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an `UpdateOriginEndpoint` that leaves the endpoint retaining
//! no startover window does to the harvest jobs that were harvesting it.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//!
//! RULE under test: a harvest exports content its origin endpoint has already published,
//! which the endpoint keeps only for as long as its startover (time-shift) window retains
//! it, so ANY write that leaves the endpoint retaining nothing takes that source away and
//! settles every job that has not finished harvesting it into the modelled failure status,
//! owing no further transition — while a job that had already finished, a job of another
//! endpoint and a job of another Region are untouched, and the job records themselves keep
//! every member they were submitted with. The write that removes the endpoint
//! (`tests/all/harvest_source_deleted.rs`) and the write that closes its window are the
//! same rule: this suite is the second one, so an endpoint can never be left both refusing
//! a new submission for retaining nothing (`tests/all/harvest_startover.rs`) and reporting
//! a harvest in progress against it.
//!
//! The rows of the first suite span what the rule generalises over: HOW the update leaves
//! the endpoint retaining nothing (a window of zero seconds, the member left out so the
//! service fills its default) against updates that leave it retaining content (shortened
//! to the smallest enabled window, widened, left where it was), and how many jobs named the
//! endpoint and what the clock had already made of each (none, one, several, already
//! finished, both at once). The second suite spans the Region pairs the scope generalises
//! over.

use serde_json::{json, Value};
// The bound the service itself applies, taken from its own constants rather than restated
// here: the member's documented default (startover disabled) and the smallest window with
// startover playback enabled.
use service_com_amazonaws_mediapackage_mediapackage::state::{
    self, DEFAULT_STARTOVER_WINDOW_SECONDS, MIN_ENABLED_STARTOVER_WINDOW_SECONDS,
};

/// The crate's shared test client, plus the router builder that also hands back the shared
/// engine: a case here has to hold the transition a submitted job is owed open, so the
/// window in which a job is still harvesting is deterministic rather than raced against the
/// wall clock.
use crate::channel_lifecycle::{
    call, call_in, emulator_with_engine as fixture, error_code, REGION,
};
/// Writing the emulator's OWN stored due instant lives once, in the harvest-job suite.
use crate::harvest_jobs::{hold_transitions_open, make_transitions_due};
/// The fixture the sibling write-path suite already owns, driven here against the other
/// write: one channel/endpoint/job fixture for both paths, so neither grows a second copy.
use crate::harvest_source_deleted::{
    create_channel_in, create_endpoint_in, job_body, status_of, submit, without_status,
    REGION_PAIRS,
};

/// The channel whose endpoint every case below updates, and that endpoint.
const HARVEST_CHANNEL: &str = "window-channel";
const HARVEST_ENDPOINT: &str = "window-endpoint";

/// A second channel and endpoint, untouched by the update: the control that shows the write
/// reaches exactly the jobs of the endpoint written and no others.
const OTHER_CHANNEL: &str = "other-window-channel";
const OTHER_ENDPOINT: &str = "other-window-endpoint";

/// The job harvesting [`OTHER_ENDPOINT`].
const CONTROL_JOB: &str = "control-job";

/// The body that updates [`HARVEST_ENDPOINT`] and, when `startover` is `Some`, sets that
/// startover window.
///
/// `UpdateOriginEndpoint` carries the id as an `@httpLabel` and models no `ChannelId` at
/// all (`smithy_query(query="operation",operation="UpdateOriginEndpoint")`), so neither
/// travels in its body; exactly one packaging configuration does.
fn update_body(startover: Option<i32>) -> Value {
    let mut body = json!({ "hlsPackage": {} });
    if let Some(seconds) = startover {
        body["startoverWindowSeconds"] = json!(seconds);
    }
    body
}

/// Whether an endpoint written with `startover` retains startover content afterwards,
/// decided the way the service decides it: the member's own default fills an omitted
/// member, and anything below the smallest enabled window retains nothing.
fn retains_content(startover: Option<i32>) -> bool {
    startover.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS) >= MIN_ENABLED_STARTOVER_WINDOW_SECONDS
}

/// One row of the first suite: the window the update writes, the jobs still harvesting the
/// endpoint when it lands, and the jobs that had already finished harvesting it by then.
struct Row {
    /// What a failing assertion calls this row.
    label: &'static str,
    /// `StartoverWindowSeconds` as the update sends it; `None` leaves the member out, so
    /// the service fills its own default.
    startover: Option<i32>,
    unfinished: &'static [&'static str],
    finished: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        label: "a window of zero seconds, one job still harvesting",
        startover: Some(DEFAULT_STARTOVER_WINDOW_SECONDS),
        unfinished: &["harvest-1"],
        finished: &[],
    },
    Row {
        label: "the member left out, one job still harvesting",
        startover: None,
        unfinished: &["harvest-1"],
        finished: &[],
    },
    Row {
        label: "the member left out, three jobs still harvesting",
        startover: None,
        unfinished: &["harvest-1", "harvest_2", "Harvest3"],
        finished: &[],
    },
    Row {
        label: "a window of zero seconds, no job named the endpoint",
        startover: Some(0),
        unfinished: &[],
        finished: &[],
    },
    Row {
        label: "a window of zero seconds, one job that had already finished",
        startover: Some(0),
        unfinished: &[],
        finished: &["done-1"],
    },
    Row {
        label: "the member left out, finished and unfinished jobs",
        startover: None,
        unfinished: &["harvest-1", "harvest-4"],
        finished: &["done-1", "done_2"],
    },
    Row {
        label: "shortened to the smallest window that still retains content",
        startover: Some(MIN_ENABLED_STARTOVER_WINDOW_SECONDS),
        unfinished: &["harvest-1"],
        finished: &[],
    },
    Row {
        label: "widened to a fortnight",
        startover: Some(1_209_600),
        unfinished: &["harvest-1", "harvest_2"],
        finished: &["done-1"],
    },
    Row {
        label: "left at the day-long window it was created with",
        startover: Some(86_400),
        unfinished: &["harvest-1"],
        finished: &[],
    },
];

#[tokio::test]
async fn an_update_that_closes_the_window_fails_the_harvests_still_running_against_it() {
    for row in ROWS {
        let case = row.label;
        let retains = retains_content(row.startover);
        let (router, engine) = fixture();
        create_channel_in(&router, REGION, HARVEST_CHANNEL).await;
        create_channel_in(&router, REGION, OTHER_CHANNEL).await;
        create_endpoint_in(&router, REGION, HARVEST_ENDPOINT, HARVEST_CHANNEL).await;
        create_endpoint_in(&router, REGION, OTHER_ENDPOINT, OTHER_CHANNEL).await;

        // The jobs that finish BEFORE the update: submitted, then made due and read, so it
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

        // The jobs still harvesting at the update, and the control job on the endpoint the
        // update does not touch. Each submitted job's owed transition is held open, so what
        // the update finds is a job that has not finished.
        let mut unfinished = Vec::new();
        for id in row.unfinished {
            let created = submit(&router, REGION, id, HARVEST_ENDPOINT).await;
            hold_transitions_open(&engine);
            unfinished.push((*id, created));
        }
        let control = submit(&router, REGION, CONTROL_JOB, OTHER_ENDPOINT).await;
        hold_transitions_open(&engine);

        for (id, _) in &unfinished {
            let (status, body) = call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading job {id:?} expected 200, got {status} {body:?}"
            );
            assert_eq!(
                status_of(&body),
                Some(state::HARVEST_IN_PROGRESS),
                "case {case:?}: job {id:?} must still be {} before the update, got {body:?}",
                state::HARVEST_IN_PROGRESS
            );
        }

        // The update itself: the jobs harvesting the endpoint never refuse it, and the
        // window the write leaves is the one the rule is judged on.
        let expected_window = row.startover.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS);
        let (status, written) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{HARVEST_ENDPOINT}"),
            Some(&update_body(row.startover)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the jobs harvesting the endpoint must not refuse its update, \
             expected 200, got {status} {written:?}"
        );
        assert_eq!(
            written.get("startoverWindowSeconds"),
            Some(&json!(expected_window)),
            "case {case:?}: the written endpoint must report startoverWindowSeconds \
             {expected_window}, got {written:?}"
        );

        // A job that had not finished harvesting content the endpoint no longer retains can
        // never finish it: it carries the modelled failure status the moment the window
        // closes — while the transition the clock owes it is still an hour out — and keeps
        // every other member the submit stored, the ids among them. An update that leaves
        // the endpoint retaining content takes nothing away, so its jobs go on harvesting.
        let expected_status = if retains {
            state::HARVEST_IN_PROGRESS
        } else {
            state::HARVEST_FAILED
        };
        for (id, created) in &unfinished {
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: job {id:?} must still be readable after the update, got \
                 {status} {described:?}"
            );
            assert_eq!(
                status_of(&described),
                Some(expected_status),
                "case {case:?}: job {id:?} must be {expected_status} after the update, got \
                 {described:?}"
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
                "case {case:?}: reading job {id:?} must not be what moves it, got {again:?}"
            );
        }

        // A job that had already finished harvesting the endpoint did so while the endpoint
        // still held its content: the update leaves its record exactly as it stood.
        for (id, before) in &finished {
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: job {id:?} must still be readable, got {status} {described:?}"
            );
            assert_eq!(
                &described, before,
                "case {case:?}: job {id:?} had already finished, so the update must change \
                 nothing about it, got {described:?}"
            );
        }

        // The control job harvests an endpoint this update did not write, so it is still
        // harvesting: the write reaches the jobs of the endpoint written and no others.
        let (status, described) =
            call(&router, "GET", &format!("/harvest_jobs/{CONTROL_JOB}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the control job expected 200, got {status} {described:?}"
        );
        assert_eq!(
            status_of(&described),
            Some(state::HARVEST_IN_PROGRESS),
            "case {case:?}: the job of the untouched endpoint must still be {}, got \
             {described:?}",
            state::HARVEST_IN_PROGRESS
        );
        assert_eq!(
            without_status(&described),
            without_status(&control),
            "case {case:?}: the control job's record must be untouched, got {described:?}"
        );

        // What a LATER submission against the very same endpoint is answered now agrees
        // with what the jobs already harvesting it report: an endpoint retaining nothing
        // refuses the submission (422, keyed by the member naming it) and reports no
        // harvest in progress against it, and one retaining content accepts it.
        let (status, answered) = call(
            &router,
            "POST",
            "/harvest_jobs",
            Some(&job_body("later-job", HARVEST_ENDPOINT)),
        )
        .await;
        if retains {
            assert_eq!(
                status, 200,
                "case {case:?}: the endpoint still retains content, so a later submission \
                 expected 200, got {status} {answered:?}"
            );
        } else {
            assert_eq!(
                status, 422,
                "case {case:?}: the endpoint retains nothing, so a later submission expected \
                 422, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "UnprocessableEntityException",
                "case {case:?}: expected UnprocessableEntityException, got {answered:?}"
            );
            assert!(
                answered.pointer("/errors/originEndpointId").is_some(),
                "case {case:?}: the refusal must be keyed by originEndpointId, got {answered:?}"
            );
        }

        // Only the clock finishes a job the update left harvesting: with its owed harvest
        // brought forward, every job of the endpoint reports the terminal status that
        // harvest earned — these submissions name a bucket no object store holds, so
        // nothing could be written and each one failed.
        make_transitions_due(&engine);
        for (id, _) in &unfinished {
            let (status, described) =
                call(&router, "GET", &format!("/harvest_jobs/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: job {id:?} must still be readable, got {status} {described:?}"
            );
            assert_eq!(
                status_of(&described),
                Some(state::HARVEST_FAILED),
                "case {case:?}: job {id:?} must have reached {} once the clock made its \
                 harvest due, got {described:?}",
                state::HARVEST_FAILED
            );
        }
    }
}

#[tokio::test]
async fn a_closed_window_fails_only_the_harvests_of_its_own_region() {
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
            "PUT",
            &format!("/origin_endpoints/{HARVEST_ENDPOINT}"),
            Some(&update_body(Some(DEFAULT_STARTOVER_WINDOW_SECONDS))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
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
            "case {case:?}: the job harvesting the endpoint whose window closed must be {}, \
             got {described:?}",
            state::HARVEST_FAILED
        );

        // The foreign Region's endpoint is a separate resource, so its window is untouched
        // and its job is still harvesting it.
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
        assert!(
            retains_content(
                body.get("startoverWindowSeconds")
                    .and_then(Value::as_i64)
                    .map(|seconds| seconds as i32)
            ),
            "case {case:?}: the other Region's endpoint must still retain its startover \
             window, got {body:?}"
        );
    }
}
