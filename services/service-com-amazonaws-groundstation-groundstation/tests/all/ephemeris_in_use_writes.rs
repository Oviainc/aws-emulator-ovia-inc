// SPDX-License-Identifier: CC0-1.0

//! The in-use hold on a stored ephemeris as `UpdateEphemeris` (`PUT
//! /ephemeris/{ephemerisId}`) meets it, end to end over HTTP through the same front door as
//! the rest of this target's suites, with every write read back through `DescribeEphemeris`
//! (`GET /ephemeris/{ephemerisId}`) and `GetSatellite` (`GET /satellite/{satelliteId}`).
//!
//! The RULE under test: a resource in use by non-terminal contacts "cannot be modified or
//! deleted" (`smithy_query(query="doc", shape="ResourceInUseException")`), so the hold a
//! contact still to be flown has on the trajectory it is flown on binds EVERY writer of a
//! stored ephemeris and not only the delete — a write that would leave such a pass with no
//! trajectory at all (this ephemeris taken out of force for its satellite and no other
//! ephemeris of the account taking that satellite over) is refused with the modelled
//! `InvalidParameterException` (HTTP 431) naming the contacts that would have been left
//! without one, and writes nothing, while a write that leaves the pass a trajectory is
//! answered.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table of writes that would take the only trajectory of a standing pass away,
//!   spanning which members they carry beside the flag — nothing, a new name, the documented
//!   minimum `priority` and the published maximum of `EphemerisPriority` — each refused,
//!   with every member of the ephemeris then asserted as the create stored it and the delete
//!   that refusal protects still refused, so the two-call release of that guard is closed;
//! * the whole `ContactStatus` enum, the one contact in each value in turn: the write is
//!   refused for every non-terminal status and answered for every terminal one, in both
//!   directions of the flag;
//! * the boundary of "would be left without one": a table of ephemerides whose write leaves
//!   no pass without a trajectory — one another ephemeris of the same satellite takes over,
//!   one that is not the ephemeris in force, one of a satellite no contact is flown against
//!   and one that applies to no satellite at all — every one of which is answered while that
//!   contact stands.
//!
//! The harness, the signed `restJson1` requests, the onboarding helper, the ephemeris data
//! builders and the stored-contact helpers are [`crate::scheduling_loop`]'s,
//! [`crate::ephemeris_catalog`]'s and [`crate::ephemeris_deletion`]'s, reused rather than
//! rebuilt.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    ephemeris_in_use_for_update_message, ephemeris_in_use_message, EPHEMERIS_DISABLED,
    EPHEMERIS_ENABLED, TERMINAL_CONTACT_STATUSES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{create_ok, onboard, tle_inline, wait_out_validation, FUTURE};
use crate::ephemeris_deletion::{put_in_status, reserve_against, satellite_arn, CONTACT_STATUSES};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE,
};

/// The ground station every satellite here is on-boarded to and every contact is reserved
/// at: the station [`crate::ephemeris_deletion::reserve_against`] places a reservation at.
const STATION: &str = "Ohio 1";

/// The validity range the data every ephemeris here carries declares.
const DATA_RANGE: (i64, i64) = (1_800_000_000, 1_800_086_400);

/// The `name` every ephemeris here is created with, so a refused write is caught by the
/// name it did NOT change as well as by the flag.
const HELD_NAME: &str = "held trajectory";

/// The `priority` every ephemeris under test is created with: neither the documented
/// default nor either published bound, so a refused write is caught by the priority it did
/// not change too.
const CREATED_PRIORITY: i64 = 50;

/// The largest priority the modelled `EphemerisPriority` admits (range 0..99999 —
/// `smithy_query(query="shape", shape="EphemerisPriority")`).
const MAX_EPHEMERIS_PRIORITY: i64 = 99_999;

/// The smallest priority the member's own documentation admits: "Higher numbers take
/// precedence. The default is 1. Must be 1 or greater." (`smithy_query(query="doc",
/// shape="UpdateEphemerisRequest", member="priority")`).
const MIN_PRIORITY: i64 = 1;

