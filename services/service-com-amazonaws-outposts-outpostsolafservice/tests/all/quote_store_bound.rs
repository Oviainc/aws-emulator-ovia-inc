// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE rule `CreateQuote` carries that
//! [`crate::quote_lifecycle`] does not state: the store a created quote lands in is
//! BOUNDED, and crossing that bound is reported as a modelled refusal rather than as
//! an accepted-but-dropped create.
//!
//! A quote is never released — `GetQuote` must still answer what `CreateQuote`
//! created — so the only thing that keeps a caller repeating `CreateQuote` from
//! growing the store without limit is a maximum entry count. The rule, stated once:
//!
//! * Below the emulator's entry ceiling a create is ACCEPTED: it mints a `QuoteId`,
//!   persists the quote, and the quote reads back through `GetQuote`.
//! * At the ceiling the create is REFUSED with the error its model declares for a
//!   request this service will not accept — `CreateQuote` models no quota error, so
//!   that is 400 `ValidationException` — and nothing is stored.
//! * A refused create disturbs nothing: every quote already persisted still reads
//!   back exactly as its own create reported it.
//!
//! The ceiling is `MAX_RESOURCES` entries, far more than a test can fill one call at
//! a time, so each row seeds the store directly to one short of it and then drives
//! the boundary over the wire — the same way [`crate::capacity_task_lifecycle`]
//! states the bound on the capacity-task store.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::Quote;
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    store_full, timestamp, QuoteRecord, MAX_RESOURCES, QUOTE_STATUS_CREATED, QUOTE_VALIDITY,
};

use crate::quote_lifecycle::{create_quote, get_quote, request};
use crate::site_and_outpost_lifecycle::{is_minted_id, text, Outposts, HOME_REGION};

/// How this service words the refusal of a create that would take the quote store
/// past [`MAX_RESOURCES`].
///
/// The ceiling is the emulator's own bound and no AWS rule fixes its wording; what a
/// caller branches on is the modelled code and status, which the rows below assert
/// too. The wording is the one every capped path of this service states that ceiling
/// in, so the refusal names the emulator, the resource kind and the bound rather than
/// blaming the request ([`crate::store_ceiling_wording`] states that rule across the
/// service).
fn store_full_message() -> String {
    store_full("quotes")
}

/// Fill the quote store with `count` quotes, each under an id of the modelled
/// `^oq-[a-f0-9]{17}$` form that nothing reads, so a case can stand one call short of
/// the ceiling.
///
/// Each seeded quote is `CREATED` with an expiry a full validity window out, so none
/// of them is a transition the clock owes: what the rows observe is the bound and
/// nothing a settle pass did on the way.
fn seed_quotes(outposts: &Outposts, count: usize) {
    let created = std::time::SystemTime::now();
    let store = outposts.store();
    let mut store = store.lock();
    for index in 0..count {
        let quote_id = format!("oq-{index:017x}");
        store.quotes.insert(
            quote_id.clone(),
            QuoteRecord {
                sequence: index as u64,
                region: HOME_REGION.to_string(),
                quote: Quote {
                    quote_id: Some(quote_id),
                    quote_status: Some(QUOTE_STATUS_CREATED.to_string()),
                    country_code: Some("US".to_string()),
                    requested_capacities: Some(Vec::new()),
                    created_date: Some(timestamp(created)),
                    expiration_date: Some(timestamp(created + QUOTE_VALIDITY)),
                    ..Quote::default()
                },
            },
        );
    }
}

/// How many quotes the store holds right now.
fn held(outposts: &Outposts) -> usize {
    outposts.store().lock().quotes.len()
}

/// The rule, over a SET of create requests.
///
/// The rows differ along everything `CreateQuote` generalises over — how many
/// requested capacities and of which modelled types, whether the optional
/// constraints, payment options, payment terms and description are present, the
/// country code, and whether the quote is associated with an Outpost — so what
/// refuses the second call of each row can only be the bound, and not anything about
/// the request it refused.
#[tokio::test]
async fn the_quote_store_is_bounded_and_the_bound_is_a_modelled_refusal() {
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "required members only, empty capacity list",
            request("US", json!([]), json!({})),
            false,
        ),
        (
            "one EC2 capacity",
            request(
                "GB",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 4.0 }]),
                json!({}),
            ),
            false,
        ),
        (
            "every modelled capacity type, a constraint, every payment option and term",
            request(
                "DE",
                json!([
                    { "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 12.0 },
                    { "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 2.5 },
                    { "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 48.0 },
                ]),
                json!({
                    "RequestedConstraints": [{ "QuoteConstraintType": "RACK_MAXIMUM", "Value": "2" }],
                    "RequestedPaymentOptions": ["ALL_UPFRONT", "NO_UPFRONT", "PARTIAL_UPFRONT"],
                    "RequestedPaymentTerms": ["ONE_YEAR", "THREE_YEARS", "FIVE_YEARS"],
                    "Description": "at the ceiling",
                }),
            ),
            false,
        ),
        (
            "associated with an Outpost",
            request("JP", json!([]), json!({})),
            true,
        ),
    ];

    for (case, body, associate) in &cases {
        // A fresh emulator per row: the bound is a property of ONE store, so a row
        // must not inherit the entries another row left behind.
        let outposts = Outposts::new();
        let mut body = body.clone();
        if *associate {
            let (outpost_id, _) = outposts.site_and_outpost("bound-outpost").await;
            if let Some(target) = body.as_object_mut() {
                target.insert("OutpostIdentifier".to_string(), json!(outpost_id));
            }
        }
        seed_quotes(&outposts, MAX_RESOURCES - 1);

        // One short of the ceiling the create is accepted, persisted, and readable.
        let created = create_quote(&outposts, &body).await;
        let quote_id = text(&created, "QuoteId");
        assert!(
            is_minted_id(&quote_id, "oq"),
            "case {case:?}: below the ceiling a create mints a QuoteId, got {quote_id:?}"
        );
        let at_ceiling = held(&outposts);
        assert_eq!(
            at_ceiling, MAX_RESOURCES,
            "case {case:?}: the accepted create must be the entry that fills the store: \
             expected {MAX_RESOURCES} quotes held, got {at_ceiling}"
        );

        // At the ceiling the same create is refused, as the 400 `ValidationException`
        // this operation models for a request it will not accept.
        let reply = outposts.call("POST", "/quotes", Some(&body)).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: at the ceiling the create must be refused 400 ValidationException, \
             got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let message = text(&reply.body, "Message");
        let expected = store_full_message();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal message {expected:?}, got {:?}",
            reply.body
        );

        // The refusal stored nothing...
        let after = held(&outposts);
        assert_eq!(
            after, MAX_RESOURCES,
            "case {case:?}: a refused create must store nothing: expected {MAX_RESOURCES} \
             quotes held, got {after}"
        );
        // ...and disturbed nothing: the quote accepted just below the ceiling still
        // reads back exactly as its own create reported it.
        let read = get_quote(&outposts, &quote_id).await;
        assert_eq!(
            read, created,
            "case {case:?}: after the refusal GetQuote must still answer the quote the \
             accepted create reported"
        );
    }
}
