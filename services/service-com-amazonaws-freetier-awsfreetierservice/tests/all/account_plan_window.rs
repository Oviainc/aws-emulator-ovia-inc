// SPDX-License-Identifier: CC0-1.0

//! The free plan's one-off window, and the agreement between what
//! `UpgradeAccountPlan` reports and what the next `GetAccountPlanState` reports.
//!
//! RULE: a plan-writing call leaves the stored plan in the state its own
//! expiration instant implies at the instant of the write — so the status it
//! reports is the status the next read of the same record reports — and the free
//! plan is a one-off, time-boxed window, so an account whose window is no longer
//! open (it left the free plan, or the window's instant has passed) is refused
//! with the modelled `ValidationException` (HTTP 400) rather than moved back
//! onto it.
//!
//! Grounding: `UpgradeAccountPlanResponse$accountPlanStatus` is `@required` and
//! `AccountPlanStatus` declares `EXPIRED`
//! (`smithy_query(query="shape",shape="AccountPlanStatus")`), while
//! `GetAccountPlanStateResponse$accountPlanExpirationDate` is "the timestamp for
//! when the current account plan expires" — so past that instant the plan a
//! response describes is `EXPIRED`, whichever operation describes it.
//!
//! The rows differ along what the rule generalises over: which plan the store
//! holds, where its expiration instant sits relative to now (both sides of the
//! boundary and on it), and which of the two modelled target types the call asks
//! for. The harness is the one `free_tier.rs` builds.

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    ComAmazonawsFreetierAwsfreetierserviceState, PLAN_STATUS_ACTIVE, PLAN_STATUS_EXPIRED,
    PLAN_TYPE_FREE, PLAN_TYPE_PAID,
};

use crate::free_tier::{harness, ok, refused, text};

/// Seconds in a day, for the offsets below.
const DAY: i64 = 86_400;

/// One row of the table: the plan the store holds, the target the call asks for,
/// and whether the service accepts the call.
struct Row {
    case: &'static str,
    /// The `AccountPlanType` the store holds for the account.
    stored_type: &'static str,
    /// Where the stored expiration instant sits relative to now, in seconds;
    /// `None` is a plan with no expiration instant at all, which is what a paid
    /// plan carries.
    stored_expiry_offset: Option<i64>,
    /// The `accountPlanType` the request asks for.
    target: &'static str,
    /// Whether `UpgradeAccountPlan` accepts the request.
    accepted: bool,
}

/// The service mounted over a store holding exactly the plan a row describes:
/// the router, the engine that store lives on (so a test can age the plan the
/// way the clock would) and the stored expiration instant.
struct Mounted {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: std::sync::Arc<engine::InMemoryEngine>,
    expires_at: Option<DateTime>,
}

/// Mount the service over a store holding exactly the plan `row` describes.
fn store_plan(row: &Row) -> Mounted {
    let (router, engine) = harness();
    let now = DateTime::from(std::time::SystemTime::now()).secs();
    let expires_at = row
        .stored_expiry_offset
        .map(|offset| DateTime::from_secs(now + offset));
    {
        let state = engine.state::<ComAmazonawsFreetierAwsfreetierserviceState>();
        let mut store = state.lock();
        // This suite is about the free plan's one-off window, so its account
        // carries a vetted payment instrument: the paid plan's own precondition
        // is pinned in `paid_plan_vetting.rs` and is not what these rows
        // exercise.
        store.vet_payment_instrument(engine::emulator_account_id());
        let plan = store.plan_mut(engine::emulator_account_id());
        plan.plan_type = row.stored_type.to_string();
        // Stored ACTIVE on purpose: the status the service reports has to follow
        // the expiration instant, not whatever the record was last left at.
        plan.plan_status = PLAN_STATUS_ACTIVE.to_string();
        plan.expiration_date = expires_at;
    }
    Mounted {
        router,
        engine,
        expires_at,
    }
}

/// Move the stored plan's expiration instant `seconds` into the past, which is
/// what the clock does to a window that elapses.
fn age_past_expiry(mounted: &Mounted, seconds: i64) {
    let state = mounted
        .engine
        .state::<ComAmazonawsFreetierAwsfreetierserviceState>();
    let mut store = state.lock();
    let plan = store.plan_mut(engine::emulator_account_id());
    let now = DateTime::from(std::time::SystemTime::now()).secs();
    plan.expiration_date = Some(DateTime::from_secs(now - seconds));
}