/// `CreateEphemeris` for an ephemeris named `name`, asked to be enabled, carrying
/// `priority` when the caller names one, and applying to `satellite_id` when it names one.
async fn create_for(
    router: &MultiServiceRouter,
    case: &str,
    satellite_id: Option<&str>,
    name: &str,
    priority: Option<i64>,
) -> String {
    let mut body = json!({
        "name": name,
        "enabled": true,
        "expirationTime": FUTURE,
        "ephemeris": tle_inline(&[DATA_RANGE]),
    });
    if let Some(satellite_id) = satellite_id {
        body["satelliteId"] = json!(satellite_id);
    }
    if let Some(priority) = priority {
        body["priority"] = json!(priority);
    }
    create_ok(router, case, body).await
}

/// `UpdateEphemeris` for `ephemeris_id` with `body`.
async fn update(router: &MultiServiceRouter, ephemeris_id: &str, body: Value) -> Reply {
    send(
        router,
        "PUT",
        &format!("/ephemeris/{ephemeris_id}"),
        Some(body),
    )
    .await
}

/// `UpdateEphemeris` with `body`, asserting it was answered and answered the id it was
/// addressed by.
async fn update_ok(router: &MultiServiceRouter, case: &str, ephemeris_id: &str, body: Value) {
    let reply = update(router, ephemeris_id, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected UpdateEphemeris 200, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("ephemerisId"),
        ephemeris_id,
        "case {case}: an update must answer the id it addressed, got {}",
        reply.body
    );
}

/// Assert the write `body` is refused as the in-use failure naming `holding`.
async fn assert_update_refused(
    router: &MultiServiceRouter,
    case: &str,
    ephemeris_id: &str,
    body: Value,
    holding: &[String],
) {
    let refused = update(router, ephemeris_id, body).await;
    assert_eq!(
        refused.status, 431,
        "case {case}: expected the modelled InvalidParameterException status 431, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterException",
        "case {case}: wrong error shape, got {}",
        refused.body
    );
    assert_eq!(
        refused.string("message"),
        ephemeris_in_use_for_update_message(holding),
        "case {case}: the refusal must name the contacts that would have been left without \
         a trajectory, got {}",
        refused.body
    );
}

/// Assert the delete of `ephemeris_id` is refused as the modelled in-use failure naming
/// `holding` — the guard the write refusal above is what keeps standing.
async fn assert_delete_refused(
    router: &MultiServiceRouter,
    case: &str,
    ephemeris_id: &str,
    holding: &[String],
) {
    let refused = send(router, "DELETE", &format!("/ephemeris/{ephemeris_id}"), None).await;
    assert_eq!(
        refused.status, 409,
        "case {case}: expected the modelled ResourceInUseException status 409, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("message"),
        ephemeris_in_use_message(holding),
        "case {case}: the delete refusal must name the contact that holds the ephemeris, \
         got {}",
        refused.body
    );
}

