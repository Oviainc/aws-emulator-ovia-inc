// SPDX-License-Identifier: CC0-1.0

//! The page bounds of this service's listings, end to end over the restJson1
//! wire.
//!
//! RULE: a listing's page size is the OPERATION's own — `maxResults` when the
//! request names one, that operation's default page when it does not, and never
//! more than that operation's ceiling, whatever `maxResults` asked for. The
//! bounds come from the operation's own documentation where it states them, and
//! are the emulator's own only where it states none; they are therefore not
//! shared between the listings of one service.
//!
//! The table spans all three listings (whose bounds differ) against page sizes
//! either side of each listing's ceiling, plus the bounds the shared
//! `MaxResults` shape publishes (1..=1000) and a request that names none.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::ops::types::{
    Address, SupplementalTaxRegistrationEntry, TaxRegistrationEntry,
};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, PageBounds, StoredSupplementalRegistration,
    StoredTaxRegistration, SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS, TAX_EXEMPTIONS_PAGE_BOUNDS,
    TAX_REGISTRATIONS_PAGE_BOUNDS,
};

// One exemption per account in store order, as the exemptions suite already
// seeds it, so the stored record's shape cannot drift between the two suites.
use crate::tax_exemptions::seed_many as seed_exemptions;
use crate::tax_registration_lifecycle::{call, emulator_with_engine, Answer};

