// SPDX-License-Identifier: CC0-1.0

//! What DATES and what NUMBERS a multiplex alert: its `SetTimestamp` and its
//! `Id`, as `ListMultiplexAlerts` reports them — end to end over the restJson1
//! wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The rule both suites assert is that an alert is a property of the STATE and
//! not of the polling schedule: it is dated by the state change that made its
//! condition true, and numbered in the order the conditions became true, so two
//! callers polling at any instants read the same alerts with the same ids and
//! the same timestamps. The suites are TABLES spanning what that generalises
//! over — which state change made the condition true, how late the first call
//! that could observe it arrives, and whether one call or two observed two
//! multiplexes' conditions.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_medialive_medialive::state::{
    multiplex_alert_state, TRANSIENT_STATE_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_alerts::{
    alert_id, assert_set_alert, every_alert, feeding_channel, program, run_multiplex,
    start_channel, stop_channel,
};
use crate::multiplex_lifecycle::{describe_multiplex, settings};
use crate::multiplex_start::{multiplex_with_settings, start_multiplex};
use crate::support::{epoch_seconds, timestamp, unix_seconds};
use crate::wire::medialive;

/// How far apart two instants this suite compares may be and still count as the
/// same one: the clock is read on both sides of a request, and the wire form is
/// an ISO-8601 date-time, so neither reading is exact to the nanosecond.
const CLOCK_SLACK: f64 = 0.2;

/// The program every fixture here alerts about. One name, because what the rows
/// differ along is the state change that makes the condition about it true, not
/// which program it is about.
const PROGRAM: &str = "alpha";

/// Which state change makes the one condition this service observes on a
/// multiplex become true — the dimension the dating rule generalises over.
#[derive(Debug, Clone, Copy)]
enum Trigger {
    /// The multiplex BEGAN TRANSMITTING while holding a program nothing feeds,
    /// which the clock owes at the instant the start it was asked for came due.
    BeganTransmitting,
    /// A program nothing feeds was ADDED to a multiplex that was already
    /// transmitting, which happens at the creating call's own instant.
    ProgramAdded,
    /// The running channel feeding a program of a transmitting multiplex was
    /// STOPPED, so from that stop on nothing healthy feeds it.
    FeederStopped,
}

/// The window the triggering state change happened in: the clock either side of
/// it, including the span the clock takes to make the transition it starts due.
#[derive(Debug, Clone, Copy)]
struct Window {
    from: SystemTime,
    until: SystemTime,
}

/// A multiplex that is transmitting with one program, plus whatever `trigger`
/// needs beside it — and then the triggering state change itself, bracketed by
/// the clock either side of it.
async fn triggered(router: &MultiServiceRouter, case: &str, trigger: Trigger) -> (String, Window) {
    let multiplex_id = multiplex_with_settings(router, case, "dated", settings(2_000_000, 1)).await;
    match trigger {
        Trigger::BeganTransmitting => {
            program(router, case, &multiplex_id, PROGRAM, 1).await;
            wait_for_the_clock();
            let from = SystemTime::now();
            run_multiplex(router, case, &multiplex_id).await;
            let until = SystemTime::now();
            (multiplex_id, Window { from, until })
        }
        Trigger::ProgramAdded => {
            wait_for_the_clock();
            run_multiplex(router, case, &multiplex_id).await;
            let from = SystemTime::now();
            program(router, case, &multiplex_id, PROGRAM, 1).await;
            let until = SystemTime::now();
            (multiplex_id, Window { from, until })
        }
        Trigger::FeederStopped => {
            program(router, case, &multiplex_id, PROGRAM, 1).await;
            let channel_id = feeding_channel(router, case, &multiplex_id, PROGRAM).await;
            wait_for_the_clock();
            start_channel(router, case, &channel_id).await;
            wait_for_the_clock();
            run_multiplex(router, case, &multiplex_id).await;
            // A running channel feeds it, so nothing is wrong yet: the state
            // change the alert is dated by is the stop below and not the run.
            let (fed, _) = every_alert(router, case, &multiplex_id, None, None).await;
            assert!(
                fed.is_empty(),
                "case {case:?}: expected no alert while a running channel feeds the program, \
                 got {fed:?}"
            );
            let from = SystemTime::now();
            stop_channel(router, case, &channel_id).await;
            wait_for_the_clock();
            let until = SystemTime::now();
            (multiplex_id, Window { from, until })
        }
    }
}

