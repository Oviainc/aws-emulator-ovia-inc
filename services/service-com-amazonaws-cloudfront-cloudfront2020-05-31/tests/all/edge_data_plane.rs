// SPDX-License-Identifier: CC0-1.0

//! The CloudFront distribution DATA plane — the edge — end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises the claim, the
//! resolution of the distribution, the cache-behaviour selection, the enforcement
//! CloudFront performs itself and the origin fetch — the same path a viewer takes. The
//! rules under test are:
//!
//! * a DEPLOYED distribution's own domain name ANSWERS: a request to it is resolved to
//!   that distribution, by whichever of the three addressings the client used;
//! * the cache behaviour is selected by PATH PATTERN, the ordered behaviours first and the
//!   default behaviour when none matches;
//! * `AllowedMethods` is enforced BY CLOUDFRONT: a viewer method the selected behaviour
//!   does not allow is refused with `403` and `X-Cache: Error from cloudfront`, carries no
//!   origin bytes, and never reaches the origin — and the SAME method is served on a
//!   behaviour that does allow it, which is what makes the behaviour (not the origin) the
//!   thing deciding;
//! * `CachedMethods` decides what may come out of the edge cache: a cacheable method under
//!   a caching cache policy is answered `Hit from cloudfront` the second time, while a
//!   method the behaviour allows but does not cache is forwarded every time and is never a
//!   hit;
//! * an origin nothing in this emulator serves answers nothing, and that is reported as
//!   CloudFront reports an origin it could not reach: `502` with `X-Cache: Error from
//!   cloudfront` — never as a response invented for an origin that did not run;
//! * an INVALIDATION is asynchronous: it is accepted with a status that is not
//!   `Completed`, the cached object is still served while it has not finished, and the
//!   objects its paths name leave the cache when it reports `Completed`.
//!
//! ## The origin these suites point at
//!
//! An edge fetch is a real in-process HTTP request to the origin's own `DomainName`, so a
//! case that needs an origin to ANSWER names an endpoint this emulator serves: this
//! service's own control-plane host, whose `GET /2020-05-31/distribution` answers a
//! `DistributionList`. Nothing about the edge is special-cased for it — it is an origin
//! like any other, reached over the same transport an S3 bucket endpoint would be. The
//! unreachable-origin case names a Lambda function URL host, which no service of this
//! emulator claims.

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use super::distribution_lifecycle::{
    call, create_distribution, emulator, past_propagation, API_VERSION, HOST,
};

/// The AWS-managed cache policy `Managed-CachingOptimized`, whose `DefaultTTL` is a day:
/// a behaviour under it caches what it is allowed to cache. Its id is the one
/// `MANAGED_CACHE_POLICIES` publishes.
const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: a
/// behaviour under it caches nothing.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// An origin this emulator SERVES: this service's own control-plane endpoint. A viewer
/// path of `/2020-05-31/distribution` fetched from it answers a `DistributionList`.
const SERVED_ORIGIN: &str = HOST;

/// A viewer path the served origin answers with a `DistributionList`.
const SERVED_PATH: &str = "/2020-05-31/distribution";

/// An origin NOTHING in this emulator serves: the host shape of a Lambda function URL
/// (`https://<url-id>.lambda-url.<region>.on.aws`). Lambda has no in-process runtime for a
/// deployment package, so no service claims that host and no function response exists to
/// fetch.
const UNSERVED_ORIGIN: &str = "a0b1c2d3e4f5g6h7.lambda-url.us-east-1.on.aws";

/// `X-Cache` for an answer the edge cache held.
const HIT: &str = "Hit from cloudfront";

/// `X-Cache` for an answer the origin produced for this request.
const MISS: &str = "Miss from cloudfront";

/// `X-Cache` for an answer CloudFront produced itself.
const ERROR: &str = "Error from cloudfront";

/// What the edge answered one viewer request with.
struct Viewed {
    status: u16,
    x_cache: String,
    cf_id: String,
    body: String,
}

/// Send one VIEWER request: no AWS credential, addressed to `host` (which is what
/// resolves the distribution), exactly as a browser reaches a distribution.
async fn view(router: &MultiServiceRouter, method: &str, host: &str, path: &str) -> Viewed {
    view_with(router, method, host, path, &[]).await
}

