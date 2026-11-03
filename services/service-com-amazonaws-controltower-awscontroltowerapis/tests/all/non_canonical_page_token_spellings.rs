// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` resumes a listing only AS THE STRING a page of this service handed out.
//!
//! The rule: a token names a position in the listing it was minted from and carries that
//! listing's identity beside it, so it resumes only when it is byte for byte the string a
//! page wrote. Every other spelling of the same position — the position re-spelled (a
//! leading `+`, leading zeros, whitespace padding), the position alone without the listing
//! it indexes, the listing alone without a position, the two parts swapped, a character
//! flipped, the string truncated or extended — is a token no call of this service issued,
//! and is refused with the modelled `ValidationException` (400) in the one wording every
//! listing refuses an unhandled token with.
//!
//! Read as positions, the re-spellings would be silent data loss on a READ: the page they
//! resume omits every record before it and nothing in the answer says records were skipped,
//! where the listing's own refusal path exists to tell the caller the token resumes nothing.
//!
//! Both arms of the rule are asserted over a TABLE whose rows differ in what the rule
//! generalises over: which of the six listings is paged (and therefore whether its refusal
//! is scoped to the request's parameters), which position in the listing is spelled, and
//! which spelling of that token is sent. The second test drives the shared helper directly,
//! over positions and magnitudes an end-to-end walk cannot reach cheaply and over the
//! whitespace-padded forms the model's own `nextToken` patterns refuse before a handler.
//!
//! Which listing a token resumes at all — the parameters a narrowed listing binds its token
//! to — is the sibling suite `page_token_listing_scope`.
//!
//! The listings, the HTTP harness and the seeding are the sibling suites': `list_pagination`
//! holds the table of listings that share this rule and the fixture they page over, and
//! `landing_zone_lifecycle` the emulator every suite drives.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state;

use crate::landing_zone_lifecycle::{call, emulator};
use crate::list_pagination::{
    emulator_for_empty_listing, identities, list, listings, page_body, reported_token, seed,
    unhandled_token_refusal, Listing,
};

/// The two parts of a token a page handed out: the listing it indexes and the position in
/// it, as the minting site wrote them. `None` when the token carries no position to
/// re-spell, so the structural rows below are simply not driven.
fn token_parts(token: &str) -> Option<(&str, usize)> {
    let (listing, position) = token.rsplit_once('.')?;
    Some((listing, position.parse().ok()?))
}

/// Every spelling of `token` BESIDE the one a page wrote, each with the case it stands for:
/// the position re-spelled in the forms a decimal parse would otherwise admit, the parts
/// taken apart, and the string itself damaged. A row that comes out equal to `token` is
/// dropped, so a row is never a token the service could have written.
fn other_spellings(token: &str) -> Vec<(String, String)> {
    let mut spellings: Vec<(String, String)> = vec![
        (
            "the token with a letter appended".to_string(),
            format!("{token}x"),
        ),
        (
            "the token with its first character cut off".to_string(),
            token.chars().skip(1).collect(),
        ),
        (
            "the token with its characters reversed".to_string(),
            token.chars().rev().collect(),
        ),
        (
            "the token with its letters in the other case".to_string(),
            token
                .chars()
                .map(|character| {
                    if character.is_ascii_uppercase() {
                        character.to_ascii_lowercase()
                    } else {
                        character.to_ascii_uppercase()
                    }
                })
                .collect(),
        ),
    ];

    if let Some((listing, position)) = token_parts(token) {
        spellings.extend([
            (
                "its position with a leading plus".to_string(),
                format!("{listing}.+{position}"),
            ),
            (
                "its position with one leading zero".to_string(),
                format!("{listing}.0{position}"),
            ),
            (
                "its position with two leading zeros".to_string(),
                format!("{listing}.00{position}"),
            ),
            (
                "its position zero-padded to eight digits".to_string(),
                format!("{listing}.{position:08}"),
            ),
            (
                "its position without the listing it indexes".to_string(),
                position.to_string(),
            ),
            (
                "the listing it indexes without a position".to_string(),
                listing.to_string(),
            ),
            (
                "its two parts swapped".to_string(),
                format!("{position}.{listing}"),
            ),
            (
                "another listing's identity with its position".to_string(),
                format!("{}.{position}", flip_first_hex_digit(listing)),
            ),
        ]);
    }

    spellings.retain(|(_, spelling)| spelling != token && !spelling.is_empty());
    spellings
}

/// `identity` with its first hexadecimal digit moved on by one, which is the identity of a
/// listing this call is not paging.
fn flip_first_hex_digit(identity: &str) -> String {
    identity
        .chars()
        .enumerate()
        .map(|(position, character)| match (position, character.to_digit(16)) {
            (0, Some(digit)) => std::char::from_digit((digit + 1) % 16, 16).unwrap_or(character),
            _ => character,
        })
        .collect()
}