/// RULE over the table: which `UpgradeAccountPlan` calls the one-off free window
/// allows, and that an accepted call's response is the plan the following read
/// reports.
#[tokio::test]
async fn the_free_window_is_entered_only_while_it_is_open() {
    let rows: [Row; 10] = [
        Row {
            case: "a free plan a day past its expiry, asked for FREE again",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(-DAY),
            target: PLAN_TYPE_FREE,
            accepted: false,
        },
        Row {
            case: "a free plan one second past its expiry, asked for FREE",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(-1),
            target: PLAN_TYPE_FREE,
            accepted: false,
        },
        Row {
            case: "a free plan expiring at this very instant, asked for FREE",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(0),
            target: PLAN_TYPE_FREE,
            accepted: false,
        },
        Row {
            case: "a free plan years past its expiry, asked for FREE",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(-3_650 * DAY),
            target: PLAN_TYPE_FREE,
            accepted: false,
        },
        Row {
            case: "a free plan with an hour of its window left, asked for FREE",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(3_600),
            target: PLAN_TYPE_FREE,
            accepted: true,
        },
        Row {
            case: "a free plan with a day of its window left, asked for FREE",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(DAY),
            target: PLAN_TYPE_FREE,
            accepted: true,
        },
        Row {
            case: "a free plan past its expiry, upgraded to PAID",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(-DAY),
            target: PLAN_TYPE_PAID,
            accepted: true,
        },
        Row {
            case: "a free plan inside its window, upgraded to PAID",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(3_600),
            target: PLAN_TYPE_PAID,
            accepted: true,
        },
        Row {
            case: "a paid plan, asked for FREE",
            stored_type: PLAN_TYPE_PAID,
            stored_expiry_offset: None,
            target: PLAN_TYPE_FREE,
            accepted: false,
        },
        Row {
            case: "a paid plan, asked for PAID",
            stored_type: PLAN_TYPE_PAID,
            stored_expiry_offset: None,
            target: PLAN_TYPE_PAID,
            accepted: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let Mounted {
            router,
            engine: _engine,
            expires_at,
        } = store_plan(row);
        let before = ok(&router, "GetAccountPlanState", &json!({})).await;
        let request = json!({"accountPlanType": row.target});

        if !row.accepted {
            refused(
                &router,
                "UpgradeAccountPlan",
                &request,
                400,
                "ValidationException",
            )
            .await;
            let after = ok(&router, "GetAccountPlanState", &json!({})).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused upgrade must leave the plan exactly as it was, \
                 got {after} after {before}"
            );
            assert_eq!(
                text(&after, "accountPlanType"),
                row.stored_type,
                "case {case:?}: the account stays on the plan it held, got {after}"
            );
            continue;
        }

        let upgraded = ok(&router, "UpgradeAccountPlan", &request).await;
        assert_eq!(
            text(&upgraded, "accountPlanType"),
            row.target,
            "case {case:?}: expected the plan asked for, got {upgraded}"
        );
        assert_eq!(
            text(&upgraded, "accountPlanStatus"),
            PLAN_STATUS_ACTIVE,
            "case {case:?}: an accepted upgrade lands on a plan that is in force, got {upgraded}"
        );

        // The response and the next read describe the same stored plan at the
        // same instant, so they must agree member for member.
        let after = ok(&router, "GetAccountPlanState", &json!({})).await;
        for member in ["accountId", "accountPlanType", "accountPlanStatus"] {
            assert_eq!(
                text(&after, member),
                text(&upgraded, member),
                "case {case:?}: the upgrade response and the following read must agree on \
                 {member}, got {after} after {upgraded}"
            );
        }

        // A paid plan does not expire; a free plan keeps the instant its own
        // one-off window already had — asking for it again does not extend it.
        let expected_expiry = if row.target == PLAN_TYPE_PAID {
            None
        } else {
            expires_at.map(|instant| instant.secs())
        };
        assert_eq!(
            expiry_secs(&after),
            expected_expiry,
            "case {case:?}: expected the expiration instant {expected_expiry:?}, got {after}"
        );
    }
}

/// `accountPlanExpirationDate` of a reported plan, in epoch seconds, or `None`
/// when the plan reports no expiration instant.
fn expiry_secs(payload: &Value) -> Option<i64> {
    let rendered = payload["accountPlanExpirationDate"].as_str()?;
    Some(
        DateTime::from_str(rendered, aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("accountPlanExpirationDate {rendered:?} must be a date-time: {error}")
            })
            .secs(),
    )
}

/// RULE, the same one seen over time rather than over stored states: repeating
/// `UpgradeAccountPlan(FREE)` inside the window neither extends the window nor
/// changes what is reported, and once the window's instant has passed the call
/// is refused and the plan stays `EXPIRED`.
///
/// Rows differ in how much of the window is left when the repeats happen.
#[tokio::test]
async fn repeating_the_free_upgrade_neither_extends_nor_reopens_the_window() {
    // Rows differ in how much of the window is left. The smallest row still has to
    // outlast the calls this loop then makes against it — a window that elapses
    // mid-row would refuse a call the row expects to be accepted, which says nothing
    // about the rule — so the "barely open" row leaves a minute rather than a second.
    for remaining in [60_i64, 3_600, DAY, 180 * DAY] {
        let case = format!("a free plan with {remaining}s of its window left");
        let mounted = store_plan(&Row {
            case: "",
            stored_type: PLAN_TYPE_FREE,
            stored_expiry_offset: Some(remaining),
            target: PLAN_TYPE_FREE,
            accepted: true,
        });
        let router = &mounted.router;
        let request = json!({"accountPlanType": PLAN_TYPE_FREE});

        let first = ok(router, "UpgradeAccountPlan", &request).await;
        for attempt in 2..=4 {
            let again = ok(router, "UpgradeAccountPlan", &request).await;
            assert_eq!(
                again, first,
                "case {case:?}: repeat {attempt} must report the same plan, got {again} \
                 after {first}"
            );
            let read = ok(router, "GetAccountPlanState", &json!({})).await;
            assert_eq!(
                expiry_secs(&read),
                mounted.expires_at.map(|instant| instant.secs()),
                "case {case:?}: repeat {attempt} must not move the window's instant, got {read}"
            );
        }

        // The same account, once the clock has carried its window past the
        // instant the plan itself names: the window does not reopen.
        age_past_expiry(&mounted, remaining);
        refused(
            router,
            "UpgradeAccountPlan",
            &request,
            400,
            "ValidationException",
        )
        .await;
        let read = ok(router, "GetAccountPlanState", &json!({})).await;
        assert_eq!(
            text(&read, "accountPlanStatus"),
            PLAN_STATUS_EXPIRED,
            "case {case:?} aged past its instant: the plan stays expired, got {read}"
        );
        assert_eq!(
            text(&read, "accountPlanType"),
            PLAN_TYPE_FREE,
            "case {case:?} aged past its instant: the plan type is unchanged, got {read}"
        );
    }
}
