// SPDX-License-Identifier: CC0-1.0

//! The AWS-managed POLICY CATALOGS as reference data, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a policy family Amazon Web Services publishes is reference data
//! every account holds, not account state, so every read of that family resolves the
//! published catalogue BESIDE the account's own records — the listing filtered to the
//! Amazon Web Services kind serves the whole catalogue with each summary stating that kind,
//! a read of a published id answers from the catalogue through both single-policy readers,
//! and the two partitions stay disjoint: nothing the account created is served as Amazon
//! Web Services', and nothing Amazon Web Services published is served as the account's.
//!
//! The rows span the two families whose catalogues this suite covers — origin request
//! policies and response headers policies — and, inside each, EVERY entry of the published
//! catalogue, at page sizes of one, two, the catalogue's own length and the server's
//! ceiling. [`crate::managed_cache_policy_catalog`] and [`crate::cache_policy_list`] assert
//! the same rule for the cache policy family, including the mutations a published policy
//! refuses.
//!
//! The hand-written tables below are EVERY entry Amazon publishes, spelled out HERE rather
//! than read from the crate and asserted against the crate's catalogue entry for entry, so a
//! catalogue that drifts from what Amazon publishes — an entry renamed, missing or added —
//! fails this suite. Source: the CloudFront Developer Guide's lists of managed policies
//! ("Using the managed origin request policies" and "Using the managed response headers
//! policies"), which state each policy's name, its identifier and its settings. They include
//! entries no issue cited.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    ORIGIN_REQUEST_POLICY_TYPE_VALUES, RESPONSE_HEADERS_POLICY_TYPE_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_ORIGIN_REQUEST_POLICIES, MANAGED_RESPONSE_HEADERS_POLICIES, MAX_ITEMS,
    NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE, NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
    ORIGIN_REQUEST_POLICY_TYPE_CUSTOM, ORIGIN_REQUEST_POLICY_TYPE_MANAGED,
    RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE, RESPONSE_HEADERS_POLICY_TYPE_CUSTOM,
    RESPONSE_HEADERS_POLICY_TYPE_MANAGED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::origin_request_policy_lifecycle::{create_policy as create_origin_request_policy, Policy};
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the two surfaces
// ---------------------------------------------------------------------------

/// A `GET` of `path` under this service's API version.
async fn get_path(router: &MultiServiceRouter, path: &str) -> Answer {
    call(router, "GET", &format!("/{API_VERSION}{path}"), None, None).await
}

