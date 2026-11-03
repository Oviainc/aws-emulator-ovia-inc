// SPDX-License-Identifier: CC0-1.0

//! The distribution's GEOGRAPHIC RESTRICTION, enforced at the edge, end to end over HTTP.
//!
//! The rule under test, over the whole class rather than over one configuration: a
//! distribution's `Restrictions.GeoRestriction` decides whether a viewer's COUNTRY is
//! served at all, and `RestrictionType` decides how its `Location` items are read —
//! `blacklist` names the countries CloudFront does NOT distribute content in, so a viewer
//! in one of them is refused and every other viewer is served; `whitelist` names the
//! countries it DOES, so a viewer anywhere else is refused; `none` restricts nobody, which
//! is also what a configuration carrying no `Restrictions` means. A refused viewer gets
//! CloudFront's OWN `403` — its generated page, `X-Cache: Error from cloudfront`, no origin
//! bytes — before the edge cache is read and before any origin request is made.
//!
//! Each row drives the rule through a request to the distribution's own domain name, so
//! what is asserted is the wire behaviour a viewer sees.
//!
//! ## How a viewer states where it is
//!
//! CloudFront resolves a viewer's country from its ADDRESS, through the MaxMind GeoIP
//! databases `GeoRestriction` documents. Every viewer of this emulator dials a loopback
//! address, which is in no country those databases place, and nothing off this machine is
//! consulted — so the viewer states its country on the request, in the header CloudFront
//! itself carries it in (`CloudFront-Viewer-Country`, the two-letter uppercase code a
//! `Location` is spelled with). A viewer that states none is in no listed country.
//!
//! ## The origin this suite points at
//!
//! A row whose viewer is SERVED has to reach an origin, so the origin named here is an
//! endpoint this emulator serves: this service's own control-plane host, whose
//! `GET /2020-05-31/distribution` answers a `DistributionList`. That document is what makes
//! "the origin answered" distinguishable from "CloudFront answered instead".

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use super::distribution_lifecycle::{
    call, create_distribution, emulator, past_propagation, API_VERSION, HOST,
};

/// The AWS-managed cache policy `Managed-CachingOptimized`, whose `DefaultTTL` is a day, so
/// an answer the edge may cache IS cached: that is what lets a row assert that a blocked
/// viewer is not served another viewer's cached answer. Its id is the one
/// `MANAGED_CACHE_POLICIES` publishes.
const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

/// An origin this emulator SERVES: this service's own control-plane endpoint.
const SERVED_ORIGIN: &str = HOST;

/// A viewer path the served origin answers with a `DistributionList`.
const SERVED_PATH: &str = "/2020-05-31/distribution";

/// The header a viewer states its country in, as CloudFront spells it.
const VIEWER_COUNTRY: &str = "cloudfront-viewer-country";

/// `X-Cache` for an answer CloudFront produced itself.
const ERROR: &str = "Error from cloudfront";

/// `X-Cache` for an answer the origin produced for this request.
const MISS: &str = "Miss from cloudfront";

/// `X-Cache` for an answer the edge cache held.
const HIT: &str = "Hit from cloudfront";

/// What the edge answered one viewer request with.
struct Viewed {
    status: u16,
    x_cache: String,
    body: String,
}

/// Send one VIEWER request to `host`: no AWS credential, and the country stated the way
/// CloudFront states it when `country` is `Some` — exactly as a browser reaches a
/// distribution.
async fn view(
    router: &MultiServiceRouter,
    host: &str,
    path: &str,
    country: Option<&str>,
) -> Viewed {
    let mut builder = ::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host);
    if let Some(country) = country {
        builder = builder.header(VIEWER_COUNTRY, country);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let x_cache = response
        .headers()
        .get("x-cache")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    Viewed {
        status,
        x_cache,
        body,
    }
}

/// The `DistributionConfig` of a distribution whose only restriction is `restrictions`:
/// one origin and a `GET`/`HEAD` default behaviour under a caching cache policy.
///
/// `restrictions` is the whole `Restrictions` element, or the empty string for a
/// configuration that carries none.
fn config(caller_reference: &str, restrictions: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{SERVED_ORIGIN}</DomainName>\
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
         <CachePolicyId>{CACHING_OPTIMIZED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <Comment>edge geo restriction</Comment>\
         {restrictions}\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// A `Restrictions` element of `restriction_type` over `locations`.
///
/// `Quantity` is the number of locations the element carries, which is what the control
/// plane checks it against.
fn restrictions(restriction_type: &str, locations: &[&str]) -> String {
    let items: String = locations
        .iter()
        .map(|location| format!("<Location>{location}</Location>"))
        .collect();
    let items = if locations.is_empty() {
        String::new()
    } else {
        format!("<Items>{items}</Items>")
    };
    format!(
        "<Restrictions><GeoRestriction>\
         <RestrictionType>{restriction_type}</RestrictionType>\
         <Quantity>{}</Quantity>{items}\
         </GeoRestriction></Restrictions>",
        locations.len()
    )
}

/// Create one distribution and let the clock deploy it: the domain name CloudFront
/// assigned it, which is the host a viewer reaches it at.
async fn deployed(router: &MultiServiceRouter, caller_reference: &str, config: &str) -> String {
    let created = create_distribution(router, config).await;
    assert_eq!(
        created.status, 201,
        "case {caller_reference:?}: creating the distribution answered {}: {}",
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
        "case {caller_reference:?}: the distribution did not reach Deployed: {}",
        read.raw
    );
    domain
}

/// Assert that CloudFront refused this viewer itself: its own 403 page, and nothing of the
/// origin.
fn assert_refused(case: &str, answer: &Viewed) {
    assert_eq!(
        (answer.status, answer.x_cache.as_str()),
        (403, ERROR),
        "case {case:?}: expected CloudFront's own 403 with X-Cache {ERROR:?}, got {} with \
         {:?} (body {:?})",
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
        "case {case:?}: the refusal carries origin bytes, so the request reached the \
         origin: {:?}",
        answer.body
    );
}

/// Assert that the viewer was served: the origin's own answer, through the edge.
fn assert_served(case: &str, answer: &Viewed) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the origin's 200 through the edge, got {} with X-Cache \
         {:?} (body {:?})",
        answer.status, answer.x_cache, answer.body
    );
    assert!(
        answer.x_cache == MISS || answer.x_cache == HIT,
        "case {case:?}: expected the answer to come from the origin or the cache, got \
         X-Cache {:?} (body {:?})",
        answer.x_cache,
        answer.body
    );
    assert!(
        answer.body.contains("DistributionList"),
        "case {case:?}: expected the ORIGIN's own bytes through the edge, got {:?}",
        answer.body
    );
}

