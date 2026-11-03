// SPDX-License-Identifier: CC0-1.0

//! The page-build rule every listing of this service shares, driven end to end
//! over the restJson1 wire.
//!
//! RULE: one page of a listing is the next ascending stretch of its ordered
//! store — the entries whose key is strictly greater than the key the request's
//! cursor names, at most the page size of them, in store order — and the cursor
//! the response carries names the LAST key of that page, issued only when a
//! further entry remains. Every listing builds its page that way, through the
//! single page-builder that lives beside the token codec in `state.rs`, so the
//! answers of all three move together.
//!
//! The table spans every listing this service serves — one pages a list of
//! account records, one a map keyed by account, one records whose keys the
//! server minted — against page sizes that divide the store and page sizes that
//! do not, and against cursors naming each stored key in turn plus a key below
//! every stored one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    encode_page_token, ComAmazonawsTaxsettingsTaxsettingsState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::supplemental_tax_registrations::{entry_for, put as put_supplemental};
use crate::tax_exemptions::seed_many as seed_exemptions;
use crate::tax_registration_lifecycle::{call, emulator_with_engine, Answer};

/// One listing of this service: which operation pages, how it reports a page,
/// and which of the store's ordered maps it pages over.
#[derive(Clone, Copy, Debug)]
enum Listing {
    /// Pages `registrations`, reporting a list of account records.
    TaxRegistrations,
    /// Pages `exemptions`, reporting a map keyed by account id.
    TaxExemptions,
    /// Pages `supplemental_registrations`, whose keys the server minted.
    SupplementalTaxRegistrations,
}

/// Every listing this service serves; each row drives the shared page-builder
/// through its own operation.
const LISTINGS: &[Listing] = &[
    Listing::TaxRegistrations,
    Listing::TaxExemptions,
    Listing::SupplementalTaxRegistrations,
];

impl Listing {
    /// The operation a row calls.
    const fn operation(self) -> &'static str {
        match self {
            Self::TaxRegistrations => "ListTaxRegistrations",
            Self::TaxExemptions => "ListTaxExemptions",
            Self::SupplementalTaxRegistrations => "ListSupplementalTaxRegistrations",
        }
    }

    /// A store key smaller than every key [`Self::seed`] records, which the
    /// listing's own token decoder admits: a cursor naming it must therefore
    /// skip nothing.
    const fn key_below_all(self) -> &'static str {
        match self {
            // Both of these page by 12-digit account id, and the seeded ids
            // start at one.
            Self::TaxRegistrations | Self::TaxExemptions => "000000000000",
            // `authorityId`s are minted from a counter that starts at one
            // (`mint_supplemental_authority_id`), so this is the id before the
            // first one minted.
            Self::SupplementalTaxRegistrations => "supplemental-trn-00000000",
        }
    }

    /// Record `count` entries for this listing to page over.
    async fn seed(
        self,
        router: &MultiServiceRouter,
        engine: &Arc<engine::InMemoryEngine>,
        count: usize,
    ) {
        match self {
            Self::TaxRegistrations => {
                let ids: Vec<String> = (1..=count).map(|index| format!("{index:012}")).collect();
                // The minimal entry the batch-write suite already stores;
                // nothing here depends on what it carries.
                let entry = crate::batch_tax_registration::entries()
                    .into_iter()
                    .next()
                    .expect("the shared entry table has a first row")
                    .1;
                let answer = call(
                    router,
                    "BatchPutTaxRegistration",
                    &json!({ "accountIds": ids, "taxRegistrationEntry": entry }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "seeding {count} registrations must succeed, got {answer:?}"
                );
            }
            Self::TaxExemptions => {
                seed_exemptions(engine, count);
            }
            Self::SupplementalTaxRegistrations => {
                // A supplemental registration supplements the account's own, so
                // the account holds a settled registration before these writes.
                crate::supplemental_tax_registrations::hold_a_settled_own_registration(engine);
                for index in 1..=count {
                    // A distinct TRN per record, so each is stored under its own
                    // minted `authorityId` instead of updating one record.
                    let entry = entry_for(
                        "CH",
                        &format!("CHE-{index:09}"),
                        &format!("Acme {index} AG"),
                    );
                    let answer = put_supplemental(router, &entry).await;
                    assert_eq!(
                        answer.status, 200,
                        "seeding supplemental registration {index} must succeed, got {answer:?}"
                    );
                }
            }
        }
    }

    /// The keys of the map this listing pages, in store order — the ground truth
    /// a page is compared against, read from the store rather than from the
    /// listing under test.
    fn stored_keys(self, engine: &Arc<engine::InMemoryEngine>) -> Vec<String> {
        let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
        let store = state.lock();
        match self {
            Self::TaxRegistrations => store.registrations.keys().cloned().collect(),
            Self::TaxExemptions => store.exemptions.keys().cloned().collect(),
            Self::SupplementalTaxRegistrations => {
                store.supplemental_registrations.keys().cloned().collect()
            }
        }
    }

    /// The store keys one reported page carries. A list member is read in the
    /// order the response lists them; a map member is sorted, because a map is
    /// unordered on the wire and the store's own order is ascending by key, so
    /// the sorted keys are the stretch the page emitted.
    fn reported_keys(self, answer: &Answer) -> Vec<String> {
        let (member, id) = match self {
            Self::TaxRegistrations => ("accountDetails", "accountId"),
            Self::SupplementalTaxRegistrations => ("taxRegistrations", "authorityId"),
            Self::TaxExemptions => {
                let mut keys: Vec<String> = answer
                    .get("taxExemptionDetailsMap")
                    .as_object()
                    .unwrap_or_else(|| {
                        panic!(
                            "taxExemptionDetailsMap is required, got {:?}",
                            answer.body
                        )
                    })
                    .keys()
                    .cloned()
                    .collect();
                keys.sort();
                return keys;
            }
        };
        answer
            .get(member)
            .as_array()
            .unwrap_or_else(|| panic!("{member} is required, got {:?}", answer.body))
            .iter()
            .map(|entry| {
                entry
                    .get(id)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("{member} entries carry {id}, got {entry:?}"))
                    .to_string()
            })
            .collect()
    }
}