/// Assert `DescribeEphemeris` reports the ephemeris exactly as the case expects it to
/// stand: its status, its flag, its name and its priority.
async fn assert_reads_back(
    router: &MultiServiceRouter,
    case: &str,
    ephemeris_id: &str,
    status: &str,
    enabled: bool,
    name: &str,
    priority: i64,
) {
    let read = send(router, "GET", &format!("/ephemeris/{ephemeris_id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected DescribeEphemeris 200, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        read.string("status"),
        status,
        "case {case}: expected status {status}, got {}",
        read.body
    );
    assert_eq!(
        read.field("enabled").as_bool(),
        Some(enabled),
        "case {case}: expected enabled {enabled}, got {}",
        read.body
    );
    assert_eq!(
        read.string("name"),
        name,
        "case {case}: expected name {name:?}, got {}",
        read.body
    );
    assert_eq!(
        read.field("priority").as_i64(),
        Some(priority),
        "case {case}: expected priority {priority}, got {}",
        read.body
    );
}

/// The `ephemerisId` `GetSatellite` reports as the satellite's `currentEphemeris`, or
/// `None` where the account holds none in force for it.
async fn current_ephemeris(
    router: &MultiServiceRouter,
    case: &str,
    satellite_id: &str,
) -> Option<String> {
    let reply = send(router, "GET", &format!("/satellite/{satellite_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetSatellite 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .body
        .get("currentEphemeris")
        .filter(|value| !value.is_null())
        .and_then(|current| current.get("ephemerisId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One row of the refusal table: a write that would take the only trajectory of a pass
/// still to be flown away, and what it carries beside the flag that does it.
struct Taking {
    case: &'static str,
    body: Value,
}

/// The table: the flag that takes the ephemeris out of force, alone and beside each other
/// member an `UpdateEphemerisRequest` may carry — a new `name`, the documented minimum
/// `priority` and the published maximum of `EphemerisPriority` — because what makes the
/// write a taking is the trajectory it removes, not which members ride with it.
fn takings() -> Vec<Taking> {
    vec![
        Taking {
            case: "the flag alone",
            body: json!({"enabled": false}),
        },
        Taking {
            case: "the flag beside a new name",
            body: json!({"enabled": false, "name": "renamed while held"}),
        },
        Taking {
            case: "the flag beside the documented minimum priority",
            body: json!({"enabled": false, "priority": MIN_PRIORITY}),
        },
        Taking {
            case: "the flag beside the published maximum priority",
            body: json!({"enabled": false, "priority": MAX_EPHEMERIS_PRIORITY}),
        },
    ]
}

#[tokio::test]
async fn a_write_that_would_take_the_only_trajectory_of_a_standing_pass_is_refused() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, &format!("25994={STATION}"));
    let [satellite_id] = <[String; 1]>::try_from(satellites).expect("one satellite is on-boarded");
    let flown = satellite_arn(&engine, &satellite_id);
    let profile_arn = create_mission_profile(&router, "ephemeris write hold", 60, None, None).await;
    let ephemeris_id = create_for(
        &router,
        "the only trajectory",
        Some(&satellite_id),
        HELD_NAME,
        Some(CREATED_PRIORITY),
    )
    .await;
    let contact_id = reserve_against(&router, "the standing pass", &profile_arn, &flown, 0).await;
    // The ephemeris has left `VALIDATING` and the scheduler has made its own move by the
    // time the writes below run, so each row is judged against the statuses that hold then.
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);

    let holding = vec![contact_id];
    // The pass is flown on this ephemeris: the satellite reports it as its trajectory, and
    // the delete of it is refused as the modelled in-use failure.
    let before = current_ephemeris(&router, "before any write", &satellite_id).await;
    assert_eq!(
        before.as_deref(),
        Some(ephemeris_id.as_str()),
        "case before any write: the satellite must report the ephemeris as its \
         currentEphemeris, got {before:?}"
    );
    assert_delete_refused(&router, "before any write", &ephemeris_id, &holding).await;

    for row in takings() {
        assert_update_refused(&router, row.case, &ephemeris_id, row.body, &holding).await;
        // A refused write writes NOTHING: every member stands as the create stored it, the
        // satellite still reports the ephemeris as its trajectory, and the delete that
        // refusal protects is still refused — so the two-call release of that guard is
        // closed.
        assert_reads_back(
            &router,
            row.case,
            &ephemeris_id,
            EPHEMERIS_ENABLED,
            true,
            HELD_NAME,
            CREATED_PRIORITY,
        )
        .await;
        let still = current_ephemeris(&router, row.case, &satellite_id).await;
        assert_eq!(
            still.as_deref(),
            Some(ephemeris_id.as_str()),
            "case {}: a refused write leaves the satellite reporting the same trajectory, \
             got {still:?}",
            row.case
        );
        assert_delete_refused(&router, row.case, &ephemeris_id, &holding).await;
    }

    // The writes that leave the pass its trajectory are answered on that same store: the
    // flag as it already stands, and a new name and priority beside it.
    update_ok(
        &router,
        "the flag re-asserted",
        &ephemeris_id,
        json!({"enabled": true}),
    )
    .await;
    assert_reads_back(
        &router,
        "the flag re-asserted",
        &ephemeris_id,
        EPHEMERIS_ENABLED,
        true,
        HELD_NAME,
        CREATED_PRIORITY,
    )
    .await;
    update_ok(
        &router,
        "a new name and priority while held",
        &ephemeris_id,
        json!({"enabled": true, "name": "renamed while flown", "priority": MAX_EPHEMERIS_PRIORITY}),
    )
    .await;
    assert_reads_back(
        &router,
        "a new name and priority while held",
        &ephemeris_id,
        EPHEMERIS_ENABLED,
        true,
        "renamed while flown",
        MAX_EPHEMERIS_PRIORITY,
    )
    .await;
    let kept = current_ephemeris(&router, "a new name and priority while held", &satellite_id).await;
    assert_eq!(
        kept.as_deref(),
        Some(ephemeris_id.as_str()),
        "case a new name and priority while held: the ephemeris is still the satellite's \
         trajectory, got {kept:?}"
    );
}

#[tokio::test]
async fn the_hold_on_a_write_follows_the_whole_contact_status_enum() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, &format!("27424={STATION}"));
    let [satellite_id] = <[String; 1]>::try_from(satellites).expect("one satellite is on-boarded");
    let flown = satellite_arn(&engine, &satellite_id);
    let profile_arn =
        create_mission_profile(&router, "ephemeris lifecycle hold", 60, None, None).await;
    let ephemeris_id = create_for(
        &router,
        "the only trajectory",
        Some(&satellite_id),
        HELD_NAME,
        Some(CREATED_PRIORITY),
    )
    .await;
    let contact_id = reserve_against(&router, "the one contact", &profile_arn, &flown, 0).await;
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);
    let holding = vec![contact_id.clone()];

    for status in CONTACT_STATUSES {
        put_in_status(&engine, &contact_id, status);

        if TERMINAL_CONTACT_STATUSES.contains(status) {
            // Nothing of a contact at an end of its lifecycle is still to be flown, so it
            // holds no trajectory and the write lands.
            update_ok(&router, status, &ephemeris_id, json!({"enabled": false})).await;
            assert_reads_back(
                &router,
                status,
                &ephemeris_id,
                EPHEMERIS_DISABLED,
                false,
                HELD_NAME,
                CREATED_PRIORITY,
            )
            .await;
            let emptied = current_ephemeris(&router, status, &satellite_id).await;
            assert!(
                emptied.is_none(),
                "case {status}: a disabled ephemeris is in force for nothing, got {emptied:?}"
            );
            // Put back in force for the next row, which is the same write in the other
            // direction.
            update_ok(&router, status, &ephemeris_id, json!({"enabled": true})).await;
            assert_reads_back(
                &router,
                status,
                &ephemeris_id,
                EPHEMERIS_ENABLED,
                true,
                HELD_NAME,
                CREATED_PRIORITY,
            )
            .await;
            continue;
        }

        assert_update_refused(
            &router,
            status,
            &ephemeris_id,
            json!({"enabled": false}),
            &holding,
        )
        .await;
        assert_reads_back(
            &router,
            status,
            &ephemeris_id,
            EPHEMERIS_ENABLED,
            true,
            HELD_NAME,
            CREATED_PRIORITY,
        )
        .await;
    }
}

