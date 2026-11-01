// SPDX-License-Identifier: CC0-1.0

//! What a `$next` poll does with the two request members it accepts, against BOTH
//! stages an execution it resolves to can be in.
//!
//! The rule under test: `StartNextPendingJobExecution` applies `statusDetails` and
//! `stepTimeoutInMinutes` to the pending execution the poll resolves to whenever the
//! request CARRIES them, and leaves a stored value alone only for a member the request
//! does not carry — the one condition the model states ("If not specified, the
//! statusDetails are unchanged"; `stepTimeoutInMinutes` "Specifies the amount of time
//! this device has to finish execution of this job", from
//! `smithy_query(query="doc", shape="StartNextPendingJobExecutionRequest",
//! member="statusDetails" / member="stepTimeoutInMinutes")`). Both stages are in
//! scope, since the operation documents the pending statuses it serves as "IN_PROGRESS
//! or QUEUED": a device re-acquiring work it already holds — the normal way an agent
//! picks up after a restart — reaches an execution already in progress, and its
//! details and its step timer are applied there too. Applying a member is a device
//! update ("Job execution versions are incremented each time they are updated by a
//! device", `JobExecution$versionNumber`), so the stamps and the version move with it;
//! the one poll that writes nothing is the one carrying NEITHER member against an
//! execution already in progress, which is merely re-handed.
//!
//! The table spans the stage the resolved execution is in (queued, already in
//! progress), which members the poll carries (neither, each alone, both), the size and
//! overlap of the map it carries (one pair, several, a map with no pair at all) and the
//! magnitude of the step timeout (each published bound and a value inside the range).
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a device agent sees, and every row reads the
//! execution back through `DescribeJobExecution`.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    now_epoch_seconds, MAX_STEP_TIMEOUT_MINUTES, MIN_STEP_TIMEOUT_MINUTES, STATUS_IN_PROGRESS,
};

// The emulator, its HTTP calls and the stored-record readers every suite of this crate
// shares.
use crate::support::{Emulator, StoredExecution};

/// Seconds in one minute: `stepTimeoutInMinutes` is documented in MINUTES while every
/// stored deadline is in seconds since the epoch.
const SECONDS_PER_MINUTE: i64 = 60;

/// The step timeout the setup poll of an already-in-progress row arms, so a row that
/// carries none of its own has a stored timer to be left alone.
const BASELINE_STEP_MINUTES: i64 = 30;

/// The stage the execution a row's poll resolves to is in. Both are pending, which is
/// what puts both in this operation's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Enqueued and never taken up: the poll STARTS it.
    Queued,
    /// Taken up by an earlier poll that armed [`BASELINE_STEP_MINUTES`] and stored a
    /// baseline map: the poll under test resolves to it again.
    InProgress,
}

/// One row: the stage the resolved execution is in and what the poll under test
/// carries.
struct Row {
    case: &'static str,
    /// One device per row, so no row depends on another's queue.
    thing: &'static str,
    stage: Stage,
    /// The `statusDetails` the poll under test carries, if any.
    details: Option<Value>,
    /// The `stepTimeoutInMinutes` the poll under test carries, if any.
    step_minutes: Option<i64>,
}

impl Row {
    /// The body of the poll under test: only the members the row carries.
    fn poll_body(&self) -> Value {
        let mut body = json!({});
        if let Some(details) = &self.details {
            body["statusDetails"] = details.clone();
        }
        if let Some(minutes) = self.step_minutes {
            body["stepTimeoutInMinutes"] = json!(minutes);
        }
        body
    }

    /// Whether this poll writes to the execution at all: it starts a queued one, and
    /// it applies any member it carries.
    fn writes(&self) -> bool {
        self.stage == Stage::Queued || self.details.is_some() || self.step_minutes.is_some()
    }
}

/// The map an already-in-progress row's setup poll stored, which a row carrying no map
/// of its own must still hold afterwards.
fn baseline_details() -> Value {
    json!({"stage": "boot", "attempt": "1"})
}

