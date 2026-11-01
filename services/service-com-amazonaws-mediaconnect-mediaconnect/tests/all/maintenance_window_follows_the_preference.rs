// SPDX-License-Identifier: CC0-1.0

//! The maintenance window a router resource reports follows the resource's OWN maintenance
//! preference, end to end over HTTP: Create -> Start -> Update -> Get.
//!
//! The rule the rows below pin: the concrete window a start scheduled was DERIVED from the
//! maintenance preference the resource carried then, so an update that replaces that
//! preference with one resolving elsewhere re-resolves the window at once — the update's own
//! answer and every later read report the next occurrence of the NEW preference — while a
//! replacement that resolves to the same window, and an update that carries no preference at
//! all, leave the scheduled window exactly as it was. A resource nothing has scheduled a
//! window for still reports none.
//!
//! The rows vary what the rule generalises over: which half of the preference moves (its
//! day, its time of day, both), the direction between the two `MaintenanceConfiguration`
//! union members the model declares
//! (`smithy_query(query="shape",shape="MaintenanceConfiguration")`: PreferredDayTime,
//! Default), a preference re-stated unchanged, one re-stated in another spelling of the same
//! time of day, and an update that carries no maintenance configuration — each row run for
//! BOTH router resources that carry a preference and a window, through UpdateRouterInput and
//! through UpdateRouterOutput.
//!
//! `tests/all/router_maintenance_reschedule.rs` pins the same rule over its own table; what
//! this suite adds is one table driven through BOTH resources by the same code — so a rule
//! that holds for one and not the other fails here — plus the cases that table does not
//! carry: two spellings of one time of day, a minute-level preference, the DEFAULT
//! configuration re-stated, and an update carrying no maintenance configuration and an
//! unstarted resource on the router OUTPUT side as well as the input one.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization. The create fixtures, the
//! state seeding and the window assertions are the ones the router input and router output
//! suites already own, so the suites cannot drift apart.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{
    configuration_rows, create_body as router_input_create_body, settle_router_input_in, union,
    union_member,
};
use crate::router_input_start::{
    assert_window_shape, day_of_week, now_secs, time_of_day, SECONDS_PER_DAY, SECONDS_PER_HOUR,
};
use crate::router_output_stop::settle_router_output_in;
use crate::router_outputs::{minted_configuration_targets, standard};

/// The hour this service documents as the one it maintains a resource at when no preference
/// names a time: "the default value is 02:00"
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceStartHour")`).
/// Stated here as seconds past midnight UTC so a row's expectation is not read from the same
/// constant the handler derives the window from; the two are checked against each other once
/// below.
const DEFAULT_MAINTENANCE_TIME: i64 = 2 * SECONDS_PER_HOUR;

/// A `MaintenanceConfiguration` naming a preferred day and time of day, keyed under both
/// union keys the crate's codec can answer with (see [`union`]).
fn preferred(day: &str, time: &str) -> Value {
    union(
        "PreferredDayTime",
        "preferredDayTime",
        json!({ "day": day, "time": time }),
    )
}

/// The other `MaintenanceConfiguration` member the model declares: the DEFAULT maintenance,
/// which names no day and no time.
fn default_maintenance() -> Value {
    union("Default", "default", json!({}))
}

/// The preference a window must follow, as a row states it: the position of the preferred
/// day in a Monday-first week (`None` for a preference that names no day, whose window is at
/// the default hour on whichever day comes next) and the seconds past midnight UTC the
/// window opens at.
struct Preference {
    day: Option<i64>,
    time: i64,
}

/// One row: the preference the resource is created with, the one an update replaces it with,
/// and the window the resource must report afterwards.
struct Row {
    case: &'static str,
    created: Value,
    /// The `MaintenanceConfiguration` the update carries, or `None` for an update that
    /// carries none at all.
    replacement: Option<Value>,
    /// The preference the scheduled window must follow once the update is applied, or `None`
    /// when the window the start scheduled must still be reported unchanged.
    rescheduled_to: Option<Preference>,
}

