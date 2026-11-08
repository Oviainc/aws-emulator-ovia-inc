// SPDX-License-Identifier: CC0-1.0

//! The CACHING contract of a distribution's edge over a private Amazon S3 origin, end to
//! end over HTTP across the two services on one engine.
//!
//! RULE: an answer the edge fetched from the origin is HELD under the cache policy the
//! matching cache behaviour names — `CachePolicyConfig$DefaultTTL` is "the object's time to
//! live (TTL) only when the origin does *not* send `Cache-Control` or `Expires` headers
//! with the object", and `MinTTL` is the least time an object stays in the cache — so while
//! that TTL has not run out the same viewer request is answered from the cache, and
//! overwriting the object AT THE ORIGIN does not change what a viewer is served. The thing
//! that does is an INVALIDATION, and an invalidation is ASYNCHRONOUS: `CreateInvalidation`
//! answers with a status that is not `Completed`, the cached object is still served while
//! the batch has not finished, and the objects its paths name leave the cache when it
//! reports `Completed` — only after which the next viewer request reaches the origin again
//! and is served the NEW bytes.
//!
//! Only a cross-service suite can pin that: what a viewer is served has to change AT THE
//! ORIGIN between two viewer requests, which needs a real S3 bucket behind the
//! distribution. The cloudfront crate's own `edge_data_plane` suites pin the cache outcome
//! (`X-Cache`) and the invalidation's own asynchrony against an origin whose bytes never
//! change, and this suite is what states that the BYTES a viewer gets follow the same rule.
//!
//! The rows span what the rule generalises over: the shape of the invalidation batch (the
//! object's exact path, a wildcard over the whole distribution, a wildcard over the
//! object's own prefix, a batch naming several paths one of which matches, and a batch
//! naming a DIFFERENT object — which refreshes nothing), the key form of the object (a bare
//! name, a nested prefix) and the TTLs the cache policy publishes (a policy naming all
//! three, and one naming only the minimum).
//!
//! The harness is [`super::cloudfront_oac_s3_origin_edge`]'s, reused rather than repeated:
//! the same two routers (setup calls through a router with request authentication off, so
//! each carries a credential scope without a computed signature; VIEWER requests through an
//! authenticating router and carrying no credential at all, as a browser reaches a
//! distribution), the same private bucket, and the same origin access control — so what is
//! new here is the caching, not the way the services are reached.

use std::time::Duration;

use bytes::Bytes;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::PROPAGATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::cloudfront_oac_s3_origin_edge::{
    assert_deployed, call, create_bucket, create_distribution, create_origin_access_control,
    emulator, policy, put_bucket_policy, put_object, view, Answer, API_VERSION, CLOUDFRONT_HOST,
    CLOUDFRONT_PRINCIPAL,
};

/// `X-Cache` for an answer the edge cache held.
const HIT: &str = "Hit from cloudfront";

/// `X-Cache` for an answer the origin produced for this request.
const MISS: &str = "Miss from cloudfront";

/// Long enough past a propagation deadline that the clock owes the transition, whatever
/// the span the service chose: a distribution's deployment and an invalidation's
/// completion are both owed at [`PROPAGATION_DELAY`].
fn past_propagation() -> Duration {
    PROPAGATION_DELAY + Duration::from_millis(250)
}

/// The `CachePolicyConfig` a client submits, in the model's member declaration order
/// (`smithy_query(query="shape",shape="CachePolicyConfig")`): a TTL the row leaves out is
/// left out of the document entirely, and the cache key carries no header, cookie or query
/// string at all.
fn cache_policy_xml(
    name: &str,
    min_ttl: i64,
    default_ttl: Option<i64>,
    max_ttl: Option<i64>,
) -> String {
    let mut out =
        format!("<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">");
    out.push_str(&format!("<Name>{name}</Name>"));
    if let Some(default_ttl) = default_ttl {
        out.push_str(&format!("<DefaultTTL>{default_ttl}</DefaultTTL>"));
    }
    if let Some(max_ttl) = max_ttl {
        out.push_str(&format!("<MaxTTL>{max_ttl}</MaxTTL>"));
    }
    out.push_str(&format!("<MinTTL>{min_ttl}</MinTTL>"));
    out.push_str(
        "<ParametersInCacheKeyAndForwardedToOrigin>\
         <EnableAcceptEncodingGzip>false</EnableAcceptEncodingGzip>\
         <HeadersConfig><HeaderBehavior>none</HeaderBehavior></HeadersConfig>\
         <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
         <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior></QueryStringsConfig>\
         </ParametersInCacheKeyAndForwardedToOrigin>",
    );
    out.push_str("</CachePolicyConfig>");
    out
}

/// `cloudfront:CreateCachePolicy`, answering the policy's id.
async fn create_cache_policy(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    min_ttl: i64,
    default_ttl: Option<i64>,
    max_ttl: Option<i64>,
) -> String {
    let created = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/cache-policy"),
        &[("content-type", "application/xml")],
        Bytes::from(cache_policy_xml(name, min_ttl, default_ttl, max_ttl)),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateCachePolicy must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let id = created.element("Id");
    assert!(
        !id.is_empty(),
        "case {case:?}: the created cache policy reports no Id: {:?}",
        created.text()
    );
    id
}

/// `cloudfront:CreateInvalidation` for `paths` under `distribution`.
async fn create_invalidation(
    router: &MultiServiceRouter,
    distribution: &str,
    caller_reference: &str,
    paths: &[&str],
) -> Answer {
    let items: String = paths.iter().map(|path| format!("<Path>{path}</Path>")).collect();
    let body = format!(
        "<InvalidationBatch xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Paths><Quantity>{}</Quantity><Items>{items}</Items></Paths>\
         <CallerReference>{caller_reference}</CallerReference></InvalidationBatch>",
        paths.len()
    );
    call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/distribution/{distribution}/invalidation"),
        &[("content-type", "application/xml")],
        Bytes::from(body),
    )
    .await
}

