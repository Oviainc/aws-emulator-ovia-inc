// SPDX-License-Identifier: CC0-1.0

//! The instant `PutAppInstanceRetentionSettings` / `GetAppInstanceRetentionSettings`
//! report data deletion starts from, end to end over HTTP.
//!
//! The suite beside this one (`app_instance_retention_settings`) pins WHAT settings the
//! two operations carry. This one pins the clock and the refusals:
//!
//! * `InitiateDeletionTimestamp` is the instant the settings in effect took effect, read
//!   from the clock AT the put that put them in effect — so every served put re-stamps it
//!   and no put can report an instant stamped by an earlier call;
//! * a get only REPORTS that instant, so reading never re-stamps it, however long after
//!   the put the read happens;
//! * a put whose body carries no `AppInstanceRetentionSettings` is refused — the model
//!   makes that member `required` — and a refused put leaves the settings in effect, and
//!   their deletion instant, exactly as they were.
//!
//! Each case is a row of a table spanning what the rule generalises over: the settings
//! value put in effect, how many puts in a row an instance has had, how long after a put
//! the read happens, and the form a body missing the required member takes.

use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::app_instance_lifecycle::Emulator;
use crate::app_instance_retention_settings::{
    get_retention, put_retention, put_retention_body, reported_instant, reported_settings,
    settings, MAX_RETENTION_DAYS, MIN_RETENTION_DAYS,
};

/// How far apart consecutive calls are held, so the wall-clock window of one call does
/// not overlap the window of the call before it. The stored instant has millisecond
/// resolution, so a gap of a few tens of milliseconds separates two windows
/// unambiguously — several times the slack below, and no longer.
///
/// Waited out with a blocking [`sleep`] rather than an async one: `tokio` is depended on
/// here for its `macros` and runtime features only, so `tokio::time` is not available,
/// and nothing else is in flight while a case waits.
const CALL_GAP: Duration = Duration::from_millis(25);

/// Slack allowed when comparing a reported instant against a window measured in this
/// test: the stored instant is truncated to whole milliseconds on the wire.
const SLACK_SECONDS: f64 = 0.005;

/// Now, as the epoch seconds the model's `Timestamp` is rendered as on this protocol.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock at or after the unix epoch")
        .as_secs_f64()
}


/// RULE: the deletion instant a put reports is read from the clock at that put, so it
/// falls inside the wall-clock window of the call that reported it — never inside an
/// earlier call's window, and never in the future.
#[tokio::test]
async fn every_served_put_stamps_the_deletion_instant_from_the_clock_at_that_put() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("retention-stamp", "tok-ret-stamp")
        .await;

    // Rows: the published bounds, a value between them, the same value twice in a row
    // (a repeat still puts settings in effect, so it still re-stamps), and settings
    // naming no number of days, which puts no retention period in effect at all.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "the lower published bound",
            settings(MIN_RETENTION_DAYS),
            true,
        ),
        ("a value between the bounds", settings(180), true),
        ("that same value again", settings(180), true),
        (
            "the upper published bound",
            settings(MAX_RETENTION_DAYS),
            true,
        ),
        (
            "settings naming no number of days",
            json!({ "ChannelRetentionSettings": {} }),
            false,
        ),
        ("a number of days again", settings(14), true),
    ];

    let mut previous: Option<f64> = None;
    for (label, written, retention_in_effect) in rows {
        sleep(CALL_GAP);
        let before = now_epoch_seconds();
        let (status, body) = put_retention(&emulator, &instance, written.clone()).await;
        let after = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {label:?}: expected the put to answer 200, got {status} {body:?}"
        );

        let instant = reported_instant(&body);
        assert_eq!(
            instant.is_some(),
            retention_in_effect,
            "case {label:?}: expected a deletion instant to be reported = \
             {retention_in_effect}, got {instant:?}"
        );

        let Some(instant) = instant else {
            // Settings that put no retention period in effect leave nothing due for
            // deletion, so the previous put's instant must not survive as this one's.
            previous = None;
            continue;
        };
        assert!(
            instant >= before - SLACK_SECONDS && instant <= after + SLACK_SECONDS,
            "case {label:?}: expected the deletion instant to be stamped during this \
             put, inside [{before}, {after}], got {instant:?}"
        );
        if let Some(previous) = previous {
            assert!(
                instant > previous,
                "case {label:?}: expected this put to re-stamp the deletion instant past \
                 the previous {previous}, got {instant:?}"
            );
        }
        previous = Some(instant);
    }
}

