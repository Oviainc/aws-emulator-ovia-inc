// SPDX-License-Identifier: CC0-1.0

//! What `ListLinkedWhatsAppBusinessAccounts` reports beside the page.
//!
//! RULE: the listing reports the account-level `setupEnabled` fact as a top-level member
//! beside `linkedAccounts`, read from the store — so every page of every listing carries
//! it, whatever that page holds, including a listing with nothing in it, and it reports
//! `false` exactly when the account's stored fact is `false`.
//!
//! The rows span what the listing HOLDS (nothing, one account, several, one page of
//! several, the page a `nextToken` resumes, and a listing emptied again by a
//! disassociation) and, for each, both values of the stored fact — the dimensions the flag
//! is independent of, and the one it is not.

use serde_json::json;
use service_com_amazonaws_socialmessaging_socialmessaging::state::{
    ComAmazonawsSocialmessagingSocialmessagingState as Store, SETUP_ENABLED_DEFAULT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, emulator_with_engine, link, query_escape, Answer};

/// The three accounts the paging rows list, in the order their minted ids sort.
const WABA_IDS: [&str; 3] = [
    "waba-11111111111111111111111111111111",
    "waba-22222222222222222222222222222222",
    "waba-33333333333333333333333333333333",
];

async fn listing(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/v1/whatsapp/waba/list{query}"), None).await
}

/// Every listing of one account reports that account's stored fact, whatever the page
/// holds and however the page was asked for.
#[tokio::test]
async fn every_page_reports_the_accounts_stored_setup_fact() {
    for setup_enabled in [true, false] {
        let (router, engine) = emulator_with_engine();
        engine.state::<Store>().lock().setup_enabled = setup_enabled;

        // An empty account first: the flag is not derived from the listing, so it is
        // reported where there is nothing to list.
        let empty = listing(&router, "").await;
        assert_eq!(
            (empty.status, empty.at("/linkedAccounts")),
            (200, &json!([])),
            "case setup_enabled={setup_enabled}, nothing linked: expected an empty 200 \
             listing, got {} {:?}",
            empty.status,
            empty.body
        );
        assert_eq!(
            empty.at("/setupEnabled"),
            &json!(setup_enabled),
            "case setup_enabled={setup_enabled}, nothing linked: expected setupEnabled \
             {setup_enabled}, got {:?}",
            empty.body
        );

        let mut linked = Vec::new();
        for waba_id in WABA_IDS {
            linked.push(link(&router, waba_id, &["phone-number-id-a"]).await);
        }
        linked.sort();

        // Each row is one way of asking for a page: the whole listing, a page of one, the
        // page its token resumes, and a page bigger than the listing.
        let first = listing(&router, "?maxResults=1").await;
        let resumed_query = format!(
            "?maxResults=1&nextToken={}",
            query_escape(&first.string_at("/nextToken"))
        );
        let pages = [
            ("the whole listing", listing(&router, "").await, 3),
            ("a page of one", first, 1),
            ("the page its token resumes", listing(&router, &resumed_query).await, 1),
            ("a page wider than the listing", listing(&router, "?maxResults=100").await, 3),
        ];
        for (case, page, expected_len) in pages {
            let listed = page.at("/linkedAccounts").as_array().cloned().unwrap_or_default();
            assert_eq!(
                (page.status, listed.len()),
                (200, expected_len),
                "case setup_enabled={setup_enabled}, {case}: expected a 200 page of \
                 {expected_len}, got {} {:?}",
                page.status,
                page.body
            );
            assert_eq!(
                page.at("/setupEnabled"),
                &json!(setup_enabled),
                "case setup_enabled={setup_enabled}, {case}: expected setupEnabled \
                 {setup_enabled}, got {:?}",
                page.body
            );
        }

        // And a listing emptied again: unlinking every account does not unset the fact.
        for id in &linked {
            let gone = call(
                &router,
                "DELETE",
                &format!("/v1/whatsapp/waba/disassociate?id={}", query_escape(id)),
                None,
            )
            .await;
            assert_eq!(
                gone.status, 200,
                "case setup_enabled={setup_enabled}, disassociating {id:?}: expected 200, \
                 got {} {:?}",
                gone.status, gone.body
            );
        }
        let emptied = listing(&router, "").await;
        assert_eq!(
            (emptied.at("/linkedAccounts"), emptied.at("/setupEnabled")),
            (&json!([]), &json!(setup_enabled)),
            "case setup_enabled={setup_enabled}, emptied again: expected an empty listing \
             still reporting setupEnabled {setup_enabled}, got {:?}",
            emptied.body
        );
    }
}

/// A store nobody has touched reports the account fact it starts from, so a caller reading
/// the flag to decide whether to drive the sign-up flow gets a value rather than nothing.
#[tokio::test]
async fn a_fresh_account_reports_the_default_setup_fact() {
    let router = emulator();
    let answer = listing(&router, "").await;
    assert_eq!(
        answer.at("/setupEnabled"),
        &json!(SETUP_ENABLED_DEFAULT),
        "a fresh account: expected setupEnabled {SETUP_ENABLED_DEFAULT}, got {:?}",
        answer.body
    );
}
