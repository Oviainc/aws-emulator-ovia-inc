// SPDX-License-Identifier: CC0-1.0

//! The span this service gives a transition it owes the clock, measured through the front
//! door.
//!
//! The RULE under test: a transition this service owes the clock falls due a FIXED,
//! non-zero span after the call that STARTED it, and that span is long enough to outlast
//! the calls the caller makes next — so the transient status is what every caller is
//! answered until the span has passed, however many requests it takes meanwhile, and the
//! settled status only after it. The span is measured from the starting call alone, so one
//! resource's span neither shortens another's nor is shortened by it.
//!
//! The table spans every transition this service chooses the span of, which is what the
//! rule generalises over — a different operation, a different resource and a different
//! status pair in each row:
//!
//! * `ReserveContact`, which admits a contact `SCHEDULING` and owes it `SCHEDULED`;
//! * `UpdateContact`, which appends a version `UPDATING` and owes it `ACTIVE`;
//! * `CancelContact` on a pass already under way, which reports `CANCELLING` and owes the
//!   contact `CANCELLED`;
//! * `CreateEphemeris`, which admits an ephemeris `VALIDATING` and owes it the verdict its
//!   data and request earned.
//!
//! `CreateDataflowEndpointGroup` is NOT in that table: the recorded group lifecycle reads
//! a group back after creating it and is answered `endpoint.status` `created` with
//! `healthStatus` `HEALTHY`, so a dataflow endpoint is handed back ATTACHED and owes the
//! clock no move at all — what a read of it must report, and that no later read differs, is
//! asserted in `crate::dataflow_endpoint_groups` and
//! `crate::contact_endpoint_settle_consistency`.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up, and each row's span is MEASURED from the answer to its
//! own starting call rather than compared against a constant, so the assertions hold the
//! service to what a caller can observe.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    ACTIVE_VERSION_STATUS, CONTACT_CANCELLED, CONTACT_CANCELLING, CONTACT_PASS, CONTACT_SCHEDULED,
    CONTACT_SCHEDULING, EPHEMERIS_ENABLED, EPHEMERIS_VALIDATING, UPDATING_VERSION_STATUS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_cancellation::put_in_status;
use crate::ephemeris_catalog::{create_ok, tle_inline, FUTURE};
use crate::ephemeris_description::describe_ok;
use crate::minute_usage::reserve_pass;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, stock_ground_stations, Reply, STATION,
};

/// The first window this suite reserves: antenna time far enough ahead that no
/// reservation is refused for having already elapsed.
const PASS_START: i64 = 1_900_000_000;

/// How long each reserved pass runs, past the minimum its mission profile requires.
const PASS_SECONDS: i64 = 600;

/// How often the measurement below asks again while a transition is still owed. Short
/// enough that the measured span is the service's and not the poller's.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The longest the measurement below waits for an owed transition: far past any span a
/// caller should have to wait for, so a transition that never falls due fails as a bounded
/// failure instead of hanging the suite.
const POLL_CEILING: Duration = Duration::from_secs(30);

/// The least a span may be and still be observable: a caller that goes on working against
/// the service — creating the rest of a set of resources, reading the first one back —
/// must still be answered the transient status, so a span that is over before the next
/// few requests have been answered is the defect this suite pins.
const AT_LEAST: Duration = Duration::from_secs(1);

/// The most a span may be: this emulator stands in for a service that takes minutes, so
/// the span is compressed, and no caller waits longer than this for a transition.
const AT_MOST: Duration = Duration::from_secs(10);

/// How far two measured spans may differ and still be the ONE span this service gives
/// every transition it owes: the polling granularity plus the requests taken between one
/// row's starting call and the next's.
const SPAN_AGREEMENT: Duration = Duration::from_secs(1);

/// Where a row's status is read from, and through which operation.
enum Reader {
    /// `DescribeContact`'s `contactStatus`, for the contact of this id.
    Contact(String),
    /// `DescribeContact`'s newest `version.status`, for the contact of this id.
    ContactVersion(String),
    /// `DescribeEphemeris`'s `status`, for the ephemeris of this id.
    Ephemeris(String),
}

/// One started transition: what was started, where its status is read, and the two
/// statuses it must report on either side of the span.
struct Started {
    case: &'static str,
    reader: Reader,
    transient: &'static str,
    settled: &'static str,
    /// When the starting call was ANSWERED. Taken after the answer, so a measurement from
    /// it can only understate the span the service actually chose.
    at: Instant,
}

