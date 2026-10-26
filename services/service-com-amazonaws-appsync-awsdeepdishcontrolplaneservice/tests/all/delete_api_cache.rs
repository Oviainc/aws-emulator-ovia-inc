// SPDX-License-Identifier: CC0-1.0

//! `DeleteApiCache`, end to end over HTTP.
//!
//! One rule, asserted over a table that spans the class rather than a single recorded
//! request: an API's cache instance is torn down only from the status that reports it
//! available for use. From any other status the delete is refused with the operation's
//! modelled `BadRequestException` (400) carrying AppSync's fixed sentence for that
//! class, and the instance is left exactly where it was; from the available status it
//! answers 200 with a response that carries no members and the cache stops existing, so
//! every later reader of that API answers not-found and a fresh create is accepted
//! again.
//!
//! The `call`/`create_api`/`Answer` harness is the error-contract suite's.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, Answer};

/// The number of characters an AppSync API id has, as the service itself states it, so
/// the id-shape rows below cannot drift from the rule the service applies.
const API_ID_LEN: usize = state::API_ID_LEN;

/// The span the service records for a cache's transition to available — read from the
/// service itself, so this suite waits exactly as long as the service asks for rather
/// than on a number of its own.
const CACHE_SPAN: Duration = state::CACHE_SPAN;

/// AppSync's fixed refusal text for a delete of a cache that is not available; the
/// service's own copy, so the assertion cannot drift from what the handler answers.
const NOT_DELETABLE: &str = state::CACHE_NOT_DELETABLE_MESSAGE;

/// The text a not-found carries for an API that holds no cache.
const NO_CACHE: &str = state::CACHE_NOT_FOUND_MESSAGE;

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
const CACHE_TYPES: &[&str] = &["SMALL", "LARGE_12X", "R4_LARGE", "T2_MEDIUM"];

/// `CacheHealthMetricsConfig`'s whole value set.
/// Source: `smithy_query(query="shape", shape="CacheHealthMetricsConfig")`.
const HEALTH_METRICS: &[&str] = &["ENABLED", "DISABLED"];

/// A router with only this service mounted, over a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
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
        "atRestEncryptionEnabled": false,
        "healthMetricsConfig": health,
    })
}

/// Create the API's cache.
async fn create_cache(
    router: &MultiServiceRouter,
    api_id: &str,
    ttl: i64,
    behavior: &str,
    cache_type: &str,
    health: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        Some(create_cache_body(ttl, behavior, cache_type, health)),
    )
    .await
}

/// Read the API's cache back.
async fn get_cache(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(router, "GET", &format!("/v1/apis/{api_id}/ApiCaches"), None).await
}

/// Delete the API's cache.
async fn delete_cache(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/ApiCaches"),
        None,
    )
    .await
}

/// Whether the response body carries no members at all — the shape of the modelled
/// empty `DeleteApiCacheResponse`, which is either an object with no members or no body
/// at all.
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
    let created = create_cache(router, &api_id, ttl, behavior, cache_type, health).await;
    assert_eq!(
        created.status, 200,
        "case seed a {cache_type} cache for {name:?}: expected 200, got {created:?}"
    );
    api_id
}

/// Which non-available status the instance is in when the row deletes it.
#[derive(Debug, Clone, Copy)]
enum NotYet {
    /// Straight after the create, while the instance is still being provisioned.
    JustCreated,
    /// Straight after an update, while the new settings are being applied.
    JustUpdated,
}

