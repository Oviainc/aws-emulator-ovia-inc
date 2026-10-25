// SPDX-License-Identifier: CC0-1.0

//! `FlushApiCache`, end to end over HTTP.
//!
//! One rule, asserted over a table that spans the class rather than a single recorded
//! request: a flush discards the entries the API's cache holds and leaves the cache
//! INSTANCE alone — it answers 200 with a response that carries no members, the stored
//! `ApiCache` every later read reports is exactly the one the create or the last update
//! left behind (settings and status alike), and the only thing the call writes is the
//! flushed API's own entry boundary. An API that holds no cache has no cache to flush,
//! so the call is a not-found; an `apiId` that is not an API id is a bad request.
//!
//! The entries themselves are written by the GraphQL endpoint rather than by any
//! control-plane call, so the boundary the flush records — read here from the shared
//! engine, the only place it is observable — is what the call leaves behind.
//!
//! The `call`/`create_api`/`Answer` harness is the error-contract suite's; the router
//! here is built over an engine this suite keeps a handle on, so a test can read the
//! state the handler wrote.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, Answer};

/// The number of characters an AppSync API id has, as the service itself states it, so
/// the id-length rows below cannot drift from the rule the service applies.
const API_ID_LEN: usize = state::API_ID_LEN;

/// The span the service records for a cache's transition to available — read from the
/// service itself, so this suite waits exactly as long as the service asks for rather
/// than on a number of its own.
const CACHE_SPAN: Duration = state::CACHE_SPAN;

/// `ApiCachingBehavior`'s whole value set, in the order the model declares it.
/// Source: `smithy_query(query="shape", shape="ApiCachingBehavior")`.
const CACHING_BEHAVIORS: &[&str] = &[
    "FULL_REQUEST_CACHING",
    "PER_RESOLVER_CACHING",
    "OPERATION_LEVEL_CACHING",
];

/// `ApiCacheType` values spanning the class the rule generalises over: the generic
/// instance identifiers and the discouraged legacy EC2-style ones alike.
/// Source: `smithy_query(query="shape", shape="ApiCacheType")`.
const CACHE_TYPES: &[&str] = &["T2_SMALL", "R4_2XLARGE", "LARGE", "LARGE_8X"];

/// `CacheHealthMetricsConfig`'s whole value set.
/// Source: `smithy_query(query="shape", shape="CacheHealthMetricsConfig")`.
const HEALTH_METRICS: &[&str] = &["ENABLED", "DISABLED"];

/// The `ApiCacheType` a row drives, so consecutive rows span that catalog.
fn cache_type(row: usize) -> &'static str {
    CACHE_TYPES[row % CACHE_TYPES.len()]
}

/// A router with only this service mounted, over the engine the caller keeps — so a
/// test can read what a handler wrote to the SHARED store.
fn router_over(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// A `CreateApiCache` body with the members the create requires, plus the optional
/// health-metrics setting.
fn create_cache_body(ttl: i64, behavior: &str, cache_type: &str, health: &str) -> Value {
    json!({
        "ttl": ttl,
        "apiCachingBehavior": behavior,
        "type": cache_type,
        "transitEncryptionEnabled": true,
        "atRestEncryptionEnabled": true,
        "healthMetricsConfig": health,
    })
}

/// Read the API's cache back.
async fn get_cache(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(router, "GET", &format!("/v1/apis/{api_id}/ApiCaches"), None).await
}

/// Flush the API's cache.
async fn flush_cache(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/FlushCache"),
        None,
    )
    .await
}

/// The entry boundary the service stored for the API's cache, or `None` while nothing
/// has flushed it (or the API holds no cache).
fn flush_boundary(engine: &Arc<engine::InMemoryEngine>, api_id: &str) -> Option<SystemTime> {
    let store = state::service_state(&**engine).expect("the appsync store on this engine");
    let guard = store.lock();
    guard
        .graphql_apis
        .get(api_id)
        .and_then(|api| api.api_cache.as_ref())
        .and_then(|cache| cache.last_flushed_at)
}

/// Whether the response body carries no members at all — the shape of the modelled
/// empty `FlushApiCacheResponse`, which is either an object with no members or no body
/// at all, and in particular carries no `apiCache`.
fn carries_no_members(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// Create one API and one cache under it, and return the API's id.
async fn api_with_cache(
    router: &MultiServiceRouter,
    name: &str,
    ttl: i64,
    behavior: &str,
    cache_type: &str,
    health: &str,
) -> String {
    let api_id = create_api(router, name).await;
    let created = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        Some(create_cache_body(ttl, behavior, cache_type, health)),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed a {cache_type} cache for {name:?}: expected 200, got {created:?}"
    );
    api_id
}

