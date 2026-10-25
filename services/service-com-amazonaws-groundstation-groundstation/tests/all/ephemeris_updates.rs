// SPDX-License-Identifier: CC0-1.0

//! `UpdateEphemeris` (`PUT /ephemeris/{ephemerisId}`) end to end over HTTP, through the
//! same front door as the rest of this target's suites, with every write read back through
//! `DescribeEphemeris` (`GET /ephemeris/{ephemerisId}`).
//!
//! The RULE under test: `UpdateEphemeris` REPLACES, on the ephemeris this account holds
//! under the `{ephemerisId}` it is addressed by, the members the request carries — the
//! required `enabled`, plus `name` and `priority` when present — and leaves every other
//! member of that ephemeris, and every other ephemeris, exactly as it was. Because
//! enabling or disabling needs no re-validation, an ephemeris whose data has already
//! validated takes the status its new flag selects at once (`ENABLED` / `DISABLED`), one
//! still `VALIDATING` lands on the flag as it stands when the move it is owed falls due,
//! and one `INVALID` or `EXPIRED` keeps the status its data earned; an id that names no
//! ephemeris, and a priority below the documented minimum, are refused and write nothing.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning which members an update carries and what it carries in them — the
//!   flag in both directions, a new name, a priority at the documented minimum and at the
//!   published maximum of `EphemerisPriority`, each member omitted, and all of them at
//!   once — each row read back member by member against what the create stored;
//! * the status an update lands the ephemeris on, over a table spanning every
//!   `EphemerisStatus` this service puts a customer ephemeris in, in both directions of the
//!   flag, with `invalidReason` asserted present only where that status is `INVALID`;
//! * the same update applied twice, which the model declares idempotent, beside a second
//!   ephemeris that no update addressed and which must not move;
//! * the refusals: several well-formed `Uuid`s that name no ephemeris, an id outside the
//!   modelled `Uuid` pattern, a priority of 0 (inside the modelled range, outside the
//!   documented one), a priority past the modelled range and a request carrying no
//!   `enabled` at all — none of which disturbs the store.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::ops::update_ephemeris::{
    PRIORITY_BELOW_MINIMUM, PRIORITY_PARAMETER,
};
use service_com_amazonaws_groundstation_groundstation::state::{
    BAD_REQUEST_ERROR_CODE, EPHEMERIS_DISABLED, EPHEMERIS_ENABLED, EPHEMERIS_EXPIRED,
    EPHEMERIS_INVALID, EPHEMERIS_NOT_FOUND, EPHEMERIS_TIME_RANGE_INVALID, EPHEMERIS_VALIDATING,
    INVALID_REQUEST_BODY, MIN_EPHEMERIS_PRIORITY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{create_ok, onboard, tle_inline, wait_out_validation, FUTURE, PAST};
use crate::ephemeris_description::describe_ok;
use crate::scheduling_loop::{harness_with_engine, send, Reply, ABSENT_UUID};

/// The largest priority the modelled `EphemerisPriority` admits (range 0..99999 —
/// `smithy_query(query="shape", shape="EphemerisPriority")`).
const MAX_EPHEMERIS_PRIORITY: i32 = 99_999;

/// A priority the create stores, so a row that omits `priority` has a value to keep that is
/// neither the documented default nor either bound.
const CREATED_PRIORITY: i32 = 50;

/// A validity range that runs forwards, so the data it carries validates.
const VALID_RANGE: (i64, i64) = (1_800_000_000, 1_800_086_400);

/// A validity range that runs backwards, so the data it carries describes no usable
/// interval and the ephemeris is owed `INVALID`.
const BACKWARDS_RANGE: (i64, i64) = (1_800_086_400, 1_800_000_000);

/// `UpdateEphemeris` for `ephemeris_id`, with `body`.
async fn update(router: &MultiServiceRouter, ephemeris_id: &str, body: Value) -> Reply {
    send(
        router,
        "PUT",
        &format!("/ephemeris/{ephemeris_id}"),
        Some(body),
    )
    .await
}

/// `UpdateEphemeris` for `ephemeris_id` with `body`, asserting it was answered and that it
/// answered the id it was addressed by.
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

/// An `UpdateEphemeris` body carrying the required flag plus whichever optional members the
/// row names.
fn update_body(enabled: bool, name: Option<&str>, priority: Option<i32>) -> Value {
    let mut body = json!({"enabled": enabled});
    if let Some(name) = name {
        body["name"] = json!(name);
    }
    if let Some(priority) = priority {
        body["priority"] = json!(priority);
    }
    body
}

/// A `CreateEphemeris` body for an ephemeris of `satellite_id`, named `name`, expiring at
/// `expiration`, carrying data over `ranges`, and asked to be enabled or not.
fn create_body(
    name: &str,
    satellite_id: &str,
    enabled: bool,
    expiration: i64,
    ranges: &[(i64, i64)],
) -> Value {
    json!({
        "name": name,
        "enabled": enabled,
        "priority": CREATED_PRIORITY,
        "expirationTime": expiration,
        "satelliteId": satellite_id,
        "ephemeris": tle_inline(ranges),
    })
}

/// A JSON member of `body`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The members of an ephemeris no member of an `UpdateEphemeris` request addresses, so one
/// an update must report unchanged.
const UNTOUCHED_MEMBERS: &[&str] = &[
    "ephemerisId",
    "satelliteId",
    "creationTime",
    "suppliedData",
];

/// One row of the member table: which optional members the update carries, and the `name`,
/// `priority` and `enabled` the ephemeris must report afterwards.
struct MemberRow {
    case: &'static str,
    created_enabled: bool,
    update_enabled: bool,
    update_name: Option<&'static str>,
    update_priority: Option<i32>,
    expected_name: &'static str,
    expected_priority: i32,
    expected_status: &'static str,
}

/// The name every row's create stores, so a row that omits `name` has one to keep.
const CREATED_NAME: &str = "created ephemeris";

/// Which members an update may carry, and what each combination leaves on the ephemeris.
///
/// The rows differ along the flag (both directions), whether a new name is carried, and the
/// priority carried: the documented minimum, the published maximum of `EphemerisPriority`,
/// a value between them, and the member omitted.
fn member_rows() -> Vec<MemberRow> {
    vec![
        MemberRow {
            case: "enable a disabled ephemeris, carrying nothing else",
            created_enabled: false,
            update_enabled: true,
            update_name: None,
            update_priority: None,
            expected_name: CREATED_NAME,
            expected_priority: CREATED_PRIORITY,
            expected_status: EPHEMERIS_ENABLED,
        },
        MemberRow {
            case: "disable an enabled ephemeris, carrying nothing else",
            created_enabled: true,
            update_enabled: false,
            update_name: None,
            update_priority: None,
            expected_name: CREATED_NAME,
            expected_priority: CREATED_PRIORITY,
            expected_status: EPHEMERIS_DISABLED,
        },
        MemberRow {
            case: "keep the flag and rename",
            created_enabled: true,
            update_enabled: true,
            update_name: Some("renamed ephemeris"),
            update_priority: None,
            expected_name: "renamed ephemeris",
            expected_priority: CREATED_PRIORITY,
            expected_status: EPHEMERIS_ENABLED,
        },
        MemberRow {
            case: "priority at the documented minimum",
            created_enabled: true,
            update_enabled: true,
            update_name: None,
            update_priority: Some(MIN_EPHEMERIS_PRIORITY),
            expected_name: CREATED_NAME,
            expected_priority: MIN_EPHEMERIS_PRIORITY,
            expected_status: EPHEMERIS_ENABLED,
        },
        MemberRow {
            case: "priority at the published maximum",
            created_enabled: false,
            update_enabled: false,
            update_name: None,
            update_priority: Some(MAX_EPHEMERIS_PRIORITY),
            expected_name: CREATED_NAME,
            expected_priority: MAX_EPHEMERIS_PRIORITY,
            expected_status: EPHEMERIS_DISABLED,
        },
        MemberRow {
            case: "every member at once",
            created_enabled: false,
            update_enabled: true,
            update_name: Some("re-pointed and re-prioritised"),
            update_priority: Some(7),
            expected_name: "re-pointed and re-prioritised",
            expected_priority: 7,
            expected_status: EPHEMERIS_ENABLED,
        },
        MemberRow {
            case: "a name of one character, the shortest the model admits",
            created_enabled: true,
            update_enabled: false,
            update_name: Some("a"),
            update_priority: Some(CREATED_PRIORITY),
            expected_name: "a",
            expected_priority: CREATED_PRIORITY,
            expected_status: EPHEMERIS_DISABLED,
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_leaves_every_other_one_alone() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1");

    for row in member_rows() {
        let case = row.case;
        let ephemeris_id = create_ok(
            &router,
            case,
            create_body(
                CREATED_NAME,
                &satellites[0],
                row.created_enabled,
                FUTURE,
                &[VALID_RANGE],
            ),
        )
        .await;
        // The create's validation has to have landed before the update, so the status the
        // update moves is the one the flag selects rather than the one still owed.
        wait_out_validation();
        let before = describe_ok(&router, case, &ephemeris_id).await;

        update_ok(
            &router,
            case,
            &ephemeris_id,
            update_body(row.update_enabled, row.update_name, row.update_priority),
        )
        .await;

        let after = describe_ok(&router, case, &ephemeris_id).await;
        assert_eq!(
            text(&after, "name"),
            row.expected_name,
            "case {case}: wrong name after the update, got {after}"
        );
        assert_eq!(
            after.get("priority").and_then(Value::as_i64),
            Some(i64::from(row.expected_priority)),
            "case {case}: wrong priority after the update, got {after}"
        );
        assert_eq!(
            after.get("enabled").and_then(Value::as_bool),
            Some(row.update_enabled),
            "case {case}: wrong enabled flag after the update, got {after}"
        );
        assert_eq!(
            text(&after, "status"),
            row.expected_status,
            "case {case}: wrong status after the update, got {after}"
        );
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: no member of this request addresses {member}, so it must not \
                 move: before {before}, after {after}"
            );
        }
    }
}

/// One row of the status table: the ephemeris a create stores, whether its validation is
/// waited out before the update, the flag the update carries, and the `EphemerisStatus` the
/// ephemeris must report afterwards.
struct StatusRow {
    case: &'static str,
    created_enabled: bool,
    expiration: i64,
    ranges: &'static [(i64, i64)],
    settle_before_update: bool,
    update_enabled: bool,
    expected_status: &'static str,
}

/// Every `EphemerisStatus` this service puts a customer ephemeris in, each met by an update
/// of the flag in both directions.
///
/// `VALIDATING` is the status straight after a create, before the move the service owes
/// falls due; `ENABLED` and `DISABLED` are the two that flag selects once the data has
/// validated; `INVALID` belongs to data that describes no usable validity range, and
/// `EXPIRED` to an ephemeris whose expiration has passed. Neither of the last two is a
/// status a flag can move, since neither re-validates nor un-expires the data.
fn status_rows() -> Vec<StatusRow> {
    vec![
        StatusRow {
            case: "still validating, asked to be enabled",
            created_enabled: false,
            expiration: FUTURE,
            ranges: &[VALID_RANGE],
            settle_before_update: false,
            update_enabled: true,
            expected_status: EPHEMERIS_VALIDATING,
        },
        StatusRow {
            case: "still validating, asked to be disabled",
            created_enabled: true,
            expiration: FUTURE,
            ranges: &[VALID_RANGE],
            settle_before_update: false,
            update_enabled: false,
            expected_status: EPHEMERIS_VALIDATING,
        },
        StatusRow {
            case: "enabled, asked to be disabled",
            created_enabled: true,
            expiration: FUTURE,
            ranges: &[VALID_RANGE],
            settle_before_update: true,
            update_enabled: false,
            expected_status: EPHEMERIS_DISABLED,
        },
        StatusRow {
            case: "disabled, asked to be enabled",
            created_enabled: false,
            expiration: FUTURE,
            ranges: &[VALID_RANGE],
            settle_before_update: true,
            update_enabled: true,
            expected_status: EPHEMERIS_ENABLED,
        },
        StatusRow {
            case: "enabled, asked to be enabled again",
            created_enabled: true,
            expiration: FUTURE,
            ranges: &[VALID_RANGE],
            settle_before_update: true,
            update_enabled: true,
            expected_status: EPHEMERIS_ENABLED,
        },
        StatusRow {
            case: "invalid data, asked to be enabled",
            created_enabled: false,
            expiration: FUTURE,
            ranges: &[BACKWARDS_RANGE],
            settle_before_update: true,
            update_enabled: true,
            expected_status: EPHEMERIS_INVALID,
        },
        StatusRow {
            case: "expired, asked to be enabled",
            created_enabled: false,
            expiration: PAST,
            ranges: &[VALID_RANGE],
            settle_before_update: true,
            update_enabled: true,
            expected_status: EPHEMERIS_EXPIRED,
        },
        StatusRow {
            case: "expired, asked to be disabled",
            created_enabled: true,
            expiration: PAST,
            ranges: &[VALID_RANGE],
            settle_before_update: true,
            update_enabled: false,
            expected_status: EPHEMERIS_EXPIRED,
        },
    ]
}

#[tokio::test]
async fn the_flag_an_update_carries_moves_the_status_without_re_validating_the_data() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1");

    for row in status_rows() {
        let case = row.case;
        let ephemeris_id = create_ok(
            &router,
            case,
            create_body(
                CREATED_NAME,
                &satellites[0],
                row.created_enabled,
                row.expiration,
                row.ranges,
            ),
        )
        .await;
        if row.settle_before_update {
            wait_out_validation();
        }

        update_ok(
            &router,
            case,
            &ephemeris_id,
            update_body(row.update_enabled, None, None),
        )
        .await;

        let after = describe_ok(&router, case, &ephemeris_id).await;
        assert_eq!(
            text(&after, "status"),
            row.expected_status,
            "case {case}: wrong status straight after the update, got {after}"
        );
        assert_eq!(
            after.get("enabled").and_then(Value::as_bool),
            Some(row.update_enabled),
            "case {case}: the flag the update carried must be the one reported, got {after}"
        );
        // "Appears only when the status is INVALID" — so a row in any other status names no
        // reason, and the one that did not validate names the verdict its data earned.
        let reason = after.get("invalidReason").and_then(Value::as_str);
        let expected_reason = (row.expected_status == EPHEMERIS_INVALID)
            .then_some(EPHEMERIS_TIME_RANGE_INVALID);
        assert_eq!(
            reason, expected_reason,
            "case {case}: wrong invalidReason after the update, got {after}"
        );

        // An ephemeris the update left `VALIDATING` is still owed the move out of it, and
        // lands on the flag the update carried rather than the one the create did.
        if !row.settle_before_update {
            wait_out_validation();
            let settled = describe_ok(&router, case, &ephemeris_id).await;
            let owed = if row.update_enabled {
                EPHEMERIS_ENABLED
            } else {
                EPHEMERIS_DISABLED
            };
            assert_eq!(
                text(&settled, "status"),
                owed,
                "case {case}: the owed move must land on the flag the update carried, got \
                 {settled}"
            );
        }
    }
}

