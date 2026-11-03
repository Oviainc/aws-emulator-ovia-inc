// SPDX-License-Identifier: CC0-1.0

//! `UpdateSpendingLimit`, end to end over HTTP.
//!
//! The rules these suites pin:
//!
//! * An update WRITES each member it carries to the stored limit and leaves every member
//!   it omits as it was, which `SearchSpendingLimits` reads back; `updatedAt` moves to
//!   the instant of the call that changed something, and a call that changes nothing
//!   leaves it where it was.
//! * `spendingLimitArn` is the only thing that resolves the limit: an ARN naming no
//!   stored limit is `ResourceNotFoundException` (404), and a value that is not an ARN of
//!   a spending limit is `ValidationException` (400).
//! * An amount outside the form the model publishes for it is refused as the body it is,
//!   and persists nothing.
//! * A `timePeriod` whose end does not follow its start covers no time and is refused.
//! * `clientToken` is an `@idempotencyToken`: the same token with the same request
//!   applies the change once, and with a different request it is a reuse and refused.
//!
//! Each suite asserts its rule over a TABLE spanning the class rather than one recorded
//! call: the rows differ along which members the request carries, the amount's form, the
//! period's length, and the kind of ARN the request names. Everything goes through the
//! production router, reusing the harness in `quantum_task_loop.rs`, so what is pinned is
//! what a Braket client sees.

use serde_json::{json, Value};

use crate::quantum_task_loop::{encode, member, Braket, Reply};
use crate::spending_limits::{
    a_device, create_limit, limit_body, limit_by_arn, limits_on, now_epoch, period, period_second,
    CREATED,
};

/// The region the harness's SigV4 credential scope names, so an ARN it mints carries it —
/// and so a hand-written ARN has to carry it to name anything at all.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The amount every limit in these suites is created with, so a row that changes it is
/// visibly a change.
const ORIGINAL_AMOUNT: &str = "100.50";

/// `PATCH /spending-limit/{spendingLimitArn}/update`.
async fn update_limit(braket: &Braket, arn: &str, body: Value) -> Reply {
    braket
        .call(
            "PATCH",
            &format!("/spending-limit/{}/update", encode(arn)),
            Some(body),
        )
        .await
}

/// An `UpdateSpendingLimit` body carrying `clientToken` and whichever of the two
/// modifiable members the caller names.
fn update_body(client_token: &str, amount: Option<&str>, time_period: Option<Value>) -> Value {
    let mut body = json!({ "clientToken": client_token });
    if let Some(amount) = amount {
        body["spendingLimit"] = json!(amount);
    }
    if let Some(time_period) = time_period {
        body["timePeriod"] = time_period;
    }
    body
}

/// A numeric timestamp member of a reported summary, as epoch seconds.
fn stamp(limit: &Value, name: &str) -> f64 {
    limit
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("the summary has no numeric {name}: {limit}"))
}

/// Create one limit on `device` with [`ORIGINAL_AMOUNT`], the period `start..start+3600`
/// and one tag, and return its ARN.
async fn a_limit(braket: &Braket, device: &str, token: &str, start: i64) -> String {
    let mut body = limit_body(token, device, ORIGINAL_AMOUNT);
    body["timePeriod"] = period(start, start + 3600);
    body["tags"] = json!({ "team": "quantum" });
    let reply = create_limit(braket, body).await;
    assert_eq!(
        reply.status, CREATED,
        "creating the limit to update: expected {CREATED}, got {} {}",
        reply.status, reply.body
    );
    reply.str_member("spendingLimitArn")
}

