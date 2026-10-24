// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `FlushStageAuthorizersCache`: dropping every authorizer cache
//! entry of one stage of a REST API
//! (`DELETE /restapis/{restApiId}/stages/{stageName}/cache/authorizers`).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The emulator, the request builder, the `CreateRestApi`/`CreateDeployment`
//! helpers, the settled-stage builder and the authorizer helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle`, `stage_lifecycle`,
//! `flush_stage_cache` and `authorizer_lifecycle`.
//!
//! The rule under test, over a TABLE of stages and authorizer sets rather than one
//! recorded call: a flush of a stage's authorizer cache is accepted (202, no body) for
//! every stage that exists — whatever authorizers the API holds and whether or not the
//! stage has a cache cluster — invalidates the cached authorizer results of ONLY the
//! addressed stage (each flush advancing that stage's invalidation watermark), deletes no
//! authorizer and changes no member the stage reports (its `cacheClusterStatus`
//! included, which is what the sibling cache-DATA flush moves), and is a 404
//! `NotFoundException`, writing nothing, when either path label names no record.
//!
//! The watermark is read from the store rather than off the wire because `Stage` models
//! no member for the authorizer cache: API Gateway reports none either, so there is
//! nothing on the wire to read it from.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    service_state, CACHE_CLUSTER_AVAILABLE, CACHE_CLUSTER_NOT_AVAILABLE,
    DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS, MAX_AUTHORIZER_RESULT_TTL_SECONDS,
    MIN_AUTHORIZER_RESULT_TTL_SECONDS,
};

use crate::authorizer_lifecycle::{created_authorizer, get_authorizer, lambda_uri, user_pool_arn};
use crate::flush_stage_cache::{cluster_status, settled_stage};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// Long enough for the monotonic clock to move between two calls, so "this flush
/// invalidated later than the last one" is observable whatever the platform clock's
/// granularity. It waits out NO span of the service: a flush is due at once.
const CLOCK_TICK: Duration = Duration::from_millis(5);

/// `FlushStageAuthorizersCache` for `stage_name` of `api`.
async fn flush_authorizers_cache(gateway: &Gateway, api: &str, stage_name: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/stages/{stage_name}/cache/authorizers"),
            None,
        )
        .await
}

/// `FlushStageCache` for `stage_name` of `api`: the sibling flush, called in one case to
/// show the two addresses move different records.
async fn flush_cache_data(gateway: &Gateway, api: &str, stage_name: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/stages/{stage_name}/cache/data"),
            None,
        )
        .await
}

/// The stage's authorizer-cache invalidation watermark as the STORE holds it: `None`
/// until the stage's authorizer cache is flushed at all. Read from the engine the
/// emulator is mounted on, because no operation of this service reports it.
fn authorizers_cache_watermark(gateway: &Gateway, api: &str, stage: &str) -> Option<Instant> {
    let state = service_state(gateway.engine())
        .expect("the service is mounted on the shared in-memory engine");
    let store = state.lock();
    store
        .rest_apis
        .get(api)
        .and_then(|stored| stored.stages.get(stage))
        .and_then(|stage| stage.authorizers_cache_flushed_at)
}

/// Every authorizer of `api`, by the id its create answered, read back through
/// `GetAuthorizer`: what a flush of cached RESULTS must leave untouched.
async fn authorizers_by_id(
    gateway: &Gateway,
    api: &str,
    bodies: &[Value],
    case: &str,
) -> Vec<Value> {
    let mut read_back = Vec::new();
    for body in bodies {
        let id = string_member(body, "id", case);
        let read = get_authorizer(gateway, api, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetAuthorizer({id}), got {} with {:?}",
            read.status, read.body
        );
        read_back.push(read.body);
    }
    read_back
}

/// A TOKEN authorizer request carrying every member that type requires, so a row that
/// varies the TTL varies only the TTL.
fn token_request(name: &str, ttl: Option<i32>) -> Value {
    let mut request = json!({
        "name": name,
        "type": "TOKEN",
        "identitySource": "method.request.header.Auth",
        "authorizerUri": lambda_uri("authorize"),
    });
    if let (Some(object), Some(ttl)) = (request.as_object_mut(), ttl) {
        object.insert("authorizerResultTtlInSeconds".to_string(), json!(ttl));
    }
    request
}

