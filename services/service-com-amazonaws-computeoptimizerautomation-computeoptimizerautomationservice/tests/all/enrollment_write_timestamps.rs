// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `lastUpdatedTimestamp` an enrollment write leaves on
//! an account's configuration.
//!
//! The rule under test: a write to an account's enrollment configuration stamps
//! `lastUpdatedTimestamp` with the instant the store applied the change — the
//! member means "The timestamp when the enrollment configuration was last
//! updated"
//! (`smithy_query(query="doc",shape="UpdateEnrollmentConfigurationResponse",member="lastUpdatedTimestamp")`)
//! and "The timestamp when the account's Automation enrollment status was last
//! updated"
//! (`smithy_query(query="doc",shape="AccountInfo",member="lastUpdatedTimestamp")`)
//! — while a write that leaves the stored configuration exactly as it stood
//! moves it nowhere. Recording a configuration for an account the store holds
//! none for is itself a change, so a first call stamps the instant it stored one.
//!
//! The rows differ along what the rule generalises over: which status is
//! requested, whether that status is the one the account already reports, how the
//! request spells its idempotency (no `clientToken`, a fresh one, a replayed
//! one), and whether the stored configuration carries a `statusReason` the write
//! drops. A request's effect on the store may not depend on whether it carries an
//! optional token, so the same repeat is driven all three ways.
//!
//! Every case drives the service over the wire through the same router the
//! emulator binary mounts (the harness lives in
//! [`super::automation_rule_lifecycle`]) and reads the configuration back through
//! `GetEnrollmentConfiguration` and, while the account is opted in,
//! `ListAccounts` — so a timestamp that is not the one the store holds fails
//! here.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use super::account_association::{seed_enrollment, stored_enrollment};
use super::automation_rule_lifecycle::{
    call_ok, member, string_member, timestamp, unenrolled_emulator_with_engine,
};

/// A pause between two writes, so "the timestamp moved" is a statement about the
/// write rather than about the resolution of the clock the two calls read.
const CLOCK_MARGIN: Duration = Duration::from_millis(25);

/// How close two epoch-seconds readings have to be to be the SAME instant.
///
/// A stored instant is read both off the wire and out of the store, and the two
/// renderings of one microsecond-resolution instant differ in the last bits of a
/// `f64` that large. Anything a second write stamped is a whole [`CLOCK_MARGIN`]
/// away, so a millisecond separates "the same instant" from "re-stamped" with
/// room to spare.
const SAME_INSTANT: f64 = 1e-3;

/// Are these two epoch-seconds readings the same stored instant?
fn same_instant(left: f64, right: f64) -> bool {
    (left - right).abs() < SAME_INSTANT
}

/// One row of the walk: one `UpdateEnrollmentConfiguration` call, and whether it
/// changes the stored configuration.
struct Case {
    name: &'static str,
    /// The `status` the call asks for; one of the two settable
    /// `EnrollmentStatus` values.
    status: &'static str,
    /// The `clientToken` the call carries, if any. A token no earlier call used
    /// is classified fresh, so the handler's replay arm is not involved.
    client_token: Option<&'static str>,
    /// Whether this call changes what the store holds for the account, which is
    /// what decides whether the configuration was updated at all.
    changes_the_configuration: bool,
}

/// The walk over one account's configuration, from the state a brand-new account
/// is in (no stored entry at all) through both settable statuses, repeating each
/// one with no token and with a fresh token.
fn cases() -> Vec<Case> {
    vec![
        // The store holds no entry for this account yet, so recording one is a
        // change even though the status it records is the one an account with no
        // entry already reports.
        Case {
            name: "first-call-stores-the-default-status",
            status: "Inactive",
            client_token: None,
            changes_the_configuration: true,
        },
        Case {
            name: "repeat-inactive-without-a-token",
            status: "Inactive",
            client_token: None,
            changes_the_configuration: false,
        },
        Case {
            name: "repeat-inactive-under-a-fresh-token",
            status: "Inactive",
            client_token: Some("enroll-stamp-inactive-fresh"),
            changes_the_configuration: false,
        },
        Case {
            name: "opt-in",
            status: "Active",
            client_token: None,
            changes_the_configuration: true,
        },
        Case {
            name: "repeat-active-without-a-token",
            status: "Active",
            client_token: None,
            changes_the_configuration: false,
        },
        Case {
            name: "repeat-active-under-a-fresh-token",
            status: "Active",
            client_token: Some("enroll-stamp-active-fresh"),
            changes_the_configuration: false,
        },
        Case {
            name: "opt-out",
            status: "Inactive",
            client_token: Some("enroll-stamp-opt-out"),
            changes_the_configuration: true,
        },
        Case {
            name: "repeat-inactive-after-the-opt-out",
            status: "Inactive",
            client_token: None,
            changes_the_configuration: false,
        },
        Case {
            name: "opt-in-again",
            status: "Active",
            client_token: Some("enroll-stamp-opt-in-again"),
            changes_the_configuration: true,
        },
    ]
}