/// An update writes the members it carries and leaves the rest: the amount, the period,
/// both, or neither.
#[tokio::test]
async fn an_update_writes_each_member_it_carries_and_leaves_the_rest() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();

    // (case, new amount, new period) — the rows span which members the request carries
    // and the forms the model admits for each: the amount whole, with one and with two
    // decimals, zero and large; the period a second, an hour, a month and one that has
    // already ended.
    let rows: Vec<(&str, Option<&str>, Option<Value>)> = vec![
        ("the amount alone, whole", Some("250"), None),
        ("the amount alone, one decimal", Some("0.5"), None),
        ("the amount alone, two decimals", Some("999.99"), None),
        ("the amount alone, zero", Some("0"), None),
        ("the amount alone, large", Some("1000000"), None),
        (
            "the period alone, one second",
            None,
            Some(period(start, start + 1)),
        ),
        (
            "the period alone, a month",
            None,
            Some(period(start, start + 30 * 24 * 60 * 60)),
        ),
        (
            "the period alone, one that has already ended",
            None,
            Some(period(start - 7200, start - 3600)),
        ),
        (
            "both members at once",
            Some("42.00"),
            Some(period(start + 60, start + 120)),
        ),
        ("neither member", None, None),
    ];

    for (index, (case, amount, time_period)) in rows.iter().enumerate() {
        let arn = a_limit(&braket, &device, &format!("tok-create-{index}"), start).await;
        let before = limit_by_arn(&braket, &device, &arn).await;

        let reply = update_limit(
            &braket,
            &arn,
            update_body(
                &format!("tok-update-{index}"),
                *amount,
                time_period.clone(),
            ),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {}",
            reply.status, reply.body
        );

        let after = limit_by_arn(&braket, &device, &arn).await;
        let expected_amount = amount.unwrap_or(ORIGINAL_AMOUNT);
        assert_eq!(
            member(&after, "spendingLimit"),
            expected_amount,
            "case {case:?}: expected the amount {expected_amount}, got {after}"
        );
        let (expected_start, expected_end) = match time_period {
            Some(sent) => (
                sent["startAt"].as_f64().expect("a numeric startAt"),
                sent["endAt"].as_f64().expect("a numeric endAt"),
            ),
            None => (
                period_second(&before, "startAt"),
                period_second(&before, "endAt"),
            ),
        };
        assert!(
            (period_second(&after, "startAt") - expected_start).abs() < f64::EPSILON,
            "case {case:?}: expected startAt {expected_start}, got {}",
            period_second(&after, "startAt")
        );
        assert!(
            (period_second(&after, "endAt") - expected_end).abs() < f64::EPSILON,
            "case {case:?}: expected endAt {expected_end}, got {}",
            period_second(&after, "endAt")
        );

        // The members this request cannot write are untouched.
        assert_eq!(
            member(&after, "deviceArn"),
            device,
            "case {case:?}: an update must not move the limit to another device, got {after}"
        );
        assert_eq!(
            after.get("tags"),
            before.get("tags"),
            "case {case:?}: an update must not re-tag the limit, got {after}"
        );
        assert!(
            (stamp(&after, "createdAt") - stamp(&before, "createdAt")).abs() < f64::EPSILON,
            "case {case:?}: createdAt must not move, was {}, got {}",
            stamp(&before, "createdAt"),
            stamp(&after, "createdAt")
        );

        let changed = amount.is_some() || time_period.is_some();
        if changed {
            assert!(
                stamp(&after, "updatedAt") > stamp(&before, "updatedAt"),
                "case {case:?}: a change moves updatedAt past {}, got {}",
                stamp(&before, "updatedAt"),
                stamp(&after, "updatedAt")
            );
        } else {
            assert!(
                (stamp(&after, "updatedAt") - stamp(&before, "updatedAt")).abs() < f64::EPSILON,
                "case {case:?}: a call that changes nothing leaves updatedAt at {}, got {}",
                stamp(&before, "updatedAt"),
                stamp(&after, "updatedAt")
            );
        }
    }

    assert_eq!(
        limits_on(&braket, &device).await.len(),
        rows.len(),
        "an update creates no limit of its own"
    );
}

