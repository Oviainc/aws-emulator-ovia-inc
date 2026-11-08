// SPDX-License-Identifier: CC0-1.0

//! `ListConfigurationSets`, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call
//! is a real awsQuery request against the service: form-urlencoded body in, XML
//! out, one shared engine behind it. What is asserted is the RULE the operation
//! follows over a SET of inputs rather than one recorded request:
//!
//! * the listing reports the configuration sets this account holds, in one stable
//!   order, and paging with `MaxItems`/`NextToken` visits every set exactly once
//!   for EVERY page size;
//! * a set created over the wire appears in the listing, and one dropped from the
//!   store disappears from it, so the answer is computed from state rather than
//!   fixed;
//! * `MaxItems` follows the bounds the operation publishes: a value past the
//!   ceiling is clamped to it, a value below the floor is refused, and an absent
//!   value takes the ceiling as the page size;
//! * a `NextToken` this listing did not issue is refused with HTTP 400
//!   `InvalidParameterValue`, whatever shape the token has, and the store is left
//!   alone.

use crate::harness::{call, element, elements, emulator_over, field, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    encode_page_token, service_state, LISTING_CONFIGURATION_SETS,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The largest page this operation serves, and so also the page size a request
/// that names none is served: "This operation returns up to 1,000 configuration
/// sets each time it is run. If your Amazon SES account has more than 1,000
/// configuration sets, this operation also returns NextToken"
/// (`ListConfigurationSets`).
const MAX_PAGE_SIZE: usize = 1_000;

/// A router over an engine this suite keeps, so a suite can seed or read this
/// service's store beside the calls it makes over the wire.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    (emulator_over(Arc::clone(&engine)), engine)
}

/// A router holding one configuration set per name in `names`, recorded through
/// the same state API the handlers use, and the names in the order the listing
/// reports them (lexicographic).
fn emulator_holding(names: &[String]) -> (MultiServiceRouter, Vec<String>) {
    let (router, engine) = emulator_with_store();
    let store = service_state(&*engine).expect("this service has a store on the shared engine");
    for name in names {
        let recorded = store.lock().add_configuration_set(name).is_some();
        assert!(
            recorded,
            "case {name:?}: a configuration set inside the account's limit must be recorded"
        );
    }
    let mut expected = names.to_vec();
    expected.sort();
    (router, expected)
}

/// `set-<index>` for each of `count` configuration sets, in an order whose
/// lexicographic sort differs from the insertion order.
fn set_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("set-{:04}", count - index))
        .collect()
}

/// One `ListConfigurationSets` call: `(status, body)`.
async fn list_raw(
    router: &MultiServiceRouter,
    max_items: Option<i32>,
    next_token: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = Vec::new();
    if let Some(max_items) = max_items {
        params.push(field("MaxItems", max_items.to_string()));
    }
    if let Some(next_token) = next_token {
        params.push(field("NextToken", next_token));
    }
    call(router, "ListConfigurationSets", &params).await
}

/// `ListConfigurationSets`, as `(the names on this page, the next token)`.
/// Asserts the call succeeded and that every reported entry carries its name.
async fn list(
    router: &MultiServiceRouter,
    max_items: Option<i32>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let (status, body) = list_raw(router, max_items, next_token).await;
    assert_eq!(
        status, 200,
        "case ListConfigurationSets(max={max_items:?}, token={next_token:?}): expected 200, \
         got {status} with {body}"
    );
    let page = element(&body, "ConfigurationSets")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    let names = page
        .iter()
        .map(|member| {
            element(member, "Name").unwrap_or_else(|| {
                panic!(
                    "case ListConfigurationSets(max={max_items:?}): an entry carries Name, \
                     got {member}"
                )
            })
        })
        .collect();
    (names, element(&body, "NextToken"))
}

/// Every page of the listing at `max_items`, concatenated, following each
/// `NextToken` until none comes back. Also asserts no page is larger than the page
/// size the request asks for, and that only the last page is short.
async fn every_page(
    router: &MultiServiceRouter,
    max_items: Option<i32>,
    page_size: usize,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count this suite can ask for, so a listing that
    // never terminates fails here instead of looping forever.
    for round in 0..2_000 {
        let (page, next) = list(router, max_items, token.as_deref()).await;
        assert!(
            page.len() <= page_size,
            "case max={max_items:?} page {round}: expected at most {page_size} entries, got {}",
            page.len()
        );
        assert!(
            next.is_none() || page.len() == page_size,
            "case max={max_items:?} page {round}: a page that is not the last must be full, \
             got {} of {page_size}",
            page.len()
        );
        seen.extend(page);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case max={max_items:?}: the listing never ran out of pages");
}

/// One `CreateConfigurationSet` call, asserted to have succeeded.
async fn create(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateConfigurationSet({name:?}): expected 200, got {status} with {body}"
    );
}