#[tokio::test]
async fn flushing_a_stage_authorizers_cache_invalidates_only_that_stage_and_changes_nothing_else() {
    // Rows differ along what the rule generalises over: how many authorizers the API
    // holds and of which `AuthorizerType`, the cached-result TTL each carries (the
    // default the member documents, its published minimum — where caching is disabled —
    // and its published maximum, from
    // `smithy_query(query="doc", shape="com.amazonaws.apigateway#Authorizer",
    // member="authorizerResultTtlInSeconds")`), whether the stage has a cache cluster at
    // all, and which other stage members the record carries.
    let cases: Vec<(&str, &str, Value, Vec<Value>, &str)> = vec![
        (
            "no authorizers and no cache cluster",
            "plain",
            json!({}),
            vec![],
            CACHE_CLUSTER_NOT_AVAILABLE,
        ),
        (
            "one TOKEN authorizer at the documented default TTL",
            "token",
            json!({
                "description": "the documented default",
                "variables": { "table": "pets" },
                "tags": { "team": "api" }
            }),
            vec![token_request("token-default", None)],
            CACHE_CLUSTER_NOT_AVAILABLE,
        ),
        (
            "one REQUEST authorizer with caching disabled at the published minimum TTL",
            "uncached",
            json!({}),
            vec![json!({
                "name": "request-uncached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "authorizerResultTtlInSeconds": MIN_AUTHORIZER_RESULT_TTL_SECONDS,
            })],
            CACHE_CLUSTER_NOT_AVAILABLE,
        ),
        (
            "three authorizers spanning the type catalog, on a stage with a cache cluster",
            "cached",
            json!({
                "cacheClusterEnabled": true,
                "cacheClusterSize": "0.5",
                "tracingEnabled": true
            }),
            vec![
                token_request("token-short", Some(MIN_AUTHORIZER_RESULT_TTL_SECONDS + 1)),
                json!({
                    "name": "request-cached",
                    "type": "REQUEST",
                    "authorizerUri": lambda_uri("request"),
                    "identitySource": "method.request.header.Auth,method.request.querystring.Name",
                    "authorizerResultTtlInSeconds": DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS,
                }),
                json!({
                    "name": "cognito-max",
                    "type": "COGNITO_USER_POOLS",
                    "identitySource": "method.request.header.Authorization",
                    "providerARNs": [user_pool_arn("us-east-1_pool")],
                    "authorizerResultTtlInSeconds": MAX_AUTHORIZER_RESULT_TTL_SECONDS,
                }),
            ],
            CACHE_CLUSTER_AVAILABLE,
        ),
    ];

    for (case, stage_name, extra, authorizer_requests, expected_cluster) in &cases {
        // One API per row, so the authorizers of one row are not the authorizers of the
        // next: an authorizer belongs to an API, not to a stage.
        let gateway = Gateway::new();
        let (api, _) = create_api(&gateway, &format!("flush-authorizers-{stage_name}")).await;
        let deployment = create_deployment(
            &gateway,
            &api,
            &format!("CreateDeployment for {case}"),
            json!({}),
        )
        .await;
        let mut created = Vec::new();
        for request in authorizer_requests {
            created.push(created_authorizer(&gateway, &api, case, request.clone()).await);
        }

        let before = settled_stage(&gateway, &api, &deployment, case, stage_name, extra).await;
        let authorizers_before = authorizers_by_id(&gateway, &api, &created, case).await;
        assert_eq!(
            authorizers_cache_watermark(&gateway, &api, stage_name),
            None,
            "case {case}: a stage nobody flushed carries no authorizer-cache watermark"
        );

        let flushed = flush_authorizers_cache(&gateway, &api, stage_name).await;
        assert_eq!(
            flushed.status, 202,
            "case {case}: expected 202 from FlushStageAuthorizersCache, got {} with {:?}",
            flushed.status, flushed.body
        );
        assert_eq!(
            flushed.body,
            Value::Null,
            "case {case}: FlushStageAuthorizersCache answers no body, got {:?}",
            flushed.body
        );

        // The flush invalidated this stage's cached authorizer results: the store now
        // carries the watermark the call stamped.
        let first_mark = authorizers_cache_watermark(&gateway, &api, stage_name)
            .unwrap_or_else(|| {
                panic!("case {case}: expected the flush to be recorded on the stage")
            });

        // It is not a stage update and not the cache-DATA flush, so every member a reader
        // sees — the cluster status included — reads back exactly as it did before.
        let after = get_stage(&gateway, &api, stage_name).await;
        assert_eq!(
            after.status, 200,
            "case {case}: expected 200 from GetStage after the flush, got {} with {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.body, before,
            "case {case}: the authorizers-cache flush must change no reported member, \
             expected {before:?}, got {:?}",
            after.body
        );
        let status = cluster_status(&after.body, case);
        assert_eq!(
            status, *expected_cluster,
            "case {case}: expected the cluster to stay {expected_cluster}, got {status}"
        );

        // Cached RESULTS are dropped, the authorizers that produce them are not.
        let authorizers_after = authorizers_by_id(&gateway, &api, &created, case).await;
        assert_eq!(
            authorizers_after, authorizers_before,
            "case {case}: the flush must delete no authorizer, expected \
             {authorizers_before:?}, got {authorizers_after:?}"
        );

        // A repeated flush is accepted too, and it is a FRESH invalidation: the watermark
        // advances, so a result cached between the two calls is a miss as well. The pause
        // is the platform clock's, not the service's: two calls must land on distinct
        // instants for "advances" to be observable at all.
        tokio::time::sleep(CLOCK_TICK).await;
        let again = flush_authorizers_cache(&gateway, &api, stage_name).await;
        assert_eq!(
            again.status, 202,
            "case {case}: expected 202 from a repeated FlushStageAuthorizersCache, got {} \
             with {:?}",
            again.status, again.body
        );
        let second_mark = authorizers_cache_watermark(&gateway, &api, stage_name)
            .unwrap_or_else(|| panic!("case {case}: expected the repeated flush to be recorded"));
        assert!(
            second_mark > first_mark,
            "case {case}: a repeated flush advances the watermark, got {second_mark:?} \
             after {first_mark:?}"
        );
    }
}

