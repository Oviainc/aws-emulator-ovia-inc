// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListThirdPartyFirewallFirewallPolicies`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: for the third-party firewall vendor the
//! request names, whose tenant association is onboarded, the operation reports
//! exactly the firewall policies the store holds for THAT vendor — their stored
//! id and name — in id order, bounded by `MaxResults` and resumable from a
//! `NextToken` it minted for that same vendor; a vendor whose association is not
//! onboarded has no tenant to enumerate and is refused as not found, and nothing
//! else is written.
//!
//! The firewall-policy inventory is seeded through the store (as the
//! `GetProtectionStatus` suite seeds its attacks): a
//! third-party firewall policy is created in the VENDOR's console or API and no
//! Firewall Manager operation creates one, so the only honest source of one is
//! something that put it in the store. The vendor associations are seeded the
//! same way while `AssociateThirdPartyFirewall` is a stub tracked by its own
//! issue.
//!
//! The tables below are the class the rule ranges over: the vendor rows are
//! every value of the model's `ThirdPartyFirewall` enum, each with an inventory
//! of a different size (and one of none at all); the policy rows differ in id
//! length (both published bounds of `FirewallPolicyId`, 1 and 1024), in
//! character class (digits, non-ASCII letters, a space, every punctuation mark
//! the modelled pattern allows) and in name; the page rows span both published
//! `MaxResults` bounds and one past each; the association rows are every value
//! of the model's `ThirdPartyFirewallAssociationStatus` enum, plus no
//! association at all and one the clock has just settled.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{
    THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES, THIRD_PARTY_FIREWALL_VALUES,
};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the store seam live in one place for this test target, so
// no two suites can drift in how a request is signed or the store is seeded.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, REGION};

/// The operation under test, spelled once.
const OPERATION: &str = "ListThirdPartyFirewallFirewallPolicies";

/// The largest page the operation serves: the maximum the model's
/// `com.amazonaws.fms#PaginationMaxResults` declares for `MaxResults`
/// (range 1..100).
const MAX_PAGE_SIZE: usize = 100;

/// A `MaxResults` one past each published bound of that range.
const BELOW_MIN_MAX_RESULTS: i64 = 0;
const ABOVE_MAX_MAX_RESULTS: i64 = 101;

/// A second Region, to assert the administrator gate's answer does not depend on
/// which Region the request addresses.
const OTHER_REGION: &str = "eu-central-1";

/// The vendor whose tenant holds the seeded inventory under test: the first
/// value of the model's `ThirdPartyFirewall` enum (see [`vendors`]).
///
/// `pub(crate)` so a sibling suite in this test target names the same vendor as
/// this one rather than spelling a second copy of the catalog lookup.
pub(crate) fn primary_vendor() -> &'static str {
    vendors()
        .first()
        .expect("the model declares at least one ThirdPartyFirewall vendor")
}

/// The other vendor, whose tenant holds a DIFFERENT inventory, so a reported
/// policy can only come from the vendor the request named.
fn other_vendor() -> &'static str {
    vendors()
        .get(1)
        .expect("the model declares at least two ThirdPartyFirewall vendors")
}

/// The vendor catalog: the model's own `ThirdPartyFirewall` enum, as
/// `ops::types::THIRD_PARTY_FIREWALL_VALUES` lists it — so every vendor the
/// model declares is covered, not only the one the issue cited.
fn vendors() -> &'static [&'static str] {
    THIRD_PARTY_FIREWALL_VALUES
}

/// The firewall-policy ids seeded for [`primary_vendor`], deliberately NOT in
/// sorted order so the order the operation serves is the store's doing and not
/// the seeding's.
///
/// Every id satisfies the model's `FirewallPolicyId` (length 1..1024, pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), and the rows span that length range's
/// published bounds and the character classes the pattern admits.
fn primary_policy_ids() -> Vec<String> {
    vec![
        "global-1".to_string(),
        // Every punctuation mark the pattern allows, in one id.
        "a_b.c:d/e=f+g-h@i".to_string(),
        // `\p{N}` only.
        "00000042".to_string(),
        // `\p{L}` beyond ASCII, and a `\p{Z}` space.
        "Zürich Außenstelle".to_string(),
        // The published MAXIMUM length of `FirewallPolicyId`.
        "x".repeat(1024),
        // The published MINIMUM length of `FirewallPolicyId`.
        "m".to_string(),
    ]
}