/// `POST /2020-05-31/response-headers-policy` with a `ResponseHeadersPolicyConfig` that
/// carries nothing but the name — the smallest configuration the create accepts, which is
/// all the control rows below need.
async fn create_response_headers_policy(router: &MultiServiceRouter, name: &str) -> Answer {
    let body = format!(
        "<ResponseHeadersPolicyConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Name>{}</Name>\
         </ResponseHeadersPolicyConfig>",
        escape(name)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/response-headers-policy"),
        Some(&body),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// One family, described so both can be driven by the same walk
// ---------------------------------------------------------------------------

/// Everything the rule needs to know about ONE policy family: where its listing lives, what
/// its summaries and its single-policy reads are named, which `Type` values it uses, and the
/// catalogue the crate serves for it.
struct Family {
    /// The case name this family is reported under.
    case: &'static str,
    /// The path segment the family's operations live under.
    path: &'static str,
    /// The element one page item is wrapped in.
    summary: &'static str,
    /// The element inside a summary that carries the policy itself.
    policy: &'static str,
    /// The element inside a policy that carries its configuration.
    config: &'static str,
    /// The `Type` a policy Amazon Web Services published reports.
    managed: &'static str,
    /// The `Type` a policy this account created reports.
    custom: &'static str,
    /// The wire code a read of an id neither source holds answers with.
    missing_code: &'static str,
    /// The message that refusal carries for a WELL-FORMED id of this family that names no
    /// policy.
    missing_message: &'static str,
    /// The message that refusal carries for an identifier that is not of this family's id
    /// shape at all.
    malformed_message: &'static str,
    /// `(Id, Name)` for every entry of the published catalogue, as the crate serves it.
    catalogue: Vec<(String, String)>,
}

/// The origin request policy family.
fn origin_request_family() -> Family {
    Family {
        case: "origin request policies",
        path: "origin-request-policy",
        summary: "OriginRequestPolicySummary",
        policy: "OriginRequestPolicy",
        config: "OriginRequestPolicyConfig",
        managed: ORIGIN_REQUEST_POLICY_TYPE_MANAGED,
        custom: ORIGIN_REQUEST_POLICY_TYPE_CUSTOM,
        missing_code: "NoSuchOriginRequestPolicy",
        missing_message: NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
        // This family answers one sentence for both conditions: nothing recorded shows it
        // telling an unusable identifier apart from a well-formed one that names no policy.
        malformed_message: NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
        catalogue: MANAGED_ORIGIN_REQUEST_POLICIES
            .iter()
            .map(|managed| (managed.id.to_string(), managed.name.to_string()))
            .collect(),
    }
}

/// The response headers policy family.
fn response_headers_family() -> Family {
    Family {
        case: "response headers policies",
        path: "response-headers-policy",
        summary: "ResponseHeadersPolicySummary",
        policy: "ResponseHeadersPolicy",
        config: "ResponseHeadersPolicyConfig",
        managed: RESPONSE_HEADERS_POLICY_TYPE_MANAGED,
        custom: RESPONSE_HEADERS_POLICY_TYPE_CUSTOM,
        missing_code: "NoSuchResponseHeadersPolicy",
        missing_message: NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
        malformed_message: RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
        catalogue: MANAGED_RESPONSE_HEADERS_POLICIES
            .iter()
            .map(|managed| (managed.id.to_string(), managed.name.to_string()))
            .collect(),
    }
}

impl Family {
    /// `(Type, Id, Name)` for every summary a page of this family reports, in page order.
    fn page_summaries(&self, answer: &Answer) -> Vec<(String, String, String)> {
        answer
            .root()
            .child("Items")
            .map(|items| {
                items
                    .children_named(self.summary)
                    .into_iter()
                    .map(|summary| {
                        (
                            summary.text_at("Type").unwrap_or_default().to_string(),
                            summary
                                .text_at(&format!("{}/Id", self.policy))
                                .unwrap_or_default()
                                .to_string(),
                            summary
                                .text_at(&format!("{}/{}/Name", self.policy, self.config))
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Walk the listing `filter` describes at `size` items a page, asserting the page
    /// bookkeeping on the way, and report every `(Type, Id, Name)` the walk reported.
    async fn walk(
        &self,
        router: &MultiServiceRouter,
        filter: &str,
        size: usize,
        total: usize,
    ) -> Vec<(String, String, String)> {
        let case = self.case;
        let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
        let mut walked: Vec<(String, String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        // At most one page per item plus one, so a listing that never stops truncating
        // fails here rather than looping forever.
        for page in 0..=total + 1 {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}{filter}&Marker={value}"),
                None => format!("?MaxItems={size}{filter}"),
            };
            let answer = get_path(router, &format!("/{}{query}", self.path)).await;
            assert_eq!(
                answer.status, 200,
                "case {case} ({query}): expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let items = self.page_summaries(&answer);
            assert!(
                items.len() <= size.min(ceiling),
                "case {case} ({query}): a page served {} items, got {}",
                items.len(),
                answer.raw
            );
            assert_eq!(
                answer.root().text_at("Quantity"),
                Some(items.len().to_string().as_str()),
                "case {case} ({query}): expected Quantity to state the page's length, got {}",
                answer.raw
            );
            walked.extend(items.iter().cloned());

            let next = answer.root().text_at("NextMarker").map(str::to_string);
            let remaining = walked.len() < total;
            assert_eq!(
                next.is_some(),
                remaining,
                "case {case} ({query}) page {page}: expected NextMarker present {remaining} \
                 after {} of {total} items, got {}",
                walked.len(),
                answer.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        items.last().map(|(_, id, _)| id.as_str()),
                        "case {case} ({query}): expected NextMarker to name the id the page \
                         ended at, got {}",
                        answer.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        walked
    }

    /// Read the policy `id` through both single-policy readers and report
    /// `(Name, ETag)` as they agree on it, asserting both answer served.
    async fn read(&self, router: &MultiServiceRouter, id: &str) -> (String, String) {
        let case = self.case;

        let read = get_path(router, &format!("/{}/{id}", self.path)).await;
        assert_eq!(
            read.status, 200,
            "case {case} ({id}): expected HTTP 200 from the policy read, got {} with {}",
            read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id),
            "case {case} ({id}): expected the read to report the id asked for, got {}",
            read.raw
        );
        let name = read
            .root()
            .text_at(&format!("{}/Name", self.config))
            .unwrap_or_default()
            .to_string();
        let etag = read.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case} ({id}): expected a version token in the ETag header, got {etag:?}"
        );

        let config_read = get_path(router, &format!("/{}/{id}/config", self.path)).await;
        assert_eq!(
            config_read.status, 200,
            "case {case} ({id}): expected HTTP 200 from the configuration read, got {} with {}",
            config_read.status, config_read.raw
        );
        assert_eq!(
            config_read.root().text_at("Name"),
            Some(name.as_str()),
            "case {case} ({id}): expected both readers to report one name, got {}",
            config_read.raw
        );
        assert_eq!(
            config_read.header("etag"),
            etag,
            "case {case} ({id}): expected one version token across both readers, got {:?}",
            config_read.header("etag")
        );

        (name, etag)
    }
}

// ---------------------------------------------------------------------------
// The published entries, written out from Amazon's own lists
// ---------------------------------------------------------------------------

/// Origin request policies Amazon publishes: `(Id, Name, HeaderBehavior)`, in the order
/// Amazon lists them. EVERY entry of the published list, so an extra, missing or renamed
/// catalogue entry fails this suite; several of them are entries no issue cited.
const PUBLISHED_ORIGIN_REQUEST_POLICIES: &[(&str, &str, &str)] = &[
    (
        "acba4595-bd28-49b8-b9fe-13317c0390fa",
        "Managed-UserAgentRefererHeaders",
        "whitelist",
    ),
    (
        "59781a5b-3903-41f3-afcb-af62929ccde1",
        "Managed-CORS-CustomOrigin",
        "whitelist",
    ),
    (
        "88a5eaf4-2fd4-4709-b370-b4c650ea3fcf",
        "Managed-CORS-S3Origin",
        "whitelist",
    ),
    (
        "216adef6-5c7f-47e4-b989-5492eafa07d3",
        "Managed-AllViewer",
        "allViewer",
    ),
    (
        "33f36d7e-f396-46d9-90e0-52428a34d9dc",
        "Managed-AllViewerAndCloudFrontHeaders-2022-06",
        "allViewerAndWhitelistCloudFront",
    ),
    (
        "775133bc-15f2-49f9-abea-afb2e0bf67d2",
        "Managed-Elemental-MediaTailor-PersonalizedManifests",
        "whitelist",
    ),
    (
        "b689b0a8-53d0-40ab-baf2-68738e2966ac",
        "Managed-AllViewerExceptHostHeader",
        "allExcept",
    ),
];

/// Response headers policies Amazon publishes: `(Id, Name, carries a CorsConfig, carries a
/// SecurityHeadersConfig)`, in the order Amazon lists them. EVERY entry of the published
/// list, for the same reason.
const PUBLISHED_RESPONSE_HEADERS_POLICIES: &[(&str, &str, bool, bool)] = &[
    (
        "60669652-455b-4ae9-85a4-c4c02393f86c",
        "Managed-SimpleCORS",
        true,
        false,
    ),
    (
        "5cc3b908-e619-4b99-88e5-2cf7f45965bd",
        "Managed-CORS-With-Preflight",
        true,
        false,
    ),
    (
        "eaab4381-ed33-4a86-88ca-d9558dc6cd63",
        "Managed-CORS-with-preflight-and-SecurityHeadersPolicy",
        true,
        true,
    ),
    (
        "e61eb345-f6c1-4b97-f0d2-0bdcb2b0b6d3",
        "Managed-CORS-and-SecurityHeadersPolicy",
        true,
        true,
    ),
    (
        "67f7725c-6f97-4210-82d7-5512b31e9d03",
        "Managed-SecurityHeadersPolicy",
        false,
        true,
    ),
];

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The two kinds each family names are the two its model's enum admits, so a filter built
/// from either is a value a client may send.
#[test]
fn the_filter_values_are_the_modelled_ones() {
    for (case, used, modelled) in [
        (
            "origin request policies",
            [
                ORIGIN_REQUEST_POLICY_TYPE_MANAGED,
                ORIGIN_REQUEST_POLICY_TYPE_CUSTOM,
            ],
            ORIGIN_REQUEST_POLICY_TYPE_VALUES,
        ),
        (
            "response headers policies",
            [
                RESPONSE_HEADERS_POLICY_TYPE_MANAGED,
                RESPONSE_HEADERS_POLICY_TYPE_CUSTOM,
            ],
            RESPONSE_HEADERS_POLICY_TYPE_VALUES,
        ),
    ] {
        for kind in used {
            assert!(
                modelled.contains(&kind),
                "case {case}: expected {kind:?} to be a value of the modelled enum, got \
                 {modelled:?}"
            );
        }
    }
}

/// An account that has created nothing is served the whole published catalogue of each
/// family: every entry once, at every page size, each summary stating that Amazon Web
/// Services created it, and each entry readable by its id through both single-policy
/// readers at one unchanging version.
#[tokio::test]
async fn an_account_that_created_nothing_is_served_the_published_catalogues() {
    for family in [origin_request_family(), response_headers_family()] {
        let router = emulator();
        let case = family.case;
        let total = family.catalogue.len();
        assert!(
            total > 0,
            "case {case}: expected the published catalogue to hold at least one policy"
        );

        let expected: BTreeSet<(String, String, String)> = family
            .catalogue
            .iter()
            .map(|(id, name)| (family.managed.to_string(), id.clone(), name.clone()))
            .collect();
        let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
        for size in [1usize, 2, total, ceiling] {
            let walked = family
                .walk(&router, &format!("&Type={}", family.managed), size, total)
                .await;
            assert_eq!(
                walked.len(),
                total,
                "case {case} (page size {size}): expected the walk to enumerate every \
                 published policy exactly once, got {walked:?}"
            );
            assert_eq!(
                walked.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {case} (page size {size}): expected the published catalogue, got \
                 {walked:?}"
            );
            let mut ids: Vec<String> = walked.iter().map(|(_, id, _)| id.clone()).collect();
            let published_order: Vec<String> = family
                .catalogue
                .iter()
                .map(|(id, _)| id.clone())
                .collect();
            assert_eq!(
                ids, published_order,
                "case {case} (page size {size}): expected the published catalogue's own \
                 order, got {ids:?}"
            );
            ids.dedup();
            assert_eq!(
                ids.len(),
                total,
                "case {case} (page size {size}): expected every id once, got {ids:?}"
            );
        }

        // Every entry is readable by its id, and reports the same record on a later read:
        // nothing changes a published policy, so nothing changes its version.
        for (id, name) in &family.catalogue {
            let (read_name, etag) = family.read(&router, id).await;
            assert_eq!(
                read_name, *name,
                "case {case} ({id}): expected the published name, got {read_name:?}"
            );
            let (again_name, again_etag) = family.read(&router, id).await;
            assert_eq!(
                (again_name, again_etag),
                (read_name, etag),
                "case {case} ({id}): expected a later read to answer from the same record"
            );
        }
    }
}

/// The published catalogues carry EXACTLY the entries Amazon publishes — no entry missing
/// and none extra — each under the identifier, the name and the settings Amazon publishes
/// for it.
#[tokio::test]
async fn the_published_entries_are_the_ones_amazon_publishes() {
    let router = emulator();

    let origin_request = origin_request_family();
    assert_eq!(
        origin_request
            .catalogue
            .iter()
            .map(|(id, name)| (id.clone(), name.clone()))
            .collect::<Vec<(String, String)>>(),
        PUBLISHED_ORIGIN_REQUEST_POLICIES
            .iter()
            .map(|(id, name, _)| ((*id).to_string(), (*name).to_string()))
            .collect::<Vec<(String, String)>>(),
        "case origin request policies: expected the catalogue to be exactly Amazon's \
         published list, in the published order, got {:?}",
        origin_request.catalogue
    );
    for (id, name, header_behavior) in PUBLISHED_ORIGIN_REQUEST_POLICIES {
        let (read_name, _) = origin_request.read(&router, id).await;
        assert_eq!(
            read_name, *name,
            "case {name}: expected the published name for {id}, got {read_name:?}"
        );
        let read = get_path(&router, &format!("/origin-request-policy/{id}")).await;
        assert_eq!(
            read.root()
                .text_at("OriginRequestPolicyConfig/HeadersConfig/HeaderBehavior"),
            Some(*header_behavior),
            "case {name}: expected the published HeaderBehavior {header_behavior}, got {}",
            read.raw
        );
    }

    let response_headers = response_headers_family();
    assert_eq!(
        response_headers
            .catalogue
            .iter()
            .map(|(id, name)| (id.clone(), name.clone()))
            .collect::<Vec<(String, String)>>(),
        PUBLISHED_RESPONSE_HEADERS_POLICIES
            .iter()
            .map(|(id, name, _, _)| ((*id).to_string(), (*name).to_string()))
            .collect::<Vec<(String, String)>>(),
        "case response headers policies: expected the catalogue to be exactly Amazon's \
         published list, in the published order, got {:?}",
        response_headers.catalogue
    );
    for (id, name, has_cors, has_security) in PUBLISHED_RESPONSE_HEADERS_POLICIES {
        let (read_name, _) = response_headers.read(&router, id).await;
        assert_eq!(
            read_name, *name,
            "case {name}: expected the published name for {id}, got {read_name:?}"
        );
        let read = get_path(&router, &format!("/response-headers-policy/{id}")).await;
        let config = read
            .root()
            .child("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!("case {name}: the read carries no configuration: {}", read.raw)
            });
        assert_eq!(
            config.child("CorsConfig").is_some(),
            *has_cors,
            "case {name}: expected a CorsConfig present {has_cors}, got {}",
            read.raw
        );
        assert_eq!(
            config.child("SecurityHeadersConfig").is_some(),
            *has_security,
            "case {name}: expected a SecurityHeadersConfig present {has_security}, got {}",
            read.raw
        );
    }
}

/// The two partitions are DISJOINT: a policy this account created is served as the
/// account's kind and never as Amazon Web Services', a published policy is served as Amazon
/// Web Services' kind and never as the account's, and a request naming no kind is served
/// both in one order.
#[tokio::test]
async fn the_published_and_account_partitions_stay_disjoint() {
    let router = emulator();

    // One policy of each family, created in this account.
    let own = Policy {
        case: "a policy this account created",
        name: "account-owned-origin-request".to_string(),
        ..Policy::default()
    };
    let created = create_origin_request_policy(&router, &own.xml()).await;
    assert_eq!(
        created.status, 201,
        "case origin request policies: expected HTTP 201 from the create, got {} with {}",
        created.status, created.raw
    );
    let own_origin_request = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    let created = create_response_headers_policy(&router, "account-owned-response-headers").await;
    assert_eq!(
        created.status, 201,
        "case response headers policies: expected HTTP 201 from the create, got {} with {}",
        created.status, created.raw
    );
    let own_response_headers = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    for (family, own_id) in [
        (origin_request_family(), own_origin_request),
        (response_headers_family(), own_response_headers),
    ] {
        let case = family.case;
        let published: BTreeSet<String> = family
            .catalogue
            .iter()
            .map(|(id, _)| id.clone())
            .collect();

        // The account's kind: its own policy, and none of the published ones.
        let own_page = family
            .walk(&router, &format!("&Type={}", family.custom), 100, 1)
            .await;
        assert_eq!(
            own_page
                .iter()
                .map(|(kind, id, _)| (kind.clone(), id.clone()))
                .collect::<Vec<(String, String)>>(),
            vec![(family.custom.to_string(), own_id.clone())],
            "case {case}: expected the account's own kind to list its one policy, got \
             {own_page:?}"
        );

        // Amazon Web Services' kind: the published catalogue, and not the account's policy.
        let managed_page = family
            .walk(
                &router,
                &format!("&Type={}", family.managed),
                100,
                published.len(),
            )
            .await;
        assert!(
            managed_page.iter().all(|(kind, id, _)| kind == family.managed
                && published.contains(id)
                && *id != own_id),
            "case {case}: expected only published policies under the Amazon Web Services \
             kind, got {managed_page:?}"
        );

        // No filter: both partitions, each id once, in ONE order — the published catalogue
        // in its own order, then the policies this account created.
        let both = family.walk(&router, "", 100, published.len() + 1).await;
        let ids: Vec<String> = both.iter().map(|(_, id, _)| id.clone()).collect();
        let mut expected: Vec<String> = family
            .catalogue
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        expected.push(own_id.clone());
        assert_eq!(
            ids, expected,
            "case {case}: expected both partitions in one order, got {both:?}"
        );
        assert!(
            both.iter().all(|(kind, id, _)| if *id == own_id {
                kind == family.custom
            } else {
                kind == family.managed
            }),
            "case {case}: expected each summary to state which of the two created it, got \
             {both:?}"
        );
    }
}

/// An id outside BOTH sources — the published catalogue and the account's own records —
/// names no policy at all, so both readers answer the modelled 404. The ids are shaped like
/// the catalogue's (a UUID) and unlike it, and the MESSAGE follows the shape: a well-formed
/// id names no policy, while an identifier outside the shape is unusable.
#[tokio::test]
async fn an_id_in_neither_source_does_not_exist() {
    let router = emulator();

    for family in [origin_request_family(), response_headers_family()] {
        let case = family.case;
        for (shape, id, expected) in [
            (
                "uuid-shaped",
                "11111111-2222-3333-4444-555555555555",
                family.missing_message,
            ),
            ("not-an-id-at-all", "nope", family.malformed_message),
            ("a version token", "E1AAAAAAAAAAAA", family.malformed_message),
        ] {
            for path in [format!("/{}/{id}", family.path), format!("/{}/{id}/config", family.path)]
            {
                let answer = get_path(&router, &path).await;
                assert_eq!(
                    answer.status, 404,
                    "case {case} ({shape}, {path}): expected HTTP 404, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    family.missing_code,
                    "case {case} ({shape}, {path}): expected {}, got {}",
                    family.missing_code,
                    answer.raw
                );
                assert_eq!(
                    answer.root().find_text("Message").unwrap_or_default(),
                    expected,
                    "case {case} ({shape}, {path}): expected {expected:?}, got {}",
                    answer.raw
                );
            }
        }
    }
}

/// A published origin request policy id RESOLVES wherever that family's ids are resolved:
/// `ListDistributionsByOriginRequestPolicyId` serves a page for one (no distribution
/// attaches it in a fresh account) rather than claiming the policy does not exist, while an
/// id in neither source is still the modelled 404.
#[tokio::test]
async fn a_published_id_resolves_in_the_by_policy_listing() {
    let router = emulator();

    for (id, name) in &origin_request_family().catalogue {
        let answer = get_path(
            &router,
            &format!("/distributionsByOriginRequestPolicyId/{id}"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected HTTP 200 listing the distributions of {id}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.root().text_at("Quantity"),
            Some("0"),
            "case {name}: expected no distribution to attach {id} in a fresh account, got {}",
            answer.raw
        );
    }

    let answer = get_path(
        &router,
        "/distributionsByOriginRequestPolicyId/11111111-2222-3333-4444-555555555555",
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "case an id in neither source: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchOriginRequestPolicy",
        "case an id in neither source: expected NoSuchOriginRequestPolicy, got {}",
        answer.raw
    );
}