/// Why the row's write leaves no pass still to be flown without a trajectory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Another ephemeris the account holds enabled for the SAME satellite takes it over, so
    /// the standing pass is handed to the one this ephemeris superseded.
    TakenOver,
    /// The row's ephemeris is not the one in force, so no pass is flown on it.
    Superseded,
    /// Its satellite is one no contact is flown against.
    NoContact,
    /// It applies to no satellite at all.
    NoSatellite,
}

/// One row of the boundary table.
struct Boundary {
    case: &'static str,
    standing: Standing,
}

/// The table: every way a write that takes an ephemeris out of force still leaves no pass
/// without a trajectory.
fn boundaries() -> Vec<Boundary> {
    vec![
        Boundary {
            case: "another ephemeris takes the satellite over",
            standing: Standing::TakenOver,
        },
        Boundary {
            case: "not the ephemeris in force",
            standing: Standing::Superseded,
        },
        Boundary {
            case: "a satellite no contact is flown against",
            standing: Standing::NoContact,
        },
        Boundary {
            case: "an ephemeris applying to no satellite",
            standing: Standing::NoSatellite,
        },
    ]
}

/// What one row of [`boundaries`] stored: the satellite it was given, the ephemeris the
/// write is addressed at, the other ephemeris of that satellite when the row holds one, and
/// the ephemeris in force for it once the write has landed.
struct Stored {
    satellite_id: String,
    written: String,
    in_force_after: Option<String>,
}