/// [`view`], carrying `headers` beside the `Host` — for a rule about a header the cache
/// policy puts in the cache key.
async fn view_with(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Viewed {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let (x_cache, cf_id) = (header("x-cache"), header("x-amz-cf-id"));
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    Viewed {
        status,
        x_cache,
        cf_id,
        body,
    }
}

/// The `DistributionConfig` of the distribution these suites serve through: one origin,
/// a default behaviour that allows only `GET` and `HEAD`, and an ordered `/opt/*`
/// behaviour that also allows `OPTIONS` but caches only `GET` and `HEAD`.
///
/// This is the configuration AWS treats as a security control: a GET/HEAD-only behaviour
/// in front of a writable origin.
fn config(caller_reference: &str, origin: &str, cache_policy: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{origin}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items>\
         <CachedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></CachedMethods>\
         </AllowedMethods>\
         <CachePolicyId>{cache_policy}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
         <PathPattern>/opt/*</PathPattern><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>3</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method><Method>OPTIONS</Method></Items>\
         <CachedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></CachedMethods>\
         </AllowedMethods>\
         <CachePolicyId>{cache_policy}</CachePolicyId>\
         </CacheBehavior></Items></CacheBehaviors>\
         <Comment>edge data plane</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// Create one distribution and let the clock deploy it: its id and the domain name
/// CloudFront assigned it.
async fn deployed(
    router: &MultiServiceRouter,
    caller_reference: &str,
    origin: &str,
    cache_policy: &str,
) -> (String, String) {
    let created = create_distribution(router, &config(caller_reference, origin, cache_policy)).await;
    assert_eq!(
        created.status, 201,
        "creating the distribution answered {}: {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .find_text("Id")
        .expect("the created distribution reports an id")
        .to_string();
    let domain = created
        .root()
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    ::tokio::time::sleep(past_propagation()).await;
    let read = call(router, "GET", &format!("/{API_VERSION}/distribution/{id}"), None, None).await;
    assert_eq!(
        read.root().find_text("Status"),
        Some("Deployed"),
        "the distribution did not reach Deployed: {}",
        read.raw
    );
    (id, domain)
}

/// `AllowedMethods` is enforced at the edge, per the behaviour the viewer path selects:
/// the same method is refused on one behaviour and served on another, and a refusal is
/// CloudFront's own answer rather than anything the origin said.
#[tokio::test]
async fn the_edge_refuses_a_method_the_selected_behavior_does_not_allow() {
    let router = emulator();
    let (_id, domain) = deployed(&router, "edge-methods", SERVED_ORIGIN, CACHING_OPTIMIZED).await;

    // (case, method, viewer path, refused by CloudFront). The rows differ along the
    // method, along which behaviour the path selects — the default one and the ordered
    // `/opt/*` one — and so along whether the SAME method is allowed: `OPTIONS` is
    // refused on the default behaviour and served on `/opt/*`.
    let cases: [(&str, &str, &str, bool); 10] = [
        ("GET on the default behavior", "GET", SERVED_PATH, false),
        ("HEAD on the default behavior", "HEAD", SERVED_PATH, false),
        ("POST on the default behavior", "POST", SERVED_PATH, true),
        ("PUT on the default behavior", "PUT", SERVED_PATH, true),
        ("PATCH on the default behavior", "PATCH", SERVED_PATH, true),
        ("DELETE on the default behavior", "DELETE", SERVED_PATH, true),
        ("OPTIONS on the default behavior", "OPTIONS", SERVED_PATH, true),
        ("GET on the /opt/* behavior", "GET", "/opt/thing", false),
        ("OPTIONS on the /opt/* behavior", "OPTIONS", "/opt/thing", false),
        ("POST on the /opt/* behavior", "POST", "/opt/thing", true),
    ];

    for (case, method, path, refused) in cases {
        let answer = view(&router, method, &domain, path).await;
        assert!(
            !answer.cf_id.is_empty(),
            "case {case:?}: expected the edge to stamp an X-Amz-Cf-Id, got none (status \
             {}, body {:?})",
            answer.status,
            answer.body
        );
        if refused {
            assert_eq!(
                (answer.status, answer.x_cache.as_str()),
                (403, ERROR),
                "case {case:?}: expected CloudFront's own 403 with X-Cache {ERROR:?}, got \
                 {} with {:?} (body {:?})",
                answer.status,
                answer.x_cache,
                answer.body
            );
            assert!(
                answer.body.contains("The request could not be satisfied.")
                    && answer.body.contains("Generated by cloudfront"),
                "case {case:?}: expected CloudFront's own generated page, got {:?}",
                answer.body
            );
            assert!(
                !answer.body.contains("DistributionList"),
                "case {case:?}: the refusal carries origin bytes, so the request reached \
                 the origin: {:?}",
                answer.body
            );
        } else {
            assert_ne!(
                answer.status, 403,
                "case {case:?}: expected the behavior to serve this method, got 403 with \
                 X-Cache {:?} (body {:?})",
                answer.x_cache, answer.body
            );
            assert!(
                answer.x_cache == MISS || answer.x_cache == HIT,
                "case {case:?}: expected the answer to come from the origin or the cache, \
                 got X-Cache {:?} (status {}, body {:?})",
                answer.x_cache,
                answer.status,
                answer.body
            );
        }
    }
}

/// A method the behaviour allows is fetched from the origin and the origin's own answer is
/// what the edge serves.
#[tokio::test]
async fn an_allowed_method_is_served_from_the_origin() {
    let router = emulator();
    let (_id, domain) = deployed(&router, "edge-origin", SERVED_ORIGIN, CACHING_DISABLED).await;

    let answer = view(&router, "GET", &domain, SERVED_PATH).await;
    assert_eq!(
        (answer.status, answer.x_cache.as_str()),
        (200, MISS),
        "expected the origin's 200 with X-Cache {MISS:?}, got {} with {:?} (body {:?})",
        answer.status,
        answer.x_cache,
        answer.body
    );
    assert!(
        answer.body.contains("DistributionList"),
        "expected the ORIGIN's own bytes through the edge, got {:?}",
        answer.body
    );
}

/// `CachedMethods` and the cache policy decide what the edge cache may answer: a cacheable
/// method under a caching policy is a HIT the second time, the same method under a policy
/// that caches nothing is always a MISS, and a method the behaviour allows but does not
/// cache is forwarded every time.
#[tokio::test]
async fn the_edge_cache_answers_only_what_the_behavior_caches() {
    let router = emulator();
    let (_id, caching) = deployed(&router, "edge-caching", SERVED_ORIGIN, CACHING_OPTIMIZED).await;
    let (_id, uncached) =
        deployed(&router, "edge-uncached", SERVED_ORIGIN, CACHING_DISABLED).await;

    // (case, host, method, path, the X-Cache of each successive request)
    let cases: [(&str, &str, &str, &str, &[&str]); 4] = [
        (
            "a cached method under a caching cache policy",
            caching.as_str(),
            "GET",
            SERVED_PATH,
            &[MISS, HIT, HIT],
        ),
        (
            "a cached method under a cache policy that caches nothing",
            uncached.as_str(),
            "GET",
            SERVED_PATH,
            &[MISS, MISS],
        ),
        (
            "an allowed method the behavior does not cache",
            caching.as_str(),
            "OPTIONS",
            "/opt/thing",
            &[MISS, MISS, MISS],
        ),
        (
            "a cached method on the ordered behavior",
            caching.as_str(),
            "GET",
            "/opt/thing",
            &[MISS, HIT],
        ),
    ];

    for (case, host, method, path, expected) in cases {
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..expected.len() {
            seen.push(view(&router, method, host, path).await.x_cache);
        }
        let seen: Vec<&str> = seen.iter().map(String::as_str).collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the successive X-Cache values {expected:?}, got {seen:?}"
        );
    }
}

/// An invalidation removes the objects its paths name from the edge cache when it
/// FINISHES, not when it is accepted: the batch is created `InProgress`, the cached object
/// is still served while it is, and only once the invalidation reports `Completed` does the
/// next request for one of them go to the origin again.
#[tokio::test]
async fn an_invalidation_takes_a_cached_object_out_of_the_edge_cache_when_it_completes() {
    let router = emulator();
    let (id, domain) = deployed(&router, "edge-invalidation", SERVED_ORIGIN, CACHING_OPTIMIZED).await;

    assert_eq!(
        view(&router, "GET", &domain, SERVED_PATH).await.x_cache,
        MISS,
        "the first request for an uncached object is a miss"
    );
    assert_eq!(
        view(&router, "GET", &domain, SERVED_PATH).await.x_cache,
        HIT,
        "the second request for a cached object is a hit"
    );

    let invalidated = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution/{id}/invalidation"),
        Some(
            "<InvalidationBatch xmlns=\"http://cloudfront.amazonaws.com/doc/2020-05-31/\">\
             <Paths><Quantity>1</Quantity><Items><Path>/2020-05-31/*</Path></Items></Paths>\
             <CallerReference>edge-invalidation-1</CallerReference></InvalidationBatch>",
        ),
        None,
    )
    .await;
    assert_eq!(
        invalidated.status, 201,
        "creating the invalidation answered {}: {}",
        invalidated.status, invalidated.raw
    );
    let created_status = invalidated
        .root()
        .find_text("Status")
        .expect("the created invalidation reports a status")
        .to_string();
    assert_ne!(
        created_status, "Completed",
        "an invalidation is asynchronous, so the create cannot report it finished: {}",
        invalidated.raw
    );
    let invalidation_id = invalidated
        .root()
        .find_text("Id")
        .expect("the created invalidation reports an id")
        .to_string();

    let during = view(&router, "GET", &domain, SERVED_PATH).await;
    assert_eq!(
        during.x_cache, HIT,
        "case {created_status:?}: expected the cached object to still be served while the \
         invalidation has not finished, got X-Cache {:?}",
        during.x_cache
    );

    ::tokio::time::sleep(past_propagation()).await;
    let read = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/invalidation/{invalidation_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        read.root().find_text("Status"),
        Some("Completed"),
        "the invalidation did not reach Completed: {}",
        read.raw
    );

    let after = view(&router, "GET", &domain, SERVED_PATH).await;
    assert_eq!(
        after.x_cache, MISS,
        "expected the invalidated object to be fetched from the origin again once the \
         invalidation completed, got X-Cache {:?}",
        after.x_cache
    );
}

/// Every addressing of one distribution's edge resolves to the same distribution, so the
/// same viewer request is decided the same way however the client reached the emulator.
#[tokio::test]
async fn every_addressing_of_the_edge_resolves_the_same_distribution() {
    let router = emulator();
    let (id, domain) = deployed(&router, "edge-addressing", SERVED_ORIGIN, CACHING_DISABLED).await;

    // (case, host, path)
    let cases: [(&str, String, String); 3] = [
        (
            "host-style, the domain name CloudFront assigned",
            domain.clone(),
            SERVED_PATH.to_string(),
        ),
        (
            "the reserved data-plane path prefix, on the emulator's own endpoint",
            "127.0.0.1:9297".to_string(),
            format!("/_aws/cloudfront/{id}{SERVED_PATH}"),
        ),
        (
            "path-style, the distribution id as the first segment",
            "127.0.0.1:9297".to_string(),
            format!("/{id}{SERVED_PATH}"),
        ),
    ];

    for (case, host, path) in cases {
        let served = view(&router, "GET", &host, &path).await;
        assert_eq!(
            (served.status, served.x_cache.as_str()),
            (200, MISS),
            "case {case:?}: expected the origin's 200 with X-Cache {MISS:?}, got {} with \
             {:?} (body {:?})",
            served.status,
            served.x_cache,
            served.body
        );
        let refused = view(&router, "POST", &host, &path).await;
        assert_eq!(
            (refused.status, refused.x_cache.as_str()),
            (403, ERROR),
            "case {case:?}: expected CloudFront's own 403 with X-Cache {ERROR:?}, got {} \
             with {:?} (body {:?})",
            refused.status,
            refused.x_cache,
            refused.body
        );
    }
}

/// An origin nothing in this emulator serves answers nothing, and the edge reports that as
/// CloudFront reports an origin it could not reach — never as a response invented for an
/// origin that did not run.
#[tokio::test]
async fn an_origin_nothing_serves_is_reported_as_an_origin_that_could_not_be_reached() {
    let router = emulator();
    let (_id, domain) = deployed(&router, "edge-unserved", UNSERVED_ORIGIN, CACHING_OPTIMIZED).await;

    let answer = view(&router, "GET", &domain, "/").await;
    assert_eq!(
        (answer.status, answer.x_cache.as_str()),
        (502, ERROR),
        "expected CloudFront's own 502 with X-Cache {ERROR:?}, got {} with {:?} (body \
         {:?})",
        answer.status,
        answer.x_cache,
        answer.body
    );
    assert!(
        answer
            .body
            .contains("CloudFront wasn't able to connect to the origin."),
        "expected CloudFront's own unreachable-origin page, got {:?}",
        answer.body
    );
    assert!(
        !answer.body.contains("UnknownService"),
        "the answer carries the transport's own refusal rather than CloudFront's: {:?}",
        answer.body
    );

    // Nothing that was never fetched is cached, so a second request is answered the same
    // way rather than from the cache.
    let again = view(&router, "GET", &domain, "/").await;
    assert_eq!(
        (again.status, again.x_cache.as_str()),
        (502, ERROR),
        "expected the second request to be answered the same way, got {} with {:?}",
        again.status,
        again.x_cache
    );
}

/// A caller-supplied alternate domain name cannot make the edge claim an address this
/// emulator is itself reached at: an alias of `localhost` is stored, and a request on the
/// loopback endpoint is still not viewer traffic, while the domain CloudFront assigned
/// goes on serving.
#[tokio::test]
async fn a_caller_supplied_alias_cannot_claim_the_emulators_own_endpoint() {
    let router = emulator();
    // An alternate domain name needs a certificate for that name, which is all the create
    // requires of one; the name itself is what this case is about.
    let aliased = config("edge-alias", SERVED_ORIGIN, CACHING_DISABLED)
        .replace(
            "<CallerReference>edge-alias</CallerReference>",
            "<CallerReference>edge-alias</CallerReference>\
             <Aliases><Quantity>2</Quantity>\
             <Items><CNAME>localhost</CNAME><CNAME>127.0.0.1</CNAME></Items></Aliases>",
        )
        .replace(
            "<Enabled>true</Enabled>",
            "<Enabled>true</Enabled><ViewerCertificate>\
             <ACMCertificateArn>arn:aws:acm:us-east-1:000000000000:certificate/edge</ACMCertificateArn>\
             <SSLSupportMethod>sni-only</SSLSupportMethod>\
             <MinimumProtocolVersion>TLSv1.2_2021</MinimumProtocolVersion></ViewerCertificate>",
        );
    let created = create_distribution(&router, &aliased).await;
    assert_eq!(
        created.status, 201,
        "creating the aliased distribution answered {}: {}",
        created.status, created.raw
    );
    let domain = created
        .root()
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    ::tokio::time::sleep(past_propagation()).await;

    // (case, the host the request is addressed to, served by the edge)
    let cases: [(&str, &str, bool); 4] = [
        ("the loopback name the alias names", "localhost", false),
        ("the loopback address the alias names", "127.0.0.1:9297", false),
        ("the loopback address with no port", "127.0.0.1", false),
        ("the domain CloudFront assigned", domain.as_str(), true),
    ];
    for (case, host, served) in cases {
        let answer = view(&router, "GET", host, SERVED_PATH).await;
        let by_the_edge = !answer.x_cache.is_empty();
        assert_eq!(
            by_the_edge, served,
            "case {case:?}: expected the edge to serve this request == {served}, got \
             X-Cache {:?} with status {}",
            answer.x_cache, answer.status
        );
    }
}

/// The cache key carries the headers the behaviour's cache policy names, so two requests
/// that differ in such a header are different objects to the edge and neither is served
/// the other's answer.
#[tokio::test]
async fn the_cache_key_separates_requests_that_differ_in_a_keyed_header() {
    let router = emulator();
    // A cache policy of this account that caches for a minute and puts ONE viewer header
    // in the cache key: `CachePolicyHeadersConfig$HeaderBehavior` `whitelist`.
    let created = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/cache-policy"),
        Some(&format!(
            "<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>edge-keyed-header</Name><DefaultTTL>60</DefaultTTL><MaxTTL>60</MaxTTL>\
             <MinTTL>0</MinTTL><ParametersInCacheKeyAndForwardedToOrigin>\
             <EnableAcceptEncodingGzip>false</EnableAcceptEncodingGzip>\
             <HeadersConfig><HeaderBehavior>whitelist</HeaderBehavior>\
             <Headers><Quantity>1</Quantity><Items><Name>x-viewer</Name></Items></Headers>\
             </HeadersConfig>\
             <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
             <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior>\
             </QueryStringsConfig></ParametersInCacheKeyAndForwardedToOrigin>\
             </CachePolicyConfig>"
        )),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "creating the cache policy answered {}: {}",
        created.status, created.raw
    );
    let policy = created
        .root()
        .find_text("Id")
        .expect("the created cache policy reports an id")
        .to_string();
    let (_id, domain) = deployed(&router, "edge-keyed", SERVED_ORIGIN, &policy).await;

    // (case, the keyed header's value, the expected X-Cache)
    let cases: [(&str, &str, &str); 4] = [
        ("the first request for one value", "alice", MISS),
        ("the same value again", "alice", HIT),
        ("a different value for the same path", "bob", MISS),
        ("the different value again", "bob", HIT),
    ];
    for (case, value, expected) in cases {
        let answer = view_with(
            &router,
            "GET",
            &domain,
            SERVED_PATH,
            &[("x-viewer", value)],
        )
        .await;
        assert_eq!(
            answer.x_cache, expected,
            "case {case:?} (x-viewer {value:?}): expected X-Cache {expected:?}, got {:?}",
            answer.x_cache
        );
    }
}

/// An origin request that would return to the edge it came from is refused instead of
/// made: an origin whose host the distribution itself serves, and a request that already
/// carries an edge's `Via`, are answered the way an origin CloudFront could not get a
/// response from is — so one viewer request cannot loop the edge through itself.
#[tokio::test]
async fn an_origin_request_that_would_return_to_the_edge_is_refused() {
    let router = emulator();
    // The origin is not known until the distribution exists, so this one points at an
    // origin the emulator serves and is then pointed at its own edge domain.
    let (id, domain) = deployed(&router, "edge-loop", SERVED_ORIGIN, CACHING_DISABLED).await;
    let read = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        None,
        None,
    )
    .await;
    let etag = read.header("etag").to_string();
    let looping = config("edge-loop", &domain, CACHING_DISABLED);
    let updated = call(
        &router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        Some(&looping),
        Some(&etag),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "pointing the origin at the distribution's own domain answered {}: {}",
        updated.status, updated.raw
    );

    let answer = view(&router, "GET", &domain, SERVED_PATH).await;
    assert_eq!(
        (answer.status, answer.x_cache.as_str()),
        (502, ERROR),
        "expected the self-referential origin to be refused with 502 and X-Cache \
         {ERROR:?}, got {} with {:?} (body {:?})",
        answer.status,
        answer.x_cache,
        answer.body
    );
}

/// A DISABLED distribution accepts no viewer request: `Enabled` is "whether the
/// distribution is enabled to accept user requests for content".
#[tokio::test]
async fn a_disabled_distribution_accepts_no_viewer_request() {
    let router = emulator();
    let (id, domain) = deployed(&router, "edge-disabled", SERVED_ORIGIN, CACHING_DISABLED).await;
    let read = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        None,
        None,
    )
    .await;
    let etag = read.header("etag").to_string();
    let disabled = config("edge-disabled", SERVED_ORIGIN, CACHING_DISABLED)
        .replace("<Enabled>true</Enabled>", "<Enabled>false</Enabled>");
    let updated = call(
        &router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        Some(&disabled),
        Some(&etag),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "disabling the distribution answered {}: {}",
        updated.status, updated.raw
    );

    let answer = view(&router, "GET", &domain, SERVED_PATH).await;
    assert_eq!(
        (answer.status, answer.x_cache.as_str()),
        (403, ERROR),
        "expected a disabled distribution to refuse the request with 403 and X-Cache \
         {ERROR:?}, got {} with {:?} (body {:?})",
        answer.status,
        answer.x_cache,
        answer.body
    );
    assert!(
        !answer.body.contains("DistributionList"),
        "the refusal carries origin bytes, so the request reached the origin: {:?}",
        answer.body
    );
}
