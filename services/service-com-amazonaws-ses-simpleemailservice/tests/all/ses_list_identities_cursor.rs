// SPDX-License-Identifier: CC0-1.0

//! `ListIdentities`' pagination cursor, end to end over the wire.
//!
//! The listing is paginated (`inputToken` `NextToken`, `outputToken` `NextToken`,
//! `items` `Identities`, `pageSize` `MaxItems`), and a cursor is a value the SERVICE
//! issued: it is opaque to the caller, it resumes exactly where the page it was cut
//! from ended, and one this listing never issued is REFUSED rather than read as some
//! arbitrary position — a caller that hands back a stale or hand-built cursor is told
//! so instead of silently restarting the walk at page one.
//!
//! What is asserted is that rule over a SET of cursors: the one the listing issued,
//! and a table of values it did not — made up, empty, an identity name sent bare,
//! hex too short to carry a tag, a tampered tag, a truncated or extended token, and a
//! cursor this service issued for ANOTHER of its listings.

use crate::harness::{call, element, elements, field, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    encode_page_token, LISTING_CONFIGURATION_SETS, LISTING_IDENTITIES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The identities the account holds for these cases, in the order the store keys
/// them (lexicographic), so the expected pages are the expected pages.
fn stored_identities() -> Vec<String> {
    let mut names: Vec<String> = (0..12)
        .map(|index| format!("id{index:02}.example.com"))
        .collect();
    names.sort();
    names
}

/// A router whose account holds [`stored_identities`].
async fn emulator_holding() -> (MultiServiceRouter, Vec<String>) {
    let router = crate::harness::emulator();
    let expected = stored_identities();
    for domain in &expected {
        let (status, body) = call(&router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
        assert_eq!(
            status, 200,
            "VerifyDomainIdentity({domain:?}): expected 200, got {status} with {body}"
        );
    }
    (router, expected)
}

/// `ListIdentities`, as the raw `(status, body)`.
async fn list_raw(
    router: &MultiServiceRouter,
    max_items: Option<i64>,
    next_token: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = Vec::new();
    if let Some(max_items) = max_items {
        params.push(field("MaxItems", max_items.to_string()));
    }
    if let Some(next_token) = next_token {
        params.push(field("NextToken", next_token));
    }
    call(router, "ListIdentities", &params).await
}

/// `ListIdentities`, as `(the identities on this page, the next cursor)`.
async fn list(
    router: &MultiServiceRouter,
    max_items: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let (status, body) = list_raw(router, max_items, next_token).await;
    assert_eq!(
        status, 200,
        "ListIdentities(max={max_items:?}, token={next_token:?}): expected 200, got {status} \
         with {body}"
    );
    let identities = element(&body, "Identities")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    (identities, element(&body, "NextToken"))
}

/// Every page of the listing at `page_size`, concatenated.
async fn every_page(router: &MultiServiceRouter, page_size: i64) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list(router, Some(page_size), token.as_deref()).await;
        seen.extend(page);
        match next {
            None => break,
            Some(next) => token = Some(next),
        }
        assert!(
            seen.len() <= stored_identities().len(),
            "case paging at {page_size}: pagination did not terminate, saw {seen:?}"
        );
    }
    seen
}

#[tokio::test]
async fn the_issued_cursor_is_opaque_and_resumes_exactly_where_the_page_ended() {
    let (router, expected) = emulator_holding().await;

    // Every page size the walk can take: the cursor must resume exactly, and the
    // walk must visit every identity once in the listing's stable order.
    for page_size in [1_i64, 2, 5, 11, 12, 13] {
        let seen = every_page(&router, page_size).await;
        assert_eq!(
            seen, expected,
            "case MaxItems={page_size}: paging must visit every identity exactly once in order, \
             got {seen:?}"
        );
    }

    // The cursor is the SERVICE's value, not a position the caller can read off it:
    // it is not the identity the next page starts at.
    let (first_page, issued) = list(&router, Some(5), None).await;
    let issued = issued.expect("a store larger than the page size yields a NextToken");
    let resumes_at = &expected[first_page.len()];
    assert_ne!(
        issued.as_str(),
        resumes_at.as_str(),
        "case the issued cursor: expected an opaque token, got the identity itself {issued:?}"
    );
    let (second_page, _) = list(&router, Some(5), Some(&issued)).await;
    assert_eq!(
        second_page,
        expected[first_page.len()..first_page.len() + 5].to_vec(),
        "case the issued cursor: expected the page after the first, got {second_page:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let (router, expected) = emulator_holding().await;
    let (_, issued) = list(&router, Some(5), None).await;
    let issued = issued.expect("a store larger than the page size yields a NextToken");

    let real = encode_page_token(LISTING_IDENTITIES, &expected[3]);
    let tampered = {
        let mut bytes = ::hex::decode(&real).expect("an issued token is hex");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        ::hex::encode(bytes)
    };
    // Each row is a cursor shaped unlike anything this listing issues, or one issued
    // for a different listing or over a tampered position.
    let rows: Vec<(&str, String)> = vec![
        (
            "a token the caller made up",
            "c2ltMnJlYWwtZm9yZ2VkLXYxLXRva2Vu".to_string(),
        ),
        ("an empty token", String::new()),
        ("an identity name sent bare", expected[0].clone()),
        ("hex that carries no key", "0011223344556677".to_string()),
        ("hex too short to carry a tag", "00112233".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("a tag that does not match the key", tampered),
        ("a token truncated by a byte", real[..real.len() - 2].to_string()),
        ("a token with a byte appended", format!("{real}ff")),
        (
            "a cursor of another listing of this service",
            encode_page_token(LISTING_CONFIGURATION_SETS, &expected[3]),
        ),
        // A rejected value is quoted back into the error envelope, so one carrying
        // XML metacharacters must travel escaped rather than breaking the envelope.
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
            Some(format!("Invalid NextToken <{token}>.").as_str()),
            "case {case} ({token:?}): the message must name the rejected token, got {body}"
        );
        assert!(
            !body.contains(token.as_str()) || !token.contains(['<', '>', '&']),
            "case {case} ({token:?}): an XML metacharacter must be escaped in the envelope, \
             got {body}"
        );
    }

    // A refused cursor changes nothing: the account still holds every identity, and
    // the cursor issued before the refusals still resumes the same page.
    let (whole, token) = list(&router, None, None).await;
    assert_eq!(
        (whole, token),
        (expected.clone(), None),
        "case after {} refusals: the identity list must be untouched",
        rows.len()
    );
    let (resumed, _) = list(&router, Some(5), Some(&issued)).await;
    assert_eq!(
        resumed,
        expected[5..10].to_vec(),
        "case after {} refusals: the issued cursor must still resume the same page, got \
         {resumed:?}",
        rows.len()
    );

    // A cursor IS accepted over a position the account no longer holds: it names
    // where the next page starts, so the listing resumes at the first identity at or
    // after it rather than refusing a caller whose page boundary was deleted between
    // calls.
    let over_a_gap = encode_page_token(LISTING_IDENTITIES, &format!("{}a", expected[7]));
    let (after_gap, _) = list(&router, None, Some(&over_a_gap)).await;
    assert_eq!(
        after_gap,
        expected[8..].to_vec(),
        "case a cursor over a position the account does not hold: expected the identities after \
         it, got {after_gap:?}"
    );
}