/// RULE: a get REPORTS the instant the settings in effect were stamped with and changes
/// nothing, so however long after the put it runs, and however often, it answers the
/// put's instant rather than one stamped by the read.
#[tokio::test]
async fn a_get_reports_the_put_instant_and_never_re_stamps_it() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("retention-report", "tok-ret-report")
        .await;

    // Rows: a few settings values spanning the value space, each read back twice at a
    // measurable distance from the put that wrote it.
    let rows: Vec<(&str, Value)> = vec![
        ("the lower published bound", settings(MIN_RETENTION_DAYS)),
        ("a value between the bounds", settings(365)),
        ("the upper published bound", settings(MAX_RETENTION_DAYS)),
    ];

    for (label, written) in rows {
        let (status, put_body) = put_retention(&emulator, &instance, written.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the put to answer 200, got {status} {put_body:?}"
        );
        let put_instant = reported_instant(&put_body).unwrap_or_else(|| {
            panic!("case {label:?}: expected the put to report a deletion instant: {put_body:?}")
        });

        for read in 1..=2 {
            sleep(CALL_GAP);
            let read_started = now_epoch_seconds();
            let (status, get_body) = get_retention(&emulator, &instance).await;
            assert_eq!(
                (status, reported_settings(&get_body)),
                (200, written.clone()),
                "case {label:?} / read {read}: expected the get to report {written:?}, \
                 got {status} {get_body:?}"
            );
            // The suite beside this one already pins that the two instants are EQUAL;
            // the overlap is deliberate, because what is new here is the assertion
            // below — that the reported instant predates the read, so the read cannot
            // have stamped it.
            assert_eq!(
                reported_instant(&get_body),
                Some(put_instant),
                "case {label:?} / read {read}: expected the get to report the put's \
                 instant {put_instant}, got {get_body:?}"
            );
            assert!(
                put_instant < read_started - SLACK_SECONDS,
                "case {label:?} / read {read}: expected the reported instant \
                 {put_instant} to predate this read, which started at {read_started}"
            );
        }
    }
}

/// RULE: `AppInstanceRetentionSettings` is a required member of the put's body, so a body
/// that carries none is refused, and the refused put leaves the settings in effect — and
/// the instant they were stamped with — untouched.
#[tokio::test]
async fn a_put_carrying_no_retention_settings_member_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("retention-required", "tok-ret-required")
        .await;

    // What nothing below may change.
    let surviving = settings(120);
    let (status, body) = put_retention(&emulator, &instance, surviving.clone()).await;
    assert_eq!(status, 200, "the surviving put answered: {status} {body:?}");
    let surviving_instant = reported_instant(&body).unwrap_or_else(|| {
        panic!("expected the surviving put to report a deletion instant: {body:?}")
    });

    // Rows: an empty body, a body carrying only members this operation does not model,
    // and the required member misspelled.
    let rows: Vec<(&str, Value)> = vec![
        ("an empty body", json!({})),
        (
            "only members this operation does not model",
            json!({ "ChannelRetentionSettings": { "RetentionDays": 7 } }),
        ),
        (
            "the required member misspelled",
            json!({ "AppInstanceRetentionSetting": { "ChannelRetentionSettings": { "RetentionDays": 7 } } }),
        ),
    ];

    for (label, body) in rows {
        sleep(CALL_GAP);
        let (status, response) = put_retention_body(&emulator, &instance, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected {body:?} to be refused with 400, got {status} \
             {response:?}"
        );

        let (status, after) = get_retention(&emulator, &instance).await;
        assert_eq!(
            (status, reported_settings(&after)),
            (200, surviving.clone()),
            "case {label:?}: a refused put must leave {surviving:?} in place, got \
             {status} {after:?}"
        );
        assert_eq!(
            reported_instant(&after),
            Some(surviving_instant),
            "case {label:?}: a refused put must leave the deletion instant \
             {surviving_instant} in place, got {after:?}"
        );
    }
}