/// The rule's refusal half: while the instance is in any status other than the
/// available one the delete is refused, with 400 `BadRequestException` and AppSync's
/// fixed sentence, and the cache is left in place — so the very next read still reports
/// it and a later delete, once the clock has made the instance available, is served.
///
/// The rows differ along every input the rule generalises over: the status the instance
/// is in (being created, having new settings applied), the cache's settings (`ttl`,
/// caching behaviour, instance type, health metrics) and how many times the refused
/// delete is repeated.
#[tokio::test]
async fn deleting_a_cache_that_is_not_available_is_refused_and_keeps_the_cache() {
    let router = router();

    let rows: &[(&str, i64, NotYet)] = &[
        ("the published minimum ttl, being created", 1, NotYet::JustCreated),
        ("a mid-window ttl, being created", 300, NotYet::JustCreated),
        ("the published maximum ttl, settings being applied", 3_600, NotYet::JustUpdated),
        ("a mid-window ttl, settings being applied", 42, NotYet::JustUpdated),
    ];

    for (index, (case, ttl, when)) in rows.iter().enumerate() {
        let cache_type = CACHE_TYPES[index % CACHE_TYPES.len()];
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("{case} ({cache_type} caching {behavior}, metrics {health})");
        let api_id = api_with_cache(
            &router,
            &format!("Api_delete_busy_{index}"),
            *ttl,
            behavior,
            cache_type,
            health,
        )
        .await;

        if let NotYet::JustUpdated = when {
            // The instance has to be available before it accepts new settings, so let
            // the clock make the create due first; the slack covers the scheduler.
            tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;
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
        }

        // Two refusals in a row: the refusal writes nothing, so the second answers
        // exactly as the first did.
        for attempt in 1..=2 {
            let refused = delete_cache(&router, &api_id).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} delete {attempt}: expected status 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_type, "BadRequestException",
                "case {case:?} delete {attempt}: expected error type BadRequestException, \
                 got {refused:?}"
            );
            assert_eq!(
                refused.message, NOT_DELETABLE,
                "case {case:?} delete {attempt}: expected the fixed refusal sentence, \
                 got {refused:?}"
            );

            // The refused delete did not remove the cache: it is still reported, with
            // the settings the create or the update left on it.
            let read = get_cache(&router, &api_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} delete {attempt}: expected the cache to survive the refusal, \
                 got {read:?}"
            );
            assert_eq!(
                read.body["apiCache"]["ttl"].as_i64(),
                Some(*ttl),
                "case {case:?} delete {attempt}: expected the stored ttl to be unchanged, \
                 got {read:?}"
            );
            assert_eq!(
                read.body["apiCache"]["type"].as_str(),
                Some(cache_type),
                "case {case:?} delete {attempt}: expected the stored type to be unchanged, \
                 got {read:?}"
            );
        }

        // And the refusal is about the status, not about the operation: once the clock
        // has made the instance available the very same call is served.
        tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;
        let settled = get_cache(&router, &api_id).await;
        assert_eq!(
            settled.body["apiCache"]["status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: expected the instance to be available on the clock, got {settled:?}"
        );
        let served = delete_cache(&router, &api_id).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected the delete to be served once available, got {served:?}"
        );
    }
}

/// The rule's served half: a delete of an available instance answers 200 with no
/// members and the cache stops existing — every operation that resolves it answers
/// not-found afterwards, a second delete included, and a fresh create is accepted and
/// starts the lifecycle again.
///
/// The rows differ along the cache's settings and along how it reached the available
/// status (straight from the create, or after its settings were applied again).
#[tokio::test]
async fn deleting_an_available_cache_removes_it_and_answers_without_members() {
    let router = router();

    let rows: &[(&str, i64, bool)] = &[
        ("created then deleted", 1, false),
        ("created, updated, then deleted", 900, true),
        ("the published maximum ttl, created then deleted", 3_600, false),
    ];

    for (index, (case, ttl, update_first)) in rows.iter().enumerate() {
        let cache_type = CACHE_TYPES[index % CACHE_TYPES.len()];
        let behavior = CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()];
        let health = HEALTH_METRICS[index % HEALTH_METRICS.len()];
        let case = format!("{case} ({cache_type} caching {behavior}, metrics {health})");
        let api_id = api_with_cache(
            &router,
            &format!("Api_delete_available_{index}"),
            *ttl,
            behavior,
            cache_type,
            health,
        )
        .await;
        tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;

        if *update_first {
            let updated = call(
                &router,
                "POST",
                &format!("/v1/apis/{api_id}/ApiCaches/update"),
                Some(json!({
                    "ttl": ttl,
                    "apiCachingBehavior": behavior,
                    "type": cache_type,
                })),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected the update to be accepted, got {updated:?}"
            );
            tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;
        }

        let deleted = delete_cache(&router, &api_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected status 200, got {deleted:?}"
        );
        assert!(
            carries_no_members(&deleted.body),
            "case {case:?}: expected a response with no members, got {deleted:?}"
        );

        // The cache is gone, so every operation that resolves one answers not-found
        // with the absent-cache sentence.
        for (op, method, path, body) in [
            ("get", "GET", format!("/v1/apis/{api_id}/ApiCaches"), None),
            (
                "flush",
                "DELETE",
                format!("/v1/apis/{api_id}/FlushCache"),
                None,
            ),
            (
                "update",
                "POST",
                format!("/v1/apis/{api_id}/ApiCaches/update"),
                Some(json!({
                    "ttl": ttl,
                    "apiCachingBehavior": behavior,
                    "type": cache_type,
                })),
            ),
            (
                "delete again",
                "DELETE",
                format!("/v1/apis/{api_id}/ApiCaches"),
                None,
            ),
        ] {
            let answer = call(&router, method, &path, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} {op} after delete: expected status 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "NotFoundException",
                "case {case:?} {op} after delete: expected error type NotFoundException, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.message, NO_CACHE,
                "case {case:?} {op} after delete: expected the absent-cache sentence, \
                 got {answer:?}"
            );
        }

        // The API itself survived the delete, and holds room for a cache again: the
        // create that was a duplicate before is accepted, and the new instance starts
        // the lifecycle over.
        let recreated = create_cache(&router, &api_id, *ttl, behavior, cache_type, health).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: expected the cache to be creatable again, got {recreated:?}"
        );
        assert_eq!(
            recreated.body["apiCache"]["status"].as_str(),
            Some("CREATING"),
            "case {case:?}: expected the recreated instance to start over, got {recreated:?}"
        );
    }
}