/// The table. The bounds of `stepTimeoutInMinutes` are the ones its own documentation
/// publishes (`MIN_STEP_TIMEOUT_MINUTES`..=`MAX_STEP_TIMEOUT_MINUTES`: "Valid values
/// for this parameter range from 1 to 10080").
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a queued execution started by a poll carrying neither member",
            thing: "laser-apply-queued-bare",
            stage: Stage::Queued,
            details: None,
            step_minutes: None,
        },
        Row {
            case: "a queued execution started by a poll carrying details only",
            thing: "laser-apply-queued-details",
            stage: Stage::Queued,
            details: Some(json!({"stage": "download"})),
            step_minutes: None,
        },
        Row {
            case: "a queued execution started by a poll carrying a step timeout only",
            thing: "laser-apply-queued-timer",
            stage: Stage::Queued,
            details: None,
            step_minutes: Some(MIN_STEP_TIMEOUT_MINUTES),
        },
        Row {
            case: "a queued execution started by a poll carrying both",
            thing: "laser-apply-queued-both",
            stage: Stage::Queued,
            details: Some(json!({"stage": "download", "attempt": "1"})),
            step_minutes: Some(10),
        },
        Row {
            case: "a queued execution started by a poll carrying a map with no pair",
            thing: "laser-apply-queued-empty",
            stage: Stage::Queued,
            details: Some(json!({})),
            step_minutes: None,
        },
        Row {
            case: "an execution in progress re-polled with neither member",
            thing: "laser-apply-live-bare",
            stage: Stage::InProgress,
            details: None,
            step_minutes: None,
        },
        Row {
            case: "an execution in progress re-polled with details only",
            thing: "laser-apply-live-details",
            stage: Stage::InProgress,
            details: Some(json!({"stage": "verify"})),
            step_minutes: None,
        },
        Row {
            case: "an execution in progress re-polled with a larger map",
            thing: "laser-apply-live-bigger",
            stage: Stage::InProgress,
            details: Some(json!({"stage": "install", "attempt": "2", "retry": "0"})),
            step_minutes: None,
        },
        Row {
            case: "an execution in progress re-polled with a map carrying no pair",
            thing: "laser-apply-live-empty",
            stage: Stage::InProgress,
            details: Some(json!({})),
            step_minutes: None,
        },
        Row {
            case: "an execution in progress re-polled at the documented minimum step timeout",
            thing: "laser-apply-live-min",
            stage: Stage::InProgress,
            details: None,
            step_minutes: Some(MIN_STEP_TIMEOUT_MINUTES),
        },
        Row {
            case: "an execution in progress re-polled with a step timeout inside the range",
            thing: "laser-apply-live-mid",
            stage: Stage::InProgress,
            details: None,
            step_minutes: Some(7),
        },
        Row {
            case: "an execution in progress re-polled at the documented maximum step timeout",
            thing: "laser-apply-live-max",
            stage: Stage::InProgress,
            details: None,
            step_minutes: Some(MAX_STEP_TIMEOUT_MINUTES),
        },
        Row {
            case: "an execution in progress re-polled with both members",
            thing: "laser-apply-live-both",
            stage: Stage::InProgress,
            details: Some(json!({"stage": "reboot"})),
            step_minutes: Some(MIN_STEP_TIMEOUT_MINUTES + 1),
        },
    ]
}

/// The details a response reports for a map: the map itself, or JSON `null` while it
/// holds no pair (an execution with no details reports no map at all).
fn reported(details: &Value) -> Value {
    if details.as_object().is_some_and(serde_json::Map::is_empty) {
        Value::Null
    } else {
        details.clone()
    }
}