/// How a listing reports one page: the output member carrying it, and where an
/// entry's identity sits.
enum Page {
    /// A list member whose entries name themselves in the `id` member.
    List {
        member: &'static str,
        id: &'static str,
    },
    /// A map member keyed by the entry's identity.
    Map { member: &'static str },
}

/// One listing: the operation, the bounds it pages by, how it reports a page and
/// how a store of a given size is seeded for it.
struct Listing {
    operation: &'static str,
    bounds: PageBounds,
    page: Page,
    seed: fn(&Arc<engine::InMemoryEngine>, usize) -> Vec<String>,
}

/// Every listing this service serves, each with the bounds it pages by.
const LISTINGS: &[Listing] = &[
    Listing {
        operation: "ListTaxRegistrations",
        bounds: TAX_REGISTRATIONS_PAGE_BOUNDS,
        page: Page::List {
            member: "accountDetails",
            id: "accountId",
        },
        seed: seed_registrations,
    },
    Listing {
        operation: "ListTaxExemptions",
        bounds: TAX_EXEMPTIONS_PAGE_BOUNDS,
        page: Page::Map {
            member: "taxExemptionDetailsMap",
        },
        seed: seed_exemptions,
    },
    Listing {
        operation: "ListSupplementalTaxRegistrations",
        bounds: SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS,
        page: Page::List {
            member: "taxRegistrations",
            id: "authorityId",
        },
        seed: seed_supplemental_registrations,
    },
];

/// Record `count` tax registrations in the shared engine's store — the store the
/// handlers read — and answer their account ids in store order. Seeded as
/// records rather than through `PutTaxRegistration`, because these cases are
/// about what one read reports, not about the write that recorded it.
fn seed_registrations(engine: &Arc<engine::InMemoryEngine>, count: usize) -> Vec<String> {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    let mut ids = Vec::with_capacity(count);
    for index in 1..=count {
        let account_id = format!("{index:012}");
        let entry = TaxRegistrationEntry {
            registration_id: format!("VAT-{index:05}"),
            registration_type: "VAT".to_string(),
            legal_name: Some(format!("Account {index} Ltd")),
            legal_address: Some(Address {
                postal_code: "10115".to_string(),
                country_code: "DE".to_string(),
                ..Address::default()
            }),
            ..TaxRegistrationEntry::default()
        };
        store.registrations.insert(
            account_id.clone(),
            StoredTaxRegistration::from_entry(&entry, &account_id, 1, Instant::now()),
        );
        ids.push(account_id);
    }
    ids
}

/// Record `count` supplemental tax registrations, answering the `authorityId`s
/// the store minted for them in store order. See [`seed_registrations`] for why
/// the records are seeded directly.
fn seed_supplemental_registrations(
    engine: &Arc<engine::InMemoryEngine>,
    count: usize,
) -> Vec<String> {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    let mut ids = Vec::with_capacity(count);
    for index in 1..=count {
        let entry = SupplementalTaxRegistrationEntry {
            registration_id: format!("VAT-{index:05}"),
            registration_type: "VAT".to_string(),
            legal_name: format!("Account {index} AG"),
            address: Address {
                postal_code: "8001".to_string(),
                country_code: "CH".to_string(),
                ..Address::default()
            },
        };
        let authority_id = store.mint_supplemental_authority_id();
        store.supplemental_registrations.insert(
            authority_id.clone(),
            StoredSupplementalRegistration::from_entry(&entry, Instant::now()),
        );
        ids.push(authority_id);
    }
    ids
}

/// The identities one reported page carries: for a list member in the order the
/// response lists them, and for a map member sorted, because a map is unordered
/// on the wire — a page of one is compared as a set, and what pins the paging
/// order is that each page is the next ascending stretch of the store.
fn reported(answer: &Answer, page: &Page) -> Vec<String> {
    match page {
        Page::List { member, id } => answer
            .get(member)
            .as_array()
            .unwrap_or_else(|| panic!("{member} is required, got {:?}", answer.body))
            .iter()
            .map(|entry| {
                entry
                    .get(id)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
        Page::Map { member } => {
            let mut keys: Vec<String> = answer
                .get(member)
                .as_object()
                .unwrap_or_else(|| panic!("{member} is required, got {:?}", answer.body))
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        }
    }
}

/// The `maxResults` values a listing with this ceiling is asked for: none at all,
/// a small page, either side of its ceiling, and the top of the shared
/// `MaxResults` range (1..=1000) — anything outside that range is refused at the
/// boundary, so it is not a page size the server ever decides on.
fn requested_page_sizes(ceiling: usize) -> Vec<Option<i64>> {
    let mut sizes = vec![None, Some(1), Some(3)];
    for candidate in [ceiling as i64 - 1, ceiling as i64, ceiling as i64 + 1, 1_000] {
        if (1..=1_000).contains(&candidate) && !sizes.contains(&Some(candidate)) {
            sizes.push(Some(candidate));
        }
    }
    sizes
}

/// RULE: every listing fills its pages to the size ITS bounds decide —
/// `maxResults` when the request names one, its own default when it does not,
/// capped at its own ceiling — emits every stored item exactly once in store
/// order, and carries a cursor on every page but the last.
#[tokio::test]
async fn every_listing_pages_by_its_own_bounds() {
    for listing in LISTINGS {
        // More items than this listing's default page, so the default is
        // observable: a request naming no page size must still page.
        let store_size = listing.bounds.default_size + 7;
        for requested in requested_page_sizes(listing.bounds.max_size) {
            // A page size of 1 over a store this large would be hundreds of
            // round-trips for a bound two other rows already pin, so it is asked
            // against a small store instead.
            let store_size = if requested == Some(1) { 4 } else { store_size };
            let case = format!(
                "{} with maxResults {requested:?} over {store_size} items",
                listing.operation
            );
            let (router, engine) = emulator_with_engine();
            let expected = (listing.seed)(&engine, store_size);
            let bound = listing.bounds.page_size(
                requested.map(|size| i32::try_from(size).expect("the row is a wire-valid size")),
            );

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut body = serde_json::Map::new();
                if let Some(size) = requested {
                    body.insert("maxResults".to_string(), json!(size));
                }
                if let Some(cursor) = &token {
                    body.insert("nextToken".to_string(), json!(cursor));
                }
                let answer = call(&router, listing.operation, &Value::Object(body)).await;
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (200, ""),
                    "case {case:?}: page {pages} expected HTTP 200, got {answer:?}"
                );
                let entries = reported(&answer, &listing.page);
                let outstanding = expected.len() - seen.len();
                let expected_len = outstanding.min(bound);
                assert_eq!(
                    entries.len(),
                    expected_len,
                    "case {case:?}: page {pages} must hold {expected_len} entries (this \
                     operation pages by {bound}), got {}",
                    entries.len()
                );
                seen.extend(entries);
                pages += 1;
                token = answer
                    .get("nextToken")
                    .as_str()
                    .map(std::string::ToString::to_string);
                let more_remain = seen.len() < expected.len();
                assert_eq!(
                    token.is_some(),
                    more_remain,
                    "case {case:?}: page {pages} emitted {} of {} items, so a cursor must be \
                     {}; got {:?}",
                    seen.len(),
                    expected.len(),
                    if more_remain { "issued" } else { "withheld" },
                    answer.get("nextToken")
                );
                if token.is_none() {
                    break;
                }
            }

            assert_eq!(
                seen, expected,
                "case {case:?}: expected every item exactly once in store order, got {seen:?}"
            );
            let expected_pages = expected.len().div_ceil(bound).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: a page of {bound} walks {} items in {expected_pages} pages, \
                 took {pages}",
                expected.len()
            );
        }
    }
}