/// The request one row sends.
fn request(case: &Case) -> Value {
    let mut body = json!({"status": case.status});
    if let Some(token) = case.client_token {
        body["clientToken"] = json!(token);
    }
    body
}

/// The calling account's configuration as `GetEnrollmentConfiguration` reports
/// it.
async fn read_configuration(router: &MultiServiceRouter, case: &str) -> Value {
    call_ok(router, "GetEnrollmentConfiguration", &json!({}), case).await
}

/// The calling account's entry as `ListAccounts` reports it. Only reachable while
/// the account is opted in: every operation but the two enrollment-configuration
/// ones refuses an account that is not.
async fn listed_account(router: &MultiServiceRouter, case: &str) -> Value {
    let listed = call_ok(router, "ListAccounts", &json!({}), case).await;
    let caller = state::caller_account_id();
    let found = member(&listed, "accounts")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|account| string_member(account, "accountId") == caller);
    assert!(
        found.is_some(),
        "case {case:?}: the calling account {caller} must be listed, got {listed}"
    );
    found.unwrap_or(Value::Null)
}

/// An enrollment write that changes the stored configuration stamps its
/// `lastUpdatedTimestamp`; one that leaves the configuration as it stood leaves
/// the timestamp alone, whether or not the request carries a `clientToken`. Both
/// readers of the stored configuration report the same instant.
#[tokio::test]
async fn an_enrollment_write_stamps_last_updated_timestamp_when_it_changes_the_configuration() {
    // The unenrolled harness: the first row is about the account the store holds
    // no entry for, so nothing may have been stored for it first.
    let (router, engine) = unenrolled_emulator_with_engine();
    let caller = state::caller_account_id();

    let initial = read_configuration(&router, "before-any-write").await;
    assert!(
        initial.get("lastUpdatedTimestamp").is_none(),
        "case \"before-any-write\": nothing was ever stored, so there is no update time, got \
         {initial}"
    );

    let mut previous: Option<f64> = None;
    for case in cases() {
        let name = case.name;
        sleep(CLOCK_MARGIN).await;

        let answer = call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &request(&case),
            name,
        )
        .await;
        assert_eq!(
            string_member(&answer, "status"),
            case.status,
            "case {name:?}: the update must report the status it stored, got {answer}"
        );
        assert!(
            answer.get("statusReason").is_none(),
            "case {name:?}: a status the caller set successfully has no reason, got {answer}"
        );

        let stamped = timestamp(&answer, "lastUpdatedTimestamp", name);
        match (previous, case.changes_the_configuration) {
            (Some(before), true) => assert!(
                stamped > before,
                "case {name:?}: a write that changes the configuration stamps the instant it \
                 was applied, expected a lastUpdatedTimestamp later than {before}, got {stamped}"
            ),
            (Some(before), false) => assert!(
                same_instant(stamped, before),
                "case {name:?}: a write that changes nothing updated nothing, expected \
                 lastUpdatedTimestamp to stay {before}, got {stamped}"
            ),
            (None, _) => {}
        }

        // The reader the caller has for its own configuration reports the same
        // instant, so the response is the store's and not the handler's own.
        let read_back = read_configuration(&router, name).await;
        let read_stamp = timestamp(&read_back, "lastUpdatedTimestamp", name);
        assert!(
            same_instant(read_stamp, stamped),
            "case {name:?}: Get must report the instant the store holds, expected {stamped}, \
             got {read_stamp}"
        );
        assert_eq!(
            string_member(&read_back, "status"),
            case.status,
            "case {name:?}: Get must report the persisted status, got {read_back}"
        );

        // The listing of the organization's accounts is the second reader of the
        // same field, reachable while the account is opted in.
        if case.status == state::ENROLLED_STATUS {
            let listed = listed_account(&router, name).await;
            let listed_stamp = timestamp(&listed, "lastUpdatedTimestamp", name);
            assert!(
                same_instant(listed_stamp, stamped),
                "case {name:?}: the listed account must carry the instant Get reports, expected \
                 {stamped}, got {listed_stamp}"
            );
        }

        // And the store itself, where the handler wrote it.
        let stored = stored_enrollment(&engine, &caller);
        assert!(
            stored
                .last_updated_timestamp
                .is_some_and(|at| same_instant(at.as_secs_f64(), stamped)),
            "case {name:?}: the stored configuration must hold the instant the response \
             reports ({stamped}), got {stored:?}"
        );

        previous = Some(stamped);
    }
}

