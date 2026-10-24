// SPDX-License-Identifier: CC0-1.0

//! The API cache's request rules and its lifecycle, end to end over HTTP.
//!
//! Two rules, each asserted over a table that spans its class rather than one
//! recorded request:
//!
//! * `ttl` is documented "TTL in seconds for cache entries. Valid values are 1–3,600
//!   seconds" on `CreateApiCacheRequest$ttl` AND on `UpdateApiCacheRequest$ttl`, and
//!   both members target an unconstrained `Long` — so the service enforces the window
//!   itself: a value outside it is refused with `BadRequestException` (400) naming the
//!   window, and the refusal writes nothing, so the cache state the call would have
//!   changed is exactly what it was.
//! * a create is answered from the record it stored, for every `apiCachingBehavior`
//!   and every `ApiCacheType` the model declares, and that is the record `GetApiCache`
//!   reports; a provisioning instance is reported in its non-terminal status until the
//!   clock makes it available.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract
//! suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};

/// The published lower bound of the cache-entry window, in seconds. Source:
/// `smithy_query(query="shape", shape="CreateApiCacheRequest,UpdateApiCacheRequest")`
/// — `ttl` is documented "Valid values are 1–3,600 seconds" on both.
const TTL_MIN: i64 = 1;

/// The published upper bound of the cache-entry window, in seconds ([`TTL_MIN`] states
/// the source).
const TTL_MAX: i64 = 3_600;

/// The refusal a `ttl` outside the window takes, built from the two bounds the same
/// way the service builds it, so the message names the window the caller must stay in.
fn ttl_refusal() -> String {
    format!(
        "TTL value cannot be smaller than {TTL_MIN} second and larger than {TTL_MAX} seconds."
    )
}

/// The span the service records for a cache's transition to available — read from the
/// service itself, so this suite waits exactly as long as the service asks for rather
/// than on a number of its own.
const CACHE_SPAN: std::time::Duration =
    service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state::CACHE_SPAN;

/// `ApiCachingBehavior`'s whole value set, in the order the model declares it.
/// Source: `smithy_query(query="shape", shape="ApiCachingBehavior")`.
const CACHING_BEHAVIORS: &[&str] = &[
    "FULL_REQUEST_CACHING",
    "PER_RESOLVER_CACHING",
    "OPERATION_LEVEL_CACHING",
];

/// `ApiCacheType`'s whole value set, in the order the model declares it — the generic
/// identifiers and the discouraged legacy EC2-style ones alike.
/// Source: `smithy_query(query="shape", shape="ApiCacheType")`.
const CACHE_TYPES: &[&str] = &[
    "T2_SMALL",
    "T2_MEDIUM",
    "R4_LARGE",
    "R4_XLARGE",
    "R4_2XLARGE",
    "R4_4XLARGE",
    "R4_8XLARGE",
    "SMALL",
    "MEDIUM",
    "LARGE",
    "XLARGE",
    "LARGE_2X",
    "LARGE_4X",
    "LARGE_8X",
    "LARGE_12X",
];

/// `CacheHealthMetricsConfig`'s whole value set.
/// Source: `smithy_query(query="shape", shape="CacheHealthMetricsConfig")`.
const HEALTH_METRICS: &[&str] = &["ENABLED", "DISABLED"];

/// A `CreateApiCache` body with the four members the create requires.
fn create_cache_body(ttl: i64, behavior: &str, cache_type: &str) -> Value {
    json!({
        "ttl": ttl,
        "apiCachingBehavior": behavior,
        "type": cache_type,
        "transitEncryptionEnabled": true,
        "atRestEncryptionEnabled": true,
    })
}

/// Read the API's cache back.
async fn get_cache(router: &smithy_server_runtime::MultiServiceRouter, api_id: &str) -> Answer {
    call(router, "GET", &format!("/v1/apis/{api_id}/ApiCaches"), None).await
}