/// Rule: an alert carries the instant the condition behind it BECAME TRUE —
/// which is the instant of the state change that made it so, whether that was a
/// call's own mutation or a transition the clock made due — so however late the
/// first call that can observe it arrives, the `SetTimestamp` it reads is the
/// same one.
///
/// The rows differ along the two things the rule generalises over: which state
/// change made the condition true (the multiplex beginning to transmit, a
/// program being added to one that already was, the channel feeding a program
/// being stopped), and how long after that change the first call that can
/// observe it arrives — at once, a span later, and several spans later.
#[tokio::test]
async fn an_alert_is_dated_by_the_state_change_that_made_its_condition_true() {
    for trigger in [
        Trigger::BeganTransmitting,
        Trigger::ProgramAdded,
        Trigger::FeederStopped,
    ] {
        for lateness in [
            Duration::ZERO,
            TRANSIENT_STATE_SPAN,
            4 * TRANSIENT_STATE_SPAN,
        ] {
            let case = format!("{trigger:?} observed {lateness:?} late");
            let case = case.as_str();
            let router = medialive();
            let (multiplex_id, window) = triggered(&router, case, trigger).await;

            // Nothing calls in between, so this listing is the FIRST call that
            // can observe the condition: the one that mints the alert's id and
            // writes the instant a later caller reads.
            std::thread::sleep(lateness);
            let polled_at = SystemTime::now();
            let (alerts, _) = every_alert(&router, case, &multiplex_id, None, None).await;
            assert_eq!(
                alerts.len(),
                1,
                "case {case:?}: expected the one condition that holds to have raised one alert, \
                 got {alerts:?}"
            );
            assert_set_alert(case, &alerts[0], PROGRAM);

            let set_at = epoch_seconds(timestamp(case, "setTimestamp", &alerts[0]));
            let (from, until) = (unix_seconds(window.from), unix_seconds(window.until));
            assert!(
                set_at >= from - CLOCK_SLACK && set_at <= until + CLOCK_SLACK,
                "case {case:?}: expected the set instant inside the window the state change \
                 happened in ({from}..{until}), got {set_at}"
            );
            // The same reading from the other side: the stamp is as old as the
            // state change, so the lateness of this call is not in it.
            let polled = unix_seconds(polled_at);
            assert!(
                polled - set_at >= lateness.as_secs_f64() - CLOCK_SLACK,
                "case {case:?}: expected the set instant to predate the observing call at \
                 {polled} by at least the {lateness:?} it arrived late, got {set_at}"
            );

            // And a second listing reads exactly what the first wrote.
            let (again, _) = every_alert(&router, case, &multiplex_id, None, None).await;
            assert_eq!(
                again, alerts,
                "case {case:?}: expected repeating the listing to answer the same alert, got \
                 {again:?}"
            );
        }
    }
}

/// Whether one call or two observed the conditions of two multiplexes — the
/// dimension the numbering rule generalises over.
#[derive(Debug, Clone, Copy)]
enum Observed {
    /// Nobody calls between the two multiplexes beginning to transmit, so ONE
    /// later call observes both conditions at once.
    ByOneCall,
    /// A reporting call lands between them, so it observes the first condition
    /// and a later call observes the second.
    ByACallBetween,
}

