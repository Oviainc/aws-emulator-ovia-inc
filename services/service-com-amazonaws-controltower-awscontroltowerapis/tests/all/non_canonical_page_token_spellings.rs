// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` resumes a listing only AS THE STRING a page of this service handed out.
//!
//! The rule: every listing of this service reports a page position as a `usize`'s canonical
//! decimal — no sign, no leading zeros — so a token is read as a position only in that one
//! spelling, and every other spelling of the same number is a token no call of this service
//! issued and is refused with the modelled `ValidationException` (400), in the one wording
//! every listing refuses an unhandled token with.
//!
//! The spellings a decimal parse admits BESIDE the canonical one are an optional leading
//! `+`, any number of leading zeros, and the combination of the two. Read as positions they
//! are silent data loss on a READ: the page they resume omits every record before it and
//! nothing in the answer says records were skipped, where the listing's own refusal path
//! exists to tell the caller the token resumes nothing.
//!
//! Both arms of the rule are asserted over a TABLE whose rows differ in what the rule
//! generalises over: which of the six listings is paged (and therefore whether its refusal
//! is scoped to the request's parameters), which position in the listing is spelled — every
//! index the listing admits, from its start to one past its last record — and which
//! non-canonical spelling of that position is sent. The second test drives the shared helper
//! directly, over magnitudes an end-to-end walk cannot reach cheaply and over the
//! whitespace-padded forms the model's own `nextToken` patterns refuse before a handler.
//!
//! The listings, the HTTP harness and the seeding are the sibling suites': `list_pagination`
//! holds the table of listings that share this rule and the fixture they page over, and
//! `landing_zone_lifecycle` the emulator every suite drives.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state;

use crate::landing_zone_lifecycle::{call, emulator};
use crate::list_pagination::{
    identities, list, listings, page_body, reported_token, seed, unhandled_token_refusal, Listing,
};

/// Every spelling of `position` a decimal parse admits BESIDE the canonical one, each with
/// the case it stands for. The canonical spelling itself is filtered out, so a row is never
/// a token the service could have written.
fn non_canonical_spellings(position: usize) -> Vec<(String, String)> {
    let canonical = position.to_string();
    let mut spellings = vec![
        ("a leading plus".to_string(), format!("+{canonical}")),
        ("one leading zero".to_string(), format!("0{canonical}")),
        ("two leading zeros".to_string(), format!("00{canonical}")),
        (
            "a leading plus before a leading zero".to_string(),
            format!("+0{canonical}"),
        ),
        (
            "zero-padded to eight digits".to_string(),
            format!("{position:08}"),
        ),
    ];
    spellings.retain(|(_, spelling)| *spelling != canonical);
    spellings
}

/// The whitespace-padded spellings of `position`, which a decimal parse does not admit at
/// all — the neighbouring form of the same mistake, kept in its own table because the
/// model's `nextToken` patterns refuse some of them at the front door rather than in the
/// shared rule.
fn padded_spellings(position: usize) -> Vec<(String, String)> {
    vec![
        ("a leading space".to_string(), format!(" {position}")),
        ("a trailing space".to_string(), format!("{position} ")),
        ("a leading tab".to_string(), format!("\t{position}")),
    ]
}

/// Assert `listing` refuses `token` as a token no page of it handed out.
async fn assert_refused(router: &MultiServiceRouter, listing: Listing, case: &str, token: &str) {
    let refused = call(router, listing.path, page_body(None, Some(token))).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?} on {}: expected 400 ValidationException for {token:?}, got {:?} body {:?}",
        listing.path,
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    let expected = unhandled_token_refusal(listing, token);
    assert_eq!(
        refused.string("message"),
        expected,
        "case {case:?} on {}: expected the shared refusal wording {expected:?}, got {:?}",
        listing.path,
        refused.string("message")
    );
    assert!(
        refused.body.get(listing.records).is_none(),
        "case {case:?} on {}: a refusal must report no records, got {:?}",
        listing.path,
        refused.body
    );
}