/// `cloudfront:GetInvalidation`.
async fn get_invalidation(
    router: &MultiServiceRouter,
    distribution: &str,
    invalidation: &str,
) -> Answer {
    call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "GET",
        &format!("/{API_VERSION}/distribution/{distribution}/invalidation/{invalidation}"),
        &[],
        Bytes::new(),
    )
    .await
}

/// One row: a private bucket holding one object, a distribution over it under an origin
/// access control and a cache policy that CACHES, and the invalidation batch submitted
/// after the object is overwritten at the origin.
struct Row {
    case: &'static str,
    /// The bucket, which is this row's own so one row's cache cannot answer another's
    /// request.
    bucket: &'static str,
    /// The object's key, and so the viewer path: a bare name or a nested prefix.
    key: &'static str,
    /// The bytes the object is first written with, and what the origin fetch serves.
    first: &'static str,
    /// The bytes the object is OVERWRITTEN with, which only an invalidation can surface.
    second: &'static str,
    /// `MinTTL`, the one TTL the model makes required.
    min_ttl: i64,
    /// `DefaultTTL` and `MaxTTL`, each left out of the document when the row names none.
    default_ttl: Option<i64>,
    max_ttl: Option<i64>,
    /// The paths the invalidation batch names.
    paths: &'static [&'static str],
    /// Whether the batch names this row's object, and so whether the completed
    /// invalidation refreshes it.
    refreshed: bool,
}

/// The table: one row per shape of invalidation batch, key form and published TTL set.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the object's exact path, under a policy naming all three TTLs",
            bucket: "edge-refresh-exact",
            key: "page.txt",
            first: "first revision",
            second: "second revision",
            min_ttl: 300,
            default_ttl: Some(3_600),
            max_ttl: Some(86_400),
            paths: &["/page.txt"],
            refreshed: true,
        },
        Row {
            case: "a wildcard over the whole distribution, for a nested key",
            bucket: "edge-refresh-wildcard",
            key: "assets/site/index.html",
            first: "<p>one</p>",
            second: "<p>two</p>",
            min_ttl: 60,
            default_ttl: Some(600),
            max_ttl: Some(3_600),
            paths: &["/*"],
            refreshed: true,
        },
        Row {
            case: "a wildcard over the object's own prefix",
            bucket: "edge-refresh-prefix",
            key: "assets/site/style.css",
            first: "body{color:#111}",
            second: "body{color:#222}",
            min_ttl: 300,
            default_ttl: Some(3_600),
            max_ttl: Some(86_400),
            paths: &["/assets/*"],
            refreshed: true,
        },
        Row {
            case: "a batch naming several paths, one of which matches",
            bucket: "edge-refresh-many",
            key: "page.txt",
            first: "many: first",
            second: "many: second",
            min_ttl: 300,
            default_ttl: Some(3_600),
            max_ttl: Some(86_400),
            paths: &["/elsewhere.txt", "/page.txt", "/assets/*"],
            refreshed: true,
        },
        Row {
            case: "a batch naming a different object, which refreshes nothing",
            bucket: "edge-refresh-other",
            key: "page.txt",
            first: "other: first",
            second: "other: second",
            min_ttl: 300,
            default_ttl: Some(3_600),
            max_ttl: Some(86_400),
            paths: &["/elsewhere.txt"],
            refreshed: false,
        },
        Row {
            case: "a policy naming only the minimum TTL",
            bucket: "edge-refresh-min-only",
            key: "page.txt",
            first: "min only: first",
            second: "min only: second",
            min_ttl: 900,
            default_ttl: None,
            max_ttl: None,
            paths: &["/page.txt"],
            refreshed: true,
        },
    ]
}