/// The table. The `Day` values are the ones the model declares
/// (`smithy_query(query="shape",shape="Day")`: MONDAY | TUESDAY | WEDNESDAY | THURSDAY |
/// FRIDAY | SATURDAY | SUNDAY), each row stating the Monday-first position of the day it
/// names beside it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "another day at another time of day",
            created: preferred("MONDAY", "02:00"),
            replacement: Some(preferred("FRIDAY", "10:00")),
            rescheduled_to: Some(Preference {
                day: Some(4),
                time: 10 * SECONDS_PER_HOUR,
            }),
        },
        Row {
            case: "the same day at another time of day",
            created: preferred("SUNDAY", "23:00"),
            replacement: Some(preferred("SUNDAY", "01:00")),
            rescheduled_to: Some(Preference {
                day: Some(6),
                time: SECONDS_PER_HOUR,
            }),
        },
        Row {
            case: "another day at the same time of day",
            created: preferred("WEDNESDAY", "12:00"),
            replacement: Some(preferred("THURSDAY", "12:00")),
            rescheduled_to: Some(Preference {
                day: Some(3),
                time: 12 * SECONDS_PER_HOUR,
            }),
        },
        Row {
            case: "a minute-level preference replaced by another one",
            created: preferred("TUESDAY", "17:45"),
            replacement: Some(preferred("SATURDAY", "06:15")),
            rescheduled_to: Some(Preference {
                day: Some(5),
                time: 6 * SECONDS_PER_HOUR + 15 * 60,
            }),
        },
        Row {
            case: "a weekly preference replaced by the default maintenance",
            created: preferred("SATURDAY", "06:15"),
            replacement: Some(default_maintenance()),
            rescheduled_to: Some(Preference {
                day: None,
                time: DEFAULT_MAINTENANCE_TIME,
            }),
        },
        Row {
            case: "the default maintenance replaced by a weekly preference",
            created: default_maintenance(),
            replacement: Some(preferred("TUESDAY", "17:45")),
            rescheduled_to: Some(Preference {
                day: Some(1),
                time: 17 * SECONDS_PER_HOUR + 45 * 60,
            }),
        },
        Row {
            case: "the stored preference re-stated unchanged",
            created: preferred("THURSDAY", "04:00"),
            replacement: Some(preferred("THURSDAY", "04:00")),
            rescheduled_to: None,
        },
        Row {
            case: "the same time of day written out with its seconds",
            created: preferred("FRIDAY", "09:00"),
            replacement: Some(preferred("FRIDAY", "09:00:00")),
            rescheduled_to: None,
        },
        Row {
            case: "the default maintenance re-stated unchanged",
            created: default_maintenance(),
            replacement: Some(default_maintenance()),
            rescheduled_to: None,
        },
        Row {
            case: "an update that carries no maintenance configuration",
            created: preferred("MONDAY", "02:00"),
            replacement: None,
            rescheduled_to: None,
        },
    ]
}

/// The two router resources that carry a maintenance PREFERENCE and the concrete WINDOW it
/// is resolved to, so the rule holds for both.
#[derive(Clone, Copy)]
enum Router {
    Input,
    Output,
}

impl Router {
    /// The word this resource's URIs and its response member are both keyed by
    /// (`smithy_query(query="operation",operation="UpdateRouterInput,UpdateRouterOutput")`:
    /// `PUT /v1/routerInput/{Arn}`, `PUT /v1/routerOutput/{Arn}`).
    fn word(self) -> &'static str {
        match self {
            Router::Input => "routerInput",
            Router::Output => "routerOutput",
        }
    }

    /// Put the stored record in the settled state a start is accepted from, through the
    /// seeding helper of the suite that owns that resource's lifecycle.
    fn settle_in_standby(self, emulator: &Emulator, arn: &str) {
        match self {
            Router::Input => {
                settle_router_input_in(emulator, arn, mc::ROUTER_INPUT_STATE_STANDBY);
            }
            Router::Output => {
                settle_router_output_in(emulator, arn, mc::ROUTER_OUTPUT_STATE_STANDBY);
            }
        }
    }
}