/// Assert the rule over one listing: every index it admits resumes in the canonical
/// spelling and in no other spelling of the same number.
async fn only_the_canonical_spelling_resumes(router: &MultiServiceRouter, listing: Listing) {
    let population = identities(listing, &list(router, listing, serde_json::json!({})).await);

    // Every index the listing admits is a row: its start, each record boundary inside it,
    // and the index just past its last record.
    for index in 0..=population.len() {
        let canonical = index.to_string();
        let case = format!("the position {canonical} spelled as this service writes it");
        let resumed = list(router, listing, page_body(None, Some(&canonical))).await;
        assert_eq!(
            identities(listing, &resumed),
            population[index..].to_vec(),
            "case {case:?} on {}: expected the listing from index {index}, got {:?}",
            listing.path,
            resumed.body
        );
        assert_eq!(
            reported_token(&resumed),
            None,
            "case {case:?} on {}: a page carrying every remaining record must report no \
             nextToken, got {:?}",
            listing.path,
            resumed.body
        );

        for (spelling_case, spelling) in non_canonical_spellings(index) {
            let case = format!("the position {canonical} spelled with {spelling_case}");
            assert_refused(router, listing, &case, &spelling).await;
        }
    }
}

/// RULE: a page position is a token only in the one spelling this service writes it in, so
/// every other spelling of that number is refused as a token no call returned, rather than
/// silently read as a page position that skips the records before it.
#[tokio::test]
async fn only_the_spelling_a_page_hands_out_resumes_a_listing() {
    let router = emulator();
    seed(&router).await;

    for listing in listings() {
        only_the_canonical_spelling_resumes(&router, listing).await;
    }
}

/// RULE (the same rule where no record is stored): an EMPTY listing admits only its own
/// start, and refuses every non-canonical spelling of it in the same wording a populated
/// one does.
#[tokio::test]
async fn an_empty_listing_refuses_a_non_canonical_spelling_of_its_start() {
    let router = emulator();

    for listing in listings() {
        only_the_canonical_spelling_resumes(&router, listing).await;
    }
}

/// RULE at the shared helper every listing reaches it through, over positions an end-to-end
/// walk cannot reach cheaply and over the whitespace-padded forms a modelled `nextToken`
/// pattern refuses before a handler sees them.
#[test]
fn the_shared_rule_admits_the_position_it_writes_and_no_other_spelling_of_it() {
    // A listing long enough for the magnitudes below to be indices into it; the entries
    // themselves are not what the rule generalises over.
    let listing: Vec<String> = (0..1000).map(|index| format!("entry-{index}")).collect();

    for row in listings() {
        let scope = if row.token_scoped_to_parameters {
            state::TokenScope::SameParameters
        } else {
            state::TokenScope::WholeListing
        };
        for position in [0_usize, 1, 2, 9, 10, 99, 100, 1000] {
            let canonical = position.to_string();
            let case = format!("{} resuming at {canonical}", row.operation);
            let paged = state::page(
                row.operation,
                scope,
                Some(&canonical),
                None,
                listing.as_slice(),
            );
            assert_eq!(
                paged.as_ref().map(|(page, token)| (page.len(), token.clone())),
                Ok((listing.len() - position, None)),
                "case {case:?}: the spelling this service writes must resume the listing, got \
                 {paged:?}"
            );

            for (spelling_case, spelling) in non_canonical_spellings(position)
                .into_iter()
                .chain(padded_spellings(position))
            {
                let case = format!("{case} spelled with {spelling_case}");
                let refused = state::page(
                    row.operation,
                    scope,
                    Some(&spelling),
                    None,
                    listing.as_slice(),
                );
                let expected = unhandled_token_refusal(row, &spelling);
                assert_eq!(
                    refused.as_ref().err().map(String::as_str),
                    Some(expected.as_str()),
                    "case {case:?}: a spelling this service never writes must be refused as a \
                     token no page returned, got {:?}",
                    refused.as_ref().map(|(page, token)| (page.len(), token.clone()))
                );
            }
        }
    }
}