#[tokio::test]
async fn an_invalidation_is_what_refreshes_what_the_edge_serves() {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    // The setup plane: authentication off, so each call carries a credential scope without a
    // computed signature. The VIEWER plane is the authenticating one, which is what makes the
    // signature CloudFront puts on its origin request the thing S3 serves the fetch for.
    let setup = emulator(&engine, false);
    let viewers = emulator(&engine, true);

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        create_bucket(&setup, case, row.bucket).await;
        put_object(&setup, case, row.bucket, row.key, row.first, &[]).await;
        let cache_policy = create_cache_policy(
            &setup,
            case,
            &format!("edge-refresh-policy-{index}"),
            row.min_ttl,
            row.default_ttl,
            row.max_ttl,
        )
        .await;
        let control = create_origin_access_control(
            &setup,
            case,
            &format!("edge-refresh-oac-{index}"),
            "always",
        )
        .await;
        let (id, domain, arn) = create_distribution(
            &setup,
            case,
            &format!("edge-refresh-reference-{index}"),
            row.bucket,
            &control,
            &cache_policy,
        )
        .await;
        // Written AFTER the distribution exists, because it is scoped to that distribution's
        // own ARN — the order a client has to follow.
        put_bucket_policy(
            &setup,
            case,
            row.bucket,
            &policy(
                CLOUDFRONT_PRINCIPAL,
                "s3:GetObject",
                &format!("arn:aws:s3:::{}/*", row.bucket),
                Some(&format!(
                    "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
                )),
            ),
        )
        .await;

        ::tokio::time::sleep(past_propagation()).await;
        assert_deployed(&setup, case, &id).await;

        let path = format!("/{}", row.key);

        // The first viewer request is the ORIGIN's answer: nothing is cached yet.
        let fetched = view(&viewers, &domain, &path).await;
        assert_eq!(
            (fetched.status, fetched.text().as_str()),
            (200, row.first),
            "case {case:?}: expected the origin's object through the edge, got {} with {:?}",
            fetched.status,
            fetched.text()
        );
        assert_eq!(
            fetched.header("x-cache"),
            MISS,
            "case {case:?}: expected the first request to reach the origin, got X-Cache {:?}",
            fetched.header("x-cache")
        );

        // The second is the CACHE's: the policy's TTL has not run out.
        let cached = view(&viewers, &domain, &path).await;
        assert_eq!(
            (cached.text().as_str(), cached.header("x-cache").as_str()),
            (row.first, HIT),
            "case {case:?}: expected the cached object, got {:?} with X-Cache {:?}",
            cached.text(),
            cached.header("x-cache")
        );

        // An ORIGIN WRITE alone changes nothing a viewer sees: the object is still held.
        put_object(&setup, case, row.bucket, row.key, row.second, &[]).await;
        let stale = view(&viewers, &domain, &path).await;
        assert_eq!(
            (stale.text().as_str(), stale.header("x-cache").as_str()),
            (row.first, HIT),
            "case {case:?}: an origin write alone must not change what the edge serves, got \
             {:?} with X-Cache {:?}",
            stale.text(),
            stale.header("x-cache")
        );

        // The invalidation is ASYNCHRONOUS: it is accepted unfinished.
        let created = create_invalidation(
            &setup,
            &id,
            &format!("edge-refresh-invalidation-{index}"),
            row.paths,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateInvalidation must succeed, got {} with {:?}",
            created.status,
            created.text()
        );
        let created_status = created.element("Status");
        assert_ne!(
            created_status, "Completed",
            "case {case:?}: an invalidation is asynchronous, so the create cannot report it \
             finished: {:?}",
            created.text()
        );
        let invalidation_id = created.element("Id");
        assert!(
            !invalidation_id.is_empty(),
            "case {case:?}: the created invalidation reports no Id: {:?}",
            created.text()
        );

        // While it has not finished, the cached object is STILL what a viewer is served.
        let during = view(&viewers, &domain, &path).await;
        assert_eq!(
            (during.text().as_str(), during.header("x-cache").as_str()),
            (row.first, HIT),
            "case {case:?}: an invalidation reporting {created_status:?} must not have \
             refreshed anything yet, got {:?} with X-Cache {:?}",
            during.text(),
            during.header("x-cache")
        );

        ::tokio::time::sleep(past_propagation()).await;
        let read = get_invalidation(&setup, &id, &invalidation_id).await;
        assert_eq!(
            read.element("Status"),
            "Completed",
            "case {case:?}: the invalidation did not reach Completed: {:?}",
            read.text()
        );

        // Only now does the batch decide: an object its paths name is fetched from the
        // origin again and serves the NEW bytes, and one they do not name is still held.
        let after = view(&viewers, &domain, &path).await;
        let expected = if row.refreshed {
            (row.second, MISS)
        } else {
            (row.first, HIT)
        };
        assert_eq!(
            (after.text().as_str(), after.header("x-cache").as_str()),
            expected,
            "case {case:?}: after the invalidation completed, expected {expected:?}, got \
             {:?} with X-Cache {:?}",
            after.text(),
            after.header("x-cache")
        );
    }
}