#[tokio::test]
async fn a_write_is_answered_wherever_no_pass_is_left_without_a_trajectory() {
    let (router, engine) = harness_with_engine();
    let rows = boundaries();
    // One satellite per row, each with its own ephemerides and its own contact, so one
    // row's write cannot be what the next row observes. The NORAD catalogue numbers fall
    // inside `state::NORAD_SATELLITE_ID_RANGE`.
    let setting: Vec<String> = (0..rows.len())
        .map(|row| format!("{}={STATION}", 25_200 + row))
        .collect();
    let satellites = onboard(&engine, &setting.join(","));
    assert_eq!(
        satellites.len(),
        rows.len(),
        "case setup: the onboarding must store one satellite per row, got {satellites:?}"
    );
    let profile_arn =
        create_mission_profile(&router, "ephemeris write boundary", 60, None, None).await;

    let mut stored: Vec<Stored> = Vec::new();
    for (slot, row) in rows.iter().enumerate() {
        let satellite_id = satellites[slot].clone();
        let applies_to = (row.standing != Standing::NoSatellite).then(|| satellite_id.clone());
        // The row's two ephemerides, where it holds two: which of them the write is
        // addressed at is what the row differs by, so the one in force differs with it.
        let (written_priority, other_priority) = match row.standing {
            Standing::TakenOver => (9, Some(1)),
            Standing::Superseded => (1, Some(9)),
            Standing::NoContact | Standing::NoSatellite => (CREATED_PRIORITY, None),
        };
        let written = create_for(
            &router,
            row.case,
            applies_to.as_deref(),
            HELD_NAME,
            Some(written_priority),
        )
        .await;
        let other = match other_priority {
            None => None,
            Some(priority) => Some(
                create_for(
                    &router,
                    row.case,
                    Some(&satellite_id),
                    "the other trajectory",
                    Some(priority),
                )
                .await,
            ),
        };
        // A contact is flown against the row's satellite wherever the row has one to hold
        // the trajectory.
        if matches!(row.standing, Standing::TakenOver | Standing::Superseded) {
            let flown = satellite_arn(&engine, &satellite_id);
            reserve_against(&router, row.case, &profile_arn, &flown, slot).await;
        }
        let in_force_after = match row.standing {
            // The write takes the row's ephemeris out of force, so whatever else the row
            // holds for that satellite takes it over; where the row holds nothing else, and
            // where the ephemeris applies to no satellite, the satellite is left with none.
            Standing::TakenOver => other.clone(),
            Standing::Superseded => other.clone(),
            Standing::NoContact | Standing::NoSatellite => None,
        };
        stored.push(Stored {
            satellite_id,
            written,
            in_force_after,
        });
    }

    // ONE wait for the whole table, after every row is stored.
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);

    for (row, stored) in rows.iter().zip(&stored) {
        update_ok(
            &router,
            row.case,
            &stored.written,
            json!({"enabled": false}),
        )
        .await;
        assert_reads_back(
            &router,
            row.case,
            &stored.written,
            EPHEMERIS_DISABLED,
            false,
            HELD_NAME,
            match row.standing {
                Standing::TakenOver => 9,
                Standing::Superseded => 1,
                Standing::NoContact | Standing::NoSatellite => CREATED_PRIORITY,
            },
        )
        .await;

        let reported = current_ephemeris(&router, row.case, &stored.satellite_id).await;
        assert_eq!(
            reported.as_deref(),
            stored.in_force_after.as_deref(),
            "case {}: expected the satellite to report {:?} as its trajectory once the \
             write landed, got {reported:?}",
            row.case,
            stored.in_force_after
        );
    }
}
