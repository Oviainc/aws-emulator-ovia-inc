// SPDX-License-Identifier: CC0-1.0

//! `ListReceiptRuleSets`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs rather than one recorded request:
//!
//! * the listing reports the receipt rule sets this account holds — each with the
//!   name and the creation instant the store holds for it — in one stable order,
//!   whatever order they were created in and however many there are;
//! * it serves them a page at a time, at the page size the output member
//!   publishes, and paging visits every set exactly once;
//! * a `NextToken` this listing did not issue is refused with HTTP 400
//!   `InvalidParameterValue`, whatever shape the token has, and the store is left
//!   alone.

use crate::harness::{call, element, elements, emulator, emulator_over, field, xml_text, Field};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_ses_simpleemailservice::state::{
    encode_page_token, service_state, ReceiptRuleSet, LISTING_RECEIPT_RULE_SETS,
    MAX_RECEIPT_RULE_SETS, MAX_RECEIPT_RULE_SET_NAME,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// The largest page this listing serves: `ListReceiptRuleSetsResponse$NextToken`
/// is "A token indicating that there are additional receipt rule sets available to
/// be listed. Pass this token to successive calls of ListReceiptRuleSets to
/// retrieve up to 100 receipt rule sets at a time."
const PAGE_SIZE: usize = 100;

/// The epoch second the seeded creation instants start at, so each stored
/// timestamp is a distinct whole second and the value the listing reports can be
/// compared with the one the store holds.
const FIRST_CREATED_AT_SECS: u64 = 1_767_225_600;

/// A router over an engine this suite keeps, holding one receipt rule set per name
/// in `names` — created a second apart, in the order given, which is NOT the
/// lexicographic order the listing reports.
///
/// The sets are written through the state API rather than
/// `CreateReceiptRuleSet` for two reasons: the creating operations apply the
/// account's own ceiling of [`MAX_RECEIPT_RULE_SETS`] sets, which is smaller than
/// the page this listing serves, and a stored creation instant can then be a known
/// value instead of "now".
///
/// Returns the router and, per set name, the epoch second it was created at, in
/// the order the listing must report them.
fn emulator_holding(names: &[String]) -> (MultiServiceRouter, Vec<(String, i64)>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let store = service_state(&*engine).expect("this service has a store on the shared engine");
    let mut created = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let secs = FIRST_CREATED_AT_SECS + index as u64;
        store.lock().receipt_rule_sets.insert(
            name.clone(),
            ReceiptRuleSet {
                name: name.clone(),
                created_at: UNIX_EPOCH + Duration::from_secs(secs),
                rules: Vec::new(),
            },
        );
        created.push((name.clone(), secs as i64));
    }
    created.sort();
    (emulator_over(engine), created)
}

/// `rule-set-<index>` for each of `count` sets, in an order whose lexicographic
/// sort differs from the insertion order.
fn rule_set_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("rule-set-{:04}", count - index))
        .collect()
}

/// One `ListReceiptRuleSets` call: `(status, body)`.
async fn list_rule_sets_raw(
    router: &MultiServiceRouter,
    next_token: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = Vec::new();
    if let Some(next_token) = next_token {
        params.push(field("NextToken", next_token));
    }
    call(router, "ListReceiptRuleSets", &params).await
}

/// `ListReceiptRuleSets`, as `(the (name, created-at second) pairs on this page,
/// the next token)`. Asserts the call succeeded and that every reported entry
/// carries both members.
async fn list_rule_sets(
    router: &MultiServiceRouter,
    next_token: Option<&str>,
) -> (Vec<(String, i64)>, Option<String>) {
    let (status, body) = list_rule_sets_raw(router, next_token).await;
    assert_eq!(
        status, 200,
        "case ListReceiptRuleSets(token={next_token:?}): expected 200, got {status} with {body}"
    );
    let page = element(&body, "RuleSets")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    let entries = page
        .iter()
        .map(|member| {
            let name = element(member, "Name").unwrap_or_else(|| {
                panic!(
                    "case ListReceiptRuleSets(token={next_token:?}): an entry carries Name, \
                     got {member}"
                )
            });
            let rendered = element(member, "CreatedTimestamp").unwrap_or_else(|| {
                panic!(
                    "case ListReceiptRuleSets(token={next_token:?}): an entry carries \
                     CreatedTimestamp, got {member}"
                )
            });
            let parsed = DateTime::from_str(&rendered, Format::DateTime).unwrap_or_else(|error| {
                panic!(
                    "case {name:?}: CreatedTimestamp {rendered:?} must be a date-time, \
                     got {error}"
                )
            });
            (name, parsed.secs())
        })
        .collect();
    (entries, element(&body, "NextToken"))
}

