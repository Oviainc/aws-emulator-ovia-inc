// SPDX-License-Identifier: CC0-1.0

//! Two rules of `UpgradeAccountPlan`, driven end to end over the wire
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSFreeTierService.<Operation>`)
//! through the same multi-service router the emulator boots.
//!
//! 1. The move onto the PAID plan is conditional on the ACCOUNT: an account
//!    whose payment instrument has not been vetted is refused with the modelled
//!    `ValidationException` (HTTP 400) stating the service's fixed sentence for
//!    that refusal, and the plan record it holds is left exactly as it was — for
//!    an account that holds none, no record is established at all. Because the
//!    condition is a property of the account and not of the call, the refusal is
//!    idempotent: repeating it answers identically and still stores nothing. An
//!    account whose instrument IS vetted is moved onto the plan, and the next
//!    read reports it.
//! 2. A value outside the modelled `AccountPlanType` enum is refused naming the
//!    set that enum admits, in the order the SERVICE states it in
//!    (`crate::state::ACCOUNT_PLAN_TYPE_VALUE_SET`) — which is not the order the
//!    model declares its members in — with the submitted value as the clause's
//!    only variable part.
//!
//! Grounding: `UpgradeAccountPlanRequest$accountPlanType` is `@required` and
//! targets the enum `AccountPlanType`, which declares `FREE, PAID`
//! (`smithy_query(query="shape",shape="AccountPlanType")`), and the operation
//! models `ValidationException` at HTTP 400
//! (`smithy_query(query="operation",operation="UpgradeAccountPlan")`).
//!
//! The rows of rule 1 differ along the plan record the account holds when the
//! call arrives (none, a free plan inside its window, a free plan past it, the
//! paid plan) and along the number of attempts; the rows of rule 2 differ along
//! HOW the submitted value misses the set. Both value sets are first pinned to
//! be permutations of what the model declares, so a published order can reorder
//! a set and never change what it holds.

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::ACCOUNT_PLAN_TYPE_VALUES;
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    stated_enum_value_set, ComAmazonawsFreetierAwsfreetierserviceState,
    ACCOUNT_PLAN_TYPE_VALUE_SET, PAYMENT_INSTRUMENT_VET_FAILURE, PLAN_STATUS_ACTIVE,
    PLAN_TYPE_FREE, PLAN_TYPE_PAID,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{call, error_code, harness, ok, text};

/// Seconds in a day, for the offsets below.
const DAY: i64 = 86_400;

/// The `AccountPlanType` value set as the SERVICE states it when it refuses a
/// value outside the enum, written out here rather than read off the service so
/// the expectation and the implementation do not share one source. The
/// permutation check below ties it to the set the model declares.
const PUBLISHED_VALUE_SET: &str = "PAID, FREE";

/// The whole `message` of the refusal of a paid upgrade an account's payment
/// instrument does not back. Fixed text with no variable part, written out here
/// for the same reason as [`PUBLISHED_VALUE_SET`].
const VET_REFUSAL: &str = "Account plan upgrade failed PI vet failure";

/// The plan record an account holds when the call under test arrives: its type
/// and where its expiration instant sits relative to now, or `None` for an
/// account the store holds no plan record for at all.
type StoredPlan = Option<(&'static str, Option<i64>)>;

/// The service mounted over a store holding exactly `stored`, with the account's
/// payment instrument vetted when `vetted`.
fn mounted(stored: StoredPlan, vetted: bool) -> MultiServiceRouter {
    let (router, engine) = harness();
    let account_id = engine::emulator_account_id();
    {
        let state = engine.state::<ComAmazonawsFreetierAwsfreetierserviceState>();
        let mut store = state.lock();
        if let Some((plan_type, expiry_offset)) = stored {
            let now = DateTime::from(std::time::SystemTime::now()).secs();
            let plan = store.plan_mut(account_id);
            plan.plan_type = plan_type.to_string();
            plan.plan_status = PLAN_STATUS_ACTIVE.to_string();
            plan.expiration_date = expiry_offset.map(|offset| DateTime::from_secs(now + offset));
        }
        if vetted {
            store.vet_payment_instrument(account_id);
        }
    }
    router
}

/// What `GetAccountPlanState` answers right now: its status and body. Taken
/// through the wire rather than off the store, so a refused call that touched
/// the record would show up here whatever it wrote.
async fn snapshot(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GetAccountPlanState", &json!({})).await
}

/// RULE 1: without a vetted payment instrument the paid upgrade is refused, in
/// the service's fixed words, leaving the plan record untouched — however often
/// it is attempted and whatever the account already holds.
#[tokio::test]
async fn the_paid_plan_is_refused_to_an_account_with_no_vetted_payment_instrument() {
    assert_eq!(
        PAYMENT_INSTRUMENT_VET_FAILURE, VET_REFUSAL,
        "the service must state this refusal in the words this suite names, got {:?}",
        PAYMENT_INSTRUMENT_VET_FAILURE
    );
    let rows: [(&str, StoredPlan); 4] = [
        ("an account that holds no plan record", None),
        (
            "a free plan inside its window",
            Some((PLAN_TYPE_FREE, Some(DAY))),
        ),
        (
            "a free plan past its expiration instant",
            Some((PLAN_TYPE_FREE, Some(-DAY))),
        ),
        ("the paid plan already stored", Some((PLAN_TYPE_PAID, None))),
    ];

    for (case, stored) in rows {
        let router = mounted(stored, false);
        let before = snapshot(&router).await;

        // Four attempts: the refusal is a property of the account, so every one
        // of them answers alike and none of them advances anything.
        for attempt in 1..=4 {
            let (status, payload) = call(
                &router,
                "UpgradeAccountPlan",
                &json!({"accountPlanType": PLAN_TYPE_PAID}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} attempt {attempt}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?} attempt {attempt}: expected the modelled ValidationException, \
                 got {payload}"
            );
            assert_eq!(
                text(&payload, "message"),
                VET_REFUSAL,
                "case {case:?} attempt {attempt}: expected the service's fixed vetting \
                 sentence, got {payload}"
            );

            let after = snapshot(&router).await;
            assert_eq!(
                after, before,
                "case {case:?} attempt {attempt}: a refused upgrade must leave the plan record \
                 exactly as it was, got {after:?} after {before:?}"
            );
        }
    }
}

/// RULE 1, its other branch: an account whose payment instrument IS vetted is
/// moved onto the paid plan, and the following read reports what was stored —
/// so the refusal above is the vetting gate and not a paid plan the emulator
/// cannot reach.
#[tokio::test]
async fn a_vetted_account_is_moved_onto_the_paid_plan_and_reads_it_back() {
    let rows: [(&str, StoredPlan); 4] = [
        ("an account that holds no plan record", None),
        (
            "a free plan inside its window",
            Some((PLAN_TYPE_FREE, Some(DAY))),
        ),
        (
            "a free plan past its expiration instant",
            Some((PLAN_TYPE_FREE, Some(-DAY))),
        ),
        ("the paid plan already stored", Some((PLAN_TYPE_PAID, None))),
    ];

    for (case, stored) in rows {
        let router = mounted(stored, true);
        let upgraded = ok(
            &router,
            "UpgradeAccountPlan",
            &json!({"accountPlanType": PLAN_TYPE_PAID}),
        )
        .await;
        assert_eq!(
            text(&upgraded, "accountPlanType"),
            PLAN_TYPE_PAID,
            "case {case:?}: a vetted account reaches the paid plan, got {upgraded}"
        );
        assert_eq!(
            text(&upgraded, "accountPlanStatus"),
            PLAN_STATUS_ACTIVE,
            "case {case:?}: the plan it was moved onto is in force, got {upgraded}"
        );

        // Round trip: the write is reported by the service's own reader.
        let reported = ok(&router, "GetAccountPlanState", &json!({})).await;
        for member in ["accountId", "accountPlanType", "accountPlanStatus"] {
            assert_eq!(
                text(&reported, member),
                text(&upgraded, member),
                "case {case:?}: the upgrade and the following read must agree on {member}, \
                 got {reported} after {upgraded}"
            );
        }
    }
}

/// RULE 2: the `accountPlanType` value set is stated in the order the service
/// states that enum in, with the submitted value as the clause's only variable
/// part — and the rejected call stores nothing.
#[tokio::test]
async fn an_account_plan_type_outside_the_enum_names_the_published_value_set() {
    // The published order holds exactly the values the model declares, so it can
    // only reorder the set.
    let mut sorted_published = ACCOUNT_PLAN_TYPE_VALUE_SET.to_vec();
    sorted_published.sort_unstable();
    let mut sorted_declared = ACCOUNT_PLAN_TYPE_VALUES.to_vec();
    sorted_declared.sort_unstable();
    assert_eq!(
        sorted_published, sorted_declared,
        "the published order must hold exactly the values the model declares, got \
         {ACCOUNT_PLAN_TYPE_VALUE_SET:?} against {ACCOUNT_PLAN_TYPE_VALUES:?}"
    );

    // The rule reaches the refusal through one lookup, keyed by the value SET:
    // an enum this service publishes no order for is stated as presented.
    let presented: Vec<String> = ACCOUNT_PLAN_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    assert_eq!(
        stated_enum_value_set(&presented),
        ACCOUNT_PLAN_TYPE_VALUE_SET.to_vec(),
        "the model's order must be restated in the published one, got {:?}",
        stated_enum_value_set(&presented)
    );

    let value_set = ACCOUNT_PLAN_TYPE_VALUE_SET.join(", ");
    assert_eq!(
        value_set, PUBLISHED_VALUE_SET,
        "the service must state the AccountPlanType set in the order this suite names, \
         got {value_set:?}"
    );
    let router = mounted(None, false);

    // Rows differ in HOW the submitted value misses the set: casing, padding, an
    // unknown word, a value of another of this service's enums, and the empty
    // string.
    for submitted in [
        "GOLD", "free", "paid", "Paid", "PAID ", " FREE", "", "NOT_STARTED", "FREE,PAID",
    ] {
        assert!(
            !ACCOUNT_PLAN_TYPE_VALUES.contains(&submitted),
            "case {submitted:?}: the probe must not be a modelled AccountPlanType"
        );
        let before = snapshot(&router).await;
        let (status, payload) = call(
            &router,
            "UpgradeAccountPlan",
            &json!({"accountPlanType": submitted}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {submitted:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {submitted:?}: expected the modelled ValidationException, got {payload}"
        );
        let expected = format!(
            "1 validation error detected: Value '{submitted}' at 'accountPlanType' failed to \
             satisfy constraint: Member must satisfy enum value set: [{value_set}]"
        );
        let actual = text(&payload, "message");
        assert_eq!(
            actual, expected,
            "case {submitted:?}: expected {expected:?}, got {actual:?}"
        );

        let after = snapshot(&router).await;
        assert_eq!(
            after, before,
            "case {submitted:?}: a refused request must store no plan, got {after:?} \
             after {before:?}"
        );
    }
}