/// What `spendingLimitArn` resolves to decides the answer: a stored limit is updated, an
/// ARN naming none is a 404, and a value that is not an ARN of a spending limit is a 400.
#[tokio::test]
async fn the_arn_decides_which_limit_an_update_resolves() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();
    let arn = a_limit(&braket, &device, "tok-create-arn", start).await;
    let before = limit_by_arn(&braket, &device, &arn).await;

    // (case, the ARN the request names, expected status, expected error code) — a
    // well-formed spending-limit ARN naming nothing stored, the same in another region
    // and under another account, a resource field with no id (which `SpendingLimitArn`'s
    // own `@pattern` admits), an ARN of another Braket resource, and values that are no
    // ARN at all.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a well-formed ARN naming no stored limit",
            format!("arn:aws:braket:{REGION}:{ACCOUNT}:spending-limit/ghost-1"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stored limit's id in another region",
            format!("arn:aws:braket:us-east-1:{ACCOUNT}:spending-limit/ghost-2"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stored limit's id under another account",
            format!("arn:aws:braket:{REGION}:123456789012:spending-limit/ghost-3"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an empty resource id",
            format!("arn:aws:braket:{REGION}:{ACCOUNT}:spending-limit/"),
            400,
            "ValidationException",
        ),
        (
            "an ARN of another Braket resource",
            format!("arn:aws:braket:{REGION}:{ACCOUNT}:quantum-task/t1"),
            400,
            "ValidationException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:spending-limit/l1"),
            400,
            "ValidationException",
        ),
        (
            "no ARN at all",
            "not-an-arn".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (index, (case, named, status, code)) in rows.iter().enumerate() {
        let reply = update_limit(
            &braket,
            named,
            update_body(&format!("tok-arn-{index}"), Some("777"), None),
        )
        .await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some(*code),
            "case {case:?}: expected {code}, got {:?} in {}",
            reply.error,
            reply.body
        );
        if *status == 404 {
            assert_eq!(
                reply.message(),
                format!("Spending limit {named} not found"),
                "case {case:?}: the 404 names the ARN the request carried"
            );
        } else {
            assert_eq!(
                reply.message(),
                "spendingLimitArn is not valid.",
                "case {case:?}: the 400 is this operation's own sentence"
            );
        }
    }

    // None of those refusals touched the limit that does exist.
    let after = limit_by_arn(&braket, &device, &arn).await;
    assert_eq!(
        member(&after, "spendingLimit"),
        ORIGINAL_AMOUNT,
        "a refused update leaves the stored limit alone, got {after}"
    );
    assert!(
        (stamp(&after, "updatedAt") - stamp(&before, "updatedAt")).abs() < f64::EPSILON,
        "a refused update does not move updatedAt, was {}, got {}",
        stamp(&before, "updatedAt"),
        stamp(&after, "updatedAt")
    );

    // The control: the ARN the create minted does resolve.
    let served = update_limit(
        &braket,
        &arn,
        update_body("tok-arn-served", Some("777"), None),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "the ARN the create minted resolves: expected 200, got {} {}",
        served.status, served.body
    );
    assert_eq!(
        member(&limit_by_arn(&braket, &device, &arn).await, "spendingLimit"),
        "777",
        "the served update wrote the new amount"
    );
}

/// An amount outside the form the model publishes for it is refused and changes nothing;
/// every form the model admits is written.
#[tokio::test]
async fn an_amount_is_held_to_the_form_the_model_publishes() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();
    let arn = a_limit(&braket, &device, "tok-create-amount", start).await;

    // (case, amount, admitted) — the catalog is the form the model publishes for this
    // member, `@pattern ^\d+(\.\d{1,2})?$` with `@length(min: 1)`
    // (`smithy_query(query="operation", operation="UpdateSpendingLimit")`): digits, with
    // at most two decimal places. The admitted rows are the shapes of that form; the
    // refused rows each break one part of it.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("digits only", "250", true),
        ("one decimal place", "250.5", true),
        ("two decimal places", "250.55", true),
        ("zero", "0", true),
        ("a leading zero", "007", true),
        ("not a number at all", "not-a-number", false),
        ("empty", "", false),
        ("three decimal places", "250.555", false),
        ("a trailing point", "250.", false),
        ("a leading point", ".5", false),
        ("two points", "2.5.5", false),
        ("negative", "-250", false),
        ("an exponent", "2.5e2", false),
        ("a thousands separator", "1,000", false),
        ("a currency symbol", "$250", false),
        ("surrounded by spaces", " 250 ", false),
    ];

    let mut stored = ORIGINAL_AMOUNT.to_string();
    for (index, (case, amount, admitted)) in rows.iter().enumerate() {
        let reply = update_limit(
            &braket,
            &arn,
            update_body(&format!("tok-amount-{index}"), Some(amount), None),
        )
        .await;
        if *admitted {
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({amount:?}): expected 200, got {} {}",
                reply.status, reply.body
            );
            stored = (*amount).to_string();
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?} ({amount:?}): expected 400, got {} {}",
                reply.status, reply.body
            );
        }
        let limit = limit_by_arn(&braket, &device, &arn).await;
        assert_eq!(
            member(&limit, "spendingLimit"),
            stored,
            "case {case:?} ({amount:?}): expected the stored amount {stored}, got {limit}"
        );
    }
}