/// The same repeat of an unchanged status has the same effect on the store
/// however the request spells its idempotency: with no `clientToken`, with a
/// token no call has used, and with the first call's token replayed. An optional
/// member cannot decide what a call persists.
#[tokio::test]
async fn a_repeat_of_the_stored_status_persists_nothing_whatever_token_it_carries() {
    // (case, the `clientToken` the repeat carries) — the three spellings of the
    // same request.
    let rows: [(&str, Option<&str>); 3] = [
        ("no-token", None),
        ("a-fresh-token", Some("enroll-repeat-fresh")),
        ("the-first-calls-token", Some("enroll-repeat-first")),
    ];

    for (case, repeat_token) in rows {
        // One fresh store per row, so each row starts from an account that has
        // never configured an enrollment.
        let (router, engine) = unenrolled_emulator_with_engine();
        let caller = state::caller_account_id();

        let first = call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": "Active", "clientToken": "enroll-repeat-first"}),
            case,
        )
        .await;
        let stamped = timestamp(&first, "lastUpdatedTimestamp", case);

        sleep(CLOCK_MARGIN).await;

        let mut repeat = json!({"status": "Active"});
        if let Some(token) = repeat_token {
            repeat["clientToken"] = json!(token);
        }
        let again = call_ok(&router, "UpdateEnrollmentConfiguration", &repeat, case).await;
        let repeated = timestamp(&again, "lastUpdatedTimestamp", case);
        assert!(
            same_instant(repeated, stamped),
            "case {case:?}: the repeat changed nothing, so it moves no timestamp, expected \
             {stamped}, got {repeated}"
        );

        let stored = stored_enrollment(&engine, &caller);
        assert!(
            stored
                .last_updated_timestamp
                .is_some_and(|at| same_instant(at.as_secs_f64(), stamped)),
            "case {case:?}: the store must still hold the instant the first call stamped \
             ({stamped}), got {stored:?}"
        );
        let read_back = read_configuration(&router, case).await;
        assert!(
            same_instant(timestamp(&read_back, "lastUpdatedTimestamp", case), stamped),
            "case {case:?}: Get must still report the first call's instant, got {read_back}"
        );
    }
}

/// A write that drops a `statusReason` the stored configuration carries DOES
/// change it — the reason belongs to the status it explains — so it stamps the
/// instant it applied that, even when the status itself is the one the account
/// already reports.
#[tokio::test]
async fn an_enrollment_write_that_drops_a_stored_reason_stamps_the_configuration() {
    // (case, the stored status, the stored reason, the status the call asks for,
    // whether the call changes the configuration). The two statuses a caller
    // cannot set but the service reports — "The Pending and Failed options ... are
    // returned in the response" — are what carry a reason in practice, so they are
    // seeded where the service would hold them.
    let rows: [(&str, &str, Option<&str>, &str, bool); 5] = [
        ("active-with-a-reason", "Active", Some("opt-in complete"), "Active", true),
        ("active-without-a-reason", "Active", None, "Active", false),
        ("pending-to-active", "Pending", Some("opt-in in progress"), "Active", true),
        ("failed-to-active", "Failed", Some("the opt-in did not complete"), "Active", true),
        ("inactive-with-a-reason", "Inactive", Some("opted out"), "Inactive", true),
    ];

    for (case, stored_status, stored_reason, requested, changes) in rows {
        let (router, engine) = unenrolled_emulator_with_engine();
        let caller = state::caller_account_id();
        seed_enrollment(&engine, &caller, stored_status, stored_reason, None);
        let before = stored_enrollment(&engine, &caller)
            .last_updated_timestamp
            .map(|at| at.as_secs_f64())
            .unwrap_or_default();

        sleep(CLOCK_MARGIN).await;

        let answer = call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": requested}),
            case,
        )
        .await;
        assert!(
            answer.get("statusReason").is_none(),
            "case {case:?}: the reason the stored status explained is dropped with it, got \
             {answer}"
        );
        let stamped = timestamp(&answer, "lastUpdatedTimestamp", case);
        if changes {
            assert!(
                stamped > before,
                "case {case:?}: the write changed the stored configuration, so it stamps the \
                 instant it was applied, expected a lastUpdatedTimestamp later than {before}, \
                 got {stamped}"
            );
        } else {
            assert!(
                same_instant(stamped, before),
                "case {case:?}: the write changed nothing, so the stored instant stands, \
                 expected {before}, got {stamped}"
            );
        }

        let stored = stored_enrollment(&engine, &caller);
        assert_eq!(
            stored.status, requested,
            "case {case:?}: the requested status must be what the store holds, got {stored:?}"
        );
        assert_eq!(
            stored.status_reason, None,
            "case {case:?}: a status the caller set successfully has no reason, got {stored:?}"
        );
        assert!(
            stored
                .last_updated_timestamp
                .is_some_and(|at| same_instant(at.as_secs_f64(), stamped)),
            "case {case:?}: the store must hold the instant the response reports ({stamped}), \
             got {stored:?}"
        );
    }
}