/// Ask one listing for a single page, optionally from a cursor.
async fn one_page(
    router: &MultiServiceRouter,
    listing: Listing,
    page_size: Option<usize>,
    cursor: Option<&str>,
    case: &str,
) -> Answer {
    let mut body = serde_json::Map::new();
    if let Some(size) = page_size {
        body.insert("maxResults".to_string(), json!(size));
    }
    if let Some(token) = cursor {
        body.insert("nextToken".to_string(), json!(token));
    }
    let answer = call(router, listing.operation(), &Value::Object(body)).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (200, ""),
        "case {case:?}: expected HTTP 200, got {answer:?}"
    );
    answer
}

/// RULE: every listing walks its store as the next ascending stretch per page,
/// each page holding the requested size of entries while entries remain, and
/// carries a cursor naming the last key of the page on every page but the last.
#[tokio::test]
async fn every_listing_pages_through_the_shared_builder() {
    // Page sizes that divide the store, that do not, that equal it and that
    // exceed it, plus a request naming none (served the operation's own default,
    // which is larger than this store, so it is the whole store in one page).
    let page_sizes: &[Option<usize>] = &[Some(1), Some(2), Some(3), Some(5), Some(9), None];
    let store_size = 5usize;

    for &listing in LISTINGS {
        for &requested in page_sizes {
            let case = format!(
                "{} with maxResults {requested:?} over {store_size} entries",
                listing.operation()
            );
            let (router, engine) = emulator_with_engine();
            listing.seed(&router, &engine, store_size).await;
            let stored = listing.stored_keys(&engine);
            assert_eq!(
                stored.len(),
                store_size,
                "case {case:?}: expected {store_size} seeded entries, got {stored:?}"
            );

            let mut emitted: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let answer =
                    one_page(&router, listing, requested, cursor.as_deref(), &case).await;
                let keys = listing.reported_keys(&answer);
                let outstanding = stored.len() - emitted.len();
                // A request naming no page size is served the operation's own
                // default, which no row here exceeds.
                let expected_len = outstanding.min(requested.unwrap_or(store_size));
                assert_eq!(
                    keys.len(),
                    expected_len,
                    "case {case:?}: page {pages} must hold {expected_len} entries, got {keys:?}"
                );
                assert_eq!(
                    keys,
                    stored[emitted.len()..emitted.len() + expected_len].to_vec(),
                    "case {case:?}: page {pages} must be the next ascending stretch of the \
                     store, got {keys:?}"
                );

                emitted.extend(keys.iter().cloned());
                pages += 1;
                let reported_cursor = answer
                    .get("nextToken")
                    .as_str()
                    .map(std::string::ToString::to_string);
                let expected_cursor = if emitted.len() < stored.len() {
                    keys.last().map(|key| encode_page_token(key))
                } else {
                    None
                };
                assert_eq!(
                    reported_cursor, expected_cursor,
                    "case {case:?}: page {pages} must carry {expected_cursor:?} (the cursor \
                     names the last key of the page, and only while entries remain), got \
                     {reported_cursor:?}"
                );
                cursor = reported_cursor;
                if cursor.is_none() {
                    break;
                }
            }

            assert_eq!(
                emitted, stored,
                "case {case:?}: expected every stored entry exactly once in store order, got \
                 {emitted:?}"
            );
            let expected_pages = stored
                .len()
                .div_ceil(requested.unwrap_or(store_size))
                .max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages, took {pages}"
            );
        }
    }
}

