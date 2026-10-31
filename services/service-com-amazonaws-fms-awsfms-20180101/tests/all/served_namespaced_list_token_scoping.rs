// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the scope of a `NextToken` a `DefaultLists`-selected
//! listing HANDS BACK TO A CLIENT.
//!
//! RULE under test: a `NextToken` is a boundary only of the page that minted it,
//! where a page of this key class is identified by BOTH halves of what selects it
//! — the collection the listing enumerates and the namespace `DefaultLists` picks
//! inside it. So a token one of these pages returned resumes that page exactly
//! after the lists already served, and every OTHER such page refuses it rather
//! than using it as a lower bound: a `ListId` of another page is either absent
//! here or sorts past lists this page does hold, so resuming from it would drop
//! them and report a short page with no `NextToken`, which a client cannot tell
//! from a complete one.
//!
//! What this suite adds over its siblings: the foreign token is the one the
//! SERVER returned over the wire to a previous page call, not one a test minted
//! through [`state::encode_namespaced_list_page_token`]. That is the token a
//! client actually holds, so it is what pins that the stamping the refusal rests
//! on is on the minting side too — a listing that scoped only its decode, or
//! returned a bare `ListId`, passes the sibling suites and fails here.
//! [`crate::list_namespace_page_token_scoping`] owns the refusal of a
//! test-minted token and of the half-stamped forms;
//! [`crate::namespaced_list_page_single_rule`] owns the page walk itself.
//!
//! Asserted over a TABLE whose rows are the four pages of this key class:
//! `ListAppsLists` and `ListProtocolsLists`, each in the Firewall Manager MANAGED
//! namespace and in the customer one. The rows differ in the collection they
//! enumerate (two stores whose `ListId`s are minted independently of each other),
//! in the namespace flag and in the output member carrying the items, and each
//! row is replayed against all three others — twelve ordered pairs.
//!
//! The ids are seeded to INTERLEAVE across the four pages so that the boundary
//! each page returns sorts past the first list of every other page;
//! [`a_served_boundary_of_each_page_would_truncate_every_other`] pins that
//! precondition, so the refusal rows below cannot quietly stop covering it.
//!
//! Every call goes over the wire — an awsJson1_1 `POST /` through the harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{
    associate_admin, seed_apps_list, seed_protocols_list, with_store,
};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, REGION};

/// A `com.amazonaws.fms#ListId` as the model declares it (36 characters matching
/// `^[a-z0-9A-Z-]{36}$`, from `smithy_query(query="shape", shape="ListId")`),
/// whose ASCII order is decided by `leading`: the ids this suite seeds are
/// written rather than minted, so the order each page is served in — and the way
/// the four pages interleave in it — is known.
fn list_id(leading: char) -> String {
    format!(
        "{}-3333-4c48-9f20-8d6b0f4c9e53",
        leading.to_string().repeat(8)
    )
}

/// One `DefaultLists`-selected page: the listing that serves it, the output
/// member carrying its items, the namespace it enumerates, the error code it
/// refuses a token naming no boundary of it with, and the leading characters of
/// the `ListId`s it holds.
struct ServedPage {
    operation: &'static str,
    items: &'static str,
    default_lists: bool,
    refusal: &'static str,
    leading: &'static [char],
}

impl ServedPage {
    /// How this page reads in an assertion message.
    fn label(&self) -> String {
        format!("{} / DefaultLists={}", self.operation, self.default_lists)
    }

    /// The `ListId`s this page holds, in the ascending order it serves them in.
    fn expected(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.leading.iter().copied().map(list_id).collect();
        ids.sort();
        ids
    }
}

/// The four pages the rule is asserted over. The leading characters round-robin
/// across them, so the SECOND list of each page — the boundary a first page of
/// one list ends at — sorts past the FIRST list of every other page.
fn pages() -> Vec<ServedPage> {
    vec![
        ServedPage {
            operation: "ListAppsLists",
            items: "AppsLists",
            default_lists: false,
            // The model declares no client-input error for either listing, so a
            // request one of them refuses is a `ValidationException`.
            refusal: "ValidationException",
            leading: &['1', '5', '9'],
        },
        ServedPage {
            operation: "ListAppsLists",
            items: "AppsLists",
            default_lists: true,
            refusal: "ValidationException",
            leading: &['2', '6', 'a'],
        },
        ServedPage {
            operation: "ListProtocolsLists",
            items: "ProtocolsLists",
            default_lists: false,
            refusal: "ValidationException",
            leading: &['3', '7', 'b'],
        },
        ServedPage {
            operation: "ListProtocolsLists",
            items: "ProtocolsLists",
            default_lists: true,
            refusal: "ValidationException",
            leading: &['4', '8', 'c'],
        },
    ]
}

/// A fresh emulator holding every page of [`pages`].
///
/// Seeded through the store seam the sibling suites use: a Firewall Manager
/// MANAGED list is created by no API, and a list with a CHOSEN `ListId` by none
/// either — and the ids are what has to interleave here.
async fn seeded() -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    with_store(&engine, |store| {
        for page in pages() {
            for id in page.expected() {
                match page.operation {
                    "ListAppsLists" => {
                        seed_apps_list(
                            store,
                            REGION,
                            &id,
                            &format!("apps-{id}"),
                            &[],
                            page.default_lists,
                        );
                    }
                    "ListProtocolsLists" => {
                        seed_protocols_list(
                            store,
                            &id,
                            &format!("protocols-{id}"),
                            &["TCP"],
                            page.default_lists,
                        );
                    }
                    other => panic!("case {other}: this suite seeds no list of that kind"),
                }
            }
        }
    });
    router
}

