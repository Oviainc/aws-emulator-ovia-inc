// SPDX-License-Identifier: CC0-1.0

//! The ORDER a published policy catalogue is served in, the CONTENTS of the published CORS
//! bodies, and the DEFAULT page size each listing answers with — end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! Three rules are under test.
//!
//! 1. A policy family Amazon Web Services publishes is reference data every account holds,
//!    and a listing serves it in the order Amazon publishes it rather than sorted by id,
//!    ahead of the policies this account created — which sort among themselves by id. That
//!    one order is what `Marker` resumes in, so a walk at ANY page size enumerates every
//!    policy exactly once in it. The rows span the three families that have such a catalogue
//!    (cache policies, origin request policies, response headers policies) and, inside each,
//!    page sizes of one, two, the catalogue's length and past it.
//! 2. Each published response headers policy carries the CORS body Amazon published for it:
//!    one origin (`*`), no allow-header list of its own, an exposed-header element, and no
//!    override of a header the origin already set; a policy that answers preflight requests
//!    adds the methods a preflight answer names and the published `Access-Control-Max-Age`.
//!    The rows are EVERY entry of the published catalogue, including entries no issue cited.
//! 3. A listing that is sent no `MaxItems` answers with the default page size of ITS OWN
//!    operation, and one that is sent a `MaxItems` echoes the value asked for. The rows span
//!    the listings whose default is their own number and listings that page at this
//!    service's common default, so a single service-wide constant fails them.
//!    `ListRealtimeLogConfigs` adds the empty-page rule: its `Items` wrapper is serialized
//!    even when the page lists nothing.
//!
//! The hand-written tables below name their source: the published catalogue each policy
//! family's state module cites, read here through the crate's own catalogue constants for
//! the ORDER (so an entry added, removed or moved fails [`crate::managed_policy_catalogs`]
//! and [`crate::cache_policy_list`], which spell the entries out), and the per-operation
//! default page sizes the state module states beside each listing.
//!
//! The walk lives here rather than in [`crate::managed_policy_catalogs`] because that
//! suite's harness describes the two families whose single-policy reads and not-found
//! wording it also asserts; the order rule holds for the CACHE policy family too, whose
//! reads and refusals are a different suite's ([`crate::managed_cache_policy_catalog`]), so
//! the three families are walked together here.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_CACHE_POLICIES, MANAGED_ORIGIN_REQUEST_POLICIES, MANAGED_RESPONSE_HEADERS_POLICIES,
    MAX_ITEMS, MAX_ITEMS_CLOUD_FRONT_ORIGIN_ACCESS_IDENTITIES, MAX_ITEMS_ORIGIN_ACCESS_CONTROLS,
    MAX_ITEMS_REALTIME_LOG_CONFIGS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};

/// A `GET` of `path` under this service's API version, with `query` appended verbatim.
async fn get_listing(router: &MultiServiceRouter, path: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/{path}{query}"),
        None,
        None,
    )
    .await
}

/// One policy family's listing, described so one walk can drive all three.
struct Family {
    /// The case name this family is reported under.
    case: &'static str,
    /// The path segment the family's listing lives under.
    path: &'static str,
    /// The element one page item is wrapped in.
    summary: &'static str,
    /// The element inside a summary that carries the policy itself.
    policy: &'static str,
    /// The `Type` value a policy Amazon Web Services published reports.
    managed: &'static str,
    /// The published catalogue's ids, in the order the catalogue states them.
    published: Vec<String>,
}

/// The three families whose listings serve a published catalogue.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "cache policies",
            path: "cache-policy",
            summary: "CachePolicySummary",
            policy: "CachePolicy",
            managed: "managed",
            published: MANAGED_CACHE_POLICIES
                .iter()
                .map(|managed| managed.id.to_string())
                .collect(),
        },
        Family {
            case: "origin request policies",
            path: "origin-request-policy",
            summary: "OriginRequestPolicySummary",
            policy: "OriginRequestPolicy",
            managed: "managed",
            published: MANAGED_ORIGIN_REQUEST_POLICIES
                .iter()
                .map(|managed| managed.id.to_string())
                .collect(),
        },
        Family {
            case: "response headers policies",
            path: "response-headers-policy",
            summary: "ResponseHeadersPolicySummary",
            policy: "ResponseHeadersPolicy",
            managed: "managed",
            published: MANAGED_RESPONSE_HEADERS_POLICIES
                .iter()
                .map(|managed| managed.id.to_string())
                .collect(),
        },
    ]
}

