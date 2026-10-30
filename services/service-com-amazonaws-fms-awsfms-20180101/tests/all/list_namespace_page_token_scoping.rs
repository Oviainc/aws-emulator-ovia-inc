// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the token rule the two `DefaultLists`-selected listings
//! of this service share.
//!
//! The rule: a `NextToken` names a page boundary only in the collection that
//! minted it, so a token minted for one page is refused by every other page of
//! the same key class rather than resumed from as an arbitrary key — a boundary
//! of one page may be absent from the next, or sort past lists the next does
//! hold, so resuming from it there would silently drop them and report a short
//! page as a complete one. Here a page is identified by BOTH halves of what
//! selects it: the collection the listing enumerates, and the namespace
//! `DefaultLists` picks inside it. A boundary carrying only one half — the bare
//! `ListId`, or the namespace with no collection — is therefore no boundary of
//! any of these pages either. A refused call reports nothing and leaves the page
//! as it was, and the token a page does mint still walks that page whole.
//!
//! Asserted over a TABLE whose rows are the four such pages: `ListAppsLists` and
//! `ListProtocolsLists`, each in the Firewall Manager MANAGED namespace and in
//! the customer one. The rows therefore differ in the collection they enumerate
//! (two stores whose `ListId`s are minted independently of each other), in the
//! namespace flag, in how many lists the page holds (two and three) and in the
//! output member carrying the items.
//!
//! The four pages are seeded to INTERLEAVE in ascending `ListId` order — the
//! order every one of these pages is served in — so a cross-page token
//! truncates the page it is replayed against;
//! [`the_pages_this_suite_serves_differ_and_interleave`] pins that precondition,
//! so the refusal rows below cannot quietly stop covering it.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`] — so the assertions
//! are about the WIRE response.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{
    associate_admin, seed_apps_list, seed_protocols_list, with_store,
};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, REGION};

/// A `com.amazonaws.fms#ListId` as the model declares it (36 characters matching
/// `^[a-z0-9A-Z-]{36}$`), whose ASCII order is decided by `leading`: the ids this
/// suite seeds are written rather than minted, so the order each page is served
/// in — and the way the four pages interleave in it — is known.
fn list_id(leading: char) -> String {
    format!(
        "{}-1111-4a26-9d0e-6b4f8d2a7c31",
        leading.to_string().repeat(8)
    )
}

/// One `DefaultLists`-selected page: the listing that serves it, the output
/// member carrying its items, the collection its boundaries are stamped with,
/// the namespace it enumerates, the error code it refuses a token naming no
/// boundary of it with, and the leading characters of the `ListId`s it holds.
struct ListPage {
    operation: &'static str,
    items: &'static str,
    collection: &'static str,
    default_lists: bool,
    refusal: &'static str,
    leading: &'static [char],
}

impl ListPage {
    /// How this page reads in an assertion message.
    fn label(&self) -> String {
        format!("{} / DefaultLists={}", self.operation, self.default_lists)
    }

    /// The `NextToken` this page mints for the boundary `list_id`.
    fn token(&self, list_id: &str) -> String {
        state::encode_namespaced_list_page_token(self.collection, self.default_lists, list_id)
    }

    /// The `ListId`s this page holds, in the ascending order it serves them in.
    fn expected(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.leading.iter().copied().map(list_id).collect();
        ids.sort();
        ids
    }
}