#[tokio::test]
async fn the_same_update_applied_twice_lands_the_same_ephemeris_and_moves_no_other() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1");
    let updated = create_ok(
        &router,
        "updated",
        create_body(CREATED_NAME, &satellites[0], false, FUTURE, &[VALID_RANGE]),
    )
    .await;
    let bystander = create_ok(
        &router,
        "bystander",
        create_body("bystander", &satellites[0], true, FUTURE, &[VALID_RANGE]),
    )
    .await;
    wait_out_validation();
    let bystander_before = describe_ok(&router, "bystander", &bystander).await;

    let body = update_body(true, Some("twice over"), Some(42));
    update_ok(&router, "first apply", &updated, body.clone()).await;
    let once = describe_ok(&router, "first apply", &updated).await;
    update_ok(&router, "second apply", &updated, body).await;
    let twice = describe_ok(&router, "second apply", &updated).await;

    assert_eq!(
        once, twice,
        "case idempotence: applying the same update twice must leave the same ephemeris, got \
         {once} then {twice}"
    );
    assert_eq!(
        text(&twice, "name"),
        "twice over",
        "case idempotence: wrong name, got {twice}"
    );
    assert_eq!(
        twice.get("priority").and_then(Value::as_i64),
        Some(42),
        "case idempotence: wrong priority, got {twice}"
    );

    let bystander_after = describe_ok(&router, "bystander", &bystander).await;
    assert_eq!(
        bystander_before, bystander_after,
        "case bystander: an ephemeris no update addressed must not move: before \
         {bystander_before}, after {bystander_after}"
    );
}