/// RULE: a cursor names the key a page stopped after, so the page it resumes is
/// the entries whose key is strictly greater than it — the cursor of the
/// greatest key resumes an empty page with no further cursor, and a cursor
/// naming a key below every stored one skips nothing.
#[tokio::test]
async fn a_cursor_resumes_at_the_first_key_after_it() {
    let store_size = 4usize;
    // One page size smaller than the store and one larger, so the resumed page
    // is bounded by the page size in some rows and by the store in others.
    let page_sizes: &[Option<usize>] = &[Some(2), Some(9)];

    for &listing in LISTINGS {
        let (router, engine) = emulator_with_engine();
        listing.seed(&router, &engine, store_size).await;
        let stored = listing.stored_keys(&engine);
        assert_eq!(
            stored.len(),
            store_size,
            "{}: expected {store_size} seeded entries, got {stored:?}",
            listing.operation()
        );

        // Every stored key as a cursor, plus a key below all of them: the keys
        // expected after each are the stretch strictly greater than it.
        let mut cursor_keys: Vec<String> = vec![listing.key_below_all().to_string()];
        cursor_keys.extend(stored.iter().cloned());

        for cursor_key in &cursor_keys {
            for &requested in page_sizes {
                let case = format!(
                    "{} resuming after {cursor_key:?} with maxResults {requested:?}",
                    listing.operation()
                );
                let after: Vec<String> = stored
                    .iter()
                    .filter(|key| *key > cursor_key)
                    .cloned()
                    .collect();
                let expected: Vec<String> = after
                    .iter()
                    .take(requested.unwrap_or(store_size))
                    .cloned()
                    .collect();

                let token = encode_page_token(cursor_key);
                let answer =
                    one_page(&router, listing, requested, Some(&token), &case).await;
                let keys = listing.reported_keys(&answer);
                assert_eq!(
                    keys, expected,
                    "case {case:?}: expected the keys strictly greater than the cursor, got \
                     {keys:?}"
                );
                let expected_cursor = if expected.len() < after.len() {
                    expected.last().map(|key| encode_page_token(key))
                } else {
                    None
                };
                let reported_cursor = answer
                    .get("nextToken")
                    .as_str()
                    .map(std::string::ToString::to_string);
                assert_eq!(
                    reported_cursor, expected_cursor,
                    "case {case:?}: expected {expected_cursor:?}, got {reported_cursor:?}"
                );
            }
        }
    }
}
