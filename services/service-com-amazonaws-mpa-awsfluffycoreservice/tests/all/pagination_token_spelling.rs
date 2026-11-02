// SPDX-License-Identifier: CC0-1.0

//! How a `NextToken` is SPELLED is part of its provenance, driven end-to-end through
//! every paginated listing of this service.
//!
//! The rule under test: a listing hands a resume key out by rendering the value it names,
//! so one value has exactly one spelling in a token of this service's — and a listing
//! resumes only from that spelling. Text that merely parses to a key the listing holds
//! (a key carrying a sign, zero padding or surrounding space) is a token no page ever
//! handed out, so it earns the same refusal as every other never-issued token: 400
//! `ValidationException` with the one sentence this service states for the class
//! (`state::FOREIGN_NEXT_TOKEN_MESSAGE`), never a page.
//!
//! The rows span the class the rule generalises over rather than one request: every
//! paginated operation of this service, the three of them that page over one resource's
//! records driven twice over DIFFERENT resources (the fixtures in
//! `pagination_token_provenance.rs`), each at every key its own pages hand out — a
//! creation-order position, a catalog version id, a catalog policy ARN — crossed with the
//! ways a key can be respelled without changing the value it parses as.
//!
//! Every request goes over the wire the real client uses. The listings that bind
//! `NextToken` to the query string get the '+' or space a row respells a key with sent as
//! its escape, so it reaches the reader as itself rather than as the query string's own
//! meaning for it; that the escape arrives decoded is what the walk over each listing's
//! OWN tokens proves, since those pages are answered only when the token arrives as the
//! one the listing minted.

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{text, Harness};
use crate::pagination_token_provenance::{assert_refused, fixtures, Kind, Listing, NARROWEST_PAGE};

/// Pages a walk of any fixture below may take before it has to have ended: more than the
/// largest fixture here holds records (the policy catalog, which this crate's harness
/// records entries into), since every page below reports one record. A walk that has not
/// reached a page without a token by then is a listing that keeps minting them, which the
/// walk asserts rather than loops on.
const MOST_PAGES: usize = 200;

/// `token` as this listing's wire binding carries it: `ListSessions` binds `NextToken` to
/// the JSON body, where every byte travels as written, and every other listing binds it to
/// the query string, where the two characters the rows below respell a key with cannot
/// travel literally — a bare '+' means a space there, and a bare space is no query
/// character at all — so each is sent as the escape for the byte it is. Nothing else is
/// escaped: a token's other characters (the ':' and '/' of a scope, an ARN's own
/// separators) are query characters already, which is how the listings' own tokens travel
/// in `pagination_token_provenance.rs`.
fn on_the_wire(listing: &Listing, token: &str) -> String {
    match listing.kind {
        Kind::Sessions => token.to_string(),
        _ => token.replace('+', "%2B").replace(' ', "%20"),
    }
}

/// One page of `listing`, at the narrowest width a request may ask for, resuming from
/// `token` exactly as a client holding that token would send it.
async fn page(harness: &Harness, listing: &Listing, token: Option<&str>) -> (u16, Value) {
    let wire = token.map(|token| on_the_wire(listing, token));
    listing.page(harness, NARROWEST_PAGE, wire.as_deref()).await
}

/// Respellings of the resume key `key` that parse to the same value (or to no key at all)
/// but that no listing of this service ever RENDERS, since a key is rendered by `Display`:
/// a position through `u64`'s, a catalog version id through `i32`'s, a catalog policy ARN
/// as itself.
///
/// The sign and padding rows are the ones Rust's integer `FromStr` reads as the bare value
/// — so they are the spellings a numeric key could be smuggled in as — and the space rows
/// are what a hand-edited or re-encoded cursor picks up. A row is derived from the key the
/// listing itself just handed out, so each one names a key the listing DOES hold and would
/// answer with a page if the spelling were not part of the token.
fn never_rendered_spellings(key: &str) -> Vec<(String, String)> {
    let mut rows = vec![
        ("a leading zero".to_string(), format!("0{key}")),
        ("two leading zeros".to_string(), format!("00{key}")),
        ("a leading plus".to_string(), format!("+{key}")),
        ("a leading space".to_string(), format!(" {key}")),
        ("a trailing space".to_string(), format!("{key} ")),
    ];
    if key.parse::<i64>().is_ok() {
        rows.push((
            "zero padding to twenty digits".to_string(),
            format!("{key:0>20}"),
        ));
        rows.push((
            "a leading plus over zero padding".to_string(),
            format!("+0{key}"),
        ));
    }
    rows
}