/// The ids seeded for [`other_vendor`]: a different, disjoint inventory of a
/// different size.
fn other_policy_ids() -> Vec<String> {
    vec![
        "fortigate.policy-2".to_string(),
        "fortigate.policy-1".to_string(),
    ]
}

/// The name stored beside an id, so an assertion can tell the two members apart
/// and a reported name can only have come from the record.
fn policy_name(vendor: &str, policy_id: &str) -> String {
    format!("{vendor} tenant policy {policy_id}")
}

/// Seed one vendor association, as `AssociateThirdPartyFirewall` will write it.
///
/// `settles_to` / `settles_at` carry the transition the clock owes, so a test can
/// place an association before or after its own due instant.
fn seed_association(
    store: &mut state::ComAmazonawsFmsAwsfms20180101State,
    vendor: &str,
    status: &str,
    settles_to: Option<&str>,
    settles_at: Option<SystemTime>,
) {
    store.third_party_firewalls.insert(
        vendor.to_string(),
        state::ThirdPartyFirewallRecord {
            status: status.to_string(),
            settles_to: settles_to.map(str::to_string),
            settles_at,
            marketplace_onboarding_status: "COMPLETE".to_string(),
        },
    );
}

/// Seed one firewall policy into a vendor's tenant inventory.
fn seed_policy(
    store: &mut state::ComAmazonawsFmsAwsfms20180101State,
    vendor: &str,
    policy_id: &str,
) {
    let record = state::ThirdPartyFirewallPolicyRecord {
        third_party_firewall: vendor.to_string(),
        firewall_policy_id: policy_id.to_string(),
        firewall_policy_name: policy_name(vendor, policy_id),
    };
    store
        .third_party_firewall_policies
        .insert(record.key(), record);
}

/// An emulator with an administrator associated, both vendors onboarded, and the
/// two tables above seeded into their tenants.
///
/// `pub(crate)` so a sibling suite in this test target stands this vendor
/// inventory up through this one seeder rather than a second copy of it.
pub(crate) async fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    with_store(&engine, |store| {
        for vendor in vendors() {
            seed_association(
                store,
                vendor,
                state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
                None,
                None,
            );
        }
        for policy_id in primary_policy_ids() {
            seed_policy(store, primary_vendor(), &policy_id);
        }
        for policy_id in other_policy_ids() {
            seed_policy(store, other_vendor(), &policy_id);
        }
    });
    (router, engine)
}

/// The ids seeded for `vendor`, in the order the store keeps them in: its keys
/// are `<vendor>#<id>` in one total order, so within a vendor that is the ids'
/// own order.
fn expected_ids(vendor: &str) -> Vec<String> {
    let mut ids = if vendor == primary_vendor() {
        primary_policy_ids()
    } else {
        other_policy_ids()
    };
    ids.sort();
    ids
}

/// The `(FirewallPolicyId, FirewallPolicyName)` pairs a response reports, with
/// the modelled list's presence asserted.
fn reported(case: &str, body: &Value) -> Vec<(String, String)> {
    let policies = body["ThirdPartyFirewallFirewallPolicies"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry ThirdPartyFirewallFirewallPolicies, got {body}")
        });
    policies
        .iter()
        .map(|policy| {
            let id = policy["FirewallPolicyId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every reported policy carries a FirewallPolicyId, got {policy}")
            });
            let name = policy["FirewallPolicyName"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every reported policy carries a FirewallPolicyName, got {policy}")
            });
            (id.to_string(), name.to_string())
        })
        .collect()
}

