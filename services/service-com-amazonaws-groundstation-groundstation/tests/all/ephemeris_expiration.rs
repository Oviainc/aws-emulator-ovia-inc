// SPDX-License-Identifier: CC0-1.0

//! `DescribeEphemeris` (`GET /ephemeris/{ephemerisId}`) over the EXPIRATION of the
//! ephemeris it reports, end to end over HTTP through the same front door as the rest of
//! this target's suites.
//!
//! The RULE under test: the `EphemerisStatus` `DescribeEphemeris` reports is `EXPIRED`
//! exactly when the expiration in force for that ephemeris has already passed — the
//! `expirationTime` the create named, or, when it named none, the last instant the
//! supplied data declares itself valid for, and no expiration at all when neither is
//! known — and the move onto `EXPIRED` is owed by the CLOCK, so a read that materialises
//! it reports what any caller would have been owed at that instant and changes nothing
//! else about the ephemeris.
//!
//! `CreateEphemeris` documents `expirationTime` as "an overall expiration time for the
//! ephemeris in UTC, after which it will become EXPIRED" (`smithy_query(query="doc",
//! shape="CreateEphemerisRequest", member="expirationTime")`), and a `validTimeRange` is
//! the window its data "is valid" over (`smithy_query(query="shape", shape="TLEData")` /
//! `shape="AzElSegment"`), which is where an ephemeris created without an explicit
//! expiration stops being usable.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning what the rule generalises over — whether the request names an
//!   expiration at all, whether the one it names has passed, which `EphemerisData` member
//!   carries the data (`tle`, `oem`, `azEl`), whether that data declares a validity of its
//!   own (inline) or declares none here (supplied from an Amazon S3 object, and an OEM
//!   body this emulator stores verbatim), how many validity ranges it declares, and
//!   whether the caller asked for the ephemeris to be enabled — every row read back
//!   through `DescribeEphemeris`;
//! * the clock-owed move itself, on an ephemeris whose expiration falls DURING the test:
//!   the same id reports the status in force before that instant and `EXPIRED` after it,
//!   with nothing but the status moving.
//!
//! The harness, the signed `restJson1` requests and the ephemeris request builders are
//! [`crate::scheduling_loop`]'s and [`crate::ephemeris_catalog`]'s, reused rather than
//! rebuilt.

use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    EPHEMERIS_DISABLED, EPHEMERIS_ENABLED, EPHEMERIS_EXPIRED, EPHEMERIS_VALIDATING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, s3_object, tle_inline, wait_out_validation, FUTURE, OEM_BODY, PAST,
};
use crate::scheduling_loop::{harness, send};

/// `DescribeEphemeris` for `ephemeris_id`, asserting it was answered, and the body it
/// answered with.
async fn describe_ok(router: &MultiServiceRouter, case: &str, ephemeris_id: &str) -> Value {
    let reply = send(router, "GET", &format!("/ephemeris/{ephemeris_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeEphemeris 200, got {} {}",
        reply.status, reply.body
    );
    reply.body
}

/// A JSON member of `body`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Now, in epoch seconds — the clock the expiration in force is judged against.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs() as i64
}

/// One row of the expiration table: what the request asks for, and the `EphemerisStatus`
/// the ephemeris must be reported in once the validation the create started has fallen
/// due.
struct Row {
    case: &'static str,
    /// The `ephemeris` member the request carries.
    data: Value,
    /// The `expirationTime` the request names, or `None` where it names none and the
    /// data's own validity is what expires the ephemeris.
    expiration_time: Option<i64>,
    /// Whether the request asks for the ephemeris to be enabled, so the table shows the
    /// expiration overriding either verdict a validated ephemeris can land on.
    enabled: bool,
    settled_status: &'static str,
}