#[tokio::test]
async fn a_flush_invalidates_only_the_stage_it_addresses() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "flush-authorizers-one-stage").await;
    let (other_api, _) = create_api(&gateway, "flush-authorizers-one-stage-other").await;
    let deployment = create_deployment(
        &gateway,
        &api,
        "CreateDeployment for the targets",
        json!({}),
    )
    .await;
    let other_deployment = create_deployment(
        &gateway,
        &other_api,
        "CreateDeployment on the other API",
        json!({}),
    )
    .await;
    created_authorizer(
        &gateway,
        &api,
        "the authorizer the flushed stage caches",
        token_request("token-shared", None),
    )
    .await;

    // Two stages of one API, and the SAME stage name on another API: a flush is addressed
    // by (API, stage), so neither of the other two may be invalidated.
    let cached = json!({ "cacheClusterEnabled": true, "cacheClusterSize": "0.5" });
    for (case, stage) in [("the flushed stage", "prod"), ("a sibling stage", "beta")] {
        settled_stage(&gateway, &api, &deployment, case, stage, &cached).await;
    }
    settled_stage(
        &gateway,
        &other_api,
        &other_deployment,
        "the same stage name on another API",
        "prod",
        &cached,
    )
    .await;

    let flushed = flush_authorizers_cache(&gateway, &api, "prod").await;
    assert_eq!(
        flushed.status, 202,
        "case FlushStageAuthorizersCache(prod): expected 202, got {} with {:?}",
        flushed.status, flushed.body
    );

    for (case, target_api, stage, expected_flushed) in [
        ("the addressed stage", &api, "prod", true),
        ("the sibling stage", &api, "beta", false),
        ("the same name on another API", &other_api, "prod", false),
    ] {
        let mark = authorizers_cache_watermark(&gateway, target_api, stage);
        assert_eq!(
            mark.is_some(),
            expected_flushed,
            "case {case}: expected flushed={expected_flushed} after flushing {api}/prod, \
             got {mark:?}"
        );
    }

    // The two cache addresses are two different caches: flushing the cache DATA of the
    // sibling stage moves its cluster, not any authorizer-cache watermark, and flushing
    // the authorizer cache left the addressed stage's cluster alone.
    let data_flushed = flush_cache_data(&gateway, &api, "beta").await;
    assert_eq!(
        data_flushed.status, 202,
        "case FlushStageCache(beta): expected 202, got {} with {:?}",
        data_flushed.status, data_flushed.body
    );
    assert_eq!(
        authorizers_cache_watermark(&gateway, &api, "beta"),
        None,
        "case FlushStageCache(beta): a cache-data flush stamps no authorizer-cache watermark"
    );
    let addressed = get_stage(&gateway, &api, "prod").await;
    let status = cluster_status(&addressed.body, "the authorizers-flushed stage");
    assert_eq!(
        status, CACHE_CLUSTER_AVAILABLE,
        "case the authorizers-flushed stage: expected the cluster to stay \
         {CACHE_CLUSTER_AVAILABLE}, got {status}"
    );
}

#[tokio::test]
async fn flushing_an_authorizers_cache_the_request_cannot_resolve_is_a_not_found() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "flush-authorizers-notfound").await;
    let (other_api, _) = create_api(&gateway, "flush-authorizers-notfound-other").await;
    let deployment = create_deployment(
        &gateway,
        &api,
        "CreateDeployment for the refusals",
        json!({}),
    )
    .await;
    settled_stage(
        &gateway,
        &api,
        &deployment,
        "the stage the refusals are measured against",
        "prod",
        &json!({}),
    )
    .await;

    // Rows span each identifier the call resolves, one unresolvable at a time.
    let missing = "zzzzzzzzzz";
    let cases: Vec<(&str, String)> = vec![
        ("an unknown restApiId", format!("{missing}/prod")),
        ("an unknown stageName", format!("{api}/{missing}")),
        (
            "a stage name stored on another API",
            format!("{other_api}/prod"),
        ),
        ("both labels unknown", format!("{missing}/{missing}")),
    ];

    for (case, target) in &cases {
        let (target_api, stage) = target
            .split_once('/')
            .expect("every row names an API and a stage");
        assert_failure(
            &flush_authorizers_cache(&gateway, target_api, stage).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // A refusal writes nothing: the stage that does exist was never invalidated. The
    // write a flush makes is immediate, so there is no span to wait out here — a
    // watermark would already be there if a refused call had stamped one.
    assert_eq!(
        authorizers_cache_watermark(&gateway, &api, "prod"),
        None,
        "case the stored stage after the refusals: a refused flush invalidates nothing"
    );
}