/// A period whose end does not follow its start covers no time, so an update carrying one
/// is refused and changes nothing.
#[tokio::test]
async fn a_time_period_that_covers_no_time_is_refused_by_an_update() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();
    let arn = a_limit(&braket, &device, "tok-create-period", start).await;
    let before = limit_by_arn(&braket, &device, &arn).await;

    // (case, period) — the rows differ in how far the end falls before the start.
    let rows: Vec<(&str, Value)> = vec![
        ("the end equals the start", period(start, start)),
        (
            "the end is one second before the start",
            period(start, start - 1),
        ),
        (
            "the end is a year before the start",
            period(start, start - 31_536_000),
        ),
        ("the end is the epoch", period(start, 0)),
    ];

    for (index, (case, time_period)) in rows.iter().enumerate() {
        let reply = update_limit(
            &braket,
            &arn,
            update_body(
                &format!("tok-period-{index}"),
                Some("777"),
                Some(time_period.clone()),
            ),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );

        let after = limit_by_arn(&braket, &device, &arn).await;
        assert_eq!(
            member(&after, "spendingLimit"),
            ORIGINAL_AMOUNT,
            "case {case:?}: a refused update writes neither member, got {after}"
        );
        assert!(
            (period_second(&after, "endAt") - period_second(&before, "endAt")).abs()
                < f64::EPSILON,
            "case {case:?}: a refused update leaves the period alone, was {}, got {}",
            period_second(&before, "endAt"),
            period_second(&after, "endAt")
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: the same token with the same request applies
/// the change once, and with a different request it is a reuse and refused.
#[tokio::test]
async fn the_client_token_makes_an_update_idempotent() {
    let braket = Braket::new();
    let device = a_device(&braket, "QPU", "ONLINE").await;
    let start = now_epoch();
    let arn = a_limit(&braket, &device, "tok-create-idempotent", start).await;

    let token = "tok-update-idempotent";
    let body = update_body(token, Some("321.00"), Some(period(start, start + 7200)));

    let first = update_limit(&braket, &arn, body.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first update: expected 200, got {} {}",
        first.status, first.body
    );
    let applied = limit_by_arn(&braket, &device, &arn).await;

    // Replaying the same request, several times over, applies the same change once.
    for attempt in 1..=3 {
        let replay = update_limit(&braket, &arn, body.clone()).await;
        assert_eq!(
            replay.status, 200,
            "replay {attempt}: expected 200, got {} {}",
            replay.status, replay.body
        );
        let after = limit_by_arn(&braket, &device, &arn).await;
        assert_eq!(
            member(&after, "spendingLimit"),
            member(&applied, "spendingLimit"),
            "replay {attempt}: the amount is the one the first call wrote, got {after}"
        );
        assert!(
            (stamp(&after, "updatedAt") - stamp(&applied, "updatedAt")).abs() < f64::EPSILON,
            "replay {attempt}: a replayed token does not move updatedAt, was {}, got {}",
            stamp(&applied, "updatedAt"),
            stamp(&after, "updatedAt")
        );
    }

    // (case, the member that differs) — one row per member this request can write, plus
    // the limit it names.
    let other = a_limit(&braket, &device, "tok-create-other", start).await;
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "a different amount",
            arn.clone(),
            update_body(token, Some("654.00"), Some(period(start, start + 7200))),
        ),
        (
            "a different period",
            arn.clone(),
            update_body(token, Some("321.00"), Some(period(start, start + 10_800))),
        ),
        (
            "no period at all",
            arn.clone(),
            update_body(token, Some("321.00"), None),
        ),
        (
            "a different limit",
            other.clone(),
            update_body(token, Some("321.00"), Some(period(start, start + 7200))),
        ),
    ];

    for (case, named, body) in rows {
        let reply = update_limit(&braket, &named, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token reused for a different request is refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} in {}",
            reply.error,
            reply.body
        );
    }

    let after = limit_by_arn(&braket, &device, &arn).await;
    assert_eq!(
        member(&after, "spendingLimit"),
        member(&applied, "spendingLimit"),
        "no refused reuse changed the limit, got {after}"
    );
    assert_eq!(
        member(&limit_by_arn(&braket, &device, &other).await, "spendingLimit"),
        ORIGINAL_AMOUNT,
        "no refused reuse changed the other limit either"
    );
}