/// RULE: where an operation's own documentation states its page bounds, those
/// are the bounds it serves. `ListTaxRegistrations` states both —
/// `smithy_query(query="doc",shape="com.amazonaws.taxsettings#ListTaxRegistrations")`
/// answers "This can be used to retrieve up to 100 accounts' tax registrations
/// in one call (default 50)" — so a request naming no `maxResults` is served 50
/// entries and a cursor, and one naming more than 100 is served at most 100 and
/// a cursor, however large a `maxResults` the shared `MaxResults` range
/// (1..=1000) lets it name.
#[tokio::test]
async fn list_tax_registrations_serves_the_page_bounds_it_documents() {
    // The two figures the operation's documentation states.
    let documented_default = 50usize;
    let documented_ceiling = 100usize;

    let (router, engine) = emulator_with_engine();
    let stored = seed_registrations(&engine, 205);

    // A page size at the ceiling, either side of it, and the top of the wire
    // range: none of them raises the page past the documented ceiling.
    for requested in [None, Some(99), Some(100), Some(101), Some(205), Some(1_000)] {
        let case = format!("maxResults {requested:?} over {} accounts", stored.len());
        let mut body = serde_json::Map::new();
        if let Some(size) = requested {
            body.insert("maxResults".to_string(), json!(size));
        }
        let answer = call(&router, "ListTaxRegistrations", &Value::Object(body)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        let entries = reported(
            &answer,
            &Page::List {
                member: "accountDetails",
                id: "accountId",
            },
        );
        let expected_len = requested.map_or(documented_default, |size| {
            (size as usize).min(documented_ceiling)
        });
        assert_eq!(
            entries.len(),
            expected_len,
            "case {case:?}: expected a page of {expected_len} (default {documented_default}, \
             ceiling {documented_ceiling}), got {}",
            entries.len()
        );
        assert_eq!(
            &entries,
            &stored[..expected_len],
            "case {case:?}: expected the first {expected_len} accounts in store order, got \
             {entries:?}"
        );
        assert!(
            answer.get("nextToken").as_str().is_some(),
            "case {case:?}: {} accounts remain after the page, so a cursor is owed; got {:?}",
            stored.len() - expected_len,
            answer.body
        );
    }
}

/// RULE: the bounds belong to the operation, not to the service — a sibling
/// listing whose documentation states no page bound pages by its own default,
/// not by the one `ListTaxRegistrations` documents.
#[tokio::test]
async fn a_sibling_listing_keeps_its_own_default_page() {
    let store_size = TAX_EXEMPTIONS_PAGE_BOUNDS.default_size + 5;
    let (router, engine) = emulator_with_engine();
    let stored = seed_exemptions(&engine, store_size);

    let answer = call(&router, "ListTaxExemptions", &json!({})).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (200, ""),
        "case \"ListTaxExemptions, no maxResults\": expected HTTP 200, got {answer:?}"
    );
    let entries = reported(
        &answer,
        &Page::Map {
            member: "taxExemptionDetailsMap",
        },
    );
    assert_eq!(
        entries.len(),
        TAX_EXEMPTIONS_PAGE_BOUNDS.default_size,
        "case \"ListTaxExemptions, no maxResults\": expected this operation's own default page \
         of {} over {} accounts, got {}",
        TAX_EXEMPTIONS_PAGE_BOUNDS.default_size,
        stored.len(),
        entries.len()
    );
    assert_ne!(
        TAX_EXEMPTIONS_PAGE_BOUNDS.default_size, TAX_REGISTRATIONS_PAGE_BOUNDS.default_size,
        "the listings' defaults are each their own operation's, not one shared pair"
    );
}