/// Rule 1, on the create: a `ttl` inside the documented window is accepted and stored;
/// one outside it is refused with `BadRequestException` naming the window, and leaves
/// the API with no cache at all — so the following read is a not-found.
///
/// The rows differ along the one thing the rule reads, the value of `ttl`: both
/// published bounds, one past each, the modelled default of an absent member, a
/// negative value, a value three orders of magnitude past the maximum and the largest
/// `Long` there is.
#[tokio::test]
async fn a_ttl_outside_the_documented_window_is_refused_and_stores_no_cache() {
    let router = router();

    let rows: &[(&str, i64)] = &[
        ("the published minimum", TTL_MIN),
        ("the published maximum", TTL_MAX),
        ("inside the window", 300),
        // 0 is also what an absent `ttl` deserializes to (the member's modelled
        // default), so this row covers a request that omits it.
        ("one below the minimum", TTL_MIN - 1),
        ("negative", -1),
        ("one past the maximum", TTL_MAX + 1),
        ("a day", 86_400),
        ("the largest Long", i64::MAX),
    ];

    for (index, (case, ttl)) in rows.iter().enumerate() {
        // One API per row: the rule is about what a refused create leaves behind, so
        // each row needs an API that holds no cache yet.
        let api_id = create_api(&router, &format!("Api_ttl_{index}")).await;
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/ApiCaches"),
            Some(create_cache_body(*ttl, "FULL_REQUEST_CACHING", "SMALL")),
        )
        .await;

        if (TTL_MIN..=TTL_MAX).contains(ttl) {
            assert_eq!(
                created.status, 200,
                "case {case:?} (ttl {ttl}): expected the create to be accepted, got {created:?}"
            );
            assert_eq!(
                created.body["apiCache"]["ttl"].as_i64(),
                Some(*ttl),
                "case {case:?} (ttl {ttl}): expected the stored ttl, got {created:?}"
            );
            let read = get_cache(&router, &api_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} (ttl {ttl}): expected the cache to be readable, got {read:?}"
            );
            continue;
        }

        assert_eq!(
            created.status, 400,
            "case {case:?} (ttl {ttl}): expected status 400, got {created:?}"
        );
        assert_eq!(
            created.error_type, "BadRequestException",
            "case {case:?} (ttl {ttl}): expected error type BadRequestException, got {created:?}"
        );
        assert_eq!(
            created.message,
            ttl_refusal(),
            "case {case:?} (ttl {ttl}): expected the refusal to name the window, got {created:?}"
        );

        // The refusal left no cache behind, so the read finds none.
        let read = get_cache(&router, &api_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?} (ttl {ttl}): expected no cache to have been created, got {read:?}"
        );
        assert_eq!(
            read.error_type, "NotFoundException",
            "case {case:?} (ttl {ttl}): expected error type NotFoundException, got {read:?}"
        );
        assert!(
            !read.message.is_empty(),
            "case {case:?} (ttl {ttl}): expected the not-found to say what is absent, \
             got {read:?}"
        );
    }
}

/// The window is a property of the request, so it is read before the `apiId` is
/// resolved: a `ttl` the request may not carry is refused the same way whatever the
/// `apiId` names, and only a well-formed request can produce a not-found.
#[tokio::test]
async fn the_window_is_read_before_the_api_is_resolved() {
    let router = router();
    // A well-formed id (26 characters, the length `state::API_ID_LEN` states) that
    // names no API of this account.
    let absent = "a".repeat(26);

    let refused = call(
        &router,
        "POST",
        &format!("/v1/apis/{absent}/ApiCaches"),
        Some(create_cache_body(TTL_MAX + 1, "FULL_REQUEST_CACHING", "SMALL")),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case an absent api with an out-of-window ttl: expected status 400, got {refused:?}"
    );
    assert_eq!(
        refused.message,
        ttl_refusal(),
        "case an absent api with an out-of-window ttl: expected the window refusal, \
         got {refused:?}"
    );

    // The same call with a ttl inside the window reaches the lookup and is a
    // not-found.
    let not_found = call(
        &router,
        "POST",
        &format!("/v1/apis/{absent}/ApiCaches"),
        Some(create_cache_body(TTL_MAX, "FULL_REQUEST_CACHING", "SMALL")),
    )
    .await;
    assert_eq!(
        not_found.status, 404,
        "case an absent api with a ttl inside the window: expected status 404, \
         got {not_found:?}"
    );
    assert_eq!(
        not_found.error_type, "NotFoundException",
        "case an absent api with a ttl inside the window: expected error type \
         NotFoundException, got {not_found:?}"
    );
}

/// Rule 1, on the update: the SAME window, enforced by the operation that changes a
/// stored cache — and a refused update changes nothing, so the cache still reports the
/// settings it had.
#[tokio::test]
async fn a_ttl_outside_the_documented_window_is_refused_by_the_update_too() {
    let router = router();
    let api_id = create_api(&router, "Api_ttl_update").await;
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        Some(create_cache_body(300, "FULL_REQUEST_CACHING", "SMALL")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed a cache: expected 200, got {created:?}"
    );

    // Accepted values, then refused ones; after each refusal the stored ttl is still
    // the last accepted one.
    let rows: &[(&str, i64)] = &[
        ("the published minimum", TTL_MIN),
        ("the published maximum", TTL_MAX),
        ("one below the minimum", TTL_MIN - 1),
        ("one past the maximum", TTL_MAX + 1),
        ("the smallest Long", i64::MIN),
    ];
    let mut stored = 300;
    for (case, ttl) in rows {
        let updated = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/ApiCaches/update"),
            Some(json!({
                "ttl": ttl,
                "apiCachingBehavior": "PER_RESOLVER_CACHING",
                "type": "MEDIUM",
            })),
        )
        .await;
        if (TTL_MIN..=TTL_MAX).contains(ttl) {
            assert_eq!(
                updated.status, 200,
                "case update to {case:?} (ttl {ttl}): expected 200, got {updated:?}"
            );
            stored = *ttl;
        } else {
            assert_eq!(
                updated.status, 400,
                "case update to {case:?} (ttl {ttl}): expected status 400, got {updated:?}"
            );
            assert_eq!(
                updated.error_type, "BadRequestException",
                "case update to {case:?} (ttl {ttl}): expected error type \
                 BadRequestException, got {updated:?}"
            );
            assert_eq!(
                updated.message,
                ttl_refusal(),
                "case update to {case:?} (ttl {ttl}): expected the refusal to name the \
                 window, got {updated:?}"
            );
        }

        let read = get_cache(&router, &api_id).await;
        assert_eq!(
            read.body["apiCache"]["ttl"].as_i64(),
            Some(stored),
            "case update to {case:?} (ttl {ttl}): expected the stored ttl to be {stored}, \
             got {read:?}"
        );
    }
}