/// Read every page for `vendor`, following `NextToken`, and return the ids in
/// the order they were served.
///
/// Asserts the page invariants on every page: a page is never larger than the
/// effective bound, a page that carries a token is exactly that bound long, and
/// a token appears only while policies remain.
async fn page_through(
    router: &MultiServiceRouter,
    case: &str,
    vendor: &str,
    max_results: usize,
) -> Vec<String> {
    let bound = max_results.min(MAX_PAGE_SIZE);
    let mut served: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // Enough iterations for the tables at the smallest page size, so a token
    // that never clears fails here instead of looping forever.
    for page in 0..100 {
        let mut body = json!({ "ThirdPartyFirewall": vendor, "MaxResults": max_results });
        if let Some(token) = next_token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, OPERATION, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page} must answer 200, got {status} with {response}"
        );
        let ids: Vec<String> = reported(case, &response)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            ids.len() <= bound,
            "case {case:?}: page {page} served {} policies, past the bound of {bound}: {response}",
            ids.len()
        );
        served.extend(ids.iter().cloned());
        next_token = response["NextToken"].as_str().map(str::to_string);
        if next_token.is_some() {
            assert_eq!(
                ids.len(),
                bound,
                "case {case:?}: page {page} carried a NextToken, so it must be a full page of \
                 {bound}, got {response}"
            );
        } else {
            return served;
        }
    }
    panic!("case {case:?}: NextToken never cleared after 100 pages, having served {served:?}");
}

