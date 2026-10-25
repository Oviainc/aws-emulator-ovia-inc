// SPDX-License-Identifier: CC0-1.0

//! `CreateApiCache`'s resource rules, end to end over HTTP.
//!
//! `api_cache.rs` pins the `ttl` window and the create/read round trip. This suite
//! pins the two rules the create shares with the rest of the service, each over a
//! table that spans its class rather than one request:
//!
//! 1. the `apiId` a cache operation resolves is read the same way every other
//!    `apiId`-resolving operation reads one: an id holding more characters than an
//!    AppSync API id has names no API, so the call is a bad request (400
//!    `BadRequestException`) naming the rejected id, while a well-formed id this
//!    account does not hold is a not-found (404) — asserted on all three implemented
//!    cache operations, over ids from one character to well past the id length;
//! 2. a cache belongs to the ONE API it was created for ("Creates a cache for the
//!    GraphQL API", and every cache operation resolves it from the `apiId` alone), so
//!    a create writes only that API's cache: a sibling API still holds none, and each
//!    API reports the settings its own create carried.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract
//! suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};

/// The number of characters an AppSync API id has — `state::API_ID_LEN` is the
/// service's own copy. An id of at most this length could name an API; a longer one
/// could not.
const API_ID_LEN: usize = 26;

/// AppSync's fixed text for an API that is absent.
const API_NOT_FOUND: &str = "API not found.";

/// A `CreateApiCache` body with the members the create reads.
/// Source: `smithy_query(query="operation", operation="CreateApiCache")`.
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

/// An `UpdateApiCache` body — the three members that operation requires.
fn update_cache_body(ttl: i64, behavior: &str, cache_type: &str) -> Value {
    json!({
        "ttl": ttl,
        "apiCachingBehavior": behavior,
        "type": cache_type,
    })
}

/// Rule 1: the id's SHAPE decides bad-request from not-found, on every cache
/// operation that resolves an `apiId`.
///
/// The rows differ along the one thing the rule reads — the id's length — from a
/// single character to well past the id length, and each id is sent to the create,
/// the read and the update.
#[tokio::test]
async fn the_api_id_shape_decides_bad_request_from_not_found_on_every_cache_operation() {
    let router = router();

    // (label, id). Could name an API: at most API_ID_LEN characters. Could not:
    // longer. The alphabet is not part of the rule, so one row sits outside it.
    let ids = [
        ("one character".to_string(), "a".to_string()),
        (
            "21 characters outside the id alphabet".to_string(),
            "nonexistent0000000000".to_string(),
        ),
        (
            "one short of the id length".to_string(),
            "b".repeat(API_ID_LEN - 1),
        ),
        ("exactly the id length".to_string(), "c".repeat(API_ID_LEN)),
        (
            "one past the id length".to_string(),
            "d".repeat(API_ID_LEN + 1),
        ),
        (
            "32 characters".to_string(),
            "f3a91c07e6b5d0a48c2e1976badcafe0".to_string(),
        ),
        ("64 characters".to_string(), "e".repeat(64)),
    ];

    for (label, id) in &ids {
        let malformed = id.chars().count() > API_ID_LEN;
        let calls: [(&str, &str, String, Option<Value>); 3] = [
            (
                "CreateApiCache",
                "POST",
                format!("/v1/apis/{id}/ApiCaches"),
                // A `ttl` inside the documented window, so the request is refused on
                // the id alone.
                Some(create_cache_body(300, "FULL_REQUEST_CACHING", "SMALL", "ENABLED")),
            ),
            (
                "GetApiCache",
                "GET",
                format!("/v1/apis/{id}/ApiCaches"),
                None,
            ),
            (
                "UpdateApiCache",
                "POST",
                format!("/v1/apis/{id}/ApiCaches/update"),
                Some(update_cache_body(300, "FULL_REQUEST_CACHING", "SMALL")),
            ),
        ];

        for (operation, method, path, body) in calls {
            let case = format!("{operation} with an apiId of {label}");
            let answer = call(&router, method, &path, body).await;
            if malformed {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected status 400, got {answer:?}"
                );
                assert_eq!(
                    answer.error_type, "BadRequestException",
                    "case {case:?}: expected error type BadRequestException, got {answer:?}"
                );
                assert_eq!(
                    answer.message,
                    format!("Invalid API ID {id}"),
                    "case {case:?}: expected the refusal to name the rejected id, got {answer:?}"
                );
            } else {
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: expected status 404, got {answer:?}"
                );
                assert_eq!(
                    answer.error_type, "NotFoundException",
                    "case {case:?}: expected error type NotFoundException, got {answer:?}"
                );
                assert_eq!(
                    answer.message, API_NOT_FOUND,
                    "case {case:?}: expected the absent-API text, got {answer:?}"
                );
            }
        }
    }
}