/// Every paginated listing of this service refuses a `NextToken` whose key is a respelling
/// of a key it really handed out, and answers the spelling it minted.
#[tokio::test]
async fn no_listing_resumes_from_a_spelling_of_a_key_it_never_rendered() {
    let (harness, listings) = fixtures().await;

    for listing in &listings {
        let prefix = state::listing_token(listing.token_listing(), "");
        let mut token: Option<String> = None;
        let mut keys_offered = 0usize;
        let mut walked_out = false;

        for page_number in 1..=MOST_PAGES {
            let case = format!("{} page {page_number}", listing.label());
            // The listing's own token is answered, so every refusal below is about the
            // SPELLING of the key and the walk reaches every key the listing mints.
            let (status, answered) = page(&harness, listing, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the listing must answer the token it handed out, got {status} \
                 with {answered:?}"
            );
            let issued = text(&answered, "NextToken");
            if issued.is_empty() {
                walked_out = true;
                break;
            }
            let key = issued
                .strip_prefix(&prefix)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {issued:?} is no token of {prefix:?}'s listing")
                })
                .to_string();
            keys_offered += 1;

            // The control that an escape in a query-bound token reaches the reader
            // decoded — which is what makes the '+' and space rows below a test of the
            // SPELLING rather than of the escape: the listing's own token, with the
            // separator of its scope sent as the escape for that byte, is answered
            // exactly as the token itself is.
            if listing.kind != Kind::Sessions {
                let escaped = issued.replace(':', "%3A");
                let (escaped_status, escaped_page) = listing
                    .page(&harness, NARROWEST_PAGE, Some(&escaped))
                    .await;
                let (raw_status, raw_page) =
                    listing.page(&harness, NARROWEST_PAGE, Some(&issued)).await;
                assert_eq!(
                    (escaped_status, listing.reported(&escaped_page)),
                    (raw_status, listing.reported(&raw_page)),
                    "case {case:?}: a percent escape in a query-bound token must reach the \
                     reader as the byte it names: {escaped:?} answered {escaped_page:?}"
                );
                assert_eq!(
                    escaped_status, 200,
                    "case {case:?}: the listing's own token must be answered however it is \
                     escaped, got {escaped_status} with {escaped_page:?}"
                );
            }

            for (row, spelling) in never_rendered_spellings(&key) {
                let forged = state::listing_token(listing.token_listing(), &spelling);
                assert_ne!(
                    forged, issued,
                    "case {case:?}: {row} must respell the key {key:?}, not reproduce it"
                );
                let case = format!(
                    "{} resuming from {row} of the key it handed out ({forged:?})",
                    listing.label()
                );
                let (status, body) = page(&harness, listing, Some(&forged)).await;
                assert_refused(&case, status, &body);
            }

            token = Some(issued);
        }

        assert!(
            walked_out,
            "case {:?}: the walk must reach a page that hands out no token within \
             {MOST_PAGES} pages, so a listing that keeps minting them is caught rather \
             than truncated",
            listing.label()
        );
        assert!(
            keys_offered >= 2,
            "case {:?}: the fixture must hand out tokens for at least two keys for this table \
             to span them, got {keys_offered}",
            listing.label()
        );
    }
}

/// The refusal is the token reader's, so the width the request asks for is no part of it:
/// a respelled key is refused with the same sentence whatever `MaxResults` asks for, so no
/// client reaches a page by widening the one it asked for.
#[tokio::test]
async fn a_respelled_key_is_refused_at_every_width() {
    let (harness, listings) = fixtures().await;

    // `MaxResults` is modelled `range(min: 1, max: 20)`: both published bounds, and a
    // width between them.
    for listing in &listings {
        let mut rows_driven = 0usize;
        for width in [1, 7, 20] {
            let (status, first) = listing.page(&harness, width, None).await;
            assert_eq!(
                status, 200,
                "case {:?} at width {width}: the first page returned {status} with {first:?}",
                listing.label()
            );
            let issued = text(&first, "NextToken");
            if issued.is_empty() {
                // A width that reports every record hands out no token, so there is no
                // key of this listing's to respell at this width.
                continue;
            }
            let key = issued
                .strip_prefix(&state::listing_token(listing.token_listing(), ""))
                .expect("a token of this listing's")
                .to_string();

            for (row, spelling) in never_rendered_spellings(&key) {
                let forged = state::listing_token(listing.token_listing(), &spelling);
                let case = format!("{} at width {width} refusing {row}", listing.label());
                // Sent exactly as a client holding the respelled cursor would.
                let wire = on_the_wire(listing, &forged);
                let (status, body) = listing.page(&harness, width, Some(&wire)).await;
                assert_refused(&case, status, &body);
                rows_driven += 1;
            }
        }
        assert!(
            rows_driven > 0,
            "case {:?}: at least one width must hand out a token to respell, or this listing \
             is asserted on nothing",
            listing.label()
        );
    }
}