/// An update of an API that holds no cache has nothing to change, so it is a not-found
/// rather than a create.
#[tokio::test]
async fn updating_a_cache_that_was_never_created_is_not_found() {
    let router = router();
    let api_id = create_api(&router, "Api_update_absent").await;
    let updated = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches/update"),
        Some(json!({
            "ttl": 60,
            "apiCachingBehavior": "FULL_REQUEST_CACHING",
            "type": "SMALL",
        })),
    )
    .await;
    assert_eq!(
        updated.status, 404,
        "case update with no cache: expected status 404, got {updated:?}"
    );
    assert_eq!(
        updated.error_type, "NotFoundException",
        "case update with no cache: expected error type NotFoundException, got {updated:?}"
    );
}

/// Rule 2: the create is answered from the record it stored, and that is the record
/// the read reports — for every `apiCachingBehavior`, every `ApiCacheType` and both
/// health-metrics settings the model declares.
///
/// One row per cache type (the widest of the three value sets), each carrying a
/// different caching behavior, health-metrics setting and ttl, so the rows differ
/// along every member the projection reads.
#[tokio::test]
async fn a_created_cache_is_read_back_exactly_as_it_was_stored() {
    let router = router();

    for (index, cache_type) in CACHE_TYPES.iter().enumerate() {
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let ttl = TTL_MIN + (index as i64 * 7) % TTL_MAX;
        let case = format!("a {cache_type} cache caching {behavior} with ttl {ttl}");
        let api_id = create_api(&router, &format!("Api_cache_{index}")).await;

        let mut body = create_cache_body(ttl, behavior, cache_type);
        body["healthMetricsConfig"] = json!(health);
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/ApiCaches"),
            Some(body),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {created:?}"
        );
        // A fresh instance is still being provisioned, so it is reported in the
        // non-terminal status rather than as available.
        assert_eq!(
            created.body["apiCache"]["status"].as_str(),
            Some("CREATING"),
            "case {case:?}: expected a provisioning instance, got {created:?}"
        );

        let read = get_cache(&router, &api_id).await;
        assert_eq!(read.status, 200, "case {case:?}: expected 200, got {read:?}");
        for member in [
            "ttl",
            "apiCachingBehavior",
            "type",
            "healthMetricsConfig",
            "transitEncryptionEnabled",
            "atRestEncryptionEnabled",
        ] {
            assert_eq!(
                read.body["apiCache"][member],
                created.body["apiCache"][member],
                "case {case:?}: expected the read to report the stored {member}, \
                 got {read:?}"
            );
        }

        // A second create has no cache to create: the API holds one.
        let again = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/ApiCaches"),
            Some(create_cache_body(ttl, behavior, cache_type)),
        )
        .await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected a second create to be refused, got {again:?}"
        );
        assert_eq!(
            again.error_type, "BadRequestException",
            "case {case:?}: expected error type BadRequestException, got {again:?}"
        );
    }
}

/// The instance the create reports as provisioning becomes available on its own: the
/// status is a transition the clock owes, not a value a read invents, so it is still
/// `CREATING` straight after the create and `AVAILABLE` once the span has passed —
/// and it stays `AVAILABLE` across reads.
#[tokio::test]
async fn a_provisioning_cache_becomes_available_on_the_clock() {
    let router = router();
    let api_id = create_api(&router, "Api_cache_clock").await;
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        Some(create_cache_body(60, "FULL_REQUEST_CACHING", "SMALL")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create a cache: expected 200, got {created:?}"
    );

    let immediate = get_cache(&router, &api_id).await;
    assert_eq!(
        immediate.body["apiCache"]["status"].as_str(),
        Some("CREATING"),
        "case reading straight after the create: expected the transient status, \
         got {immediate:?}"
    );

    // Longer than the span the service records for the transition, so the clock has
    // made it due; the slack covers the scheduler, not a guessed duration. Nothing in
    // the service sleeps for it.
    tokio::time::sleep(CACHE_SPAN + std::time::Duration::from_millis(100)).await;
    for attempt in 0..2 {
        let settled = get_cache(&router, &api_id).await;
        assert_eq!(
            settled.body["apiCache"]["status"].as_str(),
            Some("AVAILABLE"),
            "case read {attempt} after the span: expected an available instance, \
             got {settled:?}"
        );
        assert_eq!(
            settled.body["apiCache"]["ttl"].as_i64(),
            Some(60),
            "case read {attempt} after the span: expected the stored ttl to be unchanged, \
             got {settled:?}"
        );
    }
}