/// The expiration table: one row per way the expiration in force is established, crossed
/// with the data forms that declare a validity of their own and the ones that declare
/// none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no expirationTime, a tle validity that already ended",
            data: tle_inline(&[(PAST - 86_400, PAST)]),
            expiration_time: None,
            enabled: true,
            settled_status: EPHEMERIS_EXPIRED,
        },
        Row {
            case: "no expirationTime, a tle validity still ahead",
            data: tle_inline(&[(FUTURE, FUTURE + 86_400)]),
            expiration_time: None,
            enabled: true,
            settled_status: EPHEMERIS_ENABLED,
        },
        Row {
            case: "no expirationTime, a tle validity still ahead, not asked to be enabled",
            data: tle_inline(&[(FUTURE, FUTURE + 3_600)]),
            expiration_time: None,
            enabled: false,
            settled_status: EPHEMERIS_DISABLED,
        },
        Row {
            case: "no expirationTime, azEl segments that already ended",
            data: az_el_inline(&[(PAST - 86_400, PAST)]),
            expiration_time: None,
            enabled: false,
            settled_status: EPHEMERIS_EXPIRED,
        },
        Row {
            case: "no expirationTime, azEl segments still ahead",
            data: az_el_inline(&[(FUTURE, FUTURE + 3_600)]),
            expiration_time: None,
            enabled: true,
            settled_status: EPHEMERIS_ENABLED,
        },
        Row {
            case: "no expirationTime, an oem body, which declares no validity here",
            data: json!({"oem": {"oemData": OEM_BODY}}),
            expiration_time: None,
            enabled: true,
            settled_status: EPHEMERIS_ENABLED,
        },
        Row {
            case: "no expirationTime, a tle supplied from an Amazon S3 object",
            data: json!({"tle": {"s3Object": s3_object("tle/aqua.txt")}}),
            expiration_time: None,
            enabled: false,
            settled_status: EPHEMERIS_DISABLED,
        },
        Row {
            case: "no expirationTime, azEl segments supplied from an Amazon S3 object",
            data: json!({"azEl": {
                "groundStation": "Ohio 1",
                "data": {"s3Object": s3_object("azel/ohio.json")},
            }}),
            expiration_time: None,
            enabled: true,
            settled_status: EPHEMERIS_ENABLED,
        },
        Row {
            case: "an expirationTime already passed, over data still valid",
            data: tle_inline(&[(FUTURE, FUTURE + 86_400)]),
            expiration_time: Some(PAST),
            enabled: true,
            settled_status: EPHEMERIS_EXPIRED,
        },
        Row {
            case: "an expirationTime still ahead, over data that already ended",
            data: tle_inline(&[(PAST - 86_400, PAST)]),
            expiration_time: Some(FUTURE),
            enabled: true,
            settled_status: EPHEMERIS_ENABLED,
        },
        Row {
            case: "no expirationTime, the last of several tle validity ranges still ahead",
            data: tle_inline(&[(PAST - 86_400, PAST), (FUTURE, FUTURE + 86_400)]),
            expiration_time: None,
            enabled: true,
            settled_status: EPHEMERIS_ENABLED,
        },
        Row {
            case: "no expirationTime, every one of several tle validity ranges already ended",
            data: tle_inline(&[(PAST - 172_800, PAST - 86_400), (PAST - 86_400, PAST)]),
            expiration_time: None,
            enabled: false,
            settled_status: EPHEMERIS_EXPIRED,
        },
    ]
}