/// The configurations the creates below carry, pointing at resources this emulator HOLDS: a
/// create whose configuration names a resource the account does not hold is refused, so the
/// router network interface and the flow are minted over the wire first.
struct Fixtures {
    input_configuration: Value,
    output_configuration: Value,
}

async fn mint_fixtures(emulator: &Emulator) -> Fixtures {
    let targets = minted_configuration_targets(emulator, "maintenance-window").await;
    let input_configuration = configuration_rows(&targets.interface_arn, &targets.flow_arn)
        .into_iter()
        .next()
        .expect("the router input configuration table has rows")
        .3;
    Fixtures {
        input_configuration,
        output_configuration: standard(
            &targets.interface_arn,
            json!({"Rtp": {"destinationAddress": "198.51.100.10", "destinationPort": 5004}}),
        ),
    }
}

/// Create one router resource of kind `router` with the maintenance preference `maintenance`
/// and answer its ARN.
async fn create_resource(
    emulator: &Emulator,
    router: Router,
    fixtures: &Fixtures,
    case: &str,
    name: &str,
    maintenance: &Value,
) -> String {
    let body = match router {
        Router::Input => {
            let mut body = router_input_create_body(name, fixtures.input_configuration.clone());
            body["maintenanceConfiguration"] = maintenance.clone();
            body
        }
        Router::Output => json!({
            "name": name,
            "configuration": fixtures.output_configuration.clone(),
            "maximumBitrate": 20_000_000i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
            "maintenanceConfiguration": maintenance.clone(),
        }),
    };
    let (status, code, created) = emulator
        .send("POST", &format!("/v1/{}", router.word()), Some(body))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the seeding create expected 201, got {status} {code:?}: {created}"
    );
    created[router.word()]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
        .to_string()
}

/// Start the resource `arn` — the call that SCHEDULES its first maintenance window — and
/// answer the window it reports.
async fn start_resource(emulator: &Emulator, router: Router, case: &str, arn: &str) -> Value {
    let (status, code, started) = emulator
        .send(
            "POST",
            &format!("/v1/{}/start/{arn}", router.word()),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the start expected 202, got {status} {code:?}: {started}"
    );
    assert_eq!(
        started["maintenanceScheduleType"].as_str(),
        Some(mc::MAINTENANCE_SCHEDULE_TYPE_WINDOW),
        "case {case:?}: the start answers the type of the window it scheduled, got {started}"
    );
    started["maintenanceSchedule"].clone()
}

/// The resource the store holds for `arn`, read back through the single-resource read.
async fn read_resource(emulator: &Emulator, router: Router, case: &str, arn: &str) -> Value {
    let (status, code, read) = emulator
        .send("GET", &format!("/v1/{}/{arn}", router.word()), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the read back expected 200, got {status} {code:?}: {read}"
    );
    read[router.word()].clone()
}

/// How a `MaintenanceConfiguration` payload reads: its `MaintenanceType`, and the preferred
/// day and time a `PreferredDayTime` preference states — under either union key (see
/// [`union_member`]).
fn maintenance_reading(payload: &Value) -> (&'static str, Option<(String, String)>) {
    let preference = union_member(payload, "PreferredDayTime", "preferredDayTime");
    if preference.is_object() {
        return (
            mc::MAINTENANCE_TYPE_PREFERRED_DAY_TIME,
            Some((
                preference["day"].as_str().unwrap_or_default().to_string(),
                preference["time"].as_str().unwrap_or_default().to_string(),
            )),
        );
    }
    (mc::MAINTENANCE_TYPE_DEFAULT, None)
}