/// One call of a listing, as its HTTP status, the error code or an empty string,
/// the `ListId`s it served and its `NextToken`. `MaxResults` is REQUIRED by both
/// listings, so every call names a page size.
async fn page(
    router: &MultiServiceRouter,
    listing: &ServedPage,
    max_results: usize,
    token: Option<&str>,
) -> (u16, String, Vec<String>, Option<String>) {
    let mut request = json!({
        "DefaultLists": listing.default_lists,
        "MaxResults": max_results,
    });
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let (status, body) = call(router, listing.operation, &request).await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    let served: Vec<String> = body[listing.items]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["ListId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: a page item must carry its ListId, got {item:?}",
                        listing.label()
                    )
                })
                .to_string()
        })
        .collect();
    let next_token = body
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, served, next_token)
}

/// The listing's whole page, asserted to be served, to hold exactly the lists the
/// table says it does and to carry no `NextToken`.
async fn whole(router: &MultiServiceRouter, listing: &ServedPage) -> Vec<String> {
    let case = listing.label();
    let (status, code, served, next) = page(router, listing, state::MAX_PAGE_SIZE, None).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {case}: expected the whole page to be served, got {status} / {code:?}"
    );
    assert_eq!(
        (served.clone(), next.clone()),
        (listing.expected(), None),
        "case {case}: expected this page's own lists in ascending ListId order and no NextToken, \
         got {served:?} / {next:?}"
    );
    served
}

/// The `NextToken` a first page of ONE list hands back, with the page that call
/// served asserted to be exactly that page's first list — so the token is the
/// boundary of the rest of this page and nothing else.
async fn served_boundary(router: &MultiServiceRouter, listing: &ServedPage) -> String {
    let case = listing.label();
    let expected = listing.expected();
    let (status, code, served, next) = page(router, listing, 1, None).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {case}: expected a one-list page to be served, got {status} / {code:?}"
    );
    assert_eq!(
        served,
        expected[..1].to_vec(),
        "case {case}: expected the first list of this page, got {served:?}"
    );
    next.unwrap_or_else(|| {
        panic!(
            "case {case}: a page with {} lists left after it must carry a NextToken",
            expected.len() - 1
        )
    })
}

/// The precondition the refusal rows rest on: the boundary each page returns
/// after one list — that page's SECOND list — sorts past the FIRST list of every
/// other page, so resuming from it there would silently drop at least one list
/// rather than being harmless.
#[tokio::test]
async fn a_served_boundary_of_each_page_would_truncate_every_other() {
    let listings = pages();
    let mut all: Vec<String> = listings
        .iter()
        .flat_map(|listing| listing.expected())
        .collect();
    let seen = all.len();
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        seen,
        "case seeding: every page must hold ids of its own, got {all:?}"
    );
    for listing in &listings {
        let own = listing.expected();
        assert!(
            own.len() >= 2,
            "case {}: this suite needs a page that leaves a list after a one-list page, got \
             {own:?}",
            listing.label()
        );
        let boundary = own[1].clone();
        for other in &listings {
            if other.label() == listing.label() {
                continue;
            }
            let dropped: Vec<String> = other
                .expected()
                .into_iter()
                .filter(|id| *id < boundary)
                .collect();
            assert!(
                !dropped.is_empty(),
                "case {} vs {}: this suite needs the boundary {boundary} to sort past an entry of \
                 the second, got {:?}",
                listing.label(),
                other.label(),
                other.expected()
            );
        }
    }
}

/// A `NextToken` the server handed back for one of these pages names no boundary
/// of any OTHER of them: each of the three other pages refuses it with its own
/// error code, reports nothing, and is left exactly as it was.
#[tokio::test]
async fn a_served_token_is_refused_by_every_other_namespaced_page() {
    let router = seeded().await;
    let listings = pages();
    for listing in &listings {
        let minted = listing.label();
        let token = served_boundary(&router, listing).await;
        for other in &listings {
            if other.label() == minted {
                continue;
            }
            let case = format!("{minted} minted it, {} is asked", other.label());
            let before = whole(&router, other).await;
            let (status, code, served, next) =
                page(&router, other, state::MAX_PAGE_SIZE, Some(&token)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, other.refusal),
                "case {case}: a token naming no boundary of this page must be refused, got \
                 {status} / {code:?}"
            );
            assert!(
                served.is_empty() && next.is_none(),
                "case {case}: a refused call must report nothing, got {served:?} / {next:?}"
            );
            let after = whole(&router, other).await;
            assert_eq!(
                after, before,
                "case {case}: a refused token must leave the page as it was, got {after:?}"
            );
        }
    }
}

/// The page that MINTED the token resumes at it: the next call carries exactly
/// the lists of that page after the one already served, and the chain ends with
/// every list of that page reported once — so the scoping above refuses foreign
/// tokens without costing a page its own.
#[tokio::test]
async fn a_served_token_resumes_the_page_that_minted_it() {
    let router = seeded().await;
    for listing in &pages() {
        let case = listing.label();
        let expected = whole(&router, listing).await;
        let token = served_boundary(&router, listing).await;
        let (status, code, served, next) =
            page(&router, listing, state::MAX_PAGE_SIZE, Some(&token)).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case}: a token this page minted must resume it, got {status} / {code:?}"
        );
        assert_eq!(
            (served.clone(), next.clone()),
            (expected[1..].to_vec(), None),
            "case {case}: expected the rest of this page and no NextToken, got {served:?} / \
             {next:?}"
        );
    }
}