/// Put the row's device into the stage it describes and answer the execution's id, the
/// stored record as it stands when the poll under test arrives, and the details it
/// holds by then.
async fn seed(emulator: &Emulator, row: &Row) -> (String, StoredExecution, Value) {
    let case = row.case;
    let id = emulator.start_command_on(row.thing, None).await;
    let details = match row.stage {
        Stage::Queued => Value::Null,
        Stage::InProgress => {
            // A device takes an execution up by POLLING for it, so the stage is
            // reached through the operation under test rather than by writing state.
            let (status, taken) = emulator
                .start_next(
                    row.thing,
                    json!({
                        "statusDetails": baseline_details(),
                        "stepTimeoutInMinutes": BASELINE_STEP_MINUTES,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: taking the execution up must answer 200, got {status} {taken}"
            );
            assert_eq!(
                taken["execution"]["status"].as_str(),
                Some(STATUS_IN_PROGRESS),
                "case {case:?}: the setup poll must take the execution up, got {taken}"
            );
            baseline_details()
        }
    };
    let before = emulator.stored_execution(row.thing, &id);
    (id, before, details)
}

/// RULE: a `$next` poll applies every member it CARRIES to the pending execution it
/// resolves to — `statusDetails` replacing the stored map, `stepTimeoutInMinutes`
/// arming the step timer from the instant of the call — whichever pending stage that
/// execution is in, and leaves a stored value alone only for a member the request does
/// not carry. Applying a member is a device update, so `lastUpdatedAt` and
/// `versionNumber` move with it, while `startedAt` and the job's own in-progress timer
/// belong to the first start alone.
#[tokio::test]
async fn a_poll_applies_the_details_and_step_timeout_it_carries_at_either_pending_stage() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let (id, before, stored_details) = seed(&emulator, &row).await;

        let earliest = now_epoch_seconds();
        let (status, answered) = emulator.start_next(row.thing, row.poll_body()).await;
        let latest = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: the poll must answer 200, got {status} {answered}"
        );
        assert_eq!(
            answered["execution"]["jobId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the poll must hand out this row's execution, got {answered}"
        );

        let after = emulator.stored_execution(row.thing, &id);

        // The version: one device update for a poll that applied anything, and none
        // at all for the poll that merely re-hands an execution already in progress.
        let expected_version = before.version_number + i64::from(row.writes());
        assert_eq!(
            (after.status.as_str(), after.version_number),
            (STATUS_IN_PROGRESS, expected_version),
            "case {case:?}: expected IN_PROGRESS at version {expected_version}, got {after:?}"
        );

        // `startedAt` and the job's own in-progress timer belong to the FIRST start.
        match row.stage {
            Stage::Queued => assert!(
                after
                    .started_at
                    .is_some_and(|at| (earliest..=latest).contains(&at)),
                "case {case:?}: the start must stamp startedAt within {earliest}..={latest}, got {after:?}"
            ),
            Stage::InProgress => assert_eq!(
                after.started_at, before.started_at,
                "case {case:?}: a resumed execution keeps its original startedAt, got {after:?}"
            ),
        }
        assert_eq!(
            after.execution_timeout_expires_at, before.execution_timeout_expires_at,
            "case {case:?}: the job's own timer is never re-armed by a poll, got {after:?}"
        );

        // `lastUpdatedAt` moves exactly when something was written.
        if row.writes() {
            assert!(
                (earliest..=latest).contains(&after.last_updated_at),
                "case {case:?}: expected lastUpdatedAt within {earliest}..={latest}, got {after:?}"
            );
        } else {
            assert_eq!(
                after.last_updated_at, before.last_updated_at,
                "case {case:?}: a poll that applies nothing must not stamp the execution, got {after:?}"
            );
        }

        // The step timer: armed from the instant of the call for the minutes the poll
        // carried, and otherwise exactly the deadline the record already held.
        match row.step_minutes {
            Some(minutes) => {
                let armed = after.step_timeout_expires_at.unwrap_or_else(|| {
                    panic!("case {case:?}: the poll's step timeout must arm a timer, got {after:?}")
                });
                let from = earliest + minutes * SECONDS_PER_MINUTE;
                let to = latest + minutes * SECONDS_PER_MINUTE;
                assert!(
                    (from..=to).contains(&armed),
                    "case {case:?}: expected the step timer within {from}..={to}, got {armed}"
                );
                assert_ne!(
                    Some(armed), before.step_timeout_expires_at,
                    "case {case:?}: the row must move the deadline it asserts, got {after:?}"
                );
            }
            None => assert_eq!(
                after.step_timeout_expires_at, before.step_timeout_expires_at,
                "case {case:?}: a poll carrying no step timeout must leave the timer as it was, got {after:?}"
            ),
        }

        // The details: the map the poll carried, or the stored map while it carried
        // none.
        let expected_details = reported(&row.details.clone().unwrap_or(stored_details));
        assert_eq!(
            answered["execution"]["statusDetails"], expected_details,
            "case {case:?}: expected the poll to report {expected_details}, got {answered}"
        );

        // The seconds the poll reported are the ones the record it handed out carries.
        let reported_seconds = answered["execution"]["approximateSecondsBeforeTimedOut"].as_i64();
        match after.deadline() {
            Some(deadline) => assert!(
                reported_seconds
                    .is_some_and(|left| ((deadline - latest)..=(deadline - earliest)).contains(&left)),
                "case {case:?}: expected the seconds left on the deadline {deadline}, got {answered}"
            ),
            None => assert_eq!(
                reported_seconds, None,
                "case {case:?}: an execution with no timer armed reports no seconds left, got {answered}"
            ),
        }

        // Read it back the way a device does: what the poll applied is what the
        // execution holds.
        let (status, described) = emulator.describe(row.thing, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected to read the execution back, got {status} {described}"
        );
        assert_eq!(
            (
                described["execution"]["status"].as_str(),
                described["execution"]["versionNumber"].as_i64(),
                described["execution"]["statusDetails"].clone(),
            ),
            (
                Some(STATUS_IN_PROGRESS),
                Some(expected_version),
                expected_details.clone(),
            ),
            "case {case:?}: the read must report what the poll applied, got {described}"
        );
    }
}