/// Assert the window `schedule` is the next occurrence of `preference`, as of `called_at`
/// (epoch seconds).
///
/// The shape of the window — it opens in the future, lasts one
/// [`mc::MAINTENANCE_WINDOW_SPAN`] and contains the maintenance it schedules — is asserted
/// through the one reading the router input start suite owns ([`assert_window_shape`]), so
/// every suite reads a window the same way; what is added here is the preference it must
/// follow and the bound on how far out the NEXT occurrence of that preference can be.
fn assert_window_follows(case: &str, schedule: &Value, called_at: i64, preference: &Preference) {
    let (start, _, _) = assert_window_shape(case, schedule, called_at);
    assert_eq!(
        time_of_day(start),
        preference.time,
        "case {case:?}: the window must open {} seconds past midnight UTC, got {start} \
         ({} seconds past midnight)",
        preference.time,
        time_of_day(start)
    );
    match preference.day {
        Some(day) => {
            assert_eq!(
                day_of_week(start),
                day,
                "case {case:?}: the window must fall on day {day} of a Monday-first week, got \
                 {start} (day {})",
                day_of_week(start)
            );
            assert!(
                start <= called_at + 7 * SECONDS_PER_DAY,
                "case {case:?}: the window must be the NEXT occurrence of the preferred day, \
                 within a week of {called_at}, got {start}"
            );
        }
        None => assert!(
            start <= called_at + SECONDS_PER_DAY,
            "case {case:?}: a preference naming no day comes round every day, so the window \
             must be within a day of {called_at}, got {start}"
        ),
    }
}

/// A replacement maintenance preference re-resolves the window a start scheduled, and a
/// replacement that resolves to the same window — or no replacement at all — leaves it
/// alone, for both router resources.
#[tokio::test]
async fn a_replaced_maintenance_preference_reschedules_the_scheduled_window() {
    // The default hour the rows expect is the one the service derives a dayless window at.
    assert_eq!(
        i64::try_from(mc::DEFAULT_MAINTENANCE_TIME_OF_DAY.as_secs()).unwrap_or(i64::MAX),
        DEFAULT_MAINTENANCE_TIME,
        "the documented default maintenance hour is {DEFAULT_MAINTENANCE_TIME} seconds past \
         midnight, got {:?}",
        mc::DEFAULT_MAINTENANCE_TIME_OF_DAY
    );

    let emulator = Emulator::new();
    let fixtures = mint_fixtures(&emulator).await;

    for router in [Router::Input, Router::Output] {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{} / {}", router.word(), row.case);
            let case = case.as_str();
            let name = format!("{}-maintenance-{index}", router.word());

            let arn =
                create_resource(&emulator, router, &fixtures, case, &name, &row.created).await;
            router.settle_in_standby(&emulator, &arn);
            let scheduled = start_resource(&emulator, router, case, &arn).await;

            // The update: a rename every row carries, so the call is one an accepted update
            // answers 202 for, plus the replacement preference the row states.
            let renamed = format!("{name}-renamed");
            let mut body = json!({ "name": renamed });
            if let Some(replacement) = row.replacement.as_ref() {
                body["maintenanceConfiguration"] = replacement.clone();
            }
            let called_at = now_secs();
            let (status, code, answered) = emulator
                .send("PUT", &format!("/v1/{}/{arn}", router.word()), Some(body))
                .await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: the update expected 202, got {status} {code:?}: {answered}"
            );
            let updated = answered[router.word()].clone();
            assert_eq!(
                updated["name"].as_str(),
                Some(renamed.as_str()),
                "case {case:?}: the update applied what it carried beside the preference, got \
                 {updated}"
            );

            // The preference the resource now carries is the replacement where the request
            // carried one, and the stored one where it did not.
            let expected_configuration = row.replacement.clone().unwrap_or(row.created.clone());
            let (expected_type, expected_preference) = maintenance_reading(&expected_configuration);
            assert_eq!(
                maintenance_reading(&updated["maintenanceConfiguration"]),
                (expected_type, expected_preference),
                "case {case:?}: the stored MaintenanceConfiguration must be the one in force, \
                 got {updated}"
            );
            assert_eq!(
                updated["maintenanceType"].as_str(),
                Some(expected_type),
                "case {case:?}: MaintenanceType follows the configuration in force, got {updated}"
            );

            // The window the update answers, and the window every later read reports, are one
            // window: the next occurrence of the preference now in force when the replacement
            // resolves elsewhere, and the window the start scheduled when it does not.
            let answered_schedule = updated["maintenanceSchedule"].clone();
            match row.rescheduled_to.as_ref() {
                Some(preference) => {
                    assert_window_follows(case, &answered_schedule, called_at, preference);
                    assert_ne!(
                        answered_schedule, scheduled,
                        "case {case:?}: the window the previous preference implied must be \
                         gone, got {answered_schedule}"
                    );
                }
                None => assert_eq!(
                    answered_schedule, scheduled,
                    "case {case:?}: the scheduled window must be left exactly as it was \
                     ({scheduled}), got {answered_schedule}"
                ),
            }
            assert_eq!(
                updated["maintenanceScheduleType"].as_str(),
                Some(mc::MAINTENANCE_SCHEDULE_TYPE_WINDOW),
                "case {case:?}: the type beside the window is the one the model declares, got \
                 {updated}"
            );

            let stored = read_resource(&emulator, router, case, &arn).await;
            assert_eq!(
                (
                    stored["maintenanceSchedule"].clone(),
                    stored["maintenanceScheduleType"].clone(),
                    stored["maintenanceConfiguration"].clone(),
                ),
                (
                    answered_schedule.clone(),
                    updated["maintenanceScheduleType"].clone(),
                    updated["maintenanceConfiguration"].clone(),
                ),
                "case {case:?}: the read side must report the record the update stored, got \
                 {stored}"
            );
        }
    }
}

