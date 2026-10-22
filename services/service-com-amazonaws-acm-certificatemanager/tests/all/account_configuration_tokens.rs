// SPDX-License-Identifier: CC0-1.0

//! The hour a `PutAccountConfiguration` idempotency token stays live, and the token
//! values the model admits — the part of the account-configuration contract that the
//! round-trip suite in [`crate::account_configuration`] does not pin.
//!
//! RULE: an idempotency token identifies one request for exactly as long as it is
//! live — "Idempotency tokens time out after one hour" — so a second request carrying
//! a LIVE token is refused as a different request under a processed token, while the
//! same string once the token has timed out "ACM treats … as a new request" and
//! applies it. The token itself is admitted exactly where the model says: 1..32
//! characters matching `^\w+$`.
//!
//! The table below spans the class the first rule generalises over by walking the
//! published span itself — just inside it, exactly at it, and past it — and the
//! second by taking both declared length bounds, one character past each, and a
//! character outside the declared pattern. Ages are applied by moving the
//! first-processed instant the emulator itself stores, which is the value its own
//! timeout is computed from, so the test pins the documented hour without sleeping
//! for it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_acm_certificatemanager::state;

use crate::certificate_lifecycle::{call, error_code, router, router_with_engine};

/// How old the stored request is made before the second call, and whether its token
/// is still live at that age. `IDEMPOTENCY_TOKEN_TTL` is the documented hour, so the
/// span either side of it is what decides a replay from a new request.
const TOKEN_AGES: &[(&str, Duration, bool)] = &[
    ("just created", Duration::ZERO, true),
    (
        "five seconds inside the hour",
        state::IDEMPOTENCY_TOKEN_TTL.saturating_sub(Duration::from_secs(5)),
        true,
    ),
    ("exactly the hour", state::IDEMPOTENCY_TOKEN_TTL, false),
    (
        "one second past the hour",
        state::IDEMPOTENCY_TOKEN_TTL.saturating_add(Duration::from_secs(1)),
        false,
    ),
    (
        "a day past the hour",
        Duration::from_secs(24 * 60 * 60),
        false,
    ),
];

/// Age every stored `PutAccountConfiguration` request by `age`, by moving the instant
/// the emulator itself stamped it with. Only that instant is written — the timeout is
/// still computed by the handler, exactly as in production.
fn age_processed_requests(engine: &Arc<engine::InMemoryEngine>, age: Duration) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let processed_at = SystemTime::now()
        .checked_sub(age)
        .expect("an instant that far back exists");
    for request in store.account_configuration_requests.values_mut() {
        request.processed_at = processed_at;
    }
}

/// How many `PutAccountConfiguration` tokens the store is currently holding.
fn tracked_token_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let tracked = store.lock();
    tracked.account_configuration_requests.len()
}

/// RULE: a token is live for the documented hour and no longer. While it is live, a
/// different request carrying it is refused and nothing is written; once it has timed
/// out, the same string is a new request and applies.
#[tokio::test]
async fn an_idempotency_token_is_live_for_exactly_the_documented_hour() {
    for (case, age, still_live) in TOKEN_AGES {
        let (router, engine) = router_with_engine();

        let (status, response) = call(
            &router,
            "PutAccountConfiguration",
            &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 10 }, "IdempotencyToken": "aged" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the first PutAccountConfiguration must succeed, got {status} \
             {response:?}"
        );

        age_processed_requests(&engine, *age);

        let (status, response) = call(
            &router,
            "PutAccountConfiguration",
            &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 20 }, "IdempotencyToken": "aged" }),
        )
        .await;
        let (read_status, read) = call(&router, "GetAccountConfiguration", &json!({})).await;
        assert_eq!(
            read_status, 200,
            "case {case}: the read-back GetAccountConfiguration must answer 200, got \
             {read_status} {read:?}"
        );

        if *still_live {
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ConflictException"),
                "case {case}: a live token on a different request must be a ConflictException, \
                 got {status} {response:?}"
            );
            assert_eq!(
                read["ExpiryEvents"]["DaysBeforeExpiry"],
                json!(10),
                "case {case}: a refused second request must leave the first one's \
                 configuration, got {read:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: a timed-out token must be a new request, got {status} \
                 {response:?}"
            );
            assert_eq!(
                read["ExpiryEvents"]["DaysBeforeExpiry"],
                json!(20),
                "case {case}: the new request's value must be what Get reports, got {read:?}"
            );
        }
    }
}