/// `DescribeContact` for `contact_id`, asserting it was answered.
async fn describe_contact(router: &MultiServiceRouter, contact_id: &str, case: &str) -> Reply {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeContact expected 200, got {} {}",
        described.status, described.body
    );
    described
}

/// The status `row` reports right now, read through its own operation.
async fn observed(router: &MultiServiceRouter, row: &Started) -> String {
    match &row.reader {
        Reader::Contact(contact_id) => describe_contact(router, contact_id, row.case)
            .await
            .string("contactStatus"),
        Reader::ContactVersion(contact_id) => {
            let described = describe_contact(router, contact_id, row.case).await;
            let version = described.field("version").clone();
            version
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        }
        Reader::Ephemeris(ephemeris_id) => {
            let body = describe_ok(router, row.case, ephemeris_id).await;
            body.get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        }
    }
}

/// An ephemeris request whose data validates, so the verdict the service owes it is the
/// `enabled` the request asked for.
fn ephemeris_body(name: &str) -> Value {
    json!({
        "name": name,
        "enabled": true,
        "expirationTime": FUTURE,
        "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
    })
}

#[tokio::test]
async fn every_transition_this_service_owes_outlasts_the_calls_the_caller_makes_next() {
    let (router, engine) = harness_with_engine();
    // The account is served by the station the reserved pass below is held at.
    stock_ground_stations(&engine, STATION);
    let profile_arn =
        create_mission_profile(&router, "owed transitions", 60, Some(120), Some(180)).await;

    // The two contacts whose transitions are started by a later call, each holding a
    // window of its own: one antenna serves one pass at a time.
    let to_update =
        reserve_pass(&router, &profile_arn, "contact version", PASS_START, PASS_SECONDS).await;
    let to_stop = reserve_pass(
        &router,
        &profile_arn,
        "stopped pass",
        PASS_START + 86_400,
        PASS_SECONDS,
    )
    .await;
    // A cancel of a pass already UNDER WAY is a stop, and the stop is what the service then
    // owes the contact. No request drives a pass, so the contact is put under way in the
    // store — as `contact_cancellation.rs` does for the same reason.
    put_in_status(&engine, &to_stop, CONTACT_PASS);

    // Every transition this service chooses the span of, started back to back. The first
    // one's span is measured from its own call, so it must outlast every start below it and
    // the reads that follow them.
    let mut started: Vec<Started> = Vec::new();

    let reserved = reserve_pass(
        &router,
        &profile_arn,
        "reservation",
        PASS_START + 2 * 86_400,
        PASS_SECONDS,
    )
    .await;
    started.push(Started {
        case: "a reservation the scheduler owes SCHEDULED",
        reader: Reader::Contact(reserved),
        transient: CONTACT_SCHEDULING,
        settled: CONTACT_SCHEDULED,
        at: Instant::now(),
    });

    let appended = send(
        &router,
        "POST",
        &format!("/contact/{to_update}/versions"),
        Some(json!({"trackingOverrides": {}})),
    )
    .await;
    assert_eq!(
        appended.status, 200,
        "UpdateContact expected 200, got {} {}",
        appended.status, appended.body
    );
    started.push(Started {
        case: "a contact version the service owes ACTIVE",
        reader: Reader::ContactVersion(to_update.clone()),
        transient: UPDATING_VERSION_STATUS,
        settled: ACTIVE_VERSION_STATUS,
        at: Instant::now(),
    });

    let stopped = send(&router, "DELETE", &format!("/contact/{to_stop}"), None).await;
    assert_eq!(
        stopped.status, 200,
        "CancelContact expected 200, got {} {}",
        stopped.status, stopped.body
    );
    started.push(Started {
        case: "a stopped pass the service owes CANCELLED",
        reader: Reader::Contact(to_stop.clone()),
        transient: CONTACT_CANCELLING,
        settled: CONTACT_CANCELLED,
        at: Instant::now(),
    });

    let ephemeris_id = create_ok(&router, "ephemeris", ephemeris_body("owed validation")).await;
    started.push(Started {
        case: "an ephemeris the service owes its validation verdict",
        reader: Reader::Ephemeris(ephemeris_id),
        transient: EPHEMERIS_VALIDATING,
        settled: EPHEMERIS_ENABLED,
        at: Instant::now(),
    });

    // Every row is still transient now that every start has been answered — and still
    // transient after a second pass of reads, so the span outlasts the calls a caller goes
    // on making rather than racing them.
    for pass in ["once every transition was started", "after a pass of reads"] {
        for row in &started {
            let now = observed(&router, row).await;
            assert_eq!(
                now, row.transient,
                "case {} ({pass}): a transition is owed for a span that outlasts the \
                 caller's next calls, so the status {} after {:?} must still be {}, got \
                 {now:?}",
                row.case,
                pass,
                row.at.elapsed(),
                row.transient
            );
        }
    }

    // Each row's span, measured from the answer to its own starting call: the status is
    // polled until the move the service owed has been materialised.
    let mut measured: Vec<(&'static str, Duration)> = Vec::new();
    for row in &started {
        loop {
            let now = observed(&router, row).await;
            if now != row.transient {
                assert_eq!(
                    now, row.settled,
                    "case {}: a transition settles on the status it was owed, got {now:?}",
                    row.case
                );
                break;
            }
            assert!(
                row.at.elapsed() < POLL_CEILING,
                "case {}: the transition was still {} {:?} after the call that started it",
                row.case,
                row.transient,
                row.at.elapsed()
            );
            std::thread::sleep(POLL_INTERVAL);
        }
        measured.push((row.case, row.at.elapsed()));
    }

    for (case, span) in &measured {
        assert!(
            *span >= AT_LEAST,
            "case {case}: an owed transition must stay transient at least {AT_LEAST:?}, \
             measured {span:?}"
        );
        assert!(
            *span <= AT_MOST,
            "case {case}: an owed transition must fall due within {AT_MOST:?}, measured \
             {span:?}"
        );
    }

    // And it is ONE span: a caller waits the same for every transition this service owes,
    // whichever resource it is owed to.
    for (case, span) in &measured {
        for (other_case, other) in &measured {
            let apart = span.abs_diff(*other);
            assert!(
                apart <= SPAN_AGREEMENT,
                "case {case} against {other_case}: every transition this service owes falls \
                 due after the same span, so {span:?} and {other:?} must agree within \
                 {SPAN_AGREEMENT:?}, measured {apart:?} apart"
            );
        }
    }
}

#[tokio::test]
async fn a_transition_is_owed_from_its_own_call_so_a_resource_created_later_is_still_transient() {
    let (router, _engine) = harness_with_engine();

    // Two resources of the same kind, so the rule is asserted on the one dimension it is
    // about: the first is created now and the second once the first has settled.
    let first_ephemeris = create_ok(&router, "the first ephemeris", ephemeris_body("first")).await;

    let first_rows = vec![Started {
        case: "the first ephemeris",
        reader: Reader::Ephemeris(first_ephemeris),
        transient: EPHEMERIS_VALIDATING,
        settled: EPHEMERIS_ENABLED,
        at: Instant::now(),
    }];

    // Wait each one out through the front door rather than by a fixed sleep, so this suite
    // asserts the per-call rule and not a span it hard-codes.
    for row in &first_rows {
        loop {
            let now = observed(&router, row).await;
            if now != row.transient {
                assert_eq!(
                    now, row.settled,
                    "case {}: a transition settles on the status it was owed, got {now:?}",
                    row.case
                );
                break;
            }
            assert!(
                row.at.elapsed() < POLL_CEILING,
                "case {}: the transition was still {} {:?} after the call that started it",
                row.case,
                row.transient,
                row.at.elapsed()
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    // A resource created NOW is owed its own span from its own call, so it is transient
    // even though a resource of the same kind created earlier has already settled.
    let second_ephemeris =
        create_ok(&router, "the second ephemeris", ephemeris_body("second")).await;

    let second_rows = vec![Started {
        case: "the second ephemeris",
        reader: Reader::Ephemeris(second_ephemeris),
        transient: EPHEMERIS_VALIDATING,
        settled: EPHEMERIS_ENABLED,
        at: Instant::now(),
    }];

    for row in &second_rows {
        let now = observed(&router, row).await;
        assert_eq!(
            now, row.transient,
            "case {}: a transition is owed from the call that started it, so a resource \
             created after an earlier one settled is still {}, got {now:?}",
            row.case, row.transient
        );
    }

    // And the earlier resources did not move again when the later ones were created: the
    // span belongs to the call that started it, not to the store.
    for row in &first_rows {
        let now = observed(&router, row).await;
        assert_eq!(
            now, row.settled,
            "case {}: a settled resource must keep the status it settled on when another \
             resource is created, got {now:?}",
            row.case
        );
    }
}