#[tokio::test]
async fn describe_ephemeris_reports_expired_exactly_when_the_expiration_in_force_has_passed() {
    let router = harness();
    let rows = rows();

    let mut created: Vec<(String, String, &Row)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("expiration row {index}");
        let mut body = json!({
            "name": name,
            "enabled": row.enabled,
            "ephemeris": row.data,
        });
        if let Some(expiration) = row.expiration_time {
            body["expirationTime"] = json!(expiration);
        }
        let id = create_ok(&router, row.case, body).await;
        created.push((id, name, row));
    }

    wait_out_validation();
    for (id, name, row) in &created {
        let body = describe_ok(&router, row.case, id).await;

        // Each id describes its OWN ephemeris: the whole table lives in one account.
        assert_eq!(
            text(&body, "ephemerisId"),
            *id,
            "case {}: an ephemeris is described under the id it was minted with, got {body}",
            row.case
        );
        assert_eq!(
            text(&body, "name"),
            *name,
            "case {}: wrong reported name, got {body}",
            row.case
        );
        assert_eq!(
            text(&body, "status"),
            row.settled_status,
            "case {}: expected status {} once the validation fell due, got {body}",
            row.case,
            row.settled_status
        );
        // None of these rows supplies data that fails validation, so none names a reason.
        assert!(
            body.get("invalidReason").is_none_or(Value::is_null),
            "case {}: an ephemeris that did not fail validation names no invalidReason, \
             got {body}",
            row.case
        );

        // A read reports; it changes nothing a later caller sees.
        assert_eq!(
            describe_ok(&router, row.case, id).await,
            body,
            "case {}: describing an ephemeris twice must answer the same ephemeris",
            row.case
        );
    }
}

/// How far ahead the suite below puts the expiration it then waits out: long enough that
/// the read taken once the validation has fallen due is still inside it on a loaded
/// machine, short enough to wait for.
const EXPIRES_IN: Duration = Duration::from_secs(3);

#[tokio::test]
async fn an_ephemeris_moves_to_expired_on_the_clock_alone() {
    let router = harness();

    // The expiration in force is read from the clock, so this one is set in the test's own
    // future rather than at a fixed instant: the same id is then described on both sides
    // of it.
    let expiration = now_seconds() + EXPIRES_IN.as_secs() as i64;
    let id = create_ok(
        &router,
        "an expiration during the test",
        json!({
            "name": "expiring ephemeris",
            "enabled": true,
            "expirationTime": expiration,
            "ephemeris": tle_inline(&[(FUTURE, FUTURE + 86_400)]),
        }),
    )
    .await;

    // Admitted in VALIDATING: an expiration still ahead does not skip the validation the
    // create started.
    let admitted = describe_ok(&router, "before the validation fell due", &id).await;
    assert_eq!(
        text(&admitted, "status"),
        EPHEMERIS_VALIDATING,
        "case before the validation fell due: an ephemeris is admitted in \
         {EPHEMERIS_VALIDATING}, got {admitted}"
    );

    wait_out_validation();
    // Only assert the pre-expiration status while the clock really is inside the window:
    // a machine slow enough to spend the whole window waiting out the validation owes
    // EXPIRED already, and that is the rule rather than a failure.
    let before = describe_ok(&router, "before the expiration", &id).await;
    if now_seconds() < expiration {
        assert_eq!(
            text(&before, "status"),
            EPHEMERIS_ENABLED,
            "case before the expiration: an ephemeris whose expiration is still ahead \
             reports the verdict it validated to, got {before}"
        );
    }

    // Past the expiration, with no call in between: the move is the clock's.
    while now_seconds() <= expiration {
        sleep(Duration::from_millis(100));
    }
    let after = describe_ok(&router, "after the expiration", &id).await;
    assert_eq!(
        text(&after, "status"),
        EPHEMERIS_EXPIRED,
        "case after the expiration: an ephemeris whose expiration has passed reports \
         {EPHEMERIS_EXPIRED}, got {after}"
    );

    // And nothing but the status moved with it.
    let mut before_expiry = before.clone();
    let mut after_expiry = after.clone();
    for body in [&mut before_expiry, &mut after_expiry] {
        body.as_object_mut()
            .expect("a described ephemeris is an object")
            .remove("status");
    }
    assert_eq!(
        before_expiry, after_expiry,
        "case after the expiration: only the status moves when an expiration falls due"
    );

    // The verdict holds: reading again reports the same ephemeris.
    assert_eq!(
        describe_ok(&router, "after the expiration, read again", &id).await,
        after,
        "case after the expiration: an expired ephemeris must not move again on the next read"
    );
}