/// Rule 2: a create writes the cache of the ONE API it addressed.
///
/// Three APIs, each created with different settings, plus a fourth that gets no
/// create at all: every API reports its own cache and the one without a create
/// reports no cache — and the cache-less API is told the CACHE is absent, not the
/// API, so the two not-founds stay distinguishable.
#[tokio::test]
async fn a_create_writes_only_the_addressed_apis_cache() {
    let router = router();

    // (label, ttl, caching behavior, cache type, health metrics). The rows differ
    // along every member the create stores.
    let rows: &[(&str, i64, &str, &str, &str)] = &[
        ("the minimum ttl, full-request", 1, "FULL_REQUEST_CACHING", "SMALL", "ENABLED"),
        ("a mid ttl, per-resolver", 600, "PER_RESOLVER_CACHING", "LARGE_2X", "DISABLED"),
        (
            "the maximum ttl, operation-level",
            3_600,
            "OPERATION_LEVEL_CACHING",
            "T2_MEDIUM",
            "ENABLED",
        ),
    ];

    // Create every API first, so each create below runs with siblings already in the
    // store and the rule is about the addressed API rather than the only API.
    let mut cached = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        cached.push((create_api(&router, &format!("Api_cache_owner_{index}")).await, row));
    }
    let uncached = create_api(&router, "Api_cache_none").await;

    for (api_id, (case, ttl, behavior, cache_type, health)) in &cached {
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/ApiCaches"),
            Some(create_cache_body(*ttl, behavior, cache_type, health)),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be accepted, got {created:?}"
        );
    }

    // Every API reports the settings ITS create carried, whatever the others carried.
    for (api_id, (case, ttl, behavior, cache_type, health)) in &cached {
        let read = call(&router, "GET", &format!("/v1/apis/{api_id}/ApiCaches"), None).await;
        assert_eq!(read.status, 200, "case {case:?}: expected 200, got {read:?}");
        assert_eq!(
            read.body["apiCache"]["ttl"].as_i64(),
            Some(*ttl),
            "case {case:?}: expected the ttl this API was created with, got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["apiCachingBehavior"].as_str(),
            Some(*behavior),
            "case {case:?}: expected the caching behavior this API was created with, \
             got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["type"].as_str(),
            Some(*cache_type),
            "case {case:?}: expected the cache type this API was created with, got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["healthMetricsConfig"].as_str(),
            Some(*health),
            "case {case:?}: expected the health-metrics setting this API was created with, \
             got {read:?}"
        );
    }

    // The API nobody created a cache for still holds none, and the refusal says the
    // CACHE is absent rather than the API.
    let read = call(&router, "GET", &format!("/v1/apis/{uncached}/ApiCaches"), None).await;
    assert_eq!(
        read.status, 404,
        "case an API with no create: expected status 404, got {read:?}"
    );
    assert_eq!(
        read.error_type, "NotFoundException",
        "case an API with no create: expected error type NotFoundException, got {read:?}"
    );
    assert_ne!(
        read.message, API_NOT_FOUND,
        "case an API with no create: expected the absent CACHE to be named, got {read:?}"
    );
    assert!(
        !read.message.is_empty(),
        "case an API with no create: expected the refusal to say what is absent, got {read:?}"
    );

    // The API itself is still there, so the not-found above is about the cache alone.
    let api = call(&router, "GET", &format!("/v1/apis/{uncached}"), None).await;
    assert_eq!(
        api.status, 200,
        "case an API with no create: expected the API itself to be readable, got {api:?}"
    );
}

/// An update of one API's cache leaves every other API's cache exactly as it was: the
/// cache a cache operation reaches is the addressed API's own.
#[tokio::test]
async fn an_update_touches_only_the_addressed_apis_cache() {
    let router = router();
    let first = create_api(&router, "Api_cache_update_one").await;
    let second = create_api(&router, "Api_cache_update_two").await;

    for api_id in [&first, &second] {
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/ApiCaches"),
            Some(create_cache_body(60, "FULL_REQUEST_CACHING", "SMALL", "DISABLED")),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case seed a cache on {api_id}: expected 200, got {created:?}"
        );
    }

    let updated = call(
        &router,
        "POST",
        &format!("/v1/apis/{first}/ApiCaches/update"),
        Some(update_cache_body(1_200, "PER_RESOLVER_CACHING", "LARGE")),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case update the first API's cache: expected 200, got {updated:?}"
    );

    // (label, api, the settings that API must now report).
    let expectations: &[(&str, &String, i64, &str, &str)] = &[
        ("the updated API", &first, 1_200, "PER_RESOLVER_CACHING", "LARGE"),
        ("the untouched API", &second, 60, "FULL_REQUEST_CACHING", "SMALL"),
    ];
    for (case, api_id, ttl, behavior, cache_type) in expectations {
        let read: Answer = call(&router, "GET", &format!("/v1/apis/{api_id}/ApiCaches"), None).await;
        assert_eq!(read.status, 200, "case {case:?}: expected 200, got {read:?}");
        assert_eq!(
            read.body["apiCache"]["ttl"].as_i64(),
            Some(*ttl),
            "case {case:?}: expected ttl {ttl}, got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["apiCachingBehavior"].as_str(),
            Some(*behavior),
            "case {case:?}: expected caching behavior {behavior}, got {read:?}"
        );
        assert_eq!(
            read.body["apiCache"]["type"].as_str(),
            Some(*cache_type),
            "case {case:?}: expected cache type {cache_type}, got {read:?}"
        );
    }
}