/// The four pages the rule is asserted over. The leading characters interleave
/// across ALL of them, so every pair has a boundary of the one sorting past an
/// entry of the other.
fn pages() -> Vec<ListPage> {
    vec![
        ListPage {
            operation: "ListAppsLists",
            items: "AppsLists",
            collection: state::APPS_LISTS_PAGE,
            default_lists: false,
            // The model declares no client-input error for either listing, so a
            // request one of them refuses is a `ValidationException`.
            refusal: "ValidationException",
            leading: &['1', '5'],
        },
        ListPage {
            operation: "ListAppsLists",
            items: "AppsLists",
            collection: state::APPS_LISTS_PAGE,
            default_lists: true,
            refusal: "ValidationException",
            leading: &['3', '8'],
        },
        ListPage {
            operation: "ListProtocolsLists",
            items: "ProtocolsLists",
            collection: state::PROTOCOLS_LISTS_PAGE,
            default_lists: false,
            refusal: "ValidationException",
            leading: &['2', '6', '9'],
        },
        ListPage {
            operation: "ListProtocolsLists",
            items: "ProtocolsLists",
            collection: state::PROTOCOLS_LISTS_PAGE,
            default_lists: true,
            refusal: "ValidationException",
            leading: &['0', '4'],
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
            for list_id in page.expected() {
                match page.operation {
                    "ListAppsLists" => {
                        seed_apps_list(
                            store,
                            REGION,
                            &list_id,
                            &format!("apps-{list_id}"),
                            &[],
                            page.default_lists,
                        );
                    }
                    "ListProtocolsLists" => {
                        seed_protocols_list(
                            store,
                            &list_id,
                            &format!("protocols-{list_id}"),
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
/// the list ids it served and its NextToken. `MaxResults` is REQUIRED by both
/// listings, so a call that names no page size asks for the largest page the
/// service serves.
async fn page(
    router: &MultiServiceRouter,
    listing: &ListPage,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (u16, String, Vec<String>, Option<String>) {
    let mut request = json!({
        "DefaultLists": listing.default_lists,
        "MaxResults": max_results.unwrap_or(state::MAX_PAGE_SIZE),
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

/// The listing's whole page, asserted to be served, to carry no token and to
/// hold exactly the lists the table says it does.
async fn whole(router: &MultiServiceRouter, listing: &ListPage) -> Vec<String> {
    let case = listing.label();
    let (status, code, served, next) = page(router, listing, None, None).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {case}: expected the whole page to be served, got {status} / {code:?}"
    );
    assert_eq!(
        (served.clone(), next.clone()),
        (listing.expected(), None),
        "case {case}: expected that page's own lists in ascending ListId order and no NextToken, \
         got {served:?} / {next:?}"
    );
    served
}

/// The precondition the refusal rows rest on: the four pages are pairwise
/// different, and each pair has a boundary of the one that sorts past at least
/// one entry of the other — so resuming from a foreign boundary would silently
/// drop lists rather than being harmless.
#[tokio::test]
async fn the_pages_this_suite_serves_differ_and_interleave() {
    let router = seeded().await;
    let listings = pages();
    for listing in &listings {
        let served = whole(&router, listing).await;
        for other in &listings {
            if other.label() == listing.label() {
                continue;
            }
            let other_served = whole(&router, other).await;
            assert_ne!(
                served, other_served,
                "case {} vs {}: this suite needs the two pages to differ, got {served:?} for both",
                listing.label(),
                other.label()
            );
            let truncating = other_served
                .iter()
                .any(|boundary| served.iter().any(|list_id| list_id < boundary));
            assert!(
                truncating,
                "case {} vs {}: this suite needs a boundary of the second to sort past an entry \
                 of the first, got {served:?} against {other_served:?}",
                listing.label(),
                other.label()
            );
        }
    }
}

/// A token minted for ANOTHER of these pages — the other namespace of the same
/// collection, or either namespace of the other collection — names no boundary
/// of this page, and neither does a boundary carrying only one half of what
/// identifies a page: each is refused with the operation's error code, reports
/// nothing, and leaves the page as it was.
#[tokio::test]
async fn a_token_of_another_namespaced_list_page_is_refused() {
    let router = seeded().await;
    let listings = pages();
    for listing in &listings {
        let case_page = listing.label();
        let expected = whole(&router, listing).await;

        let mut foreign: Vec<(String, String)> = Vec::new();
        for other in &listings {
            if other.label() == listing.label() {
                continue;
            }
            // Every boundary of the other page, so the rows cover a boundary
            // that sorts inside this page, one that sorts past all of it, and
            // one this page does not hold at all.
            for list_id in other.expected() {
                foreign.push((
                    format!("a token {} minted for {list_id}", other.label()),
                    other.token(&list_id),
                ));
            }
        }
        for list_id in &expected {
            // The bare `ListId`, carrying neither collection nor namespace.
            foreign.push((
                format!("the bare ListId {list_id}, carrying no page at all"),
                hex::encode(list_id),
            ));
            // One half only: the namespace with no collection, which both
            // collections' pages would otherwise answer to alike.
            foreign.push((
                format!("{list_id} stamped with the namespace but no collection"),
                state::encode_scoped_page_token(&listing.default_lists.to_string(), list_id),
            ));
            // The other half only: the collection with no namespace.
            foreign.push((
                format!("{list_id} stamped with the collection but no namespace"),
                state::encode_scoped_page_token(listing.collection, list_id),
            ));
        }

        for (case, token) in foreign {
            let (status, code, served, next) =
                page(&router, listing, None, Some(&token)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, listing.refusal),
                "case {case_page} / {case}: a token naming no boundary of this page must be \
                 refused, got {status} / {code:?}"
            );
            assert!(
                served.is_empty() && next.is_none(),
                "case {case_page} / {case}: a refused call must report nothing, got {served:?} / \
                 {next:?}"
            );

            let after = whole(&router, listing).await;
            assert_eq!(
                after, expected,
                "case {case_page} / {case}: a refused token must leave the page as it was, got \
                 {after:?}"
            );
        }
    }
}

/// The token a page MINTS names a boundary of that page: it resumes it exactly
/// at that `ListId`, and walking the chain reports that page's lists exactly
/// once, in ascending `ListId` order, at every page size.
#[tokio::test]
async fn a_pages_own_token_resumes_and_walks_that_page() {
    let router = seeded().await;
    for listing in &pages() {
        let case_page = listing.label();
        let expected = whole(&router, listing).await;

        for (index, list_id) in expected.iter().enumerate() {
            let (status, code, served, _) =
                page(&router, listing, None, Some(&listing.token(list_id))).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case_page} / boundary {list_id}: a token this page minted must resume it, \
                 got {status} / {code:?}"
            );
            assert_eq!(
                served,
                expected[index..].to_vec(),
                "case {case_page} / boundary {list_id}: expected the page to resume at that list, \
                 got {served:?}"
            );
        }

        // Page sizes inside the modelled `PaginationMaxResults` range (1..100):
        // the smallest, one below the page's own length, and the largest.
        for max_results in [1usize, 2, state::MAX_PAGE_SIZE] {
            let case = format!("MaxResults {max_results}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut ended = false;
            // One more round than the chain can need, so a token that never
            // clears fails here instead of looping forever.
            for round in 0..expected.len() + 3 {
                let (status, code, served, next) =
                    page(&router, listing, Some(max_results), token.as_deref()).await;
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {case_page} / {case} / page {round}: expected 200, got {status} / \
                     {code:?}"
                );
                assert!(
                    served.len() <= max_results,
                    "case {case_page} / {case} / page {round}: a page must not exceed the \
                     requested size, got {served:?}"
                );
                walked.extend(served);
                match next {
                    Some(next) => token = Some(next),
                    None => {
                        ended = true;
                        break;
                    }
                }
            }
            assert!(
                ended,
                "case {case_page} / {case}: the NextToken chain did not end, walked {walked:?}"
            );
            assert_eq!(
                walked, expected,
                "case {case_page} / {case}: walking this page's own tokens must report its whole \
                 page exactly once in ascending ListId order, got {walked:?}"
            );
        }
    }
}