/// Every page of the listing, concatenated, following each `NextToken` until none
/// comes back. Also asserts no page is larger than the page size the operation
/// publishes, and that a page which is not the last is full.
async fn every_page(router: &MultiServiceRouter) -> Vec<(String, i64)> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count this suite can ask for, so a listing that
    // never terminates fails here instead of looping forever.
    for round in 0..1_000 {
        let (page, next) = list_rule_sets(router, token.as_deref()).await;
        assert!(
            page.len() <= PAGE_SIZE,
            "page {round}: expected at most {PAGE_SIZE} entries, got {}",
            page.len()
        );
        assert!(
            next.is_none() || page.len() == PAGE_SIZE,
            "page {round}: a page that is not the last must be full, got {} of {PAGE_SIZE}",
            page.len()
        );
        seen.extend(page);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("the listing never ran out of pages");
}

#[tokio::test]
async fn an_account_with_no_rule_sets_lists_nothing() {
    let router = emulator();
    let (page, token) = list_rule_sets(&router, None).await;
    assert!(
        page.is_empty(),
        "case an empty account: expected no rule sets, got {page:?}"
    );
    assert_eq!(
        token, None,
        "case an empty account: expected no NextToken, got {token:?}"
    );
}

/// What `CreateReceiptRuleSet` records is what the listing reports: the rows span
/// the set counts an account can hold, and within each row the names span the
/// documented name form, created in an order that is not the reported one.
#[tokio::test]
async fn every_created_rule_set_is_reported_in_name_order() {
    // The documented name form (`CreateReceiptRuleSetRequest$RuleSetName`): ASCII
    // letters, numbers, underscores or dashes, starting and ending with a letter
    // or number, 64 characters or fewer. Ordered here so the insertion order is
    // neither the lexicographic one nor its reverse.
    let names: Vec<String> = vec![
        "Prod-Rules_2024".to_string(),
        "9".to_string(),
        "x".repeat(MAX_RECEIPT_RULE_SET_NAME),
        "a".to_string(),
        "rule_set".to_string(),
        "rule-set".to_string(),
    ];
    // Every prefix of that list, so the rule is asserted from one set up to the
    // whole span of forms, and once more at the account's own ceiling.
    for count in 1..=names.len() {
        let router = emulator();
        let mut expected: Vec<String> = Vec::new();
        for name in &names[..count] {
            let (status, body) =
                call(&router, "CreateReceiptRuleSet", &[field("RuleSetName", name)]).await;
            assert_eq!(
                status, 200,
                "case {count} sets, {name:?}: expected 200 from CreateReceiptRuleSet, \
                 got {status} with {body}"
            );
            expected.push(name.clone());
        }
        expected.sort();

        let (page, token) = list_rule_sets(&router, None).await;
        let reported: Vec<String> = page.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(
            reported, expected,
            "case {count} sets: expected every created set once, in name order, got {reported:?}"
        );
        assert_eq!(
            token, None,
            "case {count} sets: {count} sets fit one page of {PAGE_SIZE}, so no NextToken, \
             got {token:?}"
        );

        // Round trip: the instant the listing reports for a set is the instant
        // `DescribeReceiptRuleSet` reports for the same set.
        for (name, listed_secs) in &page {
            let (status, body) = call(
                &router,
                "DescribeReceiptRuleSet",
                &[field("RuleSetName", name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {count} sets, {name:?}: expected 200 from DescribeReceiptRuleSet, \
                 got {status} with {body}"
            );
            let metadata = element(&body, "Metadata").unwrap_or_default();
            let described = element(&metadata, "CreatedTimestamp").unwrap_or_default();
            let described = DateTime::from_str(&described, Format::DateTime)
                .unwrap_or_else(|error| panic!("case {name:?}: {described:?}: {error}"));
            assert_eq!(
                *listed_secs,
                described.secs(),
                "case {count} sets, {name:?}: the listed creation instant must be the stored \
                 one, got {listed_secs} against {}",
                described.secs()
            );
        }
    }

    // And the account's whole capacity is reported on one page.
    let router = emulator();
    let mut at_the_ceiling: Vec<String> = Vec::new();
    for index in 0..MAX_RECEIPT_RULE_SETS {
        let name = format!("ceiling-rs-{index:02}");
        let (status, body) =
            call(&router, "CreateReceiptRuleSet", &[field("RuleSetName", &name)]).await;
        assert_eq!(
            status, 200,
            "case set {index} of {MAX_RECEIPT_RULE_SETS}: expected 200, got {status} with {body}"
        );
        at_the_ceiling.push(name);
    }
    at_the_ceiling.sort();
    let (page, token) = list_rule_sets(&router, None).await;
    let reported: Vec<String> = page.into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        reported, at_the_ceiling,
        "case the account at its ceiling: expected all {MAX_RECEIPT_RULE_SETS} sets, \
         got {reported:?}"
    );
    assert_eq!(
        token, None,
        "case the account at its ceiling: {MAX_RECEIPT_RULE_SETS} sets fit one page, \
         got {token:?}"
    );
}

/// Paging visits every stored set exactly once, in name order, at every store size
/// around the page boundary the output member publishes.
#[tokio::test]
async fn paging_visits_every_stored_rule_set_exactly_once() {
    // The rows differ in how the store size sits against the page size: well
    // inside it, one short of it, exactly it, one past it, and more than two
    // pages' worth.
    let sizes = [
        1,
        PAGE_SIZE - 1,
        PAGE_SIZE,
        PAGE_SIZE + 1,
        2 * PAGE_SIZE,
        2 * PAGE_SIZE + 7,
    ];
    for stored in sizes {
        let (router, expected) = emulator_holding(&rule_set_names(stored));
        let seen = every_page(&router).await;
        assert_eq!(
            seen, expected,
            "case {stored} stored sets: expected every set once, in name order, with its \
             stored creation instant, got {seen:?}"
        );

        let (first_page, token) = list_rule_sets(&router, None).await;
        assert_eq!(
            first_page.len(),
            stored.min(PAGE_SIZE),
            "case {stored} stored sets: expected a first page of {}, got {}",
            stored.min(PAGE_SIZE),
            first_page.len()
        );
        assert_eq!(
            token.is_some(),
            stored > PAGE_SIZE,
            "case {stored} stored sets: a NextToken must come back exactly when the store \
             does not fit one page of {PAGE_SIZE}, got {token:?}"
        );
    }
}

/// A `NextToken` this listing did not issue is refused, and the one it issues
/// resumes the page after the one it was cut from.
#[tokio::test]
async fn a_page_token_this_listing_did_not_issue_is_refused() {
    const STORED: usize = PAGE_SIZE + 12;
    let (router, expected) = emulator_holding(&rule_set_names(STORED));

    // The cursor the listing itself hands out is accepted, and resumes the page
    // after the one it was cut from.
    let (first_page, issued) = list_rule_sets(&router, None).await;
    let issued = issued.expect("a store larger than the page size yields a NextToken");
    let (second_page, _) = list_rule_sets(&router, Some(&issued)).await;
    assert_eq!(
        second_page,
        expected[first_page.len()..].to_vec(),
        "case the issued cursor: expected the sets after the first page, got {second_page:?}"
    );

    // Each row is a token shaped unlike anything this listing issues, or one it
    // issued for a different listing or a tampered position.
    let real = encode_page_token(LISTING_RECEIPT_RULE_SETS, &expected[3].0);
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
        ("a rule set name sent bare", expected[0].0.clone()),
        ("hex that carries no key", "0011223344556677".to_string()),
        ("hex too short to carry a tag", "00112233".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("a tag that does not match the key", tampered),
        ("a token truncated by a byte", real[..real.len() - 2].to_string()),
        ("a token with a byte appended", format!("{real}ff")),
        (
            "a cursor of another listing",
            encode_page_token("ListTemplates", &expected[3].0),
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
        let (status, body) = list_rule_sets_raw(&router, Some(token)).await;
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
    let seen = every_page(&router).await;
    assert_eq!(
        seen, expected,
        "case after {} refusals: the store must be untouched, got {seen:?}",
        rows.len()
    );
    let (resumed, _) = list_rule_sets(&router, Some(&issued)).await;
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
    let resume_at = &expected[7].0;
    let over_a_gap = encode_page_token(LISTING_RECEIPT_RULE_SETS, &format!("{resume_at}-gone"));
    let (after_gap, _) = list_rule_sets(&router, Some(&over_a_gap)).await;
    assert_eq!(
        after_gap,
        expected[8..8 + PAGE_SIZE].to_vec(),
        "case a cursor over a position the store does not hold: expected the sets after it, \
         got {after_gap:?}"
    );
}