/// Which lifecycle state the cache is in when the row flushes it.
#[derive(Debug, Clone, Copy)]
enum When {
    /// Straight after the create, while the instance is still being provisioned.
    JustCreated,
    /// Once the clock has made the provisioning due, so the instance is available.
    Available,
    /// Straight after an update, while the new settings are being applied.
    JustUpdated,
}

/// The rule: a flush answers 200 with no members, leaves the stored cache exactly as it
/// was, and writes nothing but that cache's entry boundary.
///
/// The rows differ along every input the rule generalises over: the cache's settings
/// (`ttl`, caching behaviour, instance type, health metrics), the lifecycle state the
/// instance is in when it is flushed, and how many times it is flushed.
#[tokio::test]
async fn a_flush_answers_without_members_and_leaves_the_stored_cache_unchanged() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(&engine);

    let rows: &[(&str, i64, When)] = &[
        ("the published minimum ttl, just created", 1, When::JustCreated),
        ("a mid-window ttl, available", 300, When::Available),
        ("the published maximum ttl, just updated", 3_600, When::JustUpdated),
        ("a mid-window ttl, just created", 42, When::JustCreated),
    ];

    for (index, (case, ttl, when)) in rows.iter().enumerate() {
        let cache_type = cache_type(index);
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("{case} ({cache_type} caching {behavior}, metrics {health})");
        let api_id = api_with_cache(
            &router,
            &format!("Api_flush_{index}"),
            *ttl,
            behavior,
            cache_type,
            health,
        )
        .await;

        // The statuses the instance may legitimately report while the row flushes it:
        // the one it is in, and the one the clock owes a provisioning instance. A
        // flush must introduce neither any other status nor a new transition.
        let permitted: &[&str] = match when {
            When::JustCreated => &["CREATING", "AVAILABLE"],
            When::Available => {
                // Longer than the span the service records for the transition, so the
                // clock has made it due; the slack covers the scheduler.
                tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;
                &["AVAILABLE"]
            }
            When::JustUpdated => {
                let updated = call(
                    &router,
                    "POST",
                    &format!("/v1/apis/{api_id}/ApiCaches/update"),
                    Some(json!({
                        "ttl": ttl,
                        "apiCachingBehavior": behavior,
                        "type": cache_type,
                        "healthMetricsConfig": health,
                    })),
                )
                .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case:?}: expected the update to be accepted, got {updated:?}"
                );
                &["MODIFYING", "AVAILABLE"]
            }
        };

        let before = get_cache(&router, &api_id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected the seeded cache to be readable, got {before:?}"
        );
        assert_eq!(
            flush_boundary(&engine, &api_id),
            None,
            "case {case:?}: expected an unflushed cache to carry no entry boundary"
        );

        // Two flushes in a row: the second has the same effect as the first, so the
        // rule holds however many times it is applied.
        let mut previous_boundary: Option<SystemTime> = None;
        for attempt in 1..=2 {
            let flushed = flush_cache(&router, &api_id).await;
            assert_eq!(
                flushed.status, 200,
                "case {case:?} flush {attempt}: expected status 200, got {flushed:?}"
            );
            assert!(
                carries_no_members(&flushed.body),
                "case {case:?} flush {attempt}: expected a response with no members, \
                 got {flushed:?}"
            );

            let boundary = flush_boundary(&engine, &api_id);
            let Some(boundary) = boundary else {
                panic!(
                    "case {case:?} flush {attempt}: expected the flush to record an entry \
                     boundary, got {boundary:?}"
                );
            };
            if let Some(earlier) = previous_boundary {
                assert!(
                    boundary >= earlier,
                    "case {case:?} flush {attempt}: expected the boundary to move forward, \
                     got {boundary:?} after {earlier:?}"
                );
            }
            previous_boundary = Some(boundary);

            // The instance is untouched: every member of the stored cache reads back
            // as it did before the flush.
            let after = get_cache(&router, &api_id).await;
            assert_eq!(
                after.status, 200,
                "case {case:?} flush {attempt}: expected the cache to survive the flush, \
                 got {after:?}"
            );
            for member in [
                "ttl",
                "apiCachingBehavior",
                "type",
                "healthMetricsConfig",
                "transitEncryptionEnabled",
                "atRestEncryptionEnabled",
            ] {
                assert_eq!(
                    after.body["apiCache"][member], before.body["apiCache"][member],
                    "case {case:?} flush {attempt}: expected the stored {member} to be \
                     unchanged, got {after:?}"
                );
            }
            let status = after.body["apiCache"]["status"].as_str().unwrap_or_default();
            assert!(
                permitted.contains(&status),
                "case {case:?} flush {attempt}: expected the instance status to stay one of \
                 {permitted:?}, got {after:?}"
            );
        }

        // And the transition the clock owed is still owed to it: a flushed instance
        // becomes available on the same clock, so the flush restarted nothing.
        tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;
        let settled = get_cache(&router, &api_id).await;
        assert_eq!(
            settled.body["apiCache"]["status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: expected the flushed instance to be available on the clock, \
             got {settled:?}"
        );
    }
}

