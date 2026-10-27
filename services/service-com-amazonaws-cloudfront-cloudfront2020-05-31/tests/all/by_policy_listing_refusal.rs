// SPDX-License-Identifier: CC0-1.0

//! The refusal every `ListDistributionsBy<Policy>Id` listing answers for a policy id it holds
//! no policy for, end to end over HTTP.
//!
//! The rule under test: WHICH sentence an operation serves for a code that covers two
//! conditions is a property of that OPERATION rather than of the identifier submitted. These
//! listings serve the DISTRIBUTION resource and take the policy id only as a lookup key into
//! the distributions they enumerate — they never address a policy by it — so none of them
//! examines the identifier's shape before the lookup: every id the store holds no policy of
//! that family for is refused with that family's ONE absent-policy sentence (HTTP 404 and the
//! family's `NoSuch…Policy` code), whatever shape the id takes. An id either source of the
//! family holds — the account's own records or the catalogue AWS publishes — is served a page
//! instead, so the refusal is not blanket.
//!
//! What this suite adds beside [`crate::response_headers_policy_refusal_message`], which
//! asserts the same rule for ONE family against the reads that address a policy by the
//! identifier (and so do precheck its shape), is the FAMILY axis: the rule holds for each
//! policy family that models such a listing, each with its own code and its own sentence. The
//! identifier rows are that suite's shape-class tables, shared rather than copied.
//!
//! The wire harness (the router and the request helper) is the one
//! [`crate::distribution_lifecycle`] already builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_CACHE_POLICIES, MANAGED_ORIGIN_REQUEST_POLICIES, MANAGED_RESPONSE_HEADERS_POLICIES,
    NO_SUCH_CACHE_POLICY_MESSAGE, NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
    NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::response_headers_policy_refusal_message::{
    absent_policy_identifiers, create_one, unusable_identifiers, Candidate,
};

/// One policy family a distribution attaches, and the listing keyed by that family's id.
struct Family {
    case: &'static str,
    /// The path segment the model binds that listing to.
    segment: &'static str,
    /// The wire code the listing refuses an unknown id with.
    code: &'static str,
    /// The ONE sentence that refusal carries for an id the family holds no policy for.
    absent_message: &'static str,
    /// Every id of the catalogue this crate publishes for the family.
    published: Vec<String>,
}

/// The families whose by-policy listing this suite drives. The published ids come from the
/// catalogues the crate serves, so a catalogue that grows an entry is covered without a row
/// being written for it.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "cache policies",
            segment: "distributionsByCachePolicyId",
            code: "NoSuchCachePolicy",
            absent_message: NO_SUCH_CACHE_POLICY_MESSAGE,
            published: MANAGED_CACHE_POLICIES
                .iter()
                .map(|managed| managed.id.to_string())
                .collect(),
        },
        Family {
            case: "origin request policies",
            segment: "distributionsByOriginRequestPolicyId",
            code: "NoSuchOriginRequestPolicy",
            absent_message: NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
            published: MANAGED_ORIGIN_REQUEST_POLICIES
                .iter()
                .map(|managed| managed.id.to_string())
                .collect(),
        },
        Family {
            case: "response headers policies",
            segment: "distributionsByResponseHeadersPolicyId",
            code: "NoSuchResponseHeadersPolicy",
            absent_message: NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
            published: MANAGED_RESPONSE_HEADERS_POLICIES
                .iter()
                .map(|managed| managed.id.to_string())
                .collect(),
        },
    ]
}

/// `GET /2020-05-31/{segment}/{id}` — the route the model binds each of these listings to.
async fn list_by_policy_id(router: &MultiServiceRouter, segment: &str, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/{segment}/{id}"),
        None,
        None,
    )
    .await
}

/// Every identifier the sibling suite spans the shape classes with, as ONE table: the values
/// outside the shape a policy id takes, then the well-formed ones that name no policy. The
/// rows are shared because the rule under test is about the OPERATION and not about which
/// family's ids a value resembles.
fn unknown_identifiers(minted: &str, published: &str) -> Vec<Candidate> {
    unusable_identifiers(minted, published)
        .into_iter()
        .chain(absent_policy_identifiers(minted, published))
        .collect()
}

/// Every by-policy listing refuses an id its family holds no policy for with that family's one
/// absent-policy sentence, whatever shape the id takes: the listing resolves the id as a
/// lookup key, so no shape precheck stands in front of the lookup.
#[tokio::test]
async fn a_by_policy_listing_refuses_every_unknown_id_as_an_absent_policy() {
    let router = emulator();
    let (minted, _) = create_one(&router, "by-policy-listing-refusal").await;
    let published_policy = MANAGED_RESPONSE_HEADERS_POLICIES
        .first()
        .expect("the published catalogue is not empty")
        .id;
    let rows = unknown_identifiers(&minted, published_policy);

    for family in families() {
        let case = family.case;
        for candidate in &rows {
            let shape = candidate.case;
            let id = &candidate.id;
            assert!(
                !family.published.iter().any(|published| published == id) && id != &minted,
                "case {case} ({shape}): the row must name no policy of this family, got {id:?}"
            );
            let answer = list_by_policy_id(&router, family.segment, id).await;
            assert_eq!(
                answer.status, 404,
                "case {case} ({shape}, {id:?}): expected HTTP 404, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                family.code,
                "case {case} ({shape}, {id:?}): expected {}, got {}",
                family.code,
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                family.absent_message,
                "case {case} ({shape}, {id:?}): expected {:?}, got {}",
                family.absent_message,
                answer.raw
            );
        }
    }
}

/// The same listing SERVES an id either source of its family holds, so the refusal above is
/// the store's answer about that id and not a blanket one: every published policy id lists the
/// distributions attaching it (none in a fresh account), and so does an id this account just
/// created.
#[tokio::test]
async fn a_by_policy_listing_serves_an_id_the_family_holds() {
    let router = emulator();

    for family in families() {
        let case = family.case;
        for id in &family.published {
            let answer = list_by_policy_id(&router, family.segment, id).await;
            assert_eq!(
                answer.status, 200,
                "case {case} (a published id, {id:?}): expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.root().text_at("Quantity"),
                Some("0"),
                "case {case} (a published id, {id:?}): expected no distribution to attach it in \
                 a fresh account, got {}",
                answer.raw
            );
        }
    }

    // One id this account created, through the family whose create this crate's harness
    // drives: the listing resolves an account-owned id over the same route.
    let (minted, _) = create_one(&router, "by-policy-listing-serves").await;
    let answer =
        list_by_policy_id(&router, "distributionsByResponseHeadersPolicyId", &minted).await;
    assert_eq!(
        answer.status, 200,
        "case an id this account created ({minted:?}): expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.root().text_at("Quantity"),
        Some("0"),
        "case an id this account created ({minted:?}): expected no distribution to attach a \
         policy just created, got {}",
        answer.raw
    );
}
