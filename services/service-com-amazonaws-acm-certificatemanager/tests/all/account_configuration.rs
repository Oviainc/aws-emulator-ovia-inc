// SPDX-License-Identifier: CC0-1.0

//! The ACM account-level expiry-events configuration, end to end over the wire
//! (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! The rule under test: `GetAccountConfiguration` answers 200 and reports the
//! account's expiry-events configuration exactly as the last successful
//! `PutAccountConfiguration` persisted it, and ACM's documented default while no
//! configuration has been written.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`] — the path the emulator
//! serves real traffic on — and every expected value is written by one operation and
//! read back by the other, so a response that is not computed from the persisted
//! store cannot pass. The tables span the class the rule generalises over: the
//! `DaysBeforeExpiry` range (its published bound, one past it, and values either
//! side), repeated writes in sequence, and the request forms that write nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{call, error_code, router};

/// `DaysBeforeExpiry` values ACM accepts: its target `PositiveInteger` declares
/// `@range(min: 1)`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ExpiryEventsConfiguration")`)
/// and the service refuses anything over its default 45-day notice window, so both
/// published bounds and values between them are in class.
const ACCEPTED_DAYS_BEFORE_EXPIRY: &[i64] = &[1, 2, 15, 30, 44, 45];

/// `DaysBeforeExpiry` values below the modelled minimum of 1: the bound itself is 1,
/// so 0 is one past it and a negative value is further out. These are refused by the
/// boundary validator, before any handler runs.
const BELOW_RANGE_DAYS_BEFORE_EXPIRY: &[i64] = &[0, -1, -45, i32::MIN as i64];

/// `DaysBeforeExpiry` values over the maximum ACM accepts: 46 is one past the bound,
/// and the rest are further out, up to the largest integer the modelled type holds.
const ABOVE_RANGE_DAYS_BEFORE_EXPIRY: &[i64] = &[46, 90, 365, i32::MAX as i64];

/// The `DaysBeforeExpiry` ACM reports for an account that has written no
/// configuration: "By default, accounts receive events starting 45 days before
/// certificate expiration"
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#ExpiryEventsConfiguration", member="DaysBeforeExpiry")`).
const DEFAULT_DAYS_BEFORE_EXPIRY: i64 = 45;

/// `GetAccountConfiguration`'s body, failing the test unless the call answered 200 —
/// the only success this operation models (its sole errors are
/// `AccessDeniedException` and `ThrottlingException`).
async fn get_account_configuration(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, body) = call(router, "GetAccountConfiguration", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAccountConfiguration must answer 200, got {status} {body:?}"
    );
    body
}

/// Write `body` with `PutAccountConfiguration` and return `(status, response)`.
async fn put_account_configuration(
    router: &MultiServiceRouter,
    body: &Value,
) -> (u16, Value) {
    call(router, "PutAccountConfiguration", body).await
}

