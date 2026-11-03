// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the run-cache slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns; only the run-cache helpers live here.
//!
//! The rules under test:
//!
//! * a run cache is a resource of its own, stored once: whatever `CreateRunCache` carried
//!   is what the create's own response and a later `GetRunCache` report, each member taken
//!   from that one record and nothing invented for a member the request left out;
//!   * the cache's id is the decimal the modelled `RunCacheId` length admits and its ARN
//!     names it, both minted per cache;
//!   * `cacheBehavior` and `cacheBucketOwnerId` fall back to the values their own
//!     documentation names as the defaults when the create names none, and are echoed
//!     when it does;
//!   * every value of the modelled `RunCacheStatus` enum is terminal, so a cache is
//!     `ACTIVE` from the moment its create answered and a read never moves it;
//!   * a replayed `requestId` answers with the cache the first call created rather than
//!     minting a second one;
//! * an id no cache holds is the modelled 404, however the id is shaped;
//! * a run that names a cache is BOUND to that stored record: its canonical id is what
//!   `GetRun` reports, the cache's own default behavior applies when the run names none,
//!   and a `cacheId` no cache holds is the modelled 404.

use serde_json::{json, Map, Value};
use service_com_amazonaws_omics_omics::state::{
    FIRST_RESOURCE_ID, MAX_TAGS, RUN_CACHE_ID_OFFSET,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{
    assert_absent, assert_arn, assert_members, assert_timestamp, call, created_id, emulator,
    error_code, get_run, start_run, start_run_body, tag_map, workflow_for_runs, ACCOUNT,
};

/// Every value the modelled `CacheBehavior` enum admits.
/// Source: `smithy_query(query="shape",shape="CacheBehavior")` -> CACHE_ON_FAILURE |
/// CACHE_ALWAYS.
const CACHE_BEHAVIORS: &[&str] = &["CACHE_ON_FAILURE", "CACHE_ALWAYS"];

/// The `CacheBehavior` in force when a create names none.
/// Source: the `cacheBehavior` member of `CreateRunCacheRequest` ("If you don't specify a
/// value, the default behavior is CACHE_ON_FAILURE").
const DEFAULT_CACHE_BEHAVIOR: &str = "CACHE_ON_FAILURE";

/// Every value the modelled `RunCacheStatus` enum admits.
/// Source: `smithy_query(query="shape",shape="RunCacheStatus")` -> ACTIVE | DELETED |
/// FAILED.
const RUN_CACHE_STATUSES: &[&str] = &["ACTIVE", "DELETED", "FAILED"];

/// The published bounds of the modelled `RunCacheId` length (1..18).
/// Source: the `id` member of `CreateRunCacheResponse`.
const RUN_CACHE_ID_WIDTH: (usize, usize) = (1, 18);

/// An S3 location satisfying the modelled `S3UriForBucketOrObject` pattern.
const CACHE_LOCATION: &str = "s3://omics-emulator-cache/cache/";

/// The members `CreateRunCacheResponse` does NOT model, so no create response may carry
/// them however much the request named: they are read through `GetRunCache`.
const NOT_ON_THE_CREATE_RESPONSE: &[&str] = &[
    "name",
    "description",
    "cacheS3Uri",
    "cacheBehavior",
    "cacheBucketOwnerId",
    "creationTime",
];

/// `POST /runCache`.
async fn create_cache(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/runCache", Some(body)).await
}

/// `GET /runCache/{id}`.
async fn get_cache(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/runCache/{id}"), None).await
}

/// Assert `id` is the decimal the modelled `RunCacheId` describes — `^[0-9]+$` at
/// [`RUN_CACHE_ID_WIDTH`] — and one this emulator could have minted.
fn assert_run_cache_id(case: &str, id: &str) {
    let (min, max) = RUN_CACHE_ID_WIDTH;
    assert!(
        id.len() >= min && id.len() <= max && id.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a {min}..{max} digit decimal id, got {id:?}"
    );
    let value: u64 = id
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: id {id:?} does not parse as a number"));
    let first = FIRST_RESOURCE_ID + RUN_CACHE_ID_OFFSET;
    assert!(
        value >= first,
        "case {case:?}: expected an id at or above {first}, got {value}"
    );
}

/// A row of the create/read-back table: a request body, the members a later read must
/// report for it, and the members that read may not carry.
struct CacheCase {
    /// How the row is reported in a failure, and the cache's `requestId`.
    case: &'static str,
    /// The request body, without the `requestId` the row's `case` fills in.
    request: Value,
    /// Every member `GetRunCache` must carry with exactly this value.
    expected: Value,
    /// Members `GetRunCache` must NOT carry: nothing is invented for what the request
    /// left out.
    absent: Vec<&'static str>,
}

/// The table: one row per way a create can differ, so the rule is asserted over the class
/// and not over one request. The rows vary along the members present, the enum value
/// named, the form of the S3 location, the bucket owner and the number of tags.
fn cache_cases() -> Vec<CacheCase> {
    let mut cases = vec![
        CacheCase {
            case: "only the required location",
            request: json!({ "cacheS3Location": CACHE_LOCATION }),
            expected: json!({
                "cacheS3Uri": CACHE_LOCATION,
                // The documented defaults, applied because the request named neither.
                "cacheBehavior": DEFAULT_CACHE_BEHAVIOR,
                "cacheBucketOwnerId": ACCOUNT,
            }),
            absent: vec!["name", "description", "tags"],
        },
        CacheCase {
            case: "a named cache",
            request: json!({ "cacheS3Location": CACHE_LOCATION, "name": "autumn cohort" }),
            expected: json!({
                "name": "autumn cohort",
                "cacheS3Uri": CACHE_LOCATION,
                "cacheBehavior": DEFAULT_CACHE_BEHAVIOR,
            }),
            absent: vec!["description", "tags"],
        },
        CacheCase {
            case: "a described cache",
            request: json!({
                "cacheS3Location": CACHE_LOCATION,
                "description": "task outputs for the autumn cohort",
            }),
            expected: json!({
                "description": "task outputs for the autumn cohort",
                "cacheS3Uri": CACHE_LOCATION,
            }),
            absent: vec!["name", "tags"],
        },
        CacheCase {
            // The modelled `S3UriForBucketOrObject` pattern admits a bucket with no key.
            case: "a bucket-only location",
            request: json!({ "cacheS3Location": "s3://omics-emulator-cache" }),
            expected: json!({ "cacheS3Uri": "s3://omics-emulator-cache" }),
            absent: vec!["name", "description", "tags"],
        },
        CacheCase {
            // The same pattern admits a dotted bucket name and a multi-segment key.
            case: "a dotted bucket and a deep key",
            request: json!({
                "cacheS3Location": "s3://omics.emulator.cache/runs/cohort/2026/",
            }),
            expected: json!({ "cacheS3Uri": "s3://omics.emulator.cache/runs/cohort/2026/" }),
            absent: vec!["name", "description", "tags"],
        },
        CacheCase {
            case: "a bucket owned by another account",
            request: json!({
                "cacheS3Location": CACHE_LOCATION,
                "cacheBucketOwnerId": "123456789012",
            }),
            expected: json!({
                "cacheS3Uri": CACHE_LOCATION,
                "cacheBucketOwnerId": "123456789012",
            }),
            absent: vec!["name", "description", "tags"],
        },
        CacheCase {
            case: "one tag",
            request: json!({ "cacheS3Location": CACHE_LOCATION, "tags": tag_map(1) }),
            expected: json!({ "cacheS3Uri": CACHE_LOCATION, "tags": tag_map(1) }),
            absent: vec!["name", "description"],
        },
        CacheCase {
            case: "tags at the documented bound",
            request: json!({ "cacheS3Location": CACHE_LOCATION, "tags": tag_map(MAX_TAGS) }),
            expected: json!({ "cacheS3Uri": CACHE_LOCATION, "tags": tag_map(MAX_TAGS) }),
            absent: vec!["name", "description"],
        },
        CacheCase {
            case: "every optional member at once",
            request: json!({
                "cacheS3Location": "s3://omics-emulator-cache/everything/",
                "name": "everything named",
                "description": "every optional member",
                "cacheBehavior": "CACHE_ALWAYS",
                "cacheBucketOwnerId": "999999999999",
                "tags": tag_map(3),
            }),
            expected: json!({
                "name": "everything named",
                "description": "every optional member",
                "cacheS3Uri": "s3://omics-emulator-cache/everything/",
                "cacheBehavior": "CACHE_ALWAYS",
                "cacheBucketOwnerId": "999999999999",
                "tags": tag_map(3),
            }),
            absent: vec![],
        },
    ];
    // One row per value the modelled enum admits, so the whole catalog is exercised and
    // not only the default.
    for behavior in CACHE_BEHAVIORS {
        cases.push(CacheCase {
            case: behavior,
            request: json!({ "cacheS3Location": CACHE_LOCATION, "cacheBehavior": behavior }),
            expected: json!({ "cacheS3Uri": CACHE_LOCATION, "cacheBehavior": behavior }),
            absent: vec!["name", "description", "tags"],
        });
    }
    cases
}

/// The request body for a row: its own members plus the `requestId` the row's `case` names
/// it by.
fn request_for(case: &CacheCase) -> Value {
    let mut body = case.request.as_object().cloned().unwrap_or_default();
    body.insert(
        "requestId".to_string(),
        Value::String(format!("cache-{}", case.case)),
    );
    Value::Object(body)
}

/// Create a cache that must succeed, and return its id.
async fn cache_for_runs(router: &MultiServiceRouter, request_id: &str, extra: &Value) -> String {
    let mut body = Map::new();
    body.insert("cacheS3Location".to_string(), json!(CACHE_LOCATION));
    body.insert("requestId".to_string(), json!(request_id));
    for (member, value) in extra.as_object().expect("extra members are an object") {
        body.insert(member.clone(), value.clone());
    }
    let (status, response) = create_cache(router, &Value::Object(body)).await;
    created_id(request_id, status, &response)
}

#[tokio::test]
async fn a_created_run_cache_reads_back_from_state() {
    let router = emulator();
    let cases = cache_cases();

    // Create them all first, so each read-back also proves the store keeps the rows apart
    // rather than answering with the most recent one.
    let mut created = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for cache in &cases {
        let (status, body) = create_cache(&router, &request_for(cache)).await;
        let id = created_id(cache.case, status, &body);
        assert_run_cache_id(cache.case, &id);
        let arn = body.get("arn").and_then(Value::as_str).unwrap_or_default();
        assert_arn(cache.case, arn, "runCache", &id);
        assert_eq!(
            body.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {:?}: no RunCacheStatus value is transient, so a created cache must \
             report ACTIVE, got {body:?}",
            cache.case
        );
        // The create models exactly four members; the rest are read through GetRunCache.
        assert_absent(cache.case, NOT_ON_THE_CREATE_RESPONSE, &body);
        assert_eq!(
            body.get("tags"),
            cache.expected.get("tags"),
            "case {:?}: the create must echo the tags it stored, got {body:?}",
            cache.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {:?}: id {id} was already handed out",
            cache.case
        );
        created.push((id, arn.to_string()));
    }

    for (cache, (id, arn)) in cases.iter().zip(&created) {
        let (status, body) = get_cache(&router, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRunCache, got {body:?}",
            cache.case
        );
        assert_timestamp(cache.case, "creationTime", &body);
        assert_members(cache.case, "GetRunCache", &cache.expected, &body);
        assert_absent(cache.case, &cache.absent, &body);
        assert_members(
            cache.case,
            "GetRunCache",
            &json!({ "id": id, "arn": arn, "status": "ACTIVE" }),
            &body,
        );
        // Reading does not move the cache or restamp it: a second read is identical.
        let (_, again) = get_cache(&router, id).await;
        assert_eq!(
            again, body,
            "case {:?}: a second read must report the same record, got {again:?}",
            cache.case
        );
        let status = body.get("status").and_then(Value::as_str).unwrap_or("");
        assert!(
            RUN_CACHE_STATUSES.contains(&status),
            "case {:?}: expected a modelled RunCacheStatus, got {status:?}",
            cache.case
        );
    }
}

#[tokio::test]
async fn a_replayed_request_id_returns_the_same_run_cache() {
    let router = emulator();
    // The rule generalises over the token's text and over how much of the rest of the
    // request changes on the replay: the token alone decides.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "identical replay",
            json!({ "requestId": "cache-1", "cacheS3Location": CACHE_LOCATION }),
            json!({ "requestId": "cache-1", "cacheS3Location": CACHE_LOCATION }),
        ),
        (
            "replay with different members",
            json!({
                "requestId": "cache-2",
                "cacheS3Location": CACHE_LOCATION,
                "name": "beta",
            }),
            json!({
                "requestId": "cache-2",
                "cacheS3Location": "s3://omics-emulator-cache/other/",
                "name": "beta-renamed",
                "cacheBehavior": "CACHE_ALWAYS",
            }),
        ),
        (
            "replay of a token that is all punctuation",
            json!({ "requestId": "::cache-3::", "cacheS3Location": CACHE_LOCATION }),
            json!({ "requestId": "::cache-3::", "cacheS3Location": CACHE_LOCATION }),
        ),
    ];

    let mut first = Vec::new();
    for (case, body, _) in &rows {
        let (status, response) = create_cache(&router, body).await;
        let id = created_id(case, status, &response);
        let (_, record) = get_cache(&router, &id).await;
        first.push((id, record));
    }

    for ((case, _, replay), (expected_id, expected_record)) in rows.iter().zip(&first) {
        let (status, body) = create_cache(&router, replay).await;
        let id = created_id(case, status, &body);
        assert_eq!(
            &id, expected_id,
            "case {case:?}: a replayed requestId must answer with the original cache id, \
             got {body:?}"
        );
        // Nothing the replay carried reached the stored record either.
        let (_, record) = get_cache(&router, &id).await;
        assert_eq!(
            &record, expected_record,
            "case {case:?}: a replayed requestId must leave the stored cache untouched, \
             got {record:?}"
        );
    }

    // A different token is a different cache, even when every other member is identical.
    let one = cache_for_runs(&router, "distinct-a", &json!({})).await;
    let two = cache_for_runs(&router, "distinct-b", &json!({})).await;
    assert_ne!(
        one, two,
        "two creates carrying different requestIds must be two caches, got {one} twice"
    );
}

