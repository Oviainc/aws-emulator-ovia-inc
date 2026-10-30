// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE page walk both `DefaultLists`-selected listings
//! of this service serve a page through.
//!
//! RULE under test: a page of such a listing reports exactly the lists of the
//! collection it enumerates in the namespace `DefaultLists` selects, in
//! ascending `ListId` order, bounded by the shared page-size rule, and carries a
//! `NextToken` exactly when a list of that page is left after it — so a page is
//! short only when its own page is exhausted, and walking the tokens a page
//! mints reports that page whole, once, whatever the page size. Which listing is
//! asked decides only the store the page is drawn from and the projection each
//! item is reported as: both resolve the page through the same walk, so neither
//! can drift from the other on how a namespace is selected, where a page starts
//! or when it ends.
//!
//! Asserted over a TABLE whose rows are the four such pages — `ListAppsLists`
//! and `ListProtocolsLists`, each in the Firewall Manager MANAGED namespace and
//! in the customer one. The rows differ in the collection they enumerate (two
//! stores whose `ListId`s are minted independently of each other), in the
//! namespace flag, in the output member carrying the items, in the projection
//! each item is reported as, and in HOW MANY lists the page holds: none, one,
//! four and seven, so the walk is covered on an empty page, a single-page page
//! and pages that both divide evenly by a page size and do not.
//!
//! The ids of the four pages INTERLEAVE in ascending `ListId` order, so a walk
//! that read the wrong store or ignored the namespace flag would report a
//! foreign list rather than quietly agreeing;
//! [`the_pages_this_suite_serves_interleave`] pins that precondition.
//!
//! The token a page mints names no boundary of any OTHER of these pages; that
//! refusal is the sibling suite
//! [`crate::list_namespace_page_token_scoping`]'s subject and is not re-asserted
//! here.
//!
//! Every call goes over the wire — an awsJson1_1 `POST /` through the harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::App;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{
    associate_admin, seed_apps_list, seed_protocols_list, with_store,
};
use crate::policy_lifecycle::{call, emulator_with_engine, REGION};

/// A `com.amazonaws.fms#ListId` as the model declares it (36 characters matching
/// `^[a-z0-9A-Z-]{36}$`), whose ASCII order is decided by `leading`: the ids this
/// suite seeds are written rather than minted, so the order each page is served
/// in — and the way the four pages interleave in it — is known.
fn list_id(leading: char) -> String {
    format!(
        "{}-2222-4b37-8e1f-7c5a9e3b8d42",
        leading.to_string().repeat(8)
    )
}

/// One `DefaultLists`-selected page: the listing that serves it, the output
/// member carrying its items, the member each item reports its contents under,
/// the collection its boundaries are stamped with, the namespace it enumerates,
/// and the leading characters of the `ListId`s it holds.
struct ListingPage {
    operation: &'static str,
    items: &'static str,
    contents: &'static str,
    collection: &'static str,
    default_lists: bool,
    leading: &'static [char],
}

impl ListingPage {
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

    /// The `ListName` the seeding gives the list `list_id` names. It carries the
    /// collection and the namespace, so an item served by the wrong store or out
    /// of the wrong namespace is named as such instead of passing for this
    /// page's own.
    fn list_name(&self, list_id: &str) -> String {
        format!("{}-{}-{list_id}", self.items, self.default_lists)
    }

    /// The `NextToken` this page mints for the boundary `list_id`.
    fn token(&self, list_id: &str) -> String {
        state::encode_namespaced_list_page_token(self.collection, self.default_lists, list_id)
    }
}

/// The applications this suite stores on an applications list, so an
/// `AppsListDataSummary` carries a non-empty `AppsList` and a summary of the
/// other collection's projection cannot pass for it.
fn apps() -> Vec<App> {
    vec![App {
        app_name: "https".to_string(),
        protocol: "TCP".to_string(),
        port: 443,
    }]
}