/// An account that has never written a configuration is running on ACM's documented
/// default, and that is what the read reports — the same answer however often it is
/// read.
#[tokio::test]
async fn baseline_account_configuration_is_served_and_reports_the_documented_default() {
    let router = router();

    for read in ["first read", "second read"] {
        let body = get_account_configuration(&router, read).await;
        assert_eq!(
            body["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(DEFAULT_DAYS_BEFORE_EXPIRY),
            "case {read:?}: an account that never wrote a configuration must report the \
             documented default, got {body:?}"
        );
    }
}

/// Every accepted `DaysBeforeExpiry` written by `PutAccountConfiguration` is what
/// `GetAccountConfiguration` then reports — the Put/Get round trip, one fresh store
/// per row so no row can pass on another's write.
#[tokio::test]
async fn get_reports_the_days_before_expiry_that_put_persisted() {
    for days in ACCEPTED_DAYS_BEFORE_EXPIRY {
        let router = router();
        let (status, response) = put_account_configuration(
            &router,
            &json!({ "ExpiryEvents": { "DaysBeforeExpiry": days }, "IdempotencyToken": "tok1" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {days}: PutAccountConfiguration must succeed, got {status} {response:?}"
        );

        let body = get_account_configuration(&router, &days.to_string()).await;
        assert_eq!(
            body["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(days),
            "case {days}: GetAccountConfiguration must report the persisted \
             DaysBeforeExpiry, got {body:?}"
        );
    }
}

/// A later write replaces the configuration an earlier one left, whatever order the
/// values come in, and every read in between reports the value then current.
#[tokio::test]
async fn each_write_replaces_what_the_previous_one_persisted() {
    let router = router();

    // Walk the accepted values forwards and then backwards, so a row is reached
    // both from a smaller and from a larger predecessor.
    let sequence: Vec<i64> = ACCEPTED_DAYS_BEFORE_EXPIRY
        .iter()
        .copied()
        .chain(ACCEPTED_DAYS_BEFORE_EXPIRY.iter().rev().copied())
        .collect();

    for (index, days) in sequence.iter().enumerate() {
        // A distinct idempotency token per call: "If you change the idempotency
        // token for each call, ACM treats each call as a new request"
        // (`PutAccountConfigurationRequest$IdempotencyToken` doc).
        let token = format!("token{index}");
        let (status, response) = put_account_configuration(
            &router,
            &json!({
                "ExpiryEvents": { "DaysBeforeExpiry": days },
                "IdempotencyToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {days} (write {index}): PutAccountConfiguration must succeed, got \
             {status} {response:?}"
        );

        let body = get_account_configuration(&router, &format!("write {index}: {days}")).await;
        assert_eq!(
            body["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(days),
            "case {days} (write {index}): the latest write must be what Get reports, got \
             {body:?}"
        );
    }
}

/// Re-sending one request is not a second change: the same token and value applied
/// again leave the configuration exactly as the first call left it.
#[tokio::test]
async fn resending_one_request_leaves_the_configuration_unchanged() {
    let router = router();
    let request = json!({
        "ExpiryEvents": { "DaysBeforeExpiry": 30 },
        "IdempotencyToken": "sametoken",
    });

    for attempt in 1..=3 {
        let (status, response) = put_account_configuration(&router, &request).await;
        assert_eq!(
            status, 200,
            "case attempt {attempt}: PutAccountConfiguration must succeed, got {status} \
             {response:?}"
        );
        let body = get_account_configuration(&router, &format!("attempt {attempt}")).await;
        assert_eq!(
            body["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(30),
            "case attempt {attempt}: a re-sent request must leave the same configuration, \
             got {body:?}"
        );
    }
}

/// A live idempotency token stands for the request it was first sent with: another
/// request carrying it is refused with `ConflictException` and writes nothing,
/// whatever the second request asks for.
#[tokio::test]
async fn a_live_token_reused_by_a_different_request_is_a_conflict() {
    // Each row is the `ExpiryEvents` a second request sends under a token already
    // spent on `{"DaysBeforeExpiry": 30}`: a different value, either side of it, and
    // the forms that carry no value at all.
    let second_requests = [
        json!({ "ExpiryEvents": { "DaysBeforeExpiry": 1 } }),
        json!({ "ExpiryEvents": { "DaysBeforeExpiry": 29 } }),
        json!({ "ExpiryEvents": { "DaysBeforeExpiry": 31 } }),
        json!({ "ExpiryEvents": { "DaysBeforeExpiry": 45 } }),
        json!({ "ExpiryEvents": {} }),
        json!({}),
    ];

    for second in second_requests {
        let router = router();
        let (status, response) = put_account_configuration(
            &router,
            &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 30 }, "IdempotencyToken": "spent" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {second}: the first PutAccountConfiguration must succeed, got {status} \
             {response:?}"
        );

        let mut body = second.clone();
        body["IdempotencyToken"] = json!("spent");
        let (status, response) = put_account_configuration(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {second}: a live token reused by a different request must be refused with \
             400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {second}: the refusal must be a ConflictException, got {response:?}"
        );

        let read = get_account_configuration(&router, &second.to_string()).await;
        assert_eq!(
            read["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(30),
            "case {second}: a refused reuse must leave the first request's configuration, \
             got {read:?}"
        );
    }
}

/// The same value under a fresh token is a new request, and it is applied: the
/// conflict above is about the TOKEN standing for another request, not about the
/// value already being stored.
#[tokio::test]
async fn a_fresh_token_is_a_new_request_whatever_it_asks_for() {
    let router = router();

    for (index, days) in [30, 30, 45, 45, 1].iter().enumerate() {
        let token = format!("fresh{index}");
        let (status, response) = put_account_configuration(
            &router,
            &json!({
                "ExpiryEvents": { "DaysBeforeExpiry": days },
                "IdempotencyToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {token}: a request under a fresh token must succeed, got {status} \
             {response:?}"
        );
        let body = get_account_configuration(&router, &token).await;
        assert_eq!(
            body["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(days),
            "case {token}: the new request's value must be what Get reports, got {body:?}"
        );
    }
}

/// A `DaysBeforeExpiry` outside the range ACM accepts is refused — below the
/// modelled minimum or over the maximum the service enforces — and a refused request
/// writes nothing: the configuration a successful call left is still what Get
/// reports.
#[tokio::test]
async fn a_value_outside_the_accepted_range_is_refused_and_writes_nothing() {
    let router = router();
    let (status, response) = put_account_configuration(
        &router,
        &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 30 }, "IdempotencyToken": "good" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case setup: PutAccountConfiguration must succeed, got {status} {response:?}"
    );

    let out_of_range = BELOW_RANGE_DAYS_BEFORE_EXPIRY
        .iter()
        .chain(ABOVE_RANGE_DAYS_BEFORE_EXPIRY.iter());
    for days in out_of_range {
        let (status, response) = put_account_configuration(
            &router,
            &json!({ "ExpiryEvents": { "DaysBeforeExpiry": days }, "IdempotencyToken": "bad" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {days}: a DaysBeforeExpiry outside 1..=45 must be refused with 400, got \
             {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {days}: the refusal must carry the modelled validation error code, got \
             {response:?}"
        );

        let body = get_account_configuration(&router, &days.to_string()).await;
        assert_eq!(
            body["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(30),
            "case {days}: a refused write must leave the stored configuration alone, got \
             {body:?}"
        );
    }
}

/// A value over the maximum ACM accepts is refused by the handler, which names the
/// bound it exceeded.
#[tokio::test]
async fn a_value_over_the_maximum_names_the_bound_it_exceeded() {
    let router = router();

    for days in ABOVE_RANGE_DAYS_BEFORE_EXPIRY {
        let (status, response) = put_account_configuration(
            &router,
            &json!({ "ExpiryEvents": { "DaysBeforeExpiry": days }, "IdempotencyToken": "over" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {days}: a DaysBeforeExpiry over 45 must be refused with 400, got {status} \
             {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains("45"),
            "case {days}: the refusal must name the 45-day maximum, got {message:?}"
        );
    }
}

/// `IdempotencyToken` is `@required`: a request without one is refused before any
/// configuration is written.
#[tokio::test]
async fn a_request_without_an_idempotency_token_is_refused_and_writes_nothing() {
    let router = router();
    let (status, response) = put_account_configuration(
        &router,
        &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 30 } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case no token: a request missing the required IdempotencyToken must be refused \
         with 400, got {status} {response:?}"
    );

    let body = get_account_configuration(&router, "no token").await;
    assert_eq!(
        body["ExpiryEvents"]["DaysBeforeExpiry"],
        json!(DEFAULT_DAYS_BEFORE_EXPIRY),
        "case no token: a refused write must leave the account on the default, got {body:?}"
    );
}

/// A request that names no `ExpiryEvents` names no supported configuration option,
/// so it changes nothing — neither configuring an account that was unconfigured nor
/// clearing a configuration that was written.
#[tokio::test]
async fn a_request_naming_no_expiry_events_changes_nothing() {
    let router = router();

    let (status, response) =
        put_account_configuration(&router, &json!({ "IdempotencyToken": "empty1" })).await;
    assert_eq!(
        status, 200,
        "case unconfigured: PutAccountConfiguration without ExpiryEvents must succeed, got \
         {status} {response:?}"
    );
    let body = get_account_configuration(&router, "unconfigured").await;
    assert_eq!(
        body["ExpiryEvents"]["DaysBeforeExpiry"],
        json!(DEFAULT_DAYS_BEFORE_EXPIRY),
        "case unconfigured: a request naming no ExpiryEvents must leave the account on the \
         default, got {body:?}"
    );

    let (status, response) = put_account_configuration(
        &router,
        &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 30 }, "IdempotencyToken": "set30" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case configured: PutAccountConfiguration must succeed, got {status} {response:?}"
    );

    let (status, response) =
        put_account_configuration(&router, &json!({ "IdempotencyToken": "empty2" })).await;
    assert_eq!(
        status, 200,
        "case configured: PutAccountConfiguration without ExpiryEvents must succeed, got \
         {status} {response:?}"
    );
    let body = get_account_configuration(&router, "configured").await;
    assert_eq!(
        body["ExpiryEvents"]["DaysBeforeExpiry"],
        json!(30),
        "case configured: a request naming no ExpiryEvents must not clear the stored \
         configuration, got {body:?}"
    );
}

/// Reading the configuration never changes it: the write that follows a read is what
/// decides what the next read sees, and two reads around it agree with the store.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = router();
    let (status, response) = put_account_configuration(
        &router,
        &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 7 }, "IdempotencyToken": "seven" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case setup: PutAccountConfiguration must succeed, got {status} {response:?}"
    );

    for read in 1..=5 {
        let body = get_account_configuration(&router, &format!("read {read}")).await;
        assert_eq!(
            body["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(7),
            "case read {read}: every read must report the same stored configuration, got \
             {body:?}"
        );
    }
}