/// Each vendor's tenant reports exactly its OWN policies — their stored id and
/// name — in id order, with no token on a page that holds them all.
#[tokio::test]
async fn reports_each_vendors_own_policies_in_id_order() {
    let (router, _engine) = seeded().await;

    for vendor in vendors() {
        let case = format!("vendor={vendor}");
        let (status, body) = call(
            &router,
            OPERATION,
            &json!({ "ThirdPartyFirewall": vendor, "MaxResults": MAX_PAGE_SIZE }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an onboarded vendor must answer 200, got {status} with {body}"
        );
        let expected: Vec<(String, String)> = expected_ids(vendor)
            .into_iter()
            .map(|id| {
                let name = policy_name(vendor, &id);
                (id, name)
            })
            .collect();
        assert_eq!(
            reported(&case, &body),
            expected,
            "case {case:?}: expected exactly this vendor's stored policies in id order, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: a page that holds every policy carries no NextToken, got {body}"
        );
    }
}

/// An onboarded vendor whose tenant holds no firewall policy reports an EMPTY
/// list and no token: the tenant exists and holds nothing, which is not the same
/// as the tenant not existing.
#[tokio::test]
async fn an_onboarded_vendor_with_no_policies_reports_an_empty_list() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    with_store(&engine, |store| {
        for vendor in vendors() {
            seed_association(
                store,
                vendor,
                state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
                None,
                None,
            );
        }
        // Only the OTHER vendor's tenant holds anything, so an empty answer
        // cannot come from an empty store.
        seed_policy(store, other_vendor(), "only-the-other-vendor");
    });

    let case = format!("vendor={}", primary_vendor());
    let (status, body) = call(
        &router,
        OPERATION,
        &json!({ "ThirdPartyFirewall": primary_vendor(), "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        (status, reported(&case, &body), body["NextToken"].is_null()),
        (200, Vec::new(), true),
        "case {case:?}: an onboarded tenant with no policies must report an empty list and no \
         token, got {status} with {body}"
    );
}

/// Every page size serves the same policies, once each, in the same id order, in
/// pages no larger than the bound.
#[tokio::test]
async fn pages_are_bounded_by_max_results_and_resumable() {
    let (router, _engine) = seeded().await;

    for vendor in vendors() {
        // Both published bounds of the model's `PaginationMaxResults` range (1
        // and 100) and the sizes around each table's own length.
        for max_results in [1, 2, 3, 5, 6, 7, MAX_PAGE_SIZE] {
            let case = format!("vendor={vendor} MaxResults={max_results}");
            let served = page_through(&router, &case, vendor, max_results).await;
            assert_eq!(
                served,
                expected_ids(vendor),
                "case {case:?}: paging must serve every policy of this vendor once, in id order"
            );
        }
    }
}

/// A `MaxResults` past either published bound of the modelled range is refused.
#[tokio::test]
async fn refuses_a_max_results_outside_the_modelled_range() {
    let (router, _engine) = seeded().await;

    for max_results in [BELOW_MIN_MAX_RESULTS, ABOVE_MAX_MAX_RESULTS] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = call(
            &router,
            OPERATION,
            &json!({ "ThirdPartyFirewall": primary_vendor(), "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside the modelled 1..100 range must be refused with \
             400, got {status} with {body}"
        );
    }
}

/// A vendor the organisation is not a tenant administrator of has no tenant to
/// enumerate: every association status but the terminal `ONBOARD_COMPLETE`, and
/// no association at all, is the modelled not-found — and the policies seeded in
/// that tenant are not reported.
#[tokio::test]
async fn refuses_a_vendor_whose_tenant_association_is_not_onboarded() {
    // The catalog is the model's own `ThirdPartyFirewallAssociationStatus` enum,
    // as `ops::types::THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES` lists it,
    // so every status it declares is covered — plus no association at all.
    let statuses: Vec<Option<&str>> = std::iter::once(None)
        .chain(
            THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES
                .iter()
                .map(|status| Some(*status)),
        )
        .collect();

    for status in statuses {
        let case = format!("association={status:?}");
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        with_store(&engine, |store| {
            if let Some(status) = status {
                seed_association(store, primary_vendor(), status, None, None);
            }
            // The tenant inventory is there either way, so a refusal cannot be
            // an empty store answering.
            for policy_id in primary_policy_ids() {
                seed_policy(store, primary_vendor(), &policy_id);
            }
        });

        let (http_status, body) = call(
            &router,
            OPERATION,
            &json!({ "ThirdPartyFirewall": primary_vendor(), "MaxResults": 100 }),
        )
        .await;
        if status == Some(state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE) {
            assert_eq!(
                http_status, 200,
                "case {case:?}: an onboarded tenant is readable, got {http_status} with {body}"
            );
            assert_eq!(
                reported(&case, &body).len(),
                primary_policy_ids().len(),
                "case {case:?}: an onboarded tenant reports its whole inventory, got {body}"
            );
        } else {
            assert_eq!(
                (http_status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: a vendor without an onboarded tenant must be refused as not \
                 found, got {http_status} with {body}"
            );
        }
    }
}

/// The association transition the CLOCK has already made due is what the answer
/// is decided from: an `ONBOARDING` association whose due instant has passed IS
/// onboarded, and one whose due instant is still ahead is not — and the read
/// materialises that status for a later reader without changing anything a
/// caller could not have derived.
#[tokio::test]
async fn an_onboarding_association_is_readable_once_the_clock_has_made_it_due() {
    for (case, settles_at, expected_status) in [
        ("already due", SystemTime::now() - Duration::from_secs(1), 200),
        (
            "not due yet",
            SystemTime::now() + Duration::from_secs(60),
            400,
        ),
    ] {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        with_store(&engine, |store| {
            seed_association(
                store,
                primary_vendor(),
                state::THIRD_PARTY_FIREWALL_ONBOARDING,
                Some(state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE),
                Some(settles_at),
            );
            seed_policy(store, primary_vendor(), "tenant-policy");
        });

        let (status, body) = call(
            &router,
            OPERATION,
            &json!({ "ThirdPartyFirewall": primary_vendor(), "MaxResults": 100 }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );

        // What the association reports afterwards, read through the implemented
        // status operation rather than out of the store.
        let (status, association) = call(
            &router,
            "GetThirdPartyFirewallAssociationStatus",
            &json!({ "ThirdPartyFirewall": primary_vendor() }),
        )
        .await;
        let expected_association = if expected_status == 200 {
            state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE
        } else {
            state::THIRD_PARTY_FIREWALL_ONBOARDING
        };
        assert_eq!(
            (status, association["ThirdPartyFirewallStatus"].as_str()),
            (200, Some(expected_association)),
            "case {case:?}: the association must report the status the clock owes, got {status} \
             with {association}"
        );
    }
}

/// A `NextToken` this service did not mint — or one it minted for ANOTHER
/// vendor's page — names no boundary in this vendor's policies, and is refused
/// as invalid input rather than resumed from.
#[tokio::test]
async fn refuses_a_next_token_it_did_not_mint_for_this_vendor() {
    let (router, _engine) = seeded().await;

    // A real token, minted for the OTHER vendor's first page.
    let (status, body) = call(
        &router,
        OPERATION,
        &json!({ "ThirdPartyFirewall": other_vendor(), "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other vendor's first page must answer 200, got {status} with {body}"
    );
    let foreign_token = body["NextToken"]
        .as_str()
        .expect("a first page of one must carry a token")
        .to_string();

    // Every row satisfies the modelled `PaginationToken` (length 1..4096,
    // pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), so each reaches the handler.
    let cases: Vec<(&str, String)> = vec![
        ("the other vendor's own token", foreign_token),
        (
            "a key of the other vendor",
            state::encode_key_token(&state::third_party_firewall_policy_key(
                other_vendor(),
                "fortigate.policy-1",
            )),
        ),
        (
            "a key of no vendor at all",
            state::encode_key_token("not-a-vendor-key"),
        ),
        ("not hexadecimal", "zzzz".to_string()),
        ("hexadecimal that is not UTF-8", "abcd".to_string()),
        (
            // A spelling of one of THIS vendor's own keys that the minter never
            // emits, so it is a token this service never issued.
            "a case-variant spelling of a key of this vendor",
            state::encode_key_token(&state::third_party_firewall_policy_key(
                primary_vendor(),
                "global-1",
            ))
            .to_uppercase(),
        ),
        (
            "a bare policy id rather than a store key",
            state::encode_key_token("global-1"),
        ),
    ];

    for (case, token) in cases {
        let (status, body) = call(
            &router,
            OPERATION,
            &json!({
                "ThirdPartyFirewall": primary_vendor(),
                "MaxResults": 100,
                "NextToken": token,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {case:?}: token {token:?} names no page of this vendor and must be refused, \
             got {status} with {body}"
        );
    }
}

/// The administrator gate is decided FIRST: with no administrator association
/// the call is refused with `AccessDeniedException` whatever the vendor's
/// association or the token says, in any Region.
#[tokio::test]
async fn the_administrator_gate_is_decided_before_the_vendor_is_looked_up() {
    for region in [REGION, OTHER_REGION] {
        let (router, engine) = emulator_with_engine();
        // An onboarded tenant with policies in it, so the refusal is the gate's
        // and not the tenant's.
        with_store(&engine, |store| {
            seed_association(
                store,
                primary_vendor(),
                state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
                None,
                None,
            );
            seed_policy(store, primary_vendor(), "tenant-policy");
        });

        for (case, body) in [
            (
                "onboarded vendor",
                json!({ "ThirdPartyFirewall": primary_vendor(), "MaxResults": 100 }),
            ),
            (
                "vendor with no association",
                json!({ "ThirdPartyFirewall": other_vendor(), "MaxResults": 100 }),
            ),
            (
                "an invalid token",
                json!({
                    "ThirdPartyFirewall": primary_vendor(),
                    "MaxResults": 100,
                    "NextToken": "zzzz",
                }),
            ),
        ] {
            let (status, response) = call_in(&router, OPERATION, &body, region).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "AccessDeniedException"),
                "case {case:?} in {region}: expected the administrator gate to refuse first, got \
                 {status} with {response}"
            );
        }
    }
}

/// Reporting changes nothing a later caller sees: the same request answers the
/// same thing twice, and the tenant inventory in the store is untouched.
#[tokio::test]
async fn reading_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = seeded().await;
    let held_before = with_store(&engine, |store| store.third_party_firewall_policies.len());

    let request = json!({ "ThirdPartyFirewall": primary_vendor(), "MaxResults": 2 });
    let (first_status, first) = call(&router, OPERATION, &request).await;
    let (second_status, second) = call(&router, OPERATION, &request).await;
    assert_eq!(
        (first_status, &first),
        (second_status, &second),
        "two identical reads must answer identically, got {first} then {second}"
    );

    // Paging through does not consume anything either.
    let paged = page_through(&router, "repeat", primary_vendor(), 2).await;
    assert_eq!(
        paged,
        expected_ids(primary_vendor()),
        "paging a second time must serve the same policies"
    );
    assert_eq!(
        with_store(&engine, |store| store.third_party_firewall_policies.len()),
        held_before,
        "a reporting operation must not add to or remove from the tenant inventory"
    );
}