/// The protocols this suite stores on a protocols list, the protocols-list twin
/// of [`apps`].
const PROTOCOLS: &[&str] = &["TCP", "UDP"];

/// The four pages the rule is asserted over, differing in collection, in
/// namespace and in how many lists they hold (none, one, four, seven). The
/// leading characters interleave across all of them.
fn pages() -> Vec<ListingPage> {
    vec![
        ListingPage {
            operation: "ListAppsLists",
            items: "AppsLists",
            contents: "AppsList",
            collection: state::APPS_LISTS_PAGE,
            default_lists: false,
            leading: &['a', 'e', 'i', 'm', 'q', 'u', 'y'],
        },
        ListingPage {
            operation: "ListAppsLists",
            items: "AppsLists",
            contents: "AppsList",
            collection: state::APPS_LISTS_PAGE,
            default_lists: true,
            leading: &[],
        },
        ListingPage {
            operation: "ListProtocolsLists",
            items: "ProtocolsLists",
            contents: "ProtocolsList",
            collection: state::PROTOCOLS_LISTS_PAGE,
            default_lists: false,
            leading: &['c'],
        },
        ListingPage {
            operation: "ListProtocolsLists",
            items: "ProtocolsLists",
            contents: "ProtocolsList",
            collection: state::PROTOCOLS_LISTS_PAGE,
            default_lists: true,
            leading: &['b', 'f', 'j', 'n'],
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
                let list_name = page.list_name(&id);
                match page.operation {
                    "ListAppsLists" => {
                        seed_apps_list(
                            store,
                            REGION,
                            &id,
                            &list_name,
                            &apps(),
                            page.default_lists,
                        );
                    }
                    "ListProtocolsLists" => {
                        seed_protocols_list(
                            store,
                            &id,
                            &list_name,
                            PROTOCOLS,
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

/// One call of a listing, as the `(ListId, ListName)` pairs it served and the
/// `NextToken` it carries. Every item is asserted to be the projection this
/// listing reports — its own contents member, carrying what the seeding stored —
/// so a page drawn from the other collection's store is caught here.
async fn page(
    router: &MultiServiceRouter,
    listing: &ListingPage,
    max_results: usize,
    token: Option<&str>,
) -> (Vec<(String, String)>, Option<String>) {
    let case = listing.label();
    let mut request = json!({
        "DefaultLists": listing.default_lists,
        "MaxResults": max_results,
    });
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    let (status, body) = call(router, listing.operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case}: a page of this listing must be served, got {status} with {body}"
    );
    let served: Vec<(String, String)> = body[listing.items]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let id = item["ListId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a page item must carry its ListId, got {item:?}")
                })
                .to_string();
            let name = item["ListName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a page item must carry its ListName, got {item:?}")
                })
                .to_string();
            let want_contents = if listing.contents == "AppsList" {
                json!([{ "AppName": "https", "Protocol": "TCP", "Port": 443 }])
            } else {
                json!(PROTOCOLS)
            };
            assert_eq!(
                item[listing.contents], want_contents,
                "case {case} / {id}: an item must be reported as this listing's own projection, \
                 got {item:?}"
            );
            (id, name)
        })
        .collect();
    let next_token = body
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string);
    (served, next_token)
}

/// The precondition the rows rest on: the ids of the four pages interleave, so
/// a page that read the wrong store, or ignored the namespace flag, would report
/// a list this page does not hold rather than agreeing by accident.
#[tokio::test]
async fn the_pages_this_suite_serves_interleave() {
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
        if own.is_empty() {
            continue;
        }
        for other in &listings {
            let foreign = other.expected();
            if other.label() == listing.label() || foreign.is_empty() {
                continue;
            }
            let interleaving = foreign.iter().any(|id| own.iter().any(|mine| mine < id));
            assert!(
                interleaving,
                "case {} vs {}: this suite needs an id of the second to sort past an entry of the \
                 first, got {own:?} against {foreign:?}",
                listing.label(),
                other.label()
            );
        }
    }
}