/// The write is scoped to the cache the call addressed: flushing one API's cache leaves
/// every other API's cache exactly as it was, boundary included.
#[tokio::test]
async fn a_flush_touches_only_the_cache_it_addressed() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(&engine);

    // Three APIs, each with a cache of its own; only the middle one is flushed.
    let mut api_ids = Vec::new();
    for index in 0..3 {
        api_ids.push(
            api_with_cache(
                &router,
                &format!("Api_flush_scope_{index}"),
                60 + index as i64,
                CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()],
                cache_type(index),
                HEALTH_METRICS[index % HEALTH_METRICS.len()],
            )
            .await,
        );
    }
    let before: Vec<Value> = {
        let mut bodies = Vec::new();
        for api_id in &api_ids {
            bodies.push(get_cache(&router, api_id).await.body["apiCache"].clone());
        }
        bodies
    };

    let flushed_id = api_ids[1].clone();
    let flushed = flush_cache(&router, &flushed_id).await;
    assert_eq!(
        flushed.status, 200,
        "case flush the second api: expected status 200, got {flushed:?}"
    );

    for (index, api_id) in api_ids.iter().enumerate() {
        let case = format!("api {index} after flushing api 1");
        let boundary = flush_boundary(&engine, api_id);
        if api_id == &flushed_id {
            assert!(
                boundary.is_some(),
                "case {case:?}: expected the flushed cache to carry a boundary, \
                 got {boundary:?}"
            );
        } else {
            assert_eq!(
                boundary, None,
                "case {case:?}: expected an unflushed cache to carry no boundary, \
                 got {boundary:?}"
            );
        }
        let read = get_cache(&router, api_id).await;
        for member in ["ttl", "apiCachingBehavior", "type", "healthMetricsConfig"] {
            assert_eq!(
                read.body["apiCache"][member], before[index][member],
                "case {case:?}: expected the stored {member} to be unchanged, got {read:?}"
            );
        }
    }
}

/// There is nothing to flush where there is no cache: the call is a not-found, and it
/// writes nothing — so the API still holds no cache afterwards. An `apiId` that is not
/// an API id at all is the bad request the shape rule makes it, on this operation as on
/// every other that resolves one.
///
/// The rows differ along what the `apiId` names and along its length: an API that holds
/// no cache, well-formed ids of several lengths that name no API, and ids past the
/// length an AppSync API id has.
#[tokio::test]
async fn flushing_what_holds_no_cache_is_a_not_found_and_writes_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(&engine);

    let cacheless = create_api(&router, "Api_flush_no_cache").await;
    let absent_short = "a".to_string();
    let absent_exact = "b".repeat(API_ID_LEN);
    let overlong_by_one = "c".repeat(API_ID_LEN + 1);
    let overlong = "d".repeat(64);

    // (case, apiId, expected status)
    let rows: &[(&str, &String, u16)] = &[
        ("an api that holds no cache", &cacheless, 404),
        ("a one-character id naming no api", &absent_short, 404),
        ("a well-formed id naming no api", &absent_exact, 404),
        ("an id one past the id length", &overlong_by_one, 400),
        ("an id of 64 characters", &overlong, 400),
    ];

    for (case, api_id, expected) in rows {
        let answer = flush_cache(&router, api_id).await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected status {expected}, got {answer:?}"
        );
        let error_type = if *expected == 404 {
            "NotFoundException"
        } else {
            "BadRequestException"
        };
        assert_eq!(
            answer.error_type, error_type,
            "case {case:?}: expected error type {error_type}, got {answer:?}"
        );
        assert!(
            !answer.message.is_empty(),
            "case {case:?}: expected the refusal to say what is wrong, got {answer:?}"
        );
        assert_eq!(
            flush_boundary(&engine, api_id),
            None,
            "case {case:?}: expected a refused flush to write nothing"
        );
    }

    // The refused flush created no cache for the API that holds none, so the read is
    // still a not-found.
    let read = get_cache(&router, &cacheless).await;
    assert_eq!(
        read.status, 404,
        "case read the cacheless api after a refused flush: expected status 404, got {read:?}"
    );

    // And once that API does hold a cache, the very same call is served — so the
    // not-found above is about the absent cache, not about the operation.
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{cacheless}/ApiCaches"),
        Some(create_cache_body(60, "FULL_REQUEST_CACHING", "SMALL", "ENABLED")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create the cache the flush wanted: expected 200, got {created:?}"
    );
    let served = flush_cache(&router, &cacheless).await;
    assert_eq!(
        served.status, 200,
        "case flush once the cache exists: expected status 200, got {served:?}"
    );
    assert!(
        flush_boundary(&engine, &cacheless).is_some(),
        "case flush once the cache exists: expected the flush to record a boundary"
    );
}