/// RULE, from the device's side: a device may keep reporting through `$next` — each
/// poll that carries a member applies it and advances the version, so the version the
/// LAST poll handed out is the one its own `UpdateJobExecution` must name, while a poll
/// carrying no member changes nothing however often it is repeated.
///
/// The rows differ in what every repeat carries, which is what decides whether the
/// repeats advance the execution or leave it exactly as the first poll did.
#[tokio::test]
async fn repeated_polls_apply_what_they_carry_and_the_last_version_is_the_one_an_update_must_name()
{
    let emulator = Emulator::new();
    const REPEATS: i64 = 3;

    for (index, (case, body, applies)) in [
        ("repeats carrying neither member", json!({}), false),
        (
            "repeats carrying details",
            json!({"statusDetails": {"stage": "flash"}}),
            true,
        ),
        (
            "repeats carrying a step timeout",
            json!({"stepTimeoutInMinutes": 7}),
            true,
        ),
        (
            "repeats carrying both",
            json!({"statusDetails": {"stage": "flash", "attempt": "2"}, "stepTimeoutInMinutes": 9}),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let thing = format!("laser-repeat-{index}");
        let id = emulator.start_command_on(&thing, None).await;
        // A second execution queued behind it: the one in progress is handed out until
        // it finishes, so every repeat resolves to the same execution.
        let behind = emulator.start_command_on(&thing, None).await;

        // The first poll starts the execution, whatever it carries.
        let (status, started) = emulator.start_next(&thing, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first poll must answer 200, got {status} {started}"
        );
        let mut version = started["execution"]["versionNumber"]
            .as_i64()
            .unwrap_or_default();

        let first_repeat = emulator.stored_execution(&thing, &id);
        for repeat in 1..=REPEATS {
            let (status, again) = emulator.start_next(&thing, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: repeat {repeat} must answer 200, got {status} {again}"
            );
            assert_eq!(
                again["execution"]["jobId"].as_str(),
                Some(id.as_str()),
                "case {case:?}: repeat {repeat} must resolve to the execution in progress, got {again}"
            );
            let expected = version + i64::from(applies);
            assert_eq!(
                again["execution"]["versionNumber"].as_i64(),
                Some(expected),
                "case {case:?}: repeat {repeat} expected version {expected}, got {again}"
            );
            version = expected;

            let now = emulator.stored_execution(&thing, &id);
            if !applies {
                assert_eq!(
                    now, first_repeat,
                    "case {case:?}: repeat {repeat} carries nothing, so it must change nothing, got {now:?}"
                );
            }
        }

        // The version the last poll handed out is the current one: the device's own
        // update is accepted naming it, and naming the one before it is refused.
        let stale = version - 1;
        let (status, refused) = emulator
            .update(
                &thing,
                &id,
                json!({"status": "SUCCEEDED", "expectedVersion": stale}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the version before the last poll to be stale, got {status} {refused}"
        );
        let (status, finished) = emulator
            .update(
                &thing,
                &id,
                json!({
                    "status": "SUCCEEDED",
                    "expectedVersion": version,
                    "includeJobExecutionState": true,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the version the last poll handed out must be current, got {status} {finished}"
        );

        // Only now is the execution behind handed out, which is what proves every
        // repeat resolved to the one in progress.
        let (status, next) = emulator.start_next(&thing, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the poll after it finished, got {status} {next}"
        );
        assert_eq!(
            next["execution"]["jobId"].as_str(),
            Some(behind.as_str()),
            "case {case:?}: the queued execution waits for the one in progress, got {next}"
        );
    }
}