/// A resource nothing has scheduled a window for still reports none once its preference is
/// replaced — the update schedules no window of its own — and the first window it is given
/// is the next occurrence of the preference then in force.
#[tokio::test]
async fn an_update_schedules_no_window_for_a_resource_that_has_none() {
    let emulator = Emulator::new();
    let fixtures = mint_fixtures(&emulator).await;

    for router in [Router::Input, Router::Output] {
        let case = format!("{} / never started", router.word());
        let case = case.as_str();
        let name = format!("{}-maintenance-unstarted", router.word());

        let arn = create_resource(
            &emulator,
            router,
            &fixtures,
            case,
            &name,
            &preferred("MONDAY", "02:00"),
        )
        .await;

        let (status, code, answered) = emulator
            .send(
                "PUT",
                &format!("/v1/{}/{arn}", router.word()),
                Some(json!({"maintenanceConfiguration": preferred("FRIDAY", "10:00")})),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the update expected 202, got {status} {code:?}: {answered}"
        );
        let updated = answered[router.word()].clone();
        assert_eq!(
            (
                updated["maintenanceSchedule"].clone(),
                updated["maintenanceScheduleType"].clone(),
            ),
            (Value::Null, Value::Null),
            "case {case:?}: nothing has scheduled a window, so the update reports none, got \
             {updated}"
        );
        let stored = read_resource(&emulator, router, case, &arn).await;
        assert_eq!(
            (
                stored["maintenanceSchedule"].clone(),
                stored["maintenanceScheduleType"].clone(),
            ),
            (Value::Null, Value::Null),
            "case {case:?}: the read side reports no window either, got {stored}"
        );

        // The first window the resource is given follows the preference in force by then,
        // which is the one the update stored.
        router.settle_in_standby(&emulator, &arn);
        let called_at = now_secs();
        let scheduled = start_resource(&emulator, router, case, &arn).await;
        assert_window_follows(
            case,
            &scheduled,
            called_at,
            &Preference {
                day: Some(4),
                time: 10 * SECONDS_PER_HOUR,
            },
        );
    }
}