/// RULE: `RestrictionType` decides which viewers a distribution's `Location` items refuse,
/// and a refused viewer gets CloudFront's own 403 rather than anything the origin said.
///
/// The rows differ along what the rule generalises over: the restriction type (`blacklist`,
/// `whitelist`, `none`, and a configuration carrying no `Restrictions` at all), HOW MANY
/// locations the restriction names, WHICH country the viewer states — one the restriction
/// names, one it does not, and the same code in lower case, since a `Location` is a code
/// rather than a spelling — and a viewer that states no country at all.
#[tokio::test]
async fn the_edge_refuses_the_viewers_the_geo_restriction_names() {
    let router = emulator();

    // (case, the Restrictions element, the country the viewer states, refused)
    let cases: [(&str, String, Option<&str>, bool); 13] = [
        (
            "a blocklisted country",
            restrictions("blacklist", &["RU"]),
            Some("RU"),
            true,
        ),
        (
            "a country outside a one-country blocklist",
            restrictions("blacklist", &["RU"]),
            Some("DE"),
            false,
        ),
        (
            "a blocklisted country, stated in lower case",
            restrictions("blacklist", &["RU"]),
            Some("ru"),
            true,
        ),
        (
            "the last of several blocklisted countries",
            restrictions("blacklist", &["RU", "CN", "KP"]),
            Some("KP"),
            true,
        ),
        (
            "a country outside a three-country blocklist",
            restrictions("blacklist", &["RU", "CN", "KP"]),
            Some("US"),
            false,
        ),
        (
            "no country stated, under a blocklist",
            restrictions("blacklist", &["RU"]),
            None,
            false,
        ),
        (
            "an allowlisted country",
            restrictions("whitelist", &["DE"]),
            Some("DE"),
            false,
        ),
        (
            "an allowlisted country, stated in lower case",
            restrictions("whitelist", &["DE"]),
            Some("de"),
            false,
        ),
        (
            "a country outside an allowlist",
            restrictions("whitelist", &["DE"]),
            Some("RU"),
            true,
        ),
        (
            "the last of several allowlisted countries",
            restrictions("whitelist", &["DE", "FR", "ES"]),
            Some("ES"),
            false,
        ),
        (
            "no country stated, under an allowlist",
            restrictions("whitelist", &["DE"]),
            None,
            true,
        ),
        (
            "a restriction of none, which restricts nobody",
            restrictions("none", &[]),
            Some("RU"),
            false,
        ),
        (
            "a configuration carrying no Restrictions at all",
            String::new(),
            Some("RU"),
            false,
        ),
    ];

    for (index, (case, restrictions, country, refused)) in cases.into_iter().enumerate() {
        let reference = format!("geo-{index}");
        let domain = deployed(&router, &reference, &config(&reference, &restrictions)).await;
        let answer = view(&router, &domain, SERVED_PATH, country).await;
        if refused {
            assert_refused(case, &answer);
        } else {
            assert_served(case, &answer);
        }
    }
}

/// RULE, the part of it a status alone cannot pin: the restriction is applied BEFORE the
/// edge cache is read, so an object another viewer already cached is not served to a
/// viewer the restriction refuses — and the admitted viewer's own second request still
/// comes out of the cache, which is what shows the object really was held.
#[tokio::test]
async fn a_refused_viewer_is_not_served_a_cached_answer() {
    let router = emulator();
    let restrictions = restrictions("blacklist", &["RU"]);
    let domain = deployed(&router, "geo-cache", &config("geo-cache", &restrictions)).await;

    let first = view(&router, &domain, SERVED_PATH, Some("DE")).await;
    assert_served("an admitted viewer's first request", &first);
    assert_eq!(
        first.x_cache, MISS,
        "case \"an admitted viewer's first request\": expected the origin to answer it, got \
         X-Cache {:?}",
        first.x_cache
    );

    let blocked = view(&router, &domain, SERVED_PATH, Some("RU")).await;
    assert_refused("a blocked viewer after the object was cached", &blocked);

    let again = view(&router, &domain, SERVED_PATH, Some("DE")).await;
    assert_served("an admitted viewer's second request", &again);
    assert_eq!(
        again.x_cache, HIT,
        "case \"an admitted viewer's second request\": expected the cached answer, got \
         X-Cache {:?} (body {:?})",
        again.x_cache, again.body
    );
}