/// There is nothing to delete where there is no cache: the call is a not-found and it
/// removes nothing. An `apiId` that is not an API id at all is the bad request the
/// shape rule makes it, on this operation as on every other that resolves one — and it
/// is NOT the status refusal, so the two refusals stay distinguishable.
///
/// The rows differ along what the `apiId` names and along its length: an API that holds
/// no cache, well-formed ids of several lengths that name no API, and ids past the
/// length an AppSync API id has.
#[tokio::test]
async fn deleting_what_holds_no_cache_is_a_not_found_and_removes_nothing() {
    let router = router();

    let cacheless = create_api(&router, "Api_delete_no_cache").await;
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
        let answer = delete_cache(&router, api_id).await;
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
        assert_ne!(
            answer.message, NOT_DELETABLE,
            "case {case:?}: expected the refusal to be about the resolved id, not the \
             instance status, got {answer:?}"
        );
        assert!(
            !answer.message.is_empty(),
            "case {case:?}: expected the refusal to say what is wrong, got {answer:?}"
        );
    }

    // The refused delete left the API alone, so the cache it holds none of can still be
    // created — and that create is what makes the very same call serveable.
    let created = create_cache(
        &router,
        &cacheless,
        60,
        "FULL_REQUEST_CACHING",
        "SMALL",
        "ENABLED",
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create the cache the delete wanted: expected 200, got {created:?}"
    );
    tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;
    let served = delete_cache(&router, &cacheless).await;
    assert_eq!(
        served.status, 200,
        "case delete once the cache exists and is available: expected status 200, got {served:?}"
    );
}

/// The removal is scoped to the cache the call addressed: deleting one API's cache
/// leaves every other API's cache exactly as it was.
#[tokio::test]
async fn a_delete_removes_only_the_cache_it_addressed() {
    let router = router();

    // Three APIs, each with a cache of its own; only the middle one is deleted.
    let mut api_ids = Vec::new();
    for index in 0..3 {
        api_ids.push(
            api_with_cache(
                &router,
                &format!("Api_delete_scope_{index}"),
                60 + index as i64,
                CACHING_BEHAVIORS[index % CACHING_BEHAVIORS.len()],
                CACHE_TYPES[index % CACHE_TYPES.len()],
                HEALTH_METRICS[index % HEALTH_METRICS.len()],
            )
            .await,
        );
    }
    tokio::time::sleep(CACHE_SPAN + Duration::from_millis(100)).await;

    let mut before = Vec::new();
    for api_id in &api_ids {
        before.push(get_cache(&router, api_id).await.body["apiCache"].clone());
    }

    let deleted_id = api_ids[1].clone();
    let deleted = delete_cache(&router, &deleted_id).await;
    assert_eq!(
        deleted.status, 200,
        "case delete the second api's cache: expected status 200, got {deleted:?}"
    );

    for (index, api_id) in api_ids.iter().enumerate() {
        let case = format!("api {index} after deleting api 1's cache");
        let read = get_cache(&router, api_id).await;
        if api_id == &deleted_id {
            assert_eq!(
                read.status, 404,
                "case {case:?}: expected the deleted cache to be gone, got {read:?}"
            );
            continue;
        }
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the untouched cache to be readable, got {read:?}"
        );
        for member in [
            "ttl",
            "apiCachingBehavior",
            "type",
            "healthMetricsConfig",
            "status",
            "transitEncryptionEnabled",
            "atRestEncryptionEnabled",
        ] {
            assert_eq!(
                read.body["apiCache"][member], before[index][member],
                "case {case:?}: expected the stored {member} to be unchanged, got {read:?}"
            );
        }
    }
}