/// The whitespace-padded spellings of `token`, the neighbouring form of the same mistake,
/// kept in its own table because the model's `nextToken` patterns refuse some of them at the
/// front door rather than in the shared rule.
fn padded_spellings(token: &str) -> Vec<(String, String)> {
    vec![
        ("a leading space".to_string(), format!(" {token}")),
        ("a trailing space".to_string(), format!("{token} ")),
        ("a leading tab".to_string(), format!("\t{token}")),
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

/// Assert the rule over one listing: the token its first page hands out resumes the walk,
/// and no other spelling of that token does.
async fn only_the_minted_spelling_resumes(router: &MultiServiceRouter, listing: Listing) {
    let population = identities(listing, &list(router, listing, serde_json::json!({})).await);
    let page_size = listing.max_results_range.0;
    if population.len() as i64 <= page_size {
        // This listing holds no more records than its smallest page, so no call of it hands
        // out a token to re-spell; the helper test below drives its scope directly.
        return;
    }

    let first = list(router, listing, page_body(Some(page_size), None)).await;
    let Some(token) = reported_token(&first) else {
        panic!(
            "{}: a page of {page_size} of {} records must report a nextToken, got {:?}",
            listing.path,
            population.len(),
            first.body
        );
    };

    let resumed = list(router, listing, page_body(None, Some(&token))).await;
    assert_eq!(
        identities(listing, &resumed),
        population[page_size as usize..].to_vec(),
        "case {:?} on {}: expected the listing from record {page_size}, got {:?}",
        "the token the page handed out",
        listing.path,
        resumed.body
    );

    for (case, spelling) in other_spellings(&token) {
        assert_refused(router, listing, &case, &spelling).await;
    }
}

/// RULE: a page position is a token only in the one spelling this service writes it in, so
/// every other spelling of it is refused as a token no call returned, rather than silently
/// read as a page position that skips the records before it.
#[tokio::test]
async fn only_the_spelling_a_page_hands_out_resumes_a_listing() {
    let router = emulator();
    seed(&router).await;

    for listing in listings() {
        only_the_minted_spelling_resumes(&router, listing).await;
    }
}

/// RULE (the same rule where no record is stored): an EMPTY listing hands out no token at
/// all, so every token-shaped string, and every spelling of one, is refused in the same
/// wording a populated listing refuses it with.
#[tokio::test]
async fn an_empty_listing_hands_out_no_token_and_refuses_every_token() {
    // A token-shaped string no call of this service wrote: an unseeded listing hands out no
    // token, so this and every spelling of it resume nothing.
    let unissued = "0123456789abcdef.0";

    for listing in listings() {
        // One emulator per listing, so the account's setup — the precondition the
        // enablement listings are served under — leaves every listing here empty.
        let router = emulator_for_empty_listing(listing).await;
        let answer = list(&router, listing, serde_json::json!({})).await;
        assert_eq!(
            reported_token(&answer),
            None,
            "{}: a call that returned every record must hand out no nextToken, got {:?}",
            listing.path,
            answer.body
        );

        for (case, spelling) in std::iter::once((
            "a token-shaped string no page wrote".to_string(),
            unissued.to_string(),
        ))
        .chain(other_spellings(unissued))
        {
            assert_refused(&router, listing, &case, &spelling).await;
        }
    }
}

/// RULE at the shared helper every listing reaches it through, over positions and
/// magnitudes an end-to-end walk cannot reach cheaply and over the whitespace-padded forms
/// a modelled `nextToken` pattern refuses before a handler sees them.
#[test]
fn the_shared_rule_admits_the_token_it_writes_and_no_other_spelling_of_it() {
    // A listing long enough for the magnitudes below to be positions in it; the entries
    // themselves are not what the rule generalises over.
    let listing: Vec<String> = (0..1000).map(|index| format!("entry-{index}")).collect();
    // The narrowing a scoped row's token is bound to: one single-valued and one list-valued
    // member, as a handler states them.
    let statuses: state::Filter<'_> = Some(BTreeSet::from(["SUCCEEDED"]));
    let key = state::ListingKey::new()
        .value("targetIdentifier", Some("arn:aws:organizations::0:ou/o-x/ou-1"))
        .filter("filter.statuses", &statuses);

    for row in listings() {
        let scope = if row.token_scoped_to_parameters {
            state::TokenScope::SameParameters(&key)
        } else {
            state::TokenScope::WholeListing
        };
        for position in [1_usize, 2, 9, 10, 99, 100, 999] {
            // The token a page ending at `position` hands out, taken from the helper rather
            // than written here: the rule is that THIS string resumes and no variant of it
            // does.
            let minted = state::page(
                row.operation,
                scope,
                None,
                Some(position as i32),
                listing.as_slice(),
            );
            let case = format!("{} resuming after {position} records", row.operation);
            let token = match &minted {
                Ok((page, Some(token))) if page.len() == position => token.clone(),
                other => panic!(
                    "case {case:?}: a page of {position} of {} entries must report a token, \
                     got {other:?}",
                    listing.len()
                ),
            };

            let paged = state::page(row.operation, scope, Some(&token), None, listing.as_slice());
            assert_eq!(
                paged
                    .as_ref()
                    .map(|(page, token)| (page.len(), token.clone())),
                Ok((listing.len() - position, None)),
                "case {case:?}: the token this service writes must resume the listing, got \
                 {paged:?}"
            );

            for (spelling_case, spelling) in other_spellings(&token)
                .into_iter()
                .chain(padded_spellings(&token))
            {
                let case = format!("{case}, sent as {spelling_case}");
                let refused =
                    state::page(row.operation, scope, Some(&spelling), None, listing.as_slice());
                let expected = unhandled_token_refusal(row, &spelling);
                assert_eq!(
                    refused.as_ref().err().map(String::as_str),
                    Some(expected.as_str()),
                    "case {case:?}: a spelling this service never writes must be refused as a \
                     token no page returned, got {:?}",
                    refused
                        .as_ref()
                        .map(|(page, token)| (page.len(), token.clone()))
                );
            }
        }
    }
}