#[tokio::test]
async fn an_account_with_no_configuration_sets_lists_nothing() {
    let (router, _) = emulator_holding(&[]);
    for max_items in [None, Some(1), Some(10), Some(MAX_PAGE_SIZE as i32)] {
        let (page, token) = list(&router, max_items, None).await;
        assert!(
            page.is_empty(),
            "case max={max_items:?}: expected no configuration sets, got {page:?}"
        );
        assert_eq!(
            token, None,
            "case max={max_items:?}: expected no NextToken, got {token:?}"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_configuration_set_exactly_once_at_every_page_size() {
    const STORED: usize = 23;
    let (router, expected) = emulator_holding(&set_names(STORED));

    // EVERY page size from one entry to one past the whole store, so the rule is
    // asserted over the whole class rather than at one size.
    let sizes: Vec<Option<i32>> = std::iter::once(None)
        .chain((1..=(STORED + 1)).map(|size| Some(size as i32)))
        .collect();
    for max_items in sizes {
        let page_size = max_items.map_or(MAX_PAGE_SIZE, |size| size as usize);
        let seen = every_page(&router, max_items, page_size).await;
        assert_eq!(
            seen, expected,
            "case max={max_items:?}: expected every stored configuration set once, in name \
             order, got {seen:?}"
        );
    }

    // A store smaller than one page is served whole, with no cursor: there is
    // nothing after it to resume at.
    let (page, token) = list(&router, None, None).await;
    assert_eq!(
        page, expected,
        "case absent MaxItems: expected the whole store on one page, got {page:?}"
    );
    assert_eq!(
        token, None,
        "case absent MaxItems: a store that fits one page yields no NextToken, got {token:?}"
    );
}

#[tokio::test]
async fn the_listing_reports_what_the_account_holds_right_now() {
    let (router, _engine) = emulator_with_store();

    // Names differing in the ways a configuration set name can differ — length,
    // case, digits, underscores, dashes — created in an order that is not the
    // order the listing reports.
    // The longest name a configuration set may carry: `ConfigurationSet$Name`
    // documents "Contain 64 characters or fewer".
    let longest = "n".repeat(64);
    let created: Vec<String> = [
        "zeta-set",
        "Alpha_Set",
        "a",
        "SET-2",
        "set-2",
        "_leading-underscore",
        "0",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .chain(std::iter::once(longest.clone()))
    .collect();
    let mut expected: Vec<String> = Vec::new();
    for name in &created {
        create(&router, name).await;
        expected.push(name.clone());
        expected.sort();
        let seen = every_page(&router, None, MAX_PAGE_SIZE).await;
        assert_eq!(
            seen, expected,
            "case after creating {name:?}: the listing must report every set the account \
             holds, got {seen:?}"
        );
    }

    // And a set the account no longer holds drops out of the listing, so the
    // answer is read from state on every call rather than fixed at the first.
    for name in ["a", "set-2", longest.as_str()] {
        // Deleted through the operation that owns the set, which is what takes it from the
        // account: a set this service holds is one the ACCOUNT holds — the SES v2 surface
        // answers for it too (`service_common::ses_configuration_sets`) — so a removal
        // straight out of this service's own store would leave the account still holding it.
        let (status, body) = call(
            &router,
            "DeleteConfigurationSet",
            &[field("ConfigurationSetName", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case delete {name:?}: expected 200, got {status} with {body}"
        );
        expected.retain(|held| held != name);
        let seen = every_page(&router, None, MAX_PAGE_SIZE).await;
        assert_eq!(
            seen, expected,
            "case after dropping {name:?}: the listing must not report it, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_past_the_ceiling_is_clamped_and_one_below_the_floor_is_refused() {
    // More sets than the largest page the operation serves, so a clamp is
    // observable as a short page plus a cursor.
    const STORED: usize = MAX_PAGE_SIZE + 17;
    let (router, expected) = emulator_holding(&set_names(STORED));

    // At and inside the ceiling the page size is honoured exactly; past it, the
    // operation serves the largest page it serves rather than refusing.
    let rows: &[(Option<i32>, usize)] = &[
        (Some(1), 1),
        (Some(2), 2),
        (Some(999), 999),
        (Some(MAX_PAGE_SIZE as i32), MAX_PAGE_SIZE),
        (Some(MAX_PAGE_SIZE as i32 + 1), MAX_PAGE_SIZE),
        (Some(100_000), MAX_PAGE_SIZE),
        (Some(i32::MAX), MAX_PAGE_SIZE),
        // An absent value takes the same ceiling as the page size.
        (None, MAX_PAGE_SIZE),
    ];
    for (max_items, page_size) in rows {
        let (page, token) = list(&router, *max_items, None).await;
        assert_eq!(
            page.len(),
            *page_size,
            "case MaxItems={max_items:?}: expected a page of {page_size}, got {}",
            page.len()
        );
        assert!(
            token.is_some(),
            "case MaxItems={max_items:?}: {STORED} stored sets do not fit a page of \
             {page_size}, so a NextToken must come back"
        );
        // Paging at the served size still visits every set exactly once. Walked
        // only for the page sizes whose walk is a handful of calls; page size one
        // is walked whole in `paging_visits_every_configuration_set_exactly_once_at_every_page_size`.
        if *page_size >= 999 {
            let seen = every_page(&router, *max_items, *page_size).await;
            assert_eq!(
                seen, expected,
                "case MaxItems={max_items:?}: paging must still visit every set once"
            );
        }
    }

    // Below the floor the value is refused, naming the member and the value.
    for max_items in [0, -1, -1_000, i32::MIN] {
        let (status, body) = list_raw(&router, Some(max_items), None).await;
        assert_eq!(
            status, 400,
            "case MaxItems={max_items}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case MaxItems={max_items}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            element(&body, "Message")
                .map(|text| xml_text(&text))
                .as_deref(),
            Some(format!("Invalid MaxItems <{max_items}>.").as_str()),
            "case MaxItems={max_items}: the message must name the member and the value, \
             got {body}"
        );
    }
}

#[tokio::test]
async fn a_page_token_this_listing_did_not_issue_is_refused() {
    const STORED: usize = 12;
    let (router, expected) = emulator_holding(&set_names(STORED));

    // The cursor the listing itself hands out is accepted, and resumes the page
    // after the one it was cut from.
    let (first_page, issued) = list(&router, Some(5), None).await;
    let issued = issued.expect("a store larger than the page size yields a NextToken");
    let (second_page, _) = list(&router, Some(5), Some(&issued)).await;
    assert_eq!(
        second_page,
        expected[first_page.len()..first_page.len() + 5].to_vec(),
        "case the issued cursor: expected the page after the first, got {second_page:?}"
    );

    // Each row is a token shaped unlike anything this listing issues, or one it
    // issued for a different listing or a tampered position.
    let real = encode_page_token(LISTING_CONFIGURATION_SETS, &expected[3]);
    let tampered = {
        let mut bytes = ::hex::decode(&real).expect("an issued token is hex");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        ::hex::encode(bytes)
    };
    let rows: Vec<(&str, String)> = vec![
        (
            "a token the caller made up",
            "not-a-real-pagination-token".to_string(),
        ),
        ("an empty token", String::new()),
        ("a configuration set name sent bare", expected[0].clone()),
        ("hex that carries no key", "0011223344556677".to_string()),
        ("hex too short to carry a tag", "00112233".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("a tag that does not match the key", tampered),
        ("a token truncated by a byte", real[..real.len() - 2].to_string()),
        ("a token with a byte appended", format!("{real}ff")),
        (
            "a cursor of another listing",
            encode_page_token("ListTemplates", &expected[3]),
        ),
        // A rejected value is quoted back into the error envelope, so one
        // carrying XML metacharacters must travel escaped rather than breaking
        // the envelope it is rendered into.
        (
            "a token carrying XML metacharacters",
            "<Message>&\"'".to_string(),
        ),
    ];
    for (case, token) in &rows {
        let (status, body) = list_raw(&router, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {case} ({token:?}): expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case {case} ({token:?}): expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            element(&body, "Message")
                .map(|text| xml_text(&text))
                .as_deref(),
            Some(format!("Invalid PageToken <{token}>.").as_str()),
            "case {case} ({token:?}): the message must name the rejected token, got {body}"
        );
        assert!(
            !body.contains(token.as_str()) || !token.contains(['<', '>', '&']),
            "case {case} ({token:?}): an XML metacharacter must be escaped in the envelope, \
             got {body}"
        );
    }

    // A refused token changes nothing: the listing still reports every stored
    // set, and the cursor issued before the refusals still resumes.
    let seen = every_page(&router, None, MAX_PAGE_SIZE).await;
    assert_eq!(
        seen, expected,
        "case after {} refusals: the store must be untouched, got {seen:?}",
        rows.len()
    );
    let (resumed, _) = list(&router, Some(5), Some(&issued)).await;
    assert_eq!(
        resumed, second_page,
        "case after {} refusals: the issued cursor must still resume the same page, \
         got {resumed:?}",
        rows.len()
    );

    // A cursor IS accepted over a position the store no longer holds: the token
    // names where the next page starts, so the listing resumes at the first set
    // at or after it rather than refusing a caller whose page boundary was
    // deleted between calls.
    let over_a_gap = encode_page_token(LISTING_CONFIGURATION_SETS, &format!("{}-gone", expected[7]));
    let (after_gap, _) = list(&router, None, Some(&over_a_gap)).await;
    assert_eq!(
        after_gap,
        expected[8..].to_vec(),
        "case a cursor over a position the store does not hold: expected the sets after it, \
         got {after_gap:?}"
    );
}