#[tokio::test]
async fn an_unknown_run_cache_is_not_found() {
    let router = emulator();
    // One real cache exists, so the 404s below are about the id, not about an empty
    // registry.
    let existing = cache_for_runs(&router, "the only cache", &json!({})).await;

    // The rule generalises over the id's width and value, across the whole modelled
    // `RunCacheId` length (1..18): anything the registry does not hold is not found,
    // however it is shaped.
    let (min, max) = RUN_CACHE_ID_WIDTH;
    let ids = [
        "0".repeat(min),
        "9".repeat(min),
        "7777777".to_string(),
        "1".repeat(max),
        "9".repeat(max),
    ];
    for id in &ids {
        assert_ne!(id, &existing, "the fixture must not collide with the real id");
        let (status, body) = get_cache(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from GetRunCache, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn too_many_tags_on_a_run_cache_are_refused() {
    let router = emulator();
    // The bound is MAX_TAGS, documented on the `tags` member of the tag-carrying creates
    // in this service, so the row at the bound is accepted and the row one past it is
    // refused.
    for (case, count, expect_ok) in [
        ("at the tag bound", MAX_TAGS, true),
        ("one past the tag bound", MAX_TAGS + 1, false),
    ] {
        let (status, body) = create_cache(
            &router,
            &json!({
                "requestId": format!("tags-{count}"),
                "cacheS3Location": CACHE_LOCATION,
                "tags": tag_map(count),
            }),
        )
        .await;
        if expect_ok {
            assert_eq!(status, 201, "case {case:?}: expected 201, got {body:?}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_run_binds_to_the_run_cache_it_names() {
    let router = emulator();
    let workflow_id = workflow_for_runs(&router, "cached").await;
    // One cache per modelled behavior, so the inheritance below is asserted over the
    // whole catalog and not over one value.
    let mut caches = Vec::new();
    for behavior in CACHE_BEHAVIORS {
        let id = cache_for_runs(
            &router,
            &format!("run-cache-{behavior}"),
            &json!({ "cacheBehavior": behavior }),
        )
        .await;
        caches.push((*behavior, id));
    }

    for (behavior, cache_id) in &caches {
        // The rule generalises over whether the run overrides the cache's default
        // behavior, and over how the id is spelled: `crate::state`'s lookup rule reads a
        // `RunCacheId` as the numeric key of the record (`numeric_id`), the same rule
        // every other resource in this crate is looked up by, so `00<id>` and `<id>` name
        // one cache. The model's `^[0-9]+$` pattern says nothing about leading zeros.
        let rows: Vec<(String, Value, &str)> = vec![
            (
                format!("{behavior}: the cache's own default"),
                json!({ "cacheId": cache_id }),
                behavior,
            ),
            (
                format!("{behavior}: an id spelled with leading zeros"),
                json!({ "cacheId": format!("00{cache_id}") }),
                behavior,
            ),
            (
                format!("{behavior}: a run that overrides the behavior"),
                json!({ "cacheId": cache_id, "cacheBehavior": "CACHE_ALWAYS" }),
                "CACHE_ALWAYS",
            ),
            (
                format!("{behavior}: a run that overrides it the other way"),
                json!({ "cacheId": cache_id, "cacheBehavior": "CACHE_ON_FAILURE" }),
                "CACHE_ON_FAILURE",
            ),
        ];
        for (case, extra, expected_behavior) in &rows {
            let body = start_run_body(&workflow_id, &format!("run-{case}"), extra);
            let (status, response) = start_run(&router, &body).await;
            let run_id = created_id(case, status, &response);
            let (status, run) = get_run(&router, &run_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetRun, got {run:?}"
            );
            assert_members(
                case,
                "GetRun",
                // The CANONICAL cache id, not the text the request spelled it with.
                &json!({ "cacheId": cache_id, "cacheBehavior": expected_behavior }),
                &run,
            );
        }
    }

    // A `cacheId` no cache holds is the modelled 404, across the same id class.
    for id in ["0", "7777777", &"9".repeat(RUN_CACHE_ID_WIDTH.1)] {
        let body = start_run_body(
            &workflow_id,
            &format!("unheld-cache-{id}"),
            &json!({ "cacheId": id }),
        );
        let (status, response) = start_run(&router, &body).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from StartRun, got {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }

    // A run that names no cache carries neither member: nothing is invented for it.
    let body = start_run_body(&workflow_id, "no-cache-at-all", &json!({}));
    let (status, response) = start_run(&router, &body).await;
    let run_id = created_id("no cache at all", status, &response);
    let (_, run) = get_run(&router, &run_id).await;
    assert_absent("no cache at all", &["cacheId", "cacheBehavior"], &run);
}