/// The whole page of each listing: exactly its own collection-and-namespace's
/// lists, in ascending `ListId` order, under the names the seeding gave them,
/// and no `NextToken` — including the page that holds no list at all.
#[tokio::test]
async fn a_whole_page_reports_its_own_namespace_of_its_own_collection() {
    let router = seeded().await;
    for listing in &pages() {
        let case = listing.label();
        let (served, next) = page(&router, listing, state::MAX_PAGE_SIZE, None).await;
        let want: Vec<(String, String)> = listing
            .expected()
            .iter()
            .map(|id| (id.clone(), listing.list_name(id)))
            .collect();
        assert_eq!(
            served, want,
            "case {case}: expected this page's own lists in ascending ListId order, got {served:?}"
        );
        assert!(
            next.is_none(),
            "case {case}: a page holding every one of its lists must carry no NextToken, got \
             {next:?}"
        );
    }
}

/// Every page size: a page holds at most that many lists and is short only when
/// its own page is exhausted, it carries a `NextToken` exactly then, and walking
/// those tokens reports the page whole, once, in ascending `ListId` order.
#[tokio::test]
async fn every_page_size_walks_the_same_page_once() {
    let router = seeded().await;
    for listing in &pages() {
        let case_page = listing.label();
        let expected = listing.expected();
        // Page sizes inside the modelled `PaginationMaxResults` range (1..100,
        // from smithy_query(query="shape", shape="PaginationMaxResults")): both
        // published bounds, and sizes that divide these pages evenly (2, 4) and
        // unevenly (3).
        for max_results in [1usize, 2, 3, 4, state::MAX_PAGE_SIZE] {
            let case = format!("{case_page} / MaxResults {max_results}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut ended = false;
            // One more round than the chain can need, so a token that never
            // clears fails here instead of looping forever.
            for round in 0..expected.len() + 3 {
                let (served, next) = page(&router, listing, max_results, token.as_deref()).await;
                assert!(
                    served.len() <= max_results,
                    "case {case} / page {round}: a page must not exceed the requested size, got \
                     {served:?}"
                );
                walked.extend(served.iter().map(|(id, _)| id.clone()));
                match next {
                    Some(next) => {
                        assert_eq!(
                            served.len(),
                            max_results,
                            "case {case} / page {round}: a page carrying a NextToken must be a \
                             full page, got {served:?}"
                        );
                        assert!(
                            walked.len() < expected.len(),
                            "case {case} / page {round}: a NextToken is minted only while a list \
                             of this page is left, got {walked:?}"
                        );
                        token = Some(next);
                    }
                    None => {
                        ended = true;
                        break;
                    }
                }
            }
            assert!(
                ended,
                "case {case}: the NextToken chain did not end, walked {walked:?}"
            );
            assert_eq!(
                walked, expected,
                "case {case}: walking this page must report its whole page exactly once in \
                 ascending ListId order, got {walked:?}"
            );
        }
    }
}

/// A boundary of this page resumes it AT that `ListId`: the page the token names
/// is the suffix of the whole page from that list onwards, at every page size.
#[tokio::test]
async fn a_boundary_of_this_page_resumes_it_there() {
    let router = seeded().await;
    for listing in &pages() {
        let case_page = listing.label();
        let expected = listing.expected();
        for (index, id) in expected.iter().enumerate() {
            let (served, _) = page(
                &router,
                listing,
                state::MAX_PAGE_SIZE,
                Some(&listing.token(id)),
            )
            .await;
            let ids: Vec<String> = served.iter().map(|(id, _)| id.clone()).collect();
            assert_eq!(
                ids,
                expected[index..].to_vec(),
                "case {case_page} / boundary {id}: expected the page to resume at that list, got \
                 {ids:?}"
            );
        }
    }
}