#[tokio::test]
async fn an_update_this_service_refuses_writes_nothing() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1");
    let kept = create_ok(
        &router,
        "kept",
        create_body(CREATED_NAME, &satellites[0], true, FUTURE, &[VALID_RANGE]),
    )
    .await;
    wait_out_validation();
    let before = describe_ok(&router, "kept", &kept).await;

    // Well-formed `Uuid`s that name no ephemeris: the id is a missing resource, answered
    // with this service's ephemeris literal.
    for absent in [
        ABSENT_UUID,
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "0123456f-89ab-cdef-0123-456789abcdef",
    ] {
        let refused = update(&router, absent, update_body(true, Some("ghost"), Some(3))).await;
        assert_eq!(
            refused.status, 434,
            "case {absent}: expected 434 for an id that names no ephemeris, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {absent}: wrong error shape, got {}",
            refused.body
        );
        assert_eq!(
            refused.string("message"),
            EPHEMERIS_NOT_FOUND,
            "case {absent}: wrong not-found text, got {}",
            refused.body
        );
    }

    // A priority of 0 is inside the modelled `EphemerisPriority` range and outside the
    // member's documented one ("Must be 1 or greater."), so the handler refuses it as a bad
    // parameter and names it.
    for priority in [0, MIN_EPHEMERIS_PRIORITY - 1] {
        let case = format!("priority {priority}");
        let refused = update(&router, &kept, update_body(false, None, Some(priority))).await;
        assert_eq!(
            refused.status, 431,
            "case {case}: expected 431 for a priority below the documented minimum, got {} {}",
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
            PRIORITY_BELOW_MINIMUM,
            "case {case}: wrong refusal text, got {}",
            refused.body
        );
        assert_eq!(
            refused.string("parameterName"),
            PRIORITY_PARAMETER,
            "case {case}: a refused parameter must be named, got {}",
            refused.body
        );
    }

    // What the model itself rules out never reaches the handler: this service's validator
    // answers it, as a bad request rather than a bad resource.
    let malformed: &[(&str, &str, Value)] = &[
        (
            "a priority past the modelled maximum",
            &kept,
            update_body(true, None, Some(MAX_EPHEMERIS_PRIORITY + 1)),
        ),
        ("no enabled member at all", &kept, json!({"name": "flagless"})),
        (
            "a name outside the modelled SafeName pattern",
            &kept,
            update_body(true, Some("not/a/safe/name"), None),
        ),
        ("an id that is not hexadecimal at all", "nope", update_body(true, None, None)),
        (
            "an id one character short",
            "0000000-0000-0000-0000-000000000000",
            update_body(true, None, None),
        ),
        (
            "an id in upper-case hex",
            "0A1B2C3D-4E5F-4A6B-8C7D-9E0F1A2B3C4D",
            update_body(true, None, None),
        ),
    ];
    for (case, ephemeris_id, body) in malformed {
        let refused = update(&router, ephemeris_id, body.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400 for a request the model rules out, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            refused.body
        );
        assert_eq!(
            refused.string("message"),
            INVALID_REQUEST_BODY,
            "case {case}: wrong refusal text, got {}",
            refused.body
        );
    }

    let after = describe_ok(&router, "kept", &kept).await;
    assert_eq!(
        before, after,
        "case refusals: a refused update must write nothing: before {before}, after {after}"
    );
}