/// Two multiplexes, each holding one program nothing feeds, both transmitting —
/// the one created SECOND (so carrying the higher minted multiplex id) having
/// begun to transmit FIRST, so the order the conditions became true is the
/// opposite of the order the multiplexes are stored in.
///
/// Answers (the alert on the multiplex that began transmitting first, the alert
/// on the one that began second).
async fn two_transmitting_multiplexes(
    router: &MultiServiceRouter,
    case: &str,
    observed: Observed,
) -> (Value, Value) {
    let later = multiplex_with_settings(router, case, "numbered-later", settings(2_000_000, 1)).await;
    let earlier =
        multiplex_with_settings(router, case, "numbered-earlier", settings(2_000_000, 2)).await;
    assert!(
        earlier > later,
        "case {case:?}: expected the multiplex created second to carry the higher id, got \
         {earlier:?} after {later:?}"
    );
    for multiplex_id in [&later, &earlier] {
        program(router, case, multiplex_id, PROGRAM, 1).await;
    }
    wait_for_the_clock();

    let (status, starting) = start_multiplex(router, &earlier).await;
    assert_eq!(
        status, 202,
        "case {case:?}: StartMultiplex({earlier}): expected 202, got {status}: {starting}"
    );
    if let Observed::ByACallBetween = observed {
        // The first multiplex is transmitting and the second has not been
        // started yet, so this reporting call observes exactly one condition.
        wait_for_the_clock();
        let (status, described) = describe_multiplex(router, &earlier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex({earlier}): expected 200, got {status}: {described}"
        );
    }
    let (status, starting) = start_multiplex(router, &later).await;
    assert_eq!(
        status, 202,
        "case {case:?}: StartMultiplex({later}): expected 202, got {status}: {starting}"
    );
    wait_for_the_clock();

    let mut alerts = Vec::new();
    for multiplex_id in [&earlier, &later] {
        let (reported, _) = every_alert(router, case, multiplex_id, None, None).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected one alert on the transmitting multiplex {multiplex_id}, got \
             {reported:?}"
        );
        assert_set_alert(case, &reported[0], PROGRAM);
        assert_eq!(
            reported[0]["state"],
            Value::String(multiplex_alert_state::SET.to_string()),
            "case {case:?}: expected the alert on {multiplex_id} to be happening, got {:?}",
            reported[0]
        );
        alerts.push(reported[0].clone());
    }
    let second = alerts.pop().unwrap_or(Value::Null);
    let first = alerts.pop().unwrap_or(Value::Null);
    (first, second)
}

/// Rule: an alert's `Id` is minted in the order the conditions became true, so
/// the id an alert carries — and therefore the `NextToken` a page of alerts
/// resumes from — depends on the state alone and not on which call happened to
/// observe a condition first.
///
/// The rows are the two ways the same two conditions can be observed: by one
/// call that sees both, and by two calls with one condition each. Each row runs
/// on a store of its own, so the ids both arrangements hand out start from the
/// same place and can be compared with one another.
#[tokio::test]
async fn alert_ids_are_numbered_by_when_their_conditions_became_true() {
    let mut numbering: Vec<(String, String)> = Vec::new();
    for observed in [Observed::ByOneCall, Observed::ByACallBetween] {
        let case = format!("{observed:?}");
        let case = case.as_str();
        let router = medialive();
        let (first, second) = two_transmitting_multiplexes(&router, case, observed).await;

        let (first_id, second_id) = (alert_id(case, &first), alert_id(case, &second));
        let parsed = |id: &str| id.parse::<u64>().unwrap_or_default();
        assert!(
            parsed(&first_id) < parsed(&second_id),
            "case {case:?}: expected the alert whose condition became true first to carry the \
             lower id, got {first_id} then {second_id}"
        );
        let first_set = epoch_seconds(timestamp(case, "setTimestamp", &first));
        let second_set = epoch_seconds(timestamp(case, "setTimestamp", &second));
        assert!(
            first_set < second_set,
            "case {case:?}: expected the alert raised for the earlier condition to carry the \
             earlier set instant, got {first_set} then {second_set}"
        );
        numbering.push((first_id, second_id));
    }

    let (by_one_call, by_a_call_between) = (&numbering[0], &numbering[1]);
    assert_eq!(
        by_one_call, by_a_call_between,
        "expected the same ids whether one call observed both conditions or a call landed \
         between them, got {by_one_call:?} and {by_a_call_between:?}"
    );
}