/// The ids one page of `family` reports, in the order the page carries them.
fn page_ids(family: &Family, answer: &Answer) -> Vec<String> {
    answer
        .root()
        .child("Items")
        .map(|items| {
            items
                .children_named(family.summary)
                .into_iter()
                .map(|summary| {
                    summary
                        .text_at(&format!("{}/Id", family.policy))
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Rule 1: the published catalogue is served in the order Amazon publishes it
// ---------------------------------------------------------------------------

/// A listing filtered to the Amazon Web Services kind serves the published catalogue in the
/// catalogue's OWN order, and walking the pages at any admissible page size enumerates every
/// entry exactly once in that same order.
#[tokio::test]
async fn a_published_catalogue_is_served_in_the_order_amazon_publishes_it() {
    for family in families() {
        let router = emulator();
        let case = family.case;
        let total = family.published.len();
        assert!(
            total > 1,
            "case {case}: expected a published catalogue of more than one entry, got {total}"
        );

        let whole = get_listing(&router, family.path, &format!("?Type={}", family.managed)).await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected HTTP 200 from the listing, got {} with {}",
            whole.status, whole.raw
        );
        assert_eq!(
            page_ids(&family, &whole),
            family.published,
            "case {case}: expected the published order, got {}",
            whole.raw
        );

        for size in [1usize, 2, total, total + 1] {
            let mut walked: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            // At most one page per entry plus one, so a listing that never stops truncating
            // fails here rather than looping forever.
            for _ in 0..=total + 1 {
                let query = match &marker {
                    Some(value) => format!(
                        "?Type={}&MaxItems={size}&Marker={value}",
                        family.managed
                    ),
                    None => format!("?Type={}&MaxItems={size}", family.managed),
                };
                let page = get_listing(&router, family.path, &query).await;
                assert_eq!(
                    page.status, 200,
                    "case {case} (page size {size}): expected HTTP 200 for {query}, got {} with \
                     {}",
                    page.status, page.raw
                );
                let ids = page_ids(&family, &page);
                assert!(
                    ids.len() <= size,
                    "case {case} (page size {size}): a page served {} ids, got {}",
                    ids.len(),
                    page.raw
                );
                walked.extend(ids.iter().cloned());
                match page.root().text_at("NextMarker") {
                    Some(next) => {
                        assert_eq!(
                            Some(next),
                            ids.last().map(String::as_str),
                            "case {case} (page size {size}): expected NextMarker to name the id \
                             the page ended at, got {}",
                            page.raw
                        );
                        marker = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                walked, family.published,
                "case {case} (page size {size}): expected the walk to enumerate the published \
                 catalogue once, in the published order, got {walked:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the published CORS bodies
// ---------------------------------------------------------------------------

/// Every published response headers policy that states CORS behaviour states the body Amazon
/// published for it, read back through `GetResponseHeadersPolicy`.
///
/// The rows are EVERY entry of the published catalogue (source: the catalogue the state
/// module cites), so an entry no issue cited is asserted too; the two that answer preflight
/// requests are told apart by the published `Access-Control-Max-Age`.
#[tokio::test]
async fn a_published_response_headers_policy_states_the_published_cors_body() {
    let router = emulator();

    for managed in MANAGED_RESPONSE_HEADERS_POLICIES {
        let case = managed.name;
        let read = get_listing(
            &router,
            &format!("response-headers-policy/{}", managed.id),
            "",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from GetResponseHeadersPolicy, got {} with {}",
            read.status, read.raw
        );
        let config = read
            .root()
            .child("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| panic!("case {case}: the read carries no configuration: {}", read.raw));
        assert_eq!(
            config.text_at("Name"),
            Some(managed.name),
            "case {case}: expected the published name, got {}",
            read.raw
        );

        let Some(cors) = config.child("CorsConfig") else {
            assert!(
                managed.cors.is_none(),
                "case {case}: expected the published CORS body to be served, got {}",
                read.raw
            );
            continue;
        };
        assert_eq!(
            cors.text_at("AccessControlAllowOrigins/Quantity"),
            Some("1"),
            "case {case}: expected one allowed origin, got {}",
            read.raw
        );
        assert_eq!(
            cors.text_at("AccessControlAllowOrigins/Items/Origin"),
            Some("*"),
            "case {case}: expected every origin to be allowed, got {}",
            read.raw
        );
        assert_eq!(
            cors.text_at("AccessControlAllowHeaders/Quantity"),
            Some("0"),
            "case {case}: expected no allow-header list of its own, got {}",
            read.raw
        );
        assert!(
            cors.child("AccessControlExposeHeaders").is_some(),
            "case {case}: expected the exposed-header element to be served, got {}",
            read.raw
        );
        assert_eq!(
            cors.text_at("OriginOverride"),
            Some("false"),
            "case {case}: expected a header the origin already set to be left alone, got {}",
            read.raw
        );

        let published_methods: Vec<&str> = managed
            .cors
            .as_ref()
            .map(|settings| settings.allow_methods.to_vec())
            .unwrap_or_default();
        let served_methods: Vec<String> = cors
            .child("AccessControlAllowMethods")
            .and_then(|methods| methods.child("Items"))
            .map(|items| {
                items
                    .children_named("Method")
                    .into_iter()
                    .map(|method| method.text().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            served_methods, published_methods,
            "case {case}: expected the published methods, got {}",
            read.raw
        );
        assert_eq!(
            cors.text_at("AccessControlAllowMethods/Quantity"),
            Some(published_methods.len().to_string().as_str()),
            "case {case}: expected Quantity to state the method list's length, got {}",
            read.raw
        );
        // A policy that answers preflight requests states a maximum age; a policy for simple
        // requests states none.
        let published_max_age = managed
            .cors
            .as_ref()
            .and_then(|settings| settings.max_age_seconds);
        assert_eq!(
            cors.text_at("AccessControlMaxAgeSec").map(str::to_string),
            published_max_age.map(|seconds| seconds.to_string()),
            "case {case}: expected the published Access-Control-Max-Age, got {}",
            read.raw
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: each listing's own default page size
// ---------------------------------------------------------------------------

/// One listing's default page size: the path it lives under and the number it pages at when
/// a request names none.
///
/// Source: the per-operation default each listing's handler names in this crate's state
/// module. The first three rows are listings whose default is their own number; the rest
/// page at this service's common default, so a single service-wide constant cannot satisfy
/// both halves of the table.
fn default_page_sizes() -> Vec<(&'static str, &'static str, i32)> {
    vec![
        (
            "origin access controls",
            "origin-access-control",
            MAX_ITEMS_ORIGIN_ACCESS_CONTROLS,
        ),
        (
            "origin access identities",
            "origin-access-identity/cloudfront",
            MAX_ITEMS_CLOUD_FRONT_ORIGIN_ACCESS_IDENTITIES,
        ),
        (
            "real-time log configurations",
            "realtime-log-config",
            MAX_ITEMS_REALTIME_LOG_CONFIGS,
        ),
        ("cache policies", "cache-policy", MAX_ITEMS),
        ("public keys", "public-key", MAX_ITEMS),
        ("distributions", "distribution", MAX_ITEMS),
        (
            "response headers policies",
            "response-headers-policy",
            MAX_ITEMS,
        ),
    ]
}

/// A listing sent no `MaxItems` answers with the default page size of its own operation, and
/// a listing sent one echoes the value asked for.
#[tokio::test]
async fn a_listing_answers_with_its_own_default_page_size() {
    let router = emulator();

    for (case, path, default) in default_page_sizes() {
        let listed = get_listing(&router, path, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected HTTP 200 from the listing, got {} with {}",
            listed.status, listed.raw
        );
        assert_eq!(
            listed.root().text_at("MaxItems"),
            Some(default.to_string().as_str()),
            "case {case}: expected the default MaxItems {default}, got {}",
            listed.raw
        );

        for asked in [1, 2, default - 1, default] {
            let page = get_listing(&router, path, &format!("?MaxItems={asked}")).await;
            assert_eq!(
                page.status, 200,
                "case {case} (MaxItems={asked}): expected HTTP 200, got {} with {}",
                page.status, page.raw
            );
            assert_eq!(
                page.root().text_at("MaxItems"),
                Some(asked.to_string().as_str()),
                "case {case} (MaxItems={asked}): expected the value asked for to be echoed, got \
                 {}",
                page.raw
            );
        }
    }
}

/// The real-time log configuration listing states its collection whether or not the
/// collection holds anything: a page that lists nothing carries an EMPTY `Items` element
/// rather than leaving it out, beside that operation's own default page size.
#[tokio::test]
async fn an_empty_real_time_log_configuration_page_carries_an_empty_items_element() {
    let router = emulator();
    let case = "an account holding no real-time log configurations";

    let listed = get_listing(&router, "realtime-log-config", "").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected HTTP 200 from ListRealtimeLogConfigs, got {} with {}",
        listed.status, listed.raw
    );
    // `RealtimeLogConfigs` states no `Quantity` of its own
    // (`smithy_query(query="shape",shape="RealtimeLogConfigs")`), so the page's emptiness
    // shows in the `Items` element it carries and in `IsTruncated`.
    assert_eq!(
        listed.root().text_at("IsTruncated"),
        Some("false"),
        "case {case}: expected nothing to remain after an empty page, got {}",
        listed.raw
    );
    let items = listed
        .root()
        .child("Items")
        .unwrap_or_else(|| panic!("case {case}: expected an Items element, got {}", listed.raw));
    assert!(
        items.children_named("RealtimeLogConfig").is_empty(),
        "case {case}: expected the Items element to be empty, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS_REALTIME_LOG_CONFIGS.to_string().as_str()),
        "case {case}: expected the default MaxItems {MAX_ITEMS_REALTIME_LOG_CONFIGS}, got {}",
        listed.raw
    );
}