/// RULE, the same hour seen from the index: a timed-out token names no earlier
/// request, so it is forgotten rather than kept for ever — the index holds only the
/// tokens ACM would still recognise.
#[tokio::test]
async fn timed_out_tokens_are_forgotten_rather_than_accumulated() {
    let (router, engine) = router_with_engine();

    for index in 0..4 {
        let (status, response) = call(
            &router,
            "PutAccountConfiguration",
            &json!({
                "ExpiryEvents": { "DaysBeforeExpiry": 20 },
                "IdempotencyToken": format!("stale{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case stale{index}: PutAccountConfiguration must succeed, got {status} {response:?}"
        );
    }
    let live = tracked_token_count(&engine);
    assert_eq!(
        live, 4,
        "case live: every live token must still be tracked, got {live}"
    );

    // Past the hour, each of those tokens names no request ACM still recognises.
    age_processed_requests(
        &engine,
        state::IDEMPOTENCY_TOKEN_TTL + Duration::from_secs(1),
    );
    let (status, response) = call(
        &router,
        "PutAccountConfiguration",
        &json!({ "ExpiryEvents": { "DaysBeforeExpiry": 30 }, "IdempotencyToken": "fresh" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case after the hour: PutAccountConfiguration must succeed, got {status} {response:?}"
    );
    let tracked = tracked_token_count(&engine);
    assert_eq!(
        tracked, 1,
        "case after the hour: only the live token must remain tracked, got {tracked}"
    );
}

/// RULE: `IdempotencyToken` is admitted exactly where the model admits it — 1..32
/// characters matching `^\w+$`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#PutAccountConfigurationRequest")`)
/// — and a refused request writes nothing.
#[tokio::test]
async fn the_idempotency_token_is_admitted_exactly_where_the_model_admits_it() {
    let router = router();

    // Both declared length bounds, and the word characters the pattern names
    // (letters, digits and the underscore), are admitted.
    let admitted: &[(&str, String)] = &[
        ("one character, the declared minimum", "t".to_string()),
        ("thirty-two characters, the declared maximum", "t".repeat(32)),
        ("digits only", "20261022".to_string()),
        ("underscores, which `\\w` includes", "a_b_c".to_string()),
    ];
    for (index, (case, token)) in admitted.iter().enumerate() {
        // A distinct day count per row, so the row that applied is identifiable.
        let days = 1 + index as i64;
        let (status, response) = call(
            &router,
            "PutAccountConfiguration",
            &json!({
                "ExpiryEvents": { "DaysBeforeExpiry": days },
                "IdempotencyToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a token of {} characters must be admitted, got {status} {response:?}",
            token.len()
        );
        let (read_status, read) = call(&router, "GetAccountConfiguration", &json!({})).await;
        assert_eq!(
            read_status, 200,
            "case {case}: the read-back GetAccountConfiguration must answer 200, got \
             {read_status} {read:?}"
        );
        assert_eq!(
            read["ExpiryEvents"]["DaysBeforeExpiry"],
            json!(days),
            "case {case}: the admitted request must have applied, got {read:?}"
        );
    }

    // One character past each bound, and a character the pattern excludes, are
    // refused — and leave the last admitted configuration standing.
    let refused: &[(&str, String)] = &[
        ("zero characters, one past the minimum", String::new()),
        (
            "thirty-three characters, one past the maximum",
            "t".repeat(33),
        ),
        ("a space, which `\\w` excludes", "has space".to_string()),
        ("a hyphen, which `\\w` excludes", "has-hyphen".to_string()),
    ];
    let applied = json!(admitted.len() as i64);
    for (case, token) in refused {
        let (status, response) = call(
            &router,
            "PutAccountConfiguration",
            &json!({
                "ExpiryEvents": { "DaysBeforeExpiry": 45 },
                "IdempotencyToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a token outside the model's constraints must be refused, got \
             {status} {response:?}"
        );
        let (read_status, read) = call(&router, "GetAccountConfiguration", &json!({})).await;
        assert_eq!(
            read_status, 200,
            "case {case}: the read-back GetAccountConfiguration must answer 200, got \
             {read_status} {read:?}"
        );
        assert_eq!(
            read["ExpiryEvents"]["DaysBeforeExpiry"],
            applied,
            "case {case}: a refused request must write nothing, got {read:?}"
        );
    }
}
